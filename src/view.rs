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

use crate::attempt::Attempt;
use crate::job::{self, Job};
use crate::observe::{self, Observation, Observed, Reason};
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
    /// *attempt N of M*, with **M from the record**. Re-entering under a different environment
    /// cannot make this misreport a Run's own budget.
    pub fn attempt_counter(&self) -> (usize, usize) {
        (self.attempts.len(), self.attempt_budget)
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
                    world::process_start_stamp(found.supervisor_pid).as_deref(),
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
pub fn supervisor_here(recorded: Option<&str>, live: Option<&str>) -> Observed<bool> {
    match (recorded, live) {
        // Nothing is running under that pid at all.
        (_, None) => Observed::Present(false),
        // The pid is alive but nothing was recorded to compare it against, so this is a
        // question Grind cannot answer rather than a yes.
        (None, Some(_)) => Observed::Unobservable(Reason::saying(
            "no supervisor identity was recorded at dispatch",
        )),
        (Some(was), Some(now)) => Observed::Present(was.trim() == now.trim()),
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
            Some(body) => Observed::Present(fanout(body)),
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
    let mut lines = 0usize;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        lines += 1;
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(skill) = value.get("attributionSkill").and_then(|s| s.as_str())
            && !skill.is_empty()
        {
            last = Some(skill.to_string());
        }
    }
    match (last, lines) {
        (Some(skill), _) => Observed::Present(skill),
        (None, 0) => Observed::Absent,
        (None, _) => Observed::Absent,
    }
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
pub fn fanout(text: &str) -> Vec<Fanout> {
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
            if part.get("name").and_then(|n| n.as_str()) == Some("Task")
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
    found
}

/// Observe a Run's durable artifacts fresh. **Reads and never writes** — this path observes and
/// persists nothing, which is the whole difference from the script's `cmd_status`.
pub fn observe_fresh(worktree: &Path, handoff_sha: &str, at: String) -> Observation {
    let readable = world::is_dir(worktree);
    let mut run = |argv: &[String]| world::run(argv, Some(worktree));
    let mut list = |relative: &str| {
        world::list_with_extension(&worktree.join(relative), "md")
            .into_iter()
            .map(|p| p.strip_prefix(worktree).unwrap_or(&p).display().to_string())
            .collect::<Vec<String>>()
    };
    observe::observe_run(at, handoff_sha, readable, &mut run, &mut list)
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

    #[test]
    fn the_read_only_reader_parses_the_same_fixture_the_writer_does() {
        let found: RunView = serde_json::from_str(DAY_ONE).expect("the base's record shape");
        assert_eq!(found.run_id, "20260806-122620-snapper-28");
        assert_eq!(found.attempt_counter(), (4, 8));
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
        assert_eq!(found.attempt_counter(), (4, 3));
    }

    #[test]
    fn a_dead_supervisor_reads_as_gone_however_it_died() {
        // Nothing is running under that pid.
        assert_eq!(
            supervisor_here(Some("Thu Aug  6 12:26:20 2026"), None),
            Observed::Present(false)
        );
        // The pid exists but belongs to something else — a reused pid must not report a dead
        // Run as alive.
        assert_eq!(
            supervisor_here(
                Some("Thu Aug  6 12:26:20 2026"),
                Some("Thu Aug  6 18:02:11 2026")
            ),
            Observed::Present(false)
        );
        // Alive and the same process.
        assert_eq!(
            supervisor_here(
                Some("Thu Aug  6 12:26:20 2026"),
                Some("Thu Aug  6 12:26:20 2026")
            ),
            Observed::Present(true)
        );
    }

    #[test]
    fn an_unrecorded_identity_is_a_question_grind_cannot_answer() {
        let found = supervisor_here(None, Some("Thu Aug  6 12:26:20 2026"));
        assert!(matches!(found, Observed::Unobservable(_)), "{found:?}");
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
    fn fan_out_reads_as_a_count_with_descriptions() {
        let found = fanout(FANOUT_PARENT);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].description, "review the diff for regressions");
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
