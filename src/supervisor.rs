//! The loop, the dispatch lock, and the **only writer of `run.json`**.
//!
//! `RunRecord` is private to this module — not `pub`, not `pub(crate)`, not re-exported.
//! Privacy only bites between siblings, so **`supervisor` and `view` stay siblings at the crate
//! root and never share a parent**: a child module reaches its ancestor's private items and
//! compiles clean, which would withdraw this guarantee by housekeeping. `tests/compile_fail.rs`
//! is what asserts the error, and `tests/topology.rs` is what asserts the arrangement.
//!
//! The bug this closes is shipped and live in the script: a read path that saves what it loaded
//! can erase `attempts[]`, which nothing can rebuild — and it erases it while the human is
//! watching the dashboard to be reassured.

use crate::attempt::{self, Attempt, Clearance, Conditions, DENIED_TOOLS, Invocation, Mode};
use crate::decide::{self, Verdict};
use crate::job::{self, Job, Refusal};
use crate::observe::{Observation, Observed, Outcome as ItemOutcome, Reason};
use crate::policy::{self, Budget, Next, Stop};
use crate::world::{self, LockHandle, TryLock};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Compiled constants, snapshotted into the record at dispatch. There is no environment
/// override (ADR-0008), so the record is what makes *attempt N of M with M from the record*
/// true and what makes a re-entry under different conditions visible rather than silent.
pub const ATTEMPT_BUDGET: usize = 8;
pub const LIMIT_SLEEP_SECONDS: u64 = 1800;
pub const REOBSERVATIONS: usize = 3;
/// How long a re-observation waits before looking again. The mechanism exists for a transient
/// window — the one after a laptop wake is the case it names — and three retries fired within
/// milliseconds of each other cannot span it; this is what spends the retry budget across real
/// time instead. Not per-Job and not recorded: like `REOBSERVATIONS`, it is Grind's own policy
/// knob rather than a fact the record needs to freeze.
pub const REOBSERVE_PAUSE_SECONDS: u64 = 15;
/// How many Waits in a row before *nothing is happening forever* becomes terminal. A Wait
/// spends no attempt budget, so without this a permanently-walled Run never stops. Twelve is
/// six hours at the recorded limit sleep; Run 2's real three-hour wall produced five
/// consecutive Waits and then cleared, so the bound has to sit well above it.
pub const CONSECUTIVE_WAITS: usize = 12;

/// **Eight states, and none of them is `running`.** A SIGKILLed supervisor would sit in
/// `running` forever, which is why the roster observes liveness for itself instead.
///
/// `Blocked` is the eighth, and it is a supervisor state rather than a `Verdict` variant for
/// the reason ADR-0006 gives: a Blocker is a fact about the world, in the same family as
/// `RateLimited`, and not a judgement about the work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum State {
    Dispatched,
    RateLimited,
    Died,
    Completed,
    Uncorroborated,
    Unobserved,
    Exhausted,
    Blocked,
}

impl State {
    fn as_str(self) -> &'static str {
        match self {
            State::Dispatched => "dispatched",
            State::RateLimited => "rate_limited",
            State::Died => "died",
            State::Completed => "completed",
            State::Uncorroborated => "uncorroborated",
            State::Unobserved => "unobserved",
            State::Exhausted => "exhausted",
            State::Blocked => "blocked",
        }
    }
}

/// The writable record. **Private, and this module is its only writer.**
///
/// Every environment-varying condition is forced at construction (`E0063`), so every later path
/// — every attempt, every `--resume`, every status read — takes its conditions from here rather
/// than from the environment it happens to be running in.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RunRecord {
    run_id: String,
    created_at: String,
    state: State,
    job: Job,
    plugin_dir: String,
    repo_path: String,
    worktree: String,
    session_id: String,
    claude_bin: String,
    model: Option<String>,
    denied_tools: Vec<String>,
    hostname: String,
    attempt_budget: usize,
    limit_sleep_seconds: u64,
    supervisor_pid: u32,
    supervisor_identity: Option<String>,
    /// Private even inside the private type. The only mutator appends — there is no
    /// `set_attempts` and no `&mut Vec<_>` getter, so *load a stale copy, then overwrite the
    /// whole list* is not expressible even from in here.
    attempts: Vec<Attempt>,
    /// What the human cleared while this Run stood at a Blocker, newest last. The same
    /// privacy-and-append shape as `attempts`. `serde(default)` because absent genuinely is
    /// empty — a record written before the `cleared` verb existed recorded none, and the
    /// Blocked Run already on disk is exactly the one the verb targets — unlike the forced
    /// dispatch-time conditions, where absence means a different environment.
    #[serde(default)]
    clearances: Vec<Clearance>,
}

impl RunRecord {
    fn attempts(&self) -> &[Attempt] {
        &self.attempts
    }

    fn push_attempt(&mut self, attempt: Attempt) {
        self.attempts.push(attempt);
    }

    fn clearances(&self) -> &[Clearance] {
        &self.clearances
    }

    fn push_clearance(&mut self, clearance: Clearance) {
        self.clearances.push(clearance);
    }

    fn load(path: &Path) -> Result<Self, Refusal> {
        let raw = world::read_to_string(path).map_err(Refusal::saying)?;
        serde_json::from_str(&raw)
            .map_err(|e| Refusal::saying(format!("{}: unreadable Run state: {e}", path.display())))
    }

    fn save(&self, path: &Path) -> Result<(), Refusal> {
        let body = serde_json::to_string_pretty(self)
            .map_err(|e| Refusal::saying(format!("could not serialise Run state: {e}")))?;
        world::write_atomic(path, &(body + "\n")).map_err(Refusal::saying)
    }
}

/// What a finished call hands back to `cli`. The record itself never leaves this module.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub run_id: String,
    pub state: &'static str,
    /// True when `resume` found a Run already at a terminal state and started nothing. The name
    /// reads narrower than the flag now is — it covers `Exhausted` as well as `Completed`, see
    /// the guard in `resume` — so nothing may render it as the word *completed*. `state` beside
    /// it carries which terminal state was actually found, and `cli` prints that.
    pub already_completed: bool,
}

// --- the dispatch lock ---------------------------------------------------------------------

/// The lock key: the **target repo plus the branch**, never a filesystem path. Two worktrees of
/// one repo must not pass each other silently, and one declared clone per repo is what makes
/// this sound.
///
/// Every branch this project dispatches contains a slash, so the raw key would name a directory
/// that does not exist and the open would fail before any lock was attempted. Sanitising is
/// what keeps the key one file directly under `~/.grind/locks/`.
pub fn lock_key(target_repo: &str, branch: &str) -> String {
    let (owner, name) = job::repo_owner_and_name(target_repo);
    format!("{owner}-{name}-{branch}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn lock_path(home: &Path, target_repo: &str, branch: &str) -> PathBuf {
    job::locks_dir(home).join(lock_key(target_repo, branch))
}

/// Take the lock, or say which of the two things went wrong.
///
/// **The handle is returned rather than dropped**: `File::try_lock` releases on drop, so a
/// handle owned by this function would evaporate seconds into a Run that lasts hours, and the
/// guarantee is that the *kernel* releases it when the holder dies — which needs a holder that
/// is still holding.
pub fn take_lock(home: &Path, target_repo: &str, branch: &str) -> Result<LockHandle, Refusal> {
    let path = lock_path(home, target_repo, branch);
    if let Err(e) = world::create_dir_all(&job::locks_dir(home)) {
        return Err(Refusal::saying(format!(
            "could not reach the lock directory: {e}"
        )));
    }
    match world::try_lock(&path) {
        TryLock::Acquired(handle) => Ok(handle),
        // A collision: a live supervisor holds this branch. Named neutrally rather than as
        // *another Run*, because for `resume` and `cleared` the holder can be the named
        // Run's own supervisor, still running through a rate-limit sleep — and *another Run*
        // sends the human hunting the roster for a collision that does not exist.
        TryLock::WouldBlock => Err(Refusal::saying(format!(
            "a running supervisor already holds {target_repo} {branch} on this host"
        ))),
        // Could not determine. Never folded into the refusal and never into proceeding —
        // collapsing the two relocates the exact bug the three-valued observation removes.
        TryLock::Failed(why) => Err(Refusal::saying(format!(
            "could not determine whether {target_repo} {branch} is held: {why}"
        ))),
    }
}

// --- dispatch -------------------------------------------------------------------------------

/// The only path that starts a Run. Grind never selects; a human names a Job.
pub fn dispatch(reference: &str) -> Result<Outcome, Refusal> {
    let home = world::home().ok_or_else(|| Refusal::saying("$HOME is unset"))?;
    let asked = job::parse_reference(reference)?;

    let mut argv = vec![
        "gh".to_string(),
        "issue".to_string(),
        "view".to_string(),
        asked.number.to_string(),
        "--json".to_string(),
        "number,title,body,url,labels,state".to_string(),
    ];
    if let Some(repo) = &asked.repo {
        argv.push("--repo".to_string());
        argv.push(repo.clone());
    }
    let issue = world::run(&argv, None);
    if issue.code != Some(0) {
        return Err(Refusal::saying(format!(
            "could not read the Job issue: {}",
            issue.stderr.lines().next().unwrap_or("no output")
        )));
    }
    let job = job::from_issue_json(&issue.stdout)?;
    world::print_line(&format!("Job #{}: {}", job.issue, job.title));

    // Presence only, local, free, no network — a host missing its clone or its `claude` fails
    // at second zero rather than three hours in.
    refuse_unless_host_ready(&home, &job)?;

    let repo_path = job::repo_path(&home, &job.target_repo);
    let plugin_dir = job::plugin_dir(&home, &job.plugin);
    let claude_bin = job::claude_bin(&home);

    // Taken before the record is written, and held for the whole process.
    let _lock = take_lock(&home, &job.target_repo, &job.branch)?;

    let worktree = adopt_or_create_worktree(&repo_path, &job.branch)?;
    let dirty = world::run(&words(&["git", "status", "--porcelain"]), Some(&worktree));
    if dirty.code != Some(0) {
        return Err(Refusal::saying(format!(
            "could not read the worktree's status: {}",
            dirty.stderr.lines().next().unwrap_or("no output")
        )));
    }
    if job::is_dirty(&dirty.stdout) {
        return Err(Refusal::saying(format!(
            "the worktree at {} is dirty, so nothing is dispatched onto it:\n{}",
            worktree.display(),
            dirty.stdout.trim()
        )));
    }
    // Four impure calls and no branching in any of them; `job::reachability` is the whole of
    // the decision. The fetch is network, so it sits outside the *presence only, local, free,
    // no network* comment that scopes the host-readiness check above — and it is what makes the
    // same Job produce the same answer on a laptop and on a box.
    let fetched = world::run(&words(&["git", "fetch"]), Some(&repo_path));
    let head = world::run(&words(&["git", "rev-parse", "HEAD"]), Some(&worktree));
    let contains = world::run(
        &[
            "git".to_string(),
            "merge-base".to_string(),
            "--is-ancestor".to_string(),
            job.handoff_sha.clone(),
            "HEAD".to_string(),
        ],
        Some(&worktree),
    );
    let reverse = world::run(
        &[
            "git".to_string(),
            "merge-base".to_string(),
            "--is-ancestor".to_string(),
            "HEAD".to_string(),
            job.handoff_sha.clone(),
        ],
        Some(&worktree),
    );
    match job::reachability(
        fetched.code == Some(0),
        contains.code,
        reverse.code == Some(0),
        &head.stdout,
        &job.handoff_sha,
    ) {
        job::Reachability::Proceed => {}
        job::Reachability::Note(note) => world::print_line(&format!("  note: {note}")),
        job::Reachability::Refuse(refusal) => return Err(refusal),
    }

    // A Run handed a path to nothing cannot invent requirements, satisfy them, and open a green
    // PR. Presence only: the Anchor's **shape** is never checked — no R-IDs, no readiness field
    // — because an admission check must not arrive through the back door of an admission rule.
    // It cannot live in `refuse_unless_host_ready`, which runs before the worktree exists.
    if !world::exists(&worktree.join(&job.anchor)) {
        return Err(Refusal::saying(format!(
            "the Anchor artifact `{}` is not in the worktree at {}, so nothing is dispatched \
             onto it",
            job.anchor,
            worktree.display()
        )));
    }

    let run_id = format!(
        "{}-{}-{}",
        world::now_stamp(),
        job::repo_owner_and_name(&job.target_repo).1,
        job.issue
    );
    let pid = world::pid();
    let mut record = RunRecord {
        run_id: run_id.clone(),
        created_at: world::now_iso(),
        state: State::Dispatched,
        plugin_dir: plugin_dir.display().to_string(),
        repo_path: repo_path.display().to_string(),
        worktree: worktree.display().to_string(),
        session_id: fresh_session_id(&run_id, pid),
        claude_bin: claude_bin.display().to_string(),
        model: job.model.clone(),
        denied_tools: DENIED_TOOLS.iter().map(|glob| glob.to_string()).collect(),
        hostname: world::hostname().unwrap_or_else(|| "unknown-host".to_string()),
        attempt_budget: ATTEMPT_BUDGET,
        limit_sleep_seconds: LIMIT_SLEEP_SECONDS,
        supervisor_pid: pid,
        supervisor_identity: recorded_identity(pid),
        attempts: Vec::new(),
        clearances: Vec::new(),
        job,
    };
    let run_dir = job::runs_dir(&home).join(&run_id);
    world::create_dir_all(&run_dir).map_err(Refusal::saying)?;
    record.save(&record_path(&run_dir))?;

    point_at_this_host(&record);

    say(
        &run_dir,
        &format!("  plugin pinned to {}", record.plugin_dir),
    );
    say(
        &run_dir,
        &format!(
            "  model {}",
            record
                .model
                .as_deref()
                .unwrap_or("(session default — unpinned)")
        ),
    );
    say(&run_dir, &format!("  claude {}", record.claude_bin));
    say(&run_dir, &format!("  run {run_id}"));

    supervise(&mut record, &run_dir)
}

/// The identity to record beside a pid. `Absent` and `Unobservable` are the same fact at *this*
/// end — nothing was read that a later reading could be compared against — and `supervisor_here`
/// already says so out loud when it meets the `None`. Written out rather than collapsed by an
/// `ok()`, because the collapse at the *reading* end is what #14 was.
fn recorded_identity(pid: u32) -> Option<String> {
    match crate::observe::process_start_stamp(&world::ps_start_stamp(pid)) {
        Observed::Present(stamp) => Some(stamp),
        Observed::Absent | Observed::Unobservable(_) => None,
    }
}

/// Re-enter a Run that died, **reading every condition from the record** rather than the
/// environment.
pub fn resume(run_id: &str) -> Result<Outcome, Refusal> {
    let home = world::home().ok_or_else(|| Refusal::saying("$HOME is unset"))?;
    let run_dir = job::runs_dir(&home).join(run_id);
    let path = record_path(&run_dir);
    if !world::exists(&path) {
        return Err(Refusal::saying(format!("no Run `{run_id}` on this host")));
    }
    let keyed = RunRecord::load(&path)?;

    // A Run being re-entered by hand holds its branch exactly as a dispatched one does, and the
    // original holder's lock died with its supervisor.
    let _lock = take_lock(&home, &keyed.job.target_repo, &keyed.job.branch)?;

    // Re-loaded under the lock, the same order `cleared` follows: `save` writes the whole
    // record, so the copy it writes must have been read while no other writer could run — a
    // clearance recorded between the first load and the lock would otherwise be erased by the
    // save below. The first load serves only the existence refusal and the lock key, and the
    // guard below runs on this fresh copy so a Run another supervisor finished in that window
    // is not re-entered.
    let mut record = RunRecord::load(&path)?;

    // Option (a) from the review: widen the guard rather than add a second branch below it.
    // `supervise` attempts before it ever consults `policy::next` (`run_one_attempt` runs, then
    // the record is saved, then the loop asks what's next) — so `Completed` is not the only
    // state a resume must not walk into. `Exhausted` means the record already holds a Run at
    // its recorded attempt budget; resuming it would spend a ninth attempt with no `policy`
    // check standing between `resume` and `run_one_attempt` to stop it, breaking *attempt N of
    // M, with M from the record* the same way the Completed case would.
    //
    // `Unobserved` and `Uncorroborated` are deliberately left resumable. Neither stopped because
    // the attempt budget ran out — `Uncorroborated` always stops regardless of budget, and
    // `Unobserved` stops on a fault in Grind's own eyes, not the Job's — so resuming one does
    // not overspend anything the record promises. For `Unobserved` in particular, the transient
    // `policy`'s new reobserve pause exists for may simply have cleared since; refusing to
    // resume it would remove the only recovery path for exactly the fault this fix's other half
    // gives more time to clear.
    //
    // **Keyed on the number, not on the state word.** A Run whose last working Attempt landed
    // `Uncorroborated` or `Unobserved` at 8 of 8 stops in a resumable state at its budget, and
    // the state-word guard waves it straight into `run_one_attempt` — with no `policy` check
    // between `resume` and the child, and a recorded Attempt in this project costing $7–$37.
    // Each further resume spends another and stops in the same state. Refusing on the count
    // keeps the case the comment above argues for resumable, and refuses only the overspend.
    if matches!(record.state, State::Completed | State::Exhausted)
        || attempt::working(record.attempts()) >= record.attempt_budget
    {
        return Ok(Outcome {
            run_id: record.run_id.clone(),
            state: record.state.as_str(),
            already_completed: true,
        });
    }

    let pid = world::pid();
    record.supervisor_pid = pid;
    record.supervisor_identity = recorded_identity(pid);
    record.save(&path)?;

    supervise(&mut record, &run_dir)
}

/// Record what the human cleared on a Blocked Run. **A one-shot supervisor process** — the
/// writer ruling holds because this verb *is* the supervisor, exactly as `resume` is — and it
/// records only; `resume` is the separate act that spends. Nothing here changes `state`,
/// posts a comment, or starts anything.
///
/// The order is **load-for-key → lock → re-load → validate → append → save**. The first load
/// exists only to learn the lock key (the target repo and branch live on the record); the
/// copy that reaches `save` is read under the lock, because `save` writes the whole record —
/// a copy read while a resumed supervisor was still running could hand back a list missing
/// that supervisor's appended Attempts, which nothing can rebuild.
pub fn cleared(run_id: &str, note: &str) -> Result<(), Refusal> {
    let home = world::home().ok_or_else(|| Refusal::saying("$HOME is unset"))?;
    let run_dir = job::runs_dir(&home).join(run_id);
    let path = record_path(&run_dir);
    if !world::exists(&path) {
        return Err(Refusal::saying(format!("no Run `{run_id}` on this host")));
    }
    let keyed = RunRecord::load(&path)?;
    let _lock = take_lock(&home, &keyed.job.target_repo, &keyed.job.branch)?;
    let mut record = RunRecord::load(&path)?;
    record_clearance(&mut record, note, world::now_iso())?;
    record.save(&path)?;
    // Logged from the row that was actually stored, so the account and the record cannot
    // say two different things about one note.
    if let Some(row) = record.clearances().last() {
        say(&run_dir, &format!("  clearance recorded: {}", row.note));
    }
    Ok(())
}

/// The pure half of `cleared`: the refusals and the append, over an already-loaded record.
/// Refusing leaves in the incoherent-input register — naming the actual state is a fact
/// about coherence, never a health verdict (ADR-0003).
fn record_clearance(record: &mut RunRecord, note: &str, at: String) -> Result<(), Refusal> {
    let note = note.trim();
    if note.is_empty() {
        return Err(Refusal::saying(
            "an empty note clears nothing — say what changed",
        ));
    }
    if record.state != State::Blocked {
        return Err(Refusal::saying(format!(
            "Run `{}` is {}, not blocked — a clearance records what changed for a Run a \
             Blocker stopped",
            record.run_id,
            record.state.as_str()
        )));
    }
    record.push_clearance(Clearance {
        cleared_at: at,
        note: note.to_string(),
    });
    Ok(())
}

/// What `resume --all` started, and what it declined to. **Never what any Run concluded**:
/// N detached children have neither a single outcome nor a single verdict-derived exit code,
/// and inventing one would be a summary over N Runs.
#[derive(Debug, Default)]
pub struct Reentry {
    pub started: Vec<String>,
    pub skipped: Vec<(String, String)>,
}

/// Re-enter every Run on this host that a restart **cut off**.
///
/// *Cut off* is `Dispatched`, `RateLimited` or `Died` **with a stale supervisor**. Liveness
/// needs nothing new: after a reboot every recorded pid is stale by construction, so the
/// existing process-identity check answers it, and the edge — a fast reboot plus a pid collision
/// plus a colliding start stamp — fails toward *declining to re-enter*, which is the safe
/// direction.
///
/// **The stopped are never re-entered**: `Uncorroborated`, `Unobserved` and `Blocked` are
/// deliberate decisions, and overriding one at the moment nobody is watching is the failure this
/// path is most able to cause. `Unobserved` is the arguable one and is excluded on purpose —
/// re-entering it means a blind Run mutating a branch.
///
/// **Concurrent, never serial** — one detached child per kept Run, each taking its own dispatch
/// lock, so genuinely independent Runs proceed in parallel and a second child on one branch gets
/// the existing `WouldBlock` refusal for free. Serial re-entry would be ordering, and ordering is
/// the human's act.
pub fn resume_all() -> Result<Reentry, Refusal> {
    let home = world::home().ok_or_else(|| Refusal::saying("$HOME is unset"))?;
    let exe = world::current_exe()
        .ok_or_else(|| Refusal::saying("could not find the `grind` binary to re-enter with"))?;
    let mut report = Reentry::default();

    for run_dir in world::list_dir(&job::runs_dir(&home)) {
        let Some(run_id) = run_dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Ok(record) = RunRecord::load(&record_path(&run_dir)) else {
            // A record written before this build lacks fields the base now forces, and there is
            // deliberately no migration read path. Skipping is the only honest answer.
            let why = "its record could not be read".to_string();
            say(&run_dir, &format!("  skipped at boot: {why}"));
            report.skipped.push((run_id.to_string(), why));
            continue;
        };
        if !matches!(
            record.state,
            State::Dispatched | State::RateLimited | State::Died
        ) {
            continue;
        }
        let supervisor = crate::view::supervisor_here(
            record.supervisor_identity.as_deref(),
            &crate::observe::process_start_stamp(&world::ps_start_stamp(record.supervisor_pid)),
        );
        match supervisor {
            // The one reading that means *a restart cut this Run off*.
            Observed::Present(false) => {}
            // Still running under the recorded identity: not cut off, and not this path's
            // business.
            Observed::Present(true) => continue,
            // **Could not tell.** `ps -p <pid> -o lstart=` is a procps/BSD spelling busybox does
            // not implement, and this is the one path that *acts* on the reading rather than
            // printing it. Skipping is the safe direction, and it is reported rather than
            // silent: a host where every reading is blind would otherwise re-enter nothing and
            // say nothing about why.
            other => {
                let why = format!(
                    "whether its supervisor is still running could not be read ({})",
                    other
                        .reason()
                        .map(|r| r.to_string())
                        .unwrap_or_else(|| "no reason".to_string())
                );
                say(&run_dir, &format!("  skipped at boot: {why}"));
                report.skipped.push((run_id.to_string(), why));
                continue;
            }
        }
        // `resume` runs no precondition checks and the new refusals live in `dispatch`, so boot
        // re-entry inherits none of them. A machine that just rebooted is exactly where someone
        // was mid-edit, and this is the one path that starts an agent with nobody present.
        // A **skip** rather than a refusal, because one unre-enterable Run must not stop the
        // others.
        let worktree = PathBuf::from(&record.worktree);
        let dirty = world::run(&words(&["git", "status", "--porcelain"]), Some(&worktree));
        if dirty.code != Some(0) || job::is_dirty(&dirty.stdout) {
            let why = format!(
                "its worktree at {} is dirty or unreadable",
                worktree.display()
            );
            say(&run_dir, &format!("  skipped at boot: {why}"));
            report.skipped.push((run_id.to_string(), why));
            continue;
        }
        match world::spawn_detached(
            &[
                exe.display().to_string(),
                "resume".to_string(),
                run_id.to_string(),
            ],
            &resume_log_path(&run_dir),
        ) {
            Ok(_) => {
                say(&run_dir, "  re-entered at boot");
                report.started.push(run_id.to_string());
            }
            Err(why) => {
                say(&run_dir, &format!("  could not re-enter at boot: {why}"));
                report.skipped.push((run_id.to_string(), why));
            }
        }
    }
    Ok(report)
}

// --- the loop ---------------------------------------------------------------------------------

fn supervise(record: &mut RunRecord, run_dir: &Path) -> Result<Outcome, Refusal> {
    let path = record_path(run_dir);
    let budget = Budget {
        attempts: record.attempt_budget,
        limit_sleep: Duration::from_secs(record.limit_sleep_seconds),
        reobservations: REOBSERVATIONS,
        reobserve_pause: Duration::from_secs(REOBSERVE_PAUSE_SECONDS),
        consecutive_waits: CONSECUTIVE_WAITS,
    };
    let worktree = PathBuf::from(&record.worktree);
    let mut reobservations = 0usize;
    // One reading per working Attempt, newest last: *did this Attempt fail to advance*. Held
    // for the process rather than recorded, because the record carries no per-Attempt commit
    // count — a fresh process needs two more working Attempts before a Blocker can fire, which
    // is the safe direction.
    let mut stalls: Vec<Observed<bool>> = Vec::new();
    let mut commits_before: Option<Observed<u64>> = None;
    let mut working_seen = attempt::working(record.attempts());

    loop {
        // The first pass has no attempt to classify, so the loop starts by attempting. But a
        // crash *inside* Attempt 1 leaves a record with no attempts while the session already
        // carries transcript lines — re-dispatching there would start the Job over under the
        // same session id and lose what the crashed session did. Dispatch therefore means an
        // untouched Run: no attempts *and* an empty transcript. A created-but-empty session
        // still re-dispatches, because there is nothing in it to lose.
        let mode = entry_mode(record.attempts(), transcript_lines(record));
        run_one_attempt(record, run_dir, &worktree, mode)?;
        record.save(&path)?;

        let stop = loop {
            let observation =
                crate::view::observe_fresh(&worktree, &record.job.handoff_sha, world::now_iso());
            // The progress window advances once per working Attempt, and only ever forward.
            let working_now = attempt::working(record.attempts());
            if working_now > working_seen {
                working_seen = working_now;
                stalls.push(policy::stalled(
                    commits_before.as_ref(),
                    &observation.commits_ahead,
                ));
                commits_before = Some(observation.commits_ahead.clone());
            }
            let signals = decide::signals_of(&observation);
            let promised = record.attempts().last().is_some_and(|a| a.done_promise);
            let verdict = decide::verdict(&signals, promised);
            announce(run_dir, record, &observation, &verdict);

            // A Blocker stops at once rather than spending the rest of the budget against a
            // wall the Run cannot move — and it never overrides a decided Run: where the
            // artifacts agree, the Run finished, denials and all.
            if !matches!(verdict, Verdict::Completed)
                && let Some(stop) = policy::blocker(record.attempts(), &stalls)
            {
                break Some(stop);
            }

            match policy::next(
                record.attempts(),
                &verdict,
                &observation.checks_red,
                reobservations,
                &budget,
            ) {
                Next::Reobserve(pause) => {
                    reobservations += 1;
                    say(
                        run_dir,
                        &format!(
                            "    a signal could not be observed — sleeping {}s before looking again",
                            pause.as_secs()
                        ),
                    );
                    world::sleep(pause);
                    continue;
                }
                Next::SpendCiBudget => {
                    reobservations = 0;
                    say(
                        run_dir,
                        "    decided, and a check came back red — spending the one CI budget",
                    );
                    run_one_attempt(record, run_dir, &worktree, Mode::CiBabysit)?;
                    record.save(&path)?;
                    continue;
                }
                Next::SleepThenReenter(nap) => {
                    reobservations = 0;
                    record.state = State::RateLimited;
                    record.save(&path)?;
                    say(
                        run_dir,
                        &format!(
                            "    rate limited — sleeping {}s, then re-entering",
                            nap.as_secs()
                        ),
                    );
                    world::sleep(nap);
                    break None;
                }
                Next::Reenter => {
                    reobservations = 0;
                    record.state = State::Died;
                    record.save(&path)?;
                    say(
                        run_dir,
                        "    ended without a DONE promise — re-entering at the stage that died",
                    );
                    break None;
                }
                Next::Stop(stop) => break Some(stop),
            }
        };

        if let Some(stop) = stop {
            record.state = match &stop {
                Stop::Completed => State::Completed,
                Stop::Uncorroborated(_) => State::Uncorroborated,
                Stop::Unobserved(_) => State::Unobserved,
                Stop::Exhausted => State::Exhausted,
                Stop::Blocked(_) => State::Blocked,
            };
            record.save(&path)?;
            if let Stop::Unobserved(blind) = &stop {
                for said in blind {
                    say(run_dir, &format!("    could not observe {said}"));
                }
            }
            // Resumable, because it never spent the budget: the world changed, not the number.
            // The repair is two verbs in order: `cleared` records what changed, `resume`
            // spends — Grind never chooses to spend an Attempt, so the acts stay separate.
            if let Stop::Blocked(what) = &stop {
                say(
                    run_dir,
                    &format!(
                        "    stopped for a human — {what} was refused twice with no progress; {}",
                        crate::render::repair_hint(&record.run_id)
                    ),
                );
            }
            report_to_the_job_issue(run_dir, record);
            return Ok(Outcome {
                run_id: record.run_id.clone(),
                state: record.state.as_str(),
                already_completed: false,
            });
        }
    }
}

fn run_one_attempt(
    record: &mut RunRecord,
    run_dir: &Path,
    worktree: &Path,
    mode: Mode,
) -> Result<(), Refusal> {
    let n = record.attempts().len() + 1;
    let conditions = Conditions {
        claude_bin: &record.claude_bin,
        session_id: &record.session_id,
        // Read from the record on every attempt. **Never re-resolved**: an eight-attempt Run
        // spans hours of rate-limit sleeps, and a version changing mid-Run is silent.
        plugin_dir: &record.plugin_dir,
        model: record.model.as_deref(),
    };
    let invocation: Invocation = match mode {
        Mode::Dispatch => attempt::dispatch(&conditions, &record.job),
        // The latest clearance rides every Resume re-entry — a fact about the world does
        // not expire — and only Resume: CiBabysit bounds itself to one reaction.
        Mode::Resume => attempt::resume(&conditions, record.clearances().last()),
        Mode::CiBabysit => attempt::ci_babysit(&conditions),
    };

    let started_at = world::now_iso();
    say(run_dir, &format!("  [{started_at}] attempt {n} ({mode}) …"));

    // **Taken before the child spawns, and this is what makes the pair belong to this Attempt.**
    // The transcript is keyed on `record.session_id`, which `fresh_session_id` fixes for the
    // Run's life, and every attempt after the first resumes that session and appends to the same
    // `.jsonl`. Counting the whole file on Attempt N therefore counted Attempts 1..N — and
    // `render::fanout_totals` sums those pairs, so a Run fanning out to 2 agents on each of 3
    // attempts recorded (2,2), (4,4), (6,6) and published *12 spawned, 12 returned*. R51 says
    // per Attempt; the suffix is what says it.
    let already_written = transcript_lines(record);

    let raw = attempt::run(
        &invocation,
        worktree,
        &run_dir.join(format!("attempt-{n}.prompt.txt")),
        &run_dir.join(format!("attempt-{n}.stdout.json")),
        &run_dir.join(format!("attempt-{n}.stderr.log")),
    )
    .map_err(|reason| Refusal::saying(reason.to_string()))?;

    // Gathered before `push_attempt`: the attempt list is append-only with no mutating
    // accessor, so this is the one wire between transcript reading and record building — and it
    // runs in the direction the topology already allows.
    let classified = raw
        .classify(n, mode, &started_at, &world::now_iso())
        .with_fanout(fanout_of(record, already_written));
    record.push_attempt(classified);
    Ok(())
}

/// Dispatch or resume, from the two facts a re-entering supervisor can see: the attempt list
/// and how much transcript the recorded session already holds.
///
/// `attempts.is_empty()` alone is not *fresh*: a crash inside Attempt 1 — after `claude`
/// created the session named by `--session-id`, before any Attempt was recorded — leaves an
/// empty list over a session that already did work. Re-dispatching there would start the Job
/// over and lose that work, so Dispatch requires the transcript to be empty too. Absence of a
/// transcript file reads as zero, which is the honest reading: nothing was written yet.
fn entry_mode(attempts: &[Attempt], transcript_lines: usize) -> Mode {
    if attempts.is_empty() && transcript_lines == 0 {
        Mode::Dispatch
    } else {
        Mode::Resume
    }
}

/// How much of the Run's transcript exists right now, in lines. A transcript that is not there
/// yet is zero rather than a refusal: the first Attempt of a Run has nothing to skip, and that
/// is the same answer.
///
/// Read while the child is not running, so the file is quiescent and the last line is whole.
fn transcript_lines(record: &RunRecord) -> usize {
    let Some(home) = world::home() else {
        return 0;
    };
    let transcript = crate::view::transcript_path(&home, &record.worktree, &record.session_id);
    match world::read_to_string(&transcript) {
        Ok(text) => text.lines().count(),
        Err(_) => 0,
    }
}

/// The Attempt's fan-out arithmetic, over **the lines this Attempt appended** and no earlier
/// ones. `world` reads the file; a pure counter reads the text.
fn fanout_of(record: &RunRecord, already_written: usize) -> Observed<(u64, u64)> {
    let Some(home) = world::home() else {
        return Observed::Unobservable(Reason::saying("$HOME is unset"));
    };
    let transcript = crate::view::transcript_path(&home, &record.worktree, &record.session_id);
    match world::read_to_string(&transcript) {
        Ok(text) => crate::view::fanout_since(&text, already_written),
        Err(said) => Observed::Unobservable(Reason::saying(&format!(
            "the transcript could not be read: {said}"
        ))),
    }
}

fn announce(run_dir: &Path, record: &RunRecord, observation: &Observation, verdict: &Verdict) {
    let last = record.attempts().last();
    let outcome = match last {
        Some(a) if a.done_promise => "DONE promised".to_string(),
        Some(a) => a.subtype.clone().unwrap_or_else(|| "ended".to_string()),
        None => "no attempt".to_string(),
    };
    let cost = last.and_then(|a| a.total_cost_usd).unwrap_or(0.0);
    say(
        run_dir,
        &format!(
            "    -> {outcome} | stage={} | commits={} | cost=${cost:.2} | {verdict:?}",
            decide::furthest_stage(observation),
            observation.commits_ahead,
        ),
    );
}

// --- dispatch's own steps ------------------------------------------------------------------

fn refuse_unless_host_ready(home: &Path, job: &Job) -> Result<(), Refusal> {
    for item in job::dispatch_subset() {
        let found = check_presence(home, job, item.check);
        match found {
            Observed::Present(ItemOutcome::Satisfied(_))
            | Observed::Present(ItemOutcome::Unchecked(_)) => {}
            Observed::Present(ItemOutcome::Unsatisfied(said)) => {
                return Err(Refusal::saying(format!("host: {}: {said}", item.name)));
            }
            Observed::Absent => {
                return Err(Refusal::saying(format!("host: {}: absent", item.name)));
            }
            Observed::Unobservable(reason) => {
                return Err(Refusal::saying(format!("host: {}: {reason}", item.name)));
            }
        }
    }
    Ok(())
}

/// The presence half of the dispatch-depth item list — local, free, no network, run by
/// `refuse_unless_host_ready` before a child is ever spawned. **`cli` does not share this**:
/// doctor keeps its own mapping in its own `check`, because doctor's depth (relaxed checks,
/// declared-clone context) differs per item, and one function serving both would drag
/// dispatch's no-network shape into every doctor row.
fn check_presence(home: &Path, job: &Job, check: job::Check) -> Observed<ItemOutcome> {
    use crate::observe as obs;
    match check {
        job::Check::DeclaredClone => obs::declared_clone(
            world::is_dir(&job::repo_path(home, &job.target_repo)),
            None,
            &job.target_repo,
        ),
        job::Check::ClaudeBinary => {
            obs::claude_binary(world::is_executable(&job::claude_bin(home)), None)
        }
        job::Check::GitVersionFloor => obs::git_version_floor(
            &world::run(&words(&["git", "--version"]), None),
            job::GIT_VERSION_FLOOR,
        ),
        job::Check::OnPath(tool) => obs::on_path(
            tool,
            &world::run(&words(&["sh", "-c", &format!("command -v {tool}")]), None),
        ),
        job::Check::PluginInstalled => {
            obs::plugin_installed(world::is_dir(&job::plugin_dir(home, &job.plugin)))
        }
        other => obs::unchecked(&format!("{other:?} is not a dispatch-depth check")),
    }
}

fn adopt_or_create_worktree(repo_path: &Path, branch: &str) -> Result<PathBuf, Refusal> {
    let listed = world::run(
        &words(&["git", "worktree", "list", "--porcelain"]),
        Some(repo_path),
    );
    if listed.code != Some(0) {
        return Err(Refusal::saying(format!(
            "could not list the declared clone's worktrees: {}",
            listed.stderr.lines().next().unwrap_or("no output")
        )));
    }
    if let Some(adopted) = job::adopt_worktree(&listed.stdout, branch) {
        world::print_line(&format!(
            "  adopting existing worktree: {}",
            adopted.display()
        ));
        return Ok(adopted);
    }
    let wanted = job::worktree_to_create(repo_path, branch);
    // A Job's declared branch is normally created *by* its Run, so at dispatch it usually
    // exists nowhere yet: plain `add` demanded an existing ref and refused every
    // fresh-branch Job (`invalid reference`, issue #81). `-b` makes the create path
    // create-the-branch-and-its-worktree; when the ref already exists but holds no
    // worktree, plain `add` checks it out exactly as before.
    let ref_exists = world::run(
        &words(&[
            "git",
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ]),
        Some(repo_path),
    );
    let mut argv = vec!["git".to_string(), "worktree".to_string(), "add".to_string()];
    // `-b` binds to the next argument, so the two forms order differently:
    // `add -b <branch> <path>` against `add <path> <branch>`.
    if ref_exists.code != Some(0) {
        argv.push("-b".to_string());
        argv.push(branch.to_string());
        argv.push(wanted.display().to_string());
    } else {
        argv.push(wanted.display().to_string());
        argv.push(branch.to_string());
    }
    let made = world::run(&argv, Some(repo_path));
    if made.code != Some(0) {
        return Err(Refusal::saying(format!(
            "could not create a worktree for {branch}: {}",
            made.stderr.lines().next().unwrap_or("no output")
        )));
    }
    world::print_line(&format!("  created worktree: {}", wanted.display()));
    Ok(wanted)
}

/// **The account that leaves the host**, posted on the Job issue at every terminal state.
///
/// Not the PR: the PR body is entirely the Run's, and supervisor prose beside the Run's own
/// narrative reads as a verdict on the work.
///
/// **Appended, never edited.** A Blocked Run a human clears and resumes reaches a terminal
/// state twice, and two comments are the honest account.
///
/// **Best-effort.** On failure, log and move on — no retry loop, and **never a verdict change**.
/// A Run that finished must not become `unobserved` because GitHub was down at 04:00.
fn report_to_the_job_issue(run_dir: &Path, record: &RunRecord) {
    let Some(home) = world::home() else {
        say(
            run_dir,
            "  note: $HOME is unset, so nothing was posted on the Job issue",
        );
        return;
    };
    // The **same construction** the Handback uses, so the two renderings cannot be fed
    // different lists.
    let Some(facts) = crate::view::gather(&home, &record.run_id) else {
        say(run_dir, "  note: could not compose the terminal comment");
        return;
    };
    let posted = world::run(
        &[
            "gh".to_string(),
            "issue".to_string(),
            "comment".to_string(),
            record.job.issue.to_string(),
            "--repo".to_string(),
            record.job.target_repo.clone(),
            "--body".to_string(),
            crate::render::job_comment(&facts),
        ],
        None,
    );
    if posted.code != Some(0) {
        say(
            run_dir,
            "  note: could not post the terminal comment on the Job issue",
        );
    }
}

/// The dispatch comment, and nothing else. Grind adds and never classifies (ADR-0012): no
/// label, no assignee, no project, no milestone, on any repo. It is not allowed to stop a Run
/// either — the Job issue is a pointer, and a pointer that failed to update is not worth
/// abandoning a dispatched Run over.
fn point_at_this_host(record: &RunRecord) {
    let number = record.job.issue.to_string();
    let repo = record.job.target_repo.clone();
    // The only thing that travels between hosts is a pointer, and it travels on the Job issue.
    let body = format!(
        "Dispatched as Run `{}` on `{}`.\n\nRun state lives on that host at `~/.grind/runs/{}/`.",
        record.run_id, record.hostname, record.run_id
    );
    let commented = world::run(
        &[
            "gh".to_string(),
            "issue".to_string(),
            "comment".to_string(),
            number,
            "--repo".to_string(),
            repo,
            "--body".to_string(),
            body,
        ],
        None,
    );
    if commented.code != Some(0) {
        world::print_line("  note: could not comment the run id on the Job issue");
    }
}

fn words(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

fn record_path(run_dir: &Path) -> PathBuf {
    run_dir.join("run.json")
}

/// `~/.grind/runs/<run-id>/supervisor.log` — beside the record and the raw attempt files.
///
/// Still Run state, so still never committed: it lives outside every checkout, which holds
/// structurally rather than by a `.gitignore` line.
fn log_path(run_dir: &Path) -> PathBuf {
    run_dir.join("supervisor.log")
}

/// `~/.grind/runs/<run-id>/resume.log` — where the detached boot-re-entry child's own stdout
/// and stderr land. The child is detached, so nobody is watching its streams; without a file
/// they are nulled and a pre-supervise refusal (`resume.log`'s whole reason to exist) — the
/// dispatch lock's `WouldBlock`, an unreadable record — vanishes after `resume --all` already
/// said *re-entered*. Appended, like `supervisor.log` beside it, so repeated boots keep every
/// account.
fn resume_log_path(run_dir: &Path) -> PathBuf {
    run_dir.join("resume.log")
}

/// The supervisor's narration, to stdout **and** to a file that outlives the terminal.
///
/// What the supervisor said is the only account of a Run between the dispatch comment and a
/// terminal state, and it died with the host. Leaving the file to the service manager makes it
/// a per-platform question, which is how you get two internally-consistent wrong answers.
///
/// A log that cannot be written is not worth abandoning a Run over.
fn say(run_dir: &Path, line: &str) {
    world::print_line(line);
    let _ = world::append_line(&log_path(run_dir), line);
}

/// A session id with no dependency on a uuid crate. It has to be unique per Run and stable for
/// the Run's life; the run id plus the dispatching pid is both.
fn fresh_session_id(run_id: &str, pid: u32) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in run_id.bytes().chain(pid.to_le_bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let high = format!("{hash:016x}");
    let low = format!("{:016x}", hash.rotate_left(17) ^ u64::from(pid));
    format!(
        "{}-{}-4{}-a{}-{}",
        &high[0..8],
        &high[8..12],
        &high[13..16],
        &low[1..4],
        &low[4..16]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_with_slashes_locks_as_one_file_under_the_locks_directory() {
        // Every branch this project dispatches contains a slash, so the raw key would name a
        // directory that does not exist and the open would fail before any lock was attempted.
        let key = lock_key(
            "FlorianRiquelme/snapper",
            "feat/28-slice-1b-agent-surface-screensource-seam",
        );
        assert!(!key.contains('/'), "the key must be one filename: {key}");
        assert_eq!(
            key,
            "FlorianRiquelme-snapper-feat_28-slice-1b-agent-surface-screensource-seam"
        );
        let path = lock_path(
            Path::new("/home/op"),
            "FlorianRiquelme/snapper",
            "feat/28-x",
        );
        assert_eq!(path.parent().unwrap(), Path::new("/home/op/.grind/locks"));
    }

    #[test]
    fn the_key_is_the_repo_and_the_branch_and_never_a_filesystem_path() {
        // Two worktrees of one repo on one branch must collide, so nothing about where either
        // of them sits may enter the key.
        assert_eq!(lock_key("o/n", "feat/x"), lock_key("o/n", "feat/x"));
        assert_ne!(lock_key("o/n", "feat/x"), lock_key("o/n", "feat/y"));
        assert_ne!(lock_key("o/n", "feat/x"), lock_key("o/other", "feat/x"));
    }

    #[test]
    fn a_hostile_branch_cannot_escape_the_locks_directory() {
        let key = lock_key("o/n", "..%2F..%2Fetc");
        assert!(!key.contains('/'));
        assert!(!key.contains('%'));
    }

    #[test]
    fn the_eight_states_round_trip_and_none_of_them_is_running() {
        let all = [
            State::Dispatched,
            State::RateLimited,
            State::Died,
            State::Completed,
            State::Uncorroborated,
            State::Unobserved,
            State::Exhausted,
            State::Blocked,
        ];
        for state in all {
            let text = serde_json::to_string(&state).unwrap();
            assert_eq!(serde_json::from_str::<State>(&text).unwrap(), state);
            assert_ne!(state.as_str(), "running");
        }
        assert_eq!(all.len(), 8);
    }

    #[test]
    fn a_blocked_run_is_resumable_and_a_completed_or_exhausted_one_is_not() {
        // A Blocker never spent the budget — the world changed, not the number — so the hand
        // `resume` path has nothing to refuse. `--all` excludes it for a different reason: a
        // stopped Run must not be re-entered at the one moment nobody is watching.
        for resumable in [
            State::Dispatched,
            State::RateLimited,
            State::Died,
            State::Uncorroborated,
            State::Unobserved,
            State::Blocked,
        ] {
            assert!(!matches!(resumable, State::Completed | State::Exhausted));
        }
    }

    #[test]
    fn a_session_id_is_shaped_like_one_and_is_stable_for_a_run() {
        let first = fresh_session_id("20260806-122620-snapper-28", 30412);
        assert_eq!(first, fresh_session_id("20260806-122620-snapper-28", 30412));
        assert_ne!(first, fresh_session_id("20260806-122620-snapper-29", 30412));
        let parts: Vec<&str> = first.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(
            first.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
            "{first}"
        );
    }

    #[test]
    fn the_record_parses_the_shape_the_day_one_fixture_holds() {
        const DAY_ONE: &str = include_str!("../tests/fixtures/record/day-one.json");
        let record: RunRecord = serde_json::from_str(DAY_ONE).expect("the base's own record shape");
        assert_eq!(record.attempt_budget, 8);
        assert_eq!(record.limit_sleep_seconds, 1800);
        assert_eq!(record.attempts().len(), 4);
        assert_eq!(record.denied_tools.len(), 7);
        assert_eq!(record.state, State::Completed);
        assert!(record.attempts().iter().any(|a| a.mode == Mode::CiBabysit));
        // The fixture predates the `cleared` verb, and absence reads as the same fact as
        // empty — that is the whole of what `serde(default)` is for here.
        assert!(record.clearances().is_empty());
    }

    #[test]
    fn the_script_s_record_shape_is_refused_rather_than_half_parsed() {
        // There is no migration read path, and a record missing the six fields the base forces
        // at construction is not a record this program can hold.
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
        assert!(serde_json::from_str::<RunRecord>(&script_shaped).is_err());
    }

    #[test]
    fn what_the_writer_serialises_is_what_the_reader_deserialises() {
        // The two types deserialise the same JSON in modules that cannot see each other, so
        // field names are duplicated by design and can drift. The carrier is this test rather
        // than the compiler, which is blind to it precisely because the wall is working.
        //
        // It has to live here: `view` cannot name `RunRecord`, which is the whole point. Under
        // `deny_unknown_fields` on the reader, a field the writer gains and the reader forgets
        // is a failure — a fixture-only check cannot see that, because a field the reader never
        // declares is not a shared field and serde drops it silently.
        const DAY_ONE: &str = include_str!("../tests/fixtures/record/day-one.json");
        let mut written: RunRecord = serde_json::from_str(DAY_ONE).expect("the writer's shape");
        written.push_clearance(Clearance {
            cleared_at: "2026-08-21T19:00:00+00:00".to_string(),
            note: "the deploy key was rotated".to_string(),
        });
        let bytes = serde_json::to_string(&written).expect("serialise");
        let read: crate::view::RunView = serde_json::from_str(&bytes)
            .expect("the reader must accept every field the writer emits");

        assert_eq!(read.run_id, written.run_id);
        assert_eq!(read.attempts.len(), written.attempts().len());
        assert_eq!(read.attempt_budget, written.attempt_budget);
        assert_eq!(read.limit_sleep_seconds, written.limit_sleep_seconds);
        assert_eq!(read.supervisor_pid, written.supervisor_pid);
        assert_eq!(read.state, written.state.as_str());
        assert_eq!(read.denied_tools, written.denied_tools);
        assert_eq!(read.clearances, written.clearances());
    }

    #[test]
    fn a_field_the_writer_gains_and_the_reader_forgets_is_caught() {
        // The failure the test above exists for, reproduced directly.
        let mut value: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/record/day-one.json")).unwrap();
        value["fanout_health"] = serde_json::json!("healthy");
        assert!(
            serde_json::from_value::<crate::view::RunView>(value).is_err(),
            "an undeclared field must fail rather than being dropped silently"
        );
    }

    #[test]
    fn the_attempt_list_can_only_grow() {
        // `attempts` is private with an appending mutator, so *load a stale copy, then
        // overwrite the whole list* is not expressible even from inside the writable type.
        const DAY_ONE: &str = include_str!("../tests/fixtures/record/day-one.json");
        let mut record: RunRecord = serde_json::from_str(DAY_ONE).unwrap();
        let before = record.attempts().len();
        let last = record.attempts().last().unwrap().clone();
        record.push_attempt(last);
        assert_eq!(record.attempts().len(), before + 1);
    }
    // --- a clearance records what changed, and only on a Blocked Run ---------------------------

    fn day_one() -> RunRecord {
        serde_json::from_str(include_str!("../tests/fixtures/record/day-one.json"))
            .expect("the day-one record")
    }

    #[test]
    fn a_clearance_on_a_run_that_is_not_blocked_is_refused_naming_the_actual_state() {
        // The same register as `resume` refusing to overspend: incoherent input, never a
        // health verdict. The day-one record is completed, so there is nothing to clear.
        let mut record = day_one();
        let refused = record_clearance(
            &mut record,
            "the wall moved",
            "2026-08-21T19:00:00+00:00".to_string(),
        );
        let said = refused
            .expect_err("a completed Run has no Blocker to clear")
            .to_string();
        assert!(said.contains("completed"), "{said}");
        assert!(said.contains(&record.run_id), "{said}");
        assert!(record.clearances().is_empty(), "nothing may be appended");
    }

    #[test]
    fn a_clearance_on_a_blocked_run_appends_a_dated_row_and_the_state_stays_blocked() {
        // `cleared` records, `resume` spends: the state word is `resume`'s to change, so a
        // cleared-but-not-resumed Run still reads blocked and stays out of `resume --all`.
        let mut record = day_one();
        record.state = State::Blocked;
        record_clearance(
            &mut record,
            "  the deploy key was rotated  ",
            "2026-08-21T19:00:00+00:00".to_string(),
        )
        .expect("a Blocked Run takes the row");
        assert_eq!(record.clearances().len(), 1);
        assert_eq!(record.clearances()[0].note, "the deploy key was rotated");
        assert_eq!(
            record.clearances()[0].cleared_at,
            "2026-08-21T19:00:00+00:00"
        );
        assert_eq!(record.state, State::Blocked);
    }

    #[test]
    fn an_empty_or_whitespace_note_is_refused_and_nothing_is_appended() {
        let mut record = day_one();
        record.state = State::Blocked;
        for empty in ["", "   ", "\n\t"] {
            let refused = record_clearance(&mut record, empty, "2026-08-21T19:00:00+00:00".into());
            assert!(refused.is_err(), "{empty:?} clears nothing");
        }
        assert!(record.clearances().is_empty());
    }

    #[test]
    fn clearances_accumulate_and_the_latest_is_last() {
        // Re-block after a clearance: clear, resume, blocked again, clear again — the
        // latest note wins on every surface, and every row survives in the record (R3).
        let mut record = day_one();
        record.state = State::Blocked;
        record_clearance(
            &mut record,
            "first wall cleared",
            "2026-08-21T19:00:00+00:00".into(),
        )
        .expect("the first row");
        record_clearance(
            &mut record,
            "second wall cleared",
            "2026-08-21T21:00:00+00:00".into(),
        )
        .expect("the second row");
        assert_eq!(record.clearances().len(), 2);
        assert_eq!(
            record.clearances().last().unwrap().note,
            "second wall cleared"
        );
        assert_eq!(record.clearances()[0].note, "first wall cleared");
    }

    /// A throwaway clone with one commit on `main`, so `worktree add -b` has a HEAD to
    /// branch from. Removed by the caller; a leftover harms nothing but tidiness. Every
    /// effect goes through `world`, which is where `tests/topology.rs` insists they live.
    fn a_clone_with_one_commit(tag: &str) -> PathBuf {
        let repo = world::temp_dir(tag);
        let git = |args: &[&str]| {
            let mut argv = vec!["git"];
            argv.extend_from_slice(args);
            let out = world::run(&words(&argv), Some(&repo));
            assert!(
                out.code == Some(0),
                "git {args:?}: {}",
                out.stderr.lines().next().unwrap_or("no output")
            );
        };
        git(&["init", "-b", "main", "."]);
        world::write(&repo.join("seed"), "seed").unwrap();
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=grind-81",
            "-c",
            "user.email=grind-81@example.invalid",
            "commit",
            "-m",
            "seed",
        ]);
        repo
    }

    fn rev_parse(repo: &Path, what: &str) -> Option<String> {
        let out = world::run(&words(&["git", "rev-parse", what]), Some(repo));
        (out.code == Some(0)).then(|| out.stdout.trim().to_string())
    }

    #[test]
    fn a_branch_that_exists_nowhere_dispatches_a_worktree() {
        // Issue #81: the create path ran `git worktree add <path> <branch>` with no `-b`,
        // so a Job declaring a brand-new branch — the normal case, since the Run creates
        // the branch — was refused with `fatal: invalid reference`. Pinned at the seam
        // itself, against a real clone.
        let repo = a_clone_with_one_commit("fresh");
        let branch = "feat/81-brand-new";
        let worktree = adopt_or_create_worktree(&repo, branch).expect("a fresh branch dispatches");
        assert_eq!(worktree, job::worktree_to_create(&repo, branch));
        assert!(
            rev_parse(&repo, &format!("refs/heads/{branch}")).is_some(),
            "the create path must create the declared branch"
        );
        let listed = world::run(
            &words(&["git", "worktree", "list", "--porcelain"]),
            Some(&repo),
        );
        assert!(
            job::adopt_worktree(&listed.stdout, branch).is_some(),
            "the worktree just made must be adoptable on the next dispatch"
        );
        world::remove_tree(&repo);
    }

    #[test]
    fn a_ref_that_exists_but_holds_no_worktree_is_checked_out_not_recreated() {
        // The other half of the seam: `-b` is reached for only when the ref is missing.
        // Recreating an existing branch would refuse (`already exists`) or, worse, reset it.
        let repo = a_clone_with_one_commit("existing");
        let sha = rev_parse(&repo, "HEAD").unwrap();
        let branched = world::run(&words(&["git", "branch", "side"]), Some(&repo));
        assert_eq!(branched.code, Some(0));
        let worktree = adopt_or_create_worktree(&repo, "side").expect("an existing ref dispatches");
        assert_eq!(rev_parse(&worktree, "HEAD").as_deref(), Some(sha.as_str()));
        world::remove_tree(&repo);
    }

    #[test]
    fn the_worktree_adopted_is_the_one_the_branch_already_holds() {
        let repo = a_clone_with_one_commit("adopted");
        // Adopt stays read-only: no `-b`, no move, just the worktree the porcelain named.
        let first = adopt_or_create_worktree(&repo, "feat/81-twice").unwrap();
        let second = adopt_or_create_worktree(&repo, "feat/81-twice").unwrap();
        // git reports worktree paths canonicalised, and on macOS /var is a symlink to
        // /private/var — so compare through the filesystem, not through the strings.
        assert_eq!(
            world::resolve_link(&first).unwrap(),
            world::resolve_link(&second).unwrap()
        );
        world::remove_tree(&repo);
    }
    /// A minimal crashed Attempt — the child never handed back anything classifiable.
    fn a_crashed_attempt(n: usize) -> Attempt {
        Attempt {
            n,
            mode: Mode::Resume,
            started_at: "2026-08-21T00:00:00+00:00".into(),
            ended_at: "2026-08-21T00:01:00+00:00".into(),
            exit_code: None,
            is_error: true,
            parse_ok: false,
            subtype: Some("unparseable-output".into()),
            stop_reason: None,
            api_error_status: None,
            terminal_reason: None,
            num_turns: None,
            total_cost_usd: None,
            usage: None,
            permission_denials: Vec::new(),
            done_promise: false,
            rate_limited: false,
            result_tail: String::new(),
            fanout: Observed::Absent,
        }
    }

    #[test]
    fn a_crash_inside_attempt_one_resumes_the_session_instead_of_redispatching_it() {
        // A crash after `claude` created the session leaves either no recorded attempts over a
        // transcript that already has lines, or one crashed attempt over any transcript. Keying
        // Dispatch on the empty attempt list alone re-dispatched under the same session id and
        // lost the crashed session's work, so Dispatch now means an untouched Run: no attempts
        // *and* no transcript. A created-but-empty session — the crash landed before Attempt 1
        // was ever recorded — still re-dispatches, because there is nothing in it to lose.
        let crashed = [a_crashed_attempt(1)];
        assert_eq!(entry_mode(&[], 0), Mode::Dispatch);
        assert_eq!(entry_mode(&crashed, 0), Mode::Resume);
        assert_eq!(entry_mode(&crashed, 3), Mode::Resume);
    }

    #[test]
    fn the_detached_resume_child_logs_beside_the_record_it_reenters() {
        // The detached child's stdout and stderr land in one file directly under the run
        // directory, next to `run.json` and `supervisor.log` — the one place a boot re-entry's
        // pre-supervise refusal can still be read after the parent has exited.
        let path = resume_log_path(Path::new("/home/op/.grind/runs/20260821-000000-snapper-90"));
        assert_eq!(
            path,
            Path::new("/home/op/.grind/runs/20260821-000000-snapper-90/resume.log")
        );
    }
}
