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
//! | malformed_tool_call_nudges_then_completes | — (#142: fault check runs before the
//!   gate and before execution; the malformed call is nudged, never denied, never run) |
//! | frontmatter_prompt_declares_its_skill | — (the writer emits what native.rs's
//!   readers parse; positional assertions stop encoding a shape no real Run produces) |
//! | a_completed_stage_without_the_sentinel_promises_nothing | — (issue #139: an ending
//!   is never synthesized into a Run-level promise) |
//! | a_declared_max_turns_ceiling_bounds_the_loop_below_the_fallback | — (issue #157:
//!   the enforced turn limit is data served from docs/tiers.toml, not the constant) |
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

/// The compiled fallback the loop enforces when nothing is declared (`native.rs`'s
/// `DEFAULT_MAX_TURNS`); the exhaustion scenario counts against it.
const DEFAULT_FALLBACK_TURNS: usize = 32;

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
            }
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
    drive_attempt_with_prompt(
        server,
        run_dir,
        cwd,
        attempt_n,
        "Do the assigned stage work.",
        None,
    )
}

/// The same seam with a caller-supplied stage prompt and the adapter's per-stage turn
/// ceiling (`None` keeps the loop's compiled fallback). `stage_invocation` composes the
/// skill file verbatim ahead of the context block, so a prompt carrying real frontmatter
/// is how `declared_skill` gets exercised end to end; the ceiling is the knob issue #157
/// put on `NativeAdapter`, driven end to end by the exhaustion scenario below.
fn drive_attempt_with_prompt(
    server: &Server,
    run_dir: &Path,
    cwd: &Path,
    attempt_n: usize,
    skill_text: &str,
    max_turns: Option<usize>,
) -> Attempt {
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
        agent: None,
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
        skill_text,
        stages_dir: &stages_dir,
        worktree: &worktree,
        job: &job,
        model: None,
        notes: None,
        landed: None,
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
        routes: runner::ClassRoutes::default(),
        max_turns,
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

#[test]
fn happy_native_tools_completes_with_usage_and_transcript() {
    let (run_dir, cwd) = scratch("happy");
    std::fs::write(cwd.join("note.txt"), "the quick brown fixture\n")
        .expect("seed the workdir file");

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
            json!({"content": "All work complete. <promise>DONE</promise>"}),
            Some("stop"),
            None,
        )])),
    ];
    let server = Server::start(script);

    let attempt = drive_attempt(&server, &run_dir, &cwd, 1);

    assert!(!attempt.is_error, "{:?}", attempt.terminal_reason);
    assert!(attempt.done_promise);
    assert_eq!(attempt.exit_code, Some(0));
    assert!(!attempt.rate_limited);
    assert!(attempt.parse_ok);
    assert_eq!(attempt.subtype, None);
    assert_eq!(
        attempt.result_tail,
        "All work complete. <promise>DONE</promise>"
    );
    assert_eq!(attempt.num_turns, Some(2));
    assert_eq!(
        attempt.usage,
        Some(json!({"prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18}))
    );

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

#[test]
fn abnormal_native_finish_fails_the_attempt_without_rate_limit() {
    let (run_dir, cwd) = scratch("abnormal");

    let abnormal = || {
        Reply::Sse(sse(vec![chunk(
            json!({}),
            Some("stop"),
            Some("network_error"),
        )]))
    };
    let server = Server::start(vec![abnormal(), abnormal(), abnormal(), abnormal()]);

    let attempt = drive_attempt(&server, &run_dir, &cwd, 1);

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

#[test]
fn empty_response_fails_loudly_after_retry_budget() {
    let (run_dir, cwd) = scratch("empty");

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

#[test]
fn text_protocol_nudges_prose_then_completes_on_done_sentinel() {
    let (run_dir, cwd) = scratch("text-nudge");
    std::fs::write(cwd.join("brief.txt"), "brief body line\n").expect("seed brief.txt");

    std::fs::write(
        run_dir.join("messages-1.jsonl"),
        "{\"event\":\"protocol_selected\",\"value\":{\"mode\":\"text\",\"reason\":\"seeded by the fixture\"}}\n",
    )
    .expect("seed the latch transcript");

    let prose = "Happy to help with that right away.";
    let server = Server::start(vec![
        Reply::Sse(sse(vec![chunk(
            json!({"content": prose}),
            Some("stop"),
            None,
        )])),
        Reply::Sse(sse(vec![chunk(
            json!({"content": "<tool>{\"name\": \"read_file\", \"arguments\": {\"path\": \"brief.txt\"}}</tool>"}),
            Some("stop"),
            None,
        )])),
        Reply::Sse(sse(vec![chunk(
            json!({"content": "<done>The brief is written and verified. <promise>DONE</promise></done>"}),
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

    assert_eq!(names[0], "protocol_selected", "{names:?}");
    assert_eq!(events[0].1["mode"], "text");
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
    let (_, final_value) = events
        .iter()
        .find(|(name, _)| name == "final")
        .expect("Final logged");
    assert_eq!(
        final_value["text"],
        "The brief is written and verified. <promise>DONE</promise>"
    );
    assert_eq!(
        attempt.result_tail,
        "The brief is written and verified. <promise>DONE</promise>"
    );

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

#[test]
fn http_429_classifies_as_rate_limited() {
    let (run_dir, cwd) = scratch("rate-limited");

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

    assert!(attempt.is_error);
    assert!(!attempt.done_promise);
    assert!(attempt.rate_limited, "policy parity with claude::classify");
    let reason = attempt.terminal_reason.as_deref().unwrap_or("");
    assert!(
        reason.contains("429"),
        "the status survives into the record: {reason}"
    );
}

#[test]
fn a_completed_stage_without_the_sentinel_promises_nothing() {
    let (run_dir, cwd) = scratch("no-promise");
    let server = Server::start(vec![Reply::Sse(sse(vec![chunk(
        json!({"content": "Stage artifacts are on disk; the ladder may advance."}),
        Some("stop"),
        None,
    )]))]);

    let attempt = drive_attempt(&server, &run_dir, &cwd, 1);

    assert!(!attempt.is_error, "{:?}", attempt.terminal_reason);
    assert!(
        !attempt.done_promise,
        "a Run-level promise must be spoken by the agent, never inferred from an ending"
    );
    assert_eq!(attempt.exit_code, Some(0));
    assert_eq!(
        attempt.result_tail,
        "Stage artifacts are on disk; the ladder may advance."
    );
}

/// A malformed tool call — arguments that are not a JSON object — arrives while a denied
/// glob would have matched had the call reached policy: the fault check must run *before*
/// the gate and before execution (#142), so the reply earns a protocol_nudge carrying what
/// was wrong, no tool_result is ever logged for it, and nothing is denied. The corrected
/// re-issue on the next turn runs and completes the stage.
///
/// This is also the only end-to-end carrier for the malformed branch itself: every
/// `protocol_fault` unit test calls the function directly, so the loop's ordering —
/// nudge instead of deny, never execute — lives here alone.
#[test]
fn malformed_tool_call_nudges_then_completes() {
    let (run_dir, cwd) = scratch("malformed-nudge");
    std::fs::write(cwd.join("note.txt"), "the quick brown fixture\n").expect("seed note.txt");

    let server = Server::start(vec![
        Reply::Sse(sse(vec![chunk(
            json!({"tool_calls": [tool_delta(
                0, Some("call_bad"), Some("bash"), Some("not json"),
            )]}),
            Some("tool_calls"),
            None,
        )])),
        Reply::Sse(sse(vec![
            chunk(
                json!({"tool_calls": [tool_delta(
                    0, Some("call_ok"), Some("read_file"), Some("{\"path\": \"note.txt\"}"),
                )]}),
                None,
                None,
            ),
            json!({
                "choices": [{"delta": {}, "finish_reason": "tool_calls", "native_finish_reason": null}],
                "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8},
            }),
        ])),
        Reply::Sse(sse(vec![chunk(
            json!({"content": "All work complete. <promise>DONE</promise>"}),
            Some("stop"),
            None,
        )])),
    ]);

    let attempt = drive_attempt(&server, &run_dir, &cwd, 1);

    assert!(!attempt.is_error, "{:?}", attempt.terminal_reason);
    assert!(attempt.done_promise);
    assert_eq!(
        attempt.num_turns,
        Some(3),
        "the malformed turn still counts"
    );

    let events = transcript_events(&run_dir, 1);
    let names: Vec<&str> = events.iter().map(|(name, _)| name.as_str()).collect();

    let nudges: Vec<&Value> = events
        .iter()
        .filter(|(name, _)| name == "protocol_nudge")
        .map(|(_, value)| value)
        .collect();
    assert_eq!(nudges.len(), 1, "exactly one corrective nudge: {names:?}");
    let fault = nudges[0]["fault"]
        .as_str()
        .expect("a malformed call logs its fault");
    assert!(
        fault.contains("not valid JSON"),
        "the fault names exactly what was wrong: {fault}"
    );

    // The fault check ran before the gate: Work's denials carry force-push shell forms,
    // which a gate-first ordering could have answered with a denial record. It never did.
    // And the malformed call itself never entered the conversation: the only
    // assistant_tool_calls row is the corrected re-issue being echoed back before it runs.
    let echoed: Vec<String> = events
        .iter()
        .filter(|(name, _)| name == "assistant_tool_calls")
        .map(|(_, value)| value.to_string())
        .collect();
    assert_eq!(
        echoed.len(),
        1,
        "one echoed batch across the attempt — the correction, not the fault: {names:?}"
    );
    assert!(
        !echoed[0].contains("not json") && !echoed[0].contains("\"bash\""),
        "the malformed call never entered the conversation: {}",
        echoed[0]
    );
    assert!(
        echoed[0].contains("read_file"),
        "the one echoed batch is the corrected re-issue: {}",
        echoed[0]
    );
    let results: Vec<&Value> = events
        .iter()
        .filter(|(name, _)| name == "tool_result")
        .map(|(_, value)| value)
        .collect();
    assert_eq!(
        results.len(),
        1,
        "exactly one executed call across the whole attempt: {names:?}"
    );
    assert_eq!(results[0]["call_id"], "call_ok");

    // The corrective exchange went back over the wire, verbatim from
    // `malformed_call_exchange`, ahead of the model's second turn.
    let bodies = server.bodies();
    assert_eq!(bodies.len(), 3, "three scripted turns: {}", bodies.len());
    assert!(
        bodies[1].contains("was not well-formed"),
        "the correction names the malformation: {}",
        bodies[1]
    );
    assert!(
        !bodies[1].contains("\"not json\""),
        "the malformed arguments themselves never re-enter messages"
    );

    let (_, final_value) = events
        .iter()
        .find(|(name, _)| name == "final")
        .expect("Final logged");
    assert!(
        final_value["text"]
            .as_str()
            .expect("string final")
            .contains("DONE")
    );
}

/// One scripted turn that never finishes: a native-mode reply whose tool call fails the
/// fault check (no such tool), so the loop nudges and asks again — one request per turn,
/// forever, until the ceiling stops it.
fn non_final_turn() -> Reply {
    Reply::Sse(sse(vec![chunk(
        json!({"tool_calls": [tool_delta(0, Some("call_x"), Some("not_a_tool"), Some("{}"))]}),
        Some("tool_calls"),
        None,
    )]))
}

/// The ceiling is data now: a declared per-stage bound must stop the loop even when the
/// script carries more turns than it allows, and the bound arrives exactly where
/// `supervisor`'s runner seam hands a tiers.toml-resolved value — `Some(low_limit)` on the
/// adapter. With the compiled 32 still in force this scenario could not exhaust (four
/// scripted turns are far short of it), so an exhausted transcript naming `{low_limit}` is
/// itself the receipt that the override moved the ceiling.
#[test]
fn a_declared_max_turns_ceiling_bounds_the_loop_below_the_fallback() {
    let (run_dir, cwd) = scratch("max-turns");
    let low_limit = 3usize;

    let script = vec![
        non_final_turn(),
        non_final_turn(),
        non_final_turn(),
        non_final_turn(),
    ];
    let server = Server::start(script);

    let attempt = drive_attempt_with_prompt(
        &server,
        &run_dir,
        &cwd,
        1,
        "Do the assigned stage work.",
        Some(low_limit),
    );

    assert!(attempt.is_error, "{:?}", attempt.terminal_reason);
    assert!(!attempt.done_promise);
    assert_eq!(attempt.exit_code, Some(1));
    assert!(!attempt.rate_limited);
    assert!(attempt.parse_ok);
    let reason = attempt.terminal_reason.as_deref().unwrap_or("");
    assert!(
        reason.contains("turn budget exhausted") && reason.contains(&low_limit.to_string()),
        "the exhausted transcript names the limit that bound ({low_limit}): {reason}"
    );
    let received = server.bodies().len();
    assert_eq!(
        received, low_limit,
        "one request per allowed turn, stopped at the declared ceiling; got {received}"
    );
    assert!(
        received < DEFAULT_FALLBACK_TURNS,
        "fewer requests than would exhaust the compiled fallback of {DEFAULT_FALLBACK_TURNS}"
    );
    assert_eq!(attempt.num_turns, Some(low_limit as u64));
}

/// A real stage prompt opens with the skill file verbatim — YAML frontmatter first — so
/// production native transcripts begin with `skill_declared`, not `protocol_selected`.
/// The harness prompt carried no frontmatter until now, which let positional assertions
/// pass on a shape no Run produces (#135 follow-up): this scenario feeds real frontmatter
/// through the same composition seam the supervisor uses and pins the writer's row at 0.
#[test]
fn frontmatter_prompt_declares_its_skill() {
    let (run_dir, cwd) = scratch("skill-declared");
    let skill_text =
        "---\nname: work\ndescription: the stage's own words.\n---\n\nDo the assigned stage work.";

    let server = Server::start(vec![Reply::Sse(sse(vec![chunk(
        json!({"content": "Stage artifacts are on disk; the ladder may advance."}),
        Some("stop"),
        None,
    )]))]);

    let attempt = drive_attempt_with_prompt(&server, &run_dir, &cwd, 1, skill_text, None);

    assert!(!attempt.is_error, "{:?}", attempt.terminal_reason);

    let events = transcript_events(&run_dir, 1);
    let names: Vec<&str> = events.iter().map(|(name, _)| name.as_str()).collect();

    assert_eq!(names[0], "skill_declared", "{names:?}");
    assert_eq!(events[0].1["skill"], "work");
    assert_eq!(
        names[1], "protocol_selected",
        "the probe row follows the declaration: {names:?}"
    );
}
