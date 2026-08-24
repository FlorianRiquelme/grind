//! Scripted-SSE fixture for the native adapter (epic #135 wave 3).
//!
//! A hand-rolled `std::net` HTTP/1.1 server (the serve.rs/ADR-0014 precedent, no new
//! deps) scripts canned `/chat/completions` SSE responses in order and captures every
//! request body, while the REAL library path — `runner::NativeAdapter` via
//! `StageRunner::run` — drives full conversations against it. Each scenario asserts
//! Attempt-level outcomes and, where counterparts exist, the classification fields the
//! `tests/fakes/shapes/*.sh` claude-code fixtures produce for equivalent shapes:
//!
//! | scenario | fake-shape counterpart |
//! |---|---|
//! | happy_native_tools | success_done.sh (`is_error:false` + done promise) |
//! | abnormal_native_finish | subtle_error.sh family (`is_error:true`, exit 1, not rate-limited) |
//! | empty_response_fails_loudly | — (R6: never a clean stop) |
//! | text_protocol_with_nudge | success_done.sh, text-wire equivalent |
//! | http_429_is_rate_limited | rate_limited.sh (`rate_limited:true`) |
//!
//! Library-level parity suffices here; the full-binary native e2e stays out of scope
//! this wave.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once};

use grind::attempt::{self, Attempt, Mode};
use grind::claude;
use grind::job::Job;
use grind::rung;
use grind::runner::{self, NativeAdapter, StageRunner};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// The scripted server: one TcpListener, sequential accepts, N canned replies.
// ---------------------------------------------------------------------------

/// One scripted reply. `Sse` answers 200 with an `text/event-stream` body;
/// `Status` answers with a bare status + plain-text body (429 et al).
enum Reply {
    Sse(String),
    Status(u16, &'static str, String),
}

/// A running server: its port plus every request body it received, in order.
struct Server {
    port: u16,
    bodies: Arc<Mutex<Vec<String>>>,
}

impl Server {
    /// Bind 127.0.0.1:0 and serve `replies` in order, one per connection, then stop.
    fn start(replies: Vec<Reply>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the scripted SSE server");
        let port = listener.local_addr().expect("a bound port").port();
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&bodies);
        std::thread::spawn(move || {
            for reply in replies {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let Some(body) = read_request_body(&mut stream) else {
                    break;
                };
                captured.lock().expect("request log lock").push(body);
                let bytes = match &reply {
                    Reply::Sse(sse) => http_bytes(200, "OK", sse, "text/event-stream"),
                    Reply::Status(code, reason, text) => {
                        http_bytes(*code, reason, text, "text/plain")
                    }
                };
                let _ = stream.write_all(&bytes);
                let _ = stream.flush();
                // Connection: close per reply — the next request opens a fresh
                // connection, so the sequential accept loop stays trivially ordered.
            }
            // Script exhausted (or a peer hung up): stop accepting. Any further
            // request fails at connect time, which the retry budget reports loudly.
        });
        Server { port, bodies }
    }

    fn bodies(&self) -> Vec<String> {
        self.bodies.lock().expect("request log lock").clone()
    }
}

/// Read one POST: headers through the blank line, then exactly Content-Length bytes.
fn read_request_body(stream: &mut TcpStream) -> Option<String> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    let mut content_length = 0usize;
    loop {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = value.trim().parse().ok()?;
        }
    }
    let mut body = vec![0u8; content_length];
    if !body.is_empty() {
        reader.read_exact(&mut body).ok()?;
    }
    Some(String::from_utf8_lossy(&body).into_owned())
}

fn http_bytes(status: u16, reason: &str, body: &str, content_type: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

// ---------------------------------------------------------------------------
// SSE script builders — literal data:/comment/[DONE] lines per scenario.
// ---------------------------------------------------------------------------

/// One chat-completion streaming chunk.
fn chunk(delta: Value, finish: Option<&str>, native_finish: Option<&str>) -> Value {
    json!({
        "id": "chatcmpl-fixture",
        "object": "chat.completion.chunk",
        "choices": [{
            "delta": delta,
            "finish_reason": finish,
            "native_finish_reason": native_finish,
        }]
    })
}

/// One tool_call delta fragment: later chunks append to name/arguments by index.
fn tool_delta(index: u64, id: Option<&str>, name: Option<&str>, arguments: Option<&str>) -> Value {
    json!({
        "index": index,
        "id": id,
        "function": {"name": name, "arguments": arguments},
    })
}

/// Join chunks into a scripted stream body: `data: …\n\n` lines plus `[DONE]`.
fn sse(chunks: Vec<Value>) -> String {
    let mut body = String::new();
    for chunk in chunks {
        body.push_str(&format!("data: {chunk}\n\n"));
    }
    body.push_str("data: [DONE]\n\n");
    body
}

/// A keepalive-only stream: comment line, `[DONE]`, nothing ever said.
fn sse_empty() -> String {
    ": keepalive\n\ndata: [DONE]\n\n".to_string()
}

// ---------------------------------------------------------------------------
// Driving the real library path, mirroring supervisor.rs's construction.
// ---------------------------------------------------------------------------

/// `Endpoint::resolve` demands a key from the environment; the fixture supplies a
/// dummy once, idempotently, so hermetic runs never depend on host credentials.
fn ensure_api_key() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        if std::env::var("OPENROUTER_API_KEY").is_err() && std::env::var("OPENAI_API_KEY").is_err()
        {
            unsafe { std::env::set_var("OPENROUTER_API_KEY", "fixture-key") };
        }
    });
}

/// A temp scratch pair: `run_dir` for transcripts, `cwd` as the tool workdir.
fn scratch(name: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("grind-sse-native-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let run_dir = root.join("run");
    let cwd = root.join("worktree");
    std::fs::create_dir_all(&run_dir).expect("run dir");
    std::fs::create_dir_all(&cwd).expect("workdir");
    (run_dir, cwd)
}

/// One attempt through the real seam, built exactly the way supervisor.rs :1319-1340
/// builds a stage attempt: StageConditions + StageContext → claude::stage_invocation,
/// denied globs from attempt::denied_for(stage), a literal hand-built Job (fields pub).
fn drive_attempt(server: &Server, run_dir: &Path, cwd: &Path, attempt_n: usize) -> Attempt {
    ensure_api_key();

    let job = Job {
        issue: 135,
        url: "https://github.com/example/grind/issues/135".to_string(),
        title: "agent harness adapters".to_string(),
        labels: Vec::new(),
        target_repo: "example/grind".to_string(),
        branch: "feat/135-agent-adapters".to_string(),
        handoff_sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
        anchor: "Ship the harness behind one seam.".to_string(),
        intent: None,
        model: None,
        done_predicate: "PR is open".to_string(),
        base_branch: "main".to_string(),
        verify_entrypoint: "cargo test".to_string(),
        declared_hot_paths: Vec::new(),
    };

    let conditions = attempt::StageConditions {
        claude_bin: "claude",
        run_id: "sse-native-fixture",
    };
    let stages_dir = run_dir.join("stages").display().to_string();
    let worktree = cwd.display().to_string();
    let ctx = attempt::StageContext {
        stage: rung::Stage::Work,
        skill_text: "Do the assigned stage work.",
        stages_dir: &stages_dir,
        worktree: &worktree,
        job: &job,
        model: None,
        notes: None,
    };
    let invocation = claude::stage_invocation(&conditions, &ctx, Mode::Dispatch, None);

    let denied_globs = attempt::denied_for(rung::Stage::Work);
    let model = runner::StageModel::Class(runner::ModelClass::Strong);
    let spec = runner::RunSpec {
        invocation: &invocation,
        cwd,
        run_dir,
        attempt_n,
        session_id: "",
        worktree: &worktree,
        model: &model,
        denied_globs: &denied_globs,
        file_label: runner::FileLabel::Attempt,
    };

    NativeAdapter {
        endpoint_override: Some(format!("http://127.0.0.1:{}", server.port)),
        fast_model: None,
        strong_model: None,
        proto_override: None,
    }
    .run(&spec)
}

/// Parse one attempt's `messages-N.jsonl` into `(event, value)` pairs.
fn transcript_events(run_dir: &Path, attempt_n: usize) -> Vec<(String, Value)> {
    let raw = std::fs::read_to_string(run_dir.join(format!("messages-{attempt_n}.jsonl")))
        .unwrap_or_default();
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let value: Value = serde_json::from_str(line).expect("a transcript JSONL line");
            (
                value["event"].as_str().expect("an event name").to_string(),
                value["value"].clone(),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Scenario a — happy native-tools path ≙ fakes/shapes/success_done.sh
// ---------------------------------------------------------------------------

#[test]
fn happy_native_tools_completes_with_usage_and_transcript() {
    let (run_dir, cwd) = scratch("happy");
    std::fs::write(cwd.join("note.txt"), "the quick brown fixture\n")
        .expect("seed the workdir file");

    // Chunked tool-call deltas, deliberately hostile: index 1 arrives FIRST
    // (out of order), call_a's NAME splits across two chunks, its ARGUMENTS across
    // three. Assembly must reorder by index and concatenate the pieces.
    let script = vec![
        Reply::Sse(sse(vec![
            chunk(
                json!({"tool_calls": [tool_delta(
                    1, Some("call_b"), Some("read_file"), Some("{\"path\": \"note.txt\"}"),
                )]}),
                None,
                None,
            ),
            chunk(
                json!({"tool_calls": [tool_delta(0, Some("call_a"), Some("read_"), None)]}),
                None,
                None,
            ),
            chunk(
                json!({"tool_calls": [tool_delta(0, None, Some("file"), Some("{\"path\":"))]}),
                None,
                None,
            ),
            chunk(
                json!({"tool_calls": [tool_delta(0, None, None, Some(" \"note.txt\"}"))]}),
                None,
                None,
            ),
            json!({
                "choices": [{"delta": {}, "finish_reason": "tool_calls", "native_finish_reason": null}],
                "usage": {"prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18},
            }),
        ])),
        Reply::Sse(sse(vec![chunk(
            json!({"content": "All work complete."}),
            Some("stop"),
            None,
        )])),
    ];
    let server = Server::start(script);

    let attempt = drive_attempt(&server, &run_dir, &cwd, 1);

    // Attempt-level outcome ≙ success_done.sh's is_error:false + done promise.
    assert!(!attempt.is_error, "{:?}", attempt.terminal_reason);
    assert!(attempt.done_promise);
    assert_eq!(attempt.exit_code, Some(0));
    assert!(!attempt.rate_limited);
    assert!(attempt.parse_ok);
    assert_eq!(attempt.subtype, None);
    assert_eq!(attempt.result_tail, "All work complete.");
    assert_eq!(attempt.num_turns, Some(2));
    // R8: the provider's usage rides through verbatim.
    assert_eq!(
        attempt.usage,
        Some(json!({"prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18}))
    );

    // R4: the grind-owned transcript carries the whole conversation.
    let events = transcript_events(&run_dir, 1);
    let names: Vec<&str> = events.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names[0], "protocol_selected", "{names:?}");
    assert_eq!(events[0].1["mode"], "native");
    for wanted in ["assistant_tool_calls", "tool_result", "usage", "final"] {
        assert!(names.contains(&wanted), "{names:?} lacks {wanted}");
    }

    let (_, calls_value) = events
        .iter()
        .find(|(name, _)| name == "assistant_tool_calls")
        .expect("AssistantToolCalls logged");
    let calls = calls_value["calls"].as_array().expect("calls array");
    assert_eq!(
        calls.len(),
        2,
        "out-of-order indices both assembled: {calls:?}"
    );
    assert_eq!(calls[0]["name"], "read_file", "split name reassembled");
    assert_eq!(calls[0]["arguments"], "{\"path\": \"note.txt\"}");
    assert_eq!(calls[1]["name"], "read_file");

    let (_, tool_result) = events
        .iter()
        .find(|(name, _)| name == "tool_result")
        .expect("ToolResult logged");
    assert_eq!(tool_result["call_id"], "call_a");
    assert!(
        tool_result["output"]
            .as_str()
            .expect("string output")
            .contains("the quick brown fixture"),
        "executed read_file fed the file back: {tool_result}"
    );
}

// ---------------------------------------------------------------------------
// Scenario b — abnormal native_finish_reason ≙ fakes/shapes/subtle_error.sh family
// ---------------------------------------------------------------------------

#[test]
fn abnormal_native_finish_fails_the_attempt_without_rate_limit() {
    let (run_dir, cwd) = scratch("abnormal");

    // OpenRouter's masked-failure shape: finish_reason "stop" hiding the real
    // cause in native_finish_reason, with an empty delta.
    let abnormal = || {
        Reply::Sse(sse(vec![chunk(
            json!({}),
            Some("stop"),
            Some("network_error"),
        )]))
    };
    // One Native probe (which latches Text immediately — an abnormal finish while
    // tools were sent counts as a tools-array rejection), then the fresh Text-mode
    // budget burns: backoff 2s + 4s, then give up. Four replies cover it.
    let server = Server::start(vec![abnormal(), abnormal(), abnormal(), abnormal()]);

    let attempt = drive_attempt(&server, &run_dir, &cwd, 1);

    // ≙ subtle_error.sh: is_error:true, exit 1 — the subtle shape classified loudly,
    // NOT as a rate limit and never as a clean stop (R6).
    assert!(attempt.is_error);
    assert_eq!(attempt.exit_code, Some(1));
    assert!(!attempt.done_promise);
    assert!(!attempt.rate_limited);
    assert!(attempt.parse_ok);
    let reason = attempt.terminal_reason.as_deref().unwrap_or("");
    assert!(
        reason.contains("abnormally") && reason.contains("network_error"),
        "terminal reason carries the wire failure: {reason}"
    );
    assert_eq!(attempt.result_tail, reason);
}

// ---------------------------------------------------------------------------
// Scenario c — empty response => failed attempt (R6)
// ---------------------------------------------------------------------------

#[test]
fn empty_response_fails_loudly_after_retry_budget() {
    let (run_dir, cwd) = scratch("empty");

    // Three replies: the retry budget spends itself (backoff 2s + 4s) and gives up.
    let server = Server::start(vec![
        Reply::Sse(sse_empty()),
        Reply::Sse(sse_empty()),
        Reply::Sse(sse_empty()),
    ]);

    let attempt = drive_attempt(&server, &run_dir, &cwd, 1);

    assert!(attempt.is_error);
    assert!(!attempt.done_promise);
    assert!(!attempt.rate_limited);
    let reason = attempt.terminal_reason.as_deref().unwrap_or("");
    assert!(
        reason.contains("empty response"),
        "the empty guard speaks for itself: {reason}"
    );
}

// ---------------------------------------------------------------------------
// Scenario d — text protocol with one prose nudge, then <done>
// ---------------------------------------------------------------------------

#[test]
fn text_protocol_nudges_prose_then_completes_on_done_sentinel() {
    let (run_dir, cwd) = scratch("text-nudge");
    std::fs::write(cwd.join("brief.txt"), "brief body line\n").expect("seed brief.txt");

    // Seed the per-run latch (R5) the honest way: a prior attempt's transcript
    // carrying ProtocolSelected{text}. This attempt (n=2) must honor it and send
    // NO tools array from the very first request.
    std::fs::write(
        run_dir.join("messages-1.jsonl"),
        "{\"event\":\"protocol_selected\",\"value\":{\"mode\":\"text\",\"reason\":\"seeded by the fixture\"}}\n",
    )
    .expect("seed the latch transcript");

    let prose = "Happy to help with that right away.";
    let server = Server::start(vec![
        // Turn 1: prose where a tag was demanded — the nudge case.
        Reply::Sse(sse(vec![chunk(
            json!({"content": prose}),
            Some("stop"),
            None,
        )])),
        // Turn 2: exactly one <tool> tag.
        Reply::Sse(sse(vec![chunk(
            json!({"content": "<tool>{\"name\": \"read_file\", \"arguments\": {\"path\": \"brief.txt\"}}</tool>"}),
            Some("stop"),
            None,
        )])),
        // Turn 3: the only legal text-mode termination.
        Reply::Sse(sse(vec![chunk(
            json!({"content": "<done>The brief is written and verified.</done>"}),
            Some("stop"),
            None,
        )])),
    ]);

    let attempt = drive_attempt(&server, &run_dir, &cwd, 2);

    assert!(!attempt.is_error, "{:?}", attempt.terminal_reason);
    assert!(attempt.done_promise);
    assert_eq!(attempt.exit_code, Some(0));
    assert!(!attempt.rate_limited);

    let events = transcript_events(&run_dir, 2);
    let names: Vec<&str> = events.iter().map(|(name, _)| name.as_str()).collect();

    // Resumed latch logged first…
    assert_eq!(names[0], "protocol_selected", "{names:?}");
    assert_eq!(events[0].1["mode"], "text");
    // …then exactly one nudge, carrying the offending prose…
    let nudges: Vec<&Value> = events
        .iter()
        .filter(|(name, _)| name == "protocol_nudge")
        .map(|(_, value)| value)
        .collect();
    assert_eq!(
        nudges.len(),
        1,
        "one corrective nudge per occurrence: {names:?}"
    );
    assert_eq!(nudges[0]["assistant_text"], prose);
    // …and the sentinel's inner text is the Final.
    let (_, final_value) = events
        .iter()
        .find(|(name, _)| name == "final")
        .expect("Final logged");
    assert_eq!(final_value["text"], "The brief is written and verified.");
    assert_eq!(attempt.result_tail, "The brief is written and verified.");

    // Server-side: the corrective exchange quotes the prose back, and Text mode
    // omits the tools parameter entirely (ADR-0018).
    let bodies = server.bodies();
    assert_eq!(bodies.len(), 3, "three scripted turns: {}", bodies.len());
    assert!(
        !bodies[0].contains("\"tools\""),
        "text mode never sends a tools array"
    );
    assert!(
        bodies[0].contains("Do the assigned stage work."),
        "the user prompt opens the run"
    );
    assert!(
        bodies[1].contains(prose),
        "the nudge exchange echoes the prose back to the model"
    );
    assert!(
        bodies[1].contains("</tool>"),
        "the corrective message restates the protocol"
    );
}

// ---------------------------------------------------------------------------
// Scenario e — HTTP 429 with a limit needle ≙ fakes/shapes/rate_limited.sh
// ---------------------------------------------------------------------------

#[test]
fn http_429_classifies_as_rate_limited() {
    let (run_dir, cwd) = scratch("rate-limited");

    // Three replies: the shared classifier's needles are asked only over the
    // final synthesized error, so the budget burns first (backoff 2s + 4s).
    let server = Server::start(vec![
        Reply::Status(
            429,
            "Too Many Requests",
            "You've hit your rate limit · resets soon".into(),
        ),
        Reply::Status(
            429,
            "Too Many Requests",
            "You've hit your rate limit · resets soon".into(),
        ),
        Reply::Status(
            429,
            "Too Many Requests",
            "You've hit your rate limit · resets soon".into(),
        ),
    ]);

    let attempt = drive_attempt(&server, &run_dir, &cwd, 1);

    // ≙ d_rate_limit_records_sleep / Run 2: the attempt is loud, errored, and —
    // via attempt::is_rate_limited over the carried error text — rate-limited,
    // so the supervisor's Wait machinery decides what happens next.
    assert!(attempt.is_error);
    assert!(!attempt.done_promise);
    assert!(attempt.rate_limited, "policy parity with claude::classify");
    let reason = attempt.terminal_reason.as_deref().unwrap_or("");
    assert!(
        reason.contains("429"),
        "the status survives into the record: {reason}"
    );
}
