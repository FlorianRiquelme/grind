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
    /// Encode one event as a full JSONL line (trailing newline included).
    pub fn encode(&self) -> String {
        let mut line = serde_json::to_string(self).expect("TranscriptEvent serializes");
        line.push('\n');
        line
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
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

/// Default OpenAI-compatible endpoint: OpenRouter first, OpenAI by base-url swap
/// (selection-line override or this constant).
pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";
pub const DEFAULT_MODEL: &str = "deepseek/deepseek-chat-v3.1";

impl Endpoint {
    /// Resolve per attempt from the environment and the selection snapshot. Keys are
    /// read-at-use via `world::var` and never serialized anywhere (ADR-0017).
    pub fn resolve(
        endpoint_override: Option<&str>,
        model_override: Option<&str>,
    ) -> Result<Self, String> {
        let api_key = crate::world::var("OPENROUTER_API_KEY")
            .or_else(|_| crate::world::var("OPENAI_API_KEY"))
            .map_err(|_| "no OPENROUTER_API_KEY / OPENAI_API_KEY in environment".to_string())?;
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
