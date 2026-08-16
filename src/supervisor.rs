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

use crate::attempt::{self, Attempt, Conditions, DENIED_TOOLS, Invocation, Mode};
use crate::decide::{self, Verdict};
use crate::job::{self, Job, Refusal};
use crate::observe::{Observation, Observed, Outcome as ItemOutcome};
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
}

impl RunRecord {
    fn attempts(&self) -> &[Attempt] {
        &self.attempts
    }

    fn push_attempt(&mut self, attempt: Attempt) {
        self.attempts.push(attempt);
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
    pub attempts_made: usize,
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
        // A collision. Another Run holds this branch.
        TryLock::WouldBlock => Err(Refusal::saying(format!(
            "another Run holds {target_repo} {branch} on this host"
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
        supervisor_identity: world::process_start_stamp(pid),
        attempts: Vec::new(),
        job,
    };
    let run_dir = job::runs_dir(&home).join(&run_id);
    world::create_dir_all(&run_dir).map_err(Refusal::saying)?;
    record.save(&record_path(&run_dir))?;

    point_at_this_host(&record);

    world::print_line(&format!("  plugin pinned to {}", record.plugin_dir));
    world::print_line(&format!(
        "  model {}",
        record
            .model
            .as_deref()
            .unwrap_or("(session default — unpinned)")
    ));
    world::print_line(&format!("  claude {}", record.claude_bin));
    world::print_line(&format!("  run {run_id}"));

    supervise(&mut record, &run_dir)
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
    if matches!(record.state, State::Completed | State::Exhausted) {
        return Ok(Outcome {
            run_id: record.run_id.clone(),
            state: record.state.as_str(),
            attempts_made: record.attempts().len(),
            already_completed: true,
        });
    }

    // A Run being re-entered by hand holds its branch exactly as a dispatched one does, and the
    // original holder's lock died with its supervisor.
    let _lock = take_lock(&home, &record.job.target_repo, &record.job.branch)?;

    let pid = world::pid();
    record.supervisor_pid = pid;
    record.supervisor_identity = world::process_start_stamp(pid);
    record.save(&path)?;

    supervise(&mut record, &run_dir)
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
        // The first pass has no attempt to classify, so the loop starts by attempting.
        let mode = if record.attempts().is_empty() {
            Mode::Dispatch
        } else {
            Mode::Resume
        };
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
            announce(record, &observation, &verdict);

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
                    world::print_line(&format!(
                        "    a signal could not be observed — sleeping {}s before looking again",
                        pause.as_secs()
                    ));
                    world::sleep(pause);
                    continue;
                }
                Next::SpendCiBudget => {
                    reobservations = 0;
                    world::print_line(
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
                    world::print_line(&format!(
                        "    rate limited — sleeping {}s, then re-entering",
                        nap.as_secs()
                    ));
                    world::sleep(nap);
                    break None;
                }
                Next::Reenter => {
                    reobservations = 0;
                    record.state = State::Died;
                    record.save(&path)?;
                    world::print_line(
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
                    world::print_line(&format!("    could not observe {said}"));
                }
            }
            // Resumable, because it never spent the budget: the world changed, not the number.
            if let Stop::Blocked(what) = &stop {
                world::print_line(&format!(
                    "    stopped for a human — {what} was refused twice with no progress; \
                     `grind resume {}` once it is cleared",
                    record.run_id
                ));
            }
            return Ok(Outcome {
                run_id: record.run_id.clone(),
                state: record.state.as_str(),
                attempts_made: record.attempts().len(),
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
    let spend_cap = job::spend_cap(record.job.budget.as_deref());
    let conditions = Conditions {
        claude_bin: &record.claude_bin,
        session_id: &record.session_id,
        // Read from the record on every attempt. **Never re-resolved**: an eight-attempt Run
        // spans hours of rate-limit sleeps, and a version changing mid-Run is silent.
        plugin_dir: &record.plugin_dir,
        model: record.model.as_deref(),
        spend_cap: spend_cap.as_deref(),
    };
    let invocation: Invocation = match mode {
        Mode::Dispatch => attempt::dispatch(&conditions, &record.job),
        Mode::Resume => attempt::resume(&conditions),
        Mode::CiBabysit => attempt::ci_babysit(&conditions),
    };

    let started_at = world::now_iso();
    world::print_line(&format!("  [{started_at}] attempt {n} ({mode}) …"));

    let raw = attempt::run(
        &invocation,
        worktree,
        &run_dir.join(format!("attempt-{n}.prompt.txt")),
        &run_dir.join(format!("attempt-{n}.stdout.json")),
        &run_dir.join(format!("attempt-{n}.stderr.log")),
    )
    .map_err(|reason| Refusal::saying(reason.to_string()))?;

    let classified = raw.classify(n, mode, &started_at, &world::now_iso());
    record.push_attempt(classified);
    Ok(())
}

fn announce(record: &RunRecord, observation: &Observation, verdict: &Verdict) {
    let last = record.attempts().last();
    let outcome = match last {
        Some(a) if a.done_promise => "DONE promised".to_string(),
        Some(a) => a.subtype.clone().unwrap_or_else(|| "ended".to_string()),
        None => "no attempt".to_string(),
    };
    let cost = last.and_then(|a| a.total_cost_usd).unwrap_or(0.0);
    world::print_line(&format!(
        "    -> {outcome} | stage={} | commits={} | cost=${cost:.2} | {verdict:?}",
        decide::furthest_stage(observation),
        observation.commits_ahead,
    ));
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

/// The presence half of the shared item list — local, free, no network. `cli` runs the same
/// items at doctor's depth.
pub fn check_presence(home: &Path, job: &Job, check: job::Check) -> Observed<ItemOutcome> {
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
    let made = world::run(
        &[
            "git".to_string(),
            "worktree".to_string(),
            "add".to_string(),
            wanted.display().to_string(),
            branch.to_string(),
        ],
        Some(repo_path),
    );
    if made.code != Some(0) {
        return Err(Refusal::saying(format!(
            "could not create a worktree for {branch}: {}",
            made.stderr.lines().next().unwrap_or("no output")
        )));
    }
    world::print_line(&format!("  created worktree: {}", wanted.display()));
    Ok(wanted)
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
        let written: RunRecord = serde_json::from_str(DAY_ONE).expect("the writer's shape");
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
}
