//! Adapter #2: the grind-owned agent loop (ADR-0018).
//!
//! A turn-budgeted conversation against any OpenAI-compatible `/chat/completions`
//! endpoint: capability-adaptive wire modes with a per-run latch (R5), per-turn
//! bounded retries (R6), first-party tool gating (R7), usage recording (R8), and
//! grind-owned `messages-N.jsonl` transcripts (R4). The seam is infallible: every
//! failure — endpoint resolution, retry exhaustion, budget exhaustion — is a loud
//! failed [`Attempt`], never a clean stop.

use crate::runner::{
    Backend, CallSummary, Endpoint, FileLabel, ProtoMode, RunSpec, StageRunner, TranscriptEvent,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::attempt::{Attempt, Mode, is_rate_limited};
use crate::net::{ChatClient, ChatTurn, NetError, ToolCallSpec};
use crate::observe::{self, Observed, Reason};
use crate::tools::{self, GateDecision, GateLayer, RawCall, ToolDef, ToolRegistry};
use crate::view::{Live, one_line};
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

/// Reason recorded when the wire mode came from the `~/.grind/agent` line's `proto=` key
/// rather than a probe or an earlier attempt's latch — the case ADR-0018's `proto=` exists
/// for: a model proven unable to execute native tool calls (`stealth/ox-alpha`) otherwise
/// wastes one failed request discovering that on every single attempt.
fn declared_reason(mode: ProtoMode) -> String {
    format!(
        "declared via `proto={}` on the `~/.grind/agent` line — the probe was skipped",
        match mode {
            ProtoMode::Native => "native",
            ProtoMode::Text => "text",
        }
    )
}

/// This call's transcript file name (ADR-0017's `messages-N.jsonl` for an Attempt, kept
/// byte-for-byte; a distinguishable name for Reflect so its events never collide with
/// attempt N's own file).
fn transcript_filename(label: FileLabel, n: usize) -> String {
    match label {
        FileLabel::Attempt => format!("messages-{n}.jsonl"),
        FileLabel::Reflect => format!("reflect-messages-{n}.jsonl"),
    }
}

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

/// The skill the stage prompt declares about itself, read off its own YAML frontmatter.
///
/// A stage prompt is the skill file verbatim followed by a context block
/// (`claude::stage_dispatch_prompt`), and every `skills/run/<stage>/SKILL.md` opens with a
/// `name:` row — so the prompt names its own rung, and nothing extra has to be threaded
/// through the seam to learn it. Only the leading `---` block is read: the composed prompt
/// uses `---` again as a separator, and scanning past the first block would start reading the
/// context block's prose as frontmatter.
///
/// `None` where there is no leading block, no `name:` row, or an empty value — the caller logs
/// nothing then, and the reader reports *could not observe* rather than a blank skill.
fn declared_skill(prompt: &str) -> Option<String> {
    let mut lines = prompt.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    for line in lines {
        let line = line.trim();
        if line == "---" {
            return None;
        }
        if let Some(name) = line.strip_prefix("name:") {
            let name = name.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
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

/// Scan the run directory's prior `messages-*.jsonl` files, oldest attempt
/// first, for the latch. This attempt's own file is skipped.
///
/// Ordered by the parsed attempt number, not the lexicographic path order
/// `list_with_extension` returns — that sort puts `messages-10.jsonl` before
/// `messages-2.jsonl`, which would make attempt 10 look older than attempt 2
/// the moment a run passes nine attempts.
fn scan_latch(run_dir: &Path, current_attempt: usize) -> Option<ProtoMode> {
    let own = format!("messages-{current_attempt}.jsonl");
    let mut numbered: Vec<(usize, PathBuf)> = Vec::new();
    for path in world::list_with_extension(run_dir, "jsonl") {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name == own {
            continue;
        }
        let Some(num) = name
            .strip_prefix("messages-")
            .and_then(|s| s.strip_suffix(".jsonl"))
            .and_then(|s| s.parse::<usize>().ok())
        else {
            continue;
        };
        numbered.push((num, path));
    }
    numbered.sort_by_key(|(num, _)| *num);
    let texts: Vec<String> = numbered
        .into_iter()
        .filter_map(|(_, path)| world::read_to_string(&path).ok())
        .collect();
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
/// An HTTP 4xx body naming tools/tool_use, or an abnormal finish that itself names
/// tools or a bare upstream error (upstreams that fail mid-stream rather than at
/// admission time). An ordinary `finish_reason` like `"length"` or
/// `"content_filter"` is also `AbnormalFinish` but names nothing about tools, so it
/// must not latch Text — that would pin the whole Run to the text protocol on the
/// strength of one long reply.
fn tools_rejected(err: &NetError) -> bool {
    match err {
        NetError::Http { status, body } => {
            (400..500).contains(status) && body.to_lowercase().contains("tool")
        }
        NetError::AbnormalFinish { finish, native } => {
            let f = finish.to_lowercase();
            let n = native.as_deref().unwrap_or("").to_lowercase();
            f.contains("tool") || n.contains("tool") || f == "error" || n == "error"
        }
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
/// itself), exit 0/1, and rate-limit detection asked of the shared classifier over a
/// payload carrying the error text — policy parity without duplicating needles.
///
/// `total_cost_usd` carries the run's summed `usage.cost` when the endpoint reported
/// one (`accumulate_usage` folds each turn's per-request cost into the total), and is
/// otherwise `Some(0.0)` — never `None`: unlike the claude-code path, where an absent
/// JSON field is genuinely ambiguous between "renamed key" and "true zero", the native
/// loop authoritatively knows it recorded no cost. `None` here would make
/// `Attempt::is_wait()` false for every native Attempt regardless of `num_turns`,
/// which lets a first-turn rate limit spend the attempt budget and keeps
/// `trailing_waits` permanently at 0 — the Run 2 failure ADR-0002/0004 exist to
/// prevent.
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
        total_cost_usd: Some(
            facts
                .usage
                .as_ref()
                .and_then(|u| u.get("cost"))
                .and_then(|c| c.as_f64())
                .unwrap_or(0.0),
        ),
        usage: facts.usage,
        permission_denials: facts.denials,
        done_promise,
        rate_limited: is_rate_limited(&payload),
        result_tail,
        fanout: Observed::Unobservable(Reason::saying("the native loop spawns no subagents")),
    }
}

/// Fold one turn's usage object into the run's running total. An OpenAI-compatible
/// stream's `usage` is per-request, not cumulative, so a plain overwrite would leave
/// the Attempt holding only the final turn's tokens (R8: usage from every response
/// belongs on the Attempt, not just the transcript). Every numeric leaf — including
/// ones nested under an object such as `prompt_tokens_details` — is summed; a
/// non-numeric leaf keeps the latest turn's value.
fn accumulate_usage(acc: Option<Value>, turn: &Value) -> Value {
    match acc {
        Some(mut existing) => {
            merge_usage(&mut existing, turn);
            existing
        }
        None => turn.clone(),
    }
}

fn merge_usage(acc: &mut Value, turn: &Value) {
    match turn {
        Value::Object(t) => {
            if !acc.is_object() {
                *acc = json!({});
            }
            let a = acc.as_object_mut().expect("just ensured object");
            for (k, v) in t {
                match a.get_mut(k) {
                    Some(existing) => merge_usage(existing, v),
                    None => {
                        a.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        Value::Number(n) => {
            *acc = match (acc.as_i64(), n.as_i64()) {
                (Some(a), Some(b)) => json!(a + b),
                _ => json!(acc.as_f64().unwrap_or(0.0) + n.as_f64().unwrap_or(0.0)),
            };
        }
        other => *acc = other.clone(),
    }
}

/// A denial's structured form, in the vocabulary `policy::denied_invocations` and
/// `render.rs` actually read (`tool_name`, `tool_input.command`) — not the
/// `tool`/`layer` keys those readers ignore, which rendered every native denial as
/// the identity string `"?()"` and made policy compare two different denials as the
/// same repeated invocation. `gate_layer`/`reason` ride along as additive keys.
fn denial_json(report: &tools::GateReport, arguments_json: &str) -> Value {
    json!({
        "tool_name": report.tool,
        "tool_input": serde_json::from_str::<Value>(arguments_json).unwrap_or(Value::Null),
        "gate_layer": layer_name(report.layer),
        "reason": report.reason,
    })
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
    /// The first append failure, if any. `world::append_line`'s own contract says a
    /// log that cannot be written is not worth abandoning a Run over and the caller
    /// may ignore it — so this never panics. It is remembered rather than dropped
    /// outright so it can ride along in `terminal_reason` when the attempt is
    /// already failing.
    append_failed: std::cell::RefCell<Option<String>>,
}

impl Transcript {
    /// Reset this attempt's file to empty. Must run exactly once per `run()`,
    /// before the first `log` call — a crashed attempt is never recorded, so a
    /// resumed retry recomputes the same `attempt_n` and would otherwise append
    /// after the dead attempt's partial content in the same file
    /// (`messages-N.jsonl` must hold exactly one attempt's events, matching the
    /// claude-code adapter's `File::create` truncation). Calling this once per
    /// `log` instead would erase the attempt as it goes, so callers must not.
    ///
    /// Never panics, for the same reason `log` doesn't: a disk-write failure
    /// here must not take down the per-run supervisor before
    /// `record.push_attempt` ever runs. A failure is folded into the same
    /// `append_failed` slot a failed append uses, so it still surfaces in
    /// `terminal_reason` when the attempt is already failing.
    fn truncate(&self) {
        if let Err(e) = world::write(&self.path, "") {
            self.note_failure(format!(
                "attempt {}: transcript truncate failed: {e}",
                self.attempt_n
            ));
        }
    }

    fn note_failure(&self, msg: String) {
        let mut slot = self.append_failed.borrow_mut();
        if slot.is_none() {
            *slot = Some(msg);
        }
    }

    /// Never panics: a disk-write failure here must not take down the per-run
    /// supervisor before `record.push_attempt` ever runs.
    fn log(&self, event: &TranscriptEvent) {
        // `encode()` returns the bare JSON line; `append_line`'s own `writeln!` is
        // what supplies the single trailing newline.
        if let Err(e) = world::append_line(&self.path, &event.encode()) {
            self.note_failure(format!(
                "attempt {}: transcript append failed: {e}",
                self.attempt_n
            ));
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

/// Swap `messages[0]` to the given mode's system prompt, in place. Pulled out as its
/// own pure step so the actual bug — the swap happening after the reply is already in
/// hand rather than before the retry that needs it goes out — is unit-testable with
/// no I/O: call it, then read `messages[0]` back.
fn install_system(
    messages: &mut [Value],
    mode: ProtoMode,
    system_for: impl Fn(ProtoMode) -> Value,
) {
    messages[0] = system_for(mode);
}

/// Drive one conversation turn through the per-turn retry budget. Retries sleep
/// linearly; a tools-array refusal from an unlatched Native start latches Text
/// (fresh budget on the new wire) instead of burning retries. The latch installs
/// the Text system prompt into `messages[0]` *before* the retry request goes out —
/// swapping it only after the reply is already in hand would send that reply to a
/// model that was never told `<tool>`/`<done>` exist, guaranteeing a spurious
/// `ProtocolNudge` on every text-latched run's first Text reply.
fn drive_turn(
    client: &ChatClient,
    ep: &Endpoint,
    messages: &mut [Value],
    tools_wire: &Value,
    start_mode: ProtoMode,
    latched_before: bool,
    system_for: &dyn Fn(ProtoMode) -> Value,
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
                    install_system(messages, ProtoMode::Text, system_for);
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
        // never a clean stop. The concrete id is this stage's routed class (or pin)
        // resolved against the host's declared fast/strong models — never the
        // claude-code alias `resolve_stage_model` used to hand every adapter verbatim
        // (Unit 1's defect).
        let model_id = spec
            .model
            .native_id(self.fast_model.as_deref(), self.strong_model.as_deref());
        let endpoint = match Endpoint::resolve(self.endpoint_override.as_deref(), Some(&model_id)) {
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
            path: spec.run_dir.join(transcript_filename(spec.file_label, n)),
            attempt_n: n,
            append_failed: std::cell::RefCell::new(None),
        };
        // Truncate once, before any `log` call: a crashed attempt was never
        // recorded, so a `resume` recomputing this same `n` must not append after
        // the dead attempt's partial content in the same file.
        transcript.truncate();
        // Which rung is running, recorded before anything else this attempt does: the loop
        // below emits only wire events, so without this line nothing in the transcript could
        // answer `grind status`'s `now` question for a native Run at all.
        if let Some(skill) = declared_skill(spec.invocation.prompt()) {
            transcript.log(&TranscriptEvent::SkillDeclared { skill });
        }
        let system_for = |mode: ProtoMode| json!({"role": "system", "content": system_prompt(&spec.cwd.display().to_string(), &defs, mode)});

        // Per-run latch (R5/ADR-0018): a host declaration (`proto=`) wins outright and
        // skips the probe entirely — the case a model proven unable to execute native
        // tool calls exists for. Absent one, an earlier attempt's ProtocolSelected
        // decides the wire before the first request goes out.
        let mut proto = self.proto_override.or_else(|| scan_latch(spec.run_dir, n));
        if let Some(mode) = proto {
            let reason = if self.proto_override.is_some() {
                declared_reason(mode)
            } else {
                RESUMED_LATCH_REASON.to_string()
            };
            transcript.log(&TranscriptEvent::ProtocolSelected { mode, reason });
        }

        let mut messages = vec![
            system_for(proto.unwrap_or(ProtoMode::Native)),
            json!({"role": "user", "content": spec.invocation.prompt()}),
        ];

        let mut turns_used = 0usize;
        let mut usage_total: Option<Value> = None;
        let mut denials: Vec<Value> = Vec::new();

        let ending = loop {
            if turns_used >= MAX_TURNS {
                break Ending::Failed(format!("turn budget exhausted ({MAX_TURNS})"));
            }
            turns_used += 1;

            let outcome = match drive_turn(
                &client,
                &endpoint,
                &mut messages,
                &tools_wire,
                proto.unwrap_or(ProtoMode::Native),
                proto.is_some(),
                &system_for,
            ) {
                Ok(outcome) => outcome,
                Err(reason) => break Ending::Failed(format!("turn {turns_used} failed: {reason}")),
            };
            let mode = outcome.mode;

            // First protocol determination of this run: logged once, before any
            // of this attempt's other wire events.
            if proto.is_none() {
                let (selected, reason) = match outcome.latch {
                    Some((m, r)) => (m, r),
                    None => (
                        mode,
                        "probe succeeded: endpoint accepted the tools array".to_string(),
                    ),
                };
                proto = Some(selected);
                // The Text system prompt, when this attempt latches, was already
                // installed inside `drive_turn` before the retry that produced this
                // very outcome went out — nothing left to swap here.
                transcript.log(&TranscriptEvent::ProtocolSelected {
                    mode: selected,
                    reason,
                });
            }

            if let Some(usage) = outcome.turn.usage.clone() {
                usage_total = Some(accumulate_usage(usage_total.take(), &usage));
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
                        denials.push(denial_json(&report, &call.arguments_json));
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

        // A transcript-append failure is folded into an already-failing attempt's
        // reason rather than raised through a new channel; it never turns a
        // completion into a failure on its own.
        let ending = match (ending, transcript.append_failed.borrow().as_ref()) {
            (Ending::Failed(reason), Some(append_err)) => {
                Ending::Failed(format!("{reason}; {append_err}"))
            }
            (other, _) => other,
        };

        synthesize(AttemptFacts {
            n,
            mode: spec.invocation.mode(),
            started_at: &started_at,
            ended_at: &world::now_iso(),
            turns_used,
            ending,
            usage: usage_total,
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

// --- the live view, read from grind's own format ----------------------------------------------
//
// The mirror of `claude::live`, over a format grind writes itself (R4) — and owning the format
// inverts `claude`'s reading discipline rather than copying it. There, tolerant `Value` lookups
// are the answer because the schema is undocumented and changes field names between its own
// lines. Here the schema *is* [`TranscriptEvent`], written by this same binary, so a line either
// deserializes as one of its variants or is not grind's at all; a typed read is the honest one,
// and a line that fails it costs itself and nothing else — the same per-line degradation rule.
//
// One field stays *could not observe* on purpose. `fanout` has nothing to report because the
// native loop has no fan-out tool to spawn a subagent with (the plan defers subagents to a later
// unit), and saying so is the answer rather than a gap — `Absent` in this view means *spawned,
// and every one returned*, which is a different and false claim.

/// Reason the fan-out field carries on every native Run. A named constant because it is a
/// standing fact about the loop, not a placeholder for a reader nobody wrote.
const NO_FANOUT: &str = "the native loop spawns no subagents";

/// The events one transcript file holds, in order, skipping what does not parse.
fn events(text: &str) -> Vec<TranscriptEvent> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<TranscriptEvent>(line).ok())
        .collect()
}

/// What an event said, as one capped line — or `None` for the events carrying no prose: usage
/// rows and the two selection records are facts about the harness, not anything the Run narrated.
fn said(event: &TranscriptEvent) -> Option<String> {
    match event {
        TranscriptEvent::Final { text } => Some(one_line(text)),
        TranscriptEvent::ProtocolNudge { assistant_text } => Some(one_line(assistant_text)),
        // A working attempt's assistant turns *are* tool calls, so this is most of what there is
        // to read while a Run is still going. Name and arguments, through the same one-line cap
        // as everything else on a fixed-shape view.
        TranscriptEvent::AssistantToolCalls { calls } => Some(one_line(
            &calls
                .iter()
                .map(|call| format!("{} {}", call.name, call.arguments))
                .collect::<Vec<String>>()
                .join("; "),
        )),
        TranscriptEvent::ToolResult { output, .. } => Some(one_line(output)),
        TranscriptEvent::Usage(_)
        | TranscriptEvent::ProtocolSelected { .. }
        | TranscriptEvent::SkillDeclared { .. } => None,
    }
}

/// Whether the assistant itself authored this event — which is what *doing* answers.
fn is_assistant(event: &TranscriptEvent) -> bool {
    matches!(
        event,
        TranscriptEvent::Final { .. }
            | TranscriptEvent::ProtocolNudge { .. }
            | TranscriptEvent::AssistantToolCalls { .. }
    )
}

/// *Could not observe*, with the event count in the reason — or `Absent` where the transcript
/// held nothing to recognise in the first place. `claude::nothing_recognised`'s rule, for the
/// same reason: a transcript full of events with no recognised row is a reader that has gone
/// stale, and reading that as `Absent` is indistinguishable from a Run with nothing to show.
fn nothing_recognised<T>(found: &[TranscriptEvent], what: &str) -> Observed<T> {
    if found.is_empty() {
        return Observed::Absent;
    }
    Observed::Unobservable(Reason::saying(&format!(
        "{} event{} in the transcript and no `{what}`",
        found.len(),
        if found.len() == 1 { "" } else { "s" }
    )))
}

/// The last thing the assistant itself said, one line: *what is it doing right now* (#82).
///
/// **Tool calls count as something said.** A native attempt's assistant turns are tool calls
/// until the very last one, so reading only [`TranscriptEvent::Final`] would leave this blank
/// for the entire window a human is watching — the exact window the field exists for. Like
/// everything in this view it describes what happened and is never a verdict input (ADR-0003).
pub fn assistant_now(text: &str) -> Observed<String> {
    let found = events(text);
    match found.iter().rev().find(|e| is_assistant(e)).and_then(said) {
        Some(line) => Observed::Present(line),
        None => nothing_recognised(&found, "assistant event"),
    }
}

/// Which rung's skill this attempt is running, from the [`TranscriptEvent::SkillDeclared`] row
/// the loop writes before its first request. The last one wins, the same rule
/// `claude::now_skill` gives `attributionSkill`.
pub fn now_skill(text: &str) -> Observed<String> {
    let found = events(text);
    let last = found.iter().rev().find_map(|event| match event {
        TranscriptEvent::SkillDeclared { skill } => Some(skill.clone()),
        _ => None,
    });
    match last {
        Some(skill) => Observed::Present(skill),
        None => nothing_recognised(&found, "skill_declared"),
    }
}

/// The last-words block, fixed at exactly `wanted` lines so `watch -n 30` never jitters —
/// `claude::last_words`' own rule, over this format's events. Tool results are in it for the
/// same reason the claude-code reader takes every message and not only the assistant's: what
/// came back is half of what a human reads to see whether a Run is getting anywhere.
pub fn last_words(text: &str, wanted: usize) -> Vec<String> {
    let said: Vec<String> = events(text).iter().filter_map(said).collect();
    let start = said.len().saturating_sub(wanted);
    let mut block: Vec<String> = said[start..].to_vec();
    while block.len() < wanted {
        block.push(String::new());
    }
    block
}

/// Shared by every field a transcript that could not be read cannot answer.
fn unread<T>() -> Observed<T> {
    Observed::Unobservable(Reason::saying("the transcript could not be read"))
}

/// A native Run's [`Live`], read off the transcripts under the Run's own directory.
///
/// **The newest-written transcript is the one read.** Each attempt writes its own file and
/// Reflect writes one more, so *what is it doing now* is whichever was touched last — not the
/// highest attempt number (Reflect runs after the ladder), and not all of them concatenated,
/// which would report attempt 1's last words for the rest of the Run's life. Freshness still
/// spans every file, through the same [`observe::native_freshness`] the floor this replaces used.
///
/// `world` supplies the bytes and the times; every field is decided by the pure readers above,
/// so each of them is testable from literals with no filesystem.
pub fn live(run_dir: &Path, now_epoch: u64) -> Live {
    let stamped: Vec<(PathBuf, Option<SystemTime>)> = world::list_with_extension(run_dir, "jsonl")
        .into_iter()
        .map(|path| {
            let at = world::mtime(&path);
            (path, at)
        })
        .collect();
    // `None` sorts below `Some`, so a file whose mtime could not be read is still the one read
    // when it is the only file there, and never wins over one that does carry a time. The path
    // breaks a tie, so two files stamped the same second pick the same one every refresh.
    let newest = stamped
        .iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    let mtimes: Vec<SystemTime> = stamped.iter().filter_map(|(_, at)| *at).collect();
    let text = newest.and_then(|(path, _)| world::read_to_string(path).ok());
    Live {
        transcript: match newest {
            Some((path, _)) => path.clone(),
            // Nothing written yet — name where it looked, so the panel still points somewhere.
            None => run_dir.to_path_buf(),
        },
        now_skill: match &text {
            Some(body) => now_skill(body),
            None => unread(),
        },
        assistant_now: match &text {
            Some(body) => assistant_now(body),
            None => unread(),
        },
        last_words: match &text {
            Some(body) => last_words(body, 3),
            // Still exactly three lines: an unreadable transcript must not change the shape of
            // the view (`claude::live`'s own rule).
            None => vec![String::new(); 3],
        },
        fanout: Observed::Unobservable(Reason::saying(NO_FANOUT)),
        freshness: observe::native_freshness(&mtimes, now_epoch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- the live view, from literals ------------------------------------------------------

    /// One attempt's transcript as `NativeAdapter` appends it: the skill row, the wire latch,
    /// then the tool-call / tool-result alternation a working attempt consists of.
    const TRANSCRIPT: &str = concat!(
        r#"{"event":"skill_declared","value":{"skill":"work"}}"#,
        "\n",
        r#"{"event":"protocol_selected","value":{"mode":"text","reason":"declared"}}"#,
        "\n",
        r#"{"event":"assistant_tool_calls","value":{"calls":[{"name":"read_file","arguments":"{\"path\":\"src/lib.rs\"}"}]}}"#,
        "\n",
        r#"{"event":"tool_result","value":{"call_id":"text","output":"pub mod job;"}}"#,
        "\n",
        r#"{"event":"usage","value":{"total_tokens":812}}"#,
        "\n",
        r#"{"event":"assistant_tool_calls","value":{"calls":[{"name":"bash","arguments":"{\"command\":\"just verify\"}"}]}}"#,
        "\n",
        r#"{"event":"tool_result","value":{"call_id":"text","output":"all green"}}"#,
        "\n",
    );

    #[test]
    fn the_declared_skill_comes_off_the_prompt_s_own_frontmatter() {
        // The real shape: a `skills/run/<stage>/SKILL.md` verbatim, then the context block the
        // stage composition appends after a `---` separator.
        let prompt = "---\nname: work\ndescription: The fourth rung.\n---\n\n# Work\n\nDo it.\n\n\
                      ---\n\nname: not-the-skill\n";
        assert_eq!(declared_skill(prompt), Some("work".to_string()));
    }

    #[test]
    fn a_prompt_declaring_no_name_declares_nothing_rather_than_a_blank() {
        // Four ways to have no name, none of which may produce `Some("")` — an empty skill
        // would render as an answered `now` line saying nothing at all.
        assert_eq!(declared_skill(""), None);
        assert_eq!(declared_skill("# Work\n\nno frontmatter here\n"), None);
        assert_eq!(declared_skill("---\ndescription: no name row\n---\n"), None);
        assert_eq!(declared_skill("---\nname:   \n---\n"), None);
    }

    #[test]
    fn a_name_row_after_the_frontmatter_closes_is_not_the_skill() {
        // The composed prompt uses `---` as its own separator, so a reader that scanned the
        // whole text would start reading the context block's prose as frontmatter.
        assert_eq!(
            declared_skill("---\ndescription: only this\n---\n\nname: prose\n"),
            None
        );
    }

    #[test]
    fn now_skill_reads_the_declared_row_and_the_last_one_wins() {
        assert_eq!(now_skill(TRANSCRIPT), Observed::Present("work".to_string()));
        let two = format!(
            "{TRANSCRIPT}{}\n",
            r#"{"event":"skill_declared","value":{"skill":"ship"}}"#
        );
        assert_eq!(now_skill(&two), Observed::Present("ship".to_string()));
    }

    #[test]
    fn doing_is_the_last_thing_the_assistant_authored_and_a_tool_call_counts() {
        // A native attempt's assistant turns *are* tool calls until the very last one, so
        // reading only `Final` would leave this blank for the whole window a human watches.
        // The newer tool *result* does not win it: that is the world talking, not the model.
        assert_eq!(
            assistant_now(TRANSCRIPT),
            Observed::Present(r#"bash {"command":"just verify"}"#.to_string())
        );
    }

    #[test]
    fn a_final_answer_wins_doing_once_it_lands() {
        let done = format!(
            "{TRANSCRIPT}{}\n",
            r#"{"event":"final","value":{"text":"twelve commits, PR open"}}"#
        );
        assert_eq!(
            assistant_now(&done),
            Observed::Present("twelve commits, PR open".to_string())
        );
    }

    #[test]
    fn a_protocol_nudge_is_something_the_assistant_said() {
        // Drift prose is what the Run is doing at that moment, and it is exactly what an
        // operator needs to see — a text-latched model that has stopped emitting tags.
        let nudged = concat!(
            r#"{"event":"skill_declared","value":{"skill":"plan"}}"#,
            "\n",
            r#"{"event":"protocol_nudge","value":{"assistant_text":"Let me think about this."}}"#,
            "\n",
        );
        assert_eq!(
            assistant_now(nudged),
            Observed::Present("Let me think about this.".to_string())
        );
    }

    #[test]
    fn last_words_is_exactly_three_lines_whatever_the_transcript_said() {
        // The block's height is fixed so `watch -n 30` never jitters — the same rule
        // `claude::last_words` carries, over this format's events.
        assert_eq!(
            last_words(TRANSCRIPT, 3),
            vec![
                "pub mod job;".to_string(),
                r#"bash {"command":"just verify"}"#.to_string(),
                "all green".to_string(),
            ]
        );
        assert_eq!(last_words("", 3), vec![String::new(); 3]);
        assert_eq!(last_words(TRANSCRIPT, 1), vec!["all green".to_string()]);
    }

    #[test]
    fn usage_and_the_two_selection_rows_are_not_words() {
        // They are facts about the harness, not anything the Run narrated — a `last words`
        // block reading `{"total_tokens":812}` tells an operator nothing about the work.
        let only_harness = concat!(
            r#"{"event":"usage","value":{"total_tokens":812}}"#,
            "\n",
            r#"{"event":"protocol_selected","value":{"mode":"native","reason":"probe"}}"#,
            "\n",
            r#"{"event":"skill_declared","value":{"skill":"work"}}"#,
            "\n",
        );
        assert_eq!(last_words(only_harness, 3), vec![String::new(); 3]);
        // …and the three of them present is *could not observe* for `doing`, never `Absent`:
        // events were recognised, just none the assistant authored.
        let found = assistant_now(only_harness);
        assert!(matches!(found, Observed::Unobservable(_)), "{found:?}");
    }

    #[test]
    fn a_transcript_of_nothing_recognisable_is_absent_and_a_bad_line_costs_only_itself() {
        // Nothing recognised at all is `Absent`; one unparseable line among good ones costs
        // its own values and no sibling's — the per-line degradation `claude` established.
        assert_eq!(assistant_now(""), Observed::Absent);
        assert_eq!(
            now_skill("not json\n{\"event\":\"turn\"}\n"),
            Observed::Absent
        );
        let holed = format!("not json at all\n{TRANSCRIPT}");
        assert_eq!(now_skill(&holed), Observed::Present("work".to_string()));
    }

    #[test]
    fn live_reads_the_newest_written_transcript_and_names_the_file() {
        // Each attempt writes its own file and Reflect writes one more, so *what is it doing
        // now* is whichever was touched last — never all of them concatenated, which would
        // report attempt 1's last words for the rest of the Run's life.
        let dir = world::temp_dir("native-live");
        world::write(
            &dir.join("messages-1.jsonl"),
            r#"{"event":"skill_declared","value":{"skill":"plan"}}"#,
        )
        .expect("attempt 1's transcript");
        world::write(&dir.join("messages-2.jsonl"), TRANSCRIPT).expect("attempt 2's transcript");

        let live = live(&dir, world::now_epoch());
        assert_eq!(live.transcript, dir.join("messages-2.jsonl"));
        assert_eq!(live.now_skill, Observed::Present("work".to_string()));
        assert!(matches!(live.freshness, Observed::Present(_)));
        // Fan-out is a standing fact about the loop, not an unfinished reader.
        match live.fanout {
            Observed::Unobservable(reason) => assert_eq!(reason.to_string(), NO_FANOUT),
            other => panic!("fan-out must stay could-not-observe: {other:?}"),
        }

        world::remove_tree(&dir);
    }

    #[test]
    fn live_over_a_run_directory_with_no_transcript_yet_names_the_directory() {
        // A Run whose first attempt has not written yet must still point the panel somewhere,
        // and freshness must read *could not observe* rather than *just wrote*.
        let dir = world::temp_dir("native-live-empty");
        let live = live(&dir, world::now_epoch());
        assert_eq!(live.transcript, dir);
        assert!(matches!(live.freshness, Observed::Unobservable(_)));
        assert_eq!(live.last_words, vec![String::new(); 3]);
        world::remove_tree(&dir);
    }

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

    #[test]
    fn scan_latch_orders_by_attempt_number_not_lexicographic_path() {
        // A lexicographic sort of filenames puts "messages-10.jsonl" before
        // "messages-2.jsonl". Attempt 2 selected Text; attempt 10 (a later,
        // still-unlatched attempt under the old bug's premise) selects Native.
        // Numeric order must read attempt 2 first and let attempt 10 win as the
        // last ProtocolSelected.
        let dir = world::temp_dir("native-scan-latch-order");
        world::write(
            &dir.join("messages-2.jsonl"),
            "{\"event\":\"protocol_selected\",\"value\":{\"mode\":\"text\",\"reason\":\"r\"}}\n",
        )
        .expect("write attempt 2");
        world::write(
            &dir.join("messages-10.jsonl"),
            "{\"event\":\"protocol_selected\",\"value\":{\"mode\":\"native\",\"reason\":\"r\"}}\n",
        )
        .expect("write attempt 10");

        assert_eq!(scan_latch(&dir, 11), Some(ProtoMode::Native));

        world::remove_tree(&dir);
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
    fn abnormal_finish_naming_tools_latches_but_text_never_does() {
        let err = NetError::AbnormalFinish {
            finish: "tool_calls".into(),
            native: None,
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
    fn abnormal_finish_for_an_ordinary_length_cutoff_backs_off_instead_of_latching() {
        // finish_reason="length" (max tokens) names nothing about tools; latching
        // Text on it would pin the whole Run to text on the strength of one long
        // reply (P1 regression).
        let err = NetError::AbnormalFinish {
            finish: "length".into(),
            native: None,
        };
        assert_eq!(
            next_action(&err, ProtoMode::Native, false, 0, 3, 2),
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
            usage: Some(json!({"prompt_tokens": 10, "cost": 0.03183387})),
            denials: vec![],
        });
        assert_eq!(attempt.exit_code, Some(0));
        assert!(!attempt.is_error);
        assert!(attempt.parse_ok);
        assert_eq!(attempt.subtype, None);
        assert!(attempt.done_promise);
        assert!(!attempt.rate_limited);
        assert_eq!(attempt.total_cost_usd, Some(0.031_833_87));
        assert_eq!(attempt.num_turns, Some(7));
        assert_eq!(attempt.usage.as_ref().unwrap()["prompt_tokens"], 10);
        assert_eq!(attempt.result_tail, "shipped it");
        assert!(matches!(attempt.fanout, Observed::Unobservable(_)));
    }

    #[test]
    fn a_single_turn_rate_limited_native_attempt_is_a_wait() {
        // total_cost_usd must be the honest Some(0.0), not None, or is_wait() is
        // false for every native Attempt regardless of num_turns (P1 regression).
        let attempt = synthesize(AttemptFacts {
            n: 1,
            mode: Mode::Dispatch,
            started_at: "s",
            ended_at: "e",
            turns_used: 1,
            ending: Ending::Failed("HTTP 429: rate limit exceeded, resets at 17:00".into()),
            usage: None,
            denials: vec![],
        });
        assert!(attempt.rate_limited);
        assert!(
            attempt.is_wait(),
            "a first-turn rate limit must not spend the attempt budget"
        );
    }

    #[test]
    fn an_attempt_that_spent_is_never_a_wait() {
        // The property the old hardcoded Some(0.0) protected, stated positively: real
        // spend means the model answered, so the Attempt did work even at one turn.
        let attempt = synthesize(AttemptFacts {
            n: 1,
            mode: Mode::Dispatch,
            started_at: "s",
            ended_at: "e",
            turns_used: 1,
            ending: Ending::Completed("shipped it".into()),
            usage: Some(json!({"prompt_tokens": 10, "cost": 0.03183387})),
            denials: vec![],
        });
        assert_eq!(attempt.total_cost_usd, Some(0.031_833_87));
        assert!(!attempt.is_wait());
    }

    #[test]
    fn a_multi_turn_completed_native_attempt_is_never_a_wait() {
        let attempt = synthesize(AttemptFacts {
            n: 1,
            mode: Mode::Dispatch,
            started_at: "s",
            ended_at: "e",
            turns_used: 5,
            ending: Ending::Completed("shipped it".into()),
            usage: Some(json!({"prompt_tokens": 10})),
            denials: vec![],
        });
        assert!(!attempt.is_wait());
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

    // --- denial identity (R7) ----------------------------------------------------

    #[test]
    fn a_denied_bash_call_carries_the_vocabulary_policy_and_render_read() {
        let report = tools::GateReport {
            layer: GateLayer::DeniedGlob,
            tool: "bash".to_string(),
            reason: "matched a denied-tool glob".to_string(),
        };
        let denial = denial_json(&report, r#"{"command":"git push --force origin main"}"#);
        assert_eq!(denial["tool_name"], "bash");
        assert_eq!(
            denial["tool_input"]["command"],
            "git push --force origin main"
        );
        assert_eq!(denial["gate_layer"], "denied_glob");
        assert_eq!(denial["reason"], "matched a denied-tool glob");
    }

    // --- usage accumulation (R8) --------------------------------------------------

    #[test]
    fn usage_accumulates_across_turns_instead_of_keeping_only_the_last() {
        let first = json!({"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15});
        let second = json!({"prompt_tokens": 7, "completion_tokens": 3, "total_tokens": 10});
        let total = accumulate_usage(Some(accumulate_usage(None, &first)), &second);
        assert_eq!(total["prompt_tokens"], 17);
        assert_eq!(total["completion_tokens"], 8);
        assert_eq!(total["total_tokens"], 25);
    }

    #[test]
    fn usage_accumulation_sums_nested_numeric_leaves_too() {
        let first = json!({"prompt_tokens": 10, "prompt_tokens_details": {"cached_tokens": 2}});
        let second = json!({"prompt_tokens": 4, "prompt_tokens_details": {"cached_tokens": 1}});
        let total = accumulate_usage(Some(first), &second);
        assert_eq!(total["prompt_tokens"], 14);
        assert_eq!(total["prompt_tokens_details"]["cached_tokens"], 3);
    }

    // --- text-latch system-prompt timing (P2) -------------------------------------

    #[test]
    fn install_system_replaces_only_the_leading_message_with_the_new_modes_prompt() {
        let mut messages = vec![
            json!({"role": "system", "content": "native prompt"}),
            json!({"role": "user", "content": "go"}),
        ];
        install_system(
            &mut messages,
            ProtoMode::Text,
            |mode| json!({"role": "system", "content": format!("prompt for {mode:?}")}),
        );
        assert_eq!(messages[0]["content"], "prompt for Text");
        assert_eq!(messages[1]["content"], "go", "only index 0 changes");
    }

    // `install_system`'s call site — the `Action::LatchText` arm of `drive_turn` — is
    // reached only through `next_action`, whose own coverage above
    // (`tools_array_rejection_latches_text_immediately_even_at_the_last_failure`,
    // `abnormal_finish_naming_tools_latches_but_text_never_does`) already pins down
    // exactly when a latch fires. Together the two prove the fix: the prompt swap
    // happens (this test), and it happens on precisely the latch action next_action
    // decided on (that coverage) — with neither test opening a socket.

    // --- transcript newline framing -----------------------------------------------

    #[test]
    fn transcript_log_writes_exactly_one_newline_per_line() {
        let dir = world::temp_dir("native-transcript-newline");
        let transcript = Transcript {
            path: dir.join("messages-1.jsonl"),
            attempt_n: 1,
            append_failed: std::cell::RefCell::new(None),
        };
        transcript.log(&TranscriptEvent::Final {
            text: "first".to_string(),
        });
        transcript.log(&TranscriptEvent::Final {
            text: "second".to_string(),
        });
        let contents = world::read_to_string(&transcript.path).expect("read transcript");
        assert_eq!(
            contents.matches('\n').count(),
            2,
            "one newline per line, no more"
        );
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("first") && !lines[0].is_empty());
        assert!(lines[1].contains("second"));
        world::remove_tree(&dir);
    }

    #[test]
    fn truncate_before_log_starts_the_attempt_from_empty_not_appended() {
        // Simulates a crashed attempt N followed by a resumed attempt that
        // recomputes the same N: the second `Transcript` open for that same
        // path must start empty rather than appending after the first's
        // partial content (finding #23).
        let dir = world::temp_dir("native-transcript-truncate");
        let path = dir.join("messages-1.jsonl");

        let dead = Transcript {
            path: path.clone(),
            attempt_n: 1,
            append_failed: std::cell::RefCell::new(None),
        };
        dead.truncate();
        dead.log(&TranscriptEvent::Final {
            text: "partial from a crashed attempt".to_string(),
        });

        let retry = Transcript {
            path: path.clone(),
            attempt_n: 1,
            append_failed: std::cell::RefCell::new(None),
        };
        retry.truncate();
        retry.log(&TranscriptEvent::Final {
            text: "the retry's own event".to_string(),
        });

        let contents = world::read_to_string(&path).expect("read transcript");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(
            lines.len(),
            1,
            "truncate must drop the dead attempt's content, not append after it"
        );
        assert!(lines[0].contains("the retry's own event"));
        assert!(!contents.contains("partial from a crashed attempt"));

        world::remove_tree(&dir);
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
