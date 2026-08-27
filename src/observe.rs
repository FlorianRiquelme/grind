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

use crate::decide::{ContentKind, DiffFacts, RiskyPathKind};
use crate::world::Completed;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::SystemTime;

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
    /// The branch the PR's head actually points at. Compared against the Job's `branch` row by
    /// [`pr_head_matches_job_branch`] — Run 2 pushed to `…-run` while its Job named `…-seam`,
    /// and nothing before this checked that the two agree.
    pub head_ref: String,
    /// The branch the PR opens against. Compared against the Job's `base_branch` row by
    /// [`pr_base_matches_declared`].
    pub base_ref: String,
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
    /// The fifth completion signal: the found PR's head ref against the Job's `branch` row.
    /// `Absent` when there is no PR yet, mirroring `pr_open`'s own fold — a Run that has not
    /// opened a PR is not a Run that is blind.
    pub pr_head_matches_job_branch: Observed<bool>,
    /// The sixth completion signal: the found PR's base ref against the Job's `base_branch` row.
    pub pr_base_matches_declared: Observed<bool>,
}

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

/// `gh pr view --json number,url,state,isDraft,headRefName,baseRefName`.
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

/// `gh pr list --search <head-sha> --state all --json
/// number,url,state,isDraft,headRefOid,headRefName,baseRefName`.
///
/// **A Run's identity on GitHub is the commit it pushed, not the branch its Job named.** Run 2
/// pushed to `…-run` while its Job named `…-seam`, so the branch lookup answered truthfully
/// about the wrong question and the Handback said `PR —` over an open, green, twelve-commit PR.
///
/// **The search index cannot be trusted to answer that identity question either** (#84):
/// against the live repo, `gh pr list --search <sha>` returned *empty* for an open PR whose
/// head was exactly that sha — GitHub's search lags or skips head SHAs of open PRs, which is
/// precisely the population Grind observes. So the query only narrows candidates and the match
/// is made here, over `headRefOid`, comparing case-insensitively because hex case is a
/// rendering choice (`git` prints lowercase, the API has answered mixed). A response whose
/// entries carry a different OID reads as `Absent`, which is what sends [`observe_run`] down
/// the branch fallback rather than what declares the PR gone.
///
/// An empty array — or no entry whose head OID equals the Run's — is `Absent`: the search ran
/// and matched nothing. Anything unreadable is could-not-observe, the direction that withholds
/// a verdict rather than inventing one.
pub fn pr_by_head(head_sha: &str, completed: &Completed) -> Observed<Pr> {
    if completed.code != Some(0) {
        return Observed::Unobservable(Reason::of("gh pr list --search", completed));
    }
    let body = completed.stdout.trim();
    let Ok(serde_json::Value::Array(matched)) = serde_json::from_str::<serde_json::Value>(body)
    else {
        return Observed::Unobservable(Reason::saying("gh pr list --search: unreadable JSON"));
    };
    let Some(first) = matched.into_iter().find(|candidate| {
        candidate
            .get("headRefOid")
            .and_then(|oid| oid.as_str())
            .is_some_and(|oid| oid.eq_ignore_ascii_case(head_sha))
    }) else {
        return Observed::Absent;
    };
    match parse_pr_value(&first) {
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
        head_ref: value
            .get("headRefName")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string(),
        base_ref: value
            .get("baseRefName")
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}

/// Whether the found PR's head ref is the branch the Job named. Trivial string equality — the
/// interesting part is that it is asked at all: Run 2 pushed to `…-run` while its Job named
/// `…-seam`, and nothing before this checked that the two agree. Never called on a PR that
/// could not be fetched; that direction is [`observe_run`]'s to fold, mirroring `pr_open`'s own
/// Absent/Unobservable handling.
pub fn pr_head_matches_job_branch(pr_head: &str, job_branch: &str) -> Observed<bool> {
    Observed::Present(pr_head == job_branch)
}

/// Whether the found PR's base ref is the branch the Job declared. Same shape as
/// [`pr_head_matches_job_branch`], compared against the Job's `base_branch` row.
///
/// **An undeclared base cannot mismatch.** A record serialized before the `Base branch` row
/// existed carries `""` (serde default) — comparing a real PR base against that would hold
/// every pre-cutover Run at `Uncorroborated` forever. The signal guards the declaration, not
/// the PR, so no declaration reads as satisfied. Only pre-cutover records can be empty here:
/// `job::from_issue_json` refuses a blank row, so nothing enqueued today reaches this arm.
pub fn pr_base_matches_declared(pr_base: &str, declared: &str) -> Observed<bool> {
    if declared.is_empty() {
        return Observed::Present(true);
    }
    Observed::Present(pr_base == declared)
}

/// `gh pr view --json statusCheckRollup`, classified twice — *is anything still running* and
/// *did anything come back red*. They are separate signals: one holds completion open and the
/// other lands on the verdict line without holding anything (ADR-0003).
pub fn checks(completed: &Completed) -> (Observed<bool>, Observed<bool>) {
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
            "FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE" | "STALE"
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

/// The compiled literal path lists Triage/Diff-triage score against. A path is a fact about
/// where the diff touched, never a grade of what it did there (ADR-0012).
///
/// Keyword segments (`auth`, `login`, `session`, `token`, `crypto`, `tls`, `cert`, `payment`,
/// `billing`, `invoice`, `schema`, `deploy`) match a **whole path token** — the path split on
/// every non-alphanumeric character — so `author.rs` does not read as `auth`. Directory-shaped
/// entries (`migrations/`, `api/`, `.github/workflows/`, `helm/`, `terraform/`) and filename
/// entries (`Justfile`, `justfile`, `Dockerfile`, `Info.plist`, `.entitlements`, `*.proto`,
/// `openapi`) match as substrings or suffixes of the raw path instead, because the slash already
/// carries the boundary a token split would otherwise have to reconstruct.
fn path_tokens(path: &str) -> Vec<String> {
    path.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|segment| !segment.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn hits_risky_path(path: &str, kind: RiskyPathKind) -> bool {
    let tokens = path_tokens(path);
    let has = |words: &[&str]| words.iter().any(|word| tokens.iter().any(|t| t == word));
    match kind {
        RiskyPathKind::Auth => has(&["auth", "login", "session", "token"]),
        RiskyPathKind::Crypto => has(&["crypto", "tls", "cert"]),
        RiskyPathKind::Payments => has(&["payment", "payments", "billing", "invoice"]),
        RiskyPathKind::Migrations => path.contains("migrations/") || has(&["schema"]),
        RiskyPathKind::PublicApi => {
            path.contains("api/")
                || path.to_ascii_lowercase().contains("openapi")
                || path.ends_with(".proto")
        }
        RiskyPathKind::CiConfig => {
            path.contains(".github/workflows/")
                || path.ends_with("Justfile")
                || path.ends_with("justfile")
                || path.ends_with("Dockerfile")
        }
        RiskyPathKind::DeploySurface => {
            has(&["deploy", "deployment"])
                || path.contains("helm/")
                || path.contains("terraform/")
                || path.ends_with("Info.plist")
                || path.ends_with(".entitlements")
        }
    }
}

/// Lockfiles and generated churn `changed_loc` excludes — a thousand-line `Cargo.lock` bump
/// reads as a huge diff and says nothing about the risk the size signal exists to catch.
fn is_lockfile_or_generated(path: &str) -> bool {
    let base = path.rsplit('/').next().unwrap_or(path);
    matches!(
        base,
        "Cargo.lock" | "package-lock.json" | "yarn.lock" | "pnpm-lock.yaml"
    ) || base.contains(".generated.")
        || path
            .split('/')
            .any(|segment| segment == "target" || segment == "node_modules")
}

fn added_lines(diff_text: &str) -> impl Iterator<Item = &str> {
    diff_text
        .lines()
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
}

/// Content-kind scan, **added lines only**. Deliberately tolerant of false positives — a raw-SQL
/// match on `UPDATE ` inside an English sentence, or `unsafe ` inside a doc comment, costs
/// nothing but a tier that rounds up one notch (the design's own rule: any ambiguity rounds up);
/// missing a real hit costs the review the diff needed.
fn hits_content(diff_text: &str, kind: ContentKind) -> bool {
    added_lines(diff_text).any(|line| match kind {
        ContentKind::Unsafe => line.contains("unsafe "),
        ContentKind::RawSql => {
            let upper = line.to_ascii_uppercase();
            upper.contains("SELECT ") || upper.contains("INSERT INTO") || upper.contains("UPDATE ")
        }
        ContentKind::EvalExec => line.contains("eval(") || line.contains("exec("),
        ContentKind::Subprocess => {
            line.contains("Command::new")
                || line.contains("subprocess")
                || line.contains("child_process")
        }
        ContentKind::Concurrency => [
            "Mutex",
            "RwLock",
            "AtomicU",
            "mpsc",
            "tokio::spawn",
            "thread::spawn",
        ]
        .iter()
        .any(|needle| line.contains(needle)),
        ContentKind::Secrets => {
            ["SECRET", "API_KEY", "PRIVATE_KEY"]
                .iter()
                .any(|needle| line.contains(needle))
                || line.contains("password")
        }
        ContentKind::TodoFixme => line.contains("TODO") || line.contains("FIXME"),
    })
}

/// A line is at signature position if, once the diff marker is stripped, it **starts with** one
/// of the exported-declaration spellings. Approximate on purpose — a `pub fn` inside a doc
/// comment example counts, a signature split across lines does not — and documented rather than
/// hidden behind a parser this module has no business owning.
fn is_signature_line(marked_line: &str) -> bool {
    let stripped = marked_line
        .strip_prefix(['+', '-'])
        .unwrap_or(marked_line)
        .trim_start();
    [
        "pub fn ",
        "pub struct ",
        "pub enum ",
        "pub trait ",
        "export function ",
        "export const ",
        "def ",
    ]
    .iter()
    .any(|prefix| stripped.starts_with(prefix))
}

fn surface_delta(diff_text: &str) -> usize {
    diff_text
        .lines()
        .filter(|line| {
            (line.starts_with('+') && !line.starts_with("+++"))
                || (line.starts_with('-') && !line.starts_with("---"))
        })
        .filter(|line| is_signature_line(line))
        .count()
}

const DEP_MANIFEST_NAMES: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "go.mod",
    "Gemfile",
];

/// `decide::DiffFacts`, from `git diff --numstat <handoff>..HEAD`, `git diff --name-only` and
/// the unified diff text — all three handed in as text; the `world` calls that produce them are
/// the supervisor's job, not this parser's.
pub fn diff_facts(numstat: &str, name_only: &str, diff_text: &str) -> DiffFacts {
    let mut changed_loc = 0usize;
    for line in numstat.lines() {
        let mut fields = line.splitn(3, '\t');
        let (Some(additions), Some(deletions), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let path = path.trim();
        if additions == "-" || deletions == "-" || is_lockfile_or_generated(path) {
            continue;
        }
        changed_loc +=
            additions.parse::<usize>().unwrap_or(0) + deletions.parse::<usize>().unwrap_or(0);
    }

    let names: Vec<&str> = name_only
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();

    let risky_paths_hit: Vec<RiskyPathKind> = [
        RiskyPathKind::Auth,
        RiskyPathKind::Crypto,
        RiskyPathKind::Payments,
        RiskyPathKind::Migrations,
        RiskyPathKind::PublicApi,
        RiskyPathKind::CiConfig,
        RiskyPathKind::DeploySurface,
    ]
    .into_iter()
    .filter(|kind| names.iter().any(|path| hits_risky_path(path, *kind)))
    .collect();

    let content_kinds: Vec<ContentKind> = [
        ContentKind::Unsafe,
        ContentKind::RawSql,
        ContentKind::EvalExec,
        ContentKind::Subprocess,
        ContentKind::Concurrency,
        ContentKind::Secrets,
        ContentKind::TodoFixme,
    ]
    .into_iter()
    .filter(|kind| hits_content(diff_text, *kind))
    .collect();

    let dep_manifest_touched = names.iter().any(|path| {
        let base = path.rsplit('/').next().unwrap_or(path);
        DEP_MANIFEST_NAMES.contains(&base)
    });

    DiffFacts {
        changed_loc,
        risky_paths_hit,
        content_kinds,
        surface_delta: surface_delta(diff_text),
        dep_manifest_touched,
    }
}

/// The ten stage skill directories a host must have copied under `~/.grind/skills/run` before
/// a Run can walk the ladder (ADR-0015). Named here, once, so `skills_present` and whatever
/// provisions the host read the same list.
pub const STAGE_SKILLS: [&str; 10] = [
    "plan",
    "plan-review",
    "work",
    "simplify",
    "review",
    "validate",
    "fixes",
    "ship",
    "babysit",
    "reflect",
];

/// The layout check: every stage skill directory present under the host skill root. `entries` is
/// the caller's directory listing — `world::list_dir` is the caller's door, this stays pure over
/// the names it was handed.
pub fn skills_present(entries: &[String]) -> Observed<Outcome> {
    let missing: Vec<&str> = STAGE_SKILLS
        .iter()
        .copied()
        .filter(|name| !entries.iter().any(|entry| entry == name))
        .collect();
    if missing.is_empty() {
        satisfied("all ten stage skill directories are present under ~/.grind/skills/run")
    } else {
        unsatisfied(&format!(
            "missing stage skill directories: {}",
            missing.join(", ")
        ))
    }
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
    job_branch: &str,
    declared_base: &str,
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
    let head = run(&argv(&["git", "rev-parse", "HEAD"]));
    let by_head = if head.code == Some(0) && !head.stdout.trim().is_empty() {
        pr_by_head(
            head.stdout.trim(),
            &run(&argv(&[
                "gh",
                "pr",
                "list",
                "--search",
                head.stdout.trim(),
                "--state",
                "all",
                "--json",
                "number,url,state,isDraft,headRefOid,headRefName,baseRefName",
            ])),
        )
    } else {
        Observed::Unobservable(Reason::of("git rev-parse HEAD", &head))
    };
    let found = match by_head {
        Observed::Present(found) => Observed::Present(found),
        Observed::Absent => pr(&run(&argv(&[
            "gh",
            "pr",
            "view",
            "--json",
            "number,url,state,isDraft,headRefName,baseRefName",
        ]))),
        Observed::Unobservable(reason) => match pr(&run(&argv(&[
            "gh",
            "pr",
            "view",
            "--json",
            "number,url,state,isDraft,headRefName,baseRefName",
        ]))) {
            Observed::Present(fallback) => Observed::Present(fallback),
            _ => Observed::Unobservable(reason),
        },
    };

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

    let (pr_head_matches_job_branch, pr_base_matches_declared) = match &found {
        Observed::Present(open) => (
            pr_head_matches_job_branch(&open.head_ref, job_branch),
            pr_base_matches_declared(&open.base_ref, declared_base),
        ),
        Observed::Absent => (Observed::Present(false), Observed::Present(false)),
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
        pr_head_matches_job_branch,
        pr_base_matches_declared,
        changed_files: changed,
        base_drift: drift,
    }
}

/// Freshness for a `Backend::Native` Run: seconds since the newest write across the Run
/// directory's `messages-*.jsonl` files, the same shape `claude::live`'s own `freshness` field
/// already carries (present-with-a-count, or could-not-observe — never zero for *nothing to
/// read*).
///
/// `mtimes` is [`crate::native::live`]'s door to `world::list_with_extension` + `world::mtime`
/// over the Run directory; this stays pure over the values so the newest-wins and
/// empty-is-unobservable rules are testable from literals with no filesystem.
pub fn native_freshness(mtimes: &[SystemTime], now_epoch: u64) -> Observed<u64> {
    match mtimes.iter().max() {
        Some(newest) => Observed::Present(seconds_since_epoch(*newest, now_epoch)),
        None => Observed::Unobservable(Reason::saying(
            "no messages-*.jsonl write under the Run directory to read a time from",
        )),
    }
}

fn seconds_since_epoch(at: SystemTime, now_epoch: u64) -> u64 {
    let then = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now_epoch.saturating_sub(then)
}

/// A PR's terminal facts, once it stopped moving. Facts only, never a grade (ADR-0012):
/// `merged: true` is what happened, and it carries no opinion about whether that was good.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrFinal {
    pub state: String,
    pub merged: bool,
    pub merged_at: Option<String>,
    pub closed_at: Option<String>,
}

/// `gh pr view --json state,mergedAt,closedAt,mergeCommit`, already reduced to its stdout.
/// Whether the call succeeded at all is the caller's to read off the exit code first — this
/// stays a parse over text, mirroring [`pr`]'s own split between spawn and classification.
pub fn pr_final_state(gh_json: &str) -> Observed<PrFinal> {
    let body = gh_json.trim();
    if body.is_empty() {
        return Observed::Unobservable(Reason::saying("gh pr view --json state,...: empty output"));
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return Observed::Unobservable(Reason::saying(
            "gh pr view --json state,mergedAt,closedAt,mergeCommit: unreadable JSON",
        ));
    };
    let Some(state) = value.get("state").and_then(|s| s.as_str()) else {
        return Observed::Unobservable(Reason::saying(
            "gh pr view --json state,mergedAt,closedAt,mergeCommit: missing state",
        ));
    };
    Observed::Present(PrFinal {
        state: state.to_string(),
        merged: state.eq_ignore_ascii_case("MERGED"),
        merged_at: value
            .get("mergedAt")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        closed_at: value
            .get("closedAt")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

/// The issue numbers, from `gh issue list --json number,title --search "<query>"`, already
/// reduced to its stdout. Tolerant by construction: empty or malformed output parses as an
/// empty list, and rows whose `number` is anything but an integer contribute nothing — a
/// repo this pass cannot query leaves [`RunOutcome::followup_issues`] empty rather than
/// failing it.
pub fn followup_issues(gh_json: &str) -> Vec<u64> {
    let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(gh_json.trim()) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| row.get("number").and_then(serde_json::Value::as_u64))
        .collect()
}

/// A line this module accepts as a commit sha rather than a path — hex, and long enough that a
/// path made only of hex-looking segments (vanishingly unlikely, but not this module's problem
/// to rule out further) is not what a `git log --format=%H --name-only` run actually emits.
fn is_sha_line(line: &str) -> bool {
    line.len() >= 7 && line.chars().all(|c| c.is_ascii_hexdigit())
}

/// The commit shas, from `git log --grep=Revert -i --format=%H --name-only <run's own diff
/// window>`, whose touched files intersect `run_paths` — the Run's own diff, so a revert of
/// somebody else's file never attributes here.
///
/// The output alternates a bare sha line with the file lines that commit touched; this parses
/// that shape without assuming a blank separator, because git's own spacing around `--name-only`
/// blocks is not part of the contract this module owns.
pub fn reverts_touching(log_text: &str, run_paths: &[String]) -> Vec<String> {
    let mut hits = Vec::new();
    let mut current: Option<(&str, Vec<&str>)> = None;
    for raw in log_text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if is_sha_line(line) {
            if let Some((sha, paths)) = current.take()
                && paths.iter().any(|p| run_paths.iter().any(|mine| mine == p))
            {
                hits.push(sha.to_string());
            }
            current = Some((line, Vec::new()));
        } else if let Some((_, paths)) = current.as_mut() {
            paths.push(line);
        }
    }
    if let Some((sha, paths)) = current
        && paths.iter().any(|p| run_paths.iter().any(|mine| mine == p))
    {
        hits.push(sha.to_string());
    }
    hits
}

/// `outcome.json`, written beside a Run's record by `grind outcomes` — a separate file, never
/// `run.json`, so the sole-writer rule on `attempts[]` is never in the same room as this
/// read-mostly pass. Facts, computed rather than classified: `reverted_by` is a list of shas,
/// never a verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunOutcome {
    pub collected_at: String,
    pub pr_state: String,
    pub pr_merged: bool,
    pub pr_merged_at: Option<String>,
    pub pr_closed_at: Option<String>,
    pub reverted_by: Vec<String>,
    /// Follow-up issues referencing the Run's PR or filed against its changed paths since
    /// the PR merged, read by `grind outcomes` through one `gh issue list --search` in the
    /// Run's own worktree. Empty when the repo is unqueryable — never a failure of the
    /// pass.
    pub followup_issues: Vec<u64>,
}

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

/// **When the restart one-shot actually fires**, which differs by platform and which doctor
/// cannot check.
///
/// The two service managers do not offer the same promise, and the difference is the whole of
/// the caveat this check carries:
///
/// - **linux** fires at boot — but only with `loginctl enable-linger <user>`, without which the
///   user's systemd instance starts at first login and stops at last logout. The check is
///   conjunctive for exactly that reason.
/// - **darwin** fires at **login**. `launchctl bootstrap gui/$(id -u)` puts the job in the GUI
///   domain, and `RunAtLoad` fires when that domain loads. A LaunchDaemon would fire earlier,
///   but on a FileVault Mac — the default — `/Users/<user>` is not decrypted until someone
///   unlocks at the login window, so `~/.grind/runs/` does not exist yet to re-enter from.
///   There is no shipping shape that re-enters a Run before a human touches the machine.
///
/// Doctor **structurally cannot tell the two apart on darwin**: `launchctl print gui/$(id -u)/…`
/// can only run from inside a logged-in GUI session, which is the very condition it would need
/// to distinguish. So the caveat rides the satisfied text rather than pretending to be checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fires {
    AtBoot,
    AtLogin,
}

/// The restart one-shot: **loaded**, not merely present.
///
/// A plist copied into `~/Library/LaunchAgents` and never bootstrapped is the likeliest way this
/// fails, and it fails one reboot later with a Run stranded and nothing saying so. So the check
/// asks the service manager what it has loaded rather than asking the filesystem what is there.
///
/// A service manager that could not be reached at all is **could not observe**, never
/// unsatisfied: *no such unit* and *no `launchctl` on this box* are different facts, and the
/// second one is about the check rather than about the host.
pub fn boot_one_shot(completed: &Completed, fires: Fires) -> Observed<Outcome> {
    match completed.code {
        Some(0) => satisfied(match fires {
            Fires::AtBoot => {
                "a one-shot calling `grind resume --all` is enabled and the user lingers, so it \
                 fires at boot"
            }
            Fires::AtLogin => {
                "a one-shot calling `grind resume --all` is loaded — it fires at login, not at \
                 boot, so a restarted host waits for a human before re-entering"
            }
        }),
        Some(127) | None => Observed::Unobservable(Reason::saying(
            "the service manager could not be reached to ask what it has loaded",
        )),
        Some(_) => unsatisfied(match fires {
            Fires::AtBoot => {
                "no one-shot calling `grind resume --all` is both enabled and lingering — a unit \
                 on disk that was never enabled counts as absent, and an enabled unit without \
                 `loginctl enable-linger <user>` does not start at boot; see dist/"
            }
            Fires::AtLogin => {
                "no one-shot calling `grind resume --all` is loaded — a plist on disk that was \
                 never bootstrapped counts as absent; see dist/"
            }
        }),
    }
}

/// When the process under a pid started, as `ps` answered.
///
/// **Three-valued, because a `ps` that could not run is not a process that is gone.** Grind
/// ships as a musl static binary aimed at minimal Linux hosts, and `-p <pid> -o lstart=` is a
/// procps/BSD spelling busybox `ps` does not implement — so *could not ask* is an ordinary
/// reading here rather than a theoretical one. It is also the reading `resume --all` **acts**
/// on: folding it into *gone* re-enters every Run on the host at boot, which is the opposite of
/// the direction that path's own doc calls safe.
///
/// `ps` exits 1 with nothing on stdout when no process matches, which is a fact about the
/// world; every other failure is a fact about the check.
pub fn process_start_stamp(completed: &Completed) -> Observed<String> {
    let stamp = completed.stdout.trim().to_string();
    match completed.code {
        Some(0) | Some(1) if !stamp.is_empty() => Observed::Present(stamp),
        Some(0) | Some(1) => Observed::Absent,
        _ => Observed::Unobservable(Reason::of("ps -p <pid> -o lstart=", completed)),
    }
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

/// The omp harness CLI is executable. Mirrors [`claude_binary`] minus the shim clause: that
/// check asserts loudly because a terminal's `claude` shim is a documented hazard on this
/// laptop, and no such story exists for omp — inventing the same assertion here would be a
/// precondition that fails for no reason. Presence only, resolved by the caller: which path
/// was tested (`GRIND_OMP_BIN` or the bun default) is the caller's sentence, not this one's.
pub fn omp_binary(executable: bool) -> Observed<Outcome> {
    if executable {
        satisfied("the omp binary is executable")
    } else {
        unsatisfied("the omp binary is missing or not executable")
    }
}

/// A provider API key is in the environment — **presence only**, never the values, and
/// never a validity judgement: only an attempt to use a key can classify it. Both backends'
/// readiness is reported regardless of which backend a Run selected (R9) — doctor takes no
/// Job and the selection is a layout fact, not this list's.
pub fn agent_key_present(openrouter: bool, openai: bool) -> Observed<Outcome> {
    match (openrouter, openai) {
        (true, true) => satisfied("OPENROUTER_API_KEY and OPENAI_API_KEY are both set"),
        (true, false) => satisfied("OPENROUTER_API_KEY is set"),
        (false, true) => satisfied("OPENAI_API_KEY is set"),
        (false, false) => {
            unsatisfied("neither OPENROUTER_API_KEY nor OPENAI_API_KEY is set in the environment")
        }
    }
}

/// The OpenAI-compatible endpoint answered a connection-level probe. `None` means the probe
/// could not even be tried — no key in the environment resolves an [`crate::runner::Endpoint`]
/// — which is could-not-observe, not unsatisfied: *no way to ask* and *the endpoint did not
/// answer* are different facts about different things.
pub fn endpoint_reachable(probed: Option<bool>) -> Observed<Outcome> {
    match probed {
        Some(true) => satisfied("the agent endpoint answers"),
        Some(false) => unsatisfied("the agent endpoint did not answer a probe request"),
        None => Observed::Unobservable(Reason::saying(
            "no provider API key in the environment, so no endpoint could be probed",
        )),
    }
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

/// Free space on the filesystem holding the grind directory, from `df -kP`, against an
/// authored floor — `job::DISK_HEADROOM_FLOOR_KIB` names the number; this function only
/// reads it, so a threshold change never touches a parser (the same split
/// [`git_version_floor`] draws between its floor argument and its own text).
///
/// The columns are read **by position**, from whitespace splitting: POSIX `df` output
/// localizes its headers like any other command, so a header cannot be trusted to name its
/// own columns and fixed positions are the only shape every locale produces. Line two's
/// fourth field is the Available count in 1 KiB blocks; everything after it — Capacity, and
/// mountpoints whose path contains spaces — belongs to later columns and is ignored rather
/// than parsed.
///
/// A call that did not cleanly succeed is **could not observe**, never unsatisfied or
/// absent, and what `df` said on the way out never reaches the reason (KTD16): those are
/// facts about the check. Only a readable count yields a verdict, spelled `>=` so a reading
/// exactly at the floor satisfies.
pub fn disk_headroom(raw: &Completed, floor_kib: u64) -> Observed<Outcome> {
    if raw.code != Some(0) {
        return Observed::Unobservable(Reason::saying(
            "`df -kP` did not run to completion, so there is no capacity reading to classify",
        ));
    }
    let Some(row) = raw.stdout.lines().nth(1) else {
        return Observed::Unobservable(Reason::saying(
            "`df -kP` printed no second line, so there is no data row to read",
        ));
    };
    let Some(available) = row.split_whitespace().nth(3) else {
        return Observed::Unobservable(Reason::saying(
            "the `df -kP` row stops before the Available column",
        ));
    };
    let Ok(available_kib) = available.parse::<u64>() else {
        return Observed::Unobservable(Reason::saying(
            "the Available column of `df -kP` is not a whole number",
        ));
    };
    if available_kib >= floor_kib {
        satisfied(&format!(
            "{available_kib} KiB free beside the grind directory"
        ))
    } else {
        unsatisfied(&format!(
            "{available_kib} KiB free is below the {floor_kib} KiB floor"
        ))
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
    fn a_ps_that_could_not_run_is_could_not_observe_and_never_a_process_that_is_gone() {
        assert_eq!(
            process_start_stamp(&completed("Thu Aug  6 12:26:20 2026\n", "", Some(0))),
            Observed::Present("Thu Aug  6 12:26:20 2026".to_string())
        );
        for gone in [completed("", "", Some(1)), completed("\n", "", Some(0))] {
            assert_eq!(process_start_stamp(&gone), Observed::Absent, "{gone:?}");
        }
        for blind in [
            completed("", "ps: unrecognized option: p\n", Some(127)),
            completed("", "No such file or directory (os error 2)", None),
            completed("", "ps: bad -o argument\n", Some(2)),
        ] {
            assert!(
                matches!(process_start_stamp(&blind), Observed::Unobservable(_)),
                "{blind:?}"
            );
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

    const PR_JSON: &str = r#"{"number":30,"url":"https://github.com/o/n/pull/30","state":"OPEN","isDraft":false,"headRefOid":"3333333333333333333333333333333333333333","headRefName":"feat/28-slice-1b-seam","baseRefName":"main"}"#;

    const HEAD_SHA: &str = "3333333333333333333333333333333333333333";

    const JOB_BRANCH: &str = "feat/28-slice-1b-seam";
    const DECLARED_BASE: &str = "main";

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
            observe_run(
                "at".to_string(),
                "9d1f4c7a",
                JOB_BRANCH,
                DECLARED_BASE,
                &mut run,
            )
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
            observe_run(
                "at".to_string(),
                "9d1f4c7a",
                "feat/some-branch",
                "main",
                &mut run,
            )
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
    fn a_search_response_that_never_indexed_the_sha_is_found_through_its_head_oid() {
        let found = pr_by_head(
            "8d30e44919bd8f372efb27a8971d2f5823680454",
            &completed(
                r#"[{"number":84,"url":"https://github.com/o/n/pull/84","state":"OPEN","isDraft":false,"headRefOid":"8D30E44919BD8F372EFB27A8971D2F5823680454"}]"#,
                "",
                Some(0),
            ),
        );
        let Observed::Present(pr) = found else {
            panic!("the head OID identifies the PR: {found:?}");
        };
        assert_eq!(pr.number, 84);
    }

    #[test]
    fn a_search_response_of_different_head_oids_falls_through_to_the_branch_fallback() {
        let (observation, _) = observing(
            completed(
                r#"[{"number":99,"url":"https://github.com/o/n/pull/99","state":"MERGED","isDraft":false,"headRefOid":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}]"#,
                "",
                Some(0),
            ),
            completed(PR_JSON, "", Some(0)),
            completed(r#"{"statusCheckRollup":[]}"#, "", Some(0)),
        );
        let Observed::Present(found) = &observation.pr else {
            panic!("the branch fallback still finds it: {:?}", observation.pr);
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
            pr_by_head(HEAD_SHA, &completed("not json at all", "", Some(0))),
            Observed::Unobservable(_)
        ));
        assert!(matches!(
            pr_by_head(
                HEAD_SHA,
                &completed(
                    r#"[{"headRefOid":"3333333333333333333333333333333333333333","url":"x"}]"#,
                    "",
                    Some(0)
                )
            ),
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
    fn a_check_that_never_ran_or_was_retired_reads_red_rather_than_completable() {
        for conclusion in ["STARTUP_FAILURE", "STALE"] {
            let rollup = format!(
                r#"{{"statusCheckRollup":[{{"status":"COMPLETED","conclusion":"{conclusion}"}}]}}"#
            );
            let (pending, red) = checks(&completed(&rollup, "", Some(0)));
            assert_eq!(pending, Observed::Present(false), "{conclusion}");
            assert_eq!(red, Observed::Present(true), "{conclusion}");
        }
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
        let none = completed("", "no pull requests found for branch\n", Some(1));
        let (pending, red) = checks(&none);
        assert_eq!(pending, Observed::Absent);
        assert_eq!(red, Observed::Absent);
    }

    #[test]
    fn an_unreadable_worktree_is_not_a_run_that_produced_nothing() {
        let missing = completed("", "fatal: not a git repository\n", Some(128));
        assert!(matches!(changed_files(&missing), Observed::Unobservable(_)));
        assert!(matches!(
            scoped_listing(&changed_files(&missing), PLAN_DIR),
            Observed::Unobservable(_)
        ));
        assert_eq!(changed_files(&completed("", "", Some(0))), Observed::Absent);
    }

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
        let stage = |o: &Observation| crate::decide::furthest_stage(o).to_string();

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
            head_ref: "feat/x".to_string(),
            base_ref: "main".to_string(),
        });
        assert_eq!(stage(&walk), "pr-open");
    }

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
    fn the_omp_binary_classifies_presence_alone() {
        assert!(matches!(
            omp_binary(true),
            Observed::Present(Outcome::Satisfied(_))
        ));
        let missing = omp_binary(false);
        let Observed::Present(Outcome::Unsatisfied(said)) = missing else {
            panic!("a missing binary is unsatisfied, never absent: {missing:?}");
        };
        assert!(said.contains("omp"), "{said}");
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

    /// One plausible `df -kP` answer with room to spare, riding the edge of the format's
    /// quirks: a multi-word mountpoint trailing the Capacity column.
    fn df_with(available: u64) -> Completed {
        completed(
            &format!(
                "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                 /dev/disk1s1 20485760 9999999 {available} 49% /System/Volumes/Data\n"
            ),
            "",
            Some(0),
        )
    }

    #[test]
    fn space_above_the_disk_headroom_floor_is_satisfied_and_names_the_free_kib() {
        let Observed::Present(Outcome::Satisfied(said)) =
            disk_headroom(&df_with(9_501_500_272), 10_485_760)
        else {
            panic!("free space far above the floor is satisfied");
        };
        assert!(said.contains("9501500272"), "{said}");
    }

    #[test]
    fn space_exactly_at_the_disk_headroom_floor_is_still_satisfied() {
        assert!(matches!(
            disk_headroom(&df_with(10_485_760), 10_485_760),
            Observed::Present(Outcome::Satisfied(_))
        ));
    }

    #[test]
    fn one_kib_below_the_disk_headroom_floor_is_unsatisfied_and_names_both_numbers() {
        let Observed::Present(Outcome::Unsatisfied(said)) =
            disk_headroom(&df_with(10_485_759), 10_485_760)
        else {
            panic!("free space below the floor is unsatisfied");
        };
        assert!(said.contains("10485759"), "{said}");
        assert!(said.contains("10485760"), "{said}");
    }

    #[test]
    fn malformed_df_output_reads_could_not_observe_never_a_verdict_on_the_disk_headroom_check() {
        let blind = [
            // A call that never got a usable answer is a fact about the check, not about the
            // disk — including when stdout rode along or stderr explains why.
            completed("", "", Some(1)),
            completed(
                "",
                "df: /System/Volumes/Data: No such file or directory\n",
                Some(1),
            ),
            completed(
                "df: illegal option -- k\nUsage: df [-hkn] [file ...]\n",
                "",
                Some(2),
            ),
            // Header only: the data row every column lives in is missing.
            completed(
                "Filesystem 1024-blocks Used Available Capacity Mounted on\n",
                "",
                Some(0),
            ),
            // A row too short to carry an Available column…
            completed(
                "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                 /dev/disk1s1 20485760 9999999\n",
                "",
                Some(0),
            ),
            // …and a fourth field that is some other column's rendering (the Capacity
            // percent) rather than a count.
            completed(
                "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                 /dev/disk1s1 20485760 9999999 49% /System/Volumes/Data\n",
                "",
                Some(0),
            ),
            // BusyBox-style placeholder where a number should be.
            completed(
                "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                 /dev/disk1s1 20485760 9999999 -- /System/Volumes/Data\n",
                "",
                Some(0),
            ),
        ];
        for df in blind {
            match disk_headroom(&df, 10_485_760) {
                Observed::Unobservable(reason) => {
                    let said = reason.to_string();
                    assert!(
                        !said.contains("/dev/disk1s1")
                            && !said.contains("illegal option")
                            && !said.contains("No such file"),
                        "raw child output must never reach a reason: {said}"
                    );
                }
                other => {
                    panic!("a blind reading must withhold the verdict, not give one: {other:?}")
                }
            }
        }
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
    fn a_boot_one_shot_is_satisfied_only_when_the_service_manager_says_it_is_loaded() {
        let loaded = completed(
            "com.grind.resume-all = {\n\tactive count = 0\n}\n",
            "",
            Some(0),
        );
        let Observed::Present(Outcome::Satisfied(on_linux)) = boot_one_shot(&loaded, Fires::AtBoot)
        else {
            panic!("a loaded unit is satisfied");
        };
        assert!(on_linux.contains("fires at boot"), "{on_linux}");
        assert!(on_linux.contains("lingers"), "{on_linux}");

        let Observed::Present(Outcome::Satisfied(on_darwin)) =
            boot_one_shot(&loaded, Fires::AtLogin)
        else {
            panic!("a loaded agent is satisfied");
        };
        assert!(
            on_darwin.contains("fires at login, not at boot"),
            "{on_darwin}"
        );
        assert!(
            on_darwin.contains("waits for a human"),
            "a satisfied darwin host must still name what the promise is:\n{on_darwin}"
        );
    }

    #[test]
    fn a_user_unit_without_linger_is_unsatisfied_rather_than_quietly_enabled() {
        let found = boot_one_shot(&completed("", "", Some(1)), Fires::AtBoot);
        let Observed::Present(Outcome::Unsatisfied(said)) = found else {
            panic!("expected unsatisfied: {found:?}");
        };
        assert!(said.contains("enable-linger"), "{said}");
        assert!(said.contains("does not start at boot"), "{said}");
    }

    #[test]
    fn a_plist_on_disk_that_was_never_bootstrapped_is_unsatisfied_and_never_satisfied() {
        for never_loaded in [
            completed(
                "",
                "Could not find service \"com.grind.resume-all\"\n",
                Some(113),
            ),
            completed("disabled\n", "", Some(1)),
            completed(
                "",
                "Failed to get unit file state: No such file or directory\n",
                Some(1),
            ),
        ] {
            let found = boot_one_shot(&never_loaded, Fires::AtLogin);
            assert!(
                matches!(found, Observed::Present(Outcome::Unsatisfied(_))),
                "{found:?}"
            );
        }
    }

    #[test]
    fn a_service_manager_that_cannot_be_reached_is_could_not_observe_never_unsatisfied() {
        for unreachable in [
            completed("", "sh: launchctl: command not found\n", Some(127)),
            completed("", "", None),
        ] {
            let found = boot_one_shot(&unreachable, Fires::AtLogin);
            assert!(matches!(found, Observed::Unobservable(_)), "{found:?}");
        }
    }

    #[test]
    fn the_steps_no_check_can_reach_are_named_rather_than_guessed() {
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
    fn a_pr_pushed_to_the_branch_the_job_named_matches_on_both_head_and_base() {
        let (observation, _) = observing(
            completed(&format!("[{PR_JSON}]"), "", Some(0)),
            no_pr_on_this_branch(),
            completed(r#"{"statusCheckRollup":[]}"#, "", Some(0)),
        );
        assert_eq!(
            observation.pr_head_matches_job_branch,
            Observed::Present(true)
        );
        assert_eq!(
            observation.pr_base_matches_declared,
            Observed::Present(true)
        );
    }

    #[test]
    fn a_pr_pushed_to_a_different_branch_than_the_job_named_is_a_false_not_a_blind_signal() {
        assert_eq!(
            pr_head_matches_job_branch("feat/28-slice-1b-run", "feat/28-slice-1b-seam"),
            Observed::Present(false)
        );
        assert_eq!(
            pr_base_matches_declared("develop", "main"),
            Observed::Present(false)
        );
    }

    #[test]
    fn an_undeclared_base_cannot_mismatch_so_a_pre_cutover_record_still_completes() {
        assert_eq!(
            pr_base_matches_declared("main", ""),
            Observed::Present(true)
        );
    }

    #[test]
    fn no_pr_yet_reads_both_new_signals_as_false_mirroring_pr_open() {
        let (observation, _) = observing_with_diff("src/lib.rs\n");
        assert_eq!(observation.pr, Observed::Absent);
        assert_eq!(
            observation.pr_head_matches_job_branch,
            Observed::Present(false)
        );
        assert_eq!(
            observation.pr_base_matches_declared,
            Observed::Present(false)
        );
    }

    #[test]
    fn a_pr_lookup_that_could_not_be_made_leaves_both_new_signals_blind() {
        let code: i32 = GH_AUTH_CODE.trim().parse().expect("the recorded exit code");
        let broken = completed(GH_AUTH_STDOUT, GH_AUTH_STDERR, Some(code));
        let (observation, _) = observing(broken.clone(), broken.clone(), broken);
        assert!(matches!(
            observation.pr_head_matches_job_branch,
            Observed::Unobservable(_)
        ));
        assert!(matches!(
            observation.pr_base_matches_declared,
            Observed::Unobservable(_)
        ));
    }

    #[test]
    fn lockfile_and_generated_churn_is_excluded_from_changed_loc() {
        let numstat = "500\t300\tCargo.lock\n\
             10\t2\tsrc/lib.rs\n\
             40\t0\tweb/dist/app.generated.js\n\
             8\t0\tnode_modules/pkg/index.js\n\
             3\t0\ttarget/debug/build.rs\n\
             0\t0\tbinary.png\n";
        let facts = diff_facts(numstat, "src/lib.rs\n", "+fn f() {}\n");
        assert_eq!(facts.changed_loc, 12);
    }

    #[test]
    fn a_binary_row_in_numstat_contributes_no_lines() {
        let facts = diff_facts("-\t-\tassets/icon.png\n", "assets/icon.png\n", "");
        assert_eq!(facts.changed_loc, 0);
    }

    #[test]
    fn risky_path_kinds_are_counted_once_each_regardless_of_how_many_paths_hit() {
        let names = "src/auth/login.rs\nsrc/auth/session.rs\n.github/workflows/ci.yml\n";
        let facts = diff_facts("2\t1\tsrc/auth/login.rs\n", names, "");
        assert_eq!(
            facts.risky_paths_hit,
            vec![RiskyPathKind::Auth, RiskyPathKind::CiConfig]
        );
        assert_eq!(facts.risky_path_hits(), 2);
    }

    #[test]
    fn a_token_boundary_keeps_author_rs_from_reading_as_auth() {
        let facts = diff_facts("1\t0\tsrc/author.rs\n", "src/author.rs\n", "");
        assert!(facts.risky_paths_hit.is_empty());
    }

    #[test]
    fn directory_shaped_risky_kinds_match_on_the_slash_boundary() {
        let names = "migrations/2026_add_users.sql\napi/v1/openapi.yaml\nhelm/values.yaml\n";
        let facts = diff_facts("1\t0\tmigrations/2026_add_users.sql\n", names, "");
        assert_eq!(
            facts.risky_paths_hit,
            vec![
                RiskyPathKind::Migrations,
                RiskyPathKind::PublicApi,
                RiskyPathKind::DeploySurface,
            ]
        );
    }

    #[test]
    fn content_signals_scan_added_lines_only_and_count_each_kind_once() {
        let diff_text = "+let m = Mutex::new(0);\n\
             -let m = old_mutex();\n\
             +tokio::spawn(async {});\n\
             + // TODO: revisit\n";
        let facts = diff_facts("4\t1\tsrc/lib.rs\n", "src/lib.rs\n", diff_text);
        assert_eq!(
            facts.content_kinds,
            vec![ContentKind::Concurrency, ContentKind::TodoFixme]
        );
        assert_eq!(facts.content_signals(), 2);
    }

    #[test]
    fn a_removed_line_never_contributes_a_content_signal() {
        let diff_text = "-unsafe { do_it() }\n+safe_call();\n";
        let facts = diff_facts("1\t1\tsrc/lib.rs\n", "src/lib.rs\n", diff_text);
        assert!(facts.content_kinds.is_empty());
    }

    #[test]
    fn surface_delta_counts_added_and_removed_public_signatures() {
        let diff_text = "+pub fn new_one() {}\n\
             -pub struct Old {}\n\
             +export function widget() {}\n\
             +fn private_helper() {}\n";
        let facts = diff_facts("4\t1\tsrc/lib.rs\n", "src/lib.rs\n", diff_text);
        assert_eq!(facts.surface_delta, 3);
    }

    #[test]
    fn a_dependency_manifest_touched_anywhere_in_the_diff_is_flagged() {
        let facts = diff_facts("2\t0\tCargo.toml\n", "Cargo.toml\nsrc/lib.rs\n", "");
        assert!(facts.dep_manifest_touched);
        let clean = diff_facts("2\t0\tsrc/lib.rs\n", "src/lib.rs\n", "");
        assert!(!clean.dep_manifest_touched);
    }

    #[test]
    fn all_ten_stage_skills_present_is_satisfied() {
        let entries: Vec<String> = STAGE_SKILLS.iter().map(|s| s.to_string()).collect();
        assert!(matches!(
            skills_present(&entries),
            Observed::Present(Outcome::Satisfied(_))
        ));
    }

    #[test]
    fn a_missing_stage_skill_is_named_in_the_unsatisfied_text() {
        let entries: Vec<String> = STAGE_SKILLS
            .iter()
            .filter(|s| **s != "reflect")
            .map(|s| s.to_string())
            .collect();
        let Observed::Present(Outcome::Unsatisfied(text)) = skills_present(&entries) else {
            panic!("a missing skill must be reported, never satisfied");
        };
        assert!(text.contains("reflect"), "{text}");
    }

    #[test]
    fn pr_final_state_reads_a_merged_pr() {
        let body = r#"{"state":"MERGED","mergedAt":"2026-08-20T10:00:00Z","closedAt":"2026-08-20T10:00:00Z","mergeCommit":{"oid":"abc"}}"#;
        assert_eq!(
            pr_final_state(body),
            Observed::Present(PrFinal {
                state: "MERGED".to_string(),
                merged: true,
                merged_at: Some("2026-08-20T10:00:00Z".to_string()),
                closed_at: Some("2026-08-20T10:00:00Z".to_string()),
            })
        );
    }

    #[test]
    fn pr_final_state_reads_a_closed_unmerged_pr_as_not_merged() {
        let body = r#"{"state":"CLOSED","mergedAt":null,"closedAt":"2026-08-20T10:00:00Z"}"#;
        assert_eq!(
            pr_final_state(body),
            Observed::Present(PrFinal {
                state: "CLOSED".to_string(),
                merged: false,
                merged_at: None,
                closed_at: Some("2026-08-20T10:00:00Z".to_string()),
            })
        );
    }

    #[test]
    fn pr_final_state_is_unobservable_over_empty_or_unreadable_output() {
        assert!(matches!(pr_final_state(""), Observed::Unobservable(_)));
        assert!(matches!(
            pr_final_state("not json"),
            Observed::Unobservable(_)
        ));
        assert!(matches!(pr_final_state("{}"), Observed::Unobservable(_)));
    }

    #[test]
    fn reverts_touching_matches_commits_whose_files_intersect_the_run_diff() {
        let log = "\
deadbeef1
src/observe.rs
docs/adr/0001.md
cafebabe2
README.md
";
        let run_paths = vec!["src/observe.rs".to_string()];
        assert_eq!(
            reverts_touching(log, &run_paths),
            vec!["deadbeef1".to_string()]
        );
    }

    #[test]
    fn reverts_touching_is_empty_when_nothing_overlaps() {
        let log = "\
deadbeef1
README.md
";
        let run_paths = vec!["src/observe.rs".to_string()];
        assert!(reverts_touching(log, &run_paths).is_empty());
    }

    #[test]
    fn reverts_touching_reads_the_final_commit_block_with_no_trailing_blank_line() {
        let log = "deadbeef1\nsrc/observe.rs";
        let run_paths = vec!["src/observe.rs".to_string()];
        assert_eq!(
            reverts_touching(log, &run_paths),
            vec!["deadbeef1".to_string()]
        );
    }

    #[test]
    fn followup_issues_reads_the_numbers_from_a_normal_listing() {
        let body = r#"[
  {"number":12,"title":"follow-up: widen the gate"},
  {"number":34,"title":"docs: mention the new flag"}
]"#;
        assert_eq!(followup_issues(body), vec![12, 34]);
    }

    #[test]
    fn followup_issues_is_empty_over_empty_or_malformed_output() {
        assert!(followup_issues("").is_empty());
        assert!(followup_issues("[]").is_empty());
        assert!(followup_issues("not json").is_empty());
        assert!(followup_issues(r#"{"number":1}"#).is_empty());
    }

    #[test]
    fn followup_issues_skips_rows_whose_number_is_not_a_number() {
        let body = r#"[{"title":"no number"},{"number":null},{"number":7,"title":"real"}]"#;
        assert_eq!(followup_issues(body), vec![7]);
    }

    #[test]
    fn every_arm_is_constructed_somewhere_in_this_module() {
        let arms: [Observed<u64>; 3] = [
            Observed::Present(1),
            Observed::Absent,
            Observed::Unobservable(Reason::saying("constructed")),
        ];
        assert_eq!(arms.len(), 3);
    }

    #[test]
    fn native_freshness_reads_the_newest_of_several_mtimes() {
        let now = 1_785_000_000u64;
        let older = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(now - 500);
        let newest = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(now - 20);
        assert_eq!(
            native_freshness(&[older, newest], now),
            Observed::Present(20)
        );
        assert_eq!(
            native_freshness(&[newest, older], now),
            Observed::Present(20)
        );
    }

    #[test]
    fn native_freshness_over_no_files_is_could_not_observe_not_zero() {
        let found = native_freshness(&[], 1_785_000_000);
        assert!(matches!(found, Observed::Unobservable(_)), "{found:?}");
        assert_ne!(found, Observed::Present(0));
    }
}
