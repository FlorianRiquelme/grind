//! One `claude` invocation: the argv the denials ride on, and the raw triple that hits disk
//! before anything reads it.
//!
//! The rule's one asterisk — a pure builder and a pure classifier around two `world` calls,
//! neither cleanly pure nor cleanly I/O (ADR-0007).

use crate::claude::PANEL_BASH_FORMS;
use crate::job::Job;
use crate::observe::Observed;
use crate::rung;
use serde::{Deserialize, Serialize};

/// A Run must never merge its own PR, discard the human's history, or rewrite a pushed branch.
/// Denials are inherited by subagents and are **not** overridden by `bypassPermissions`
/// (verified 2026-08-02), so this is a dependable constraint rather than a request.
///
/// **Nothing sits behind it.** No credential at any tier can withhold merge from something
/// allowed to open a PR — `Pull requests: write` covers both, `Contents: write` covers push and
/// branch deletion, and force-push is indistinguishable from push at every credential layer. So
/// these globs are the entire barrier, not the outer one.
///
/// Weakening the list is **intent**, and no carrier defends against intent. What is typeable is
/// the narrower, omission-shaped property below: every invocation carries them. The contents
/// stay prose, in `AGENTS.md`, where they already are.
pub const DENIED_TOOLS: [&str; 29] = [
    "Bash(gh pr merge*)",
    "Bash(git push --force*)",
    "Bash(git push -f*)",
    "Bash(git reset --hard*)",
    "Bash(git rebase*)",
    "Bash(git checkout main*)",
    "Bash(git branch -D*)",
    "Bash(git push --delete*)",
    "Bash(git push*+*)",
    "Bash(git -C*)",
    "Bash(git switch main*)",
    "Bash(gh api*merge*)",
    "Bash(git push*--force*)",
    "Bash(git push*--delete*)",
    "Bash(git push*:*)",
    "Bash(git push* -f)",
    "Bash(git push* -f *)",
    "Bash(git reset*--hard*)",
    "Bash(git branch* -D*)",
    "Bash(git branch*--delete*)",
    "Bash(git*--force-with-lease*)",
    "Bash(git -c*)",
    "Bash(git*update-ref*)",
    "Bash(git push*--mirror*)",
    "Bash(git push*--prune*)",
    "Bash(gh api*DELETE*)",
    "Bash(sh -c*)",
    "Bash(bash -c*)",
    "Bash(eval*)",
];

/// The denial set for one of the ten ladder stages. **The base list always, verbatim, never
/// filtered** — widening only, never narrowing. Report-only stages (`PlanReview`, `Review`,
/// `Validate`) additionally deny `Write`, `Edit` and `write_file`; the two fan-out panels
/// (`Review`, `Validate`) further deny the write-capable Bash forms above. The denials narrow
/// the surface without closing it: shell redirection is not deniable — it is how a panel
/// session writes its findings files — so "touches nothing" is the prompt's stated discipline
/// with the sandbox behind it, never a sandbox guarantee alone. `Plan`, `Triage`, `Work`,
/// `Simplify`, `DiffTriage`, `Fixes` and `Ship` carry the base list only: they are the stages
/// that write the worktree (or, for the two in-process `[R]` passes, write nothing at all).
///
/// `write_file` rides alongside `Write`/`Edit` rather than replacing them: those two are the
/// names the claude-code adapter's own matcher recognises, `write_file` is the name the native
/// toolkit spells its writer (`tools::ToolRegistry`), and `tools::gate`'s bare-name branch
/// matches a glob against the raw tool name — so a list that named only `Write`/`Edit` gated
/// one adapter and left the other's writer open on these three report-only stages. One list,
/// same meaning on both adapters; the entry Claude Code never matches is harmless there.
pub fn denied_for(stage: rung::Stage) -> Vec<String> {
    use rung::Stage::{PlanReview, Review, Validate};
    let mut denials: Vec<String> = DENIED_TOOLS.iter().map(|s| s.to_string()).collect();
    if matches!(stage, PlanReview | Review | Validate) {
        denials.push("Write".to_string());
        denials.push("Edit".to_string());
        denials.push("write_file".to_string());
    }
    if matches!(stage, Review | Validate) {
        denials.extend(PANEL_BASH_FORMS.iter().map(|s| s.to_string()));
    }
    denials
}

/// Reflect is not a rung on the ladder — [`rung::Stage`] has no variant for it — and it is
/// report-only like the two panels, but it must still write its own artifacts under
/// `<stages-dir>/reflect/`, which a `Write`/`Edit` denial would block. Its worktree protection is
/// instead the write-capable Bash-form denials above (plus `git push*` outright), backed by
/// dispatching it with the *run* directory as cwd rather than the *worktree* (unit C's job, noted
/// here since this is where the sandbox story is decided) — so there is no repo tree under this
/// session for `Write`/`Edit` to touch in the first place.
pub fn denied_for_reflect() -> Vec<String> {
    let mut denials: Vec<String> = DENIED_TOOLS.iter().map(|s| s.to_string()).collect();
    denials.extend(PANEL_BASH_FORMS.iter().map(|s| s.to_string()));
    denials
}

/// The Triage grader's denial set — the same shape [`denied_for_reflect`] builds, and for the
/// same reason: the grader is report-only in the sense that it judges and never edits the
/// worktree, but it **must** write its one return file (`<stages-dir>/triage/grade.json`), so
/// `Write`/`Edit` stay allowed while the write-capable Bash forms (plus `git push*` outright)
/// are denied — a shell loop writing a second file is the drift those denials catch, not a
/// sandbox promise. The skill's own prose carries the one-file discipline; this is the tool
/// layer behind it. Same cwd story as Reflect: dispatched with the run directory, so there is
/// no repo tree under the session for `Write`/`Edit` to touch in the first place.
pub fn denied_for_grade() -> Vec<String> {
    let mut denials: Vec<String> = DENIED_TOOLS.iter().map(|s| s.to_string()).collect();
    denials.extend(PANEL_BASH_FORMS.iter().map(|s| s.to_string()));
    denials
}

/// The session id a stage's own Attempt dispatches or resumes: a validly formatted UUID,
/// deterministically derived from `(run_id, stage)`. `claude -p --session-id` only accepts a
/// UUID, so the literal `<run>-<stage>` text this used to format died every dispatch at
/// startup (#111); the derivation is FNV-1a over both parts — the same std-only hash
/// [`crate::supervisor::skills_hash`] carries — because the supervisor must re-derive *the
/// same* id on `--resume`, possibly days later, and an identity check (never a security
/// boundary) takes no dependency. Version and variant bits are set, so the result parses as a
/// UUID everywhere `--session-id` lands. Re-entry resumes the dying *stage's* session, never
/// the Run's old mega-session.
pub fn stage_session_id(run_id: &str, stage: rung::Stage) -> String {
    session_id_from(run_id, &stage.to_string())
}

/// Reflect's session id — the same derivation as a stage's, named `"reflect"` since
/// [`rung::Stage`] deliberately has no variant for it (the design's own words).
pub fn reflect_session_id(run_id: &str) -> String {
    session_id_from(run_id, "reflect")
}

/// The Triage grader's session id — the same derivation as a stage's, named `"grade"`. Like
/// Reflect (and unlike every rung), grading is a judgment seat bolted beside the ladder rather
/// than on it (ADR-0015): the supervisor dispatches it directly, so it needs its own
/// deterministic, re-derivable id for `--resume` re-entry, exactly as [`reflect_session_id`]
/// documents.
pub fn grade_session_id(run_id: &str) -> String {
    session_id_from(run_id, "grade")
}

/// Two independent FNV-1a passes over `(run_id, name)` in both orders give 128 bits without
/// a dependency; the zero-byte separator keeps `"ab"`+`"c"` and `"a"`+`"bc"` apart within a
/// pass, and the differing seeds keep the two passes from echoing each other.
fn session_id_from(run_id: &str, name: &str) -> String {
    fn fnv1a(seed: u64, parts: [&str; 2]) -> u64 {
        let mut hash = seed;
        for part in parts {
            for byte in part.bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
            hash ^= 0;
        }
        hash
    }
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&fnv1a(0xcbf2_9ce4_8422_2325, [run_id, name]).to_be_bytes());
    bytes[8..].copy_from_slice(&fnv1a(0x9e37_79b9_7f4a_7c15, [name, run_id]).to_be_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-\
         {:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

/// What a stage invocation needs beyond a session id and a binary path: the stage itself, its
/// skill text (read by the caller through `world` and handed in as `&str`, so this stays pure),
/// where its returns and artifacts go, the worktree it runs in, the Job rows it is dispatched
/// against, and the model this Run's tier resolved for it. `notes` is Plan-only injected
/// notes/lessons text; every other stage leaves it `None` and it is never rendered. `landed`
/// is Work-only injected fact text — commits already on the branch and unit returns already on
/// disk from earlier attempts (issue #170): per-unit commits are checkpoints, and re-entry must
/// orient from those facts rather than rediscover state via `git status`; every other stage
/// leaves it `None`.
#[derive(Debug, Clone, Copy)]
pub struct StageContext<'a> {
    pub stage: rung::Stage,
    pub skill_text: &'a str,
    pub stages_dir: &'a str,
    pub worktree: &'a str,
    pub job: &'a Job,
    pub model: Option<&'a str>,
    pub notes: Option<&'a str>,
    pub landed: Option<&'a str>,
}

/// The two facts a stage invocation needs that [`Conditions`] does not carry the right shape
/// for: a stage invocation names no plugin (ADR-0015 retired the pin once nothing was left to
/// invoke it), so this is not `Conditions` with a field ignored — it is the narrower thing a
/// stage argv actually is.
#[derive(Debug, Clone, Copy)]
pub struct StageConditions<'a> {
    pub claude_bin: &'a str,
    pub run_id: &'a str,
}

/// Which of the three shapes an invocation is. Recorded per attempt, so a spent CI budget is
/// visible as itself rather than as an ordinary re-entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Dispatch,
    Resume,
    CiBabysit,
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let word = match self {
            Mode::Dispatch => "dispatch",
            Mode::Resume => "resume",
            Mode::CiBabysit => "ci-babysit",
        };
        write!(f, "{word}")
    }
}

/// One clearance the human recorded on a Blocked Run: when, and what changed in the world.
///
/// It lives here beside [`Attempt`] because this module is already the shared vocabulary
/// between the record's private writer and its read-only reader — and because the re-entry
/// composition consumes the note here. A shared-noun module for it would pull the writer and
/// the readers back under one roof, which `tests/topology.rs` exists to refuse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clearance {
    pub cleared_at: String,
    pub note: String,
}

/// What the one surviving [`Conditions`] user — [`ci_babysit`] — is built from, all of it read
/// from the record rather than the environment so re-entering an in-flight Run never changes it
/// mid-stage.
#[derive(Debug, Clone, Copy)]
pub struct Conditions<'a> {
    pub claude_bin: &'a str,
    pub session_id: &'a str,
    pub model: Option<&'a str>,
}

/// A built invocation. **Private fields, and `build` is the only constructor** — so an argv
/// that does not carry the denials is not a value this program can hold. That is the
/// omission-shaped half of the property; the contents of the list are prose, because weakening
/// them is intent.
#[derive(Debug, Clone)]
pub struct Invocation {
    argv: Vec<String>,
    prompt: String,
    mode: Mode,
}

impl Invocation {
    pub fn argv(&self) -> &[String] {
        &self.argv
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The crate-side constructor. The argv builders themselves now live in
    /// [`crate::claude`]; routing them through this keeps the invariant they document
    /// enforceable — code outside this crate still cannot hold an `Invocation` whose argv
    /// skipped the denial list.
    pub(crate) fn build(argv: Vec<String>, prompt: String, mode: Mode) -> Invocation {
        Invocation { argv, prompt, mode }
    }
}

/// The Run-level termination sentinel. A promise is a claim about the work, **spoken by the
/// agent in its own final text** — never inferred from an adapter's control flow (issue #139:
/// the native loop's synthesized promise ended every Run one rung into its ladder). Every
/// adapter reads it out of the words the agent said and nowhere else.
pub(crate) const DONE_PROMISE: &str = "<promise>DONE</promise>";

/// One attempt as the record holds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attempt {
    pub n: usize,
    pub mode: Mode,
    pub started_at: String,
    pub ended_at: String,
    pub exit_code: Option<i32>,
    pub is_error: bool,
    /// Whether the child's stdout parsed at all. An unparseable response is a record that says
    /// so, not an aborted supervisor.
    pub parse_ok: bool,
    /// `Some("unparseable-output")` when `parse_ok` is false, `Some("result-field-missing")`
    /// when the payload parsed but its `result` key did not arrive, the payload's own subtype
    /// otherwise. Two distinct synthetic values rather than one, so a renamed field never reads
    /// the same as garbage that never parsed.
    pub subtype: Option<String>,
    pub stop_reason: Option<String>,
    pub api_error_status: Option<String>,
    pub terminal_reason: Option<String>,
    pub num_turns: Option<u64>,
    /// What this attempt spent, in dollars — **and a field whose `None` means three different
    /// things depending on which adapter produced the row** (issue #194). Nothing at the type
    /// level says so, and [`Attempt::is_wait`] reads it, which is what makes the divergence
    /// worth writing down rather than leaving to whoever greps three files next:
    ///
    /// - **claude-code** (`crate::claude::classify`) takes the child's own `total_cost_usd`
    ///   JSON field verbatim. `None` is genuinely ambiguous — a renamed key and a true zero
    ///   look the same from here — and that ambiguity is why `is_wait` keys on *presence*.
    /// - **native** (`crate::native::cost_floor`) sums `usage.cost` live off the wire and
    ///   floors the answer to `Some(0.0)`. It never emits `None`, deliberately: the loop
    ///   authoritatively knows it recorded no cost, and `None` would make a first-turn rate
    ///   limit spend the attempt budget instead of reading as a Wait.
    /// - **omp** (`crate::omp::classify`) scans the harvested JSONL for `message_end`
    ///   assistant frames. `None` means *the harness exposed no spend channel at all*, never a
    ///   zero — the `saw_usage` gate exists so a silent stream stops manufacturing `$0.00`.
    ///
    /// The three are individually reasoned and each lands on the safe side of `is_wait`, so
    /// this is a documented divergence rather than a defect. What a fourth adapter must not do
    /// is pick a convention by accident: a producer reporting `Some(0.0)` where it means
    /// *unknown* makes every one of its attempts free and its Run endless.
    /// `tests/cost_conventions.rs` pins all three, and the `is_wait` answer each one yields.
    pub total_cost_usd: Option<f64>,
    pub usage: Option<serde_json::Value>,
    pub permission_denials: Vec<serde_json::Value>,
    pub done_promise: bool,
    pub rate_limited: bool,
    pub result_tail: String,
    /// **Spawned and returned, per Attempt.** A fan-out degrading on attempt 3 of a Run that
    /// finishes on attempt 8 leaves something durable.
    ///
    /// Three-valued, because two bare `Option<u64>` fields would collapse *no fan-out* and
    /// *could not read the transcript*. **No summary, boolean or health word sits over the two
    /// integers**: a count of processes must never become an assertion about a review
    /// (ADR-0006's sixth prohibited shape).
    pub fanout: Observed<(u64, u64)>,
    /// The transcript fact, three-valued for the same reason [`Attempt::fanout`] is: two of
    /// the facts it separates once shared one `None`.
    ///
    /// - `Recorded(name)` — the attempt allocated a transcript and this is its bare file name
    ///   (`messages-2-2.jsonl` for a re-entered attempt 2 that found slot 1 taken). A name,
    ///   never a path: the reader joins it onto the Run directory it already knows.
    /// - `PredatesName` — every record written before the field existed. The constructed
    ///   `messages-{n}.jsonl` fallback is exactly right for these rows: the file exists and is
    ///   the attempt's own.
    /// - `WroteNone` — the attempt's lifecycle ended before any transcript was allocated
    ///   (endpoint resolution fails before `allocate_transcript` is reached), so no file was
    ///   ever written and there is nothing to link.
    ///
    /// Absence-of-key decodes to `PredatesName`, never `WroteNone`: today's writer
    /// serializes `None` as an explicit `"transcript_name": null`, so absence and explicit
    /// null both stand for *the record predates the name* — a statement about when it was
    /// written — while `WroteNone` is a statement about how the attempt ended. An old
    /// record's silence is evidence about when it was written, never about how its attempt
    /// ended, and only the encoder (which watched the loop end without allocating) can state
    /// the latter. Additive and `serde(default)`, so an existing `run.json` still parses
    /// under `RunView`'s `deny_unknown_fields`.
    ///
    /// On the way out, `PredatesName` writes **nothing** under the key — the same silence it
    /// decodes from — so a legacy row carried forward by a newer binary keeps meaning exactly
    /// what it meant (the round-trip batteries in `supervisor` and `view` re-encode loaded
    /// records). `Recorded` writes the bare string today's writer wrote; only `WroteNone`
    /// states itself, as `{"wrote_none": null}` — a shape no record written before this
    /// change can carry, which is what makes the two silences tellable after the fact.
    #[serde(
        default,
        rename = "transcript_name",
        skip_serializing_if = "Transcript::predates_the_name"
    )]
    pub transcript: Transcript,
}

/// What an Attempt states about the transcript it wrote. Three-valued because two of the
/// facts it separates once shared one `None`: a record that predates the field and a record
/// whose attempt ended before allocating a transcript both decode out of today's byte shapes
/// as `transcript_name` absent or `null`, but they mean different things — one is about when
/// the record was written, the other about how the attempt ended.
///
/// The wire key stays `transcript_name` — the key every record written before this change
/// carries — via `#[serde(rename)]` on [`Attempt::transcript`]; the Rust name widens, the
/// record grammar does not.
///
/// Decoding (the spell is load-bearing; see [`Attempt::transcript`]):
/// - a string value decodes to `Recorded(name)`;
/// - an explicit `null` **and** an absent key both decode to `PredatesName`;
/// - `{"wrote_none": null}` — and only that — decodes to `WroteNone`: unreachable from any
///   old byte shape by construction, it exists only because the encoder states it on the
///   endpoint-resolution path, which no record written before this change can carry.
///
/// Encoding mirrors it: `Recorded` writes the bare string, `WroteNone` writes
/// `{"wrote_none": null}`, and `PredatesName` is skipped at the field that holds it
/// ([`Attempt::transcript`]) because its only content *is* silence-under-the-key.
#[derive(Debug, Clone, PartialEq)]
pub enum Transcript {
    /// The bare name of the transcript file this attempt wrote. A name, never a path: the
    /// reader joins it onto the Run directory it already knows.
    Recorded(String),
    /// The record was written before `transcript_name` existed; the constructed
    /// `messages-{n}.jsonl` fallback names this attempt's own transcript exactly.
    PredatesName,
    /// The attempt's lifecycle ended before any transcript was allocated — nothing was ever
    /// written, so there is nothing to link. Reachable only from the encoder.
    WroteNone,
}

/// The encoder is part of the spell: `Recorded` states its name under the record's own
/// historical key (`transcript_name`), `WroteNone` serializes an explicit null under it — a
/// stated fact, not a default — and `PredatesName` must never be written at all: it exists
/// so old records' silence can be named in the reader, and encoding it as anything would
/// forge a statement about when this record was written that the writer never had. A
/// re-encoded old record carries nothing under the key again.
impl serde::Serialize for Transcript {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Transcript::Recorded(name) => serializer.serialize_str(name),
            Transcript::WroteNone => {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("wrote_none", &Option::<()>::None)?;
                map.end()
            }
            Transcript::PredatesName => Err(serde::ser::Error::custom(
                "predates-the-name is written as silence under the key — skip the field, never encode a value",
            )),
        }
    }
}

/// The decoder is the compatibility spell (Direction): today's writer serializes a bare
/// `None` as `"transcript_name": null` — key present, value null — so absence-of-key and
/// explicit null are the same fact on records already on disk, and both must land on
/// `PredatesName`, never on `WroteNone`. An old record's silence is evidence about when it
/// was written, never about how its attempt ended; `WroteNone` is reachable only through the
/// encoder that watched the loop end without allocating.
impl<'de> Deserialize<'de> for Transcript {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Transcript;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("the bare file name of a written transcript, or its absence")
            }

            /// An `Option` field deserializes through a temp `Option` per the serde
            /// "deserialize an optional field" recipe — so a *present* string lands in the
            /// some arm and must be forwarded for the real visit, or every record would
            /// silently read as predating.
            fn visit_some<D>(self, d: D) -> Result<Self::Value, D::Error>
            where
                D: serde::de::Deserializer<'de>,
            {
                struct SomeVisitor;
                impl<'de> serde::de::Visitor<'de> for SomeVisitor {
                    type Value = Transcript;

                    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                        f.write_str("the bare file name of a written transcript, or its absence")
                    }

                    fn visit_unit<E>(self) -> Result<Self::Value, E> {
                        Ok(Transcript::PredatesName)
                    }

                    fn visit_none<E>(self) -> Result<Self::Value, E> {
                        Ok(Transcript::PredatesName)
                    }

                    fn visit_some<D>(self, d: D) -> Result<Self::Value, D::Error>
                    where
                        D: serde::de::Deserializer<'de>,
                    {
                        d.deserialize_any(Visitor)
                    }

                    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
                        Ok(Transcript::Recorded(v.to_owned()))
                    }

                    fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
                    where
                        A: serde::de::MapAccess<'de>,
                    {
                        Visitor.visit_map(map)
                    }
                }
                d.deserialize_any(SomeVisitor)
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(Transcript::PredatesName)
            }

            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(Transcript::PredatesName)
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
                Ok(Transcript::Recorded(v.to_owned()))
            }

            /// The only stated fact: `{"wrote_none": null}`. Any other key is refused rather
            /// than defaulted, so a future grammar grows this visitor on purpose.
            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let key: String = map.next_key()?.ok_or_else(|| {
                    serde::de::Error::custom("an empty transcript fact states nothing")
                })?;
                match key.as_str() {
                    "wrote_none" => {
                        let _: Option<serde::de::IgnoredAny> = map.next_value()?;
                        Ok(Transcript::WroteNone)
                    }
                    other => Err(serde::de::Error::unknown_field(other, &["wrote_none"])),
                }
            }
        }
        deserializer.deserialize_option(Visitor)
    }
}

impl Default for Transcript {
    /// Absence-of-key means *the record predates the name*. Deliberately hand-written rather
    /// than derived: deriving would read as if the variant were an arbitrary choice, where
    /// this comment can say why silence on an old record must land here.
    fn default() -> Self {
        Transcript::PredatesName
    }
}

impl Transcript {
    /// The field-skip predicate: a predating row writes nothing under the key, because
    /// silence-under-the-key *is* what predates-the-name means.
    fn predates_the_name(&self) -> bool {
        matches!(self, Transcript::PredatesName)
    }
}

impl Attempt {
    /// The fan-out arithmetic, gathered before the Attempt is pushed — `RunRecord.attempts` is
    /// append-only with no mutating accessor, so it cannot be filled in afterwards (KTD9).
    pub fn with_fanout(mut self, fanout: Observed<(u64, u64)>) -> Self {
        self.fanout = fanout;
        self
    }

    /// **A Wait is an Attempt that did no work**, and it is keyed on work done rather than on
    /// cause — this predicate never reads `rate_limited`. Six of Run 2's eight Attempts cost $0
    /// and ran one turn each, probing a wall, and spent the same budget as the three that built
    /// twelve commits.
    ///
    /// **Presence, not absence, of the two fields decides.** Run 2's real Waits carry explicit
    /// `total_cost_usd: 0.0` and `num_turns: 1`; a payload that parsed but whose cost/turn
    /// fields were renamed away (the recorded `result-field-missing` drift) must not read as
    /// *did no work* — that is the same failure mode the next clause guards, one level
    /// shallower. Absence spends the budget, which is the safe direction.
    ///
    /// **An Attempt whose stdout did not parse is never a Wait, and that clause is
    /// load-bearing.** A child that dies before emitting parseable JSON leaves both the cost and
    /// the turn count absent, so a predicate reading absence as *did no work* would make every
    /// crash loop free: no budget spent, no rate-limit match, immediate re-entry, forever, with
    /// `attempt N of M` reporting the Run as barely started. Absence of evidence is not evidence
    /// of no work, and `parse_ok` is the field that already separates the two.
    ///
    /// Derived, never persisted: the fields it reads are already on the record, so there is no
    /// migration and no reader mirror to keep in step.
    pub fn is_wait(&self) -> bool {
        self.parse_ok
            && self.total_cost_usd.is_some_and(|cost| cost <= 0.0)
            && self.num_turns.is_some_and(|turns| turns <= 1)
    }
}

/// How many of these Attempts did work. **The attempt budget counts these and no others**, on
/// every surface that prints *attempt N of M*.
pub fn working(attempts: &[Attempt]) -> usize {
    attempts.iter().filter(|a| !a.is_wait()).count()
}

/// The run of Waits at the end of the list — the only bound on a Run that never does work,
/// since Waits by design spend no attempt budget.
///
/// Counted from the **recorded** list rather than held loop-local, because the bound has to
/// survive a restart: `resume --all` re-enters rate-limited and died Runs at boot, so a
/// loop-local count would hand a permanently-walled Run a fresh allowance at every reboot and
/// never terminate.
pub fn trailing_waits(attempts: &[Attempt]) -> usize {
    attempts.iter().rev().take_while(|a| a.is_wait()).count()
}

/// A field as text, whether the child sent a string, a number or a boolean. `api_error_status`
/// arrives as a JSON **number** in Run 2's recorded triple and as a string elsewhere.
pub(crate) fn text_at(value: &serde_json::Value, key: &str) -> Option<String> {
    match value.get(key)? {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

/// Rate limits, from a **normalised haystack**.
///
/// Lowercasing and stripping non-alphanumerics is what makes `rate  limit` with two spaces
/// match; including the API error status field is what makes a bare `429` match with no
/// matching prose anywhere. Run 2's six limited attempts read *"You've hit your session limit ·
/// resets 5pm"*, which matched none of the script's phrases — only the status code classified
/// them, and had it missed, eight attempts would have burned in under a minute against a
/// three-hour wall.
///
/// No regex crate: normalising detects rate limits more broadly than a pattern does.
pub fn is_rate_limited(value: &serde_json::Value) -> bool {
    if !value
        .get("is_error")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return false;
    }
    let mut haystack = String::new();
    for key in ["result", "terminal_reason", "api_error_status", "subtype"] {
        if let Some(text) = text_at(value, key) {
            haystack.push_str(&text);
            haystack.push(' ');
        }
    }
    let normalised = normalise(&haystack);
    mentions_limit(&normalised)
}

/// The needle set over an already-normalised haystack — shared by the payload path above and
/// by `classify`'s fold over the raw streams, so both ask exactly one question.
pub(crate) fn mentions_limit(normalised: &str) -> bool {
    const NEEDLES: [&str; 8] = [
        "ratelimit",
        "usagelimit",
        "sessionlimit",
        "toomanyrequests",
        "quotaexceeded",
        "resetsat",
        "resetat",
        "429",
    ];
    NEEDLES.iter().any(|needle| normalised.contains(needle))
}

pub(crate) fn normalise(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::claude::{
        CI_BABYSIT_PROMPT, ci_babysit, classify, stage_dispatch, stage_invocation, stage_resume,
    };
    const RUN2_RATE_LIMITED: &str = include_str!("../tests/fixtures/run2/rate-limited.stdout.json");
    const RUN1_ATTEMPT_1: &str = include_str!("../tests/fixtures/run1/attempt-1.stdout.json");
    const RUN1_ATTEMPT_2: &str = include_str!("../tests/fixtures/run1/attempt-2.stdout.json");
    const RUN1_ATTEMPT_3: &str = include_str!("../tests/fixtures/run1/attempt-3.stdout.json");
    const TRUNCATED: &str = include_str!("../tests/fixtures/run1/degraded-truncated.stdout.json");
    const GARBAGE: &str = include_str!("../tests/fixtures/run1/degraded-garbage.stdout.json");
    const EMPTY: &str = include_str!("../tests/fixtures/run1/degraded-empty.stdout.json");
    const DEGRADED_RENAMED: &str =
        include_str!("../tests/fixtures/run1/degraded-renamed.stdout.json");

    fn job() -> Job {
        Job {
            issue: 28,
            url: "https://github.com/FlorianRiquelme/snapper/issues/28".to_string(),
            title: "Slice 1b".to_string(),
            labels: vec![],
            target_repo: "FlorianRiquelme/snapper".to_string(),
            branch: "feat/28-slice-1b".to_string(),
            handoff_sha: "9d1f4c7a".to_string(),
            anchor: "docs/plans/a.md".to_string(),
            intent: None,
            agent: None,
            model: None,
            done_predicate: "just verify is green".to_string(),
            base_branch: "main".to_string(),
            verify_entrypoint: "just verify".to_string(),
            declared_hot_paths: vec![],
        }
    }

    fn conditions(model: Option<&str>) -> Conditions<'_> {
        Conditions {
            claude_bin: "/home/op/.grind/bin/claude",
            session_id: "d51b4c39-ce1d-449b-8366-04b9b1aa6573",
            model,
        }
    }

    fn denials_of(invocation: &Invocation) -> Vec<String> {
        let argv = invocation.argv();
        let at = argv
            .iter()
            .position(|a| a == "--disallowedTools")
            .expect("--disallowedTools");
        argv[at + 1..].to_vec()
    }

    #[test]
    fn ci_babysit_carries_every_denial_no_plugin_flag_and_resumes_its_own_session() {
        let conditions = conditions(Some("claude-opus-5"));
        let invocation = ci_babysit(&conditions);
        assert_eq!(
            denials_of(&invocation),
            DENIED_TOOLS.to_vec(),
            "ci-babysit must carry every denial"
        );
        assert!(invocation.argv().contains(&"--resume".to_string()));
        assert!(!invocation.argv().contains(&"--session-id".to_string()));
        let at = invocation
            .argv()
            .iter()
            .position(|a| a == "--resume")
            .unwrap();
        assert_eq!(invocation.argv()[at + 1], conditions.session_id);
        assert!(!invocation.argv().contains(&"--plugin-dir".to_string()));
        assert!(!invocation.argv().contains(&"--max-budget-usd".to_string()));
        assert!(
            invocation
                .argv()
                .windows(2)
                .any(|w| w[0] == "--model" && w[1] == "claude-opus-5")
        );
    }

    #[test]
    fn ci_babysit_with_no_model_carries_no_model_flag() {
        let invocation = ci_babysit(&conditions(None));
        assert!(!invocation.argv().contains(&"--model".to_string()));
    }

    use crate::tools::{glob_matches, subcommands_of};

    fn is_denied(command: &str) -> bool {
        let (subs, complete) = subcommands_of(command);
        assert!(complete, "test commands must be fully searchable");
        subs.iter().any(|sub| {
            DENIED_TOOLS.iter().any(|glob| {
                let pattern = glob
                    .strip_prefix("Bash(")
                    .and_then(|g| g.strip_suffix(')'))
                    .expect("every DENIED_TOOLS glob is Bash(...)");
                glob_matches(pattern, sub)
            })
        })
    }

    #[test]
    fn each_forbidden_operation_has_a_glob_that_refuses_it_under_the_documented_matcher() {
        let table: [(&str, &[&str]); 21] = [
            (
                "merge via gh pr merge",
                &["gh pr merge 123 --squash", "gh pr merge --squash 123"],
            ),
            (
                "force push with --force",
                &[
                    "git push --force origin feat/x",
                    "git push origin --force",
                    "git push origin main --force",
                    "git push -u origin main --force",
                ],
            ),
            ("force push with -f", &["git push -f", "git push origin -f"]),
            (
                "force push with --force-with-lease",
                &[
                    "git push --force-with-lease origin feat/x",
                    "git push origin --force-with-lease",
                ],
            ),
            (
                "hard reset",
                &["git reset --hard HEAD~3", "git reset HEAD~3 --hard"],
            ),
            (
                "rebase",
                &["git rebase main", "git rebase --onto main feat/x"],
            ),
            (
                "checkout main",
                &["git checkout main", "git checkout main --force"],
            ),
            (
                "branch delete",
                &[
                    "git branch -D feat/x",
                    "git branch feat/x -D",
                    "git branch --delete --force feat/x",
                    "git branch --force --delete feat/x",
                ],
            ),
            (
                "branch delete via push",
                &[
                    "git push --delete origin feat/x",
                    "git push origin --delete feat/x",
                    "git push origin :feat/x",
                ],
            ),
            (
                "the +refspec force",
                &[
                    "git push origin +main",
                    "git push origin +refs/heads/main:refs/heads/main",
                ],
            ),
            (
                "the -C prefix moving the verb off the front",
                &[
                    "git -C /tmp/other push --force",
                    "git -C /tmp/other rebase main",
                ],
            ),
            (
                "the -c prefix moving the verb off the front",
                &["git -c core.pager=cat rebase main", "git -c x push --force"],
            ),
            (
                "switch onto main",
                &["git switch main", "git switch main --force"],
            ),
            (
                "merge through the api",
                &[
                    "gh api repos/o/r/pulls/12/merge -X PUT",
                    "gh api --method PUT repos/o/r/pulls/12/merge",
                ],
            ),
            (
                "branch delete and history rewrite through update-ref",
                &[
                    "git update-ref -d refs/heads/feat/x",
                    "git update-ref refs/heads/main abc123",
                ],
            ),
            (
                "mirror push force-updating every ref on the remote",
                &["git push --mirror origin", "git push origin --mirror"],
            ),
            (
                "prune push deleting remote refs with no local counterpart",
                &["git push --prune origin", "git push origin --prune"],
            ),
            (
                "branch delete through the api",
                &[
                    "gh api -X DELETE repos/o/r/git/refs/heads/feat/x",
                    "gh api --method DELETE repos/o/r/git/refs/heads/feat/x",
                ],
            ),
            (
                "shell indirection through a nested sh/bash/eval that hides the verb from a \
                 prefix-anchored glob",
                &[
                    "sh -c 'git push --force origin main'",
                    "bash -c 'gh pr merge 123'",
                    "eval 'git reset --hard HEAD~3'",
                ],
            ),
            (
                "the same indirection with a prefix in front of sh/bash/eval, which used to \
                 defeat their front anchor outright (fix 4)",
                &[
                    "env bash -c 'gh pr merge 123'",
                    "/bin/sh -c 'gh pr merge 123'",
                    "command eval 'git reset --hard HEAD~3'",
                    "env gh pr merge 123",
                    "nohup sh -c 'git push --force origin main'",
                    "sh -c \"gh pr merge 123\"",
                    "env -i bash -c 'gh pr merge 123'",
                ],
            ),
            (
                "an option-taking wrapper's own flags and operands sitting between the wrapper \
                 and the verb, which used to defeat fix 4's wrapper-name list outright (fix 5) \
                 — closed by token-suffix expansion in `tools::subcommands_of` rather than by \
                 naming `timeout` and every other option shape",
                &[
                    "nice -n 5 gh pr merge 123",
                    "stdbuf -o0 gh pr merge 123",
                    "setsid -f gh pr merge 123",
                    "env -i gh pr merge 123",
                    "env -u FOO gh pr merge 123",
                    "timeout 30 gh pr merge 123",
                    "gh pr merge 123",
                ],
            ),
        ];
        for (name, spellings) in table {
            for candidate in spellings {
                assert!(is_denied(candidate), "{name}: {candidate:?} must be denied");
            }
        }
        assert!(is_denied("cd /tmp && git push --force origin main"));
        assert!(is_denied("echo $(gh pr merge 123)"));
        assert!(is_denied("echo `git push --force origin main`"));
        assert!(is_denied("GIT_DIR=. gh pr merge 123"));
    }

    #[test]
    fn ordinary_operations_the_barrier_must_not_catch_are_not_denied() {
        for allowed in [
            "git push origin feat/x",
            "git push -u origin feat/x",
            "git push -u origin fix/PROJ-1-form-fields",
            "git push -u origin feat/PROJ-2-Dashboard",
            "git status",
            "git branch -d feat/x",
            "gh pr view 12",
            "gh pr create --fill",
            "git checkout feat/x",
            "git fetch origin",
            "git log --oneline",
            "env RUST_LOG=debug cargo test",
            "/usr/bin/git status",
            "nohup cargo build --release &",
            "echo 'hello world'",
            "cargo test --all",
        ] {
            assert!(!is_denied(allowed), "{allowed:?} must not be denied");
        }
    }

    #[test]
    fn nothing_observes_the_narrative_or_the_closing_keyword() {
        const OBSERVE: &str = include_str!("observe.rs");
        const DECIDE: &str = include_str!("decide.rs");
        for (name, source) in [("observe", OBSERVE), ("decide", DECIDE)] {
            for reader in ["narrative", "Closes #", "closes #", "surprised"] {
                assert!(
                    !source.contains(reader),
                    "`{name}` must not read the Run's prose back; it names `{reader}`"
                );
            }
        }
    }

    #[test]
    fn a_rate_limit_survives_two_spaces_between_the_words() {
        let doubled = serde_json::json!({"is_error": true, "result": "hit a rate  limit"});
        assert!(is_rate_limited(&doubled));
    }

    #[test]
    fn a_bare_429_with_no_matching_prose_anywhere_is_a_rate_limit() {
        let value: serde_json::Value = serde_json::from_str(RUN2_RATE_LIMITED).unwrap();
        assert_eq!(
            value.get("api_error_status").unwrap(),
            &serde_json::json!(429)
        );
        assert!(is_rate_limited(&value));

        let bare = serde_json::json!({"is_error": true, "api_error_status": 429, "result": "x"});
        assert!(is_rate_limited(&bare));
    }

    #[test]
    fn a_session_limit_that_never_says_rate_limit_is_a_rate_limit() {
        let session = serde_json::json!({
            "is_error": true,
            "result": "You've hit your session limit · resets 5pm (Europe/Berlin)",
        });
        assert!(is_rate_limited(&session));
    }

    #[test]
    fn a_successful_attempt_mentioning_a_limit_in_passing_is_not_rate_limited() {
        let mentions = serde_json::json!({"is_error": false, "result": "rate limit in passing"});
        assert!(!is_rate_limited(&mentions));
    }

    #[test]
    fn an_ordinary_crash_is_not_mistaken_for_a_rate_limit() {
        let crash = serde_json::json!({"is_error": true, "result": "TypeError: undefined"});
        assert!(!is_rate_limited(&crash));
        for raw in [RUN1_ATTEMPT_1, RUN1_ATTEMPT_2, RUN1_ATTEMPT_3] {
            let value: serde_json::Value = serde_json::from_str(raw).unwrap();
            assert!(
                !is_rate_limited(&value),
                "Run 1's dropped connections were crashes, not limits"
            );
        }
    }

    #[test]
    fn subtype_reads_success_on_the_attempts_that_died_so_it_is_not_the_outcome() {
        for (n, raw) in [
            (1, RUN1_ATTEMPT_1),
            (2, RUN1_ATTEMPT_2),
            (3, RUN1_ATTEMPT_3),
        ] {
            let found = classify(raw, "", Some(1), n, Mode::Resume, "start", "end");
            assert_eq!(found.subtype.as_deref(), Some("success"), "attempt {n}");
            assert!(found.is_error, "attempt {n} really did die");
            assert!(!found.done_promise, "attempt {n} promised nothing");
            assert_eq!(
                found.terminal_reason.as_deref(),
                Some("api_error"),
                "attempt {n}"
            );
        }
    }

    #[test]
    fn unparseable_stdout_becomes_a_record_that_says_so_and_keeps_the_tail() {
        for raw in [TRUNCATED, GARBAGE, EMPTY] {
            let found = classify(raw, "", Some(1), 1, Mode::Dispatch, "start", "end");
            assert!(!found.parse_ok, "this fixture does not parse");
            assert_eq!(found.subtype.as_deref(), Some("unparseable-output"));
            assert!(found.is_error);
            assert!(
                found.result_tail.chars().count() <= 1500,
                "a tail is never longer than what it was asked to keep"
            );
            assert!(
                raw.ends_with(found.result_tail.as_str()),
                "a tail must be a suffix of what it came from"
            );
        }
        let truncated = classify(TRUNCATED, "", Some(1), 1, Mode::Dispatch, "start", "end");
        assert_eq!(truncated.result_tail.chars().count(), 1500);
        assert_ne!(
            truncated.result_tail, TRUNCATED,
            "the boundary was crossed, so the tail must actually have been cut"
        );
        for raw in [GARBAGE, EMPTY] {
            let found = classify(raw, "", Some(1), 1, Mode::Dispatch, "start", "end");
            assert_eq!(
                found.result_tail, raw,
                "under the 1500-character boundary, the tail is everything there was"
            );
        }
    }

    #[test]
    fn a_killed_childs_empty_stdout_is_recorded_rather_than_lost() {
        let killed = classify("", "", None, 4, Mode::Resume, "start", "end");
        assert!(!killed.parse_ok);
        assert_eq!(killed.exit_code, None);
        assert!(
            killed.result_tail.is_empty(),
            "zero bytes is itself a recorded fact"
        );
    }

    #[test]
    fn the_done_promise_is_read_from_the_result_and_nowhere_else() {
        let promised = serde_json::json!({
            "is_error": false,
            "subtype": "success",
            "result": "PR is open, stopping here. <promise>DONE</promise>",
        })
        .to_string();
        assert!(classify(&promised, "", Some(0), 5, Mode::Resume, "s", "e").done_promise);

        let unpromised = serde_json::json!({
            "is_error": false,
            "subtype": "success",
            "result": "Made progress but the pipeline has not reached an open PR yet.",
        })
        .to_string();
        assert!(!classify(&unpromised, "", Some(0), 4, Mode::Resume, "s", "e").done_promise);
    }

    #[test]
    fn a_result_key_renamed_to_output_is_recorded_as_missing_not_as_an_empty_promise() {
        let found = classify(
            DEGRADED_RENAMED,
            "",
            Some(0),
            1,
            Mode::Dispatch,
            "start",
            "end",
        );
        assert!(found.parse_ok, "the payload itself is well-formed JSON");
        assert_eq!(
            found.subtype.as_deref(),
            Some("result-field-missing"),
            "distinct from `unparseable-output`: this payload parsed fine"
        );
        assert!(
            !found.done_promise,
            "nothing can be read from a key that never arrived"
        );
        assert!(
            found.result_tail.contains("<promise>DONE</promise>"),
            "the raw stdout still carries the promise text under its new name, so the fallback \
             to it keeps the diagnostic alive"
        );
    }

    #[test]
    fn a_recorded_denial_survives_onto_the_attempt() {
        let denied = serde_json::json!({
            "is_error": false,
            "result": "the push was refused",
            "permission_denials": [{"tool_name": "Bash", "tool_input": {"command": "git push --force"}}],
        })
        .to_string();
        let found = classify(&denied, "", Some(0), 1, Mode::CiBabysit, "s", "e");
        assert_eq!(found.permission_denials.len(), 1);
        assert_eq!(found.mode, Mode::CiBabysit);
    }

    #[test]
    fn the_ci_babysit_prompt_names_what_the_globs_will_refuse_anyway() {
        for named in [
            "merge",
            "force-push",
            "rebase",
            "hard-reset",
            "delete the branch",
        ] {
            assert!(
                CI_BABYSIT_PROMPT.contains(named),
                "the prompt must name {named}"
            );
        }
        assert!(CI_BABYSIT_PROMPT.contains("one invocation"));
        assert!(CI_BABYSIT_PROMPT.contains("do not open a second PR"));
        assert!(CI_BABYSIT_PROMPT.contains("Never weaken, trim or skip a step of `just verify`"));
    }

    fn a_clearance(note: &str) -> Clearance {
        Clearance {
            cleared_at: "2026-08-21T19:00:00+00:00".to_string(),
            note: note.to_string(),
        }
    }

    fn shaped(parse_ok: bool, cost: Option<f64>, turns: Option<u64>, limited: bool) -> Attempt {
        Attempt {
            n: 1,
            mode: Mode::Resume,
            started_at: "s".to_string(),
            ended_at: "e".to_string(),
            exit_code: Some(1),
            is_error: true,
            parse_ok,
            subtype: None,
            stop_reason: None,
            api_error_status: None,
            terminal_reason: None,
            num_turns: turns,
            total_cost_usd: cost,
            usage: None,
            permission_denials: vec![],
            done_promise: false,
            rate_limited: limited,
            result_tail: String::new(),
            fanout: Observed::Absent,
            transcript: Transcript::PredatesName,
        }
    }

    #[test]
    fn an_attempt_with_real_cost_and_many_turns_is_never_a_wait() {
        assert!(!shaped(true, Some(37.04), Some(187), true).is_wait());
        assert!(!shaped(true, Some(37.04), Some(187), false).is_wait());
    }

    #[test]
    fn an_attempt_with_explicit_zero_cost_and_one_turn_is_a_wait() {
        assert!(shaped(true, Some(0.0), Some(1), true).is_wait());
        assert!(
            shaped(true, Some(0.0), Some(1), false).is_wait(),
            "a Wait is never keyed on the rate-limit flag"
        );
    }

    #[test]
    fn an_attempt_that_parsed_with_both_fields_absent_is_not_a_wait() {
        assert!(!shaped(true, None, None, false).is_wait());
        assert!(!shaped(true, None, None, true).is_wait());
        assert!(!shaped(true, Some(37.04), None, false).is_wait());
        assert!(!shaped(true, None, Some(187), false).is_wait());
    }

    #[test]
    fn an_unparseable_attempt_is_never_a_wait_even_with_both_fields_absent() {
        assert!(!shaped(false, None, None, false).is_wait());
        assert!(!shaped(false, None, None, true).is_wait());
    }

    #[test]
    fn the_wait_arithmetic_reads_the_attempt_list_and_nothing_else() {
        let list = [
            shaped(true, Some(3.0), Some(40), false),
            shaped(true, Some(0.0), Some(1), true),
            shaped(false, None, None, false),
            shaped(true, None, None, true),
            shaped(true, Some(0.0), Some(0), true),
        ];
        assert_eq!(working(&list), 3);
        assert_eq!(trailing_waits(&list), 1);
        assert_eq!(trailing_waits(&[]), 0);
        assert_eq!(working(&[]), 0);
    }

    #[test]
    fn a_payload_amid_stdout_noise_still_classifies() {
        let payload = serde_json::json!({
            "is_error": true,
            "result": "You've hit your session limit · resets 5pm (Europe/Berlin)",
            "num_turns": 3,
            "total_cost_usd": 0.42,
        });
        let noisy = format!("WARNING: plugin cache stale\n{payload}\nretrying once\n");
        let found = classify(&noisy, "", Some(1), 2, Mode::Resume, "s", "e");
        assert!(found.parse_ok, "the payload amid noise is still a payload");
        assert!(found.rate_limited, "the limit must survive noise around it");
        assert_eq!(found.num_turns, Some(3));
        assert_eq!(found.total_cost_usd, Some(0.42));
    }

    #[test]
    fn noise_without_a_recoverable_payload_is_still_unparseable_output() {
        let found = classify(
            "fatal: worker exited mid-write { see dump above }",
            "",
            Some(1),
            1,
            Mode::Dispatch,
            "s",
            "e",
        );
        assert!(!found.parse_ok);
        assert_eq!(found.subtype.as_deref(), Some("unparseable-output"));
        assert_eq!(
            found.result_tail,
            "fatal: worker exited mid-write { see dump above }"
        );
    }

    #[test]
    fn a_rate_limit_that_killed_the_child_before_any_json_is_read_off_stderr() {
        let found = classify(
            "",
            "You've hit your session limit · resets 5pm (Europe/Berlin)",
            None,
            4,
            Mode::Resume,
            "s",
            "e",
        );
        assert!(!found.parse_ok);
        assert!(found.rate_limited, "a stderr-only limit must still be one");
    }

    #[test]
    fn an_ordinary_death_on_stderr_is_not_mistaken_for_a_rate_limit() {
        let found = classify(
            "",
            "thread 'main' panicked at 'out of memory', src/never.rs:3:5",
            Some(101),
            4,
            Mode::Resume,
            "s",
            "e",
        );
        assert!(!found.parse_ok);
        assert!(!found.rate_limited, "no needle in the prose, no limit");
    }

    #[test]
    fn a_successful_attempt_is_not_rate_limited_by_noise_on_its_stderr() {
        let payload = serde_json::json!({"is_error": false, "result": "done"}).to_string();
        let found = classify(
            &payload,
            "note: the rate limit docs have moved",
            Some(0),
            5,
            Mode::Dispatch,
            "s",
            "e",
        );
        assert!(found.parse_ok);
        assert!(!found.rate_limited);
    }

    #[test]
    fn the_mode_a_record_holds_round_trips() {
        for mode in [Mode::Dispatch, Mode::Resume, Mode::CiBabysit] {
            let text = serde_json::to_string(&mode).unwrap();
            assert_eq!(serde_json::from_str::<Mode>(&text).unwrap(), mode);
        }
        assert_eq!(
            serde_json::to_string(&Mode::CiBabysit).unwrap(),
            "\"ci-babysit\""
        );
    }

    const ALL_STAGES: [rung::Stage; 10] = [
        rung::Stage::Plan,
        rung::Stage::Triage,
        rung::Stage::PlanReview,
        rung::Stage::Work,
        rung::Stage::Simplify,
        rung::Stage::DiffTriage,
        rung::Stage::Review,
        rung::Stage::Validate,
        rung::Stage::Fixes,
        rung::Stage::Ship,
    ];

    fn stage_conditions() -> StageConditions<'static> {
        StageConditions {
            claude_bin: "/home/op/.grind/bin/claude",
            run_id: "run-20260822-001",
        }
    }

    fn stage_ctx<'a>(
        stage: rung::Stage,
        job: &'a Job,
        model: Option<&'a str>,
        notes: Option<&'a str>,
        landed: Option<&'a str>,
    ) -> StageContext<'a> {
        StageContext {
            stage,
            skill_text: "# Work\n\nDo the work and write the return.",
            stages_dir: "/home/op/.grind/runs/run-20260822-001/stages",
            worktree: "/home/op/.grind/runs/run-20260822-001/worktree",
            job,
            model,
            notes,
            landed,
        }
    }

    #[test]
    fn stage_session_ids_are_valid_uuids_deterministic_across_calls_and_distinct_per_stage() {
        fn uuid_shaped(text: &str) -> bool {
            let bytes = text.as_bytes();
            if bytes.len() != 36 {
                return false;
            }
            for (i, b) in bytes.iter().enumerate() {
                let ok = match i {
                    8 | 13 | 18 | 23 => *b == b'-',
                    14 => *b == b'4',
                    19 => matches!(b, b'8' | b'9' | b'a' | b'b'),
                    _ => matches!(b, b'0'..=b'9' | b'a'..=b'f'),
                };
                if !ok {
                    return false;
                }
            }
            true
        }
        let mut seen = std::collections::HashSet::new();
        for stage in ALL_STAGES {
            let id = stage_session_id("run-1", stage);
            assert!(uuid_shaped(&id), "{stage}: {id} is not a valid UUID");
            assert_eq!(
                stage_session_id("run-1", stage),
                id,
                "{stage}: the supervisor must re-derive the same id on --resume"
            );
            assert!(seen.insert(id), "{stage}: collides with another stage's id");
        }
    }

    #[test]
    fn a_stage_dispatch_opens_that_stages_own_session_with_no_plugin_flag() {
        let job = job();
        let ctx = stage_ctx(rung::Stage::Work, &job, None, None, None);
        let invocation = stage_dispatch(&stage_conditions(), &ctx);
        let argv = invocation.argv();
        assert!(argv.contains(&"--session-id".to_string()));
        let at = argv.iter().position(|a| a == "--session-id").unwrap();
        assert_eq!(
            argv[at + 1],
            stage_session_id("run-20260822-001", rung::Stage::Work)
        );
        assert!(!argv.contains(&"--resume".to_string()));
        assert!(!argv.contains(&"--plugin-dir".to_string()));
    }

    #[test]
    fn a_stage_resume_resumes_that_stages_own_session_with_no_plugin_flag() {
        let job = job();
        let ctx = stage_ctx(rung::Stage::Review, &job, None, None, None);
        let invocation = stage_resume(&stage_conditions(), &ctx, None);
        let argv = invocation.argv();
        assert!(argv.contains(&"--resume".to_string()));
        let at = argv.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(
            argv[at + 1],
            stage_session_id("run-20260822-001", rung::Stage::Review)
        );
        assert!(!argv.contains(&"--session-id".to_string()));
        assert!(!argv.contains(&"--plugin-dir".to_string()));
    }

    #[test]
    fn a_stage_model_puts_the_model_flag_on_the_argv_and_absence_omits_it() {
        let job = job();
        let with_model = stage_ctx(rung::Stage::Ship, &job, Some("claude-opus-5"), None, None);
        let invocation = stage_dispatch(&stage_conditions(), &with_model);
        assert!(
            invocation
                .argv()
                .windows(2)
                .any(|w| w[0] == "--model" && w[1] == "claude-opus-5")
        );

        let without_model = stage_ctx(rung::Stage::Ship, &job, None, None, None);
        let invocation = stage_dispatch(&stage_conditions(), &without_model);
        assert!(!invocation.argv().contains(&"--model".to_string()));
    }

    #[test]
    fn stage_invocation_routes_dispatch_and_resume_and_refuses_ci_babysit() {
        let job = job();
        let ctx = stage_ctx(rung::Stage::Fixes, &job, None, None, None);
        let dispatched = stage_invocation(&stage_conditions(), &ctx, Mode::Dispatch, None);
        assert_eq!(dispatched.mode(), Mode::Dispatch);
        let resumed = stage_invocation(&stage_conditions(), &ctx, Mode::Resume, None);
        assert_eq!(resumed.mode(), Mode::Resume);
    }

    #[test]
    #[should_panic(expected = "babysit continues Ship's session")]
    fn stage_invocation_panics_on_ci_babysit() {
        let job = job();
        let ctx = stage_ctx(rung::Stage::Ship, &job, None, None, None);
        let _ = stage_invocation(&stage_conditions(), &ctx, Mode::CiBabysit, None);
    }

    #[test]
    fn a_stage_prompt_carries_the_skill_text_verbatim_and_the_bounded_context() {
        let job = job();
        let ctx = stage_ctx(rung::Stage::Work, &job, None, None, None);
        let prompt = stage_dispatch(&stage_conditions(), &ctx)
            .prompt()
            .to_string();
        assert!(prompt.contains(ctx.skill_text));
        assert!(prompt.contains(ctx.stages_dir));
        assert!(prompt.contains(ctx.worktree));
        assert!(prompt.contains(&job.branch));
        assert!(prompt.contains(&job.base_branch));
        assert!(prompt.contains(&job.verify_entrypoint));
        assert!(prompt.contains(&job.done_predicate));
        assert!(prompt.contains(&job.anchor));
    }

    #[test]
    fn a_stage_resume_prompt_carries_the_stage_reentry_paragraph_and_no_dispatch_does() {
        let job = job();
        let ctx = stage_ctx(rung::Stage::Work, &job, None, None, None);
        let resumed = stage_resume(&stage_conditions(), &ctx, None)
            .prompt()
            .to_string();
        assert!(resumed.contains("Re-read this stage's own return file"));

        let dispatched = stage_dispatch(&stage_conditions(), &ctx)
            .prompt()
            .to_string();
        assert!(!dispatched.contains("Re-read this stage's own return file"));
    }

    #[test]
    fn a_stage_resume_prompt_carries_the_latest_clearance_when_given_and_nothing_otherwise() {
        let job = job();
        let ctx = stage_ctx(rung::Stage::Fixes, &job, None, None, None);
        let note = "the CI runner was fixed";
        let cleared = a_clearance(note);
        let with_note = stage_resume(&stage_conditions(), &ctx, Some(&cleared))
            .prompt()
            .to_string();
        assert!(with_note.contains("Since you stopped, the human reports"));
        assert!(with_note.contains(note));

        let without_note = stage_resume(&stage_conditions(), &ctx, None)
            .prompt()
            .to_string();
        assert!(!without_note.contains("Since you stopped, the human reports"));
    }

    #[test]
    fn plan_alone_injects_notes_and_lessons_into_its_dispatch_prompt() {
        let job = job();
        let notes = "the last Run mistook a Wait for a crash; watch for that.";
        let plan_ctx = stage_ctx(rung::Stage::Plan, &job, None, Some(notes), None);
        let plan_prompt = stage_dispatch(&stage_conditions(), &plan_ctx)
            .prompt()
            .to_string();
        assert!(plan_prompt.contains(notes));

        let work_ctx = stage_ctx(rung::Stage::Work, &job, None, Some(notes), None);
        let work_prompt = stage_dispatch(&stage_conditions(), &work_ctx)
            .prompt()
            .to_string();
        assert!(!work_prompt.contains(notes));
    }

    #[test]
    fn a_work_resume_prompt_carries_the_landed_work_block_and_nothing_without_it() {
        let job = job();
        let landed = "Commits on this branch beyond base (main):\n  abc1234 R6: constructor sweep";
        let with_landed = stage_ctx(rung::Stage::Work, &job, None, None, Some(landed));
        let prompt = stage_resume(&stage_conditions(), &with_landed, None)
            .prompt()
            .to_string();
        assert!(prompt.contains("Work already proven on this branch from earlier attempts:"));
        assert!(prompt.contains(landed));
        assert!(prompt.contains("do not redo it"));

        let without_landed = stage_ctx(rung::Stage::Work, &job, None, None, None);
        let prompt = stage_resume(&stage_conditions(), &without_landed, None)
            .prompt()
            .to_string();
        assert!(!prompt.contains("Work already proven on this branch"));
    }

    #[test]
    fn landed_work_renders_only_for_the_work_stage_and_never_on_dispatch() {
        let job = job();
        let landed = "Commits on this branch beyond base (main):";
        let review_ctx = stage_ctx(rung::Stage::Review, &job, None, None, Some(landed));
        let resumed = stage_resume(&stage_conditions(), &review_ctx, None)
            .prompt()
            .to_string();
        assert!(!resumed.contains(landed));

        let work_ctx = stage_ctx(rung::Stage::Work, &job, None, None, Some(landed));
        let dispatched = stage_dispatch(&stage_conditions(), &work_ctx)
            .prompt()
            .to_string();
        assert!(!dispatched.contains(landed));
    }

    #[test]
    fn every_stage_denial_set_carries_the_full_base_list_verbatim() {
        for stage in ALL_STAGES {
            let denied = denied_for(stage);
            for glob in DENIED_TOOLS {
                assert!(
                    denied.iter().any(|g| g == glob),
                    "{stage} must carry {glob}"
                );
            }
        }
        let reflect = denied_for_reflect();
        for glob in DENIED_TOOLS {
            assert!(
                reflect.iter().any(|g| g == glob),
                "reflect must carry {glob}"
            );
        }
    }

    #[test]
    fn report_only_stages_deny_write_and_edit_and_the_writing_stages_do_not() {
        for stage in [
            rung::Stage::PlanReview,
            rung::Stage::Review,
            rung::Stage::Validate,
        ] {
            let denied = denied_for(stage);
            assert!(denied.contains(&"Write".to_string()), "{stage}");
            assert!(denied.contains(&"Edit".to_string()), "{stage}");
        }
        for stage in [rung::Stage::Work, rung::Stage::Fixes, rung::Stage::Ship] {
            let denied = denied_for(stage);
            assert!(!denied.contains(&"Write".to_string()), "{stage}");
            assert!(!denied.contains(&"Edit".to_string()), "{stage}");
        }
    }

    #[test]
    fn only_the_two_fan_out_panels_carry_the_write_capable_bash_forms() {
        for stage in [rung::Stage::Review, rung::Stage::Validate] {
            let denied = denied_for(stage);
            for glob in PANEL_BASH_FORMS {
                assert!(
                    denied.iter().any(|g| g == glob),
                    "{stage} must carry {glob}"
                );
            }
        }
        for stage in [
            rung::Stage::Plan,
            rung::Stage::Triage,
            rung::Stage::PlanReview,
            rung::Stage::Work,
            rung::Stage::Simplify,
            rung::Stage::DiffTriage,
            rung::Stage::Fixes,
            rung::Stage::Ship,
        ] {
            let denied = denied_for(stage);
            assert!(
                !denied.iter().any(|g| *g == "Bash(git commit*)"),
                "{stage} must not carry a panel-only Bash form"
            );
        }
    }

    #[test]
    fn reflect_carries_the_write_capable_bash_forms_but_not_write_or_edit() {
        let reflect = denied_for_reflect();
        for glob in PANEL_BASH_FORMS {
            assert!(
                reflect.iter().any(|g| g == glob),
                "reflect must carry {glob}"
            );
        }
        assert!(
            !reflect.contains(&"Write".to_string()),
            "reflect must still write its own artifacts"
        );
        assert!(!reflect.contains(&"Edit".to_string()));
    }

    #[test]
    fn grade_carries_the_write_capable_bash_forms_but_not_write_or_edit() {
        let grade = denied_for_grade();
        for glob in DENIED_TOOLS {
            assert!(grade.iter().any(|g| g == glob), "grade must carry {glob}");
        }
        for glob in PANEL_BASH_FORMS {
            assert!(grade.iter().any(|g| g == glob), "grade must carry {glob}");
        }
        assert!(
            !grade.contains(&"Write".to_string()),
            "grade must still write its one verdict file"
        );
        assert!(!grade.contains(&"Edit".to_string()));
    }

    #[test]
    fn grade_session_id_is_valid_uuid_deterministic_and_distinct_from_reflect_s() {
        let id = grade_session_id("run-20260822-001");
        assert_eq!(grade_session_id("run-20260822-001"), id);
        assert_eq!(id.len(), 36);
        assert_ne!(id, reflect_session_id("run-20260822-001"));
    }

    /// An attempt record written before the transcript name was recorded. Verbatim old shape:
    /// no `transcript_name` key anywhere.
    const OLD_SHAPED_ATTEMPT: &str = r#"{
        "n": 2,
        "mode": "resume",
        "started_at": "2026-08-21T18:00:00+00:00",
        "ended_at": "2026-08-21T18:40:00+00:00",
        "exit_code": 1,
        "is_error": true,
        "parse_ok": true,
        "subtype": "success",
        "stop_reason": null,
        "api_error_status": null,
        "terminal_reason": null,
        "num_turns": 37,
        "total_cost_usd": 2.35,
        "usage": null,
        "permission_denials": [],
        "done_promise": false,
        "rate_limited": false,
        "result_tail": "",
        "fanout": "absent"
    }"#;

    #[test]
    fn an_attempt_written_before_the_transcript_name_existed_still_decodes() {
        let decoded: Attempt = serde_json::from_str(OLD_SHAPED_ATTEMPT).expect("old shape decodes");
        assert_eq!(decoded.n, 2);
        assert_eq!(decoded.mode, Mode::Resume);
        assert!(decoded.total_cost_usd.is_some());
        assert_eq!(
            decoded.transcript,
            Transcript::PredatesName,
            "a record written before the name existed defaults it away, not into a wrong name"
        );
    }

    /// The writer serializes a bare `None` as `"transcript_name": null` — key present, value
    /// null — so every transcript-less row already on disk carries this exact byte shape.
    /// It must land on `PredatesName`, indistinguishable in meaning from the absent-key case:
    /// both mean *the record predates the name*, never *the attempt wrote none*.
    #[test]
    fn an_explicit_null_transcript_name_decodes_as_predating_the_name() {
        let with_null = OLD_SHAPED_ATTEMPT.replace(
            "\"fanout\": \"absent\"",
            "\"transcript_name\": null, \"fanout\": \"absent\"",
        );
        assert!(with_null.contains("\"transcript_name\": null"));
        let decoded: Attempt = serde_json::from_str(&with_null).expect("explicit null decodes");
        assert_eq!(
            decoded.transcript,
            Transcript::PredatesName,
            "an old writer's explicit silence is still silence about when the record was written, not about how the attempt ended"
        );
    }

    #[test]
    fn a_recorded_transcript_name_round_trips_through_the_record() {
        let named = Attempt {
            transcript: Transcript::Recorded("messages-2-2.jsonl".to_string()),
            ..shaped(true, Some(2.35), Some(37), false)
        };
        let text = serde_json::to_string(&named).unwrap();
        assert_eq!(serde_json::from_str::<Attempt>(&text).unwrap(), named);
    }

    /// Done-predicate assertion (b) read strictly: a record encoding the name decodes to
    /// carries-that-name — where "encodes" is today's writer's historical shape, the bare
    /// string under `transcript_name`, hand-written so the test cannot silently track a
    /// grammar that drifted. Encode/decode symmetry alone would stay green through any
    /// self-consistent spelling, including one old records could never produce.
    #[test]
    fn a_bare_transcript_name_under_the_key_decodes_to_the_carried_name() {
        let text = OLD_SHAPED_ATTEMPT.replace(
            "\"fanout\"",
            "\"transcript_name\": \"messages-2-2.jsonl\", \"fanout\"",
        );
        assert!(text.contains("\"transcript_name\": \"messages-2-2.jsonl\""));
        let decoded: Attempt = serde_json::from_str(&text).expect("bare-string shape decodes");
        assert_eq!(
            decoded.transcript,
            Transcript::Recorded("messages-2-2.jsonl".to_string()),
        );
    }

    /// The writer's spell is pinned on the wire, not just round-tripped: the bare string for
    /// a carried name, and `{"wrote_none": null}` — a shape no record written before this
    /// change can carry — for the stated fact.
    #[test]
    fn the_writer_states_a_carried_name_as_the_bare_string() {
        let named = Attempt {
            transcript: Transcript::Recorded("messages-2-2.jsonl".to_string()),
            ..shaped(true, Some(2.35), Some(37), false)
        };
        assert!(
            serde_json::to_string(&named)
                .unwrap()
                .contains("\"transcript_name\":\"messages-2-2.jsonl\""),
            "a carried name must stay byte-compatible with today's writer"
        );
    }

    #[test]
    fn the_writer_states_wrote_none_as_the_explicit_object_and_nothing_else() {
        let written = serde_json::to_string(&Attempt {
            transcript: Transcript::WroteNone,
            ..shaped(true, Some(2.35), Some(37), false)
        })
        .unwrap();
        assert!(
            written.contains("\"transcript_name\":{\"wrote_none\":null}"),
            "{written}"
        );
    }

    #[test]
    fn a_predating_row_is_written_as_silence_under_the_key_again() {
        let legacy = Attempt {
            transcript: Transcript::PredatesName,
            ..shaped(true, Some(2.35), Some(37), false)
        };
        assert!(
            !serde_json::to_string(&legacy)
                .unwrap()
                .contains("transcript"),
            "re-encoding an old row must not forge a statement about when it was written"
        );
    }

    /// `WroteNone` is unreachable from any record written before this change — no byte shape
    /// can produce it. Its round-trip pins that the encoder states it and the decoder reads
    /// back exactly what was stated.
    #[test]
    fn a_wrote_none_transcript_round_trips_through_the_record() {
        let unwritten = Attempt {
            transcript: Transcript::WroteNone,
            ..shaped(true, Some(2.35), Some(37), false)
        };
        let text = serde_json::to_string(&unwritten).unwrap();
        assert_eq!(serde_json::from_str::<Attempt>(&text).unwrap(), unwritten);
    }
}
