//! The agent harness's one sanctioned network region (ADR-0016): an OpenAI-compatible
//! `/chat/completions` client over `ureq`, plus the SSE stream parser.
//!
//! Owned by the wave-1 net slice. No retry logic lives here — callers own retries
//! and decide what a failure means.

use std::collections::BTreeMap;
use std::io::BufRead;
use std::io::BufReader;
use std::time::Duration;

use serde_json::Value;

use crate::runner::Endpoint;

/// One pooled sync client. Connect timeout 30s; NO overall timeout — a streaming
/// response may legitimately run minutes.
pub struct ChatClient {
    agent: ureq::Agent,
}

impl ChatClient {
    pub fn new() -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(30))
            .build();
        Self { agent }
    }

    /// POST one chat completion and consume the SSE stream into an assembled turn.
    pub fn post_chat(&self, ep: &Endpoint, body: &Value) -> Result<ChatTurn, NetError> {
        let url = format!("{}/chat/completions", ep.base_url.trim_end_matches('/'));
        let resp = self
            .agent
            .post(&url)
            .set("Authorization", &format!("Bearer {}", ep.api_key))
            .set("Content-Type", "application/json")
            .send_string(&body.to_string())
            .map_err(|e| match e {
                ureq::Error::Status(status, resp) => NetError::Http {
                    status,
                    body: truncate_body(&resp.into_string().unwrap_or_default()),
                },
                ureq::Error::Transport(t) => NetError::Stream(format!("request failed: {t}")),
            })?;

        let status = resp.status();
        if !(200..300).contains(&status) {
            let raw = resp.into_string().unwrap_or_default();
            return Err(NetError::Http {
                status,
                body: truncate_body(&raw),
            });
        }

        let mut assembler = SseAssembler::new();
        let mut reader = BufReader::new(resp.into_reader());
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader
                .read_line(&mut line)
                .map_err(|e| NetError::Stream(format!("stream read failed: {e}")))?;
            if n == 0 {
                break; // server closed; some providers skip [DONE]
            }
            let line = line.trim_end_matches(['\n', '\r']);
            match classify_sse_line(line) {
                SseLine::Keepalive | SseLine::Ignored => continue,
                SseLine::Done => break,
                SseLine::Data(payload) => {
                    let chunk: Value = serde_json::from_str(payload)
                        .map_err(|e| NetError::Stream(format!("bad chunk: {e}: {payload}")))?;
                    assembler.feed_chunk(&chunk)?;
                }
            }
        }
        assembler.finish()
    }
}

impl Default for ChatClient {
    fn default() -> Self {
        Self::new()
    }
}

/// One assembled request/response cycle.
#[derive(Clone, Debug)]
pub struct ChatTurn {
    pub content: String,
    /// Tool calls assembled from deltas, ordered by index.
    pub tool_calls: Vec<ToolCallSpec>,
    pub usage: Option<Value>,
    pub finish_reason: Option<String>,
    /// OpenRouter's pass-through of the upstream's real reason — validated (R6)
    /// because upstream failures hide behind `finish_reason: "stop"`.
    pub native_finish_reason: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ToolCallSpec {
    pub id: String,
    pub name: String,
    /// Raw JSON arguments text as streamed (concatenated across deltas).
    pub arguments_json: String,
}

#[derive(Debug, PartialEq)]
pub enum NetError {
    /// Non-2xx status; body truncated for the record.
    Http { status: u16, body: String },
    /// Stream read/parse failure or an in-stream error chunk.
    Stream(String),
    /// finish_reason / native_finish_reason outside the legitimate endings (R6).
    AbnormalFinish {
        finish: String,
        native: Option<String>,
    },
    /// Neither content nor tool calls arrived — never a clean stop (R6).
    EmptyResponse,
}

/// Connection-level reachability probe for doctor (R9): GET `{base}/models`;
/// true iff the endpoint answered at all (any status counts), false on connect error.
pub fn probe_endpoint(ep: &Endpoint) -> bool {
    let url = format!("{}/models", ep.base_url.trim_end_matches('/'));
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(5))
        .build();
    match agent.get(&url).call() {
        Ok(_) => true,
        Err(ureq::Error::Status(_, _)) => true,
        Err(ureq::Error::Transport(_)) => false,
    }
}

// ---------------------------------------------------------------------------
// SSE parsing — pure pieces so edge cases are testable without sockets.
// ---------------------------------------------------------------------------

/// Classification of one raw SSE line.
#[derive(Debug, PartialEq)]
pub enum SseLine<'a> {
    /// Blank line or `:` comment — SSE keepalive, skipped.
    Keepalive,
    /// The literal `[DONE]` terminator.
    Done,
    /// A `data: ` payload carrying one JSON chunk.
    Data(&'a str),
    /// Anything else (no `data: ` prefix) — ignored.
    Ignored,
}

/// Classify a single newline-stripped SSE line (POC semantics verbatim).
pub fn classify_sse_line(line: &str) -> SseLine<'_> {
    if line.is_empty() || line.starts_with(':') {
        // SSE comment / keepalive (OpenRouter sends ": OPENROUTER PROCESSING")
        return SseLine::Keepalive;
    }
    if let Some(payload) = line.strip_prefix("data: ") {
        if payload.trim() == "[DONE]" {
            return SseLine::Done;
        }
        return SseLine::Data(payload);
    }
    SseLine::Ignored
}

/// Failure raised while feeding one chunk into [`SseAssembler`].
#[derive(Debug, PartialEq)]
pub enum StreamFailure {
    /// An in-stream error chunk (`{"error": …}`).
    Stream(String),
    /// finish_reason / native_finish_reason outside the legitimate endings (R6).
    AbnormalFinish {
        finish: String,
        native: Option<String>,
    },
}

impl From<StreamFailure> for NetError {
    fn from(f: StreamFailure) -> Self {
        match f {
            StreamFailure::Stream(msg) => NetError::Stream(msg),
            StreamFailure::AbnormalFinish { finish, native } => {
                NetError::AbnormalFinish { finish, native }
            }
        }
    }
}

/// Incremental SSE chunk assembler: accumulates delta content, tool-call deltas
/// (by index, id-overwrite + name/args CONCATENATION), usage, and finish reasons;
/// validates the ending and guards against empty responses.
///
/// EOF without `[DONE]` is legal — just call [`SseAssembler::finish`] after the
/// read loop ends.
#[derive(Debug, Default)]
pub struct SseAssembler {
    content: String,
    pending: BTreeMap<u64, ToolCallSpec>,
    usage: Option<Value>,
    finish_reason: Option<String>,
    native_finish_reason: Option<String>,
    done: bool,
}

impl SseAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one parsed chat-completion chunk into the turn under assembly.
    pub fn feed_chunk(&mut self, chunk: &Value) -> Result<(), StreamFailure> {
        if let Some(err) = chunk.get("error").filter(|e| !e.is_null()) {
            return Err(StreamFailure::Stream(format!("stream error chunk: {err}")));
        }
        if let Some(u) = chunk.get("usage").filter(|u| !u.is_null()) {
            self.usage = Some(u.clone());
        }
        let Some(choice) = chunk["choices"].get(0) else {
            return Ok(());
        };
        if let Some(c) = choice["delta"]["content"].as_str() {
            self.content.push_str(c);
        }
        if let Some(tcs) = choice["delta"]["tool_calls"].as_array() {
            for tc in tcs {
                let idx = tc["index"].as_u64().unwrap_or(0);
                let entry = self.pending.entry(idx).or_default();
                if let Some(id) = tc["id"].as_str() {
                    entry.id = id.to_string(); // overwrite: later chunks carry the canonical id
                }
                if let Some(name) = tc["function"]["name"].as_str() {
                    entry.name.push_str(name); // some providers split names across chunks
                }
                if let Some(a) = tc["function"]["arguments"].as_str() {
                    entry.arguments_json.push_str(a);
                }
            }
        }
        if let Some(fr) = choice["finish_reason"].as_str() {
            // Keep reading past finish (usage usually follows), but refuse anything
            // but the two legitimate endings. OpenRouter maps upstream provider
            // failures to finish_reason="stop" with an empty delta and stashes the
            // real cause in native_finish_reason.
            self.finish_reason = Some(fr.to_string());
            self.native_finish_reason = choice["native_finish_reason"].as_str().map(str::to_string);
            let ok = matches!(fr, "stop" | "tool_calls");
            let native_ok = matches!(
                self.native_finish_reason.as_deref(),
                None | Some("stop") | Some("tool_calls") | Some("end_turn") | Some("stop_sequence")
            );
            if !ok || !native_ok {
                return Err(StreamFailure::AbnormalFinish {
                    finish: fr.to_string(),
                    native: self.native_finish_reason.clone(),
                });
            }
        }
        Ok(())
    }

    /// Mark the stream terminated by `[DONE]`.
    pub fn done(&mut self) {
        self.done = true;
    }

    /// Validate the ending and produce the assembled turn. The empty guard fires
    /// regardless of how the stream ended — never a clean stop (R6).
    pub fn finish(self) -> Result<ChatTurn, NetError> {
        let _ = self.done; // EOF without [DONE] is legal; recorded for parity only
        if self.pending.is_empty() && self.content.trim().is_empty() {
            return Err(NetError::EmptyResponse);
        }
        Ok(ChatTurn {
            content: self.content,
            tool_calls: self.pending.into_values().collect(),
            usage: self.usage,
            finish_reason: self.finish_reason,
            native_finish_reason: self.native_finish_reason,
        })
    }
}

/// Char-boundary-safe truncation of HTTP bodies kept for the record.
fn truncate_body(raw: &str) -> String {
    const MAX: usize = 512;
    if raw.len() <= MAX {
        return raw.to_string();
    }
    let mut end = MAX;
    while !raw.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[truncated {} bytes]", &raw[..end], raw.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn feed_lines(lines: &[&str]) -> Result<SseAssembler, NetError> {
        let mut asm = SseAssembler::new();
        for line in lines {
            match classify_sse_line(line) {
                SseLine::Keepalive | SseLine::Ignored => {}
                SseLine::Done => asm.done(),
                SseLine::Data(payload) => {
                    let chunk: Value =
                        serde_json::from_str(payload).expect("test chunk is valid JSON");
                    asm.feed_chunk(&chunk)?;
                }
            }
        }
        Ok(asm)
    }

    #[test]
    fn split_name_chunks_concatenate() {
        let mut asm = SseAssembler::new();
        asm.feed_chunk(&json!({
            "choices": [{"delta": {"tool_calls": [
                {"index": 0, "id": "call_1", "function": {"name": "rea", "arguments": ""}}
            ]}}]
        }))
        .unwrap();
        asm.feed_chunk(&json!({
            "choices": [{"delta": {"tool_calls": [
                {"index": 0, "function": {"name": "d_file", "arguments": "{\"p\""}}
            ]}}]
        }))
        .unwrap();
        asm.feed_chunk(&json!({
            "choices": [{"delta": {"tool_calls": [
                {"index": 0, "function": {"arguments": ": 1}"}}
            ]}}]
        }))
        .unwrap();
        let turn = asm.finish().unwrap();
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].name, "read_file");
        assert_eq!(turn.tool_calls[0].arguments_json, "{\"p\": 1}");
        assert_eq!(turn.tool_calls[0].id, "call_1");
    }

    #[test]
    fn out_of_order_indices_assemble_sorted() {
        let mut asm = SseAssembler::new();
        asm.feed_chunk(&json!({
            "choices": [{"delta": {"tool_calls": [
                {"index": 2, "id": "c", "function": {"name": "write_file", "arguments": "{}"}}
            ]}}]
        }))
        .unwrap();
        asm.feed_chunk(&json!({
            "choices": [{"delta": {"tool_calls": [
                {"index": 0, "id": "a", "function": {"name": "bash", "arguments": "{"}},
                {"index": 1, "id": "b", "function": {"name": "read_file", "arguments": "{}"}}
            ]}}]
        }))
        .unwrap();
        asm.feed_chunk(&json!({
            "choices": [{"delta": {"tool_calls": [
                {"index": 0, "function": {"arguments": "\"ls\""}}
            ]}}]
        }))
        .unwrap();
        let turn = asm.finish().unwrap();
        let names: Vec<&str> = turn.tool_calls.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["bash", "read_file", "write_file"]);
        assert_eq!(turn.tool_calls[0].arguments_json, "{\"ls\"");
    }

    #[test]
    fn comment_keepalives_and_blank_lines_are_skipped() {
        let asm = feed_lines(&[
            ": OPENROUTER PROCESSING",
            "",
            ": keepalive",
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}",
            "",
            "data: [DONE]",
        ])
        .unwrap();
        let turn = asm.finish().unwrap();
        assert_eq!(turn.content, "hi");
        assert_eq!(turn.finish_reason, None);
    }

    #[test]
    fn lines_without_data_prefix_are_ignored() {
        let asm = feed_lines(&[
            "event: ping",
            "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}",
            "data: [DONE]",
        ])
        .unwrap();
        assert_eq!(asm.finish().unwrap().content, "x");
    }

    #[test]
    fn abnormal_native_finish_reason_is_rejected() {
        let err = feed_lines(&[
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\",\"native_finish_reason\":\"length\"}]}",
        ])
        .unwrap_err();
        assert_eq!(
            err,
            NetError::AbnormalFinish {
                finish: "stop".into(),
                native: Some("length".into()),
            }
        );
    }

    #[test]
    fn abnormal_finish_reason_is_rejected() {
        let err = feed_lines(&[
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"error\",\"native_finish_reason\":\"stop\"}]}",
        ])
        .unwrap_err();
        assert_eq!(
            err,
            NetError::AbnormalFinish {
                finish: "error".into(),
                native: Some("stop".into()),
            }
        );
    }

    #[test]
    fn legitimate_native_reasons_pass_validation() {
        for native in ["stop", "tool_calls", "end_turn", "stop_sequence"] {
            let mut asm = SseAssembler::new();
            asm.feed_chunk(&json!({
                "choices": [{
                    "delta": {"content": "ok"},
                    "finish_reason": "stop",
                    "native_finish_reason": native,
                }]
            }))
            .unwrap();
            let turn = asm.finish().unwrap();
            assert_eq!(turn.native_finish_reason.as_deref(), Some(native));
        }
    }

    #[test]
    fn mid_stream_error_chunk_yields_stream_error() {
        let err = feed_lines(&[
            "data: {\"choices\":[{\"delta\":{\"content\":\"par\"}}]}",
            "data: {\"error\":{\"message\":\"upstream exploded\",\"code\":502}}",
        ])
        .unwrap_err();
        assert!(matches!(&err, NetError::Stream(m) if m.contains("upstream exploded")));
    }

    #[test]
    fn null_error_and_usage_fields_do_not_trip_guards() {
        let asm = feed_lines(&[
            "data: {\"error\":null,\"usage\":null,\"choices\":[{\"delta\":{\"content\":\"v\"}}]}",
            "data: {\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2},\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}",
            "data: [DONE]",
        ])
        .unwrap();
        let turn = asm.finish().unwrap();
        assert_eq!(turn.content, "v");
        assert_eq!(turn.usage.unwrap()["prompt_tokens"], 3);
        assert_eq!(turn.finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn missing_done_marker_is_legal_eof() {
        // Server closes without [DONE]; keep reading past finish_reason happened.
        let asm = feed_lines(&[
            "data: {\"choices\":[{\"delta\":{\"content\":\"done text\"},\"finish_reason\":\"stop\"}]}",
            "data: {\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":4},\"choices\":[]}",
        ])
        .unwrap();
        let turn = asm.finish().unwrap();
        assert_eq!(turn.content, "done text");
        assert_eq!(turn.usage.unwrap()["completion_tokens"], 4);
        assert_eq!(turn.finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn empty_response_is_flagged() {
        // Whitespace-only content with no tool calls still counts as empty (R6).
        let asm = feed_lines(&[
            "data: {\"choices\":[{\"delta\":{\"content\":\"   \"},\"finish_reason\":\"stop\"}]}",
            "data: [DONE]",
        ])
        .unwrap();
        assert_eq!(asm.finish().unwrap_err(), NetError::EmptyResponse);

        // Truly empty stream too.
        assert_eq!(
            feed_lines(&["data: [DONE]"]).unwrap().finish().unwrap_err(),
            NetError::EmptyResponse
        );
    }

    #[test]
    fn usage_only_tail_keeps_reading_past_finish() {
        let asm = feed_lines(&[
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"t\",\"function\":{\"name\":\"bash\",\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}",
            "data: [DONE]",
        ])
        .unwrap();
        let turn = asm.finish().unwrap();
        assert_eq!(turn.tool_calls.len(), 1);
        assert!(turn.usage.is_some());
        assert_eq!(turn.finish_reason.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn truncate_body_respects_char_boundaries() {
        let short = truncate_body("hello");
        assert_eq!(short, "hello");

        let multibyte = "é".repeat(400); // 800 bytes, cuts mid-char at byte 512
        let truncated = truncate_body(&multibyte);
        assert!(truncated.contains("…[truncated 800 bytes]"));
        assert!(truncated.starts_with(&"é".repeat(256)));
    }
}
