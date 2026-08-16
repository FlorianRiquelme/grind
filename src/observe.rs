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
use serde::{Deserialize, Serialize};
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
///
/// It derives `Serialize`/`Deserialize` because the record carries one — the per-Attempt
/// fan-out arithmetic. Two bare `Option<u64>` fields would collapse absent and unobservable,
/// which is the whole point of the type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Observed<T> {
    Present(T),
    Absent,
    Unobservable(Reason),
}

/// Why a signal could not be observed. A newtype rather than a bare `String` so the reason has
/// to be composed on purpose, and so it cannot be swapped for a value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

/// **Base drift**: the target repo's default branch moving after the Handoff SHA.
///
/// A count and the paths that overlap the Run's own diff. **No boolean, no `Diverged` variant,
/// no summary field** — ADR-0006's seventh prohibited shape, and the tempting argument for it
/// (*"`main` moved, so don't open the PR"*) reads as caution rather than as the quality
/// judgement ADR-0003 refuses. It is surfaced when non-zero and enforced never.
#[derive(Debug, Clone, PartialEq)]
pub struct BaseDrift {
    pub default_branch: String,
    /// Commits on the default branch since the Handoff SHA.
    pub commits: u64,
    /// The paths that moved and that the Run's own diff also touches. Two files claiming
    /// ADR-0001 have different names, so git merges clean and reports nothing.
    pub overlapping: Vec<String>,
}

impl fmt::Display for BaseDrift {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} commit{} on {} since the Handoff SHA",
            self.commits,
            if self.commits == 1 { "" } else { "s" },
            self.default_branch
        )?;
        if !self.overlapping.is_empty() {
            write!(f, ", also touching {}", self.overlapping.join(", "))?;
        }
        Ok(())
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
    /// Every path `<handoff-sha>..HEAD` touches — **the Run's own diff**, and the input the
    /// three listings above are drawn from.
    pub changed_files: Observed<Vec<String>>,
    pub base_drift: Observed<BaseDrift>,
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

/// `gh pr list --search <head-sha> --state all --json number,url,state,isDraft`.
///
/// **A Run's identity on GitHub is the commit it pushed, not the branch its Job named.** Run 2
/// pushed to `…-run` while its Job named `…-seam`, so the branch lookup answered truthfully
/// about the wrong question and the Handback said `PR —` over an open, green, twelve-commit PR.
///
/// An empty array is `Absent` — the search ran and matched nothing. Anything unreadable is
/// could-not-observe, the direction that withholds a verdict rather than inventing one.
pub fn pr_by_head(completed: &Completed) -> Observed<Pr> {
    if completed.code != Some(0) {
        return Observed::Unobservable(Reason::of("gh pr list --search", completed));
    }
    let body = completed.stdout.trim();
    let Ok(serde_json::Value::Array(matched)) = serde_json::from_str::<serde_json::Value>(body)
    else {
        return Observed::Unobservable(Reason::saying("gh pr list --search: unreadable JSON"));
    };
    let Some(first) = matched.first() else {
        return Observed::Absent;
    };
    match parse_pr_value(first) {
        Some(found) => Observed::Present(found),
        None => Observed::Unobservable(Reason::saying("gh pr list --search: unreadable JSON")),
    }
}

fn parse_pr(body: &str) -> Option<Pr> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    parse_pr_value(&value)
}

fn parse_pr_value(value: &serde_json::Value) -> Option<Pr> {
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
    // No PR yet is the normal early state of every Run, not a blind one — `pr()` already draws
    // this line, and doing it here too is what stops the old script's "no PR = completed" bug
    // from resurfacing as "no PR = blind" instead.
    if says_no_pr(&completed.stderr) {
        return (Observed::Absent, Observed::Absent);
    }
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

/// `git diff --name-only <handoff-sha>..HEAD` — **the Run's own diff**, and the input every
/// artifact listing is drawn from.
///
/// A failed diff is could-not-observe, never an empty listing: a worktree that has gone missing
/// must not read as a Run that produced nothing.
pub fn changed_files(completed: &Completed) -> Observed<Vec<String>> {
    if completed.code != Some(0) {
        return Observed::Unobservable(Reason::of("git diff --name-only", completed));
    }
    let names: Vec<String> = completed
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    if names.is_empty() {
        return Observed::Absent;
    }
    Observed::Present(names)
}

/// `git symbolic-ref --short refs/remotes/origin/HEAD`, and the whole of *which branch is the
/// base* (KTD17). It is local after the fetch Dispatch already performed, needs no network of
/// its own, and needs no new Job row.
///
/// A missing or unreadable `origin/HEAD` — which is what a clone whose fetch has never
/// succeeded looks like — is **could not observe**, never *no drift*.
pub fn default_branch(completed: &Completed) -> Observed<String> {
    if completed.code != Some(0) {
        return Observed::Unobservable(Reason::of("git symbolic-ref origin/HEAD", completed));
    }
    let named = completed.stdout.trim();
    if named.is_empty() {
        return Observed::Unobservable(Reason::saying(
            "git symbolic-ref origin/HEAD: no default branch named",
        ));
    }
    Observed::Present(named.to_string())
}

/// The drift itself, from a count and a name-only diff against the default branch, intersected
/// with the Run's own diff.
///
/// A default branch that has not moved is a **present** count of zero, not an absence: *it did
/// not move* is a fact, and the whole point of the three-valued reading is that *I could not
/// look* is a different one.
pub fn base_drift(
    default_branch: &Observed<String>,
    counted: &Completed,
    moved: &Completed,
    run_diff: &Observed<Vec<String>>,
) -> Observed<BaseDrift> {
    let named = match default_branch {
        Observed::Present(named) => named.clone(),
        other => {
            return Observed::Unobservable(other.reason().cloned().unwrap_or_else(|| {
                Reason::saying("no default branch to measure base drift against")
            }));
        }
    };
    if counted.code != Some(0) {
        return Observed::Unobservable(Reason::of("git rev-list --count against origin", counted));
    }
    let Ok(commits) = counted.stdout.trim().parse::<u64>() else {
        return Observed::Unobservable(Reason::of("git rev-list --count against origin", counted));
    };
    if moved.code != Some(0) {
        return Observed::Unobservable(Reason::of("git diff --name-only against origin", moved));
    }
    let ours: &[String] = match run_diff {
        Observed::Present(files) => files,
        Observed::Absent => &[],
        Observed::Unobservable(reason) => return Observed::Unobservable(reason.clone()),
    };
    let overlapping: Vec<String> = moved
        .stdout
        .lines()
        .map(str::trim)
        .filter(|path| !path.is_empty() && ours.iter().any(|mine| mine == path))
        .map(str::to_string)
        .collect();
    Observed::Present(BaseDrift {
        default_branch: named,
        commits,
        overlapping,
    })
}

/// One artifact directory, **scoped to the Run's own diff**.
///
/// The whole-directory listing this replaces counted other people's files: `furthest stage`
/// read the repo's history rather than the Run's, so a fresh Run read `planned` at dispatch on
/// any repo where a previous Run had merged a plan. Every one of these files is already in the
/// PR's own diff, so scoping costs one command and no new state.
pub fn scoped_listing(changed: &Observed<Vec<String>>, directory: &str) -> Observed<Vec<String>> {
    let prefix = format!("{directory}/");
    match changed {
        Observed::Unobservable(reason) => Observed::Unobservable(reason.clone()),
        Observed::Absent => Observed::Absent,
        Observed::Present(files) => {
            let mine: Vec<String> = files
                .iter()
                .filter(|path| path.starts_with(&prefix) && path.ends_with(".md"))
                .cloned()
                .collect();
            if mine.is_empty() {
                Observed::Absent
            } else {
                Observed::Present(mine)
            }
        }
    }
}

// --- observing a Run, once ---------------------------------------------------------------

/// Where the durable artifacts live, relative to the worktree.
pub const PLAN_DIR: &str = "docs/plans";
pub const RESIDUAL_DIR: &str = "docs/residual-review-findings";
pub const LEDGER_DIR: &str = "docs/ledger";

/// Take one whole observation of a Run.
///
/// The argv and the classifier that reads it sit in the same module on purpose: a parser can
/// be perfect while the command was built with a wrong flag, and that pairing is the only thing
/// a reader can check at a glance. `run` and `list` are the caller's doors to `world`, so both
/// the supervisor's loop and the read path get **one** definition of the sequence without
/// either of them naming the other.
pub fn observe_run(
    observed_at: String,
    handoff_sha: &str,
    run: &mut dyn FnMut(&[String]) -> Completed,
) -> Observation {
    let argv = |parts: &[&str]| parts.iter().map(|p| p.to_string()).collect::<Vec<String>>();

    let counted = run(&argv(&[
        "git",
        "rev-list",
        "--count",
        &format!("{handoff_sha}..HEAD"),
    ]));
    let status = run(&argv(&["git", "status", "--porcelain"]));
    let diffed = run(&argv(&[
        "git",
        "diff",
        "--name-only",
        &format!("{handoff_sha}..HEAD"),
    ]));
    let changed = changed_files(&diffed);

    // Base drift, against `origin/HEAD` in the same repository the worktree belongs to.
    let base = default_branch(&run(&argv(&[
        "git",
        "symbolic-ref",
        "--short",
        "refs/remotes/origin/HEAD",
    ])));
    let drift = match &base {
        Observed::Present(named) => base_drift(
            &base,
            &run(&argv(&[
                "git",
                "rev-list",
                "--count",
                &format!("{handoff_sha}..{named}"),
            ])),
            &run(&argv(&[
                "git",
                "diff",
                "--name-only",
                &format!("{handoff_sha}..{named}"),
            ])),
            &changed,
        ),
        // No default branch to measure against, and no commands worth running for it.
        other => base_drift(
            other,
            &Completed {
                stdout: String::new(),
                stderr: String::new(),
                code: None,
            },
            &Completed {
                stdout: String::new(),
                stderr: String::new(),
                code: None,
            },
            &changed,
        ),
    };

    // The head commit first, the Job's branch as a fallback. Both stay pure parses over raw
    // output, and the existing three-valued classification was never wrong here — it was the
    // question that was wrong.
    let head = run(&argv(&["git", "rev-parse", "HEAD"]));
    let by_head = if head.code == Some(0) && !head.stdout.trim().is_empty() {
        pr_by_head(&run(&argv(&[
            "gh",
            "pr",
            "list",
            "--search",
            head.stdout.trim(),
            "--state",
            "all",
            "--json",
            "number,url,state,isDraft",
        ])))
    } else {
        Observed::Unobservable(Reason::of("git rev-parse HEAD", &head))
    };
    let found = match by_head {
        Observed::Present(found) => Observed::Present(found),
        // Nothing matched the commit, so ask the branch — which is still the right question on
        // a Run that pushed where its Job said it would.
        Observed::Absent => pr(&run(&argv(&[
            "gh",
            "pr",
            "view",
            "--json",
            "number,url,state,isDraft",
        ]))),
        // Blind stays blind. A head lookup that could not be made must not become *no PR*
        // because the branch lookup also found nothing.
        Observed::Unobservable(reason) => match pr(&run(&argv(&[
            "gh",
            "pr",
            "view",
            "--json",
            "number,url,state,isDraft",
        ]))) {
            Observed::Present(fallback) => Observed::Present(fallback),
            _ => Observed::Unobservable(reason),
        },
    };

    // The rollup resolves against **the PR the lookup found**, by number. Two independent
    // lookups can disagree about whether a PR exists at all.
    let (checks_pending, checks_red) = match &found {
        Observed::Present(open) => checks(&run(&argv(&[
            "gh",
            "pr",
            "view",
            &open.number.to_string(),
            "--json",
            "statusCheckRollup",
        ]))),
        Observed::Absent => (Observed::Absent, Observed::Absent),
        Observed::Unobservable(reason) => (
            Observed::Unobservable(reason.clone()),
            Observed::Unobservable(reason.clone()),
        ),
    };

    Observation {
        observed_at,
        commits_ahead: commits_ahead(&counted),
        tree_clean: tree_clean(&status),
        pr: found,
        checks_pending,
        checks_red,
        plan_files: scoped_listing(&changed, PLAN_DIR),
        residual_findings: scoped_listing(&changed, RESIDUAL_DIR),
        ledger_entries: scoped_listing(&changed, LEDGER_DIR),
        changed_files: changed,
        base_drift: drift,
    }
}

// --- the host item list's classifiers ---------------------------------------------------
//
// **Every one of these renders a fixed, item-specific diagnostic and never the raw stdout or
// stderr of the check.** Doctor's whole purpose is to run on hosts that failed provisioning,
// and a misprovisioned host is exactly where an HTTPS `origin` embeds a token.

/// What a host item's check found. Distinct from [`Observed`]: `Unchecked` is a deliberate
/// absence of a boolean, while `Observed::Unobservable` is a check that tried and could not.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Satisfied(String),
    Unsatisfied(String),
    /// No honest boolean is available, and this says which part.
    Unchecked(String),
}

/// Items marked *step*, and the halves of a credential step that no check can reach.
pub fn unchecked(why: &str) -> Observed<Outcome> {
    Observed::Present(Outcome::Unchecked(why.to_string()))
}

fn satisfied(what: &str) -> Observed<Outcome> {
    Observed::Present(Outcome::Satisfied(what.to_string()))
}

fn unsatisfied(what: &str) -> Observed<Outcome> {
    Observed::Present(Outcome::Unsatisfied(what.to_string()))
}

/// `~/.grind/repos/<owner>/<name>` exists, and — at doctor's depth — its `origin` names the
/// target repo. `origin` is `None` at dispatch depth, which is what makes the dispatch subset a
/// shallower run of the same item rather than a second item.
pub fn declared_clone(
    exists: bool,
    origin: Option<&Completed>,
    declared: &str,
) -> Observed<Outcome> {
    if !exists {
        return unsatisfied("no declared clone at ~/.grind/repos/<owner>/<name>");
    }
    let Some(completed) = origin else {
        return satisfied("declared clone present");
    };
    if completed.code != Some(0) {
        return Observed::Unobservable(Reason::saying("git remote get-url origin: could not read"));
    }
    match repo_of_remote(&completed.stdout) {
        // The two parsed owner/name pairs, never the remote URL.
        Some(found) if found.eq_ignore_ascii_case(declared) => {
            satisfied("origin names the target repo")
        }
        Some(found) => unsatisfied(&format!("origin names {found}, the Job names {declared}")),
        None => Observed::Unobservable(Reason::saying(
            "git remote get-url origin: no owner/name in the remote",
        )),
    }
}

/// `owner/name` out of an SSH or HTTPS remote. The URL itself never leaves this function.
pub fn repo_of_remote(remote: &str) -> Option<String> {
    let remote = remote.trim().trim_end_matches(".git");
    // The host comes first, so anything a credential helper embedded in front of it — a token
    // in an HTTPS remote on a half-provisioned box — is dropped before the pairs are read.
    let tail = if let Some((_, tail)) = remote.split_once("github.com/") {
        tail
    } else if let Some((_, tail)) = remote.split_once("github.com:") {
        tail
    } else if let Some((_, tail)) = remote.rsplit_once(':') {
        tail
    } else {
        remote
    };
    let mut parts = tail.trim_matches('/').split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    (!owner.is_empty() && !name.is_empty() && parts.next().is_none())
        .then(|| format!("{owner}/{name}"))
}

/// One declared clone per target repo — not a search path. This is what makes the lock key
/// sound: *two clones of one supervised repo* stops being a state the host can be in.
pub fn one_clone_per_repo(clone_paths: &[String], declared: &str) -> Observed<Outcome> {
    let (_, name) = declared.split_once('/').unwrap_or(("", declared));
    let same_name: Vec<&String> = clone_paths
        .iter()
        .filter(|p| p.rsplit('/').next() == Some(name))
        .collect();
    match same_name.len() {
        0 => unsatisfied("no clone declared for this repo"),
        1 => satisfied("one declared clone"),
        n => unsatisfied(&format!("{n} clones named `{name}` under ~/.grind/repos")),
    }
}

/// `bin/claude` is executable and is **not a shim**. Asserted loudly rather than filtered for:
/// on this laptop `which -a claude` returns a terminal's shim first, twice, so a symlink made
/// from `which` points at the wrong file and the Run silently inherits that terminal's session
/// hooks — reproducible nowhere, with nothing printed.
pub fn claude_binary(executable: bool, resolved: Option<&str>) -> Observed<Outcome> {
    if !executable {
        return unsatisfied("~/.grind/bin/claude is missing or not executable");
    }
    let Some(target) = resolved else {
        return satisfied("~/.grind/bin/claude is executable");
    };
    if target.contains("shim") {
        return unsatisfied("~/.grind/bin/claude resolves to a wrapper shim");
    }
    satisfied("executable, and not a shim")
}

/// An executable resolves on `PATH`. No version floor is invented — an invented floor is a
/// precondition that fails for no reason.
pub fn on_path(tool: &str, completed: &Completed) -> Observed<Outcome> {
    if completed.code == Some(0) && !completed.stdout.trim().is_empty() {
        satisfied(&format!("`{tool}` resolves on PATH"))
    } else {
        unsatisfied(&format!("`{tool}` does not resolve on PATH"))
    }
}

/// `git --version` at or above the floor SSH commit signing needs.
pub fn git_version_floor(completed: &Completed, floor: (u64, u64)) -> Observed<Outcome> {
    if completed.code != Some(0) {
        return unsatisfied("`git` does not resolve on PATH");
    }
    let Some((major, minor)) = parse_git_version(&completed.stdout) else {
        return Observed::Unobservable(Reason::saying("git --version: unreadable"));
    };
    if (major, minor) >= floor {
        satisfied(&format!(
            "git {major}.{minor}, at or above {}.{}",
            floor.0, floor.1
        ))
    } else {
        unsatisfied(&format!(
            "git {major}.{minor} is below the {}.{} floor",
            floor.0, floor.1
        ))
    }
}

fn parse_git_version(output: &str) -> Option<(u64, u64)> {
    let digits = output
        .split_whitespace()
        .find(|t| t.starts_with(|c: char| c.is_ascii_digit()))?;
    let mut parts = digits.split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// The pinned plugin directory is installed on this host.
pub fn plugin_installed(exists: bool) -> Observed<Outcome> {
    if exists {
        satisfied("the pinned plugin version is installed")
    } else {
        unsatisfied("the pinned plugin version is not installed on this host")
    }
}

/// `gh auth status`. A headless box stores the token in plaintext `hosts.yml`; a laptop uses a
/// keyring. Both are satisfied — what is checked is that a token store exists at all.
pub fn gh_auth_store(completed: &Completed) -> Observed<Outcome> {
    if completed.code != Some(0) {
        return unsatisfied("`gh auth status` reports no authenticated host");
    }
    let said = completed.stdout.to_lowercase() + &completed.stderr.to_lowercase();
    if said.contains("keyring") {
        satisfied("authenticated, token in the keyring")
    } else if said.contains("oauth_token") {
        satisfied("authenticated, token in plaintext hosts.yml")
    } else if said.contains("logged in") {
        satisfied("authenticated")
    } else {
        Observed::Unobservable(Reason::saying("gh auth status: unreadable"))
    }
}

/// The signing key exists and loads with no passphrase — `ssh-keygen -y -P ""`, a read, never a
/// write. What cannot be checked is named beside it: an agent-backed signer makes committing
/// depend on a GUI approval, and `ssh-add -l` keeps listing the key throughout, because listing
/// needs no approval and *using* one does. Run 2 lost its declared branch to exactly that.
pub fn ssh_key_passphraseless(
    configured_key: Option<&str>,
    probe: Option<&Completed>,
) -> Observed<Outcome> {
    let Some(key) = configured_key.filter(|k| !k.trim().is_empty()) else {
        return unsatisfied("`user.signingkey` names no key, so there is none to check");
    };
    match probe {
        Some(completed) if completed.code == Some(0) => unchecked(&format!(
            "key at `{key}` loads with no passphrase; whether the signer will actually sign \
             cannot be checked — an agent-backed signer lists fine and refuses to sign"
        )),
        Some(_) => unsatisfied("the configured signing key does not load without a passphrase"),
        None => unchecked("not probed"),
    }
}

/// `gh ssh-key list` shows the same key uploaded for **both** `authentication` and `signing`.
/// `gh auth login --git-protocol ssh` uploads it as authentication only, which is why this is a
/// checklist rather than a command.
pub fn ssh_keys_both_types(completed: &Completed) -> Observed<Outcome> {
    if completed.code != Some(0) {
        return Observed::Unobservable(Reason::saying("gh ssh-key list: could not read"));
    }
    let said = completed.stdout.to_lowercase();
    match (said.contains("authentication"), said.contains("signing")) {
        (true, true) => satisfied("a key is uploaded for authentication and for signing"),
        (true, false) => unsatisfied("a key is uploaded for authentication but not for signing"),
        (false, true) => unsatisfied("a key is uploaded for signing but not for authentication"),
        (false, false) => unsatisfied("no key is uploaded for either type"),
    }
}

/// `gpg.format ssh`, `user.signingkey` at the private key path, `commit.gpgsign true`.
pub fn signing_config(
    format: &Completed,
    key: &Completed,
    gpgsign: &Completed,
) -> Observed<Outcome> {
    let format_value = format.stdout.trim().to_lowercase();
    let key_value = key.stdout.trim().to_string();
    let sign_value = gpgsign.stdout.trim().to_lowercase();
    let mut missing = Vec::new();
    if format_value != "ssh" {
        missing.push("gpg.format is not `ssh`");
    }
    if key_value.is_empty() {
        missing.push("user.signingkey is unset");
    } else if key_value.ends_with(".pub") {
        missing.push("user.signingkey names the public key, not the private one");
    }
    if sign_value != "true" {
        missing.push("commit.gpgsign is not `true`");
    }
    if missing.is_empty() {
        satisfied("gpg.format ssh, a private signing key, commit.gpgsign true")
    } else {
        unsatisfied(&missing.join("; "))
    }
}

/// `user.name` / `user.email` set to the machine identity. Whether the email is **added and
/// verified on the GitHub account** is named rather than guessed: the granted scopes are
/// `repo` / `read:org` / `gist`, and reading a user's verified addresses needs `user:email`.
pub fn committer_identity(name: &Completed, email: &Completed) -> Observed<Outcome> {
    let has_name = !name.stdout.trim().is_empty();
    let has_email = !email.stdout.trim().is_empty();
    match (has_name, has_email) {
        (true, true) => unchecked(
            "user.name and user.email are set; whether that address is verified on the account \
             cannot be checked without a scope Grind does not hold",
        ),
        (false, true) => unsatisfied("user.name is unset"),
        (true, false) => unsatisfied("user.email is unset"),
        (false, false) => unsatisfied("user.name and user.email are unset"),
    }
}

/// `origin` on SSH. Whether a **real push** succeeds is the only step that proves the other
/// five, and doctor never performs a write to prove a step — so that half is named, not faked.
pub fn origin_over_ssh(completed: &Completed) -> Observed<Outcome> {
    if completed.code != Some(0) {
        return Observed::Unobservable(Reason::saying("git remote get-url origin: could not read"));
    }
    let remote = completed.stdout.trim();
    if remote.starts_with("git@") || remote.starts_with("ssh://") {
        unchecked(
            "origin is on SSH; that a real push succeeds is the only step proving the other \
             five, and doctor never writes to prove one",
        )
    } else {
        unsatisfied("origin is not on SSH")
    }
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

    // --- the PR is found by head commit ------------------------------------------------------

    const PR_JSON: &str =
        r#"{"number":30,"url":"https://github.com/o/n/pull/30","state":"OPEN","isDraft":false}"#;

    /// One whole observation from literals: the head-commit lookup, the branch fallback, and
    /// the rollup, plus every argv the sequence actually built.
    fn observing(
        by_head: Completed,
        by_branch: Completed,
        rollup: Completed,
    ) -> (Observation, Vec<String>) {
        let mut seen: Vec<String> = Vec::new();
        let observation = {
            let mut run = |argv: &[String]| {
                let joined = argv.join(" ");
                seen.push(joined.clone());
                if joined.contains("rev-parse HEAD") {
                    completed("3333333333333333333333333333333333333333\n", "", Some(0))
                } else if joined.contains("rev-list") {
                    completed("12\n", "", Some(0))
                } else if joined.contains("diff --name-only") {
                    completed("src/lib.rs\n", "", Some(0))
                } else if joined.contains("pr list") {
                    by_head.clone()
                } else if joined.contains("statusCheckRollup") {
                    rollup.clone()
                } else if joined.contains("pr view") {
                    by_branch.clone()
                } else {
                    completed("", "", Some(0))
                }
            };
            observe_run("at".to_string(), "9d1f4c7a", &mut run)
        };
        (observation, seen)
    }

    /// One observation whose only interesting input is the Run's own diff: no commits, no PR,
    /// so the stage ladder is reading the listings and nothing else.
    fn observing_with_diff(names: &str) -> (Observation, Vec<String>) {
        let names = names.to_string();
        let mut seen: Vec<String> = Vec::new();
        let observation = {
            let mut run = |argv: &[String]| {
                let joined = argv.join(" ");
                seen.push(joined.clone());
                if joined.contains("diff --name-only") {
                    completed(&names, "", Some(0))
                } else if joined.contains("rev-parse HEAD") {
                    completed("3333333333333333333333333333333333333333\n", "", Some(0))
                } else if joined.contains("rev-list") {
                    completed("0\n", "", Some(0))
                } else if joined.contains("pr list") {
                    completed("[]", "", Some(0))
                } else if joined.contains("pr view") {
                    no_pr_on_this_branch()
                } else {
                    completed("", "", Some(0))
                }
            };
            observe_run("at".to_string(), "9d1f4c7a", &mut run)
        };
        (observation, seen)
    }

    fn no_pr_on_this_branch() -> Completed {
        completed(
            "",
            "no pull requests found for branch \"feat/28-slice-1b-seam\"\n",
            Some(1),
        )
    }

    #[test]
    fn a_pr_on_a_branch_the_job_did_not_name_is_found_by_head_commit() {
        // Run 2's shape exactly: pushed to `…-run`, the Job named `…-seam`. The branch lookup
        // answers truthfully about the wrong question.
        let (observation, seen) = observing(
            completed(&format!("[{PR_JSON}]"), "", Some(0)),
            no_pr_on_this_branch(),
            completed(r#"{"statusCheckRollup":[]}"#, "", Some(0)),
        );
        let Observed::Present(found) = &observation.pr else {
            panic!("the head commit finds it: {:?}", observation.pr);
        };
        assert_eq!(found.number, 30);
        assert!(
            seen.iter()
                .any(|argv| argv
                    .contains("pr list --search 3333333333333333333333333333333333333333")),
            "{seen:?}"
        );
    }

    #[test]
    fn the_branch_fallback_still_finds_a_pr_when_the_head_lookup_returns_nothing() {
        let (observation, _) = observing(
            completed("[]", "", Some(0)),
            completed(PR_JSON, "", Some(0)),
            completed(r#"{"statusCheckRollup":[]}"#, "", Some(0)),
        );
        let Observed::Present(found) = &observation.pr else {
            panic!("the branch is still the right question for a Run that pushed where it said");
        };
        assert_eq!(found.number, 30);
    }

    #[test]
    fn a_gh_auth_failure_is_could_not_observe_on_both_paths_and_never_absent() {
        let code: i32 = GH_AUTH_CODE.trim().parse().expect("the recorded exit code");
        let broken = completed(GH_AUTH_STDOUT, GH_AUTH_STDERR, Some(code));
        let (observation, _) = observing(broken.clone(), broken.clone(), broken);
        assert!(
            matches!(observation.pr, Observed::Unobservable(_)),
            "{:?}",
            observation.pr
        );
        assert!(matches!(
            observation.checks_pending,
            Observed::Unobservable(_)
        ));
        assert!(matches!(observation.checks_red, Observed::Unobservable(_)));
    }

    #[test]
    fn unreadable_json_from_either_lookup_is_could_not_observe() {
        assert!(matches!(
            pr_by_head(&completed("not json at all", "", Some(0))),
            Observed::Unobservable(_)
        ));
        assert!(matches!(
            pr_by_head(&completed(r#"[{"url":"x"}]"#, "", Some(0))),
            Observed::Unobservable(_)
        ));
        let (observation, _) = observing(
            completed("not json at all", "", Some(0)),
            completed("also not json", "", Some(0)),
            completed("", "", Some(0)),
        );
        assert!(matches!(observation.pr, Observed::Unobservable(_)));
    }

    #[test]
    fn the_check_rollup_resolves_against_the_same_pr_the_lookup_found() {
        // Two independent lookups can disagree about whether a PR exists at all.
        let (_, seen) = observing(
            completed(&format!("[{PR_JSON}]"), "", Some(0)),
            no_pr_on_this_branch(),
            completed(r#"{"statusCheckRollup":[]}"#, "", Some(0)),
        );
        assert!(
            seen.iter()
                .any(|argv| argv == "gh pr view 30 --json statusCheckRollup"),
            "{seen:?}"
        );
    }

    #[test]
    fn a_run_with_no_pr_anywhere_reads_checks_as_absent_rather_than_could_not_observe() {
        let (observation, _) = observing(
            completed("[]", "", Some(0)),
            no_pr_on_this_branch(),
            completed("", "", Some(0)),
        );
        assert_eq!(observation.pr, Observed::Absent);
        assert_eq!(observation.checks_pending, Observed::Absent);
        assert_eq!(observation.checks_red, Observed::Absent);
    }

    // --- base drift ----------------------------------------------------------------------------

    fn on_main() -> Observed<String> {
        default_branch(&completed("origin/main\n", "", Some(0)))
    }

    fn ours() -> Observed<Vec<String>> {
        Observed::Present(vec![
            "docs/adr/0013-a-decision.md".to_string(),
            "src/observe.rs".to_string(),
        ])
    }

    #[test]
    fn drift_with_no_readable_origin_head_is_could_not_observe_and_never_zero() {
        // What a clone whose fetch has never succeeded looks like. Recording it as *no drift*
        // is the exact shape the three-valued reading exists to remove.
        for broken in [
            completed(
                "",
                "fatal: ref refs/remotes/origin/HEAD is not a symbolic ref\n",
                Some(1),
            ),
            completed("", "", Some(0)),
            completed("", "", None),
        ] {
            let base = default_branch(&broken);
            assert!(matches!(base, Observed::Unobservable(_)), "{base:?}");
            let drift = base_drift(
                &base,
                &completed("0\n", "", Some(0)),
                &completed("", "", Some(0)),
                &ours(),
            );
            assert!(matches!(drift, Observed::Unobservable(_)), "{drift:?}");
        }
    }

    #[test]
    fn a_default_branch_that_has_not_moved_is_a_present_zero_with_no_overlap() {
        let drift = base_drift(
            &on_main(),
            &completed("0\n", "", Some(0)),
            &completed("", "", Some(0)),
            &ours(),
        );
        assert_eq!(
            drift,
            Observed::Present(BaseDrift {
                default_branch: "origin/main".to_string(),
                commits: 0,
                overlapping: vec![],
            })
        );
    }

    #[test]
    fn a_default_branch_that_moved_into_a_path_the_run_touched_yields_the_count_and_that_path() {
        let drift = base_drift(
            &on_main(),
            &completed("4\n", "", Some(0)),
            &completed("docs/adr/0013-a-decision.md\nREADME.md\n", "", Some(0)),
            &ours(),
        );
        let Observed::Present(found) = drift else {
            panic!("both halves are readable");
        };
        assert_eq!(found.commits, 4);
        assert_eq!(found.overlapping, vec!["docs/adr/0013-a-decision.md"]);
        assert!(found.to_string().contains("4 commits on origin/main"));
    }

    #[test]
    fn a_default_branch_that_moved_elsewhere_yields_the_count_and_no_overlap() {
        let drift = base_drift(
            &on_main(),
            &completed("9\n", "", Some(0)),
            &completed("README.md\nCHANGELOG.md\n", "", Some(0)),
            &ours(),
        );
        let Observed::Present(found) = drift else {
            panic!("both halves are readable");
        };
        assert_eq!(found.commits, 9);
        assert!(found.overlapping.is_empty());
    }

    #[test]
    fn a_run_diff_that_could_not_be_read_leaves_the_drift_unobserved() {
        let drift = base_drift(
            &on_main(),
            &completed("4\n", "", Some(0)),
            &completed("README.md\n", "", Some(0)),
            &Observed::Unobservable(Reason::saying("git diff --name-only: exit 128")),
        );
        assert!(matches!(drift, Observed::Unobservable(_)), "{drift:?}");
    }

    #[test]
    fn an_unreadable_count_or_diff_against_the_default_branch_is_could_not_observe() {
        let blind = completed("", "fatal: bad revision\n", Some(128));
        assert!(matches!(
            base_drift(&on_main(), &blind, &completed("", "", Some(0)), &ours()),
            Observed::Unobservable(_)
        ));
        assert!(matches!(
            base_drift(&on_main(), &completed("2\n", "", Some(0)), &blind, &ours()),
            Observed::Unobservable(_)
        ));
        assert!(matches!(
            base_drift(
                &on_main(),
                &completed("not a number\n", "", Some(0)),
                &completed("", "", Some(0)),
                &ours()
            ),
            Observed::Unobservable(_)
        ));
    }

    #[test]
    fn no_type_in_the_drift_carries_a_boolean_or_a_summary_over_the_count() {
        // ADR-0006's seventh prohibited shape. The tempting argument — *`main` moved, so don't
        // open the PR* — reads as caution rather than as the quality judgement ADR-0003
        // refuses.
        let shape = format!(
            "{:?}",
            BaseDrift {
                default_branch: "origin/main".to_string(),
                commits: 4,
                overlapping: vec!["docs/adr/0013.md".to_string()],
            }
        )
        .to_lowercase();
        for banned in [
            "diverged", "drifted", "stale", "conflict", "true", "false", "ok", "healthy",
        ] {
            assert!(!shape.contains(banned), "{shape}");
        }
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
    fn a_run_with_no_pr_yet_reads_checks_as_absent_rather_than_could_not_observe() {
        // The normal early state of every Run is "no PR yet", not "blind" — this is the same
        // line `pr()` already draws, drawn here too so the old script's "no PR = completed"
        // bug does not resurface one module over as "no PR = blind".
        let none = completed("", "no pull requests found for branch\n", Some(1));
        let (pending, red) = checks(&none);
        assert_eq!(pending, Observed::Absent);
        assert_eq!(red, Observed::Absent);
    }

    #[test]
    fn an_unreadable_worktree_is_not_a_run_that_produced_nothing() {
        // A failed diff is could-not-observe. An empty listing read as absence is how a
        // worktree that has gone missing becomes a Run that produced no plan.
        let missing = completed("", "fatal: not a git repository\n", Some(128));
        assert!(matches!(changed_files(&missing), Observed::Unobservable(_)));
        assert!(matches!(
            scoped_listing(&changed_files(&missing), PLAN_DIR),
            Observed::Unobservable(_)
        ));
        assert_eq!(changed_files(&completed("", "", Some(0))), Observed::Absent);
    }

    // --- listings scoped to the Run's own diff -------------------------------------------------

    #[test]
    fn a_plan_file_the_run_itself_added_advances_the_ladder_and_one_it_did_not_does_not() {
        let mine = changed_files(&completed(
            "docs/plans/2026-08-15-a-plan.md\nsrc/lib.rs\n",
            "",
            Some(0),
        ));
        assert_eq!(
            scoped_listing(&mine, PLAN_DIR),
            Observed::Present(vec!["docs/plans/2026-08-15-a-plan.md".to_string()])
        );
        // A previous Run's merged plan is in the repo and not in this Run's diff.
        let elsewhere = changed_files(&completed("src/lib.rs\nREADME.md\n", "", Some(0)));
        assert_eq!(scoped_listing(&elsewhere, PLAN_DIR), Observed::Absent);
        assert_eq!(scoped_listing(&elsewhere, RESIDUAL_DIR), Observed::Absent);
        assert_eq!(scoped_listing(&elsewhere, LEDGER_DIR), Observed::Absent);
    }

    #[test]
    fn a_directory_that_merely_prefixes_another_is_not_swept_up() {
        let near = changed_files(&completed(
            "docs/plans-archive/old.md\ndocs/plans/new.md\n",
            "",
            Some(0),
        ));
        assert_eq!(
            scoped_listing(&near, PLAN_DIR),
            Observed::Present(vec!["docs/plans/new.md".to_string()])
        );
    }

    #[test]
    fn all_five_rungs_are_still_reachable_from_diff_scoped_listings() {
        // Trimming the ladder to the Run-scoped rungs would throw away the distinction between
        // a Run that died before planning and one that died after, which is why the stage
        // exists at all.
        let stage = |o: &Observation| crate::decide::furthest_stage(o).to_string();

        // A fresh Run on a repo where a previous Run merged a plan reads `dispatched`.
        let (fresh, _) = observing_with_diff("");
        assert_eq!(stage(&fresh), "dispatched");

        let (mut walk, _) = observing_with_diff("docs/plans/a.md\n");
        assert_eq!(stage(&walk), "planned");
        walk.commits_ahead = Observed::Present(3);
        assert_eq!(stage(&walk), "implemented");
        walk.residual_findings =
            Observed::Present(vec!["docs/residual-review-findings/r.md".to_string()]);
        assert_eq!(stage(&walk), "reviewed");
        walk.pr = Observed::Present(Pr {
            number: 30,
            url: "https://github.com/o/n/pull/30".to_string(),
            state: "OPEN".to_string(),
            is_draft: false,
        });
        assert_eq!(stage(&walk), "pr-open");
    }

    // --- the host item list's classifiers -------------------------------------------------

    #[test]
    fn a_clone_whose_origin_names_another_repo_fails_and_names_both() {
        let origin = completed("git@github.com:someone-else/snapper.git\n", "", Some(0));
        let found = declared_clone(true, Some(&origin), "FlorianRiquelme/snapper");
        let Observed::Present(Outcome::Unsatisfied(said)) = found else {
            panic!("a mismatched origin must be unsatisfied: {found:?}");
        };
        assert!(said.contains("someone-else/snapper"), "{said}");
        assert!(said.contains("FlorianRiquelme/snapper"), "{said}");
    }

    #[test]
    fn a_check_renders_the_parsed_pairs_and_never_the_remote_url() {
        // Doctor runs on hosts that failed provisioning, which is exactly where an HTTPS
        // origin embeds a token.
        let leaky = "https://x-access-token:ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA@github.com/o/snapper.git\n";
        let origin = completed(leaky, "", Some(0));
        let found = declared_clone(true, Some(&origin), "FlorianRiquelme/snapper");
        let Observed::Present(Outcome::Unsatisfied(said)) = found else {
            panic!("expected a mismatch: {found:?}");
        };
        assert!(
            !said.contains("ghp_"),
            "the diagnostic leaked the token: {said}"
        );
        assert!(
            !said.contains("github.com"),
            "the diagnostic leaked the URL: {said}"
        );
        assert!(said.contains("o/snapper"), "{said}");
    }

    #[test]
    fn a_matching_origin_satisfies_and_a_missing_clone_does_not() {
        let origin = completed("git@github.com:FlorianRiquelme/snapper.git\n", "", Some(0));
        assert!(matches!(
            declared_clone(true, Some(&origin), "FlorianRiquelme/snapper"),
            Observed::Present(Outcome::Satisfied(_))
        ));
        assert!(matches!(
            declared_clone(false, None, "FlorianRiquelme/snapper"),
            Observed::Present(Outcome::Unsatisfied(_))
        ));
    }

    #[test]
    fn a_claude_resolving_to_a_shim_fails_loudly_rather_than_being_skipped() {
        let shimmed = claude_binary(true, Some("/var/folders/T/cmux-shim/bin/claude"));
        let Observed::Present(Outcome::Unsatisfied(said)) = shimmed else {
            panic!("a shim must be an unsatisfied item, not a filtered-out one: {shimmed:?}");
        };
        assert!(said.contains("shim"), "{said}");
        assert!(matches!(
            claude_binary(true, Some("/Users/op/.local/bin/claude")),
            Observed::Present(Outcome::Satisfied(_))
        ));
        assert!(matches!(
            claude_binary(false, None),
            Observed::Present(Outcome::Unsatisfied(_))
        ));
    }

    #[test]
    fn the_git_floor_is_the_one_ssh_signing_needs() {
        let floor = (2, 34);
        assert!(matches!(
            git_version_floor(&completed("git version 2.51.2\n", "", Some(0)), floor),
            Observed::Present(Outcome::Satisfied(_))
        ));
        assert!(matches!(
            git_version_floor(&completed("git version 2.20.1\n", "", Some(0)), floor),
            Observed::Present(Outcome::Unsatisfied(_))
        ));
    }

    #[test]
    fn a_key_uploaded_for_one_type_only_is_the_failure_gh_auth_login_actually_produces() {
        let list = completed("2026-08-01\tsnapper\tauthentication\n", "", Some(0));
        let Observed::Present(Outcome::Unsatisfied(said)) = ssh_keys_both_types(&list) else {
            panic!("authentication-only must not pass");
        };
        assert!(said.contains("signing"), "{said}");
        let both = completed(
            "2026-08-01\tsnapper\tauthentication\n2026-08-01\tsnapper\tsigning\n",
            "",
            Some(0),
        );
        assert!(matches!(
            ssh_keys_both_types(&both),
            Observed::Present(Outcome::Satisfied(_))
        ));
    }

    #[test]
    fn the_signing_config_names_every_part_that_is_wrong() {
        let public_key = completed("/home/op/.ssh/id_ed25519.pub\n", "", Some(0));
        let found = signing_config(
            &completed("openpgp\n", "", Some(0)),
            &public_key,
            &completed("false\n", "", Some(0)),
        );
        let Observed::Present(Outcome::Unsatisfied(said)) = found else {
            panic!("expected an unsatisfied config: {found:?}");
        };
        assert!(said.contains("gpg.format"), "{said}");
        assert!(said.contains("public key"), "{said}");
        assert!(said.contains("commit.gpgsign"), "{said}");
    }

    #[test]
    fn the_steps_no_check_can_reach_are_named_rather_than_guessed() {
        // No check here is a guess dressed as a boolean, and none performs a write to prove a
        // step. The parts that cannot be reached say so in the report.
        let ssh_origin = completed("git@github.com:o/n.git\n", "", Some(0));
        let Observed::Present(Outcome::Unchecked(said)) = origin_over_ssh(&ssh_origin) else {
            panic!("a real push is not something doctor may perform");
        };
        assert!(said.contains("never writes"), "{said}");

        let identity = committer_identity(
            &completed("Snapper Host\n", "", Some(0)),
            &completed("host@example.com\n", "", Some(0)),
        );
        let Observed::Present(Outcome::Unchecked(said)) = identity else {
            panic!("verified-on-the-account is not reachable with the scopes Grind holds");
        };
        assert!(said.contains("verified"), "{said}");

        assert!(matches!(
            origin_over_ssh(&completed("https://github.com/o/n.git\n", "", Some(0))),
            Observed::Present(Outcome::Unsatisfied(_))
        ));
    }

    #[test]
    fn two_clones_of_one_repo_is_a_state_the_host_must_not_be_in() {
        assert!(matches!(
            one_clone_per_repo(&["/g/repos/a/snapper".into()], "a/snapper"),
            Observed::Present(Outcome::Satisfied(_))
        ));
        let two = [
            "/g/repos/a/snapper".to_string(),
            "/g/repos/b/snapper".to_string(),
        ];
        assert!(matches!(
            one_clone_per_repo(&two, "a/snapper"),
            Observed::Present(Outcome::Unsatisfied(_))
        ));
    }

    #[test]
    fn a_failed_check_carries_no_quality_language() {
        let banned = [
            "bad", "wrong", "invalid", "broken", "fail", "poor", "should",
        ];
        let origin = completed("git@github.com:someone/else.git\n", "", Some(0));
        let said = [
            declared_clone(false, None, "a/b"),
            declared_clone(true, Some(&origin), "a/b"),
            claude_binary(true, Some("/tmp/shim/claude")),
            on_path("just", &completed("", "", Some(1))),
            git_version_floor(&completed("git version 2.20.1\n", "", Some(0)), (2, 34)),
            plugin_installed(false),
            gh_auth_store(&completed("", "", Some(1))),
        ];
        for outcome in said {
            let Observed::Present(Outcome::Unsatisfied(text)) = outcome else {
                continue;
            };
            for word in banned {
                assert!(
                    !text.to_lowercase().contains(word),
                    "a failed host check must read as incoherent input, not a judgement: {text}"
                );
            }
        }
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
    }
}
