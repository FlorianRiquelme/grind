//! The stage-runner seam (issue #135): exactly one boundary decides how a stage executes.
//!
//! Downstream consumers — supervisor, policy, observe, serve, render — see one currency
//! ([`crate::attempt::Attempt`]) and one transcript-event schema
//! ([`TranscriptEvent`]). Normalization happens inside adapters; nothing outside
//! this module branches on backend.
//!
//! Two adapters exist:
//!
//! - [`ClaudeCodeAdapter`] — today's behavior: `claude -p` argv via
//!   [`crate::world::spawn_recorded`], Claude Code transcripts under `~/.claude/projects/`.
//! - [`NativeAdapter`] — a grind-owned agent loop speaking any OpenAI-compatible
//!   `/chat/completions` endpoint, grind-defined tools, grind-owned transcripts
//!   (`messages-N.jsonl` in the run dir).
//!
//! Selection is layout-declared (`~/.grind/agent`, ADR-0017), snapshotted into the
//! RunRecord at dispatch, and honored verbatim on resume.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::attempt;

/// One tool invocation summary (name + raw arguments JSON).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallSummary {
    pub name: String,
    /// Raw arguments as received on the wire (JSON text).
    pub arguments: String,
}

/// A normalized transcript event (R4) — the ONE schema both adapters emit.
/// Claude Code JSONL is normalized into these variants inside the claude-code adapter;
/// the native adapter emits them directly as its loop runs. Lines land in
/// `messages-N.jsonl` under the run's own directory, one file per attempt mirroring the
/// `attempt-N.*` convention. Line shape is identical to the POC's log format:
///
/// ```json
/// {"event": "tool_result", "value": {"call_id": "...", "output": "..."}}
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", content = "value", rename_all = "snake_case")]
pub enum TranscriptEvent {
    /// The assistant invoked tools.
    AssistantToolCalls { calls: Vec<CallSummary> },
    /// A tool finished; `output` is already truncated to the context budget.
    ToolResult { call_id: String, output: String },
    /// Token usage from the provider (R8: spend is recorded, never bounded).
    Usage(serde_json::Value),
    /// Prose arrived where the protocol demanded a tag or a done sentinel;
    /// a corrective nudge was issued and sent. Logged every time so drift rates
    /// stay comparable across models in P3.
    ProtocolNudge { assistant_text: String },
    /// The wire mode was determined for this run (probe result or resumed latch).
    ProtocolSelected { mode: ProtoMode, reason: String },
    /// The attempt's final answer text (also carried by Attempt.result_tail).
    Final { text: String },
}

impl TranscriptEvent {
    /// Encode one event as a single JSONL line, **without** a trailing newline: the writer
    /// (`world::append_line`) already appends one via `writeln!`, so appending here too made
    /// every `messages-N.jsonl` line double-spaced. Both current readers filter blank lines,
    /// so nothing broke — but the format was wrong, and a future reader that does not filter
    /// would see spurious empty lines.
    pub fn encode(&self) -> String {
        serde_json::to_string(self).expect("TranscriptEvent serializes")
    }
}

/// Which adapter executes a stage. Serialized into the RunRecord at dispatch;
/// absent/legacy records deserialize as the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Backend {
    #[default]
    #[serde(rename = "claude-code")]
    ClaudeCode,
    #[serde(rename = "native")]
    Native,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Native => "native",
        }
    }

    /// Parse one `~/.grind/agent` backend token. Unknown names are a loud error,
    /// never a silent default.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "claude-code" => Ok(Self::ClaudeCode),
            "native" => Ok(Self::Native),
            other => Err(format!(
                "unknown agent backend {other:?} (expected \"claude-code\" or \"native\")"
            )),
        }
    }
}

/// Wire mode for the native adapter's tool calling (ADR-0018).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtoMode {
    /// OpenAI `tools` parameter with `tool_calls` deltas.
    Native,
    /// Tools described in the system prompt; `<tool>`/`<done>` sentinels in content.
    Text,
}

/// One `~/.grind/agent` line: `<backend>[ <base-url>]`. The base-url token is the
/// hermetic-test / self-hosting seam; credentials never appear here (env only).
#[derive(Clone, Debug, Default)]
pub struct Selection {
    pub backend: Backend,
    pub endpoint_override: Option<String>,
}

impl Selection {
    /// Parse the file's single line. An empty (or whitespace) line is the default.
    pub fn parse_line(line: &str) -> Result<Self, String> {
        let line = line.trim();
        if line.is_empty() {
            return Ok(Self::default());
        }
        let mut tokens = line.split_whitespace();
        let backend = Backend::parse(tokens.next().expect("non-empty line"))?;
        let extra = tokens.next();
        match (backend, extra, tokens.next()) {
            (_, None, _) => Ok(Self {
                backend,
                endpoint_override: None,
            }),
            (Backend::Native, Some(url), None) => Ok(Self {
                backend,
                endpoint_override: Some(url.to_string()),
            }),
            (Backend::ClaudeCode, Some(extra), _) => Err(format!(
                "claude-code takes no arguments, found {extra:?} \
                 (expected a bare \"claude-code\" line)"
            )),
            (Backend::Native, Some(_), Some(_)) => {
                Err("too many tokens on the agent line (expected `<backend>[ <base-url>]`)".into())
            }
        }
    }
}

/// Where one attempt's network conversation goes. Resolved per attempt from the
/// environment and selection snapshot; NEVER serialized anywhere — keys are secrets
/// and the record records resolved paths, not credentials (ADR-0008).
///
/// `Debug` is hand-written rather than derived: a derive prints `api_key` verbatim, and a
/// single future `{ep:?}` in an error message would carry the raw key into `terminal_reason`
/// — which lands in `run.json` and the dashboard. Redacting here means every future caller
/// gets the safe behavior for free rather than having to remember not to print this struct.
#[derive(Clone)]
pub struct Endpoint {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .field("model", &self.model)
            .finish()
    }
}

/// Default OpenAI-compatible endpoint: OpenRouter first, OpenAI by base-url swap
/// (selection-line override or this constant).
pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";
pub const DEFAULT_MODEL: &str = "deepseek/deepseek-chat-v3.1";

/// Which key pays for the endpoint being dialled — the pure half of [`Endpoint::resolve`], so
/// the refusal below is testable from literals with no environment.
///
/// The two keys are not interchangeable. `OPENROUTER_API_KEY` names its own host, so it pairs
/// with the default. `OPENAI_API_KEY` does not: pairing it with the *default* base url would
/// send an OpenAI secret to openrouter.ai on every turn of every attempt — a credential
/// disclosed to a third party the operator never chose, and a 401 they cannot explain. An
/// explicitly declared endpoint is the operator saying where the key goes, so that is honored;
/// the silent default is refused instead of guessed.
fn key_for(
    openrouter: Option<String>,
    openai: Option<String>,
    endpoint_declared: bool,
) -> Result<String, String> {
    match (openrouter, openai) {
        (Some(key), _) => Ok(key),
        (None, Some(key)) if endpoint_declared => Ok(key),
        (None, Some(_)) => Err(format!(
            "only OPENAI_API_KEY is set and no endpoint is declared, so the key would be sent \
             to {DEFAULT_BASE_URL} — declare the endpoint on the `~/.grind/agent` line \
             (`native <base-url>`) so the key reaches the provider it belongs to"
        )),
        (None, None) => Err("no OPENROUTER_API_KEY / OPENAI_API_KEY in environment".to_string()),
    }
}

impl Endpoint {
    /// Resolve per attempt from the environment and the selection snapshot. Keys are
    /// read-at-use via `world::var` and never serialized anywhere (ADR-0017).
    pub fn resolve(
        endpoint_override: Option<&str>,
        model_override: Option<&str>,
    ) -> Result<Self, String> {
        let api_key = key_for(
            crate::world::var("OPENROUTER_API_KEY").ok(),
            crate::world::var("OPENAI_API_KEY").ok(),
            endpoint_override.is_some(),
        )?;
        Ok(Self {
            base_url: endpoint_override.unwrap_or(DEFAULT_BASE_URL).to_string(),
            api_key,
            model: model_override.unwrap_or(DEFAULT_MODEL).to_string(),
        })
    }
}

/// Everything one attempt execution needs, resolved by the caller (the supervisor)
/// from the RunRecord snapshot. Adapters must not re-resolve policy knobs mid-run.
pub struct RunSpec<'a> {
    /// argv + prompt + mode as built by the stage ladder. The claude-code adapter
    /// consumes the whole invocation; the native adapter consumes the prompt and mode.
    pub invocation: &'a attempt::Invocation,
    /// Working directory for the attempt (worktree for stages, run dir for reflect).
    pub cwd: &'a Path,
    /// The run's own directory (`~/.grind/runs/<run-id>/`) — where grind-owned
    /// transcripts (`messages-N.jsonl`) land.
    pub run_dir: &'a Path,
    pub attempt_n: usize,
    /// Stable per-(run, stage) session identity (attempt::stage_session_id).
    pub session_id: &'a str,
    /// The declared worktree path as recorded (transcript slugging needs the string).
    pub worktree: &'a str,
    /// Job-level model override, if any.
    pub model: Option<&'a str>,
    /// This stage's denied-tool globs (attempt::denied_for / denied_for_reflect) —
    /// the single permission source both adapters enforce.
    pub denied_globs: &'a [String],
}

/// The one seam. Infallible by design: failures are loud failed Attempts (`is_error`,
/// `terminal_reason`, `rate_limited` when limit-shaped) so the existing re-entry
/// machinery — not an Err path — decides what happens next. A clean stop is something
/// an adapter must earn, never default to.
pub trait StageRunner {
    fn backend(&self) -> Backend;

    fn run(&self, spec: &RunSpec) -> attempt::Attempt;
}

/// Adapter #1: today's behavior, moved verbatim behind the seam.
pub struct ClaudeCodeAdapter {
    /// The snapshotted binary path (RunRecord.claude_bin).
    pub claude_bin: String,
    /// Host home — transcript discovery resolves under it (~/.claude/projects/...).
    pub home: PathBuf,
}

/// Adapter #2: grind-owned agent loop (default-off until P3 evidence).
pub struct NativeAdapter {
    /// Optional base-url override from the selection line; default endpoint otherwise.
    pub endpoint_override: Option<String>,
}

/// The ONE backend branch in the codebase (R1). Everything downstream calls this.
pub fn runner_for(
    backend: Backend,
    claude_bin: &str,
    home: &Path,
    endpoint_override: Option<String>,
) -> Box<dyn StageRunner> {
    match backend {
        Backend::ClaudeCode => Box::new(ClaudeCodeAdapter {
            claude_bin: claude_bin.to_string(),
            home: home.to_path_buf(),
        }),
        Backend::Native => Box::new(NativeAdapter { endpoint_override }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A derived `Debug` would print `api_key` verbatim, and this struct's own doc comment
    /// says the key is "NEVER serialized anywhere" — a single future `{ep:?}` in an error
    /// message would break that silently. The hand-written impl is what actually keeps the
    /// promise.
    #[test]
    fn endpoint_debug_never_prints_the_api_key() {
        let ep = Endpoint {
            base_url: "https://example.test/v1".to_string(),
            api_key: "sk-super-secret-value".to_string(),
            model: "some/model".to_string(),
        };
        let printed = format!("{ep:?}");
        assert!(
            !printed.contains("sk-super-secret-value"),
            "the api key leaked into Debug output: {printed}"
        );
        assert!(printed.contains("<redacted>"), "{printed}");
        // The non-secret fields are still useful for diagnosis.
        assert!(printed.contains("https://example.test/v1"), "{printed}");
        assert!(printed.contains("some/model"), "{printed}");
    }

    /// `world::append_line` already writes its own trailing newline via `writeln!`, so
    /// `encode()` must not add a second one — that was making every `messages-N.jsonl` line
    /// double-spaced.
    #[test]
    fn encode_does_not_append_its_own_newline() {
        let event = TranscriptEvent::Final {
            text: "done".to_string(),
        };
        let line = event.encode();
        assert!(!line.ends_with('\n'), "{line:?}");
        // Still a single valid JSON value on that one line.
        let _: serde_json::Value = serde_json::from_str(&line).expect("encode produced JSON");
    }

    #[test]
    fn an_openrouter_key_pays_for_the_default_endpoint() {
        assert_eq!(
            key_for(Some("or-key".into()), None, false),
            Ok("or-key".into())
        );
        // It also pays for a declared one — the operator said where it goes.
        assert_eq!(
            key_for(Some("or-key".into()), Some("oa-key".into()), true),
            Ok("or-key".into())
        );
    }

    #[test]
    fn an_openai_key_alone_refuses_the_default_endpoint_rather_than_misrouting() {
        let refused = key_for(None, Some("oa-key".into()), false).expect_err("a refusal");
        assert!(
            refused.contains(DEFAULT_BASE_URL),
            "the refusal names where the key would have gone: {refused}"
        );
        assert!(
            !refused.contains("oa-key"),
            "a refusal must never quote the secret: {refused}"
        );
    }

    #[test]
    fn an_openai_key_pays_for_an_endpoint_the_operator_declared() {
        assert_eq!(
            key_for(None, Some("oa-key".into()), true),
            Ok("oa-key".into())
        );
    }

    #[test]
    fn no_key_at_all_is_its_own_refusal() {
        let refused = key_for(None, None, true).expect_err("a refusal");
        assert!(refused.contains("no OPENROUTER_API_KEY"), "{refused}");
    }
}
