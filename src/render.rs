//! How a Run reads to a human. **Every function returns a `String`; `cli` prints.**
//!
//! That is what makes *status degrades, never fails* an assertion rather than an intention — a
//! rendered view is a value a test can compare in full, so line order and the fixed height of
//! the last-words block are checkable rather than hoped for.
//!
//! **Verdict language describes what happened, never quality** (ADR-0003). Check every string
//! this module emits against that rule; there is a test at the bottom that does.

use crate::decide::{Stage, Verdict, VerifyContract};
use crate::observe::{Observation, Observed, Outcome, UNOBSERVABLE_MARK};
use crate::view::{Live, RosterRow, RunView};
use std::path::Path;

/// One item of doctor's report, as `cli` hands it over: the name and the depth mark alongside
/// the classified result, so this module needs no edge to the module that owns the list.
pub struct DoctorLine<'a> {
    pub name: &'a str,
    pub mark: &'a str,
    pub outcome: Observed<Outcome>,
}

/// Everything the single-Run view is composed from. A named struct rather than eight
/// arguments, so a new line's input is `E0063` at the call site rather than a positional slot
/// somebody transposes.
pub struct SingleRun<'a> {
    pub found: &'a RunView,
    pub observation: &'a Observation,
    pub live: &'a Live,
    pub verdict: &'a Verdict,
    pub contract: &'a VerifyContract,
    pub furthest: Stage,
    pub supervisor_here: &'a Observed<bool>,
    pub run_state: &'a Path,
}

/// The single-Run view: **alive, where, stuck, and about to cost something**, top to bottom,
/// with no follow-up needed. Thirty seconds of looking is the whole budget.
///
/// The line order is fixed and the last-words block is exactly three lines, so
/// `watch -n 30 grind status <id>` never jitters and the operator's eye can park on one row.
pub fn run_view(view: &SingleRun) -> String {
    let SingleRun {
        found,
        observation,
        live,
        verdict,
        contract,
        furthest,
        supervisor_here,
        run_state,
    } = view;
    let furthest = *furthest;
    let (made, budget) = found.attempt_counter();
    let mut out = String::new();
    line(
        &mut out,
        &format!("Run     {}  [{}]", found.run_id, found.state),
    );
    line(
        &mut out,
        &format!(
            "Host    {}   supervisor {} {}",
            found.hostname,
            found.supervisor_pid,
            presence_word(supervisor_here)
        ),
    );
    line(&mut out, &format!("Job     {}", found.job.url));
    line(
        &mut out,
        &format!(
            "Branch  {}  (worktree {})",
            found.job.branch, found.worktree
        ),
    );
    line(&mut out, &format!("Session {}", found.session_id));
    line(&mut out, &format!("Model   {}", model_of(found)));
    line(&mut out, "");
    line(
        &mut out,
        &format!("  verdict           {}", verdict_line(verdict, observation)),
    );
    // Two separate stage lines. *How far it got* and *what it is doing* are never conflated.
    line(&mut out, &format!("  furthest stage    {furthest}"));
    line(&mut out, &format!("  now               {}", live.now_skill));
    line(
        &mut out,
        &format!("  progress          {}", freshness_line(&live.freshness)),
    );
    line(
        &mut out,
        &format!("  fan-out           {}", fanout_line(live)),
    );
    line(
        &mut out,
        &format!("  attempts          attempt {made} of {budget}"),
    );
    // The API-pricing counterfactual. Remaining quota prints not at all: the number nothing can
    // compute is not estimated.
    line(
        &mut out,
        &format!(
            "  spend             ${:.2} (API pricing)",
            found.total_spend()
        ),
    );
    line(
        &mut out,
        &format!("  commits ahead     {}", observation.commits_ahead),
    );
    line(&mut out, &format!("  PR                {}", observation.pr));
    line(
        &mut out,
        &format!("  tree clean        {}", observation.tree_clean),
    );
    line(
        &mut out,
        &format!("  checks pending    {}", observation.checks_pending),
    );
    line(
        &mut out,
        &format!("  verify contract   {}", contract_line(contract)),
    );
    line(&mut out, "");
    line(&mut out, "  last words");
    for said in live.last_words.iter().take(3) {
        line(&mut out, &format!("    {said}"));
    }
    line(&mut out, "");
    line(
        &mut out,
        &format!("  transcript        {}", live.transcript.display()),
    );
    line(
        &mut out,
        &format!("  run state         {}", run_state.display()),
    );
    out
}

/// The roster. It says which host it is speaking for, because Run state does not travel.
pub fn roster(hostname: &str, rows: &[RosterRow]) -> String {
    let mut out = String::new();
    line(&mut out, &format!("Runs on {hostname} — this host only."));
    line(&mut out, "");
    if rows.is_empty() {
        line(&mut out, "  no Runs here.");
        return out;
    }
    for row in rows {
        line(
            &mut out,
            &format!(
                "  {}  {:<14} supervisor {:<9} attempt {} of {}  {}",
                row.run_id,
                row.recorded_state,
                presence_word(&row.supervisor_here),
                row.attempts.0,
                row.attempts.1,
                row.branch
            ),
        );
        line(&mut out, &format!("      {}", row.job_url));
    }
    out
}

/// A run id this host has never held. Not an error, and not a typo — a pointer to where to
/// look instead.
pub fn not_here(run_id: &str, hostname: &str) -> String {
    format!(
        "Run `{run_id}` is not on {hostname}.\n\nRun state does not travel. The Job issue carries \
         the pointer to the host that holds it.\n"
    )
}

/// What a finished Run leaves for the human to pick up. Its shape is what the morning costs.
pub fn handback(
    found: &RunView,
    observation: &Observation,
    contract: &VerifyContract,
    furthest: Stage,
    run_state: &Path,
) -> String {
    let mut out = String::new();
    line(
        &mut out,
        &format!("Run     {}  [{}]", found.run_id, found.state),
    );
    line(&mut out, &format!("Job     {}", found.job.url));
    line(
        &mut out,
        &format!(
            "Branch  {}  (worktree {})",
            found.job.branch, found.worktree
        ),
    );
    line(&mut out, &format!("Session {}", found.session_id));
    line(&mut out, &format!("Model   {}", model_of(found)));
    line(
        &mut out,
        &format!(
            "Attempts {}   spend ${:.2} (API pricing)   tool denials {}",
            crate::attempt::working(&found.attempts),
            found.total_spend(),
            found.denial_count()
        ),
    );
    line(&mut out, "");
    line(&mut out, &format!("  furthest stage    {furthest}"));
    line(
        &mut out,
        &format!("  commits ahead     {}", observation.commits_ahead),
    );
    line(
        &mut out,
        &format!("  plan              {}", listing(&observation.plan_files)),
    );
    line(&mut out, &format!("  PR                {}", observation.pr));
    line(
        &mut out,
        &format!(
            "  review residuals  {}",
            count(&observation.residual_findings)
        ),
    );
    line(
        &mut out,
        &format!("  ledger entries    {}", count(&observation.ledger_entries)),
    );
    line(
        &mut out,
        &format!("  verify contract   {}", contract_line(contract)),
    );
    line(&mut out, "");
    line(
        &mut out,
        &format!("  run state         {}", run_state.display()),
    );
    out
}

/// Doctor's report. Items marked *step* appear as unchecked, with **no boolean beside them** —
/// every available check for them is a guess.
pub fn doctor(hostname: &str, lines: &[DoctorLine]) -> String {
    let mut out = String::new();
    line(&mut out, &format!("Host {hostname}"));
    line(&mut out, "");
    for item in lines {
        line(
            &mut out,
            &format!(
                "  {:<9} {:<40} {}",
                item.mark,
                item.name,
                item_outcome(&item.outcome)
            ),
        );
    }
    line(&mut out, "");
    line(
        &mut out,
        "  A failed item is incoherent input, not a judgement. Checking is not gating.",
    );
    out
}

/// A refusal, in the register a refused Dispatch and a failed host check share.
pub fn refusal(said: &str) -> String {
    format!("grind: {said}\n")
}

// --- the pieces ------------------------------------------------------------------------------

/// The two negatives, for the values whose `T` is a collection and so has no `Display`. Same
/// marks as the type's own, because a reader must never have to learn two vocabularies.
fn negative_mark<T>(found: &Observed<T>) -> &'static str {
    match found {
        Observed::Present(_) => "",
        Observed::Absent => crate::observe::ABSENT_MARK,
        Observed::Unobservable(_) => UNOBSERVABLE_MARK,
    }
}

fn line(out: &mut String, text: &str) {
    out.push_str(text);
    out.push('\n');
}

fn model_of(found: &RunView) -> String {
    found
        .model
        .clone()
        .unwrap_or_else(|| "(session default — unpinned)".to_string())
}

fn presence_word(here: &Observed<bool>) -> &'static str {
    match here {
        Observed::Present(true) => "present",
        Observed::Present(false) => "gone",
        Observed::Absent => "gone",
        Observed::Unobservable(_) => UNOBSERVABLE_MARK,
    }
}

/// Red CI lands **on the verdict line** rather than holding the verdict open.
fn verdict_line(verdict: &Verdict, observation: &Observation) -> String {
    let said = match verdict {
        Verdict::Completed => "completed".to_string(),
        Verdict::Uncorroborated(unmet) => {
            format!(
                "uncorroborated — DONE promised, {} disagrees",
                unmet.join(", ")
            )
        }
        Verdict::Unobserved(blind) => format!("unobserved — {}", blind.join("; ")),
        Verdict::Incomplete(unmet) => format!("incomplete — {}", unmet.join(", ")),
    };
    match observation.checks_red {
        Observed::Present(true) => format!("{said}  (a check came back red)"),
        _ => said,
    }
}

fn freshness_line(freshness: &Observed<u64>) -> String {
    match freshness {
        Observed::Present(seconds) => format!("newest write {seconds}s ago"),
        other => other.to_string(),
    }
}

fn fanout_line(live: &Live) -> String {
    match &live.fanout {
        Observed::Present(agents) if agents.is_empty() => "none".to_string(),
        Observed::Present(agents) => {
            let described: Vec<&str> = agents.iter().map(|a| a.description.as_str()).collect();
            format!(
                "{} agent{}: {}  ({})",
                agents.len(),
                if agents.len() == 1 { "" } else { "s" },
                described.join("; "),
                freshness_line(&live.freshness)
            )
        }
        other => negative_mark(other).to_string(),
    }
}

/// Presence and absence, and **never a verdict on quality**. This is the one place a gate would
/// be one line away, which is why the contract carries no summary boolean to test.
fn contract_line(contract: &VerifyContract) -> String {
    match (contract.present.is_empty(), contract.missing.is_empty()) {
        (_, true) => format!("all {} contracted steps present", contract.present.len()),
        (true, false) => format!("none present; missing: {}", contract.missing.join(", ")),
        (false, false) => format!(
            "present: {}; missing: {}",
            contract.present.join(", "),
            contract.missing.join(", ")
        ),
    }
}

fn listing(found: &Observed<Vec<String>>) -> String {
    match found {
        Observed::Present(entries) => entries.join(", "),
        other => negative_mark(other).to_string(),
    }
}

fn count(found: &Observed<Vec<String>>) -> String {
    match found {
        Observed::Present(entries) => entries.len().to_string(),
        Observed::Absent => "0".to_string(),
        other => negative_mark(other).to_string(),
    }
}

fn item_outcome(outcome: &Observed<Outcome>) -> String {
    match outcome {
        Observed::Present(Outcome::Satisfied(said)) => format!("ok        {said}"),
        Observed::Present(Outcome::Unsatisfied(said)) => format!("not met   {said}"),
        Observed::Present(Outcome::Unchecked(said)) => format!("unchecked {said}"),
        Observed::Absent => format!("{}         absent", crate::observe::ABSENT_MARK),
        Observed::Unobservable(reason) => format!("{UNOBSERVABLE_MARK}         {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::{Pr, Reason};
    use crate::view::Fanout;
    use std::path::PathBuf;

    const DAY_ONE: &str = include_str!("../tests/fixtures/record/day-one.json");

    fn found() -> RunView {
        serde_json::from_str(DAY_ONE).expect("the day-one record")
    }

    fn observation() -> Observation {
        Observation {
            observed_at: "2026-08-06T17:41:00+00:00".to_string(),
            commits_ahead: Observed::Present(12),
            tree_clean: Observed::Present(true),
            pr: Observed::Present(Pr {
                number: 30,
                url: "https://github.com/FlorianRiquelme/snapper/pull/30".to_string(),
                state: "OPEN".to_string(),
                is_draft: false,
            }),
            checks_pending: Observed::Present(false),
            checks_red: Observed::Present(false),
            plan_files: Observed::Present(vec!["docs/plans/a.md".to_string()]),
            residual_findings: Observed::Present(vec![
                "docs/residual-review-findings/a.md".to_string(),
            ]),
            ledger_entries: Observed::Absent,
        }
    }

    fn live(words: usize) -> Live {
        Live {
            transcript: PathBuf::from("/home/op/.claude/projects/x/session.jsonl"),
            now_skill: Observed::Present("compound-engineering:ce-work".to_string()),
            last_words: (0..3)
                .map(|n| {
                    if n < words {
                        format!("line {n}")
                    } else {
                        String::new()
                    }
                })
                .collect(),
            fanout: Observed::Present(vec![Fanout {
                description: "review the diff for regressions".to_string(),
            }]),
            freshness: Observed::Present(40),
        }
    }

    fn contract() -> VerifyContract {
        VerifyContract {
            present: vec!["rust-fmt".into(), "rust-clippy".into(), "rust-test".into()],
            missing: vec!["ts-lint".into(), "ts-test".into()],
        }
    }

    fn rendered(observation: &Observation, live: &Live, verdict: &Verdict) -> String {
        run_view(&SingleRun {
            found: &found(),
            observation,
            live,
            verdict,
            contract: &contract(),
            furthest: Stage::PrOpen,
            supervisor_here: &Observed::Present(true),
            run_state: Path::new("/home/op/.grind/runs/20260806-122620-snapper-28/run.json"),
        })
    }

    fn label_order(text: &str) -> Vec<String> {
        text.lines()
            .filter(|l| l.starts_with("  ") && !l.starts_with("    "))
            .map(|l| l.trim().split("  ").next().unwrap_or_default().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    }

    #[test]
    fn the_single_run_view_prints_its_lines_in_a_fixed_order_across_two_records() {
        let first = rendered(&observation(), &live(3), &Verdict::Completed);
        let mut other = observation();
        other.pr = Observed::Absent;
        other.commits_ahead = Observed::Present(0);
        let second = rendered(
            &other,
            &live(1),
            &Verdict::Incomplete(vec!["PR open".into()]),
        );
        assert_eq!(
            label_order(&first),
            label_order(&second),
            "`watch -n 30` must never jitter"
        );
    }

    #[test]
    fn the_last_words_block_is_exactly_three_lines_whatever_the_transcript_said() {
        for words in [0, 1, 3] {
            let text = rendered(&observation(), &live(words), &Verdict::Completed);
            let at = text
                .lines()
                .position(|l| l.trim() == "last words")
                .expect("the block");
            let block: Vec<&str> = text.lines().skip(at + 1).take(3).collect();
            assert_eq!(block.len(), 3, "{words} words");
            assert!(
                text.lines()
                    .nth(at + 4)
                    .is_some_and(|l| l.trim().is_empty()),
                "the block ends after exactly three lines"
            );
        }
    }

    #[test]
    fn observed_absent_renders_differently_from_could_not_observe_in_the_same_column() {
        let mut absent = observation();
        absent.pr = Observed::Absent;
        let mut blind = observation();
        blind.pr = Observed::Unobservable(Reason::saying("gh pr view: connection reset"));

        let absent_line = pr_line(&rendered(&absent, &live(3), &Verdict::Completed));
        let blind_line = pr_line(&rendered(&blind, &live(3), &Verdict::Completed));
        assert_ne!(absent_line, blind_line);
        assert!(
            absent_line.contains(crate::observe::ABSENT_MARK),
            "{absent_line}"
        );
        assert!(blind_line.contains(UNOBSERVABLE_MARK), "{blind_line}");
    }

    fn pr_line(text: &str) -> String {
        text.lines()
            .find(|l| l.trim_start().starts_with("PR "))
            .expect("a PR line")
            .to_string()
    }

    #[test]
    fn a_pr_that_could_not_be_observed_does_not_render_as_no_pr() {
        let mut blind = observation();
        blind.pr = Observed::Unobservable(Reason::saying("gh pr view: connection reset"));
        let line = pr_line(&rendered(&blind, &live(3), &Verdict::Completed));
        assert!(
            !line.contains(crate::observe::ABSENT_MARK),
            "a blind supervisor's silence must not read as a fact: {line}"
        );
    }

    #[test]
    fn the_view_prints_no_remaining_quota_figure() {
        let text = rendered(&observation(), &live(3), &Verdict::Completed);
        for banned in ["remaining", "quota", "left in", "budget left"] {
            assert!(
                !text.to_lowercase().contains(banned),
                "the number nothing can compute is not estimated: {banned}"
            );
        }
        assert!(text.contains("(API pricing)"));
    }

    #[test]
    fn the_two_stage_lines_are_separate_and_never_conflated() {
        let text = rendered(&observation(), &live(3), &Verdict::Completed);
        assert!(text.contains("furthest stage    pr-open"));
        assert!(text.contains("now               compound-engineering:ce-work"));
    }

    #[test]
    fn the_verify_contract_line_names_both_missing_steps_and_carries_no_verdict_word() {
        let text = rendered(&observation(), &live(3), &Verdict::Completed);
        let line = text
            .lines()
            .find(|l| l.contains("verify contract"))
            .expect("a contract line");
        assert!(line.contains("ts-lint"), "{line}");
        assert!(line.contains("ts-test"), "{line}");
        for banned in ["incomplete", "gutted", "bad", "fail", "should", "must"] {
            assert!(!line.to_lowercase().contains(banned), "{line}");
        }
    }

    #[test]
    fn a_full_contract_and_an_empty_one_both_read_as_presence_and_absence() {
        let all = VerifyContract {
            present: (0..7).map(|n| format!("step-{n}")).collect(),
            missing: vec![],
        };
        assert_eq!(contract_line(&all), "all 7 contracted steps present");
        let none = VerifyContract {
            present: vec![],
            missing: vec!["rust-fmt".into()],
        };
        assert!(contract_line(&none).starts_with("none present; missing:"));
    }

    #[test]
    fn the_handback_names_where_run_state_lives() {
        let text = handback(
            &found(),
            &observation(),
            &contract(),
            Stage::PrOpen,
            Path::new("/home/op/.grind/runs/20260806-122620-snapper-28/run.json"),
        );
        assert!(text.contains(
            "run state         /home/op/.grind/runs/20260806-122620-snapper-28/run.json"
        ));
        for named in [
            "Job",
            "Branch",
            "worktree",
            "Session",
            "Model",
            "Attempts",
            "tool denials",
        ] {
            assert!(text.contains(named), "the Handback must name {named}");
        }
        assert!(text.contains("furthest stage"));
        assert!(text.contains("commits ahead"));
        assert!(text.contains("plan"));
        assert!(text.contains("review residuals"));
        assert!(text.contains("ledger entries"));
    }

    #[test]
    fn attempt_n_of_m_counts_working_attempts_only_on_every_surface_that_prints_it() {
        // The day-one record holds four Attempts, of which attempt 3 cost $0 and ran one turn.
        let text = handback(
            &found(),
            &observation(),
            &contract(),
            Stage::PrOpen,
            Path::new("/x/run.json"),
        );
        assert!(text.contains("Attempts 3"), "{text}");
        let single = rendered(&observation(), &live(3), &Verdict::Completed);
        assert!(single.contains("attempt 3 of 8"), "{single}");
    }

    #[test]
    fn red_ci_lands_on_the_verdict_line_without_holding_it_open() {
        let mut red = observation();
        red.checks_red = Observed::Present(true);
        let text = rendered(&red, &live(3), &Verdict::Completed);
        let line = text
            .lines()
            .find(|l| l.contains("verdict"))
            .expect("a verdict line");
        assert!(line.contains("completed"), "{line}");
        assert!(line.contains("a check came back red"), "{line}");
    }

    #[test]
    fn a_step_item_shows_as_unchecked_with_no_boolean() {
        let report = doctor(
            "snapper.local",
            &[
                DoctorLine {
                    name: "the grind binary on PATH",
                    mark: "step",
                    outcome: crate::observe::unchecked("every available check is a guess"),
                },
                DoctorLine {
                    name: "declared clone",
                    mark: "dispatch",
                    outcome: Observed::Present(Outcome::Unsatisfied(
                        "no declared clone at ~/.grind/repos/<owner>/<name>".into(),
                    )),
                },
            ],
        );
        assert!(report.contains("unchecked"), "{report}");
        assert!(report.contains("not met"), "{report}");
        assert!(report.contains("Checking is not gating"), "{report}");
    }

    #[test]
    fn no_rendered_string_carries_a_quality_word_for_a_verdict() {
        // ADR-0003 is enforceable as a variant set and as the strings that name those variants.
        let surfaces = [
            rendered(&observation(), &live(3), &Verdict::Completed),
            rendered(
                &observation(),
                &live(3),
                &Verdict::Uncorroborated(vec!["PR open".into()]),
            ),
            rendered(
                &observation(),
                &live(3),
                &Verdict::Unobserved(vec!["PR open: connection reset".into()]),
            ),
            handback(
                &found(),
                &observation(),
                &contract(),
                Stage::Reviewed,
                Path::new("/x/run.json"),
            ),
            roster("snapper.local", &[]),
            not_here("20260806-122620-snapper-28", "snapper.local"),
        ];
        for text in surfaces {
            let said = text.to_lowercase();
            for banned in [
                "rejected",
                "blocked",
                "failed",
                "approved",
                "good",
                "bad quality",
            ] {
                assert!(!said.contains(banned), "`{banned}` in:\n{text}");
            }
        }
    }

    #[test]
    fn the_roster_says_which_host_it_speaks_for() {
        let text = roster("snapper.local", &[]);
        assert!(text.contains("snapper.local"));
        assert!(text.contains("this host only"));
        assert!(text.contains("no Runs here"));
    }
}
