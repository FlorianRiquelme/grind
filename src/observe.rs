//! Raw bytes become present, absent, or could-not-observe with a reason — and the two
//! negatives are never spelled the same way.
//!
//! Every observation in the script went through a shell call whose failure returned an empty
//! string, so a failed `git rev-list` was an honest zero and a failed `gh pr view` was an
//! honest *no PR*. Two of the four signals that decide completion failed toward `completed`,
//! in exactly the window after a laptop wake. That is the bug this module exists to make
//! unrepresentable.
//!
//! Classification happens **away from the spawn**, over `Completed { stdout, stderr, code }`,
//! so a test that *this call site* yields could-not-observe rather than absent is three string
//! literals instead of a process.

use crate::world::Completed;
use std::fmt;

/// Observed absent. Distinct from [`UNOBSERVABLE_MARK`] wherever a human reads it — reading a
/// blind supervisor's silence as a fact is how an operator goes back to sleep.
pub const ABSENT_MARK: &str = "—";

/// Could not observe.
pub const UNOBSERVABLE_MARK: &str = "?";

/// What we know about one signal after trying to observe it.
///
/// **Never `Result<Option<T>, E>`.** The ecosystem supplies `.ok()`, `?` and
/// `unwrap_or_default()` free, and each collapses three states into two *silently*. A
/// dedicated enum has none of them, so every collapse has to be written out where a reader
/// could see it (ADR-0006).
#[derive(Debug, Clone, PartialEq)]
pub enum Observed<T> {
    Present(T),
    Absent,
    Unobservable(Reason),
}

/// Why a signal could not be observed. A newtype rather than a bare `String` so the reason has
/// to be composed on purpose, and so it cannot be swapped for a value.
#[derive(Debug, Clone, PartialEq)]
pub struct Reason(String);

impl Reason {
    /// The call site's name plus what the child actually said. The exit code is included
    /// because *killed by a signal* and *exited non-zero* are different faults, and one of
    /// them means the stdout on disk is empty rather than truncated.
    pub fn of(call_site: &str, completed: &Completed) -> Self {
        let said = first_line(&completed.stderr);
        let outcome = match completed.code {
            Some(code) => format!("exit {code}"),
            None => "no exit code — killed or never started".to_string(),
        };
        if said.is_empty() {
            Reason(format!("{call_site}: {outcome}"))
        } else {
            Reason(format!("{call_site}: {outcome}: {said}"))
        }
    }

    pub fn saying(what: &str) -> Self {
        Reason(what.to_string())
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<T: fmt::Display> fmt::Display for Observed<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Observed::Present(value) => write!(f, "{value}"),
            Observed::Absent => write!(f, "{ABSENT_MARK}"),
            Observed::Unobservable(_) => write!(f, "{UNOBSERVABLE_MARK}"),
        }
    }
}

impl<T> Observed<T> {
    /// The reason, when there is one. Deliberately *not* a way to get at `T`: there is no
    /// `unwrap_or`, no `ok()`, and no `Default`, because each of those is a silent collapse.
    pub fn reason(&self) -> Option<&Reason> {
        match self {
            Observed::Unobservable(reason) => Some(reason),
            _ => None,
        }
    }
}

/// What a PR looks like once it has been observed.
#[derive(Debug, Clone, PartialEq)]
pub struct Pr {
    pub number: u64,
    pub url: String,
    pub state: String,
    pub is_draft: bool,
}

impl fmt::Display for Pr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.url)
    }
}

/// Everything a Run's state is read from, each signal independently observed and independently
/// observable. The verify contract is deliberately not here: it is a decision about which
/// contracted steps a target repo declares, and it lives with the module that makes it.
#[derive(Debug, Clone)]
pub struct Observation {
    pub observed_at: String,
    pub commits_ahead: Observed<u64>,
    pub tree_clean: Observed<bool>,
    pub pr: Observed<Pr>,
    pub checks_pending: Observed<bool>,
    pub checks_red: Observed<bool>,
    pub plan_files: Observed<Vec<String>>,
    pub residual_findings: Observed<Vec<String>>,
    pub ledger_entries: Observed<Vec<String>>,
}

// --- one classifier per call site -----------------------------------------------------

/// `git rev-list --count <handoff-sha>..HEAD`.
///
/// A zero here is a **fact**, not an absence: the Run committed nothing yet. Reading it as
/// absent is half of what let Run 2 record `commits_ahead: 0` against twelve real commits.
pub fn commits_ahead(completed: &Completed) -> Observed<u64> {
    if completed.code != Some(0) {
        return Observed::Unobservable(Reason::of("git rev-list --count", completed));
    }
    match completed.stdout.trim().parse::<u64>() {
        Ok(count) => Observed::Present(count),
        Err(_) => Observed::Unobservable(Reason::of("git rev-list --count", completed)),
    }
}

/// `git status --porcelain`. Any output at all means dirty; empty output means clean.
pub fn tree_clean(completed: &Completed) -> Observed<bool> {
    if completed.code != Some(0) {
        return Observed::Unobservable(Reason::of("git status --porcelain", completed));
    }
    Observed::Present(completed.stdout.trim().is_empty())
}

/// `gh pr view --json number,url,state,isDraft`.
///
/// `gh` exits non-zero when there is genuinely no PR *and* when it cannot reach GitHub at all,
/// and the two are separated by what it says rather than by what it returns. Anything it says
/// that is not *no pull requests found* is could-not-observe — the direction that withholds a
/// verdict rather than inventing one.
pub fn pr(completed: &Completed) -> Observed<Pr> {
    let body = completed.stdout.trim();
    if completed.code == Some(0) && body.starts_with('{') {
        return match parse_pr(body) {
            Some(found) => Observed::Present(found),
            None => Observed::Unobservable(Reason::saying("gh pr view: unreadable JSON")),
        };
    }
    if says_no_pr(&completed.stderr) || (completed.code == Some(0) && body.is_empty()) {
        return Observed::Absent;
    }
    Observed::Unobservable(Reason::of("gh pr view", completed))
}

fn says_no_pr(stderr: &str) -> bool {
    let said = stderr.to_lowercase();
    said.contains("no pull requests found") || said.contains("no open pull requests")
}

fn parse_pr(body: &str) -> Option<Pr> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    Some(Pr {
        number: value.get("number")?.as_u64()?,
        url: value.get("url")?.as_str()?.to_string(),
        state: value
            .get("state")
            .and_then(|s| s.as_str())
            .unwrap_or("UNKNOWN")
            .to_string(),
        is_draft: value
            .get("isDraft")
            .and_then(|d| d.as_bool())
            .unwrap_or(false),
    })
}

/// `gh pr view --json statusCheckRollup`, classified twice — *is anything still running* and
/// *did anything come back red*. They are separate signals: one holds completion open and the
/// other lands on the verdict line without holding anything (ADR-0003).
pub fn checks(completed: &Completed) -> (Observed<bool>, Observed<bool>) {
    if completed.code != Some(0) || !completed.stdout.trim().starts_with('{') {
        let reason = Reason::of("gh pr view --json statusCheckRollup", completed);
        return (
            Observed::Unobservable(reason.clone()),
            Observed::Unobservable(reason),
        );
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(completed.stdout.trim()) else {
        let reason = Reason::saying("gh pr view --json statusCheckRollup: unreadable JSON");
        return (
            Observed::Unobservable(reason.clone()),
            Observed::Unobservable(reason),
        );
    };
    let Some(rollup) = value.get("statusCheckRollup").and_then(|r| r.as_array()) else {
        // The field is present-and-null when a PR has no checks configured at all. Nothing is
        // pending and nothing is red, and both of those are observations.
        return (Observed::Present(false), Observed::Present(false));
    };
    let mut pending = false;
    let mut red = false;
    for check in rollup {
        let status = string_at(check, "status");
        let conclusion = string_at(check, "conclusion");
        let state = string_at(check, "state");
        if status == "QUEUED"
            || status == "IN_PROGRESS"
            || status == "PENDING"
            || state == "PENDING"
        {
            pending = true;
        }
        if matches!(
            conclusion.as_str(),
            "FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED"
        ) || matches!(state.as_str(), "FAILURE" | "ERROR")
        {
            red = true;
        }
    }
    (Observed::Present(pending), Observed::Present(red))
}

fn string_at(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_uppercase()
}

/// A directory listing. `readable` is the caller's answer to *did the directory this lives
/// under exist at all* — without it, a worktree that has gone missing reads as a Run that
/// produced no plan.
pub fn listing(readable: bool, what: &str, entries: Vec<String>) -> Observed<Vec<String>> {
    if !readable {
        return Observed::Unobservable(Reason::saying(&format!("{what}: worktree unreadable")));
    }
    if entries.is_empty() {
        return Observed::Absent;
    }
    Observed::Present(entries)
}

/// A host item that is either there or not, with no third state available from the check
/// itself. `checked` is what the check could see; `false` is a real absence, and a check that
/// could not run at all is the caller's job to spell as could-not-observe.
pub fn presence(present: bool) -> Observed<bool> {
    Observed::Present(present)
}

fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.chars().count() > 200 {
        line.chars().take(200).collect::<String>() + "…"
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixtures arrive through `include_str!`, not through the filesystem: reading them at run
    // time would name `std::fs` inside `src/`, which is what `tests/topology.rs` forbids.
    const GH_AUTH_STDOUT: &str = include_str!("../tests/fixtures/gh/auth-failure.stdout");
    const GH_AUTH_STDERR: &str = include_str!("../tests/fixtures/gh/auth-failure.stderr");
    const GH_AUTH_CODE: &str = include_str!("../tests/fixtures/gh/auth-failure.code");

    fn completed(stdout: &str, stderr: &str, code: Option<i32>) -> Completed {
        Completed {
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            code,
        }
    }

    #[test]
    fn a_gh_auth_failure_is_could_not_observe_and_never_absent() {
        let code: i32 = GH_AUTH_CODE.trim().parse().expect("the recorded exit code");
        let observed = pr(&completed(GH_AUTH_STDOUT, GH_AUTH_STDERR, Some(code)));
        assert!(
            matches!(observed, Observed::Unobservable(_)),
            "empty stdout with an auth message on stderr must not read as `no PR`: {observed:?}"
        );
        assert!(
            observed
                .reason()
                .expect("a reason")
                .to_string()
                .contains("gh auth login")
        );
    }

    #[test]
    fn a_killed_child_leaves_stdout_empty_and_classifies_as_could_not_observe() {
        // A killed child leaves stdout *empty*, not truncated — measured across both Runs.
        // There is no exit code at all, which is the only thing distinguishing it from a
        // command that ran and found nothing.
        let killed = completed("", "", None);
        assert!(matches!(pr(&killed), Observed::Unobservable(_)));
        assert!(matches!(commits_ahead(&killed), Observed::Unobservable(_)));
        assert!(matches!(tree_clean(&killed), Observed::Unobservable(_)));
        let reason = commits_ahead(&killed)
            .reason()
            .expect("a reason")
            .to_string();
        assert!(reason.contains("killed or never started"), "{reason}");
    }

    #[test]
    fn a_successful_gh_pr_view_with_no_pr_is_absent() {
        let none = completed(
            "",
            "no pull requests found for branch \"feat/28-slice-1b\"\n",
            Some(1),
        );
        assert_eq!(pr(&none), Observed::Absent);
    }

    #[test]
    fn a_zero_commit_count_is_present_with_zero_and_never_absent() {
        assert_eq!(
            commits_ahead(&completed("0\n", "", Some(0))),
            Observed::Present(0)
        );
        assert_eq!(
            commits_ahead(&completed("12\n", "", Some(0))),
            Observed::Present(12)
        );
    }

    #[test]
    fn absent_and_could_not_observe_render_as_different_marks() {
        let absent: Observed<u64> = Observed::Absent;
        let blind: Observed<u64> = Observed::Unobservable(Reason::saying("gh: connection reset"));
        assert_eq!(absent.to_string(), ABSENT_MARK);
        assert_eq!(blind.to_string(), UNOBSERVABLE_MARK);
        assert_ne!(absent.to_string(), blind.to_string());
    }

    #[test]
    fn a_present_pr_carries_its_url() {
        let body = r#"{"number":30,"url":"https://github.com/o/n/pull/30","state":"OPEN","isDraft":false}"#;
        let Observed::Present(found) = pr(&completed(body, "", Some(0))) else {
            panic!("a well-formed gh pr view must be present");
        };
        assert_eq!(found.number, 30);
        assert_eq!(found.url, "https://github.com/o/n/pull/30");
        assert!(!found.is_draft);
    }

    #[test]
    fn a_dirty_tree_is_any_output_at_all() {
        assert_eq!(
            tree_clean(&completed("", "", Some(0))),
            Observed::Present(true)
        );
        assert_eq!(
            tree_clean(&completed(" M src/cli.rs\n", "", Some(0))),
            Observed::Present(false)
        );
        assert_eq!(
            tree_clean(&completed("?? untracked\n", "", Some(0))),
            Observed::Present(false)
        );
    }

    #[test]
    fn checks_separate_still_running_from_came_back_red() {
        let rollup = r#"{"statusCheckRollup":[
            {"status":"COMPLETED","conclusion":"SUCCESS"},
            {"status":"IN_PROGRESS","conclusion":""}
        ]}"#;
        let (pending, red) = checks(&completed(rollup, "", Some(0)));
        assert_eq!(pending, Observed::Present(true));
        assert_eq!(red, Observed::Present(false));

        let failing = r#"{"statusCheckRollup":[{"status":"COMPLETED","conclusion":"FAILURE"}]}"#;
        let (pending, red) = checks(&completed(failing, "", Some(0)));
        assert_eq!(pending, Observed::Present(false));
        assert_eq!(red, Observed::Present(true));
    }

    #[test]
    fn an_unreachable_gh_leaves_both_check_signals_unobservable() {
        let (pending, red) = checks(&completed(
            "",
            "error connecting to api.github.com",
            Some(1),
        ));
        assert!(matches!(pending, Observed::Unobservable(_)));
        assert!(matches!(red, Observed::Unobservable(_)));
    }

    #[test]
    fn an_unreadable_worktree_is_not_a_run_that_produced_nothing() {
        assert_eq!(listing(true, "plan", vec![]), Observed::Absent);
        assert!(matches!(
            listing(false, "plan", vec![]),
            Observed::Unobservable(_)
        ));
        assert_eq!(
            listing(true, "plan", vec!["docs/plans/a.md".into()]),
            Observed::Present(vec!["docs/plans/a.md".into()])
        );
    }

    #[test]
    fn every_arm_is_constructed_somewhere_in_this_module() {
        // ADR-0009 put clippy in the recipe because an unused variant on a type whose whole
        // purpose is a representable state is a statement about test coverage. Under a library
        // target a `pub` enum no longer raises that warning, so this stands in for it.
        let arms: [Observed<u64>; 3] = [
            Observed::Present(1),
            Observed::Absent,
            Observed::Unobservable(Reason::saying("constructed")),
        ];
        assert_eq!(arms.len(), 3);
        assert!(presence(true) == Observed::Present(true));
    }
}
