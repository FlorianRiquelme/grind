//! One blocked record, three surfaces.
//!
//! A Blocker is a fact about the world in the same family as a rate limit (ADR-0003), and the
//! human is the thing being waited on — so the projection they happen to be looking at decides
//! whether they learn that. `grind status <run-id>` used to be the projection that never learned
//! it: `cli::status_one` hand-built a second fact set with no `blocker` field and never reached
//! `view::gather` (#193, the third instance of the drift class ledger entry
//! `2026-08-26-grind-158-renderers-derive-from-the-record.md` names).
//!
//! This spans the three renderers because no single module's tests can: the Handback and the
//! terminal view live in `render`, the dashboard in `page`. Reverting any one of the three back
//! to its own composition turns this red.

use grind::decide::{Stage, Verdict, VerifyContract};
use grind::observe::{Observation, Observed};
use grind::view::{Facts, Live, RunView};
use std::path::PathBuf;

const RUN_ID: &str = "20260806-122620-snapper-28";

/// The two-step repair spelled out here rather than read back from `render::repair_hint`, so a
/// surface that silently stopped composing through it is caught rather than mirrored. The
/// dashboard escapes for HTML, which is the one difference between the surfaces this test
/// accepts — a compression of wording it would not.
fn repair_route(surface: Surface) -> String {
    match surface {
        Surface::Terminal => {
            format!("`grind cleared {RUN_ID} \"<what changed>\"`, then `grind resume {RUN_ID}`")
        }
        Surface::Html => format!(
            "`grind cleared {RUN_ID} &quot;&lt;what changed&gt;&quot;`, then `grind resume {RUN_ID}`"
        ),
    }
}

#[derive(Clone, Copy)]
enum Surface {
    Terminal,
    Html,
}

const WHAT_MUST_BE_CLEARED: &str = "Bash(gh pr merge 41)";

fn blocked() -> Facts {
    let mut found: RunView =
        serde_json::from_str(include_str!("fixtures/record/day-one.json")).expect("the fixture");
    found.run_id = RUN_ID.to_string();
    found.state = "blocked".to_string();
    Facts {
        found,
        observation: Observation {
            observed_at: "2026-08-06T12:00:00+00:00".to_string(),
            commits_ahead: Observed::Present(3),
            tree_clean: Observed::Present(true),
            pr: Observed::Absent,
            checks_pending: Observed::Absent,
            checks_red: Observed::Absent,
            plan_files: Observed::Absent,
            residual_findings: Observed::Absent,
            ledger_entries: Observed::Absent,
            changed_files: Observed::Absent,
            base_drift: Observed::Absent,
            pr_head_matches_job_branch: Observed::Absent,
            pr_base_matches_declared: Observed::Absent,
        },
        verdict: Verdict::Incomplete(vec!["PR open".to_string()]),
        contract: VerifyContract {
            present: Vec::new(),
            missing: Vec::new(),
        },
        coverage: Observed::Absent,
        furthest: Stage::Implemented,
        blocker: Some(WHAT_MUST_BE_CLEARED.to_string()),
        cleared: None,
        run_state: PathBuf::from("/home/op/.grind/runs/20260806-122620-snapper-28/run.json"),
        triage_decision: None,
        diff_triage_decision: None,
        outcome: None,
        calibration: None,
    }
}

fn live() -> Live {
    Live {
        transcript: PathBuf::from(
            "/home/op/.grind/runs/20260806-122620-snapper-28/messages-1.jsonl",
        ),
        now_skill: Observed::Absent,
        last_words: vec![String::new(), String::new(), String::new()],
        assistant_now: Observed::Absent,
        fanout: Observed::Absent,
        freshness: Observed::Absent,
    }
}

#[test]
fn every_surface_of_a_blocked_record_names_the_blocker_and_the_route_to_clearing_it() {
    let facts = blocked();
    let here = Observed::Present(false);
    for (name, text, surface) in surfaces(&facts, &here) {
        assert!(
            text.contains(WHAT_MUST_BE_CLEARED),
            "{name} withheld what must be cleared:\n{text}"
        );
        assert!(
            text.contains(&repair_route(surface)),
            "{name} named no route to clearing it:\n{text}"
        );
    }
}

fn surfaces(facts: &Facts, here: &Observed<bool>) -> [(&'static str, String, Surface); 3] {
    [
        (
            "handback",
            grind::render::handback(facts),
            Surface::Terminal,
        ),
        (
            "grind status",
            grind::render::run_view(facts, &live(), here),
            Surface::Terminal,
        ),
        (
            "dashboard",
            grind::page::run_page(RUN_ID, facts, &live(), here),
            Surface::Html,
        ),
    ]
}

/// The Blocker is read off `Facts` and nothing else, so a record that is not blocked carries
/// none — the surfaces stay silent rather than inventing a hold (ADR-0003: Grind never gates).
#[test]
fn a_record_with_no_blocker_says_nothing_about_one_on_any_surface() {
    let mut facts = blocked();
    facts.found.state = "dispatched".to_string();
    facts.blocker = None;
    let here = Observed::Present(true);
    for (name, text, surface) in surfaces(&facts, &here) {
        assert!(
            !text.contains(&repair_route(surface)),
            "{name} offered a repair for a Run that is not blocked:\n{text}"
        );
        assert!(
            !text.contains(WHAT_MUST_BE_CLEARED),
            "{name} named a Blocker on a Run that carries none:\n{text}"
        );
    }
}
