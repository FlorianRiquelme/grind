//! Which signals corroborate what — the furthest stage, and the four ANDed observations that
//! decide completion.
//!
//! **A fifth completion signal cannot be added and forgotten.** `RawSignals` is a named struct,
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
    // Only the steps that are **present** cover anything. A contract with `ts-test` missing does
    // not get to claim its extensions.
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

/// The four observations completion is ANDed from.
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
        // **A PR that was found is not a PR that is open.** The head-commit lookup asks with
        // `--state all` deliberately — a closed PR still belongs in the Handback's PR row, and
        // deleting it from the search would answer a different question. But the signal
        // completion is ANDed from is *`pr_open`*, and Grind's contract is *stops at an open
        // PR*: a Run whose PR a human closed was otherwise four-for-four and reported
        // `Completed`, printing the closed PR's URL as its result on the Job issue.
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
    }
}

/// The fold. Completion is **observed rather than declared**, and the DONE promise is neither
/// necessary nor sufficient — two Runs finished a pipeline without emitting it, and a session
/// that believes it finished can emit it against nothing.
pub fn verdict(signals: &RawSignals, done_promise: bool) -> Verdict {
    // No `..` and no `field: _`. Adding a fifth signal stops this line compiling, and the
    // binding it then forces is unused until it is folded in below — which is an error under
    // `-D warnings`.
    let RawSignals {
        pr_open,
        tree_clean,
        commits_ahead,
        no_check_pending,
    } = signals;

    let named: [(&str, &Observed<bool>); 4] = [
        ("PR open", pr_open),
        ("tree clean", tree_clean),
        ("commits ahead", commits_ahead),
        ("no check pending", no_check_pending),
    ];

    let mut blind = Vec::new();
    let mut unmet = Vec::new();
    for (name, signal) in named {
        match signal {
            // A could-not-observe signal never contributes a true.
            Observed::Present(true) => {}
            Observed::Present(false) => unmet.push(name.to_string()),
            Observed::Absent => unmet.push(name.to_string()),
            Observed::Unobservable(reason) => blind.push(format!("{name}: {reason}")),
        }
    }

    // One signal Grind could not ask about is enough to withhold a verdict, even when every
    // signal it *could* ask looked satisfied.
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
    // A step trimmed to green but left behind as a comment (`# cargo clippy -- -D warnings`) must
    // not read as present: the contract exists to catch exactly that trim (#82). Strip
    // `#`-to-end-of-line segments line-wise before matching — a noisy missing report is always
    // safer than a false green (ADR-0003), and package.json scripts carry no comments so that
    // path is left untouched.
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
        }
    }

    fn all_true() -> RawSignals {
        RawSignals {
            pr_open: Observed::Present(true),
            tree_clean: Observed::Present(true),
            commits_ahead: Observed::Present(true),
            no_check_pending: Observed::Present(true),
        }
    }

    #[test]
    fn four_present_and_true_signals_are_completed() {
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
        // Every arrangement with a blind signal withholds the verdict, even when the three
        // signals Grind *could* ask all looked satisfied.
        for blind in 0..4 {
            let mut signals = all_true();
            let reason = Observed::Unobservable(Reason::saying("connection reset"));
            match blind {
                0 => signals.pr_open = reason,
                1 => signals.tree_clean = reason,
                2 => signals.commits_ahead = reason,
                _ => signals.no_check_pending = reason,
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
        // A variant set is a policy: a careless type makes a forbidden thing newly expressible,
        // and expressible means reachable because nobody reads the diff.
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
        // A precondition must not quietly become a termination condition, so the contract is
        // not a field on `RawSignals` and cannot reach the fold at all.
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
        // Inherited from the Python entrypoint this replaced: the failure that costs most is
        // a step trimmed until it goes green, because that is a false positive on the target
        // repo and on Grind in one shot.
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
        // The exact failure the contract exists to catch: a step trimmed until it goes green,
        // with the invocation surviving as a comment. The commented mention must not count.
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

    #[test]
    fn a_pending_check_holds_completion_open_and_an_absent_one_does_not() {
        let mut seen = observation();
        seen.checks_pending = Observed::Present(true);
        assert!(matches!(
            verdict(&signals_of(&seen), false),
            Verdict::Incomplete(_)
        ));
        // `observe::checks` could not return `Absent` before Fix 1 — it is what a Run with no
        // PR yet now produces, and `signals_of` reads it as nothing left to hold open, not as
        // blind.
        seen.checks_pending = Observed::Absent;
        assert_eq!(verdict(&signals_of(&seen), false), Verdict::Completed);
    }

    #[test]
    fn a_closed_or_merged_pr_is_not_an_open_one_and_does_not_complete_the_run() {
        // The head-commit lookup asks with `--state all`, so a PR a human closed is still
        // `Observed::Present` — and every other completion signal on such a Run is satisfied.
        // Grind's contract is *stops at an open PR*; the closed one must hold it incomplete.
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
            // The promise does not rescue it either: DONE over a closed PR is uncorroborated.
            assert!(matches!(
                verdict(&signals_of(&seen), true),
                Verdict::Uncorroborated(_)
            ));
            // And the PR itself stays observed, so the Handback's row still names it.
            assert!(matches!(seen.pr, Observed::Present(_)));
        }
        // The spelling `gh` actually returns still completes the Run.
        assert_eq!(
            verdict(&signals_of(&observation()), false),
            Verdict::Completed
        );
    }

    // --- the verify-coverage estimate ------------------------------------------------------

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
        // `rust-only` above declares no TypeScript step, so `.tsx` is uncovered. Add them and
        // the same path becomes covered — the estimate reads the contract, not the ecosystem.
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
        // And the estimate never gates: nothing in this module branches on it.
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
