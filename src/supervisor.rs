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

use crate::attempt::{
    self, Attempt, Clearance, Conditions, DENIED_TOOLS, Mode, StageConditions, StageContext,
};
use crate::claude;
use crate::decide::{self, Pass, PlanFacts, Tier, Tiers, Verdict};
use crate::job::{self, Job, Refusal};
use crate::learnings;
use crate::observe::{self, Observation, Observed, Outcome as ItemOutcome};
use crate::policy::{self, Budget, Next, Stop};
use crate::rung::{self, ReturnStatus, Stage, StageReturn};
use crate::runner::{self, Backend};
use crate::world::{self, LockHandle, TryLock};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Compiled constants, snapshotted into the record at dispatch. There is no environment
/// override (ADR-0008), so the record is what makes *attempt N of M with M from the record*
/// true and what makes a re-entry under different conditions visible rather than silent.
/// The ten-Attempt fullest T2/T3 walk (eight [S]/[F] stages — the two [R] passes are free —
/// plus two fix rounds) plus four re-entries of headroom: sized from the ladder's own
/// arithmetic rather than picked. Was `8`, the mega-session's own unmapped figure.
pub const ATTEMPT_BUDGET: usize = 14;
/// Plan review's bounded revision round: `act-on` findings are applied once, never re-litigated.
pub const PLAN_REVISIONS: usize = 1;
/// Fixes' bounded rounds; exhaustion leaves residuals in the Record and the Run proceeds.
pub const FIX_ROUNDS: usize = 2;
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
    repo_path: String,
    worktree: String,
    session_id: String,
    claude_bin: String,
    model: Option<String>,
    denied_tools: Vec<String>,
    hostname: String,
    attempt_budget: usize,
    limit_sleep_seconds: u64,
    /// Snapshotted at dispatch, like `attempt_budget` beside them. `serde(default)` because a
    /// record from before Grit's ladder existed carries neither — the same pre-cutover reasoning
    /// `job::Job::done_predicate` documents, never a blank answer.
    #[serde(default)]
    plan_revisions: usize,
    #[serde(default)]
    fix_rounds: usize,
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
    /// One row per stage Attempt, append-only like `attempts` — same privacy discipline, same
    /// reason. Empty on a pre-cutover record (`serde(default)`), which is exactly the fact
    /// `supervise`'s legacy-path gate reads.
    #[serde(default)]
    stages: Vec<rung::StageEntry>,
    /// Frozen at dispatch: the grind binary version plus a hash of the host skill root, so a
    /// skill edit or a binary upgrade mid-Run is visible rather than silent. `None` on a
    /// pre-cutover record, which never had a skill root to hash.
    #[serde(default)]
    provenance: Option<Provenance>,
    /// Whether Reflect has been dispatched for this Run's terminal observation. Set — and
    /// saved — **before** Reflect is dispatched, not after: a supervisor that died mid-Reflect
    /// must not dispatch it again on the next look, and a lost Reflect is a cheaper loss than a
    /// duplicated one. `serde(default)` reads false, which is the correct answer for every
    /// record that predates Reflect existing.
    #[serde(default)]
    reflected: bool,
    /// Which adapter executes this Run's stages, snapshotted at dispatch like every other
    /// environment-varying condition (ADR-0017). P1 snapshots the default — selection wiring
    /// lands next wave.
    #[serde(default)]
    backend: Backend,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    endpoint_override: Option<String>,
    /// The rest of the layout-declared selection (ADR-0017, extended): the host's model
    /// classes and a declared wire mode, snapshotted at dispatch alongside `backend` and
    /// `endpoint_override` for the same reason — every policy knob a Run will ever need is
    /// in the record by the time the first attempt starts. `None` on every record written
    /// before the grammar grew these keys, which is the honest *undeclared* answer, not a
    /// blank one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fast_model_override: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    strong_model_override: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    proto_override: Option<runner::ProtoMode>,
}

/// Frozen once, at dispatch — never re-resolved, for the same reason the plugin pin never was:
/// a Run spans hours, and provenance that changed mid-Run would be silent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Provenance {
    binary_version: String,
    skills_hash: String,
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

    fn stages(&self) -> &[rung::StageEntry] {
        &self.stages
    }

    fn push_stage_entry(&mut self, entry: rung::StageEntry) {
        self.stages.push(entry);
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

    /// The one seam call (R1), bundling this Run's frozen backend selection so every call
    /// site — ladder attempts, Ship's babysit round, and Reflect — constructs it identically.
    /// The stage context is what each call site already holds (`run_ladder_attempt` passes its
    /// dispatching stage; Ship's babysit round and Reflect pass `None`), and it resolves this
    /// attempt's per-stage turn ceiling from `docs/tiers.toml` once here: a stage-aware call
    /// reads the latest decided tier on disk (diff-triage's decision, else triage's), so a
    /// declared override moves the ceiling the native loop enforces while an undeclared stage
    /// still reads the compiled fallback. No stage key means no entry can match — exactly the
    /// behavior before ceilings were data. Resolution happens inside the stage map, so a
    /// stage-less call never touches `docs/tiers.toml` or the Run state's decision files.
    fn runner(
        &self,
        home: &Path,
        run_dir: &Path,
        stage: Option<Stage>,
    ) -> Box<dyn runner::StageRunner> {
        let max_turns = stage.map(|stage| {
            let worktree = std::path::PathBuf::from(&self.worktree);
            let tiers = load_tiers(&worktree);
            let tier = latest_decided_tier(run_dir);
            crate::native::max_turns_for(&stage.to_string(), tier.as_deref(), Some(&tiers))
        });
        runner::runner_for(
            self.backend,
            &self.claude_bin,
            home,
            runner::NativeConfig {
                endpoint_override: self.endpoint_override.clone(),
                fast_model: self.fast_model_override.clone(),
                strong_model: self.strong_model_override.clone(),
                proto_override: self.proto_override,
                max_turns,
            },
        )
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
        TryLock::WouldBlock => Err(Refusal::saying(format!(
            "a running supervisor already holds {target_repo} {branch} on this host"
        ))),
        TryLock::Failed(why) => Err(Refusal::saying(format!(
            "could not determine whether {target_repo} {branch} is held: {why}"
        ))),
    }
}

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

    let selection = job::read_selection(&home).map_err(Refusal::saying)?;

    refuse_unless_host_ready(&home, &job, selection.backend)?;

    refuse_unless_native_ready(selection.backend, selection.endpoint_override.as_deref())?;

    refuse_claude_pin_on_native(selection.backend, job.model.as_deref())?;

    let repo_path = job::repo_path(&home, &job.target_repo);
    let claude_bin = job::claude_bin(&home);

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
        repo_path: repo_path.display().to_string(),
        worktree: worktree.display().to_string(),
        session_id: attempt::stage_session_id(&run_id, Stage::Plan),
        claude_bin: claude_bin.display().to_string(),
        model: job.model.clone(),
        denied_tools: DENIED_TOOLS.iter().map(|glob| glob.to_string()).collect(),
        hostname: world::hostname().unwrap_or_else(|| "unknown-host".to_string()),
        attempt_budget: ATTEMPT_BUDGET,
        limit_sleep_seconds: LIMIT_SLEEP_SECONDS,
        plan_revisions: PLAN_REVISIONS,
        fix_rounds: FIX_ROUNDS,
        supervisor_pid: pid,
        supervisor_identity: recorded_identity(pid),
        attempts: Vec::new(),
        clearances: Vec::new(),
        stages: Vec::new(),
        provenance: Some(provenance(&home)),
        reflected: false,
        backend: selection.backend,
        endpoint_override: selection.endpoint_override,
        fast_model_override: selection.fast_model,
        strong_model_override: selection.strong_model,
        proto_override: selection.proto_override,
        job,
    };
    let run_dir = job::runs_dir(&home).join(&run_id);
    world::create_dir_all(&run_dir).map_err(Refusal::saying)?;
    record.save(&record_path(&run_dir))?;

    point_at_this_host(&record);

    for said in dispatch_banner(&record) {
        say(&run_dir, &said);
    }
    say(&run_dir, &format!("  run {run_id}"));

    supervise(&mut record, &run_dir)
}

/// The lines a Dispatch says about itself, read off the record it just wrote (#141). Every line
/// must be true of the Run it introduces: a banner contradicting its own record costs exactly
/// the trust the record exists to provide. The claude binary is named only under the backend
/// that spawns one; a native Run names its declared models rather than calling itself
/// unpinned — and an undeclared class still names the concrete id [`runner::DEFAULT_MODEL`]
/// resolves to, because that is what will run.
fn dispatch_banner(record: &RunRecord) -> Vec<String> {
    let mut lines = vec![format!("  backend {}", record.backend.as_str())];
    lines.push(format!(
        "  model {}",
        runner::declared_model(
            record.backend,
            record.model.as_deref(),
            record.fast_model_override.as_deref(),
            record.strong_model_override.as_deref(),
        )
    ));
    if record.backend == Backend::ClaudeCode {
        lines.push(format!("  claude {}", record.claude_bin));
    }
    lines
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

    let _lock = take_lock(&home, &keyed.job.target_repo, &keyed.job.branch)?;

    let mut record = RunRecord::load(&path)?;

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
            Observed::Present(false) => {}
            Observed::Present(true) => continue,
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

/// Dispatch to the ladder walk for every record born under it, and to the mega-session loop for
/// the fallback ADR-0015 names: a record with attempts already on it, no `stages` rows and no
/// stage return file on disk anywhere is one this binary never started — the old script's shape.
/// A fresh record (`stages` empty **and** `attempts` empty) is not pre-cutover; it walks the
/// ladder from Attempt 1.
fn supervise(record: &mut RunRecord, run_dir: &Path) -> Result<Outcome, Refusal> {
    if is_pre_cutover(record, run_dir) {
        say(
            run_dir,
            &format!(
                "    pre-cutover record — mapped to {} via rung::from_furthest \
                 (ADR-0015's stated fallback); entering the ladder, which starts at Plan since \
                 no stage return exists on disk for it",
                rung::from_furthest(decide::furthest_stage(&crate::view::observe_fresh(
                    &PathBuf::from(&record.worktree),
                    &record.job.handoff_sha,
                    &record.job.branch,
                    &record.job.base_branch,
                    world::now_iso(),
                )))
            ),
        );
    }
    supervise_ladder(record, run_dir)
}

/// A record with attempts but no `stages` rows and no stage returns on disk — a Run dispatched
/// before the ladder existed. Read only to log `rung::from_furthest`'s mapping for the operator;
/// it changes nothing about which loop runs next, since [`supervise`] always enters the ladder.
fn is_pre_cutover(record: &RunRecord, run_dir: &Path) -> bool {
    if !record.stages().is_empty() || record.attempts().is_empty() {
        return false;
    }
    rung::ALL
        .iter()
        .all(|stage| !world::exists(&run_dir.join("stages").join(format!("{stage}.return.json"))))
}

/// The ladder walk records the terminal state, dispatches Reflect
/// once (never for a Blocker or an Exhaustion — the design's own zero-output skip), and posts the
/// account on the Job issue.
fn finish_run(
    record: &mut RunRecord,
    run_dir: &Path,
    path: &Path,
    stop: Stop,
) -> Result<Outcome, Refusal> {
    record.state = match &stop {
        Stop::Completed => State::Completed,
        Stop::Uncorroborated(_) => State::Uncorroborated,
        Stop::Unobserved(_) => State::Unobserved,
        Stop::Exhausted => State::Exhausted,
        Stop::Blocked(_) => State::Blocked,
    };
    record.save(path)?;
    if let Stop::Unobserved(blind) = &stop {
        for said in blind {
            say(run_dir, &format!("    could not observe {said}"));
        }
    }
    if let Stop::Blocked(what) = &stop {
        say(
            run_dir,
            &format!(
                "    stopped for a human — {what} was refused twice with no progress; {}",
                crate::render::repair_hint(&record.run_id)
            ),
        );
    }
    maybe_dispatch_reflect(record, run_dir, path);
    report_to_the_job_issue(run_dir, record);
    Ok(Outcome {
        run_id: record.run_id.clone(),
        state: record.state.as_str(),
        already_completed: false,
    })
}

/// Dispatch Reflect once per finished Run, immediately after a terminal observation — Completed,
/// Uncorroborated or Unobserved. **Never for `Blocked` or `Exhausted`**: the design's own
/// zero-output skip, since neither carries a Run worth mining.
///
/// **Idempotent over re-runs**: `reflected` is set and saved *before* Reflect is dispatched, so a
/// supervisor that dies mid-Reflect does not dispatch it again on the next terminal observation
/// (there is at most one) — a lost Reflect is the cheaper failure. Bounded to one re-entry on
/// death; a Reflect failure never changes the Run's own terminal state, which is why every path
/// out of this function is `()`, not a `Result`.
fn maybe_dispatch_reflect(record: &mut RunRecord, run_dir: &Path, path: &Path) {
    if record.reflected {
        return;
    }
    if !matches!(
        record.state,
        State::Completed | State::Uncorroborated | State::Unobserved
    ) {
        return;
    }
    record.reflected = true;
    if record.save(path).is_err() {
        say(
            run_dir,
            "    note: could not record reflect's idempotence — skipping reflect",
        );
        return;
    }
    let Some(home) = world::home() else {
        say(run_dir, "    note: $HOME is unset — skipping reflect");
        return;
    };
    let skill_path = skills_root(&home).join("reflect").join("SKILL.md");
    let Ok(skill_text) = world::read_to_string(&skill_path) else {
        say(
            run_dir,
            "    note: reflect's skill text is unavailable — skipping reflect",
        );
        return;
    };
    let conditions = StageConditions {
        claude_bin: &record.claude_bin,
        run_id: &record.run_id,
    };
    let session_id = attempt::reflect_session_id(&record.run_id);
    let worktree = PathBuf::from(&record.worktree);
    let reflect_attempts = record
        .stages()
        .iter()
        .filter(|entry| entry.name == "reflect")
        .count();
    if reflect_attempts >= 2 {
        say(
            run_dir,
            "    note: reflect already re-entered once — skipping",
        );
        return;
    }
    let mode = if reflect_attempts == 0 && transcript_lines_for(&worktree, &session_id) == 0 {
        Mode::Dispatch
    } else {
        Mode::Resume
    };
    let invocation = match mode {
        Mode::Dispatch => claude::reflect_dispatch(&conditions, &skill_text),
        _ => claude::reflect_resume(&conditions, &skill_text),
    };
    let n = reflect_attempts + 1;
    say(run_dir, &format!("  [reflect] attempt {n} ({mode}) …"));
    let denied = attempt::denied_for_reflect();
    let reflect_model = runner::StageModel::Class(runner::ModelClass::Strong);
    let run_dir_str = run_dir.display().to_string();
    let runner = record.runner(&home, run_dir, None);
    let spec = runner::RunSpec {
        invocation: &invocation,
        cwd: run_dir,
        run_dir,
        attempt_n: n,
        session_id: &session_id,
        worktree: &run_dir_str,
        model: &reflect_model,
        denied_globs: &denied,
        file_label: runner::FileLabel::Reflect,
    };
    let classified = runner.run(&spec);
    record.push_stage_entry(rung::StageEntry {
        name: "reflect".to_string(),
        session_id,
        status: reflect_status(classified.is_error),
        artifact_paths: Vec::new(),
        model: None,
        cost_usd: classified.total_cost_usd,
        turns: classified.num_turns,
    });
    let _ = record.save(path);
}

/// Reflect's own verdict on itself, from a fact that means the same thing on both adapters
/// (issue #146). `parse_ok` used to stand in for "the stage worked" — but on the native
/// adapter it is a constant `true`, so a Reflect that died at `turn budget exhausted (32)`
/// recorded `complete` with nothing written. `is_error` is real everywhere: native sets it
/// from the loop's `Ending::Failed` (which turn exhaustion is), and claude-code folds an
/// unparseable payload into it (`parse_ok: false` ⇒ `is_error: true`).
///
/// Error takes precedence over the done-promise (CodeRabbit review): claude-code reads
/// `done_promise` straight out of the payload's result text with no error guard, so an
/// errored ending can still carry the sentinel — and a stage that ended in error has not
/// completed, whatever it claimed mid-stream. With that precedence the promise can never
/// flip an outcome, so it is deliberately not an input here.
fn reflect_status(is_error: bool) -> ReturnStatus {
    if is_error {
        ReturnStatus::Incomplete
    } else {
        ReturnStatus::Complete
    }
}

/// The rung-by-rung walk. Same shape as [`supervise_legacy`]'s loop — attempt (or [R] pass),
/// save, observe, decide — differing only in **what one Attempt executes**: one stage's own
/// session rather than the Run's mega-session, and a zero-token in-process pass at the two [R]
/// rungs that consumes no Attempt at all.
fn supervise_ladder(record: &mut RunRecord, run_dir: &Path) -> Result<Outcome, Refusal> {
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
    let mut stalls: Vec<Observed<bool>> = Vec::new();
    let mut commits_before: Option<Observed<u64>> = None;
    let mut working_seen = attempt::working(record.attempts());

    loop {
        let walked = match rung::next(&read_stage_returns(run_dir)) {
            Some(stage @ (Stage::Triage | Stage::DiffTriage)) => {
                run_r_pass(record, run_dir, &worktree, stage)?;
                record.save(&path)?;
                continue;
            }
            Some(stage) => {
                run_ladder_attempt(record, run_dir, &worktree, stage)?;
                record.save(&path)?;
                false
            }
            None => true,
        };

        let stop = loop {
            let observation = crate::view::observe_fresh(
                &worktree,
                &record.job.handoff_sha,
                &record.job.branch,
                &record.job.base_branch,
                world::now_iso(),
            );
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

            if !matches!(verdict, Verdict::Completed)
                && let Some(stop) = policy::blocker(record.attempts(), &stalls)
            {
                break Some(stop);
            }

            let mut this_budget = budget;
            if let Some(last) = record.attempts().last()
                && last.rate_limited
                && let Some(finer) =
                    policy::reset_time_sleep(&last.result_tail, world::now_local_hour_minute())
            {
                this_budget.limit_sleep = finer;
            }

            match policy::next(
                record.attempts(),
                &verdict,
                &observation.checks_red,
                reobservations,
                &this_budget,
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
                    run_ship_babysit_attempt(record, run_dir, &worktree)?;
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
            return finish_run(record, run_dir, &path, stop);
        }

        if walked {
            run_ladder_attempt(record, run_dir, &worktree, Stage::Ship)?;
            record.save(&path)?;
        }
    }
}

/// Every stage's return, read fresh off disk. A malformed or absent file reads as `None` per
/// stage (`rung::returns_from`'s own tolerance) — fail-closed toward re-entering that one stage.
fn read_stage_returns(run_dir: &Path) -> rung::StageReturns {
    let stages_dir = run_dir.join("stages");
    let texts: Vec<Option<String>> = rung::ALL
        .iter()
        .map(|stage| world::read_to_string(&stages_dir.join(format!("{stage}.return.json"))).ok())
        .collect();
    let slots: [Option<&str>; 10] = std::array::from_fn(|i| texts[i].as_deref());
    rung::returns_from(slots)
}

/// `docs/tiers.toml` at the worktree's `HEAD`, tolerantly parsed; absent or garbage reads as
/// [`Tiers::default`] — the same fail-closed shape the parser itself already carries.
fn load_tiers(worktree: &Path) -> Tiers {
    match world::read_to_string(&worktree.join("docs/tiers.toml")) {
        Ok(text) => decide::tiers_from_toml(&text),
        Err(_) => Tiers::default(),
    }
}

/// The latest decided tier on disk for a stage-aware turn-ceiling lookup: diff-triage's
/// decision when present, else triage's — both under the Run state dir's `stages/`, the
/// same root `run_r_pass` writes to and `resolve_stage_model` reads. Absent or unreadable
/// reads as no tiered entry — the same tolerant serde shape `resolve_stage_model` already
/// uses.
fn latest_decided_tier(run_dir: &Path) -> Option<String> {
    let stages_dir = run_dir.join("stages");
    world::read_to_string(&stages_dir.join("diff-triage").join("decision.json"))
        .ok()
        .or_else(|| world::read_to_string(&stages_dir.join("triage").join("decision.json")).ok())
        .and_then(|text| serde_json::from_str::<decide::Decision>(&text).ok())
        .map(|decision| decision.tier.to_string())
}

/// The Plan stage's own `plan-facts.json`, read tolerantly for both [R] passes. Absent or
/// unparseable reads as `None`: at Triage that fails closed inside `select_tier` (plan is the
/// pass's required fact); at Diff-triage it merely degrades the panel — Performance seats off
/// `declared_hot_paths` and Docs off `forecast_paths`, so a missing file seats neither —
/// because plan is not Diff-triage's required fact.
fn load_plan_facts(stages_dir: &Path) -> Option<PlanFacts> {
    world::read_to_string(&stages_dir.join("plan").join("plan-facts.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<PlanFacts>(&text).ok())
}

/// Run one of the two zero-token in-process passes: `select_tier` over durable facts, writing
/// `decision.json` and the stage's own return. Consumes **no Attempt** — the `StageEntry` this
/// pushes carries `cost_usd: Some(0.0)`, `turns: Some(0)`, and `session_id: "[R]"`, a literal
/// chosen (over an empty string) so a rendered stage table reads *pure Rust, no session* rather
/// than looking like an unresolved field.
fn run_r_pass(
    record: &mut RunRecord,
    run_dir: &Path,
    worktree: &Path,
    stage: Stage,
) -> Result<(), Refusal> {
    let stages_dir = run_dir.join("stages");
    let tiers = load_tiers(worktree);
    let template_record = template_record_for(&record.job.target_repo, &record.run_id);
    let decision = match stage {
        Stage::Triage => {
            let plan_facts = load_plan_facts(&stages_dir);
            decide::select_tier(
                Pass::Triage,
                plan_facts.as_ref(),
                None,
                Some(&template_record),
                Tier::T0,
                &tiers,
            )
        }
        Stage::DiffTriage => {
            let range = format!("{}..HEAD", record.job.handoff_sha);
            let numstat = world::run(
                &words(&["git", "diff", "--numstat", &range]),
                Some(worktree),
            );
            let name_only = world::run(
                &words(&["git", "diff", "--name-only", &range]),
                Some(worktree),
            );
            let unified = world::run(&words(&["git", "diff", &range]), Some(worktree));
            let diff_facts =
                observe::diff_facts(&numstat.stdout, &name_only.stdout, &unified.stdout);
            let floor = world::read_to_string(&stages_dir.join("triage").join("decision.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<decide::Decision>(&text).ok())
                .map(|d| d.tier)
                .unwrap_or(Tier::T2);
            let plan_facts = load_plan_facts(&stages_dir);
            decide::select_tier(
                Pass::DiffTriage,
                plan_facts.as_ref(),
                Some(&diff_facts),
                Some(&template_record),
                floor,
                &tiers,
            )
        }
        _ => unreachable!("run_r_pass is only ever called for Triage or DiffTriage"),
    };

    let name = stage.to_string();
    let dir = stages_dir.join(&name);
    world::create_dir_all(&dir).map_err(Refusal::saying)?;
    let decision_json = serde_json::to_string_pretty(&decision)
        .map_err(|e| Refusal::saying(format!("could not serialise the {name} decision: {e}")))?;
    world::write_atomic(&dir.join("decision.json"), &(decision_json + "\n"))
        .map_err(Refusal::saying)?;

    let ret = StageReturn {
        status: ReturnStatus::Complete,
        revised: false,
    };
    let ret_json = serde_json::to_string(&ret)
        .map_err(|e| Refusal::saying(format!("could not serialise the {name} return: {e}")))?;
    world::write_atomic(&stages_dir.join(format!("{name}.return.json")), &ret_json)
        .map_err(Refusal::saying)?;

    say(
        run_dir,
        &format!(
            "  [R] {name} decided {} (floor {})",
            decision.tier, decision.floor_from_plan
        ),
    );
    record.push_stage_entry(rung::StageEntry {
        name,
        session_id: "[R]".to_string(),
        status: ReturnStatus::Complete,
        artifact_paths: vec![
            format!("stages/{stage}/decision.json"),
            format!("stages/{stage}.return.json"),
        ],
        model: None,
        cost_usd: Some(0.0),
        turns: Some(0),
    });
    Ok(())
}

/// Which model *class* (`decide::Decision::model_per_stage`) resolves to for one stage, per
/// the plan's decision 2 — never a concrete id: a model id is a provider fact (`vendor/model`
/// on native, a plain alias on claude-code), and the class is grind's own routing intent,
/// resolved to a concrete id by each adapter (`StageModel::claude_code_arg`,
/// `StageModel::native_id`) rather than by this function (Unit 1: a fast-routed stage sending
/// the claude-code alias to an OpenAI-compatible endpoint burned three retries on every attempt).
/// **The Job's `Model` row, when present, pins every stage** and short-circuits the class lookup
/// entirely — the freeze beats the routing. Absent that, Plan runs before any Decision exists
/// and is always routed `strong`; every later stage reads its class off Diff-triage's decision
/// when it exists, else Triage's, else falls back to `strong` — the same fail-closed direction
/// `select_tier` itself takes.
fn resolve_stage_model(record: &RunRecord, run_dir: &Path, stage: Stage) -> runner::StageModel {
    use runner::{ModelClass, StageModel};
    if let Some(pinned) = &record.model {
        return StageModel::Pinned(pinned.clone());
    }
    if stage == Stage::Plan {
        return StageModel::Class(ModelClass::Strong);
    }
    let stages_dir = run_dir.join("stages");
    let decision = world::read_to_string(&stages_dir.join("diff-triage").join("decision.json"))
        .ok()
        .or_else(|| world::read_to_string(&stages_dir.join("triage").join("decision.json")).ok())
        .and_then(|text| serde_json::from_str::<decide::Decision>(&text).ok());
    let class = decision
        .as_ref()
        .and_then(|d| d.model_per_stage.get(&stage.to_string()).cloned())
        .unwrap_or_else(|| "strong".to_string());
    match class.as_str() {
        "fast" => StageModel::Class(ModelClass::Fast),
        _ => StageModel::Class(ModelClass::Strong),
    }
}

/// The per-template lookback the two [R] passes fold into their tier call: every *other* Run on
/// this host that targeted the same repo, read read-only through `view::RunView` — never the
/// writer type — and derived on demand (the derivability rule: no separate store, the Records
/// already answer this). A prior Run counts as `ci_failed` when its record shows a
/// [`Mode::CiBabysit`] attempt: policy only ever spends that mode via [`Next::SpendCiBudget`],
/// which is entered when a check came back red, so *the budget was spent* *is* the fact — no
/// new state, no migration. Degrade-don't-abort still holds: a stale or unreadable record only
/// means the tier floor is computed from less history, never from wrong history.
fn template_record_for(target_repo: &str, this_run: &str) -> decide::TrackRecord {
    let Some(home) = world::home() else {
        return decide::TrackRecord::default();
    };
    let runs = job::runs_dir(&home);
    let mut gathered: Vec<(bool, bool, Option<String>)> = Vec::new();
    for entry in world::list_dir(&runs) {
        let Some(run_id) = entry.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if run_id == this_run {
            continue;
        }
        let crate::view::Lookup::Here(found) = crate::view::load(&home, run_id) else {
            continue;
        };
        if found.job.target_repo != target_repo {
            continue;
        }
        let completed = found.state == "completed";
        let ci_failed = prior_run_spent_ci_budget(&found.attempts);
        let outcome = world::read_to_string(&runs.join(run_id).join("outcome.json")).ok();
        gathered.push((completed, ci_failed, outcome));
    }
    let facts: Vec<decide::RunOutcomeFacts> = gathered
        .iter()
        .map(|(completed, ci_failed, outcome)| decide::RunOutcomeFacts {
            completed_unattended: *completed,
            ci_failed: *ci_failed,
            outcome_json: outcome.as_deref(),
        })
        .collect();
    decide::track_record_from(&facts)
}

/// Whether a Run's recorded attempts show its one bounded CI-babysit round was spent.
/// [`Mode::CiBabysit`] has exactly one entry point — [`Next::SpendCiBudget`], taken only when
/// a check came back red — so the recorded mode is the whole derivation of *CI failed on this
/// Run*, derived rather than stored (ADR-0013's shape: nothing new to persist, nothing to
/// migrate, nothing to drift).
fn prior_run_spent_ci_budget(attempts: &[Attempt]) -> bool {
    attempts.iter().any(|a| a.mode == Mode::CiBabysit)
}

/// Plan-only injected notes and lessons (unit B's `StageContext::notes`), read by the caller so
/// the composition itself stays pure. `~/.grind/repos/<owner-name>/notes.md`, read as one
/// hyphen-joined directory segment (`owner-name`, not `owner/name`) — deliberately not
/// `job::repo_path`'s two-level clone layout, since notes travel with the *host's* memory of a
/// repo rather than living inside the git checkout itself. **Default is silence**: an absent
/// file is `None`, never a refusal — notes are an enrichment, and Plan composes identically
/// without them.
fn notes_for(home: &Path, target_repo: &str) -> Option<String> {
    let (owner, name) = job::repo_owner_and_name(target_repo);
    let path = job::grind_dir(home)
        .join("repos")
        .join(format!("{owner}-{name}"))
        .join("notes.md");
    world::read_to_string(&path).ok()
}

/// Applicable `lessons.tsv` lines, appended under a short header to the Plan-stage notes text.
/// Read from `~/.grind/learnings/lessons.tsv`, same absent-is-none pattern as `notes_for` — a
/// Run must never refuse to plan for want of a lessons file. Matched **against the Job's
/// declared hot paths plus the Anchor path**, not PlanFacts' forecast: nothing has run Plan yet,
/// so this is the only forecast a pre-Plan match can key on (`learnings::applicable_lessons`
/// does the actual keyword matching; this is only the read-and-call site).
fn lessons_for(home: &Path, job: &Job) -> Option<String> {
    let path = job::grind_dir(home).join("learnings").join("lessons.tsv");
    let tsv = world::read_to_string(&path).ok()?;
    let lessons = learnings::parse_lessons(&tsv);
    let mut forecast_paths = job.declared_hot_paths.clone();
    forecast_paths.push(job.anchor.clone());
    let matched = learnings::applicable_lessons(&lessons, &forecast_paths);
    if matched.is_empty() {
        return None;
    }
    let mut text = String::from("Lessons matched against this Job's declared paths:\n");
    for lesson in matched {
        text.push_str("- ");
        text.push_str(&lesson.statement);
        text.push('\n');
    }
    Some(text)
}

/// One Attempt for one ladder rung: the stage's own session, dispatched fresh or resumed by
/// whether its transcript already has lines, run through the same `attempt::run` raw-before-parse
/// machinery every Attempt uses. After the child lands, its own return file (if now present)
/// decides the pushed `StageEntry`'s status — fail-closed to `Incomplete` when it is absent, the
/// same reading `rung::next` gives an absent return.
fn run_ladder_attempt(
    record: &mut RunRecord,
    run_dir: &Path,
    worktree: &Path,
    stage: Stage,
) -> Result<(), Refusal> {
    let home = world::home().ok_or_else(|| Refusal::saying("$HOME is unset"))?;
    let skill_path = skills_root(&home).join(stage.to_string()).join("SKILL.md");
    let skill_text = world::read_to_string(&skill_path)
        .map_err(|e| Refusal::saying(format!("stage `{stage}` skill text unreadable: {e}")))?;

    let session_id = attempt::stage_session_id(&record.run_id, stage);
    let stage_attempts_so_far = record
        .stages()
        .iter()
        .filter(|entry| entry.name == stage.to_string())
        .count();
    let mode = if stage_attempts_so_far == 0 && transcript_lines_for(worktree, &session_id) == 0 {
        Mode::Dispatch
    } else {
        Mode::Resume
    };
    let stage_model = resolve_stage_model(record, run_dir, stage);
    let claude_model_arg = stage_model.claude_code_arg();
    let notes = if stage == Stage::Plan {
        let base = notes_for(&home, &record.job.target_repo);
        let lessons = lessons_for(&home, &record.job);
        match (base, lessons) {
            (Some(base), Some(lessons)) => Some(format!("{base}\n\n{lessons}")),
            (Some(base), None) => Some(base),
            (None, Some(lessons)) => Some(lessons),
            (None, None) => None,
        }
    } else {
        None
    };
    let stages_dir_str = run_dir.join("stages").display().to_string();
    let ctx = StageContext {
        stage,
        skill_text: &skill_text,
        stages_dir: &stages_dir_str,
        worktree: &record.worktree,
        job: &record.job,
        model: claude_model_arg.as_deref(),
        notes: notes.as_deref(),
    };
    let conditions = StageConditions {
        claude_bin: &record.claude_bin,
        run_id: &record.run_id,
    };
    let invocation = claude::stage_invocation(&conditions, &ctx, mode, record.clearances().last());

    let n = record.attempts().len() + 1;
    let started_at = world::now_iso();
    say(
        run_dir,
        &format!("  [{started_at}] {stage} attempt {n} ({mode}) …"),
    );
    let denied = attempt::denied_for(stage);
    let runner = record.runner(&home, run_dir, Some(stage));
    let spec = runner::RunSpec {
        invocation: &invocation,
        cwd: worktree,
        run_dir,
        attempt_n: n,
        session_id: &session_id,
        worktree: &record.worktree,
        model: &stage_model,
        denied_globs: &denied,
        file_label: runner::FileLabel::Attempt,
    };
    let classified = runner.run(&spec);
    let cost_usd = classified.total_cost_usd;
    let turns = classified.num_turns;
    record.push_attempt(classified);

    let stage_return_path = run_dir.join("stages").join(format!("{stage}.return.json"));
    let status = world::read_to_string(&stage_return_path)
        .ok()
        .and_then(|text| serde_json::from_str::<StageReturn>(&text).ok())
        .map(|r| r.status)
        .unwrap_or(ReturnStatus::Incomplete);
    let stage_dir = run_dir.join("stages").join(stage.to_string());
    let artifact_paths: Vec<String> = world::list_dir(&stage_dir)
        .into_iter()
        .filter_map(|p| {
            p.strip_prefix(run_dir)
                .ok()
                .map(|rel| rel.display().to_string())
        })
        .collect();
    record.push_stage_entry(rung::StageEntry {
        name: stage.to_string(),
        session_id,
        status,
        artifact_paths,
        model: claude_model_arg,
        cost_usd,
        turns,
    });
    Ok(())
}

/// Ship's one bounded CI-babysit round, continuing **Ship's own session** (`<run>-ship`) rather
/// than a fresh stage composition — there is no stage-shaped babysit prompt to build, so this
/// reuses the existing [`claude::ci_babysit`] builder exactly as the legacy path does, pointed
/// at the stage session instead of the Run-level one.
fn run_ship_babysit_attempt(
    record: &mut RunRecord,
    run_dir: &Path,
    worktree: &Path,
) -> Result<(), Refusal> {
    let session_id = attempt::stage_session_id(&record.run_id, Stage::Ship);
    let stage_model = resolve_stage_model(record, run_dir, Stage::Ship);
    let claude_model_arg = stage_model.claude_code_arg();
    let conditions = Conditions {
        claude_bin: &record.claude_bin,
        session_id: &session_id,
        model: claude_model_arg.as_deref(),
    };
    let invocation = claude::ci_babysit(&conditions);
    let n = record.attempts().len() + 1;
    let started_at = world::now_iso();
    say(
        run_dir,
        &format!("  [{started_at}] attempt {n} (ci-babysit) …"),
    );
    let home = world::home().ok_or_else(|| Refusal::saying("$HOME is unset"))?;
    let denied: Vec<String> = attempt::DENIED_TOOLS
        .iter()
        .map(|glob| glob.to_string())
        .collect();
    let runner = record.runner(&home, run_dir, None);
    let spec = runner::RunSpec {
        invocation: &invocation,
        cwd: worktree,
        run_dir,
        attempt_n: n,
        session_id: &session_id,
        worktree: &record.worktree,
        model: &stage_model,
        denied_globs: &denied,
        file_label: runner::FileLabel::Attempt,
    };
    let classified = runner.run(&spec);
    record.push_attempt(classified);
    Ok(())
}
/// How much of **one session's** transcript exists right now, in lines. A transcript that is not
/// there yet is zero rather than a refusal: a fresh session has nothing to skip, and that is the
/// same answer whether the session is the Run's old mega-session or one stage's own.
///
/// Read while the child is not running, so the file is quiescent and the last line is whole.
fn transcript_lines_for(worktree: &Path, session_id: &str) -> usize {
    let Some(home) = world::home() else {
        return 0;
    };
    let transcript = claude::transcript_path(&home, &worktree.display().to_string(), session_id);
    match world::read_to_string(&transcript) {
        Ok(text) => text.lines().count(),
        Err(_) => 0,
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

fn refuse_unless_host_ready(home: &Path, job: &Job, backend: Backend) -> Result<(), Refusal> {
    for item in required_dispatch_items(backend) {
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

/// The dispatch subset, narrowed to what this Dispatch's declared backend actually needs.
/// `claude binary` is `Backend::ClaudeCode`'s requirement, never `Backend::Native`'s — a
/// native-only host carries no `bin/claude` and dispatching onto it must not be refused for a
/// binary its backend never runs. Every other dispatch-depth item is backend-agnostic, so this
/// is the one filter rather than a per-backend list.
fn required_dispatch_items(backend: Backend) -> Vec<&'static job::HostItem> {
    job::dispatch_subset()
        .into_iter()
        .filter(|item| !(item.check == job::Check::ClaudeBinary && backend == Backend::Native))
        .collect()
}

/// A provisioning refusal, same register as `refuse_unless_host_ready` — never an ADR-0003
/// quality gate. Split from the impure resolve so the decision is testable from a literal
/// `Result` without touching real environment state (the same hermeticity reasoning as the
/// doctor endpoint probe). The `Endpoint` itself is dropped at the call site below, immediately
/// — only whether it resolved crosses into this function.
fn refuse_unless_native_ready_from(
    backend: Backend,
    resolved: Result<(), String>,
) -> Result<(), Refusal> {
    if backend != Backend::Native {
        return Ok(());
    }
    resolved.map_err(|e| Refusal::saying(format!("agent: {e}")))
}

fn refuse_unless_native_ready(
    backend: Backend,
    endpoint_override: Option<&str>,
) -> Result<(), Refusal> {
    refuse_unless_native_ready_from(
        backend,
        runner::Endpoint::resolve(endpoint_override, None).map(|_endpoint| ()),
    )
}

/// Unit 3: a provisioning refusal, same register as the two above — never an ADR-0003
/// quality gate. With grind no longer injecting a claude-code alias into the native path
/// (Unit 1), the only way a Claude-shaped model id reaches the native wire is an operator's
/// own `model:` pin, and it can never resolve there. Narrow on purpose: only a pin
/// **starting with** `claude-` is refused — `gpt-4o` and similar are legitimate ids for an
/// OpenAI-compatible endpoint and must keep working, so this is not "anything without a
/// `/`".
fn refuse_claude_pin_on_native(backend: Backend, model: Option<&str>) -> Result<(), Refusal> {
    if backend != Backend::Native {
        return Ok(());
    }
    match model {
        Some(pin) if pin.starts_with("claude-") => Err(Refusal::saying(format!(
            "agent: the Job pins model {pin:?}, a Claude alias, but this host's declared \
             backend is native — an OpenAI-compatible endpoint cannot resolve it"
        ))),
        _ => Ok(()),
    }
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
        job::Check::SkillsPresent => obs::skills_present(&skill_dir_names(home)),
        other => obs::unchecked(&format!("{other:?} is not a dispatch-depth check")),
    }
}

/// `~/.grind/skills/run` — the host skill root ADR-0015 declares (decision 1). Provisioning
/// copies `skills/run/*` here.
fn skills_root(home: &Path) -> PathBuf {
    job::grind_dir(home).join("skills").join("run")
}

/// The directory names directly under the skill root, for `observe::skills_present` to check
/// against `observe::STAGE_SKILLS`.
fn skill_dir_names(home: &Path) -> Vec<String> {
    world::list_dir(&skills_root(home))
        .into_iter()
        .filter(|p| world::is_dir(p))
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
        .collect()
}

/// Provenance, frozen once at dispatch (plan's decision 1): the grind binary version this Run
/// was dispatched by, plus an identity hash of the host skill root at that moment. Never
/// re-resolved — the same rule the plugin pin followed, for the same reason: a Run spans hours,
/// and provenance that changed mid-Run would be silent.
fn provenance(home: &Path) -> Provenance {
    Provenance {
        binary_version: env!("CARGO_PKG_VERSION").to_string(),
        skills_hash: skills_hash(&skill_files(&skills_root(home))),
    }
}

/// Every file under a skill root, as `(path relative to root, bytes)` — directory reading
/// through `world` alone, recursing on `world::is_dir`. Read failures are skipped rather than
/// refused: an identity hash over what could be read is still an honest, comparable fact, and a
/// hash this fails to compute over must never block a Dispatch the presence check already
/// cleared.
fn skill_files(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in world::list_dir(dir) {
            if world::is_dir(&entry) {
                walk(&entry, root, out);
            } else if let Ok(bytes) = world::read_bytes(&entry) {
                let rel = entry
                    .strip_prefix(root)
                    .unwrap_or(&entry)
                    .display()
                    .to_string();
                out.push((rel, bytes));
            }
        }
    }
    let mut files = Vec::new();
    walk(root, root, &mut files);
    files
}

/// A sorted FNV-1a hash over relative path and file bytes. **An identity check, not a security
/// boundary** — the design's own words — so a hand-rolled hash is exactly right rather than a
/// cut corner: it takes no dependency (ADR-0005) and answers the one question this needs
/// answered, *did the skill tree change under this Run's feet*, without claiming to resist a
/// deliberate collision.
fn skills_hash(files: &[(String, Vec<u8>)]) -> String {
    let mut sorted: Vec<&(String, Vec<u8>)> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut fnv1a = |byte: u8| {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    };
    for (path, bytes) in sorted {
        for byte in path.bytes() {
            fnv1a(byte);
        }
        fnv1a(0);
        for &byte in bytes {
            fnv1a(byte);
        }
        fnv1a(0);
    }
    format!("{hash:016x}")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_backend_does_not_require_the_claude_binary() {
        let items = required_dispatch_items(Backend::Native);
        assert!(
            !items.iter().any(|i| i.check == job::Check::ClaudeBinary),
            "a native-only host must not be refused for lacking `claude`"
        );
    }

    /// The tier word the ceiling resolves from lives in the RUN STATE's stages tree, not the
    /// target worktree (F0): diff-triage's decision outranks triage's, garbage parses as no
    /// decision, and absence is none. Every case here roots at a run_dir-shaped temp tree.
    #[test]
    fn latest_decided_tier_reads_the_run_states_stages_tree() {
        let home = world::temp_dir("latest-decided-tier");
        let run_dir = home.join("runs").join("r1");
        let write = |pass: &str, body: &str| {
            world::create_dir_all(&run_dir.join("stages").join(pass))
                .expect("a stages dir");
            world::write_atomic(
                &run_dir.join("stages").join(pass).join("decision.json"),
                body,
            )
            .expect("a decision file");
        };
        let t2 = r#"{"tier":"t2","personas":[],"depth":{"reviewers":3},
            "model_per_stage":{},"floor_from_plan":"t0","rationale":[]}"#;
        let t1 = t2.replace("t2", "t1");

        write("triage", &t1);
        assert_eq!(
            latest_decided_tier(&run_dir).as_deref(),
            Some("t1"),
            "triage's word alone resolves"
        );
        write("diff-triage", &t2);
        assert_eq!(
            latest_decided_tier(&run_dir).as_deref(),
            Some("t2"),
            "diff-triage outranks triage when both exist"
        );
        write("diff-triage", "not json at all");
        assert_eq!(
            latest_decided_tier(&run_dir),
            None,
            "an unparseable decision is no decision, never a silent fall-through"
        );
        world::remove_tree(&home);
    }


    #[test]
    fn claude_code_backend_still_requires_the_claude_binary() {
        let items = required_dispatch_items(Backend::ClaudeCode);
        assert!(items.iter().any(|i| i.check == job::Check::ClaudeBinary));
    }

    #[test]
    fn narrowing_by_backend_drops_no_other_item() {
        assert_eq!(
            required_dispatch_items(Backend::ClaudeCode).len(),
            job::dispatch_subset().len()
        );
        assert_eq!(
            required_dispatch_items(Backend::Native).len(),
            job::dispatch_subset().len() - 1
        );
    }

    #[test]
    fn claude_code_needs_no_native_credential_preflight() {
        assert_eq!(
            refuse_unless_native_ready_from(
                Backend::ClaudeCode,
                Err("would refuse if this were consulted".to_string())
            ),
            Ok(())
        );
    }

    #[test]
    fn a_native_backend_with_no_credential_refuses_dispatch() {
        let refusal = refuse_unless_native_ready_from(
            Backend::Native,
            Err("no OPENROUTER_API_KEY / OPENAI_API_KEY in environment".to_string()),
        );
        let said = refusal.expect_err("no credential must refuse").to_string();
        assert!(
            said.contains("agent:"),
            "refusal must name its register: {said}"
        );
        assert!(said.contains("OPENROUTER_API_KEY"));
    }

    #[test]
    fn a_native_backend_with_a_credential_proceeds() {
        assert_eq!(
            refuse_unless_native_ready_from(Backend::Native, Ok(())),
            Ok(())
        );
    }

    #[test]
    fn a_claude_alias_pin_refuses_dispatch_on_the_native_backend() {
        let refusal = refuse_claude_pin_on_native(Backend::Native, Some("claude-sonnet-5"))
            .expect_err("a Claude alias cannot resolve on an OpenAI-compatible endpoint");
        let said = refusal.to_string();
        assert!(
            said.contains("agent:"),
            "refusal must name its register: {said}"
        );
        assert!(said.contains("claude-sonnet-5"), "{said}");
    }

    #[test]
    fn a_non_claude_pin_proceeds_on_the_native_backend() {
        assert_eq!(
            refuse_claude_pin_on_native(Backend::Native, Some("gpt-4o")),
            Ok(())
        );
        assert_eq!(
            refuse_claude_pin_on_native(Backend::Native, Some("deepseek/deepseek-chat-v3.1")),
            Ok(())
        );
    }

    #[test]
    fn an_unpinned_job_proceeds_on_the_native_backend() {
        assert_eq!(refuse_claude_pin_on_native(Backend::Native, None), Ok(()));
    }

    #[test]
    fn a_claude_code_banner_still_names_the_unpinned_default_and_the_binary() {
        let mut record = day_one();
        record.backend = Backend::ClaudeCode;
        record.model = None;
        assert_eq!(
            dispatch_banner(&record),
            vec![
                "  backend claude-code".to_string(),
                "  model (session default — unpinned)".to_string(),
                format!("  claude {}", record.claude_bin),
            ]
        );
    }

    #[test]
    fn a_native_banner_names_the_declared_model_instead_of_calling_itself_unpinned() {
        let mut record = day_one();
        record.backend = Backend::Native;
        record.model = None;
        record.fast_model_override = Some("stealth/ox-alpha".to_string());
        record.strong_model_override = Some("stealth/ox-alpha".to_string());
        assert_eq!(
            dispatch_banner(&record),
            vec![
                "  backend native".to_string(),
                "  model stealth/ox-alpha".to_string(),
            ]
        );
    }

    #[test]
    fn a_native_banner_splits_distinct_fast_and_strong_declarations() {
        let mut record = day_one();
        record.backend = Backend::Native;
        record.model = None;
        record.fast_model_override = Some("stealth/ox-alpha".to_string());
        record.strong_model_override = Some("deepseek/deepseek-chat-v3.1".to_string());
        assert_eq!(
            dispatch_banner(&record),
            vec![
                "  backend native".to_string(),
                "  model fast stealth/ox-alpha · strong deepseek/deepseek-chat-v3.1".to_string(),
            ]
        );
    }

    #[test]
    fn an_undeclared_native_banner_names_the_concrete_default_that_will_run() {
        let mut record = day_one();
        record.backend = Backend::Native;
        record.model = None;
        assert_eq!(
            dispatch_banner(&record),
            vec![
                "  backend native".to_string(),
                format!("  model {}", runner::DEFAULT_MODEL),
            ]
        );
    }

    #[test]
    fn a_pinned_job_names_its_pin_on_either_backend() {
        for backend in [Backend::Native, Backend::ClaudeCode] {
            let mut record = day_one();
            record.backend = backend;
            record.model = Some("claude-opus-9".to_string());
            let banner = dispatch_banner(&record);
            assert_eq!(banner[0], format!("  backend {}", backend.as_str()));
            assert_eq!(banner[1], "  model claude-opus-9");
            assert_eq!(
                banner.iter().any(|l| l.starts_with("  claude ")),
                backend == Backend::ClaudeCode,
                "{backend:?}: only a claude-code Run names the binary"
            );
        }
    }

    #[test]
    fn a_claude_alias_pin_is_untouched_on_the_claude_code_backend() {
        assert_eq!(
            refuse_claude_pin_on_native(Backend::ClaudeCode, Some("claude-sonnet-5")),
            Ok(())
        );
    }

    #[test]
    fn a_branch_with_slashes_locks_as_one_file_under_the_locks_directory() {
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
    fn a_new_records_session_id_is_the_plan_stages_own() {
        let run = "20260806-122620-snapper-28";
        let plan = attempt::stage_session_id(run, Stage::Plan);
        assert_ne!(plan, attempt::stage_session_id(run, Stage::Work));
        assert_ne!(plan, attempt::stage_session_id(run, Stage::Ship));
        assert_ne!(
            plan,
            attempt::reflect_session_id(run),
            "reflect is not a rung, but its session must still be its own"
        );
    }

    /// The seam itself (issue #146, CodeRabbit review): the supervisor records whatever
    /// `reflect_status` answers for the classified Attempt — the fix substituted
    /// `reflect_status(classified.is_error)` for `classified.done_promise ||
    /// classified.parse_ok`. Error taking precedence over a spoken done-promise is only
    /// guaranteed while the promise stays out of the helper's inputs, so this builds the
    /// classified Attempt the name claims — errored ending, DONE promised mid-stream — and
    /// pins the record through the same expression the supervisor runs. Reverting the
    /// substitution at the push_stage_entry call fails here, not three helper-only greens.
    #[test]
    fn an_error_ending_takes_precedence_over_a_spoken_done_promise() {
        let attempt = Attempt {
            n: 1,
            mode: Mode::Dispatch,
            started_at: "s".to_string(),
            ended_at: "e".to_string(),
            exit_code: Some(1),
            is_error: true,
            parse_ok: true,
            subtype: Some("success".to_string()),
            stop_reason: None,
            api_error_status: None,
            terminal_reason: None,
            num_turns: Some(3),
            total_cost_usd: None,
            usage: None,
            permission_denials: vec![],
            done_promise: true,
            rate_limited: false,
            result_tail: String::new(),
            fanout: Observed::Absent,
            transcript_name: None,
        };
        let status = reflect_status(attempt.is_error);
        assert_eq!(
            status,
            ReturnStatus::Incomplete,
            "an errored ending that spoke the sentinel mid-stream is still not a complete stage"
        );
    }

    #[test]
    fn a_reflect_that_spoke_for_itself_is_complete() {
        assert_eq!(reflect_status(false), ReturnStatus::Complete);
    }

    #[test]
    fn skills_hash_is_order_independent_and_content_sensitive() {
        let a = vec![
            ("plan/SKILL.md".to_string(), b"one".to_vec()),
            ("work/SKILL.md".to_string(), b"two".to_vec()),
        ];
        let shuffled = vec![
            ("work/SKILL.md".to_string(), b"two".to_vec()),
            ("plan/SKILL.md".to_string(), b"one".to_vec()),
        ];
        assert_eq!(skills_hash(&a), skills_hash(&shuffled));

        let edited = vec![
            ("plan/SKILL.md".to_string(), b"one!".to_vec()),
            ("work/SKILL.md".to_string(), b"two".to_vec()),
        ];
        assert_ne!(skills_hash(&a), skills_hash(&edited));
    }

    #[test]
    fn skills_hash_of_no_files_is_stable_and_not_a_special_case() {
        assert_eq!(skills_hash(&[]), skills_hash(&[]));
    }

    #[test]
    fn resolve_stage_model_pins_every_stage_when_the_job_names_one() {
        let mut record = day_one();
        record.model = Some("claude-opus-9".to_string());
        let run_dir = world::temp_dir("resolve-model-pinned");
        for stage in [Stage::Plan, Stage::Work, Stage::Ship] {
            assert_eq!(
                resolve_stage_model(&record, &run_dir, stage),
                runner::StageModel::Pinned("claude-opus-9".to_string()),
                "{stage} must take the Job's pinned model"
            );
        }
        world::remove_tree(&run_dir);
    }

    #[test]
    fn resolve_stage_model_routes_plan_strong_before_any_decision_exists() {
        let mut record = day_one();
        record.model = None;
        let run_dir = world::temp_dir("resolve-model-plan");
        let resolved = resolve_stage_model(&record, &run_dir, Stage::Plan);
        assert_eq!(
            resolved,
            runner::StageModel::Class(runner::ModelClass::Strong)
        );
        assert_eq!(
            resolved.claude_code_arg(),
            None,
            "strong means the harness default: no --model flag"
        );
        world::remove_tree(&run_dir);
    }
    #[test]
    fn an_unpinned_jobs_babysit_round_rides_the_stage_resolved_ship_model() {
        let mut record = day_one();
        record.model = None;
        let run_dir = world::temp_dir("babysit-model");
        let decision = run_dir.join("stages").join("diff-triage");
        world::create_dir_all(&decision).unwrap();
        world::write(
            &decision.join("decision.json"),
            r#"{
                "tier": "t1",
                "personas": [],
                "depth": {"reviewers": 1},
                "model_per_stage": {"ship": "fast"},
                "floor_from_plan": "t1",
                "rationale": []
            }"#,
        )
        .unwrap();
        assert_eq!(
            resolve_stage_model(&record, &run_dir, Stage::Ship),
            runner::StageModel::Class(runner::ModelClass::Fast),
            "the babysit round must ride the same routed class Ship's other attempts get"
        );
        world::remove_tree(&run_dir);
    }

    #[test]
    fn a_prior_runs_ci_babysit_attempt_is_what_counts_it_as_a_ci_failure() {
        let spent = day_one();
        assert!(prior_run_spent_ci_budget(spent.attempts()));
        let mut clean = day_one();
        clean.attempts.clear();
        assert!(!prior_run_spent_ci_budget(clean.attempts()));
        let facts = [
            decide::RunOutcomeFacts {
                completed_unattended: true,
                ci_failed: prior_run_spent_ci_budget(spent.attempts()),
                outcome_json: None,
            },
            decide::RunOutcomeFacts {
                completed_unattended: true,
                ci_failed: prior_run_spent_ci_budget(clean.attempts()),
                outcome_json: None,
            },
        ];
        assert_eq!(
            decide::track_record_from(&facts).ci_failures,
            1,
            "exactly the budget-spending Run counts as a CI failure"
        );
    }

    #[test]
    fn read_stage_returns_is_absent_over_an_empty_stages_directory() {
        let run_dir = world::temp_dir("read-returns-empty");
        world::create_dir_all(&run_dir).unwrap();
        assert_eq!(read_stage_returns(&run_dir), rung::StageReturns::default());
        world::remove_tree(&run_dir);
    }

    #[test]
    fn read_stage_returns_reads_a_written_plan_return_off_disk() {
        let run_dir = world::temp_dir("read-returns-plan");
        let stages_dir = run_dir.join("stages");
        world::create_dir_all(&stages_dir).unwrap();
        world::write(
            &stages_dir.join("plan.return.json"),
            r#"{"status":"complete"}"#,
        )
        .unwrap();
        let returns = read_stage_returns(&run_dir);
        assert_eq!(
            returns.plan,
            Some(StageReturn {
                status: ReturnStatus::Complete,
                revised: false,
            })
        );
        assert_eq!(rung::next(&returns), Some(Stage::Triage));
        world::remove_tree(&run_dir);
    }

    #[test]
    fn is_pre_cutover_is_false_for_a_brand_new_record_with_no_attempts() {
        let record = day_one();
        let run_dir = world::temp_dir("pre-cutover-fresh");
        world::create_dir_all(&run_dir).unwrap();
        let mut fresh = record.clone();
        fresh.attempts.clear();
        assert!(!is_pre_cutover(&fresh, &run_dir));
        world::remove_tree(&run_dir);
    }

    #[test]
    fn is_pre_cutover_is_true_for_an_old_record_with_attempts_and_no_stage_rows_or_returns() {
        let record = day_one();
        let run_dir = world::temp_dir("pre-cutover-old");
        world::create_dir_all(&run_dir).unwrap();
        assert!(record.stages().is_empty());
        assert!(!record.attempts().is_empty());
        assert!(is_pre_cutover(&record, &run_dir));
        world::remove_tree(&run_dir);
    }

    #[test]
    fn is_pre_cutover_is_false_once_any_stage_return_file_exists() {
        let record = day_one();
        let run_dir = world::temp_dir("pre-cutover-with-return");
        let stages_dir = run_dir.join("stages");
        world::create_dir_all(&stages_dir).unwrap();
        world::write(
            &stages_dir.join("plan.return.json"),
            r#"{"status":"complete"}"#,
        )
        .unwrap();
        assert!(!is_pre_cutover(&record, &run_dir));
        world::remove_tree(&run_dir);
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
        assert!(record.clearances().is_empty());
    }

    #[test]
    fn the_script_s_record_shape_is_refused_rather_than_half_parsed() {
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
        const DAY_ONE: &str = include_str!("../tests/fixtures/record/day-one.json");
        let mut written: RunRecord = serde_json::from_str(DAY_ONE).expect("the writer's shape");
        written.push_clearance(Clearance {
            cleared_at: "2026-08-21T19:00:00+00:00".to_string(),
            note: "the deploy key was rotated".to_string(),
        });
        written.push_stage_entry(rung::StageEntry {
            name: "plan".to_string(),
            session_id: "20260806-122620-snapper-28-plan".to_string(),
            status: ReturnStatus::Complete,
            artifact_paths: vec!["stages/plan/anchor-plan.md".to_string()],
            model: None,
            cost_usd: Some(3.2),
            turns: Some(11),
        });
        written.provenance = Some(Provenance {
            binary_version: env!("CARGO_PKG_VERSION").to_string(),
            skills_hash: "deadbeef".to_string(),
        });
        written.reflected = true;
        written.backend = Backend::Native;
        written.endpoint_override = Some("http://localhost:8000/v1".to_string());
        written.fast_model_override = Some("stealth/ox-alpha".to_string());
        written.strong_model_override = Some("deepseek/deepseek-chat-v3.1".to_string());
        written.proto_override = Some(runner::ProtoMode::Text);
        let bytes = serde_json::to_string(&written).expect("serialise");
        let read: crate::view::RunView = serde_json::from_str(&bytes)
            .expect("the reader must accept every field the writer emits");

        assert_eq!(read.run_id, written.run_id);
        assert_eq!(read.attempts.len(), written.attempts().len());
        assert_eq!(read.attempt_budget, written.attempt_budget);
        assert_eq!(read.limit_sleep_seconds, written.limit_sleep_seconds);
        assert_eq!(read.plan_revisions, written.plan_revisions);
        assert_eq!(read.fix_rounds, written.fix_rounds);
        assert_eq!(read.supervisor_pid, written.supervisor_pid);
        assert_eq!(read.state, written.state.as_str());
        assert_eq!(read.denied_tools, written.denied_tools);
        assert_eq!(read.clearances, written.clearances());
        assert_eq!(read.stages, written.stages());
        assert_eq!(
            read.provenance.map(|p| p.skills_hash),
            written.provenance.map(|p| p.skills_hash)
        );
        assert_eq!(read.reflected, written.reflected);
        assert_eq!(read.backend, written.backend);
        assert_eq!(read.endpoint_override, written.endpoint_override);
        assert_eq!(read.fast_model_override, written.fast_model_override);
        assert_eq!(read.strong_model_override, written.strong_model_override);
        assert_eq!(read.proto_override, written.proto_override);
    }

    #[test]
    fn a_field_the_writer_gains_and_the_reader_forgets_is_caught() {
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
        const DAY_ONE: &str = include_str!("../tests/fixtures/record/day-one.json");
        let mut record: RunRecord = serde_json::from_str(DAY_ONE).unwrap();
        let before = record.attempts().len();
        let last = record.attempts().last().unwrap().clone();
        record.push_attempt(last);
        assert_eq!(record.attempts().len(), before + 1);
    }

    fn day_one() -> RunRecord {
        serde_json::from_str(include_str!("../tests/fixtures/record/day-one.json"))
            .expect("the day-one record")
    }

    #[test]
    fn a_clearance_on_a_run_that_is_not_blocked_is_refused_naming_the_actual_state() {
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
        let first = adopt_or_create_worktree(&repo, "feat/81-twice").unwrap();
        let second = adopt_or_create_worktree(&repo, "feat/81-twice").unwrap();
        assert_eq!(
            world::resolve_link(&first).unwrap(),
            world::resolve_link(&second).unwrap()
        );
        world::remove_tree(&repo);
    }

    #[test]
    fn the_detached_resume_child_logs_beside_the_record_it_reenters() {
        let path = resume_log_path(Path::new("/home/op/.grind/runs/20260821-000000-snapper-90"));
        assert_eq!(
            path,
            Path::new("/home/op/.grind/runs/20260821-000000-snapper-90/resume.log")
        );
    }
}
