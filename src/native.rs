//! Adapter #2: the grind-owned agent loop (ADR-0018).
//!
//! A turn-budgeted conversation against any OpenAI-compatible `/chat/completions`
//! endpoint: capability-adaptive wire modes with a per-run latch (R5), per-turn
//! bounded retries (R6), first-party tool gating (R7), usage recording (R8), and
//! grind-owned `messages-N.jsonl` transcripts (R4). The seam is infallible: every
//! failure — endpoint resolution, retry exhaustion, budget exhaustion — is a loud
//! failed [`Attempt`], never a clean stop.

use crate::runner::{
    Backend, CallSummary, Endpoint, ProtoMode, RunSpec, StageRunner, TranscriptEvent,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::attempt::{Attempt, Mode, is_rate_limited};
use crate::net::{ChatClient, ChatTurn, NetError, ToolCallSpec};
use crate::observe::{Observed, Reason};
use crate::tools::{self, GateDecision, GateLayer, RawCall, ToolDef, ToolRegistry};
use crate::world;

/// Hard ceiling on conversation turns per attempt; exhaustion is loud, never a stop.
const MAX_TURNS: usize = 32;
/// Network tries per turn before the attempt fails (POC parity).
const RETRY_ATTEMPTS: usize = 3;
/// Linear backoff between retries: `backoff_secs * failure_number`.
const RETRY_BACKOFF_SECS: u64 = 2;
/// How much of the final text / last error is kept as the record's tail
/// (`claude::classify` parity).
const TAIL_CHARS: usize = 1500;
/// Synthetic call id for text-protocol tool invocations (they have no wire id).
const TEXT_CALL_ID: &str = "text";
/// Reason recorded when this attempt inherits a latch from an earlier attempt.
const RESUMED_LATCH_REASON: &str = "resumed from an earlier attempt's ProtocolSelected";

// ---------------------------------------------------------------------------
// Wire shapes — pure builders so every decision is testable from literals.
// ---------------------------------------------------------------------------

/// The system prompt: workdir context, plus — in Text mode only — the tool
/// protocol section built from the registry's own definitions.
fn system_prompt(workdir: &str, defs: &[ToolDef], mode: ProtoMode) -> String {
    let mut prompt = format!(
        "You are a coding agent operating inside the directory {workdir}. \
         Accomplish the user's task using the provided tools. \
         Prefer verifying your work by running commands over asserting it. \
         When the task is complete and verified, reply with a short summary and make no further tool calls."
    );
    if mode == ProtoMode::Text {
        prompt.push_str(
            "\n\nTool protocol (the only way to call tools):\n\
             To call a tool, reply with exactly one tag on its own line and nothing else:\n\
             <tool>{\"name\":\"…\",\"arguments\":{…}}</tool>\n\
             Tools:\n",
        );
        for def in defs {
            prompt.push_str(&format!(
                "- {}: arguments {} — {}\n",
                def.name,
                params_summary(&def.parameters),
                def.description
            ));
        }
        prompt.push_str(
            "Each result arrives in the next user message wrapped in <tool_result> tags.\n\
             Emit one tag per reply; when the task is complete and verified, reply with \
             your short summary wrapped in <done></done> and no tool tag.",
        );
    }
    prompt
}

/// Render a JSON schema's properties compactly — `{"command": string}` — for the
/// text protocol's tool listing.
fn params_summary(parameters: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(props) = parameters["properties"].as_object() {
        for (key, schema) in props {
            let ty = schema["type"].as_str().unwrap_or("any");
            parts.push(format!("\"{key}\": {ty}"));
        }
    }
    format!("{{{}}}", parts.join(", "))
}

/// The registry's definitions as an OpenAI `tools` array value.
fn defs_as_wire(defs: &[ToolDef]) -> Value {
    Value::Array(
        defs.iter()
            .map(|d| {
                json!({
                    "type": "function",
                    "function": {
                        "name": d.name,
                        "description": d.description,
                        "parameters": d.parameters,
                    },
                })
            })
            .collect(),
    )
}

/// One request body. Text mode deliberately omits the tools param — some upstreams
/// reject every request carrying one (ADR-0018).
fn request_body(model: &str, messages: &[Value], tools_wire: &Value, mode: ProtoMode) -> Value {
    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    if mode == ProtoMode::Native {
        body["tools"] = tools_wire.clone();
    }
    body
}

/// Assistant echo with tool calls, OpenAI shapes verbatim.
fn assistant_echo_native(content: &str, calls: &[ToolCallSpec]) -> Value {
    json!({
        "role": "assistant",
        "content": if content.is_empty() { Value::Null } else { Value::String(content.to_string()) },
        "tool_calls": calls.iter().map(|c| json!({
            "id": c.id,
            "type": "function",
            "function": {"name": c.name, "arguments": c.arguments_json},
        })).collect::<Vec<_>>(),
    })
}

fn assistant_echo_text(content: &str) -> Value {
    json!({"role": "assistant", "content": content})
}

/// Where an executed tool's output re-enters the conversation: a `role: "tool"`
/// message keyed by call id in Native mode, a user-wrapped `<tool_result>` in Text.
fn tool_result_message(mode: ProtoMode, call_id: &str, call_name: &str, output: &str) -> Value {
    match mode {
        ProtoMode::Native => json!({"role": "tool", "tool_call_id": call_id, "content": output}),
        ProtoMode::Text => json!({
            "role": "user",
            "content": format!("<tool_result call=\"{call_name}\">\n{output}\n</tool_result>"),
        }),
    }
}

/// The corrective nudge sent after prose arrived where a tag was demanded —
/// logged as [`TranscriptEvent::ProtocolNudge`] first, always, so drift rates stay
/// comparable across models in P3.
fn nudge_exchange(assistant_text: &str) -> Vec<Value> {
    vec![
        json!({"role": "assistant", "content": assistant_text}),
        json!({"role": "user", "content":
            "Continue working by replying with exactly one <tool>{\"name\":…,\"arguments\":{…}}</tool> \
             tag, or, if the task is complete and verified, wrap your short summary in <done></done>."}),
    ]
}

// ---------------------------------------------------------------------------
// Text-protocol parsing.
// ---------------------------------------------------------------------------

/// Pull the first `<tool>{"name":..,"arguments":..}</tool>` out of a reply.
/// Markdown fencing around the tag is irrelevant; only the span between the
/// markers is parsed.
fn extract_tool_tag(text: &str) -> Option<(String, String)> {
    let start = text.find("<tool>")? + "<tool>".len();
    let end = text[start..].find("</tool>")? + start;
    let v: Value = serde_json::from_str(text[start..end].trim()).ok()?;
    Some((
        v["name"].as_str()?.to_string(),
        v["arguments"].clone().to_string(),
    ))
}

/// Pull the inner text of a `<done>…</done>` sentinel — the attempt's final summary.
fn extract_done(text: &str) -> Option<String> {
    let start = text.find("<done>")? + "<done>".len();
    let end = text[start..].find("</done>")? + start;
    Some(text[start..end].trim().to_string())
}

// ---------------------------------------------------------------------------
// Per-run protocol latch (R5/ADR-0018).
// ---------------------------------------------------------------------------

/// The mode latched by earlier attempts of this run, from their transcript
/// contents (oldest file first). The last `ProtocolSelected` wins.
fn latched_mode(transcripts: &[&str]) -> Option<ProtoMode> {
    let mut found = None;
    for text in transcripts {
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if value["event"] != "protocol_selected" {
                continue;
            }
            found = match value["value"]["mode"].as_str() {
                Some("native") => Some(ProtoMode::Native),
                Some("text") => Some(ProtoMode::Text),
                _ => continue,
            };
        }
    }
    found
}

/// Scan the run directory's prior `messages-*.jsonl` files (sorted, so oldest
/// first) for the latch. This attempt's own file is skipped.
fn scan_latch(run_dir: &Path, current_attempt: usize) -> Option<ProtoMode> {
    let own = format!("messages-{current_attempt}.jsonl");
    let mut texts: Vec<String> = Vec::new();
    for path in world::list_with_extension(run_dir, "jsonl") {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !name.starts_with("messages-") || name == own {
            continue;
        }
        if let Ok(text) = world::read_to_string(&path) {
            texts.push(text);
        }
    }
    latched_mode(&texts.iter().map(String::as_str).collect::<Vec<_>>())
}

// ---------------------------------------------------------------------------
// Retry budget (R6).
// ---------------------------------------------------------------------------

/// What to do after one failed request inside a turn's retry budget.
#[derive(Debug, PartialEq)]
enum Action {
    /// Sleep this long, then retry in the same mode.
    Backoff(Duration),
    /// The endpoint refused the tools array — latch Text and retry immediately.
    LatchText,
    /// Budget spent: fail the attempt loudly.
    GiveUp,
}

fn next_action(
    err: &NetError,
    mode_used: ProtoMode,
    latched: bool,
    failures: usize,
    retry_attempts: usize,
    backoff_secs: u64,
) -> Action {
    if mode_used == ProtoMode::Native && !latched && tools_rejected(err) {
        return Action::LatchText;
    }
    if failures + 1 >= retry_attempts {
        return Action::GiveUp;
    }
    Action::Backoff(Duration::from_secs(backoff_secs * (failures as u64 + 1)))
}

/// Does this error look like the endpoint rejecting a native tools array?
/// An HTTP 4xx body naming tools/tool_use, or an abnormal finish while tools
/// were sent (upstreams that fail mid-stream rather than at admission time).
fn tools_rejected(err: &NetError) -> bool {
    match err {
        NetError::Http { status, body } => {
            (400..500).contains(status) && body.to_lowercase().contains("tool")
        }
        NetError::AbnormalFinish { .. } => true,
        _ => false,
    }
}

fn describe_error(err: &NetError) -> String {
    match err {
        NetError::Http { status, body } => format!("HTTP {status}: {body}"),
        NetError::Stream(msg) => format!("stream failed: {msg}"),
        NetError::AbnormalFinish { finish, native } => {
            format!("stream ended abnormally: finish_reason={finish} native={native:?}")
        }
        NetError::EmptyResponse => {
            "model returned an empty response (no content, no tool calls)".to_string()
        }
    }
}

// ---------------------------------------------------------------------------
// Attempt synthesis.
// ---------------------------------------------------------------------------

/// How one attempt ended, distilled for synthesis.
enum Ending {
    /// Clean completion: the model's final summary.
    Completed(String),
    /// Any failure; the text becomes `terminal_reason` and `result_tail`.
    Failed(String),
}

/// Everything synthesis needs, bundled so the mapper keeps one signature.
struct AttemptFacts<'a> {
    n: usize,
    mode: Mode,
    started_at: &'a str,
    ended_at: &'a str,
    turns_used: usize,
    ending: Ending,
    usage: Option<Value>,
    denials: Vec<Value>,
}

/// Map the loop's outcome onto the record's currency, mirroring
/// `claude::classify`'s conventions: `parse_ok` always true (the loop spoke for
/// itself), `total_cost_usd` None (P3 decides pricing), exit 0/1, and rate-limit
/// detection asked of the shared classifier over a payload carrying the error text
/// — policy parity without duplicating needles.
fn synthesize(facts: AttemptFacts) -> Attempt {
    let (exit_code, is_error, done_promise, spoken) = match facts.ending {
        Ending::Completed(text) => (Some(0), false, true, text),
        Ending::Failed(reason) => (Some(1), true, false, reason),
    };
    let terminal_reason = is_error.then(|| spoken.clone());
    let count = spoken.chars().count();
    let result_tail: String = spoken
        .chars()
        .skip(count.saturating_sub(TAIL_CHARS))
        .collect();
    let payload = json!({"is_error": is_error, "result": spoken});
    Attempt {
        n: facts.n,
        mode: facts.mode,
        started_at: facts.started_at.to_string(),
        ended_at: facts.ended_at.to_string(),
        exit_code,
        is_error,
        parse_ok: true,
        subtype: None,
        stop_reason: None,
        api_error_status: None,
        terminal_reason,
        num_turns: Some(facts.turns_used as u64),
        total_cost_usd: None,
        usage: facts.usage,
        permission_denials: facts.denials,
        done_promise,
        rate_limited: is_rate_limited(&payload),
        result_tail,
        fanout: Observed::Unobservable(Reason::saying("the native loop spawns no subagents")),
    }
}

fn layer_name(layer: GateLayer) -> &'static str {
    match layer {
        GateLayer::UnknownTool => "unknown_tool",
        GateLayer::InvalidArgs => "invalid_args",
        GateLayer::DeniedGlob => "denied_glob",
        GateLayer::PathEscape => "path_escape",
    }
}

// ---------------------------------------------------------------------------
// The loop.
// ---------------------------------------------------------------------------

/// One attempt's `messages-N.jsonl` writer (R4).
struct Transcript {
    path: PathBuf,
    attempt_n: usize,
}

impl Transcript {
    /// Append failures are unrecoverable local IO — loud, like spawn failure.
    fn log(&self, event: &TranscriptEvent) {
        if let Err(e) = world::append_line(&self.path, &event.encode()) {
            panic!("attempt {}: transcript append failed: {e}", self.attempt_n);
        }
    }
}

/// One successful request/response cycle plus the wire state it settled.
struct TurnOutcome {
    turn: ChatTurn,
    /// The wire mode the successful request used.
    mode: ProtoMode,
    /// Set when this very turn determined the run's protocol by rejection.
    latch: Option<(ProtoMode, String)>,
}

/// Drive one conversation turn through the per-turn retry budget. Retries sleep
/// linearly; a tools-array refusal from an unlatched Native start latches Text
/// (fresh budget on the new wire) instead of burning retries.
fn drive_turn(
    client: &ChatClient,
    ep: &Endpoint,
    messages: &[Value],
    tools_wire: &Value,
    start_mode: ProtoMode,
    latched_before: bool,
) -> Result<TurnOutcome, String> {
    let mut mode = start_mode;
    let mut latched = latched_before;
    let mut latch = None;
    let mut failures = 0usize;
    loop {
        match client.post_chat(ep, &request_body(&ep.model, messages, tools_wire, mode)) {
            Ok(turn) => return Ok(TurnOutcome { turn, mode, latch }),
            Err(err) => match next_action(
                &err,
                mode,
                latched,
                failures,
                RETRY_ATTEMPTS,
                RETRY_BACKOFF_SECS,
            ) {
                Action::LatchText => {
                    latch = Some((
                        ProtoMode::Text,
                        format!(
                            "endpoint rejected the tools array ({})",
                            describe_error(&err)
                        ),
                    ));
                    mode = ProtoMode::Text;
                    latched = true;
                    failures = 0;
                }
                Action::Backoff(delay) => {
                    world::sleep(delay);
                    failures += 1;
                }
                Action::GiveUp => return Err(describe_error(&err)),
            },
        }
    }
}

impl StageRunner for crate::runner::NativeAdapter {
    fn backend(&self) -> Backend {
        Backend::Native
    }

    fn run(&self, spec: &RunSpec) -> Attempt {
        let n = spec.attempt_n;
        let started_at = world::now_iso();

        // Resolution happens at attempt start; failure is a loud failed Attempt,
        // never a clean stop.
        let endpoint = match Endpoint::resolve(self.endpoint_override.as_deref(), spec.model) {
            Ok(endpoint) => endpoint,
            Err(reason) => {
                return synthesize(AttemptFacts {
                    n,
                    mode: spec.invocation.mode(),
                    started_at: &started_at,
                    ended_at: &world::now_iso(),
                    turns_used: 0,
                    ending: Ending::Failed(format!("endpoint resolution failed: {reason}")),
                    usage: None,
                    denials: Vec::new(),
                });
            }
        };

        let registry = ToolRegistry::standard(spec.cwd.to_path_buf());
        let defs = registry.defs();
        let tools_wire = defs_as_wire(&defs);
        let client = ChatClient::new();
        let transcript = Transcript {
            path: spec.run_dir.join(format!("messages-{n}.jsonl")),
            attempt_n: n,
        };
        let system_for = |mode: ProtoMode| json!({"role": "system", "content": system_prompt(&spec.cwd.display().to_string(), &defs, mode)});

        // Per-run latch (R5/ADR-0018): an earlier attempt's ProtocolSelected
        // decides the wire before the first request goes out.
        let mut proto = scan_latch(spec.run_dir, n);
        if let Some(mode) = proto {
            transcript.log(&TranscriptEvent::ProtocolSelected {
                mode,
                reason: RESUMED_LATCH_REASON.to_string(),
            });
        }

        let mut messages = vec![
            system_for(proto.unwrap_or(ProtoMode::Native)),
            json!({"role": "user", "content": spec.invocation.prompt()}),
        ];

        let mut turns_used = 0usize;
        let mut usage_last: Option<Value> = None;
        let mut denials: Vec<Value> = Vec::new();

        let ending = loop {
            if turns_used >= MAX_TURNS {
                break Ending::Failed(format!("turn budget exhausted ({MAX_TURNS})"));
            }
            turns_used += 1;

            let outcome = match drive_turn(
                &client,
                &endpoint,
                &messages,
                &tools_wire,
                proto.unwrap_or(ProtoMode::Native),
                proto.is_some(),
            ) {
                Ok(outcome) => outcome,
                Err(reason) => break Ending::Failed(format!("turn {turns_used} failed: {reason}")),
            };
            let mode = outcome.mode;

            // First protocol determination of this run: logged once, before any
            // of this attempt's other events.
            if proto.is_none() {
                let (selected, reason) = match outcome.latch {
                    Some((m, r)) => (m, r),
                    None => (
                        mode,
                        "probe succeeded: endpoint accepted the tools array".to_string(),
                    ),
                };
                proto = Some(selected);
                if selected != ProtoMode::Native {
                    messages[0] = system_for(selected);
                }
                transcript.log(&TranscriptEvent::ProtocolSelected {
                    mode: selected,
                    reason,
                });
            }

            if let Some(usage) = outcome.turn.usage.clone() {
                usage_last = Some(usage.clone());
                transcript.log(&TranscriptEvent::Usage(usage));
            }

            let content = outcome.turn.content;

            // Endings. Native: stop-with-content completes. Text: only an explicit
            // <done> sentinel completes — everything else keeps working.
            if mode == ProtoMode::Native && outcome.turn.tool_calls.is_empty() {
                transcript.log(&TranscriptEvent::Final {
                    text: content.clone(),
                });
                break Ending::Completed(content);
            }
            let pending: Vec<ToolCallSpec> = if mode == ProtoMode::Text {
                if let Some(summary) = extract_done(&content) {
                    transcript.log(&TranscriptEvent::Final {
                        text: summary.clone(),
                    });
                    break Ending::Completed(summary);
                }
                extract_tool_tag(&content)
                    .map(|(name, arguments_json)| ToolCallSpec {
                        id: TEXT_CALL_ID.to_string(),
                        name,
                        arguments_json,
                    })
                    .into_iter()
                    .collect()
            } else {
                outcome.turn.tool_calls
            };

            if pending.is_empty() {
                // Prose where a tag was demanded: one corrective nudge, logged.
                transcript.log(&TranscriptEvent::ProtocolNudge {
                    assistant_text: content.clone(),
                });
                messages.extend(nudge_exchange(&content));
                continue;
            }

            // Assistant echo, then the gated execution of each call.
            messages.push(match mode {
                ProtoMode::Native => assistant_echo_native(&content, &pending),
                ProtoMode::Text => assistant_echo_text(&content),
            });
            transcript.log(&TranscriptEvent::AssistantToolCalls {
                calls: pending
                    .iter()
                    .map(|c| CallSummary {
                        name: c.name.clone(),
                        arguments: c.arguments_json.clone(),
                    })
                    .collect(),
            });

            for call in pending {
                let raw = RawCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments_json: call.arguments_json.clone(),
                };
                let output = match tools::gate(spec.denied_globs, raw) {
                    GateDecision::Denied(report) => {
                        denials.push(json!({
                            "tool": report.tool,
                            "layer": layer_name(report.layer),
                            "reason": report.reason,
                        }));
                        truncate_denial(report.layer, &report.reason)
                    }
                    GateDecision::Allowed(raw) => registry.execute(&raw).output,
                };
                transcript.log(&TranscriptEvent::ToolResult {
                    call_id: call.id.clone(),
                    output: output.clone(),
                });
                messages.push(tool_result_message(mode, &call.id, &call.name, &output));
            }
        };

        synthesize(AttemptFacts {
            n,
            mode: spec.invocation.mode(),
            started_at: &started_at,
            ended_at: &world::now_iso(),
            turns_used,
            ending,
            usage: usage_last,
            denials,
        })
    }
}

/// The denial fed back to the model as its tool result — structured enough for the
/// model to route around, short enough not to spend the output budget.
fn truncate_denial(layer: GateLayer, reason: &str) -> String {
    tools::truncate_output(&format!(
        "denied by the {} gate: {reason}",
        layer_name(layer)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- text-protocol parsing ------------------------------------------------

    #[test]
    fn tool_tag_extraction_plain_and_fenced() {
        assert_eq!(
            extract_tool_tag(r#"<tool>{"name":"bash","arguments":{"command":"ls"}}</tool>"#),
            Some(("bash".into(), r#"{"command":"ls"}"#.into()))
        );
        let fenced = "Sure, running that now.\n\n```json\n\
                      <tool>{\"name\":\"read_file\",\"arguments\":{\"path\":\"a.md\"}}</tool>\n\
                      ```\n";
        assert_eq!(
            extract_tool_tag(fenced),
            Some(("read_file".into(), r#"{"path":"a.md"}"#.into()))
        );
        // First tag wins.
        let two = "<tool>{\"name\":\"a\",\"arguments\":{}}</tool> then \
                   <tool>{\"name\":\"b\",\"arguments\":{}}</tool>";
        assert_eq!(
            extract_tool_tag(two).map(|(name, _)| name),
            Some("a".into())
        );
    }

    #[test]
    fn malformed_or_absent_tool_tags_are_not_calls() {
        assert_eq!(extract_tool_tag("just prose, no tag"), None);
        assert_eq!(extract_tool_tag("<tool>{not json}</tool>"), None);
        assert_eq!(extract_tool_tag("<tool>{\"name\":\"bash\"}"), None);
        assert_eq!(
            extract_tool_tag("<tool></tool>"),
            None,
            "arguments must at least carry a name"
        );
    }

    #[test]
    fn done_sentinel_inner_text_is_trimmed() {
        assert_eq!(
            extract_done("working…\n<done>\nAll tests green.\n</done>"),
            Some("All tests green.".into())
        );
        assert_eq!(extract_done("prose without the sentinel"), None);
        assert_eq!(extract_done("<done>unclosed"), None);
        assert_eq!(extract_done("</done>closed-only"), None);
    }

    // --- latch scan -------------------------------------------------------------

    #[test]
    fn latch_scan_honors_the_last_protocol_selected() {
        let older = r#"{"event":"protocol_selected","value":{"mode":"text","reason":"rejected"}}"#;
        let newer =
            r#"{"event":"protocol_selected","value":{"mode":"native","reason":"probe ok"}}"#;
        assert_eq!(latched_mode(&[older, newer]), Some(ProtoMode::Native));
        assert_eq!(latched_mode(&[newer, older]), Some(ProtoMode::Text));
    }

    #[test]
    fn latch_scan_ignores_other_events_and_malformed_lines() {
        let transcript = "\n{\"event\":\"final\",\"value\":{\"text\":\"hi\"}}\nnot json\n";
        assert_eq!(latched_mode(&[transcript]), None);
        assert_eq!(latched_mode(&[]), None);
    }

    fn http_reject() -> NetError {
        NetError::Http {
            status: 400,
            body: "\"stealth/ox-alpha does not support tool_use\"".into(),
        }
    }

    #[test]
    fn tools_array_rejection_latches_text_immediately_even_at_the_last_failure() {
        // Injected literals: 3 tries, 2-second steps. The latch outranks the budget.
        assert_eq!(
            next_action(&http_reject(), ProtoMode::Native, false, 2, 3, 2),
            Action::LatchText
        );
    }

    #[test]
    fn abnormal_finish_while_tools_were_sent_latches_but_text_never_does() {
        let err = NetError::AbnormalFinish {
            finish: "stop".into(),
            native: Some("length".into()),
        };
        assert_eq!(
            next_action(&err, ProtoMode::Native, false, 0, 3, 2),
            Action::LatchText
        );
        assert_eq!(
            next_action(&err, ProtoMode::Text, true, 0, 3, 2),
            Action::Backoff(Duration::from_secs(2))
        );
    }

    #[test]
    fn ordinary_failures_back_off_linearly_then_give_up() {
        let err = NetError::Stream("connection reset".into());
        assert_eq!(
            next_action(&err, ProtoMode::Native, true, 0, 3, 2),
            Action::Backoff(Duration::from_secs(2))
        );
        assert_eq!(
            next_action(&err, ProtoMode::Native, true, 1, 3, 2),
            Action::Backoff(Duration::from_secs(4))
        );
        assert_eq!(
            next_action(&err, ProtoMode::Native, true, 2, 3, 2),
            Action::GiveUp
        );
    }

    #[test]
    fn a_latched_run_never_re_latches_on_a_tools_rejection() {
        assert_eq!(
            next_action(&http_reject(), ProtoMode::Native, true, 0, 3, 2),
            Action::Backoff(Duration::from_secs(2))
        );
    }

    // --- Attempt synthesis --------------------------------------------------------

    #[test]
    fn completed_attempts_mirror_the_classify_conventions() {
        let attempt = synthesize(AttemptFacts {
            n: 4,
            mode: Mode::Dispatch,
            started_at: "s",
            ended_at: "e",
            turns_used: 7,
            ending: Ending::Completed("shipped it".into()),
            usage: Some(json!({"prompt_tokens": 10})),
            denials: vec![],
        });
        assert_eq!(attempt.exit_code, Some(0));
        assert!(!attempt.is_error);
        assert!(attempt.parse_ok);
        assert_eq!(attempt.subtype, None);
        assert!(attempt.done_promise);
        assert!(!attempt.rate_limited);
        assert_eq!(attempt.total_cost_usd, None);
        assert_eq!(attempt.num_turns, Some(7));
        assert_eq!(attempt.usage.as_ref().unwrap()["prompt_tokens"], 10);
        assert_eq!(attempt.result_tail, "shipped it");
        assert!(matches!(attempt.fanout, Observed::Unobservable(_)));
    }

    #[test]
    fn failed_attempts_ask_the_shared_classifier_over_the_error_text() {
        let limited = synthesize(AttemptFacts {
            n: 1,
            mode: Mode::Resume,
            started_at: "s",
            ended_at: "e",
            turns_used: 3,
            ending: Ending::Failed(
                "HTTP 429: RateLimitExceeded — usage limit reached, resets at 17:00".into(),
            ),
            usage: None,
            denials: vec![],
        });
        assert_eq!(limited.exit_code, Some(1));
        assert!(limited.is_error);
        assert!(!limited.done_promise);
        assert!(
            limited.rate_limited,
            "limit-shaped errors read as rate-limited"
        );
        assert!(limited.terminal_reason.unwrap().contains("RateLimit"));

        let plain = synthesize(AttemptFacts {
            n: 1,
            mode: Mode::Resume,
            started_at: "s",
            ended_at: "e",
            turns_used: 3,
            ending: Ending::Failed("stream failed: connection reset".into()),
            usage: None,
            denials: vec![],
        });
        assert!(!plain.rate_limited);
        assert_eq!(plain.result_tail, "stream failed: connection reset");
    }

    #[test]
    fn result_tail_is_capped_like_the_classifier_s() {
        let long = "x".repeat(TAIL_CHARS + 50);
        let attempt = synthesize(AttemptFacts {
            n: 1,
            mode: Mode::Dispatch,
            started_at: "s",
            ended_at: "e",
            turns_used: 1,
            ending: Ending::Completed(long.clone()),
            usage: None,
            denials: vec![],
        });
        assert_eq!(attempt.result_tail.chars().count(), TAIL_CHARS);
        assert!(long.ends_with(&attempt.result_tail));
    }

    // --- nudge + prompts ----------------------------------------------------------

    #[test]
    fn the_nudge_is_an_assistant_echo_plus_one_corrective_user_turn() {
        let exchange = nudge_exchange("I could use a tool here.");
        assert_eq!(exchange.len(), 2);
        assert_eq!(exchange[0]["role"], "assistant");
        assert_eq!(exchange[0]["content"], "I could use a tool here.");
        assert_eq!(exchange[1]["role"], "user");
        let hint = exchange[1]["content"].as_str().unwrap();
        assert!(hint.contains("<tool>"));
        assert!(hint.contains("<done>"));
    }

    #[test]
    fn the_protocol_section_is_present_exactly_in_text_mode() {
        let registry = ToolRegistry::standard(std::path::PathBuf::from("/tmp/wd"));
        let defs = registry.defs();
        let plain = system_prompt("/tmp/wd", &defs, ProtoMode::Native);
        assert!(plain.contains("/tmp/wd"));
        assert!(!plain.contains("<tool>"));
        let text = system_prompt("/tmp/wd", &defs, ProtoMode::Text);
        assert!(text.contains("<tool>"));
        assert!(text.contains("- bash:"), "tools listed from the registry");
        assert!(text.contains("\"command\": string"));
        assert!(text.contains("<done>"));
    }

    #[test]
    fn the_request_body_carries_tools_only_in_native_mode() {
        let tools = defs_as_wire(&ToolRegistry::standard(std::path::PathBuf::from("/w")).defs());
        let messages = vec![json!({"role": "user", "content": "go"})];
        let native = request_body("m", &messages, &tools, ProtoMode::Native);
        assert_eq!(native["model"], "m");
        assert_eq!(native["stream"], true);
        assert_eq!(native["stream_options"]["include_usage"], true);
        assert_eq!(native["tools"][0]["type"], "function");
        assert_eq!(native["tools"][0]["function"]["name"], "bash");
        let text = request_body("m", &messages, &tools, ProtoMode::Text);
        assert!(text.get("tools").is_none());
    }

    #[test]
    fn wire_defs_shape_matches_the_openai_function_schema() {
        let wire = defs_as_wire(&ToolRegistry::standard(std::path::PathBuf::from("/w")).defs());
        let names: Vec<&str> = wire
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["bash", "read_file", "write_file"]);
        assert!(wire[1]["function"]["parameters"]["properties"]["path"].is_object());
    }

    #[test]
    fn tool_results_reenter_in_the_mode_own_shape() {
        let native = tool_result_message(ProtoMode::Native, "call_9", "bash", "out");
        assert_eq!(native["role"], "tool");
        assert_eq!(native["tool_call_id"], "call_9");
        assert_eq!(native["content"], "out");
        let text = tool_result_message(ProtoMode::Text, "text", "bash", "out");
        assert_eq!(text["role"], "user");
        assert!(
            text["content"]
                .as_str()
                .unwrap()
                .starts_with("<tool_result call=\"bash\">")
        );
    }

    #[test]
    fn native_echoes_null_content_for_empty_replies() {
        let calls = [ToolCallSpec {
            id: "c1".into(),
            name: "bash".into(),
            arguments_json: "{\"command\":\"true\"}".into(),
        }];
        let echo = assistant_echo_native("", &calls);
        assert!(echo["content"].is_null());
        assert_eq!(echo["tool_calls"][0]["id"], "c1");
        assert_eq!(echo["tool_calls"][0]["function"]["name"], "bash");
        assert_eq!(
            assistant_echo_native("thinking", &calls)["content"],
            "thinking"
        );
        assert_eq!(assistant_echo_text("tag incoming")["role"], "assistant");
    }
}
