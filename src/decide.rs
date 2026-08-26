//! Which signals corroborate what — the furthest stage, and the six ANDed observations that
//! decide completion.
//!
//! **A seventh completion signal cannot be added and forgotten.** `RawSignals` is a named struct,
//! so a new field is `E0063` at every constructor; the fold destructures it with no `..` and no
//! `field: _`, so a new field is `E0027` there, and the binding it forces is then unused —
//! which is an error under `cargo clippy -- -D warnings`. Two forced sites, neither needing a
//! grep.
//!
//! **The bypass is named because it has nothing behind it.** rustc's own `E0027` help text
//! offers `..` and `field: _` as fixes, and no clippy lint covers either. Taking one is a
//! deliberate act, and deliberate acts are not typeable — recorded rather than chased, because
//! a reader who believes the fold is airtight is the one who ships the collapse.

use crate::observe::{Observation, Observed};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The seven contracted verify steps, carried across from the script verbatim: they are data
/// rather than control flow, and re-typing them is where a step goes missing.
///
/// The Job's definition of done is `just verify` passing on a repo that has none of this yet.
/// The failure that costs most is **a step trimmed until it goes green**, because that is a
/// false positive on the target repo and on Grind in one shot. Grind never gates (ADR-0003), so
/// this is recorded and surfaced, never enforced.
pub const VERIFY_CONTRACT: [(&str, &[&str]); 7] = [
    ("rust-fmt", &["cargo fmt"]),
    ("rust-clippy", &["cargo clippy", "-D warnings"]),
    ("rust-test", &["cargo test"]),
    ("ts-typecheck", &["tsc", "--noEmit"]),
    ("ts-lint", &["eslint"]),
    ("ts-test", &["vitest"]),
    (
        "build-assertion",
        &["tauri build", "--debug", "--no-bundle"],
    ),
];

/// Which contracted steps a target repo declares, and which it does not.
///
/// **There is no summary boolean here, and there must never be one.** `present` and `missing`
/// carry everything the Handback needs; add `ok` and `if !vc.ok { return }` is one line away,
/// in the exact place the contract says *recorded and surfaced, never enforced* (ADR-0006).
#[derive(Debug, Clone, PartialEq)]
pub struct VerifyContract {
    pub present: Vec<String>,
    pub missing: Vec<String>,
}

/// What each contracted step **plausibly** covers, by file extension and directory prefix.
///
/// New data, not a refactor: `VERIFY_CONTRACT` carries tool-invocation substrings and knows
/// nothing about paths. A coarse heuristic, authored only for the ecosystems the contract
/// already names, and keyed to the same seven step names so a step that goes missing takes its
/// coverage with it.
///
/// **There is no source of truth behind this.** The contract knows which recipes exist, not
/// which paths they read, and a reader who treats the number below as a measurement has read
/// it wrong.
const STEP_COVERAGE: [(&str, &[&str]); 7] = [
    ("rust-fmt", &[".rs"]),
    ("rust-clippy", &[".rs"]),
    ("rust-test", &[".rs"]),
    ("ts-typecheck", &[".ts", ".tsx", ".d.ts"]),
    ("ts-lint", &[".ts", ".tsx", ".js", ".jsx"]),
    ("ts-test", &[".ts", ".tsx", ".js", ".jsx"]),
    ("build-assertion", &[".rs", ".ts", ".tsx", "src-tauri/"]),
];

/// How much of the Run's own diff sits outside **every** contracted step that is present.
///
/// **A rough, explicitly-estimated statement, and the list is the primary value.** A bare
/// number is the shape ADR-0006's sixth and seventh entries warn about; naming the paths is
/// what makes the estimate checkable rather than authoritative-looking.
///
/// It carries no boolean and gates nothing. ADR-0006 already prohibits a summary flag on the
/// verify contract, and this is the same shape one field over.
#[derive(Debug, Clone, PartialEq)]
pub struct VerifyCoverage {
    /// The changed paths no present step plausibly covers.
    pub uncovered: Vec<String>,
    /// How many paths the Run changed in all, so the proportion is the reader's to form.
    pub changed: usize,
}

impl std::fmt::Display for VerifyCoverage {
    /// Deliberately says **roughly** and **estimate**: a precise-looking number derived from a
    /// guess is worse than an obviously rough one.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.uncovered.is_empty() {
            return write!(
                f,
                "an estimated 0 of {} changed paths sit outside every contracted step",
                self.changed
            );
        }
        write!(
            f,
            "roughly {} of {} changed paths sit outside every contracted step (estimate): {}",
            self.uncovered.len(),
            self.changed,
            self.uncovered.join(", ")
        )
    }
}

/// The estimate, over the Run's own diff and the steps the target repo actually declares.
pub fn verify_coverage(
    contract: &VerifyContract,
    changed_files: &Observed<Vec<String>>,
) -> Observed<VerifyCoverage> {
    let changed = match changed_files {
        Observed::Present(files) => files,
        Observed::Absent => {
            return Observed::Present(VerifyCoverage {
                uncovered: Vec::new(),
                changed: 0,
            });
        }
        Observed::Unobservable(reason) => return Observed::Unobservable(reason.clone()),
    };
    let covering: Vec<&[&str]> = STEP_COVERAGE
        .iter()
        .filter(|(name, _)| contract.present.iter().any(|present| present == name))
        .map(|(_, patterns)| *patterns)
        .collect();
    let uncovered: Vec<String> = changed
        .iter()
        .filter(|path| {
            !covering
                .iter()
                .any(|patterns| patterns.iter().any(|pattern| covers(pattern, path)))
        })
        .cloned()
        .collect();
    Observed::Present(VerifyCoverage {
        uncovered,
        changed: changed.len(),
    })
}

/// A pattern is either an extension (`.rs`) or a directory prefix (`src-tauri/`).
fn covers(pattern: &str, path: &str) -> bool {
    if pattern.ends_with('/') {
        return path.starts_with(pattern) || path.contains(&format!("/{pattern}"));
    }
    path.ends_with(pattern)
}

/// The six observations completion is ANDed from.
///
/// A named struct rather than loose arguments so a new signal is `E0063` at every construction
/// site. The verify contract is deliberately **not** among them: a precondition must not
/// quietly become a termination condition.
#[derive(Debug, Clone)]
pub struct RawSignals {
    pub pr_open: Observed<bool>,
    pub tree_clean: Observed<bool>,
    pub commits_ahead: Observed<bool>,
    pub no_check_pending: Observed<bool>,
    /// The found PR's head ref against the Job's `branch` row (Run 2's fix).
    pub pr_head_matches_job_branch: Observed<bool>,
    /// The found PR's base ref against the Job's `base_branch` row.
    pub pr_base_matches_declared: Observed<bool>,
}

/// What happened. **Every variant describes what happened, never how good it was** — there is
/// no `Rejected`, no `Blocked` and no `Failed`, because ADR-0003's rule is enforceable as a
/// variant set and nowhere else. A completed Run means the pipeline finished, not that the code
/// is good.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Four ANDed observations agree the pipeline reached its end.
    Completed,
    /// The DONE promise was made and the artifacts do not corroborate it. Its own outcome, with
    /// **no path from the promise to `Completed`**.
    Uncorroborated(Vec<String>),
    /// At least one signal could not be observed. *I could not look* is never reported as
    /// *the Run died*.
    Unobserved(Vec<String>),
    /// Observed, and the pipeline has not reached its end yet.
    Incomplete(Vec<String>),
}

/// How far the Run got, inferred from durable artifacts on disk and on GitHub — which is also
/// what survives a death. `lfg` exposes no structured return to its caller, so this is the only
/// honest source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Dispatched,
    Planned,
    Implemented,
    Reviewed,
    PrOpen,
}

impl std::fmt::Display for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let word = match self {
            Stage::Dispatched => "dispatched",
            Stage::Planned => "planned",
            Stage::Implemented => "implemented",
            Stage::Reviewed => "reviewed",
            Stage::PrOpen => "pr-open",
        };
        write!(f, "{word}")
    }
}

/// Reduce an observation to the four booleans completion is decided from, carrying each
/// signal's three-valuedness through untouched.
pub fn signals_of(observation: &Observation) -> RawSignals {
    RawSignals {
        pr_open: match &observation.pr {
            Observed::Present(found) => Observed::Present(found.state.eq_ignore_ascii_case("OPEN")),
            Observed::Absent => Observed::Present(false),
            Observed::Unobservable(reason) => Observed::Unobservable(reason.clone()),
        },
        tree_clean: observation.tree_clean.clone(),
        commits_ahead: match &observation.commits_ahead {
            Observed::Present(count) => Observed::Present(*count > 0),
            Observed::Absent => Observed::Present(false),
            Observed::Unobservable(reason) => Observed::Unobservable(reason.clone()),
        },
        no_check_pending: match &observation.checks_pending {
            Observed::Present(pending) => Observed::Present(!pending),
            Observed::Absent => Observed::Present(true),
            Observed::Unobservable(reason) => Observed::Unobservable(reason.clone()),
        },
        pr_head_matches_job_branch: observation.pr_head_matches_job_branch.clone(),
        pr_base_matches_declared: observation.pr_base_matches_declared.clone(),
    }
}

/// Whether the deliverable itself exists: the PR open, the tree clean, and both head/base rows
/// matching. The check rollup corroborates against this — Grind hands off at an open PR
/// (ADR-0003), and waiting past that point is spend without a decision.
fn deliverable_present(
    pr_open: &Observed<bool>,
    tree_clean: &Observed<bool>,
    pr_head_matches_job_branch: &Observed<bool>,
    pr_base_matches_declared: &Observed<bool>,
) -> bool {
    matches!(pr_open, Observed::Present(true))
        && matches!(tree_clean, Observed::Present(true))
        && matches!(pr_head_matches_job_branch, Observed::Present(true))
        && matches!(pr_base_matches_declared, Observed::Present(true))
}

/// Whether a fold row may be skipped from the unmet list. Keyed by type rather than by the
/// row's rendering label, so renaming a label cannot silently disable a carve-out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Skip {
    /// The row always keeps its plain reading and can hold completion open.
    Never,
    /// The row may be skipped once the deliverable itself exists — all four conjuncts of
    /// [`deliverable_present`] hold. This is the check rollup's corroborated reading: Grind
    /// hands off at an open PR (ADR-0003), and waiting past that point is spend without a
    /// decision.
    WhenDeliverablePresent,
}

/// The fold. Completion is **observed rather than declared**, and the DONE promise is neither
/// necessary nor sufficient — two Runs finished a pipeline without emitting it, and a session
/// that believes it finished can emit it against nothing.
///
/// Boundary on the check rollup (`no_check_pending`): a pending-but-not-failed rollup
/// (`Present(false)`) corroborates once the four deliverable signals — `pr_open`, `tree_clean`,
/// `pr_head_matches_job_branch`, `pr_base_matches_declared` — are all `Present(true)`; before
/// that it keeps its plain reading and holds completion open. An absent rollup still reads
/// unmet and an unobservable one still blinds, so the three-valuedness passes through
/// untouched. Which checks exist or pass stays an observed fact; nothing here turns it into a
/// gate (ADR-0003).
pub fn verdict(signals: &RawSignals, done_promise: bool) -> Verdict {
    let RawSignals {
        pr_open,
        tree_clean,
        commits_ahead,
        no_check_pending,
        pr_head_matches_job_branch,
        pr_base_matches_declared,
    } = signals;

    // The carve-out keys off `Skip`, never off the rendering label: a row's display text is
    // free to change without touching the fold's behaviour (validate F1).
    let named: [(&str, &Observed<bool>, Skip); 6] = [
        ("PR open", pr_open, Skip::Never),
        ("tree clean", tree_clean, Skip::Never),
        ("commits ahead", commits_ahead, Skip::Never),
        (
            "no check pending",
            no_check_pending,
            Skip::WhenDeliverablePresent,
        ),
        (
            "PR head matches Job branch",
            pr_head_matches_job_branch,
            Skip::Never,
        ),
        (
            "PR base matches declared branch",
            pr_base_matches_declared,
            Skip::Never,
        ),
    ];

    let mut blind = Vec::new();
    let mut unmet = Vec::new();
    let corroborating = deliverable_present(
        pr_open,
        tree_clean,
        pr_head_matches_job_branch,
        pr_base_matches_declared,
    );
    for (name, signal, skip) in named {
        if skip == Skip::WhenDeliverablePresent
            && corroborating
            && matches!(signal, Observed::Present(false))
        {
            continue;
        }
        match signal {
            Observed::Present(true) => {}
            Observed::Present(false) => unmet.push(name.to_string()),
            Observed::Absent => unmet.push(name.to_string()),
            Observed::Unobservable(reason) => blind.push(format!("{name}: {reason}")),
        }
    }

    if !blind.is_empty() {
        return Verdict::Unobserved(blind);
    }
    if unmet.is_empty() {
        return Verdict::Completed;
    }
    if done_promise {
        return Verdict::Uncorroborated(unmet);
    }
    Verdict::Incomplete(unmet)
}

pub fn furthest_stage(observation: &Observation) -> Stage {
    let any = |signal: &Observed<Vec<String>>| matches!(signal, Observed::Present(found) if !found.is_empty());
    let mut stage = Stage::Dispatched;
    if any(&observation.plan_files) {
        stage = Stage::Planned;
    }
    if matches!(observation.commits_ahead, Observed::Present(count) if count > 0) {
        stage = Stage::Implemented;
    }
    if any(&observation.residual_findings) {
        stage = Stage::Reviewed;
    }
    if matches!(observation.pr, Observed::Present(_)) {
        stage = Stage::PrOpen;
    }
    stage
}

/// Which contracted steps the target repo declares. A justfile may legitimately delegate to npm
/// scripts, so `package.json`'s scripts count as evidence.
pub fn verify_contract(justfile: Option<&str>, package_json: Option<&str>) -> VerifyContract {
    let Some(justfile) = justfile else {
        return VerifyContract {
            present: Vec::new(),
            missing: VERIFY_CONTRACT
                .iter()
                .map(|(name, _)| name.to_string())
                .collect(),
        };
    };
    let mut haystack = strip_comments(justfile);
    if let Some(scripts) = package_json.and_then(npm_scripts) {
        haystack.push_str(&scripts);
    }
    let mut present = Vec::new();
    let mut missing = Vec::new();
    for (name, tokens) in VERIFY_CONTRACT {
        if tokens.iter().all(|token| haystack.contains(token)) {
            present.push(name.to_string());
        } else {
            missing.push(name.to_string());
        }
    }
    VerifyContract { present, missing }
}

fn npm_scripts(package_json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(package_json).ok()?;
    Some(value.get("scripts")?.to_string())
}

/// Removes `#`-to-end-of-line comment segments from a justfile. A `#` inside a recipe line's
/// string argument would also be stripped, but that only ever risks a false "missing" report,
/// never a false green, and no contracted token contains `#`.
fn strip_comments(justfile: &str) -> String {
    let mut stripped = String::with_capacity(justfile.len());
    for line in justfile.lines() {
        match line.find('#') {
            Some(at) => stripped.push_str(&line[..at]),
            None => stripped.push_str(line),
        }
        stripped.push('\n');
    }
    stripped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::{Pr, Reason};

    fn observation() -> Observation {
        Observation {
            observed_at: "2026-08-06T17:24:00+00:00".to_string(),
            commits_ahead: Observed::Present(12),
            tree_clean: Observed::Present(true),
            pr: Observed::Present(Pr {
                number: 30,
                url: "https://github.com/o/n/pull/30".to_string(),
                state: "OPEN".to_string(),
                is_draft: false,
                head_ref: "feat/x".to_string(),
                base_ref: "main".to_string(),
            }),
            checks_pending: Observed::Present(false),
            checks_red: Observed::Present(false),
            plan_files: Observed::Present(vec!["docs/plans/a.md".to_string()]),
            residual_findings: Observed::Present(vec![
                "docs/residual-review-findings/a.md".to_string(),
            ]),
            ledger_entries: Observed::Absent,
            changed_files: Observed::Present(vec![
                "docs/plans/a.md".to_string(),
                "docs/residual-review-findings/a.md".to_string(),
                "src/lib.rs".to_string(),
            ]),
            base_drift: Observed::Unobservable(Reason::saying("not measured in this fixture")),
            pr_head_matches_job_branch: Observed::Present(true),
            pr_base_matches_declared: Observed::Present(true),
        }
    }

    fn all_true() -> RawSignals {
        RawSignals {
            pr_open: Observed::Present(true),
            tree_clean: Observed::Present(true),
            commits_ahead: Observed::Present(true),
            no_check_pending: Observed::Present(true),
            pr_head_matches_job_branch: Observed::Present(true),
            pr_base_matches_declared: Observed::Present(true),
        }
    }

    #[test]
    fn six_present_and_true_signals_are_completed() {
        assert_eq!(verdict(&all_true(), false), Verdict::Completed);
        assert_eq!(verdict(&all_true(), true), Verdict::Completed);
    }

    #[test]
    fn done_promised_with_the_pr_absent_is_uncorroborated_and_never_completed() {
        let mut signals = all_true();
        signals.pr_open = Observed::Present(false);
        let found = verdict(&signals, true);
        assert_ne!(found, Verdict::Completed);
        let Verdict::Uncorroborated(unmet) = found else {
            panic!("a promise the artifacts do not corroborate is its own outcome: {found:?}");
        };
        assert_eq!(unmet, vec!["PR open".to_string()]);
    }

    #[test]
    fn done_promised_with_the_pr_unobservable_is_not_completed() {
        let mut signals = all_true();
        signals.pr_open = Observed::Unobservable(Reason::saying("gh pr view: connection reset"));
        let found = verdict(&signals, true);
        assert_ne!(found, Verdict::Completed);
        assert!(matches!(found, Verdict::Unobserved(_)), "{found:?}");
    }

    #[test]
    fn a_could_not_observe_signal_never_contributes_a_true() {
        for blind in 0..6 {
            let mut signals = all_true();
            let reason = Observed::Unobservable(Reason::saying("connection reset"));
            match blind {
                0 => signals.pr_open = reason,
                1 => signals.tree_clean = reason,
                2 => signals.commits_ahead = reason,
                3 => signals.no_check_pending = reason,
                4 => signals.pr_head_matches_job_branch = reason,
                _ => signals.pr_base_matches_declared = reason,
            }
            for promised in [false, true] {
                assert!(
                    matches!(verdict(&signals, promised), Verdict::Unobserved(_)),
                    "signal {blind} blind, promise {promised}"
                );
            }
        }
    }

    #[test]
    fn a_false_fifth_or_sixth_signal_withholds_completion_like_the_original_four() {
        let mut head_mismatch = all_true();
        head_mismatch.pr_head_matches_job_branch = Observed::Present(false);
        assert!(matches!(
            verdict(&head_mismatch, false),
            Verdict::Incomplete(unmet) if unmet == vec!["PR head matches Job branch".to_string()]
        ));

        let mut base_mismatch = all_true();
        base_mismatch.pr_base_matches_declared = Observed::Present(false);
        assert!(matches!(
            verdict(&base_mismatch, true),
            Verdict::Uncorroborated(unmet)
                if unmet == vec!["PR base matches declared branch".to_string()]
        ));
    }

    #[test]
    fn a_pre_cutover_run_with_no_pr_yet_reads_the_new_signals_as_absent_not_blind() {
        let mut seen = observation();
        seen.pr = Observed::Absent;
        seen.pr_head_matches_job_branch = Observed::Present(false);
        seen.pr_base_matches_declared = Observed::Present(false);
        assert!(matches!(
            verdict(&signals_of(&seen), false),
            Verdict::Incomplete(_)
        ));
        assert!(!matches!(
            verdict(&signals_of(&seen), false),
            Verdict::Unobserved(_)
        ));
    }

    #[test]
    fn no_promise_and_an_unmet_signal_is_incomplete_rather_than_a_judgement() {
        let mut signals = all_true();
        signals.commits_ahead = Observed::Present(false);
        assert_eq!(
            verdict(&signals, false),
            Verdict::Incomplete(vec!["commits ahead".to_string()])
        );
    }

    #[test]
    fn no_verdict_variant_means_rejected_blocked_or_failed() {
        let words = [
            format!("{:?}", Verdict::Completed),
            format!("{:?}", Verdict::Uncorroborated(vec![])),
            format!("{:?}", Verdict::Unobserved(vec![])),
            format!("{:?}", Verdict::Incomplete(vec![])),
        ]
        .join(" ")
        .to_lowercase();
        for banned in ["reject", "block", "fail", "bad", "invalid", "poor"] {
            assert!(
                !words.contains(banned),
                "`{banned}` must not be spellable as a verdict"
            );
        }
    }

    #[test]
    fn the_verify_contract_does_not_change_the_verdict() {
        let gutted = verify_contract(Some("verify:\n    true\n"), None);
        assert_eq!(gutted.missing.len(), 7);
        assert_eq!(verdict(&all_true(), false), Verdict::Completed);
    }

    const INTACT: &str = "verify: fmt clippy test typecheck lint fe-test build\n\
         fmt:\n    cargo fmt --check\n\
         clippy:\n    cargo clippy -- -D warnings\n\
         test:\n    cargo test\n\
         typecheck:\n    npx tsc --noEmit\n\
         lint:\n    npx eslint .\n\
         fe-test:\n    npx vitest run\n\
         build:\n    npm run tauri build -- --debug --no-bundle\n";

    #[test]
    fn a_trimmed_verify_step_on_the_target_repo_is_caught() {
        assert_eq!(
            verify_contract(Some(INTACT), None).missing,
            Vec::<String>::new()
        );
        assert_eq!(
            verify_contract(
                Some(&INTACT.replace("cargo clippy -- -D warnings", "cargo clippy")),
                None
            )
            .missing,
            vec!["rust-clippy".to_string()]
        );
        assert_eq!(
            verify_contract(Some(&INTACT.replace("npx vitest run", "true")), None).missing,
            vec!["ts-test".to_string()]
        );
        assert_eq!(
            verify_contract(
                Some(&INTACT.replace("--debug --no-bundle", "--debug")),
                None
            )
            .missing,
            vec!["build-assertion".to_string()]
        );
    }

    #[test]
    fn no_justfile_means_every_contracted_step_is_missing() {
        let none = verify_contract(None, None);
        assert!(none.present.is_empty());
        assert_eq!(none.missing.len(), VERIFY_CONTRACT.len());
    }

    #[test]
    fn steps_delegated_to_npm_scripts_still_count() {
        let delegating = INTACT.replace("npx vitest run", "npm run test");
        let contract = verify_contract(
            Some(&delegating),
            Some(r#"{"scripts":{"test":"vitest run"}}"#),
        );
        assert_eq!(contract.missing, Vec::<String>::new());
    }

    #[test]
    fn a_step_left_behind_only_as_a_comment_is_reported_missing() {
        let commented = INTACT.replace(
            "    cargo clippy -- -D warnings",
            "    # cargo clippy -- -D warnings",
        );
        let contract = verify_contract(Some(&commented), None);
        assert_eq!(contract.missing, vec!["rust-clippy".to_string()]);
    }

    #[test]
    fn a_live_recipe_line_still_matches_after_comment_stripping() {
        let with_comments = format!(
            "# verify: fmt clippy test typecheck lint fe-test build\n{INTACT}# trailing note\n"
        );
        assert_eq!(
            verify_contract(Some(&with_comments), None).missing,
            Vec::<String>::new()
        );
    }

    #[test]
    fn an_inline_trailing_comment_does_not_break_a_real_invocation() {
        let inline = INTACT.replace(
            "    cargo clippy -- -D warnings",
            "    cargo clippy -- -D warnings # deny warnings on purpose",
        );
        assert_eq!(
            verify_contract(Some(&inline), None).missing,
            Vec::<String>::new()
        );
    }

    #[test]
    fn furthest_stage_reads_reviewed_from_a_run_with_no_pr() {
        let mut seen = observation();
        seen.pr = Observed::Absent;
        assert_eq!(furthest_stage(&seen), Stage::Reviewed);
    }

    #[test]
    fn furthest_stage_walks_the_artifacts_it_can_see() {
        let mut seen = observation();
        assert_eq!(furthest_stage(&seen), Stage::PrOpen);
        seen.pr = Observed::Absent;
        seen.residual_findings = Observed::Absent;
        assert_eq!(furthest_stage(&seen), Stage::Implemented);
        seen.commits_ahead = Observed::Present(0);
        assert_eq!(furthest_stage(&seen), Stage::Planned);
        seen.plan_files = Observed::Absent;
        assert_eq!(furthest_stage(&seen), Stage::Dispatched);
    }

    #[test]
    fn red_ci_lands_on_the_verdict_line_and_does_not_hold_the_verdict_open() {
        let mut seen = observation();
        seen.checks_red = Observed::Present(true);
        seen.checks_pending = Observed::Present(false);
        assert_eq!(verdict(&signals_of(&seen), true), Verdict::Completed);
        assert_eq!(seen.checks_red, Observed::Present(true));
    }

    /// (a) of the amended boundary: pending checks still yield `Incomplete` when the
    /// deliverable does not exist. The PR is absent, so `deliverable_present` is already
    /// false from `pr_open` alone and a pending rollup keeps its plain reading.
    #[test]
    fn a_pending_check_still_holds_completion_open_when_the_pr_is_absent() {
        let mut seen = observation();
        seen.pr = Observed::Absent;
        seen.pr_head_matches_job_branch = Observed::Present(false);
        seen.pr_base_matches_declared = Observed::Present(false);
        seen.checks_pending = Observed::Present(true);
        assert!(matches!(
            verdict(&signals_of(&seen), false),
            Verdict::Incomplete(unmet) if unmet.contains(&"no check pending".to_string())
        ));
    }

    /// (b) of the amended boundary: an absent rollup — which `signals_of` maps to
    /// `no_check_pending = Present(true)` — completes when the deliverable exists.
    #[test]
    fn an_absent_rollup_completes_when_the_deliverable_exists() {
        let mut seen = observation();
        seen.checks_pending = Observed::Absent;
        assert_eq!(verdict(&signals_of(&seen), false), Verdict::Completed);
    }

    /// (c) the Job's own scenario: the deliverable exists — PR open, tree clean, head and base
    /// rows matching — and the visible check rollup is pending-but-not-failed
    /// (`no_check_pending = Present(false)`). The verdict reads `Completed`, so the Run reaches
    /// its terminal state naming the PR open instead of re-entering until the budget dies.
    #[test]
    fn a_pending_but_not_failed_rollup_completes_once_the_deliverable_exists() {
        let mut seen = observation();
        seen.checks_pending = Observed::Present(true);
        assert_eq!(verdict(&signals_of(&seen), false), Verdict::Completed);
        assert_eq!(verdict(&signals_of(&seen), true), Verdict::Completed);
    }

    /// The rollup's carve-out fires only when **every** conjunct of `deliverable_present`
    /// holds, so each of the later three is driven off `Present(true)` in turn while the
    /// rollup stays pending: dropping any one of them from the conjunction must drop the
    /// verdict back to `Incomplete`, naming the row that broke it. Without these, a
    /// regression deleting half the conjunction compiles and keeps the suite green
    /// (validate F3, probe mut-b).
    #[test]
    fn each_deliverable_conjunct_is_load_bearing_while_the_rollup_is_pending() {
        for (row, unmake) in [
            (
                "tree clean",
                Box::new(|seen: &mut Observation| {
                    seen.tree_clean = Observed::Present(false);
                }) as Box<dyn Fn(&mut Observation)>,
            ),
            (
                "PR head matches Job branch",
                Box::new(|seen: &mut Observation| {
                    seen.pr_head_matches_job_branch = Observed::Present(false);
                }),
            ),
            (
                "PR base matches declared branch",
                Box::new(|seen: &mut Observation| {
                    seen.pr_base_matches_declared = Observed::Present(false);
                }),
            ),
        ] {
            let mut seen = observation();
            seen.checks_pending = Observed::Present(true);
            unmake(&mut seen);
            assert!(
                matches!(
                    verdict(&signals_of(&seen), false),
                    Verdict::Incomplete(unmet) if unmet.contains(&row.to_string())
                ),
                "with `{row}` broken, a pending rollup must hold completion open"
            );
            // The same shape without the pending rollup isolates the row's own plain
            // reading, so a failure here names the conjunction, not the carve-out.
            let mut plain = observation();
            unmake(&mut plain);
            assert!(
                matches!(
                    verdict(&signals_of(&plain), false),
                    Verdict::Incomplete(unmet) if unmet.contains(&row.to_string())
                ),
                "`{row}` must read unmet on its own account too"
            );
        }
    }

    /// An unobservable head or base row blinds the fold even when every other deliverable
    /// signal agrees: three-valuedness passes through, and the corroborated reading of a
    /// pending rollup is unreachable on evidence the fold cannot see.
    #[test]
    fn an_unobservable_row_blinds_the_verdict_even_with_the_rest_of_the_deliverable() {
        for unmake in [
            Box::new(|seen: &mut Observation| {
                seen.pr_head_matches_job_branch =
                    Observed::Unobservable(Reason::saying("head ref unreadable"));
            }) as Box<dyn Fn(&mut Observation)>,
            Box::new(|seen: &mut Observation| {
                seen.pr_base_matches_declared =
                    Observed::Unobservable(Reason::saying("base ref unreadable"));
            }),
        ] {
            let mut seen = observation();
            seen.checks_pending = Observed::Present(true);
            unmake(&mut seen);
            assert!(
                matches!(verdict(&signals_of(&seen), false), Verdict::Unobserved(_)),
                "an unobservable deliverable signal must blind, never complete"
            );
        }
    }

    #[test]
    fn a_closed_or_merged_pr_is_not_an_open_one_and_does_not_complete_the_run() {
        for state in ["CLOSED", "MERGED", "closed", "merged"] {
            let mut seen = observation();
            let Observed::Present(pr) = &mut seen.pr else {
                panic!("the fixture holds a PR");
            };
            pr.state = state.to_string();
            assert!(
                matches!(verdict(&signals_of(&seen), false), Verdict::Incomplete(unmet)
                    if unmet.contains(&"PR open".to_string())),
                "a {state} PR must not read as open"
            );
            assert!(matches!(
                verdict(&signals_of(&seen), true),
                Verdict::Uncorroborated(_)
            ));
            assert!(matches!(seen.pr, Observed::Present(_)));
        }
        assert_eq!(
            verdict(&signals_of(&observation()), false),
            Verdict::Completed
        );
    }

    fn rust_only() -> VerifyContract {
        VerifyContract {
            present: vec!["rust-fmt".into(), "rust-clippy".into(), "rust-test".into()],
            missing: vec![
                "ts-typecheck".into(),
                "ts-lint".into(),
                "ts-test".into(),
                "build-assertion".into(),
            ],
        }
    }

    fn changed(paths: &[&str]) -> Observed<Vec<String>> {
        Observed::Present(paths.iter().map(|p| p.to_string()).collect())
    }

    #[test]
    fn a_diff_entirely_inside_a_present_steps_coverage_estimates_zero_uncovered() {
        let found = verify_coverage(&rust_only(), &changed(&["src/lib.rs", "src/observe.rs"]));
        let Observed::Present(estimate) = found else {
            panic!("the diff was readable");
        };
        assert!(estimate.uncovered.is_empty());
        assert_eq!(estimate.changed, 2);
        assert!(estimate.to_string().contains("estimated 0 of 2"));
    }

    #[test]
    fn a_diff_touching_an_extension_no_present_step_covers_names_the_paths() {
        let found = verify_coverage(
            &rust_only(),
            &changed(&["src/lib.rs", "docs/adr/0013.md", "web/app.tsx"]),
        );
        let Observed::Present(estimate) = found else {
            panic!("the diff was readable");
        };
        assert_eq!(estimate.uncovered, vec!["docs/adr/0013.md", "web/app.tsx"]);
        assert_eq!(estimate.changed, 3);
        let said = estimate.to_string();
        assert!(said.contains("estimate"), "{said}");
        assert!(said.contains("web/app.tsx"), "{said}");
    }

    #[test]
    fn a_contract_with_a_step_missing_does_not_count_that_steps_extensions_as_covered() {
        let with_ts = VerifyContract {
            present: vec!["rust-test".into(), "ts-lint".into()],
            missing: vec!["rust-fmt".into()],
        };
        let found = verify_coverage(&with_ts, &changed(&["web/app.tsx"]));
        assert_eq!(
            found,
            Observed::Present(VerifyCoverage {
                uncovered: vec![],
                changed: 1,
            })
        );
    }

    #[test]
    fn an_empty_diff_estimates_zero_and_says_so_without_a_boolean() {
        let found = verify_coverage(&rust_only(), &Observed::Absent);
        assert_eq!(
            found,
            Observed::Present(VerifyCoverage {
                uncovered: vec![],
                changed: 0,
            })
        );
    }

    #[test]
    fn a_diff_that_could_not_be_read_leaves_the_estimate_unobserved() {
        let found = verify_coverage(
            &rust_only(),
            &Observed::Unobservable(Reason::saying("git diff --name-only: exit 128")),
        );
        assert!(matches!(found, Observed::Unobservable(_)), "{found:?}");
    }

    #[test]
    fn no_type_in_the_estimate_carries_an_ok_a_health_word_or_a_summary_flag() {
        let shape = format!(
            "{:?}",
            VerifyCoverage {
                uncovered: vec!["docs/adr/0013.md".to_string()],
                changed: 3,
            }
        )
        .to_lowercase();
        for banned in ["ok", "healthy", "passed", "true", "false", "sufficient"] {
            assert!(!shape.contains(banned), "{shape}");
        }
        assert!(!include_str!("policy.rs").contains("verify_coverage"));
        assert!(!include_str!("supervisor.rs").contains("verify_coverage"));
    }

    #[test]
    fn every_contracted_step_has_a_coverage_entry_and_the_two_lists_cannot_drift() {
        assert_eq!(STEP_COVERAGE.len(), VERIFY_CONTRACT.len());
        for (name, _) in VERIFY_CONTRACT {
            assert!(
                STEP_COVERAGE.iter().any(|(covered, _)| *covered == name),
                "`{name}` is contracted and covers nothing"
            );
        }
    }
}

/// How much review a Run buys. Ord follows declaration order (`T0 < T1 < T2 < T3`) because
/// escalation is a comparison, not a lookup: `select_tier`'s `computed.max(floor)` and the
/// Diff-triage floor it binds both read as plain `Ord` rather than a hand-rolled ranking an
/// agent could get backwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tier {
    T0,
    T1,
    T2,
    T3,
}

impl std::fmt::Display for Tier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let word = match self {
            Tier::T0 => "t0",
            Tier::T1 => "t1",
            Tier::T2 => "t2",
            Tier::T3 => "t3",
        };
        write!(f, "{word}")
    }
}

/// The nine-persona review library (ADR-0015's design, issue #92). Every persona exists whether
/// or not it fires; `panel` decides who fires for a given diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Persona {
    Correctness,
    Security,
    Concurrency,
    Schema,
    Surface,
    Tests,
    Performance,
    Consistency,
    Docs,
}

impl std::fmt::Display for Persona {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let word = match self {
            Persona::Correctness => "correctness",
            Persona::Security => "security",
            Persona::Concurrency => "concurrency",
            Persona::Schema => "schema",
            Persona::Surface => "surface",
            Persona::Tests => "tests",
            Persona::Performance => "performance",
            Persona::Consistency => "consistency",
            Persona::Docs => "docs",
        };
        write!(f, "{word}")
    }
}

/// The Plan stage's own `plan-facts.json` — a grind-owned shape (nothing else writes it), so
/// `deny_unknown_fields` turns a field the writer gained and this reader forgot into a failing
/// test rather than a silently dropped key, the same discipline `view::RunView` uses for
/// `run.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanFacts {
    pub step_count: usize,
    pub forecast_paths: Vec<String>,
    pub new_module_count: usize,
    /// The Job's optional `Declared hot paths` row — human-declared, never Grind classifying
    /// (ADR-0012).
    #[serde(default)]
    pub declared_hot_paths: Vec<String>,
}

/// A risky-path kind, from the compiled literal list the design names. A path is a fact about
/// where the diff touched, never a grade of what it did there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskyPathKind {
    Auth,
    Crypto,
    Payments,
    Migrations,
    PublicApi,
    CiConfig,
    DeploySurface,
}

impl std::fmt::Display for RiskyPathKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let word = match self {
            RiskyPathKind::Auth => "auth",
            RiskyPathKind::Crypto => "crypto",
            RiskyPathKind::Payments => "payments",
            RiskyPathKind::Migrations => "migrations",
            RiskyPathKind::PublicApi => "public-api",
            RiskyPathKind::CiConfig => "ci-config",
            RiskyPathKind::DeploySurface => "deploy-surface",
        };
        write!(f, "{word}")
    }
}

/// A content signal kind. Kinds, not a bare count: the count the tier table reads
/// (`DiffFacts::content_signals`) is a method derived from this list, so the honest observable
/// fact is what a producer constructs and the count can never drift from it by omission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentKind {
    Unsafe,
    RawSql,
    EvalExec,
    Subprocess,
    Concurrency,
    Secrets,
    TodoFixme,
}

impl std::fmt::Display for ContentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let word = match self {
            ContentKind::Unsafe => "unsafe",
            ContentKind::RawSql => "raw-sql",
            ContentKind::EvalExec => "eval-exec",
            ContentKind::Subprocess => "subprocess",
            ContentKind::Concurrency => "concurrency",
            ContentKind::Secrets => "secrets",
            ContentKind::TodoFixme => "todo-fixme",
        };
        write!(f, "{word}")
    }
}

/// The real diff, from `git diff --numstat` plus name-only text (the observe-layer parse this
/// carries is a later phase's build item; `changed_loc` here is already net of lockfile and
/// generated churn — the subtraction is the producer's job, not this type's).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffFacts {
    pub changed_loc: usize,
    pub risky_paths_hit: Vec<RiskyPathKind>,
    pub content_kinds: Vec<ContentKind>,
    pub surface_delta: usize,
    pub dep_manifest_touched: bool,
}

impl DiffFacts {
    /// Derived, never stored: a stored count beside the list it counts is the shape that
    /// drifts. `risky_path_weight` in `docs/tiers.toml` scores this count elsewhere (a diff
    /// score outside the tier table itself); the table below reads the count directly.
    pub fn risky_path_hits(&self) -> usize {
        self.risky_paths_hit.len()
    }

    pub fn content_signals(&self) -> usize {
        self.content_kinds.len()
    }
}

/// A Job template's lookback — statistics, never taxonomy prose (ADR-0012). Kept to the four
/// counts a derived-on-demand read over prior Records and outcome files can honestly produce
/// today; `grind outcomes` (a later phase) is what makes `reverted` non-zero for real.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TrackRecord {
    pub runs: usize,
    pub unattended_completions: usize,
    pub ci_failures: usize,
    pub reverted: usize,
}

impl TrackRecord {
    /// A template with no history (`runs == 0`) is not bad history — the predicate must read
    /// *no data* and *known good* as different things, or a template's first-ever Run would
    /// escalate on nothing. Otherwise: any revert is disqualifying on its own, and a completion
    /// rate under half over a lookback of at least two Runs is what a single unlucky Run cannot
    /// trigger by itself. Both thresholds are this module's own defensible pick, not the
    /// design's literal words — reviewed the same way `docs/tiers.toml` is, by diff.
    pub fn is_bad(&self) -> bool {
        if self.runs == 0 {
            return false;
        }
        self.reverted > 0 || (self.runs >= 2 && self.unattended_completions * 2 < self.runs)
    }
}

/// One prior Run's facts, exactly as much as a Job-template lookback can honestly supply
/// today: the completion and CI facts a Record already carries, plus whatever `grind outcomes`
/// wrote beside that Run — text, not a parsed type, because a stale or hand-edited
/// `outcome.json` degrades to *unread* here rather than aborting the fold (tolerant,
/// degrade-don't-abort, the same rule `learnings.rs` states for foreign-ish formats).
pub struct RunOutcomeFacts<'a> {
    pub completed_unattended: bool,
    pub ci_failed: bool,
    pub outcome_json: Option<&'a str>,
}

/// The fold `template_record` names but does not itself call — **the caller-to-be is Triage in
/// `supervisor.rs`**, which is expected to gather one [`RunOutcomeFacts`] per prior Run of the
/// same Job template (the Record's own completion/CI facts, `outcome.json`'s text when
/// `grind outcomes` has run for that Run) and fold them here. Kept pure and literal-tested so
/// the wiring itself carries no logic to get wrong.
///
/// Reverted is read from `outcome.json`'s `reverted_by` alone; an unreadable or absent
/// `outcome.json` degrades to *not reverted*, matching the design's own honesty note — a stale
/// outcome file only means the tier floor is computed from less history, never from wrong
/// history.
pub fn track_record_from(outcomes: &[RunOutcomeFacts]) -> TrackRecord {
    let mut record = TrackRecord {
        runs: outcomes.len(),
        ..TrackRecord::default()
    };
    for run in outcomes {
        if run.completed_unattended {
            record.unattended_completions += 1;
        }
        if run.ci_failed {
            record.ci_failures += 1;
        }
        if run
            .outcome_json
            .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
            .and_then(|value| value.get("reverted_by").cloned())
            .is_some_and(|reverted_by| reverted_by.as_array().is_some_and(|a| !a.is_empty()))
        {
            record.reverted += 1;
        }
    }
    record
}

/// The parsed contents of `docs/tiers.toml`: the weights, the thresholds and the per-tier model
/// routing table. Carries the calibration in-repo so a threshold move is a reviewed diff, never
/// a recompile-only change (the design's own stated reason for shipping it as data).
///
/// The model map only names the eight stages a Run actually dispatches a session for —
/// `Triage` and `Diff-triage` are `[R]` pure-Rust passes with no session and so no model to
/// route.
#[derive(Debug, Clone, PartialEq)]
pub struct Tiers {
    pub risky_path_weight: u32,
    pub content_signal_weight: u32,
    pub loc_t1: usize,
    pub loc_t2: usize,
    pub loc_t3: usize,
    pub step_count_t1: usize,
    pub step_count_t2: usize,
    pub content_signal_t2: usize,
    pub content_signal_t3: usize,
    pub models_t0: BTreeMap<String, String>,
    pub models_t1: BTreeMap<String, String>,
    pub models_t2: BTreeMap<String, String>,
    pub models_t3: BTreeMap<String, String>,
}

impl Tiers {
    pub fn models_for(&self, tier: Tier) -> &BTreeMap<String, String> {
        match tier {
            Tier::T0 => &self.models_t0,
            Tier::T1 => &self.models_t1,
            Tier::T2 => &self.models_t2,
            Tier::T3 => &self.models_t3,
        }
    }
}

/// **Fail-closed defaults.** These are the values `docs/tiers.toml` ships and the values a
/// garbage or absent file falls back to, byte for byte — the shipped file is a receipt of the
/// compiled default, not a second source of truth for it (see the drift test below). Model
/// routing follows the design's stated split: mechanical stages (`work`, `simplify`, `fixes`,
/// `ship`) run a fast instruction-follower; judgment seats (`plan`, `plan-review`, `validate`)
/// run the strong model always; `review` runs fast at T0/T1, where the panel is small and
/// class-matched, and strong at T2/T3, where the panel is the one doing the scrutiny the tier
/// was raised to buy.
impl Default for Tiers {
    fn default() -> Self {
        fn models(review: &str) -> BTreeMap<String, String> {
            [
                ("plan", "strong"),
                ("plan-review", "strong"),
                ("work", "fast"),
                ("simplify", "fast"),
                ("review", review),
                ("validate", "strong"),
                ("fixes", "fast"),
                ("ship", "fast"),
            ]
            .into_iter()
            .map(|(stage, model)| (stage.to_string(), model.to_string()))
            .collect()
        }
        Tiers {
            risky_path_weight: 5,
            content_signal_weight: 3,
            loc_t1: 80,
            loc_t2: 400,
            loc_t3: 800,
            step_count_t1: 4,
            step_count_t2: 12,
            content_signal_t2: 1,
            content_signal_t3: 3,
            models_t0: models("fast"),
            models_t1: models("fast"),
            models_t2: models("strong"),
            models_t3: models("strong"),
        }
    }
}

/// A tolerant, hand-rolled parse of the subset of TOML `docs/tiers.toml` uses: flat `key =
/// value` pairs under `[section]` headers, integers and quoted strings only, no arrays, no
/// nesting past one level (`[models.t0]` is one flat section name, not a table-of-tables).
/// Unknown keys are ignored, malformed lines are skipped, and any value this loop never
/// assigns keeps `Tiers::default()`'s fail-closed reading — this function starts from that
/// default and only ever overwrites it on a value that actually parsed.
pub fn tiers_from_toml(text: &str) -> Tiers {
    let mut tiers = Tiers::default();
    let mut section = String::new();
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"');
        match section.as_str() {
            "weights" => match key {
                "risky_path_weight" => assign(&mut tiers.risky_path_weight, value),
                "content_signal_weight" => assign(&mut tiers.content_signal_weight, value),
                _ => {}
            },
            "thresholds" => match key {
                "loc_t1" => assign(&mut tiers.loc_t1, value),
                "loc_t2" => assign(&mut tiers.loc_t2, value),
                "loc_t3" => assign(&mut tiers.loc_t3, value),
                "step_count_t1" => assign(&mut tiers.step_count_t1, value),
                "step_count_t2" => assign(&mut tiers.step_count_t2, value),
                "content_signal_t2" => assign(&mut tiers.content_signal_t2, value),
                "content_signal_t3" => assign(&mut tiers.content_signal_t3, value),
                _ => {}
            },
            "models.t0" => insert(&mut tiers.models_t0, key, value),
            "models.t1" => insert(&mut tiers.models_t1, key, value),
            "models.t2" => insert(&mut tiers.models_t2, key, value),
            "models.t3" => insert(&mut tiers.models_t3, key, value),
            _ => {}
        }
    }
    tiers
}

/// A malformed integer leaves the field at whatever it already held — `Tiers::default()`'s
/// value on first assignment, never a partially-parsed number.
fn assign<T: std::str::FromStr>(field: &mut T, value: &str) {
    if let Ok(parsed) = value.parse() {
        *field = parsed;
    }
}

fn insert(map: &mut BTreeMap<String, String>, key: &str, value: &str) {
    map.insert(key.to_string(), value.to_string());
}

/// One row of a Decision's receipts: signal name, the value observed, and what it weighed
/// toward. Strings throughout and no boolean anywhere near it — the same shape
/// `VerifyCoverage` uses above, for the same reason: the Record says what was selected, never
/// what the diff is worth (ADR-0012's optics warning, ADR-0006's variant-set rule).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RationaleRow {
    pub signal: String,
    pub value: String,
    pub weight: String,
}

/// How many plan reviewers a Run's plan-review stage seats. A named struct rather than a bare
/// `usize` for the same reason `VerifyContract` is a struct and not a count: a lone integer in
/// a Decision reads as *a* number, and this one specifically means *reviewer seats*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanReviewDepth {
    pub reviewers: usize,
}

fn depth_for(tier: Tier) -> PlanReviewDepth {
    let reviewers = match tier {
        Tier::T0 | Tier::T1 => 1,
        Tier::T2 => 2,
        Tier::T3 => 3,
    };
    PlanReviewDepth { reviewers }
}

/// Which of the two free tier-selection passes is calling `select_tier`. Triage runs on plan
/// facts alone (preliminary, floor-setting); Diff-triage runs on the real diff and can only
/// raise what Triage set. The pass is what tells `select_tier` whose facts were *required* —
/// `diff: None` is ordinary at Triage and a fail-closed miss at Diff-triage, and nothing else
/// in the signature can tell those apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    Triage,
    DiffTriage,
}

/// A tier call's full receipt: the tier itself, the roster it buys, how deep plan review runs,
/// which model each stage is routed to, the floor it could only raise from, and the rationale
/// rows a human reads to see why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub tier: Tier,
    pub personas: Vec<Persona>,
    pub depth: PlanReviewDepth,
    pub model_per_stage: BTreeMap<String, String>,
    pub floor_from_plan: Tier,
    pub rationale: Vec<RationaleRow>,
}

/// The tier table, exactly as designed: any match wins, highest wins, any ambiguity rounds up.
///
/// **Fail-closed.** The pass names which facts it needs; if they are missing (a Triage call
/// with no `PlanFacts`, or the Diff-triage case the design names by name — a numstat parse that
/// failed) the result is at least T2, with a rationale row saying which fact was absent, rather
/// than silently reading the missing facts as zero signals.
///
/// **Escalation-only.** The final tier is `max(computed, floor)`. Diff-triage binds
/// `floor_from_plan` from the Triage call that ran before it; misclassification can only ever
/// buy more scrutiny, never less.
pub fn select_tier(
    pass: Pass,
    plan: Option<&PlanFacts>,
    diff: Option<&DiffFacts>,
    record: Option<&TrackRecord>,
    floor: Tier,
    tiers: &Tiers,
) -> Decision {
    let required_missing = match pass {
        Pass::Triage => plan.is_none(),
        Pass::DiffTriage => diff.is_none(),
    };

    let mut rationale = Vec::new();

    let computed = if required_missing {
        let missing = match pass {
            Pass::Triage => "plan facts",
            Pass::DiffTriage => "diff facts",
        };
        rationale.push(RationaleRow {
            signal: "missing facts".to_string(),
            value: missing.to_string(),
            weight: "fail-closed to at least t2".to_string(),
        });
        Tier::T2
    } else {
        let step_count = plan.map_or(0, |p| p.step_count);
        let loc = diff.map_or(0, |d| d.changed_loc);
        let risky = diff.map_or(0, |d| d.risky_path_hits());
        let content = diff.map_or(0, |d| d.content_signals());
        let surface = diff.map_or(0, |d| d.surface_delta);
        let dep_touched = diff.is_some_and(|d| d.dep_manifest_touched);

        let mut tier = Tier::T0;

        if loc > tiers.loc_t1 {
            rationale.push(RationaleRow {
                signal: "changed_loc".to_string(),
                value: loc.to_string(),
                weight: format!("> {} -> t1", tiers.loc_t1),
            });
            tier = tier.max(Tier::T1);
        }
        if step_count > tiers.step_count_t1 {
            rationale.push(RationaleRow {
                signal: "plan_step_count".to_string(),
                value: step_count.to_string(),
                weight: format!("> {} -> t1", tiers.step_count_t1),
            });
            tier = tier.max(Tier::T1);
        }
        if loc > tiers.loc_t2 {
            rationale.push(RationaleRow {
                signal: "changed_loc".to_string(),
                value: loc.to_string(),
                weight: format!("> {} -> t2", tiers.loc_t2),
            });
            tier = tier.max(Tier::T2);
        }
        if surface > 0 {
            rationale.push(RationaleRow {
                signal: "surface_delta".to_string(),
                value: surface.to_string(),
                weight: "> 0 -> t2".to_string(),
            });
            tier = tier.max(Tier::T2);
        }
        if dep_touched {
            rationale.push(RationaleRow {
                signal: "dep_manifest_touched".to_string(),
                value: "true".to_string(),
                weight: "-> t2".to_string(),
            });
            tier = tier.max(Tier::T2);
        }
        if content >= tiers.content_signal_t2 {
            let kinds = diff.map_or(String::new(), |d| {
                d.content_kinds
                    .iter()
                    .map(|k| k.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            });
            rationale.push(RationaleRow {
                signal: "content_signals".to_string(),
                value: format!(
                    "{content} ({kinds}, weight {})",
                    tiers.content_signal_weight
                ),
                weight: format!(">= {} -> t2", tiers.content_signal_t2),
            });
            tier = tier.max(Tier::T2);
        }
        if step_count > tiers.step_count_t2 {
            rationale.push(RationaleRow {
                signal: "plan_step_count".to_string(),
                value: step_count.to_string(),
                weight: format!("> {} -> t2", tiers.step_count_t2),
            });
            tier = tier.max(Tier::T2);
        }
        if risky > 0 {
            let kinds = diff.map_or(String::new(), |d| {
                d.risky_paths_hit
                    .iter()
                    .map(|k| k.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            });
            rationale.push(RationaleRow {
                signal: "risky_path_hits".to_string(),
                value: format!("{risky} ({kinds})"),
                weight: format!("weight {} each -> t3", tiers.risky_path_weight),
            });
            tier = tier.max(Tier::T3);
        }
        if let Some(r) = record
            && r.is_bad()
        {
            rationale.push(RationaleRow {
                signal: "template_record".to_string(),
                value: format!(
                    "{} reverted of {} runs, {} unattended completions",
                    r.reverted, r.runs, r.unattended_completions
                ),
                weight: "-> t3".to_string(),
            });
            tier = tier.max(Tier::T3);
        }
        if loc > tiers.loc_t3 && content >= tiers.content_signal_t3 {
            rationale.push(RationaleRow {
                signal: "changed_loc & content_signals".to_string(),
                value: format!("{loc}, {content}"),
                weight: format!("> {} & >= {} -> t3", tiers.loc_t3, tiers.content_signal_t3),
            });
            tier = tier.max(Tier::T3);
        }

        tier
    };

    let tier = computed.max(floor);
    if floor > computed {
        rationale.push(RationaleRow {
            signal: "floor_from_plan".to_string(),
            value: floor.to_string(),
            weight: "escalation-only, raises from below".to_string(),
        });
    }

    Decision {
        tier,
        personas: panel(tier, plan, diff),
        depth: depth_for(tier),
        model_per_stage: tiers.models_for(tier).clone(),
        floor_from_plan: floor,
        rationale,
    }
}

/// Roster selection. Correctness always fires, even at T0 — the one persona the design says is
/// never conditional. Class-matching reads the diff's kind lists (never a bare count, which
/// carries no *which*) and the plan's declared hot paths; `Docs` reads the plan's forecast
/// paths because `DiffFacts` carries no path list of its own — a deliberate choice for this
/// phase, since nothing here has a changed-path list to read that plan-facts doesn't already
/// have.
///
/// Priority order — the order a cap trims from the tail — matches the persona library's own
/// listing. The 2–3 / 4–6 / up-to-9 bands in the design are read here as ceilings the cap
/// enforces, not floors this function pads toward: `Correctness` + `Tests` + `Consistency`
/// already reach the T1 band's low end on their own, and forcing a fourth persona to fire when
/// nothing about the diff class-matches would be inventing a signal that is not there.
///
/// **T3's second-family seats** (a doubled `Correctness`, and `Security` again when it fired)
/// are *not* emitted here yet. The design's roster is a list of personas, and every entry in it
/// maps one-to-one onto a session writing `<stages-dir>/review/<persona>/findings.json` — two
/// seats named alike would write the same file, the second overwriting the first, and the lead's
/// exists-vs-spawned reconciliation could not tell a dead seat from an overwritten one. Until
/// the cross-model wiring phase gives the second seat an identity of its own (a `(persona,
/// seat)` pair with distinct artifact names), a single-model host runs the unique roster: the
/// doubling arrives together with the identity it needs.
pub fn panel(tier: Tier, plan: Option<&PlanFacts>, diff: Option<&DiffFacts>) -> Vec<Persona> {
    if tier == Tier::T0 {
        return vec![Persona::Correctness];
    }

    let risky = diff.map(|d| d.risky_paths_hit.as_slice()).unwrap_or(&[]);
    let content = diff.map(|d| d.content_kinds.as_slice()).unwrap_or(&[]);
    let surface_hit = diff.is_some_and(|d| d.surface_delta > 0);
    let hot_path_hit = plan.is_some_and(|p| !p.declared_hot_paths.is_empty());
    let docs_hit = plan.is_some_and(|p| p.forecast_paths.iter().any(|path| path.contains("docs/")));

    let security_hit = risky.iter().any(|k| {
        matches!(
            k,
            RiskyPathKind::Auth | RiskyPathKind::Crypto | RiskyPathKind::Payments
        )
    }) || content.iter().any(|k| matches!(k, ContentKind::Secrets));
    let concurrency_hit = content
        .iter()
        .any(|k| matches!(k, ContentKind::Concurrency));
    let schema_hit = risky.iter().any(|k| matches!(k, RiskyPathKind::Migrations));

    let candidates = [
        (Persona::Correctness, true),
        (Persona::Security, security_hit),
        (Persona::Concurrency, concurrency_hit),
        (Persona::Schema, schema_hit),
        (Persona::Surface, surface_hit),
        (Persona::Tests, true),
        (Persona::Performance, hot_path_hit),
        (Persona::Consistency, true),
        (Persona::Docs, docs_hit),
    ];

    let cap = match tier {
        Tier::T0 => 1,
        Tier::T1 => 3,
        Tier::T2 => 6,
        Tier::T3 => 9,
    };

    let roster: Vec<Persona> = candidates
        .into_iter()
        .filter(|(_, fires)| *fires)
        .map(|(persona, _)| persona)
        .take(cap)
        .collect();

    roster
}

#[cfg(test)]
mod tier_tests {
    use super::*;

    #[test]
    fn tiers_toml_parses_to_the_compiled_defaults() {
        assert_eq!(
            tiers_from_toml(include_str!("../docs/tiers.toml")),
            Tiers::default()
        );
    }

    #[test]
    fn garbage_toml_falls_back_to_the_fail_closed_defaults() {
        let garbage = "this is not toml at all\n[[[\nkey without equals\n=== \n";
        assert_eq!(tiers_from_toml(garbage), Tiers::default());
    }

    #[test]
    fn unknown_keys_are_ignored_and_known_ones_still_parse() {
        let text = "[thresholds]\nloc_t1 = 80\nmystery_field = 9000\n[bogus.section]\nx = 1\n";
        let tiers = tiers_from_toml(text);
        assert_eq!(tiers.loc_t1, 80);
        assert_eq!(tiers, Tiers::default());
    }

    #[test]
    fn tier_and_persona_serialize_kebab_case_matching_the_skills_vocabulary() {
        for (tier, word) in [
            (Tier::T0, "\"t0\""),
            (Tier::T1, "\"t1\""),
            (Tier::T2, "\"t2\""),
            (Tier::T3, "\"t3\""),
        ] {
            assert_eq!(serde_json::to_string(&tier).unwrap(), word);
            assert_eq!(serde_json::from_str::<Tier>(word).unwrap(), tier);
        }
        assert_eq!(
            serde_json::to_string(&Persona::Correctness).unwrap(),
            "\"correctness\""
        );
        assert_eq!(
            serde_json::to_string(&Persona::Consistency).unwrap(),
            "\"consistency\""
        );
    }

    #[test]
    fn t0_and_t3_are_the_ordering_extremes() {
        assert!(Tier::T0 < Tier::T1);
        assert!(Tier::T1 < Tier::T2);
        assert!(Tier::T2 < Tier::T3);
        assert_eq!(Tier::T3.max(Tier::T1), Tier::T3);
    }

    #[test]
    fn missing_plan_facts_at_triage_is_fail_closed_to_at_least_t2() {
        let decision = select_tier(Pass::Triage, None, None, None, Tier::T0, &Tiers::default());
        assert_eq!(decision.tier, Tier::T2);
        assert!(
            decision
                .rationale
                .iter()
                .any(|r| r.signal == "missing facts" && r.value == "plan facts")
        );
    }

    #[test]
    fn missing_diff_facts_at_diff_triage_is_fail_closed_to_at_least_t2() {
        let decision = select_tier(
            Pass::DiffTriage,
            None,
            None,
            None,
            Tier::T0,
            &Tiers::default(),
        );
        assert_eq!(decision.tier, Tier::T2);
        assert!(
            decision
                .rationale
                .iter()
                .any(|r| r.signal == "missing facts" && r.value == "diff facts")
        );
    }

    #[test]
    fn escalation_is_the_only_direction_a_floor_can_move_a_diff_triage_tier() {
        let tiny_diff = DiffFacts {
            changed_loc: 3,
            risky_paths_hit: vec![],
            content_kinds: vec![],
            surface_delta: 0,
            dep_manifest_touched: false,
        };
        let decision = select_tier(
            Pass::DiffTriage,
            None,
            Some(&tiny_diff),
            None,
            Tier::T2,
            &Tiers::default(),
        );
        assert_eq!(decision.tier, Tier::T2);
        assert!(
            decision
                .rationale
                .iter()
                .any(|r| r.signal == "floor_from_plan")
        );
    }

    #[test]
    fn a_risky_path_hit_alone_reaches_t3_regardless_of_size() {
        let diff = DiffFacts {
            changed_loc: 5,
            risky_paths_hit: vec![RiskyPathKind::Auth],
            content_kinds: vec![],
            surface_delta: 0,
            dep_manifest_touched: false,
        };
        let decision = select_tier(
            Pass::DiffTriage,
            None,
            Some(&diff),
            None,
            Tier::T0,
            &Tiers::default(),
        );
        assert_eq!(decision.tier, Tier::T3);
    }

    #[test]
    fn a_bad_track_record_alone_reaches_t3() {
        let record = TrackRecord {
            runs: 4,
            unattended_completions: 1,
            ci_failures: 0,
            reverted: 0,
        };
        assert!(record.is_bad());
        let diff = DiffFacts {
            changed_loc: 5,
            risky_paths_hit: vec![],
            content_kinds: vec![],
            surface_delta: 0,
            dep_manifest_touched: false,
        };
        let decision = select_tier(
            Pass::DiffTriage,
            None,
            Some(&diff),
            Some(&record),
            Tier::T0,
            &Tiers::default(),
        );
        assert_eq!(decision.tier, Tier::T3);
    }

    #[test]
    fn a_template_with_no_history_is_not_bad_history() {
        assert!(!TrackRecord::default().is_bad());
    }

    #[test]
    fn track_record_from_folds_completion_ci_and_reverted_facts() {
        let outcomes = [
            RunOutcomeFacts {
                completed_unattended: true,
                ci_failed: false,
                outcome_json: Some(r#"{"reverted_by":["deadbeef1"]}"#),
            },
            RunOutcomeFacts {
                completed_unattended: false,
                ci_failed: true,
                outcome_json: Some(r#"{"reverted_by":[]}"#),
            },
            RunOutcomeFacts {
                completed_unattended: true,
                ci_failed: false,
                outcome_json: None,
            },
        ];
        let record = track_record_from(&outcomes);
        assert_eq!(
            record,
            TrackRecord {
                runs: 3,
                unattended_completions: 2,
                ci_failures: 1,
                reverted: 1,
            }
        );
    }

    #[test]
    fn track_record_from_degrades_an_unreadable_outcome_json_to_not_reverted() {
        let outcomes = [RunOutcomeFacts {
            completed_unattended: true,
            ci_failed: false,
            outcome_json: Some("not json"),
        }];
        assert_eq!(track_record_from(&outcomes).reverted, 0);
    }

    #[test]
    fn track_record_from_over_no_runs_is_the_default() {
        assert_eq!(track_record_from(&[]), TrackRecord::default());
    }

    #[test]
    fn panel_is_correctness_only_at_t0_even_with_a_risky_diff() {
        let diff = DiffFacts {
            changed_loc: 5,
            risky_paths_hit: vec![RiskyPathKind::Auth],
            content_kinds: vec![],
            surface_delta: 0,
            dep_manifest_touched: false,
        };
        assert_eq!(
            panel(Tier::T0, None, Some(&diff)),
            vec![Persona::Correctness]
        );
    }

    #[test]
    fn t3_roster_stays_unique_and_security_only_when_it_fired() {
        let with_security = DiffFacts {
            changed_loc: 5,
            risky_paths_hit: vec![RiskyPathKind::Crypto],
            content_kinds: vec![],
            surface_delta: 0,
            dep_manifest_touched: false,
        };
        let roster = panel(Tier::T3, None, Some(&with_security));
        assert!(roster.contains(&Persona::Correctness));
        assert_eq!(
            roster.iter().filter(|p| **p == Persona::Security).count(),
            1
        );
        let unique = roster.iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(
            unique.len(),
            roster.len(),
            "every roster entry maps one-to-one onto review/<persona>/findings.json — a \
             duplicate seat would overwrite its sibling's file and defeat the lead's \
             exists-vs-spawned reconciliation (#117)"
        );

        let no_risky_diff = DiffFacts {
            changed_loc: 5,
            risky_paths_hit: vec![],
            content_kinds: vec![],
            surface_delta: 0,
            dep_manifest_touched: false,
        };
        let roster = panel(Tier::T3, None, Some(&no_risky_diff));
        assert_eq!(
            roster.iter().filter(|p| **p == Persona::Security).count(),
            0,
            "security never fired on this diff"
        );
    }

    #[test]
    fn depth_scales_one_two_three_by_tier() {
        assert_eq!(depth_for(Tier::T0).reviewers, 1);
        assert_eq!(depth_for(Tier::T1).reviewers, 1);
        assert_eq!(depth_for(Tier::T2).reviewers, 2);
        assert_eq!(depth_for(Tier::T3).reviewers, 3);
    }

    #[test]
    fn model_routing_differs_between_a_light_and_a_heavy_tier() {
        let tiers = Tiers::default();
        assert_eq!(
            tiers.models_for(Tier::T0).get("review").map(String::as_str),
            Some("fast")
        );
        assert_eq!(
            tiers.models_for(Tier::T3).get("review").map(String::as_str),
            Some("strong")
        );
        for tier in [Tier::T0, Tier::T1, Tier::T2, Tier::T3] {
            assert_eq!(
                tiers.models_for(tier).get("plan").map(String::as_str),
                Some("strong")
            );
            assert_eq!(
                tiers.models_for(tier).get("validate").map(String::as_str),
                Some("strong")
            );
        }
    }

    fn tiers() -> Tiers {
        Tiers::default()
    }

    /// Runs both passes the way a real Run would: Triage on plan facts alone sets the floor,
    /// then Diff-triage reads the real diff against it.
    fn replay(plan: &PlanFacts, diff: &DiffFacts, record: Option<&TrackRecord>) -> Tier {
        let triage = select_tier(Pass::Triage, Some(plan), None, record, Tier::T0, &tiers());
        let diff_triage = select_tier(
            Pass::DiffTriage,
            Some(plan),
            Some(diff),
            record,
            triage.tier,
            &tiers(),
        );
        diff_triage.tier
    }

    #[test]
    fn run_1_snapper_21_lands_t3_on_the_deploy_surface_hit() {
        let plan = PlanFacts {
            step_count: 16,
            forecast_paths: vec!["src-tauri/Info.plist".to_string()],
            new_module_count: 1,
            declared_hot_paths: vec![],
        };
        let diff = DiffFacts {
            changed_loc: 1200,
            risky_paths_hit: vec![RiskyPathKind::DeploySurface],
            content_kinds: vec![],
            surface_delta: 0,
            dep_manifest_touched: true,
        };
        assert_eq!(replay(&plan, &diff, None), Tier::T3);
    }

    #[test]
    fn run_2_snapper_28_lands_t2_on_size_and_surface() {
        let plan = PlanFacts {
            step_count: 36,
            forecast_paths: vec![],
            new_module_count: 3,
            declared_hot_paths: vec![],
        };
        let diff = DiffFacts {
            changed_loc: 4574,
            risky_paths_hit: vec![],
            content_kinds: vec![ContentKind::Concurrency],
            surface_delta: 1,
            dep_manifest_touched: true,
        };
        assert_eq!(replay(&plan, &diff, None), Tier::T2);
    }

    #[test]
    fn run_3_grind_80_lands_t1_just_over_the_loc_floor() {
        let plan = PlanFacts {
            step_count: 3,
            forecast_paths: vec![
                "docs/plans/2026-08-21-002-fix-amend-named-test-spec-plan.md".to_string(),
            ],
            new_module_count: 0,
            declared_hot_paths: vec![],
        };
        let diff = DiffFacts {
            changed_loc: 165,
            risky_paths_hit: vec![],
            content_kinds: vec![],
            surface_delta: 0,
            dep_manifest_touched: false,
        };
        assert_eq!(replay(&plan, &diff, None), Tier::T1);
    }

    #[test]
    fn run_4_grind_87_lands_t2_the_supervisor_feature_diff() {
        let plan = PlanFacts {
            step_count: 6,
            forecast_paths: vec![],
            new_module_count: 0,
            declared_hot_paths: vec![],
        };
        let diff = DiffFacts {
            changed_loc: 1041,
            risky_paths_hit: vec![],
            content_kinds: vec![ContentKind::Concurrency],
            surface_delta: 1,
            dep_manifest_touched: false,
        };
        assert_eq!(replay(&plan, &diff, None), Tier::T2);
    }
}
