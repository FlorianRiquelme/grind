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
    /// A reply did not conform to the protocol: either prose arrived where a tag or
    /// a done sentinel was demanded (`fault` absent — the original shape), or a tool
    /// call arrived malformed (an empty name, an invented tool, arguments that are
    /// not an object, a required argument missing — `fault` carrying what was
    /// wrong, #142). Either way one corrective nudge was issued and sent. Logged
    /// every time so drift rates stay comparable across models in P3.
    ProtocolNudge {
        assistant_text: String,
        #[serde(default)]
        fault: Option<String>,
    },
    /// The wire mode was determined for this run (probe result or resumed latch).
    ProtocolSelected { mode: ProtoMode, reason: String },
    /// The attempt's final answer text (also carried by Attempt.result_tail).
    Final { text: String },
    /// Which stage's skill the prompt that opened this attempt declared, read off the
    /// prompt's own frontmatter. Every other variant here records a *wire* event, so
    /// nothing in the transcript could answer *which rung is running* — the one question
    /// `grind status`'s `now` line asks, and the reason a native Run answered nothing there.
    /// A prompt declaring no name logs no line: the field degrades on its own.
    SkillDeclared { skill: String },
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

/// One `~/.grind/agent` line: `<backend> [<base-url>] [key=value ...]` (ADR-0017, extended).
/// The base-url token — bare, no `=` — is the hermetic-test / self-hosting seam kept for
/// backward compatibility with the grammar ADR-0017 first shipped; `base-url=`, `model=`,
/// `fast=`, `strong=` and `proto=` are the key/value extensions. Credentials never appear
/// here (env only).
#[derive(Clone, Debug, Default)]
pub struct Selection {
    pub backend: Backend,
    pub endpoint_override: Option<String>,
    /// The model id `StageModel::Class(ModelClass::Fast)` resolves to on the native
    /// backend (`fast=`, or `model=` when `fast=` is absent). `None` falls back to
    /// [`DEFAULT_MODEL`].
    pub fast_model: Option<String>,
    /// The model id `StageModel::Class(ModelClass::Strong)` resolves to on the native
    /// backend (`strong=`, or `model=` when `strong=` is absent). `None` falls back to
    /// [`DEFAULT_MODEL`].
    pub strong_model: Option<String>,
    /// A declared wire mode, when `proto=` is present. Declaring it skips the probe
    /// entirely and latches from the declaration (ADR-0018): `stealth/ox-alpha` proved
    /// unable to execute native tool calls at all, so an undeclared run wastes one failed
    /// request discovering that every time.
    pub proto_override: Option<ProtoMode>,
}

impl Selection {
    /// Parse the file's single line. An empty (or whitespace) line is the default.
    ///
    /// Grammar: `<backend> [<base-url>] [key=value ...]`. A token containing `=` is a
    /// key/value; the first bare (no `=`) token after the backend is the base-url
    /// positional, kept for backward compatibility. An unknown key, a duplicate key, a
    /// `key=` with an empty value, or `proto=` with anything but `native`/`text` fails
    /// loud — the same register `Backend::parse` already rejects an unknown backend on.
    pub fn parse_line(line: &str) -> Result<Self, String> {
        let line = line.trim();
        if line.is_empty() {
            return Ok(Self::default());
        }
        let mut tokens = line.split_whitespace();
        let backend = Backend::parse(tokens.next().expect("non-empty line"))?;
        let mut tokens: Vec<&str> = tokens.collect();

        if backend == Backend::ClaudeCode {
            return if tokens.is_empty() {
                Ok(Self {
                    backend,
                    ..Self::default()
                })
            } else {
                Err(format!(
                    "claude-code takes no arguments, found {:?} \
                     (expected a bare \"claude-code\" line)",
                    tokens.join(" ")
                ))
            };
        }

        let mut endpoint_override = None;
        if let Some(first) = tokens.first()
            && !first.contains('=')
        {
            endpoint_override = Some(first.to_string());
            tokens.remove(0);
        }

        let mut model: Option<String> = None;
        let mut fast: Option<String> = None;
        let mut strong: Option<String> = None;
        let mut proto: Option<ProtoMode> = None;

        for token in tokens {
            let Some((key, value)) = token.split_once('=') else {
                return Err(format!(
                    "expected `key=value` on the agent line, found {token:?}"
                ));
            };
            if value.is_empty() {
                return Err(format!("`{key}=` has an empty value on the agent line"));
            }
            match key {
                "base-url" if endpoint_override.is_some() => {
                    return Err("duplicate `base-url` on the agent line".to_string());
                }
                "base-url" => endpoint_override = Some(value.to_string()),
                "model" if model.is_some() => {
                    return Err("duplicate `model` on the agent line".to_string());
                }
                "model" => model = Some(value.to_string()),
                "fast" if fast.is_some() => {
                    return Err("duplicate `fast` on the agent line".to_string());
                }
                "fast" => fast = Some(value.to_string()),
                "strong" if strong.is_some() => {
                    return Err("duplicate `strong` on the agent line".to_string());
                }
                "strong" => strong = Some(value.to_string()),
                "proto" if proto.is_some() => {
                    return Err("duplicate `proto` on the agent line".to_string());
                }
                "proto" => {
                    proto = Some(match value {
                        "native" => ProtoMode::Native,
                        "text" => ProtoMode::Text,
                        other => {
                            return Err(format!(
                                "unknown proto {other:?} on the agent line \
                                 (expected \"native\" or \"text\")"
                            ));
                        }
                    })
                }
                other => return Err(format!("unknown key {other:?} on the agent line")),
            }
        }

        let fast_model = fast.or_else(|| model.clone());
        let strong_model = strong.or(model);

        Ok(Self {
            backend,
            endpoint_override,
            fast_model,
            strong_model,
            proto_override: proto,
        })
    }
}

/// Grind's own routing intent for one stage, not a concrete model id: a model id is a
/// provider fact (`vendor/model` on the native backend's OpenAI-compatible wire, a plain
/// alias on claude-code); the class is grind's, and each adapter resolves it to its own
/// concrete id (ADR-0017, extended). **Not named `Tier`** — [`crate::decide::Tier`] already
/// names the T0/T1/T2 plan tiers, a different axis entirely.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelClass {
    Fast,
    Strong,
}

/// Which model backs one stage's Attempt, resolved by [`crate::supervisor`]'s
/// `resolve_stage_model` and consumed by both adapters. A Job's `model:` pin is a concrete
/// id and crosses the seam verbatim (`Pinned`); grind's own `fast`/`strong` routing is a
/// class (`Class`), each adapter resolving it to its own concrete id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StageModel {
    /// A Job's `model:` pin — passed through verbatim by both adapters.
    Pinned(String),
    /// Grind's own routing intent — each adapter resolves this to its own concrete id.
    Class(ModelClass),
}

impl StageModel {
    /// The claude-code alias `Class(Fast)` resolves to — a concrete Anthropic model id,
    /// the harness's own vocabulary, never the native wire's `vendor/model` namespace.
    pub const CLAUDE_FAST_ALIAS: &'static str = "claude-sonnet-5";

    /// The claude-code `--model` argument this stage-model resolves to, or `None` for no
    /// flag at all — `Class(Strong)`'s shape, the harness default before this plan existed
    /// and the one R2 requires stay byte-for-byte unchanged.
    pub fn claude_code_arg(&self) -> Option<String> {
        match self {
            Self::Pinned(id) => Some(id.clone()),
            Self::Class(ModelClass::Fast) => Some(Self::CLAUDE_FAST_ALIAS.to_string()),
            Self::Class(ModelClass::Strong) => None,
        }
    }

    /// The concrete model id this resolves to on the native backend: a pin verbatim, or
    /// the host-declared class model (`fast_model`/`strong_model` off the selection),
    /// falling back to [`DEFAULT_MODEL`] when the host declared neither.
    pub fn native_id(&self, fast_model: Option<&str>, strong_model: Option<&str>) -> String {
        match self {
            Self::Pinned(id) => id.clone(),
            Self::Class(ModelClass::Fast) => fast_model.unwrap_or(DEFAULT_MODEL).to_string(),
            Self::Class(ModelClass::Strong) => strong_model.unwrap_or(DEFAULT_MODEL).to_string(),
        }
    }
}

/// Which artifact family one [`RunSpec`]'s file names belong to. `Attempt` is `attempt-N.*`
/// (claude-code) / `messages-N.jsonl` (native) — both adapters' default. `Reflect` is
/// `reflect-N.*` / `reflect-messages-N.jsonl`: Reflect is never an Attempt (it lands no
/// `Attempt` row and is budget-exempt), so its files must never collide with attempt N's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileLabel {
    Attempt,
    Reflect,
}

impl FileLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Attempt => "attempt",
            Self::Reflect => "reflect",
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
///
/// An empty value is not a credential. A set-but-empty `OPENROUTER_API_KEY` used to reach the
/// match as `Some("")` and was sent as a bare `Bearer ` header, which OpenRouter answers with a
/// deterministic `401 Missing Authentication header` on every attempt until the budget burns.
/// Both keys are therefore filtered so an empty string falls through to the same refusals as
/// absence, before any pairing rule is consulted.
fn key_for(
    openrouter: Option<String>,
    openai: Option<String>,
    endpoint_declared: bool,
) -> Result<String, String> {
    let openrouter = openrouter.filter(|key| !key.is_empty());
    let openai = openai.filter(|key| !key.is_empty());
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
    /// For Reflect this is the run directory's own string — Reflect's `cwd` is the run
    /// directory, never the worktree, and the claude-code adapter's transcript discovery
    /// slugs off whatever directory the child actually ran in.
    pub worktree: &'a str,
    /// This stage's routed model — a Job's pin or grind's own fast/strong class
    /// (`resolve_stage_model`). Never optional: every stage resolves to one or the other,
    /// and each adapter maps it to its own concrete id (`StageModel::claude_code_arg`,
    /// `StageModel::native_id`).
    pub model: &'a StageModel,
    /// This stage's denied-tool globs (attempt::denied_for / denied_for_reflect) —
    /// the single permission source both adapters enforce.
    pub denied_globs: &'a [String],
    /// Which artifact family this call's file names belong to (`attempt-N.*` /
    /// `messages-N.jsonl` vs. `reflect-N.*` / `reflect-messages-N.jsonl`).
    pub file_label: FileLabel,
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
    /// Host-declared model id for `StageModel::Class(ModelClass::Fast)` (`Selection::fast_model`).
    pub fast_model: Option<String>,
    /// Host-declared model id for `StageModel::Class(ModelClass::Strong)` (`Selection::strong_model`).
    pub strong_model: Option<String>,
    /// A declared wire mode (`Selection::proto_override`) — when present, skips the probe
    /// entirely and latches from the declaration (ADR-0018).
    pub proto_override: Option<ProtoMode>,
}

/// Everything [`runner_for`] needs from the layout-declared selection (ADR-0017) beyond
/// `backend` and `claude_bin`, bundled so the factory's signature does not grow a new
/// parameter every time the `~/.grind/agent` grammar gains a key.
#[derive(Clone, Debug, Default)]
pub struct NativeConfig {
    pub endpoint_override: Option<String>,
    pub fast_model: Option<String>,
    pub strong_model: Option<String>,
    pub proto_override: Option<ProtoMode>,
}

/// The ONE backend branch in the codebase (R1). Everything downstream calls this.
pub fn runner_for(
    backend: Backend,
    claude_bin: &str,
    home: &Path,
    native: NativeConfig,
) -> Box<dyn StageRunner> {
    match backend {
        Backend::ClaudeCode => Box::new(ClaudeCodeAdapter {
            claude_bin: claude_bin.to_string(),
            home: home.to_path_buf(),
        }),
        Backend::Native => Box::new(NativeAdapter {
            endpoint_override: native.endpoint_override,
            fast_model: native.fast_model,
            strong_model: native.strong_model,
            proto_override: native.proto_override,
        }),
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
        let _: serde_json::Value = serde_json::from_str(&line).expect("encode produced JSON");
    }

    #[test]
    fn an_openrouter_key_pays_for_the_default_endpoint() {
        assert_eq!(
            key_for(Some("or-key".into()), None, false),
            Ok("or-key".into())
        );
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

    #[test]
    fn an_empty_openrouter_key_is_refused_like_absence_rather_than_sent_as_a_bare_bearer() {
        let refused = key_for(Some("".into()), None, false).expect_err("a refusal");
        assert!(refused.contains("OPENROUTER_API_KEY"), "{refused}");
        assert_eq!(
            refused,
            key_for(None, None, false).expect_err("absence"),
            "an empty key must reach the identical refusal as a missing one"
        );
    }

    #[test]
    fn an_empty_openai_key_keeps_the_disclosure_and_pairing_behavior_of_the_real_key() {
        let refused = key_for(Some("".into()), Some("oa-key".into()), false)
            .expect_err("an empty openrouter key never masks the disclosure refusal");
        assert!(refused.contains(DEFAULT_BASE_URL), "{refused}");
        let refused =
            key_for(None, Some("".into()), true).expect_err("an empty openai key is absence");
        assert!(refused.contains("no OPENROUTER_API_KEY"), "{refused}");
        assert_eq!(
            key_for(Some("or-key".into()), Some("".into()), false),
            Ok("or-key".into()),
            "a real openrouter key still wins over an empty openai one"
        );
    }

    #[test]
    fn a_bare_native_line_still_parses_with_no_overrides() {
        let s = Selection::parse_line("native").expect("bare native");
        assert_eq!(s.backend, Backend::Native);
        assert_eq!(s.endpoint_override, None);
        assert_eq!(s.fast_model, None);
        assert_eq!(s.strong_model, None);
        assert_eq!(s.proto_override, None);
    }

    #[test]
    fn the_bare_base_url_positional_still_parses_backward_compatibly() {
        let s = Selection::parse_line("native https://example.invalid/v1").expect("bare url");
        assert_eq!(
            s.endpoint_override.as_deref(),
            Some("https://example.invalid/v1")
        );
    }

    #[test]
    fn model_key_sets_both_classes_at_once() {
        let s = Selection::parse_line("native model=stealth/ox-alpha").expect("model=");
        assert_eq!(s.fast_model.as_deref(), Some("stealth/ox-alpha"));
        assert_eq!(s.strong_model.as_deref(), Some("stealth/ox-alpha"));
    }

    #[test]
    fn explicit_fast_or_strong_overrides_model_individually() {
        let s =
            Selection::parse_line("native model=shared fast=quick/one").expect("model= plus fast=");
        assert_eq!(s.fast_model.as_deref(), Some("quick/one"), "fast= wins");
        assert_eq!(
            s.strong_model.as_deref(),
            Some("shared"),
            "model= still backs strong"
        );
    }

    #[test]
    fn proto_declares_the_wire_mode_and_skips_the_probe() {
        let s = Selection::parse_line("native proto=text").expect("proto=text");
        assert_eq!(s.proto_override, Some(ProtoMode::Text));
        let s = Selection::parse_line("native proto=native").expect("proto=native");
        assert_eq!(s.proto_override, Some(ProtoMode::Native));
    }

    #[test]
    fn base_url_positional_and_keys_compose_in_one_line() {
        let s = Selection::parse_line(
            "native https://example.invalid/v1 model=stealth/ox-alpha proto=text",
        )
        .expect("full grammar");
        assert_eq!(
            s.endpoint_override.as_deref(),
            Some("https://example.invalid/v1")
        );
        assert_eq!(s.fast_model.as_deref(), Some("stealth/ox-alpha"));
        assert_eq!(s.strong_model.as_deref(), Some("stealth/ox-alpha"));
        assert_eq!(s.proto_override, Some(ProtoMode::Text));
    }

    #[test]
    fn base_url_key_is_equivalent_to_the_bare_positional() {
        let s =
            Selection::parse_line("native base-url=https://example.invalid/v1").expect("base-url=");
        assert_eq!(
            s.endpoint_override.as_deref(),
            Some("https://example.invalid/v1")
        );
    }

    #[test]
    fn an_unknown_key_refuses_loudly() {
        let refused = Selection::parse_line("native color=blue").expect_err("must refuse");
        assert!(refused.contains("unknown key"), "{refused}");
        assert!(refused.contains("color"), "{refused}");
    }

    #[test]
    fn a_duplicate_key_refuses_loudly() {
        let refused = Selection::parse_line("native model=a model=b").expect_err("must refuse");
        assert!(refused.contains("duplicate"), "{refused}");
        assert!(refused.contains("model"), "{refused}");
    }

    #[test]
    fn an_empty_value_refuses_loudly() {
        let refused = Selection::parse_line("native model=").expect_err("must refuse");
        assert!(refused.contains("empty value"), "{refused}");
    }

    #[test]
    fn an_unknown_proto_value_refuses_loudly() {
        let refused = Selection::parse_line("native proto=json").expect_err("must refuse");
        assert!(refused.contains("unknown proto"), "{refused}");
    }

    #[test]
    fn claude_code_still_refuses_any_argument_key_or_not() {
        let refused = Selection::parse_line("claude-code model=a").expect_err("must refuse");
        assert!(
            refused.contains("claude-code takes no arguments"),
            "{refused}"
        );
        let refused = Selection::parse_line("claude-code https://x").expect_err("must refuse");
        assert!(
            refused.contains("claude-code takes no arguments"),
            "{refused}"
        );
    }

    #[test]
    fn claude_code_arg_maps_pinned_verbatim_fast_to_the_alias_strong_to_no_flag() {
        assert_eq!(
            StageModel::Pinned("gpt-4o".into()).claude_code_arg(),
            Some("gpt-4o".to_string())
        );
        assert_eq!(
            StageModel::Class(ModelClass::Fast).claude_code_arg(),
            Some(StageModel::CLAUDE_FAST_ALIAS.to_string())
        );
        assert_eq!(
            StageModel::Class(ModelClass::Strong).claude_code_arg(),
            None
        );
    }

    #[test]
    fn native_id_never_injects_the_claude_alias_and_falls_back_to_default() {
        let fast = StageModel::Class(ModelClass::Fast);
        assert_eq!(fast.native_id(None, None), DEFAULT_MODEL);
        assert_eq!(
            fast.native_id(
                Some("stealth/ox-alpha"),
                Some("deepseek/deepseek-chat-v3.1")
            ),
            "stealth/ox-alpha"
        );
        let strong = StageModel::Class(ModelClass::Strong);
        assert_eq!(strong.native_id(None, None), DEFAULT_MODEL);
        assert_eq!(
            strong.native_id(
                Some("stealth/ox-alpha"),
                Some("deepseek/deepseek-chat-v3.1")
            ),
            "deepseek/deepseek-chat-v3.1"
        );
        assert_eq!(
            StageModel::Pinned("gpt-4o".into()).native_id(Some("x"), Some("y")),
            "gpt-4o",
            "a pin crosses verbatim regardless of the declared classes"
        );
    }
}
