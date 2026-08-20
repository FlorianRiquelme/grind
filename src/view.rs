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

use crate::attempt::{self, Attempt};
use crate::decide::{self, Stage, Verdict, VerifyContract, VerifyCoverage};
use crate::job::{self, Job};
use crate::observe::{self, Observation, Observed, Reason};
use crate::policy;
use crate::world;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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
    pub supervisor_pid: u32,
    pub supervisor_identity: Option<String>,
    pub attempts: Vec<Attempt>,
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
    pub run_state: PathBuf,
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
    Some(Facts {
        run_state: record_path(home, run_id),
        found,
        observation,
        verdict,
        contract,
        coverage,
        furthest,
        blocker,
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
            });
        }
    }
    rows
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

// --- the live view, read from an undocumented format ----------------------------------------

/// One fanned-out subagent, as the transcript shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Fanout {
    pub description: String,
}

/// What the transcript can say. Four values, each degrading on its own — an unreadable
/// transcript costs these their values and never the whole command.
#[derive(Debug)]
pub struct Live {
    pub transcript: PathBuf,
    pub now_skill: Observed<String>,
    pub last_words: Vec<String>,
    pub fanout: Observed<Vec<Fanout>>,
    /// Seconds since the newest write across the parent transcript **and every subagent
    /// transcript**. The quietest healthy phase of a pipeline must not read as stuck.
    pub freshness: Observed<u64>,
}

/// Claude Code writes a session's transcript under a slug of the directory it ran in.
pub fn transcript_path(home: &Path, worktree: &str, session_id: &str) -> PathBuf {
    home.join(".claude")
        .join("projects")
        .join(project_slug(worktree))
        .join(format!("{session_id}.jsonl"))
}

fn project_slug(worktree: &str) -> String {
    worktree
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

pub fn live(transcript: &Path, now_epoch: u64) -> Live {
    let text = world::read_to_string(transcript).ok();
    let newest = newest_write(transcript);
    Live {
        transcript: transcript.to_path_buf(),
        now_skill: match &text {
            Some(body) => now_skill(body),
            None => Observed::Unobservable(Reason::saying("the transcript could not be read")),
        },
        last_words: match &text {
            Some(body) => last_words(body, 3),
            // Still exactly three lines: the block's height is fixed so `watch` never jitters,
            // and an unreadable transcript must not change the shape of the view.
            None => vec![String::new(); 3],
        },
        fanout: match &text {
            Some(body) => fanout(body),
            None => Observed::Unobservable(Reason::saying("the transcript could not be read")),
        },
        freshness: match newest {
            Some(at) => Observed::Present(seconds_since(at, now_epoch)),
            None => {
                Observed::Unobservable(Reason::saying("no transcript write to read a time from"))
            }
        },
    }
}

/// The newest write across the parent transcript and `<uuid>/subagents/*.jsonl`.
///
/// A fan-out makes the **parent** go quiet while subagents work, so a parent-only mtime
/// misreads a healthy fan-out as a stall and sends the operator to kill a working Run.
pub fn newest_write(transcript: &Path) -> Option<SystemTime> {
    let mut newest = world::mtime(transcript);
    let Some(stem) = transcript.file_stem() else {
        return newest;
    };
    let subagents = transcript
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(stem)
        .join("subagents");
    for child in world::list_with_extension(&subagents, "jsonl") {
        if let Some(at) = world::mtime(&child) {
            newest = Some(match newest {
                Some(current) if current > at => current,
                _ => at,
            });
        }
    }
    newest
}

fn seconds_since(at: SystemTime, now_epoch: u64) -> u64 {
    let then = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now_epoch.saturating_sub(then)
}

/// Tolerant `serde_json::Value` lookups, line by line.
///
/// A derive against an undocumented format has to track it forever, and optional-with-default
/// still loses **every sibling field on a line** when one field's type is unexpected. The same
/// real file changes field names and field types between its own lines, so an unreadable line
/// costs its own values and nothing else.
pub fn now_skill(text: &str) -> Observed<String> {
    let mut last: Option<String> = None;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(skill) = value.get("attributionSkill").and_then(|s| s.as_str())
            && !skill.is_empty()
        {
            last = Some(skill.to_string());
        }
    }
    match last {
        Some(skill) => Observed::Present(skill),
        // The same *nothing recognised* rule the fan-out matcher carries. This field is not
        // currently broken; the rule is what keeps it from breaking silently the way the
        // fan-out one did.
        None => nothing_recognised(text, "attributionSkill"),
    }
}

/// The tool a fan-out spawn names. The CLI calls it `Agent`; `Task` is the former spelling, and
/// matching only that one printed `none` on every Run that fanned out — **203 spawns to 0**
/// across sixty transcripts. The fixture that should have caught it is authored, so it asserted
/// the matcher against itself and caught nothing.
pub const FANOUT_TOOLS: [&str; 2] = ["Agent", "Task"];

/// Every tool-use block in a transcript, whatever it named.
///
/// This is what separates *nothing recognised* from *nothing there*. A transcript full of tool
/// calls and no recognised spawn is a matcher that has gone stale, and reading it as `Absent`
/// is indistinguishable from a Run that genuinely fanned out to nobody.
pub fn tool_calls(text: &str) -> usize {
    let mut calls = 0usize;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(serde_json::Value::Array(parts)) =
            value.get("message").and_then(|m| m.get("content"))
        else {
            continue;
        };
        calls += parts
            .iter()
            .filter(|part| part.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
            .count();
    }
    calls
}

/// *Could not observe*, with the tool-call count in the reason — or `Absent` where there was
/// nothing in the transcript to recognise in the first place.
fn nothing_recognised<T>(text: &str, what: &str) -> Observed<T> {
    let calls = tool_calls(text);
    if calls == 0 {
        return Observed::Absent;
    }
    Observed::Unobservable(Reason::saying(&format!(
        "{calls} tool call{} in the transcript and no recognised `{what}`",
        if calls == 1 { "" } else { "s" }
    )))
}

/// The last-words block, fixed at exactly `wanted` lines so `watch -n 30` never jitters.
pub fn last_words(text: &str, wanted: usize) -> Vec<String> {
    let mut said: Vec<String> = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(text) = first_text(&value) {
            said.push(one_line(&text));
        }
    }
    let start = said.len().saturating_sub(wanted);
    let mut block: Vec<String> = said[start..].to_vec();
    while block.len() < wanted {
        block.push(String::new());
    }
    block
}

fn first_text(value: &serde_json::Value) -> Option<String> {
    let content = value.get("message")?.get("content")?;
    match content {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(parts) => parts
            .iter()
            .find_map(|p| p.get("text").and_then(|t| t.as_str()))
            .map(str::to_string),
        _ => None,
    }
}

fn one_line(text: &str) -> String {
    let flattened: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flattened.chars().count() > 100 {
        flattened.chars().take(99).collect::<String>() + "…"
    } else {
        flattened
    }
}

/// Fan-out as a count with descriptions — *blocked on five agents, newest wrote forty seconds
/// ago* has to be an available answer.
///
/// Both spellings are recognised (`FANOUT_TOOLS`), and a transcript carrying tool-use blocks
/// with **zero** recognised spawns reads *could not observe* rather than `Absent`. Widening the
/// matcher alone would leave the next rename exactly as silent as this one was.
pub fn fanout(text: &str) -> Observed<Vec<Fanout>> {
    let mut found = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(description) = value.get("description").and_then(|d| d.as_str()) {
            found.push(Fanout {
                description: description.to_string(),
            });
            continue;
        }
        let Some(serde_json::Value::Array(parts)) =
            value.get("message").and_then(|m| m.get("content"))
        else {
            continue;
        };
        for part in parts {
            let named = part
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or_default();
            if FANOUT_TOOLS.contains(&named)
                && let Some(description) = part
                    .get("input")
                    .and_then(|i| i.get("description"))
                    .and_then(|d| d.as_str())
            {
                found.push(Fanout {
                    description: description.to_string(),
                });
            }
        }
    }
    if found.is_empty() {
        return nothing_recognised(text, "fan-out spawn");
    }
    Observed::Present(found)
}

/// The fan-out in **the lines appended since** `already_written`, which is what *per Attempt*
/// means here (R51).
///
/// A Run's transcript is one append-only file for the Run's whole life: the session id is fixed
/// at dispatch and every later Attempt resumes that session. So counting the whole file on
/// Attempt N counts Attempts 1..N, and since `render` sums the per-Attempt pairs, a Run fanning
/// out to 2 agents on each of 3 attempts reported 12 spawned. The suffix is the fix, and it is a
/// suffix by line because the transcript is line-delimited JSON.
pub fn fanout_since(text: &str, already_written: usize) -> Observed<(u64, u64)> {
    fanout_counts(
        &text
            .lines()
            .skip(already_written)
            .collect::<Vec<&str>>()
            .join("\n"),
    )
}

/// **Spawned and returned, both read from the parent transcript** (KTD8). Spawns are the
/// tool-use blocks naming the fan-out tool; returns are the `tool_result` blocks that pair to
/// them by id. The subagent files on disk are the third source and are deliberately unused:
/// they have zero observed disagreements with these counts, so they add reading and no
/// information.
///
/// **No summary, boolean or health word sits over the two integers.** A count of processes must
/// never become an assertion about a review, and whether a returned subagent errored is
/// unproven across 203 observations and is not modelled.
///
/// The `tool_use` → `tool_result` pairing is **assumed**, not verified. Where a spawn carries no
/// id it cannot be paired, so it counts as spawned and never as returned — which reads low
/// rather than high, the safe direction for a number nobody should fold into a verdict.
pub fn fanout_counts(text: &str) -> Observed<(u64, u64)> {
    let mut spawned: Vec<String> = Vec::new();
    let mut unidentified = 0u64;
    let mut returned = 0u64;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(serde_json::Value::Array(parts)) =
            value.get("message").and_then(|m| m.get("content"))
        else {
            continue;
        };
        for part in parts {
            let named = part
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or_default();
            if FANOUT_TOOLS.contains(&named) {
                match part.get("id").and_then(|i| i.as_str()) {
                    Some(id) => spawned.push(id.to_string()),
                    None => unidentified += 1,
                }
                continue;
            }
            if part.get("type").and_then(|t| t.as_str()) == Some("tool_result")
                && let Some(paired) = part.get("tool_use_id").and_then(|i| i.as_str())
                && spawned.iter().any(|id| id == paired)
            {
                returned += 1;
            }
        }
    }
    let total = spawned.len() as u64 + unidentified;
    if total == 0 {
        return nothing_recognised(text, "fan-out spawn");
    }
    Observed::Present((total, returned))
}

/// Observe a Run's durable artifacts fresh. **Reads and never writes** — this path observes and
/// persists nothing, which is the whole difference from the script's `cmd_status`.
pub fn observe_fresh(worktree: &Path, handoff_sha: &str, at: String) -> Observation {
    let mut run = |argv: &[String]| world::run(argv, Some(worktree));
    observe::observe_run(at, handoff_sha, &mut run)
}

#[cfg(test)]
mod tests {
    use super::*;

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
                r#"{{"message":{{"role":"assistant","content":[{{"type":"tool_use","id":"toolu_{n}","name":"Agent","input":{{"description":"do the thing"}}}}]}}}}"#
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
    fn a_transcript_that_cannot_be_read_at_all_costs_four_values_and_not_the_command() {
        let nowhere = Path::new("/nowhere/that/exists/none.jsonl");
        let found = live(nowhere, 1_785_000_000);
        assert!(matches!(found.now_skill, Observed::Unobservable(_)));
        assert!(matches!(found.fanout, Observed::Unobservable(_)));
        assert!(matches!(found.freshness, Observed::Unobservable(_)));
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
}
