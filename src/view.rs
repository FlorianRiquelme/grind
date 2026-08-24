//! Reading a Run without being able to damage it.
//!
//! **There is no save path in this module, and there must never be one.** The shipped bug this
//! closes is `cmd_status` calling `save()` on what it loaded: a whole-record write from a read
//! path can erase `attempts[]`, which nothing can rebuild — and it erases it while the human is
//! watching the dashboard to be reassured (issues #12, #27).
//!
//! `RunView` derives `Deserialize` only. It never sees the writable record type, and it must
//! never be nested under whatever module owns it: a child reaches its ancestor's private items
//! and compiles clean. Field names are duplicated by design, and the carrier for that is a test
//! that both readers parse the same bytes — not the compiler, which is blind to it precisely
//! because the wall is working.

use crate::attempt::{self, Attempt, Clearance};
use crate::decide::{self, Decision, Stage, Verdict, VerifyContract, VerifyCoverage};
use crate::job::{self, Job};
use crate::observe::{self, Observation, Observed, Reason, RunOutcome};
use crate::policy;
use crate::world;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// The record, read-only. `deny_unknown_fields` is what turns *the writer gained a field and
/// the reader forgot it* into a failing test — without it serde drops the unknown key silently,
/// because a field the reader never declares is not a shared field.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunView {
    pub run_id: String,
    pub created_at: String,
    pub state: String,
    pub job: Job,
    /// Pre-cutover residue: the resolved plugin cache path old records carried, read by
    /// nothing. `#[serde(default)]` so both a fresh record (which never writes the key) and a
    /// pre-cutover fixture (which still carries it under `deny_unknown_fields`) parse.
    #[serde(default)]
    pub plugin_dir: String,
    pub repo_path: String,
    pub worktree: String,
    pub session_id: String,
    pub claude_bin: String,
    pub model: Option<String>,
    pub denied_tools: Vec<String>,
    pub hostname: String,
    pub attempt_budget: usize,
    pub limit_sleep_seconds: u64,
    /// Absent on a pre-cutover record, the same reasoning `clearances` documents.
    #[serde(default)]
    pub plan_revisions: usize,
    #[serde(default)]
    pub fix_rounds: usize,
    pub supervisor_pid: u32,
    pub supervisor_identity: Option<String>,
    pub attempts: Vec<Attempt>,
    /// Absent in records written before the `cleared` verb existed, which is the same fact
    /// as empty — see the writer's field for why that default is honest.
    #[serde(default)]
    pub clearances: Vec<Clearance>,
    /// Empty on a pre-cutover record — the same fact `supervisor`'s legacy-path gate reads.
    #[serde(default)]
    pub stages: Vec<crate::rung::StageEntry>,
    #[serde(default)]
    pub provenance: Option<Provenance>,
    #[serde(default)]
    pub reflected: bool,
    /// Which adapter executes this Run's stages — snapshotted at dispatch (ADR-0017), the
    /// read-only mirror of the writer's fields of the same names. Both default so every
    /// record written before selection existed parses under `deny_unknown_fields`.
    #[serde(default)]
    pub backend: crate::runner::Backend,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_override: Option<String>,
    /// The rest of the layout-declared selection (ADR-0017 as amended): the host's model
    /// classes and a declared wire mode. `None` is the honest *undeclared* answer — both for a
    /// record written before the grammar grew these keys and for a host that declares only a
    /// backend — so the adapter falls back to its own default rather than to a blank.
    ///
    /// These three exist here for one reason: `RunView` is `deny_unknown_fields`, so the first
    /// `run.json` a Dispatch writes with a `fast=`/`strong=`/`proto=` declaration would fail to
    /// parse without them, and it would fail in `grind status` and the dashboard — the two
    /// places a human looks to be reassured. Never let the writer gain a field this mirror lacks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_model_override: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strong_model_override: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proto_override: Option<crate::runner::ProtoMode>,
}

/// The read-only mirror of `supervisor::Provenance`. Field names duplicated by design, the same
/// carrier `RunView` itself is for `RunRecord` — see this module's own doc comment.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub binary_version: String,
    pub skills_hash: String,
}

impl RunView {
    /// *attempt N of M*, with **M from the record** and N counting **working Attempts only**.
    /// Re-entering under a different environment cannot make this misreport a Run's own budget,
    /// and a Run that spent six Attempts probing a wall is not six Attempts into its budget.
    pub fn attempt_counter(&self) -> (usize, usize) {
        (attempt::working(&self.attempts), self.attempt_budget)
    }

    pub fn total_spend(&self) -> f64 {
        self.attempts.iter().filter_map(|a| a.total_cost_usd).sum()
    }

    pub fn denial_count(&self) -> usize {
        self.attempts
            .iter()
            .map(|a| a.permission_denials.len())
            .sum()
    }
}

/// Everything a terminal Run is reported from, **owned and built once**.
///
/// The Handback and the Job-issue comment differ only in where they send the human to look — a
/// terminal wants fixed width, markdown wants a table. Two independently-chosen lists would
/// drift *invisibly*, because nobody ever sees both renderings of one Run. So there is one fact
/// set and two renderers over it, and this is the fact set.
///
/// It lives here rather than in `render` because gathering it reads the world, and `render` is
/// pure — every function there returns a `String`.
pub struct Facts {
    pub found: RunView,
    pub observation: Observation,
    /// The **fresh** verdict, never the recorded state. The two are produced moments apart, and
    /// Run 2's Handback printed `[exhausted]` over an open, green, twelve-commit PR.
    pub verdict: Verdict,
    pub contract: VerifyContract,
    pub coverage: Observed<VerifyCoverage>,
    pub furthest: Stage,
    /// What must be cleared, when the Run stopped for a human. The recorded state is consulted
    /// for this one fact — no fresh verdict can carry it — and never printed as a verdict.
    pub blocker: Option<String>,
    /// The latest clearance, when the human has recorded one. Read unconditionally rather
    /// than only on a blocked record: a fact about the world does not expire, so it rides
    /// every later surface (R3) — and it decides nothing, appearing only when it exists.
    pub cleared: Option<Clearance>,
    pub run_state: PathBuf,
    /// The tier Decision Triage recorded, read fresh — `None` on a pre-cutover Run or one that
    /// never reached the ladder.
    pub triage_decision: Option<Decision>,
    /// Diff-triage's own Decision, when the Run reached it. Escalation-only over
    /// `triage_decision` (`decide::select_tier`'s own rule), never shown as a replacement.
    pub diff_triage_decision: Option<Decision>,
    /// `outcome.json`, when `grind outcomes` has run for this Run. Merged/closed/reverted
    /// facts, never a grade.
    pub outcome: Option<RunOutcome>,
    /// Reflect's calibration row, when the pass wrote one.
    pub calibration: Option<serde_json::Value>,
}

/// Gather them. **One construction**, reached from `cli`'s Handback and from the supervisor's
/// terminal comment alike, so the two renderings cannot be fed different lists.
pub fn gather(home: &Path, run_id: &str) -> Option<Facts> {
    let Lookup::Here(found) = load(home, run_id) else {
        return None;
    };
    let found = *found;
    let observation = observe_fresh(
        Path::new(&found.worktree),
        &found.job.handoff_sha,
        &found.job.branch,
        &found.job.base_branch,
        world::now_iso(),
    );
    let signals = decide::signals_of(&observation);
    let promised = found.attempts.last().is_some_and(|a| a.done_promise);
    let verdict = decide::verdict(&signals, promised);
    let contract = verify_contract_of(&found.worktree);
    let coverage = decide::verify_coverage(&contract, &observation.changed_files);
    let furthest = decide::furthest_stage(&observation);
    let blocker = (found.state == "blocked")
        .then(|| policy::what_must_be_cleared(&found.attempts))
        .flatten();
    let cleared = found.clearances.last().cloned();
    let run_dir = job::runs_dir(home).join(run_id);
    Some(Facts {
        run_state: record_path(home, run_id),
        triage_decision: decision_of(&run_dir, "triage"),
        diff_triage_decision: decision_of(&run_dir, "diff-triage"),
        outcome: outcome_of(&run_dir),
        calibration: calibration_of(&run_dir),
        found,
        observation,
        verdict,
        contract,
        coverage,
        furthest,
        blocker,
        cleared,
    })
}

/// Which contracted steps the target repo declares, read off its worktree. A justfile may
/// legitimately delegate to npm scripts, so `package.json`'s scripts count as evidence.
pub fn verify_contract_of(worktree: &str) -> VerifyContract {
    let worktree = Path::new(worktree);
    let justfile = world::read_to_string(&worktree.join("justfile")).ok();
    let package = world::read_to_string(&worktree.join("package.json")).ok();
    decide::verify_contract(justfile.as_deref(), package.as_deref())
}

/// The answer to *is this Run here*. **An unknown run id is `NotHere`, never an error** — a Run
/// on another box should send the operator to its Job issue rather than look like a typo.
#[derive(Debug)]
pub enum Lookup {
    Here(Box<RunView>),
    NotHere,
    Unreadable(Reason),
}

pub fn load(home: &Path, run_id: &str) -> Lookup {
    let path = record_path(home, run_id);
    if !world::exists(&path) {
        return Lookup::NotHere;
    }
    let Ok(raw) = world::read_to_string(&path) else {
        return Lookup::Unreadable(Reason::saying(&format!("{}: unreadable", path.display())));
    };
    match serde_json::from_str::<RunView>(&raw) {
        Ok(found) => Lookup::Here(Box::new(found)),
        Err(e) => Lookup::Unreadable(Reason::saying(&format!("{}: {e}", path.display()))),
    }
}

pub fn record_path(home: &Path, run_id: &str) -> PathBuf {
    job::runs_dir(home).join(run_id).join("run.json")
}

// --- Grit surfaces, read fresh from a Run's own directory ------------------------------------
//
// Every function here follows `observe_fresh`'s discipline: read `world`, persist nothing. A
// Decision, an Outcome and a proposal are facts a Run already left on disk — this module names
// where to look and how to fold a bad or absent file into *nothing to show*, never a crash.

/// `stages/<stage>/decision.json`, tolerant: absent or unparseable is `None` rather than an
/// error, the same rule `run_r_pass` writes it under. A pre-cutover Run or a Run that never
/// reached Diff-triage carries no such file, which is not damage.
pub fn decision_of(run_dir: &Path, stage: &str) -> Option<Decision> {
    let text =
        world::read_to_string(&run_dir.join("stages").join(stage).join("decision.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// `outcome.json`, written beside the record by `grind outcomes` — never `run.json`, and never
/// read by anything that could save it back (issues #12, #27's rule applies here too).
pub fn outcome_of(run_dir: &Path) -> Option<RunOutcome> {
    let text = world::read_to_string(&run_dir.join("outcome.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// Reflect's calibration row, if the pass wrote one. Its shape is a `select_tier` replay
/// against what Validate actually confirmed — reflect's own words, not a schema this base
/// forces — so it is read as a bare JSON object and rendered as key/value facts rather than a
/// typed struct: a field Reflect adds or drops must not turn *a row exists* into *unreadable*.
pub fn calibration_of(run_dir: &Path) -> Option<serde_json::Value> {
    let text = world::read_to_string(
        &run_dir
            .join("stages")
            .join("reflect")
            .join("calibration.json"),
    )
    .ok()?;
    serde_json::from_str(&text).ok()
}

/// One artifact in a Run's proposal queue: a drafted follow-up Job or a proposed skill diff,
/// exactly as Reflect left it under `stages/reflect/`. Nothing stores this — [`proposals_in`]
/// derives it fresh on every request (ADR-0013's derivability rule), the same shape the design
/// names at line 160: a GET-only projection.
#[derive(Debug, Clone, PartialEq)]
pub struct ProposalEntry {
    /// `"job"` or `"diff"` — which of Reflect's two drafted-artifact kinds this is.
    pub kind: &'static str,
    pub path: PathBuf,
    /// The artifact's own words, one line: a drafted Job's title, or a diff's first hunk
    /// header — whichever the file yields — capped through [`one_line`] like every other
    /// operator-facing string this module produces.
    pub summary: String,
}

/// Reflect's SKILL.md names two proposal shapes by description rather than by filename — "a
/// drafted issue body" and "a diff in the proposal queue, never a write to the skill" — so this
/// reads `stages/reflect/jobs/` for the former and `stages/reflect/diffs/` for the latter: one
/// subdirectory per kind, named for what it holds, so the scan never has to guess a file's kind
/// from its extension. Absent directories yield nothing rather than an error.
pub fn proposals_in(run_dir: &Path) -> Vec<ProposalEntry> {
    let reflect_dir = run_dir.join("stages").join("reflect");
    let mut found = Vec::new();
    for (kind, sub) in [("job", "jobs"), ("diff", "diffs")] {
        for path in world::list_dir(&reflect_dir.join(sub)) {
            if world::is_dir(&path) {
                continue;
            }
            let summary = world::read_to_string(&path)
                .ok()
                .map(|text| one_line(text.lines().find(|l| !l.trim().is_empty()).unwrap_or("")))
                .unwrap_or_else(|| "(unreadable)".to_string());
            found.push(ProposalEntry {
                kind,
                path: path.clone(),
                summary,
            });
        }
    }
    found
}

/// The proposal queue across every Run on this host — a roster projection over
/// [`proposals_in`], never a store of its own. Each entry keeps the run id it came from so the
/// queue can point back at the Run that drafted it.
pub fn proposal_queue(home: &Path) -> Vec<(String, ProposalEntry)> {
    let mut found = Vec::new();
    for entry in world::list_dir(&job::runs_dir(home)) {
        let Some(run_id) = entry.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        for proposal in proposals_in(&entry) {
            found.push((run_id.to_string(), proposal));
        }
    }
    found
}

/// One row of the roster.
#[derive(Debug)]
pub struct RosterRow {
    pub run_id: String,
    pub recorded_state: String,
    pub branch: String,
    pub job_url: String,
    /// Observed for itself rather than taken from the record: a Run sitting at `dispatched`
    /// with a dead supervisor reads as *supervisor gone*, which no recorded state can say
    /// because there is deliberately no `running` state to leave behind.
    pub supervisor_here: Observed<bool>,
    pub attempts: (usize, usize),
    /// The Job issue's title, verbatim from the record.
    pub job_title: String,
    /// Spend summed across the Attempts the record carries.
    pub spend: f64,
    /// The later of the record's creation and the newest Attempt end the record carries —
    /// recorded values only, never synthesized (KTD14).
    pub last_activity: String,
}

/// Every Run on **this host**. Nothing syncs and nothing is portable; the only thing that
/// travels is a pointer, and it travels on the Job issue.
pub fn roster(home: &Path) -> Vec<RosterRow> {
    let mut rows = Vec::new();
    for entry in world::list_dir(&job::runs_dir(home)) {
        let Some(run_id) = entry.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Lookup::Here(found) = load(home, run_id) {
            rows.push(RosterRow {
                supervisor_here: supervisor_here(
                    found.supervisor_identity.as_deref(),
                    &crate::observe::process_start_stamp(&world::ps_start_stamp(
                        found.supervisor_pid,
                    )),
                ),
                run_id: found.run_id.clone(),
                recorded_state: found.state.clone(),
                branch: found.job.branch.clone(),
                job_url: found.job.url.clone(),
                attempts: found.attempt_counter(),
                job_title: found.job.title.clone(),
                spend: found.total_spend(),
                last_activity: last_activity(&found.created_at, &found.attempts),
            });
        }
    }
    rows
}

/// The later of the record's creation and the newest Attempt end it carries. Every
/// timestamp in a record is written by [`crate::world`] in one UTC spelling, so the order
/// of the recorded strings *is* the order of the moments, and the later one is a plain
/// string max. Only values the record carries compete here — an Attempt without an end
/// contributes nothing and nothing is invented in its place (KTD14).
fn last_activity(created_at: &str, attempts: &[Attempt]) -> String {
    match attempts.iter().map(|a| a.ended_at.as_str()).max() {
        Some(end) if end > created_at => end.to_string(),
        _ => created_at.to_string(),
    }
}

/// Liveness splits into supervisor presence and progress; this is the presence half.
///
/// **Pid *and* identity.** A pid alone is reused, and a reused pid reporting a dead Run as
/// alive is exactly the reassurance that sends an operator back to sleep.
pub fn supervisor_here(recorded: Option<&str>, live: &Observed<String>) -> Observed<bool> {
    match (recorded, live) {
        // **`ps` could not be asked, which is not the supervisor being gone.** This arm used to
        // read `None` and answer `Present(false)`, and `resume --all` acts on that answer: on a
        // host whose `ps` cannot spawn, every Run read as cut off and every Run was re-entered.
        (_, Observed::Unobservable(reason)) => Observed::Unobservable(reason.clone()),
        // Nothing is running under that pid at all.
        (_, Observed::Absent) => Observed::Present(false),
        // The pid is alive but nothing was recorded to compare it against, so this is a
        // question Grind cannot answer rather than a yes.
        (None, Observed::Present(_)) => Observed::Unobservable(Reason::saying(
            "no supervisor identity was recorded at dispatch",
        )),
        (Some(was), Observed::Present(now)) => Observed::Present(was.trim() == now.trim()),
    }
}

/// One line, capped: whitespace flattened and at most 100 characters, so a long value renders
/// at a fixed width. `render` reuses this for fan-out descriptions — five long Agent
/// descriptions wrapping differently on every `watch` refresh is the jitter the fixed view
/// shape exists to prevent.
pub(crate) fn one_line(text: &str) -> String {
    let flattened: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flattened.chars().count() > 100 {
        flattened.chars().take(99).collect::<String>() + "…"
    } else {
        flattened
    }
}

/// Observe a Run's durable artifacts fresh. **Reads and never writes** — this path observes and
/// persists nothing, which is the whole difference from the script's `cmd_status`.
pub fn observe_fresh(
    worktree: &Path,
    handoff_sha: &str,
    job_branch: &str,
    declared_base: &str,
    at: String,
) -> Observation {
    let mut run = |argv: &[String]| world::run(argv, Some(worktree));
    observe::observe_run(at, handoff_sha, job_branch, declared_base, &mut run)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The transcript matchers moved verbatim to `runner::claude`; every assertion below is
    // unchanged and simply names them through the new path.
    use crate::claude::{
        Fanout, assistant_now, fanout, fanout_counts, fanout_since, last_words, live, now_skill,
        transcript_path,
    };

    const DAY_ONE: &str = include_str!("../tests/fixtures/record/day-one.json");
    const RENAMED_FIELD: &str = include_str!("../tests/fixtures/transcript/renamed-field.jsonl");
    const TYPE_CHANGED: &str = include_str!("../tests/fixtures/transcript/type-changed.jsonl");
    const EMPTY: &str = include_str!("../tests/fixtures/transcript/empty.jsonl");
    const FANOUT_PARENT: &str = include_str!(
        "../tests/fixtures/transcript/fanout/8f2c1a70-4b3d-4e51-9c02-6a7d5e8b1f43.jsonl"
    );
    const FANOUT_AGENT: &str =
        include_str!("../tests/fixtures/transcript/fanout/spelling-agent.jsonl");
    const FANOUT_UNRECOGNISED: &str =
        include_str!("../tests/fixtures/transcript/fanout/no-recognised-spawn.jsonl");

    #[test]
    fn the_read_only_reader_parses_the_same_fixture_the_writer_does() {
        let found: RunView = serde_json::from_str(DAY_ONE).expect("the base's record shape");
        assert_eq!(found.run_id, "20260806-122620-snapper-28");
        // Four Attempts, of which one — attempt 3, $0 and one turn — did no work.
        assert_eq!(found.attempt_counter(), (3, 8));
        assert_eq!(found.attempts.len(), 4);
        assert_eq!(found.denied_tools.len(), 7);
        assert_eq!(found.denial_count(), 1);
        assert!(
            found.clearances.is_empty(),
            "a record written before the `cleared` verb reads as empty, not as unreadable"
        );
        assert!(
            (found.total_spend() - 26.69).abs() < 0.001,
            "{}",
            found.total_spend()
        );
    }

    #[test]
    fn the_scripts_record_shape_is_refused_rather_than_half_parsed() {
        // There is no migration read path. A record missing what the base forces at
        // construction is not something this reader half-understands.
        let script_shaped = serde_json::json!({
            "run_id": "20260802-105828-snapper-21",
            "created_at": "2026-08-02T10:58:28+00:00",
            "state": "completed",
            "plugin_dir": "/x",
            "worktree": "/y",
            "session_id": "s",
            "denied_tools": [],
            "attempts": [],
        })
        .to_string();
        assert!(serde_json::from_str::<RunView>(&script_shaped).is_err());
    }

    #[test]
    fn attempt_n_of_m_reads_m_from_the_record() {
        let mut value: serde_json::Value = serde_json::from_str(DAY_ONE).unwrap();
        value["attempt_budget"] = serde_json::json!(3);
        let found: RunView = serde_json::from_value(value).unwrap();
        // The environment has no say: there is no override to read, and M is a record field.
        assert_eq!(found.attempt_counter(), (3, 3));
    }

    fn stamp(said: &str) -> Observed<String> {
        Observed::Present(said.to_string())
    }

    #[test]
    fn a_dead_supervisor_reads_as_gone_however_it_died() {
        // Nothing is running under that pid.
        assert_eq!(
            supervisor_here(Some("Thu Aug  6 12:26:20 2026"), &Observed::Absent),
            Observed::Present(false)
        );
        // The pid exists but belongs to something else — a reused pid must not report a dead
        // Run as alive.
        assert_eq!(
            supervisor_here(
                Some("Thu Aug  6 12:26:20 2026"),
                &stamp("Thu Aug  6 18:02:11 2026")
            ),
            Observed::Present(false)
        );
        // Alive and the same process.
        assert_eq!(
            supervisor_here(
                Some("Thu Aug  6 12:26:20 2026"),
                &stamp("Thu Aug  6 12:26:20 2026")
            ),
            Observed::Present(true)
        );
    }

    #[test]
    fn an_unrecorded_identity_is_a_question_grind_cannot_answer() {
        let found = supervisor_here(None, &stamp("Thu Aug  6 12:26:20 2026"));
        assert!(matches!(found, Observed::Unobservable(_)), "{found:?}");
    }

    #[test]
    fn a_ps_that_could_not_answer_is_never_a_supervisor_that_is_gone() {
        // The input `resume --all` acts on. `ps -p <pid> -o lstart=` is a procps/BSD spelling
        // busybox does not implement, and Grind ships as a musl static binary aimed at exactly
        // those hosts — so *could not ask* reaches this function on an ordinary box.
        let blind = crate::observe::process_start_stamp(&crate::world::Completed {
            stdout: String::new(),
            stderr: "ps: unrecognized option: p\n".to_string(),
            code: Some(127),
        });
        assert!(matches!(blind, Observed::Unobservable(_)), "{blind:?}");
        for recorded in [None, Some("Thu Aug  6 12:26:20 2026")] {
            let found = supervisor_here(recorded, &blind);
            assert!(
                matches!(found, Observed::Unobservable(_)),
                "a blind reading must not collapse into `gone`: {found:?}"
            );
        }
    }

    #[test]
    fn a_transcript_with_an_unparseable_line_still_yields_the_parseable_ones() {
        let damaged = "{\"attributionSkill\":\"compound-engineering:ce-plan\"}\n\
             not json at all\n\
             {\"attributionSkill\":\"compound-engineering:ce-work\"}\n";
        assert_eq!(
            now_skill(damaged),
            Observed::Present("compound-engineering:ce-work".to_string())
        );
    }

    #[test]
    fn a_transcript_that_spawned_three_and_saw_three_return_has_nothing_running() {
        // The live view reads the whole append-only transcript: without pairing against
        // `tool_result`, attempt 1's three agents read as *3 agents* forever after they all
        // returned — finished work presented as currently-running.
        assert_eq!(fanout(&spawning(3, 3)), Observed::Absent);
    }

    #[test]
    fn the_spawn_without_a_paired_result_is_the_one_still_listed_as_running() {
        let found = fanout(&spawning(3, 2));
        let Observed::Present(running) = found else {
            panic!("the unpaired spawn is still running: {found:?}");
        };
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].description, "do the thing 2");
    }

    #[test]
    fn a_spawn_without_an_id_cannot_pair_and_stays_listed() {
        // The same assumed pairing `fanout_counts` states: where a spawn carries no id it
        // counts as spawned and never returned, so the live view keeps it listed.
        let transcript = r#"{"message":{"content":[{"type":"tool_use","name":"Agent","input":{"description":"idless"}}]}}"#;
        let found = fanout(transcript);
        assert_eq!(
            found,
            Observed::Present(vec![Fanout {
                description: "idless".to_string()
            }])
        );
    }

    #[test]
    fn a_bare_top_level_description_line_is_not_a_spawn() {
        // The top-level `description` field belongs to subagent side-chain lines in
        // `<session>/subagents/*.jsonl` — files this view reads only for freshness. The parent
        // transcript never carries one, so matching it counted unrelated lines as spawns that
        // could never pair away.
        let transcript = concat!(
            r#"{"description":"unrelated prose","message":{"role":"user"}}"#,
            "\n",
            r#"{"message":{"content":[{"type":"tool_use","id":"t1","name":"Agent","input":{"description":"real spawn"}}]}}"#,
        );
        let found = fanout(transcript);
        assert_eq!(
            found,
            Observed::Present(vec![Fanout {
                description: "real spawn".to_string()
            }])
        );
        // And alone, such a line is no recognised spawn at all — the empty-transcript rule.
        assert_eq!(
            fanout(r#"{"description":"unrelated prose"}"#),
            Observed::Absent
        );
    }

    #[test]
    fn a_field_that_changed_name_or_type_costs_its_own_line_and_nothing_else() {
        // The same real file changes field names and field types between its own lines, which
        // is why this is read tolerantly and the child's stdout is read strictly.
        for damaged in [RENAMED_FIELD, TYPE_CHANGED] {
            let found = now_skill(damaged);
            assert!(
                !matches!(found, Observed::Unobservable(_)),
                "a damaged line must not cost the whole file: {found:?}"
            );
        }
        assert_eq!(now_skill(EMPTY), Observed::Absent);
    }

    #[test]
    fn fan_out_reads_as_a_count_with_descriptions_under_either_tool_name() {
        // Support, not proof: both of these are authored, so they assert the matcher against
        // itself. The load-bearing assertion is the negative-recognition test below.
        let Observed::Present(former) = fanout(FANOUT_PARENT) else {
            panic!("the former spelling is still recognised");
        };
        assert_eq!(former.len(), 1);
        assert_eq!(former[0].description, "review the diff for regressions");

        let Observed::Present(current) = fanout(FANOUT_AGENT) else {
            panic!("the current spelling is recognised");
        };
        assert_eq!(current.len(), 2);
        assert_eq!(current[0].description, "review the diff for regressions");
    }

    #[test]
    fn tool_calls_with_no_recognised_spawn_are_could_not_observe_and_never_absent() {
        // The one authoring cannot fake. Widening the matcher alone would leave the next rename
        // exactly as silent as this one was: 203 spawns to 0 across sixty transcripts, printed
        // as `none` every time.
        let found = fanout(FANOUT_UNRECOGNISED);
        let Observed::Unobservable(reason) = &found else {
            panic!("a transcript full of tool calls is not a Run that fanned out to nobody");
        };
        assert!(
            reason.to_string().contains('3'),
            "the tool-call count is in the reason: {reason}"
        );
        assert_ne!(found, Observed::Absent);
    }

    #[test]
    fn an_empty_transcript_is_absent_and_stays_distinguishable_from_a_stale_matcher() {
        assert_eq!(fanout(EMPTY), Observed::Absent);
        assert_eq!(
            fanout("{\"message\":{\"content\":\"just prose\"}}"),
            Observed::Absent
        );
        assert_ne!(fanout(EMPTY), fanout(FANOUT_UNRECOGNISED));
    }

    // --- fan-out, per Attempt, as two integers -------------------------------------------------

    /// A parent transcript spawning `spawns` and returning the first `returns` of them.
    fn spawning(spawns: usize, returns: usize) -> String {
        let mut lines = Vec::new();
        for n in 0..spawns {
            lines.push(format!(
                r#"{{"message":{{"role":"assistant","content":[{{"type":"tool_use","id":"toolu_{n}","name":"Agent","input":{{"description":"do the thing {n}"}}}}]}}}}"#
            ));
        }
        for n in 0..returns {
            lines.push(format!(
                r#"{{"message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_{n}","content":"done"}}]}}}}"#
            ));
        }
        lines.join("\n")
    }

    #[test]
    fn each_attempt_of_one_run_counts_only_the_lines_it_appended() {
        // One Run's transcript is one append-only file: the session id is fixed at dispatch and
        // every later Attempt resumes that session. Counting the whole file on Attempt N counted
        // Attempts 1..N — and `render` sums the per-Attempt pairs, so a Run that fanned out to
        // two agents on each of three attempts published *12 spawned, 12 returned* (R51 says
        // per Attempt).
        let mut transcript = String::new();
        let mut recorded = Vec::new();
        for _ in 0..3 {
            let already_written = transcript.lines().count();
            if !transcript.is_empty() {
                transcript.push('\n');
            }
            // Distinct ids per Attempt, because the real transcript never reuses one.
            transcript
                .push_str(&spawning(2, 2).replace("toolu_", &format!("toolu_{}_", recorded.len())));
            recorded.push(fanout_since(&transcript, already_written));
        }
        assert_eq!(
            recorded,
            vec![
                Observed::Present((2, 2)),
                Observed::Present((2, 2)),
                Observed::Present((2, 2))
            ],
            "the whole-file count would read (2,2), (4,4), (6,6) and sum to 12"
        );
        // And the suffix of an Attempt that appended nothing is absent, never a stale repeat of
        // the Attempt before it.
        let all = transcript.lines().count();
        assert_eq!(fanout_since(&transcript, all), Observed::Absent);
        // A zero offset is the whole file, which is what the Run's first Attempt reads.
        assert_eq!(fanout_since(&transcript, 0), Observed::Present((6, 6)));
    }

    #[test]
    fn an_attempt_that_spawned_three_and_saw_three_return_records_both_integers() {
        assert_eq!(fanout_counts(&spawning(3, 3)), Observed::Present((3, 3)));
    }

    #[test]
    fn an_attempt_that_spawned_three_and_saw_two_return_says_so_and_nothing_else() {
        // A count of processes must never become an assertion about a review.
        let found = fanout_counts(&spawning(3, 2));
        assert_eq!(found, Observed::Present((3, 2)));
        let rendered = format!("{found:?}").to_lowercase();
        for banned in ["health", "degraded", "ok", "complete", "true", "false"] {
            assert!(!rendered.contains(banned), "{rendered}");
        }
    }

    #[test]
    fn an_attempt_with_no_transcript_records_could_not_observe_rather_than_zero_zero() {
        // `(0, 0)` claims the Run fanned out to nobody. Nothing was read.
        let unread: Observed<(u64, u64)> =
            Observed::Unobservable(Reason::saying("the transcript could not be read"));
        assert_ne!(unread, Observed::Present((0, 0)));
        // And a transcript full of tool calls naming something else is the same kind of silence.
        assert!(matches!(
            fanout_counts(FANOUT_UNRECOGNISED),
            Observed::Unobservable(_)
        ));
        assert_eq!(fanout_counts(EMPTY), Observed::Absent);
    }

    #[test]
    fn the_record_round_trips_both_integers_through_the_reader() {
        // One shared `Attempt` type, so there is no reader mirror to drift — and the
        // `deny_unknown_fields` parity test binds only the record's top-level fields, so a
        // duplicate attempt type inside `view` would not be caught by it.
        let found: RunView = serde_json::from_str(DAY_ONE).expect("the day-one record");
        assert_eq!(found.attempts[0].fanout, Observed::Present((3, 3)));
        assert!(matches!(
            found.attempts[1].fanout,
            Observed::Unobservable(_)
        ));
        assert_eq!(found.attempts[2].fanout, Observed::Absent);
        let written = serde_json::to_string(&found.attempts[0]).expect("serialise");
        let read: crate::attempt::Attempt = serde_json::from_str(&written).expect("deserialise");
        assert_eq!(read.fanout, found.attempts[0].fanout);
    }

    #[test]
    fn the_live_stage_carries_the_same_nothing_recognised_rule() {
        // Not currently broken. The rule is what keeps it from breaking silently the way the
        // fan-out matcher did.
        let found = now_skill(FANOUT_UNRECOGNISED);
        assert!(matches!(found, Observed::Unobservable(_)), "{found:?}");
        assert_eq!(
            now_skill(FANOUT_AGENT),
            Observed::Present("compound-engineering:ce-code-review".to_string())
        );
    }

    #[test]
    fn the_last_words_block_is_always_exactly_three_lines() {
        let one = "{\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"only line\"}]}}";
        assert_eq!(last_words(one, 3).len(), 3);
        assert_eq!(last_words("", 3).len(), 3);
        let ten: String = (0..10)
            .map(|n| format!("{{\"message\":{{\"content\":\"line {n}\"}}}}\n"))
            .collect();
        let block = last_words(&ten, 3);
        assert_eq!(block.len(), 3);
        assert_eq!(block, vec!["line 7", "line 8", "line 9"]);
    }

    #[test]
    fn the_last_assistant_message_is_the_one_shown_and_user_lines_never_win() {
        // `assistant_now` answers *what is it doing right now*, which is what Claude last
        // said — not what was last said to it.
        let transcript = concat!(
            "{\"type\":\"user\",\"message\":{\"content\":\"the operator asked\"}}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"first answer\"}]}}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"second answer\"}]}}\n",
        );
        assert_eq!(
            assistant_now(transcript),
            Observed::Present("second answer".to_string())
        );
    }

    #[test]
    fn an_assistant_message_carrying_only_the_inner_role_spelling_is_still_one() {
        // The real file carries the role inconsistently between its own lines: some lines
        // name it only as `message.role`. A matcher over the top-level `type` alone is the
        // next silent-stale one, so both spellings count (#82).
        let transcript = concat!(
            "{\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"inner role\"}]}}\n",
            "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"top level\"}]}}\n",
        );
        assert_eq!(
            assistant_now(transcript),
            Observed::Present("top level".to_string())
        );
        let only_inner = "{\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"inner role\"}]}}\n";
        assert_eq!(
            assistant_now(only_inner),
            Observed::Present("inner role".to_string())
        );
    }

    #[test]
    fn an_assistant_line_that_never_parses_follows_the_nothing_recognised_rule() {
        // Tool calls in the transcript but no readable assistant line: could-not-observe,
        // never absent — the same stale-matcher distinction the fan-out matcher carries.
        let damaged = concat!(
            "{\"type\":\"assistant\",\"message\":{ oops\n",
            "{\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Bash\"}]}}\n",
        );
        let found = assistant_now(damaged);
        assert!(matches!(found, Observed::Unobservable(_)), "{found:?}");
        // Nothing in the transcript at all: absent, and distinguishable from a stale matcher.
        assert_eq!(assistant_now(EMPTY), Observed::Absent);
    }

    #[test]
    fn a_transcript_that_cannot_be_read_at_all_costs_five_values_and_not_the_command() {
        let nowhere = Path::new("/nowhere/that/exists/none.jsonl");
        let found = live(nowhere, 1_785_000_000);
        assert!(matches!(found.now_skill, Observed::Unobservable(_)));
        assert!(matches!(found.fanout, Observed::Unobservable(_)));
        assert!(matches!(found.freshness, Observed::Unobservable(_)));
        assert!(matches!(found.assistant_now, Observed::Unobservable(_)));
        assert_eq!(found.last_words.len(), 3);
        assert_eq!(found.transcript, nowhere);
    }

    #[test]
    fn a_transcript_path_is_derived_from_the_worktree_and_the_session() {
        let path = transcript_path(
            Path::new("/home/op"),
            "/home/op/Repos/mine/snapper",
            "d51b4c39-ce1d-449b-8366-04b9b1aa6573",
        );
        assert!(path.starts_with("/home/op/.claude/projects"));
        assert!(
            path.to_string_lossy()
                .ends_with("d51b4c39-ce1d-449b-8366-04b9b1aa6573.jsonl")
        );
    }

    #[test]
    fn an_unknown_run_id_is_not_here_rather_than_an_error() {
        let home = Path::new("/nowhere/that/exists");
        assert!(matches!(
            load(home, "20260806-122620-snapper-28"),
            Lookup::NotHere
        ));
        assert!(roster(home).is_empty());
    }

    #[test]
    fn a_roster_row_carries_the_job_the_spend_and_the_newest_recorded_moment() {
        let home = crate::world::temp_dir("view-roster");
        let run_dir = job::runs_dir(&home).join("20260806-122620-snapper-28");
        crate::world::create_dir_all(&run_dir).unwrap();
        crate::world::write_atomic(&run_dir.join("run.json"), DAY_ONE).unwrap();
        // A neighbouring directory whose record does not parse costs its own row and
        // never the board.
        let junk = job::runs_dir(&home).join("20260807-000000-junk-01");
        crate::world::create_dir_all(&junk).unwrap();
        crate::world::write_atomic(&junk.join("run.json"), "{not a record").unwrap();

        let rows = roster(&home);
        assert_eq!(rows.len(), 1, "{rows:?}");
        let row = &rows[0];
        assert_eq!(
            row.job_title,
            "Slice 1b: the agent surface and the ScreenSource seam"
        );
        assert!((row.spend - 26.69).abs() < 0.001, "{}", row.spend);
        // Attempt 4 ended after the record was created, and the recorded string is what
        // the row names — nothing synthesized.
        assert_eq!(row.last_activity, "2026-08-06T17:41:55+00:00");
        crate::world::remove_tree(&home);
    }

    // --- Grit surfaces, read fresh from a Run's own directory ---------------------------------

    fn decision_json() -> String {
        serde_json::json!({
            "tier": "t1",
            "personas": ["correctness", "tests"],
            "depth": {"reviewers": 1},
            "model_per_stage": {"work": "claude-sonnet-5"},
            "floor_from_plan": "t0",
            "rationale": [
                {"signal": "loc_changed", "value": "180", "weight": "t1"}
            ]
        })
        .to_string()
    }

    #[test]
    fn a_decision_is_read_fresh_and_absence_is_not_an_error() {
        let run_dir = crate::world::temp_dir("view-decision");
        crate::world::create_dir_all(&run_dir.join("stages").join("triage")).unwrap();
        crate::world::write_atomic(
            &run_dir.join("stages").join("triage").join("decision.json"),
            &decision_json(),
        )
        .unwrap();

        let found = decision_of(&run_dir, "triage").expect("the written decision");
        assert_eq!(found.tier, decide::Tier::T1);
        assert_eq!(found.rationale.len(), 1);

        // A stage that never ran, and a Run that never reached the ladder at all: both absent,
        // never an error.
        assert!(decision_of(&run_dir, "diff-triage").is_none());
        assert!(decision_of(Path::new("/nowhere/at/all"), "triage").is_none());
        crate::world::remove_tree(&run_dir);
    }

    #[test]
    fn a_malformed_decision_file_reads_as_absent_rather_than_a_crash() {
        let run_dir = crate::world::temp_dir("view-decision-bad");
        crate::world::create_dir_all(&run_dir.join("stages").join("triage")).unwrap();
        crate::world::write_atomic(
            &run_dir.join("stages").join("triage").join("decision.json"),
            "{not json",
        )
        .unwrap();
        assert!(decision_of(&run_dir, "triage").is_none());
        crate::world::remove_tree(&run_dir);
    }

    #[test]
    fn an_outcome_is_read_fresh_beside_the_record_and_absence_is_not_an_error() {
        let run_dir = crate::world::temp_dir("view-outcome");
        crate::world::create_dir_all(&run_dir).unwrap();
        let outcome_json = serde_json::json!({
            "collected_at": "2026-09-01T00:00:00+00:00",
            "pr_state": "MERGED",
            "pr_merged": true,
            "pr_merged_at": "2026-08-20T00:00:00+00:00",
            "pr_closed_at": null,
            "reverted_by": [],
            "followup_issues": []
        })
        .to_string();
        crate::world::write_atomic(&run_dir.join("outcome.json"), &outcome_json).unwrap();

        let found = outcome_of(&run_dir).expect("the written outcome");
        assert!(found.pr_merged);
        assert_eq!(found.pr_state, "MERGED");

        assert!(outcome_of(Path::new("/nowhere/at/all")).is_none());
        crate::world::remove_tree(&run_dir);
    }

    #[test]
    fn a_calibration_row_is_read_fresh_and_absence_is_not_an_error() {
        let run_dir = crate::world::temp_dir("view-calibration");
        crate::world::create_dir_all(&run_dir.join("stages").join("reflect")).unwrap();
        crate::world::write_atomic(
            &run_dir
                .join("stages")
                .join("reflect")
                .join("calibration.json"),
            &serde_json::json!({"tier": "t1", "confirmed": "P1"}).to_string(),
        )
        .unwrap();

        let found = calibration_of(&run_dir).expect("the written calibration row");
        assert_eq!(found["tier"], "t1");
        assert!(calibration_of(Path::new("/nowhere/at/all")).is_none());
        crate::world::remove_tree(&run_dir);
    }

    #[test]
    fn proposals_are_scanned_fresh_from_reflects_jobs_and_diffs_directories() {
        let run_dir = crate::world::temp_dir("view-proposals");
        let reflect = run_dir.join("stages").join("reflect");
        crate::world::create_dir_all(&reflect.join("jobs")).unwrap();
        crate::world::create_dir_all(&reflect.join("diffs")).unwrap();
        crate::world::write_atomic(
            &reflect.join("jobs").join("follow-up.md"),
            "Fix the residual finding in observe.rs\nmore body\n",
        )
        .unwrap();
        crate::world::write_atomic(
            &reflect.join("diffs").join("wording.diff"),
            "--- a/skills/run/review/SKILL.md\n+++ b/skills/run/review/SKILL.md\n",
        )
        .unwrap();

        let mut found = proposals_in(&run_dir);
        found.sort_by(|a, b| a.kind.cmp(b.kind));
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].kind, "diff");
        assert!(found[0].summary.contains("--- a/skills"), "{found:?}");
        assert_eq!(found[1].kind, "job");
        assert!(found[1].summary.contains("Fix the residual finding"));

        // A Run that never reflected has no proposal artifacts, never an error.
        assert!(proposals_in(Path::new("/nowhere/at/all")).is_empty());
        crate::world::remove_tree(&run_dir);
    }

    #[test]
    fn the_proposal_queue_names_the_run_each_entry_came_from() {
        let home = crate::world::temp_dir("view-proposal-queue");
        let run_dir = job::runs_dir(&home).join("20260901-000000-snapper-40");
        crate::world::create_dir_all(&run_dir.join("stages").join("reflect").join("jobs")).unwrap();
        crate::world::write_atomic(
            &run_dir
                .join("stages")
                .join("reflect")
                .join("jobs")
                .join("a.md"),
            "drafted Job body\n",
        )
        .unwrap();

        let found = proposal_queue(&home);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, "20260901-000000-snapper-40");
        assert_eq!(found[0].1.kind, "job");
        crate::world::remove_tree(&home);
    }
}
