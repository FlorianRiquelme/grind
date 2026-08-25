//! The only impure module: the sole namer of `std::process`, `std::fs` and `std::env`.
//!
//! It holds no branching worth the name — every decision belongs to a pure caller. Effects
//! come back as values: a child comes back as [`Completed`], a lock attempt comes back as
//! [`TryLock`], and classifying either is somebody else's job, done away from the spawn where
//! a test needs three string literals instead of a process (ADR-0007).
//!
//! It is deliberately **shallow**, inverting the usual depth rule. It is the irreducible I/O
//! edge and the only untested code in the base; shrinking it is the goal, and
//! `tests/topology.rs` is what makes *the only untested code* a checked claim rather than an
//! aspiration.
//!
//! **`world` is unconstrained by construction, so the constraint is stated rather than typed.**
//! The denial globs bind the `claude` child and nothing else — `run(argv, cwd)` reaches every
//! forbidden operation from Grind's own process with nothing in front of it. Grind's own
//! process never spawns `git reset --hard`, `git rebase`, `git push --force`, a branch
//! deletion, or `gh pr merge`.
//!
//! **One place, two writes** (ADR-0012). Grind writes on the Job issue and nowhere else, and
//! both writes are comments: the dispatch comment, and the terminal-state comment. It applies
//! no label, assignee, project or milestone on any repo — a comment is additive and ungoverned,
//! while a label is a shared namespace someone else owns. Doctor never performs a write to
//! prove a credential step. The concrete vector is the dirty-worktree refusal — an agent making
//! a stuck Dispatch go through reaches for `git reset --hard`, which is idiomatic and invisible
//! to the globs.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// What a short-lived child left behind. Three values, and nothing interpreted.
///
/// `code` is `None` when the child was killed by a signal, and also when the spawn itself
/// failed — in which case the reason is on `stderr`. Both are *could not observe* to the
/// classifier, which is why reflecting them identically loses nothing.
#[derive(Debug, Clone)]
pub struct Completed {
    pub stdout: String,
    pub stderr: String,
    pub code: Option<i32>,
}

/// An acquired lock, opaque on purpose. It carries no operations — holding it *is* the
/// operation, and dropping it releases. Wrapping the handle is also what keeps `std::fs` from
/// having to be named by the module that takes the lock.
#[derive(Debug)]
pub struct LockHandle(File);

impl Drop for LockHandle {
    /// The tidy path, for a supervisor that exits normally. The guarantee that matters does not
    /// depend on it: the kernel releases the lock when the holding process dies, killed or not,
    /// which is the whole reason this is a lock rather than a state check.
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// What the kernel said about a lock, unclassified. `WouldBlock` and `Failed` are never
/// folded together: collapsing them reproduces the exact bug `Observed<T>` exists to remove,
/// relocated to the lock.
pub enum TryLock {
    /// Acquired. **The handle must outlive the work it guards** — `File::try_lock` releases on
    /// drop, so a handle owned by a dispatch function evaporates seconds into a Run that lasts
    /// hours, and the kernel-releases-it-when-the-holder-dies guarantee needs a holder that is
    /// still holding.
    Acquired(LockHandle),
    /// Somebody else holds it.
    WouldBlock,
    /// The attempt could not be made at all — permissions, a missing directory, anything.
    Failed(String),
}

/// One short-lived child (`git`, `gh`, `ps`). Concrete, no trait: a trait here would be a
/// hypothetical seam with one production impl and one test impl, both in Rust, and the actual
/// spawn path exercised by neither (ADR-0007).
pub fn run(argv: &[String], cwd: Option<&Path>) -> Completed {
    run_scrubbed(argv, cwd, &[])
}

/// `run`, plus `command.env_remove(v)` for each of `drop_vars` before the child ever spawns.
///
/// The native backend's `bash` tool is the first thing that requires an LLM provider
/// credential (`OPENROUTER_API_KEY` / `OPENAI_API_KEY`) in the supervisor's own environment,
/// and it hands a shell to a model that can read files and receive prompt injection from the
/// target repo. A child spawned with `run` inherits that credential and can print it —
/// `env`, `echo $OPENROUTER_API_KEY` — straight into the transcript the next request replays.
/// No deny glob can cover that: it names no forbidden verb.
///
/// Scoped removal rather than `env_clear()`: a stage's shell legitimately needs `PATH`,
/// `HOME`, and the ambient git/gh environment to do ordinary work, and clearing all of it
/// would break every other tool call to close one credential's leak.
pub fn run_scrubbed(argv: &[String], cwd: Option<&Path>, drop_vars: &[&str]) -> Completed {
    let Some((program, rest)) = argv.split_first() else {
        return Completed {
            stdout: String::new(),
            stderr: "empty argv".into(),
            code: None,
        };
    };
    let mut command = Command::new(program);
    command.args(rest);
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    for var in drop_vars {
        command.env_remove(var);
    }
    match command.output() {
        Ok(out) => Completed {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            code: out.status.code(),
        },
        Err(e) => Completed {
            stdout: String::new(),
            stderr: e.to_string(),
            code: None,
        },
    }
}

/// `run_scrubbed`, plus a wall-clock deadline: a child still running when `limit` elapses is
/// killed rather than awaited forever.
///
/// The command string a native attempt's `bash` tool runs comes from an arbitrary third-party
/// model, and `Command::output()` — what `run_scrubbed` calls — blocks on `wait()` with no
/// deadline. An accidental `tail -f`, a hung network call or a child waiting on stdin wedges
/// the per-run supervisor thread forever; nothing above this function bounds wall time (`MAX_TURNS`
/// bounds turns, not seconds).
///
/// `Command::output()` cannot be given a timeout, so this spawns instead and polls
/// [`std::process::Child::try_wait`] on a short interval until either the child exits or the
/// deadline passes. Both pipes are drained on their own threads *before* the poll loop blocks on
/// anything, so a chatty child that fills one pipe's kernel buffer while nobody is reading the
/// other cannot deadlock the wait the way a synchronous read of both, one after the other, would.
/// `stdin` is nulled, not piped: a child that reads from it would otherwise block on EOF that
/// never comes, which is itself a hang this exists to catch.
///
/// A child still running past `limit` is `kill()`ed and reaped, and comes back as
/// `code: None` with the reason in `stderr` — the same *could not observe* shape `run_scrubbed`
/// already uses for a spawn failure, so the classifier stays unchanged, and worded so a caller
/// reading `stderr` can tell *killed on a deadline* apart from *never started*.
///
/// **Two more hazards, both about the child's own children.** `read_to_end` on a pipe returns
/// only at EOF, and EOF arrives only once *every* write end is closed — not when the direct
/// child exits. A command run through `sh -c` that backgrounds something (`sleep 30 &`,
/// `npm start &`, a `tail -f`) leaves that grandchild holding the inherited write end open, so
/// the direct child can exit — or be killed — while the pipe stays open indefinitely. Two
/// things follow from that:
///
/// - `child.kill()` signals the direct child alone, so it does not reach the grandchild.
///   `process_group(0)` puts the child at the head of its own process group before it spawns
///   (best-effort: `#[cfg(unix)]`, a no-op elsewhere), and [`kill_process_group_best_effort`]
///   then signals the *group*, which reaches an ordinary backgrounded grandchild too. This is
///   deliberately not load-bearing — a grandchild that double-forks into its own session
///   (real daemonizing) sits outside the group and survives it regardless, and the pid can in
///   principle be recycled between reap and signal. Both are accepted, rare misses.
/// - Because that best-effort kill can miss, the reader threads are never `join()`ed. They
///   send their bytes over a channel instead, and collection reads that channel with
///   `recv_timeout` under a fixed grace period. If nothing arrives in time, this function
///   returns with whatever it has (often nothing) and the reader thread is abandoned mid-read,
///   still blocked on the pipe. That is a leaked thread — a bounded, one-time cost — traded
///   deliberately against the alternative, which is the supervisor thread itself hanging past
///   its own deadline, exactly the failure this function exists to prevent.
pub fn run_bounded(
    argv: &[String],
    cwd: Option<&Path>,
    drop_vars: &[&str],
    limit: Duration,
) -> Completed {
    let Some((program, rest)) = argv.split_first() else {
        return Completed {
            stdout: String::new(),
            stderr: "empty argv".into(),
            code: None,
        };
    };
    let mut command = Command::new(program);
    command.args(rest);
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    for var in drop_vars {
        command.env_remove(var);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            return Completed {
                stdout: String::new(),
                stderr: e.to_string(),
                code: None,
            };
        }
    };
    let child_id = child.id();

    let mut stdout_pipe = child.stdout.take().expect("stdout was piped above");
    let mut stderr_pipe = child.stderr.take().expect("stderr was piped above");
    let (stdout_tx, stdout_rx) = std::sync::mpsc::channel();
    let (stderr_tx, stderr_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        let _ = stdout_tx.send(buf);
    });
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        let _ = stderr_tx.send(buf);
    });

    let deadline = Instant::now() + limit;
    let poll_interval = Duration::from_millis(50);
    let mut timed_out = false;
    let exit_code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) => {
                if Instant::now() >= deadline {
                    timed_out = true;
                    break None;
                }
                std::thread::sleep(poll_interval);
            }
            Err(_) => break None,
        }
    };
    if timed_out {
        let _ = child.kill();
        kill_process_group_best_effort(child_id);
        let _ = child.wait();
    }

    let collection_deadline = Instant::now() + Duration::from_secs(2);
    let remaining = || collection_deadline.saturating_duration_since(Instant::now());
    let stdout = stdout_rx.recv_timeout(remaining()).unwrap_or_default();
    let stderr_tail =
        String::from_utf8_lossy(&stderr_rx.recv_timeout(remaining()).unwrap_or_default())
            .into_owned();

    Completed {
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: if timed_out {
            format!(
                "killed after exceeding the {}s deadline\nstderr:\n{stderr_tail}",
                limit.as_secs()
            )
        } else {
            stderr_tail
        },
        code: exit_code,
    }
}

/// Signal the whole process group `run_bounded`'s child leads (its pid doubles as the group id
/// after `process_group(0)`), so an ordinary backgrounded grandchild (`sleep 30 &`, `tail -f`)
/// dies alongside it rather than being orphaned onto whatever reaps stray processes on this
/// host. No syscall for this is in `std`, and `libc` is outside the crate's dependency budget
/// (ADR-0005/ADR-0016), so the signal goes out via a spawned `kill` — `world` is exactly the
/// module allowed to do that.
///
/// Deliberately swallows every failure: a missing `kill` binary or a group that already has no
/// members look the same from here, and neither may become load-bearing — `run_bounded`'s
/// bounded-collection step is the actual guarantee. A daemonizing grandchild that double-forked
/// into its own session is not in this group at all and is not reached by this either way.
///
/// **The caller owns the one hazard this cannot swallow.** Signalling a *recycled* pid is not a
/// failure that lands here as an error — it is a successful SIGKILL of an unrelated process
/// group. So this must only ever be called while the child is still unreaped, which keeps its
/// pid out of circulation and the group id reserved by the zombie leader. `run_bounded` calls it
/// between `kill()` and `wait()` for exactly that reason; do not move it after the reap, and do
/// not call it with a pid whose child has already been waited on.
#[cfg(unix)]
fn kill_process_group_best_effort(pgid: u32) {
    let _ = Command::new("kill")
        .arg("-KILL")
        .arg(format!("-{pgid}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(unix))]
fn kill_process_group_best_effort(_pgid: u32) {}

/// The long-lived `claude` child. **The real seam, and it is the binary path** — only a real
/// process replays real SIGKILL, real empty-not-truncated stdout, a real separate stderr file
/// and a real exit code the parent did not choose.
///
/// Both streams are redirected to their files *before* the child is spawned, so the raw is on
/// disk by construction rather than by remembering to write it. `attempt` is what makes
/// *parse before write* uncallable; this is what makes it true even for a child that is killed
/// mid-write.
pub fn spawn_recorded(
    argv: &[String],
    cwd: &Path,
    prompt: &str,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<Option<i32>, String> {
    let Some((program, rest)) = argv.split_first() else {
        return Err("empty argv".to_string());
    };
    let out_file = File::create(stdout_path).map_err(|e| e.to_string())?;
    let err_file = File::create(stderr_path).map_err(|e| e.to_string())?;

    let mut child = Command::new(program)
        .args(rest)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(out_file))
        .stderr(Stdio::from(err_file))
        .spawn()
        .map_err(|e| e.to_string())?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes());
    }

    child
        .wait()
        .map(|status| status.code())
        .map_err(|e| e.to_string())
}

pub fn read_to_string(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn read_bytes(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn write(path: &Path, contents: &str) -> Result<(), String> {
    fs::write(path, contents).map_err(|e| format!("{}: {e}", path.display()))
}

/// Write via a temporary file in the same directory, then rename over the target. A crash
/// between the two leaves the **old** `run.json` intact, because the temp file is the only
/// thing that can be half-written. A plain write truncates the real file instead.
///
/// The scratch file is **fsynced before the rename**. Rename is atomic against process death
/// on its own, but not against power loss: without the flush, the directory entry can be
/// renamed while its data still sits in the page cache, and a cut then leaves a `run.json`
/// that is the right length and full of zeros — or truncated. The record is the sole account
/// of attempts that cost real money, so *old intact or new complete* has to hold across the
/// power cutting too, which is what `sync_all` buys.
pub fn write_atomic(path: &Path, contents: &str) -> Result<(), String> {
    let scratch = path.with_extension("tmp");
    let mut file = File::create(&scratch).map_err(|e| format!("{}: {e}", scratch.display()))?;
    file.write_all(contents.as_bytes())
        .map_err(|e| format!("{}: {e}", scratch.display()))?;
    file.sync_all()
        .map_err(|e| format!("{}: {e}", scratch.display()))?;
    drop(file);
    fs::rename(&scratch, path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn create_dir_all(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Every path directly under `dir` whose extension matches, sorted. An unreadable directory
/// yields nothing rather than an error: *no such directory* and *nothing in it* are the same
/// fact to every caller here, and the callers that need the difference ask `exists` first.
pub fn list_with_extension(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == extension))
        .collect();
    found.sort();
    found
}

/// Every directory entry directly under `dir`, sorted by file name.
pub fn list_dir(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    found.sort();
    found
}

pub fn mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

pub fn exists(path: &Path) -> bool {
    path.exists()
}

pub fn is_dir(path: &Path) -> bool {
    path.is_dir()
}

pub fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The target of a symlink, followed all the way. Used to tell a real `claude` from a shim,
/// which is asserted loudly rather than filtered for.
pub fn resolve_link(path: &Path) -> Option<PathBuf> {
    fs::canonicalize(path).ok()
}

/// `File::try_lock` on a file the kernel releases when its holder dies — killed or not. There
/// is no `running` state in the record, so a state-based check would refuse dispatch onto a
/// branch nothing is actually touching, forever.
pub fn try_lock(path: &Path) -> TryLock {
    let file = match File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
    {
        Ok(f) => f,
        Err(e) => return TryLock::Failed(format!("{}: {e}", path.display())),
    };
    match file.try_lock() {
        Ok(()) => TryLock::Acquired(LockHandle(file)),
        Err(fs::TryLockError::WouldBlock) => TryLock::WouldBlock,
        Err(fs::TryLockError::Error(e)) => TryLock::Failed(format!("{}: {e}", path.display())),
    }
}

/// `$HOME` is the only environment variable Grind reads, and there is no override. A unit with
/// a different `User=` resolves a different `~/.grind` and fails loudly — which is the point
/// (ADR-0008). `GRIND_HOME` would be the same mechanism that made a temp directory unsafe,
/// moved to the root of the tree.
pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// The record names the host holding it, because it does not travel.
pub fn hostname() -> Option<String> {
    let out = run(&["uname".to_string(), "-n".to_string()], None);
    let name = out.stdout.trim().to_string();
    (out.code == Some(0) && !name.is_empty()).then_some(name)
}

pub fn pid() -> u32 {
    std::process::id()
}

/// Ask `ps` when the process under this pid started — the supervisor's identity beside its pid,
/// because a pid alone is reused and a reused pid reporting a dead Run as alive is the thing the
/// split exists to stop.
///
/// **The raw triple, classified by `observe::process_start_stamp`.** The collapse this used to
/// perform here — `code == Some(0) && !stamp.is_empty()` folded into an `Option` — read a `ps`
/// that could not run as *no such process*, which `resume --all` then acts on.
pub fn ps_start_stamp(pid: u32) -> Completed {
    run(
        &[
            "ps".to_string(),
            "-p".to_string(),
            pid.to_string(),
            "-o".to_string(),
            "lstart=".to_string(),
        ],
        None,
    )
}

/// Here rather than in `cli` because `std::env` is named in one module, and `cli` parses argv
/// by hand.
pub fn args() -> Vec<String> {
    std::env::args().skip(1).collect()
}

/// Read one named environment variable. Host facts remain `$HOME`-only (ADR-0008);
/// this exists for provider credentials, which are read-at-use values that are never
/// recorded anywhere (ADR-0017) — the agent harness resolves them fresh per attempt
/// instead of letting them enter the RunRecord.
pub fn var(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("{name} not set"))
}

/// This binary's own path, so a boot one-shot re-enters with the copy that is running rather
/// than with whatever `PATH` resolves to under a service manager's environment.
pub fn current_exe() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

/// A child that **outlives this process**, in its own process group.
///
/// Never a thread: Rust terminates detached threads when `main` returns, and the boot one-shot's
/// whole shape is *spawn and exit*, so a thread-per-Run boot path re-enters nothing and reports
/// success while doing it. The new process group is what keeps a SIGHUP to the parent's terminal
/// from taking the supervisors with it; the systemd unit's own `KillMode` is the other half, and
/// it lives in `dist/`.
///
/// Nothing is waited on and nothing is piped. The child's stdout and stderr are redirected to
/// `log` — appended, like the supervisor log beside the record — because nobody is watching a
/// detached child's streams: nulled, they swallow exactly the refusals the re-entering child can
/// hit before it ever reaches `supervise` (the dispatch lock's `WouldBlock`, an unreadable
/// record) at the moment `resume --all` has already reported *re-entered*. A log that cannot be
/// opened fails the spawn rather than starting a child whose first refusal would be invisible.
pub fn spawn_detached(argv: &[String], log: &Path) -> Result<u32, String> {
    use std::os::unix::process::CommandExt;
    let Some((program, rest)) = argv.split_first() else {
        return Err("empty argv".to_string());
    };
    let log_file = File::options()
        .create(true)
        .append(true)
        .open(log)
        .map_err(|e| format!("{}: {e}", log.display()))?;
    Command::new(program)
        .args(rest)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::from(
            log_file.try_clone().map_err(|e| e.to_string())?,
        ))
        .stderr(Stdio::from(log_file))
        .spawn()
        .map(|child| child.id())
        .map_err(|e| e.to_string())
}

pub fn exit(code: i32) -> ! {
    std::process::exit(code)
}

pub fn sleep(duration: Duration) {
    std::thread::sleep(duration);
}

/// The supervisor's own progress output. A Run is long and the caller detaches it, so
/// block-buffered stdout makes a working Run look dead — and a Run that looks dead gets
/// killed. Rust's stdout is line-buffered already; the flush is what makes that a property of
/// this function rather than of the standard library's current choice.
///
/// This, `cli`'s printing of the `String`s `render` returns, and [`append_line`] are the three
/// writers of output.
pub fn print_line(line: &str) {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// The supervisor's narration, to a file that outlives the terminal it was said to.
///
/// **Line-buffered and flushed per line, exactly as stdout already is**, so a working Run
/// reaching a file never looks dead. A log that cannot be written is not worth abandoning a Run
/// over, so the failure comes back as a value and the caller may ignore it.
pub fn append_line(path: &Path, line: &str) -> Result<(), String> {
    let mut file = File::options()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    writeln!(file, "{line}").map_err(|e| format!("{}: {e}", path.display()))?;
    file.flush().map_err(|e| format!("{}: {e}", path.display()))
}

/// Refusals go to stderr, so a Run's own output stays parseable when it is piped.
pub fn print_error(line: &str) {
    let mut err = std::io::stderr();
    let _ = writeln!(err, "{line}");
    let _ = err.flush();
}

pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `2026-08-06T12:26:20+00:00` — what the record stores.
pub fn now_iso() -> String {
    let (y, mo, d, h, mi, s) = civil(now_epoch());
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}+00:00")
}

/// `20260806-122620` — the leading half of a run id.
pub fn now_stamp() -> String {
    let (y, mo, d, h, mi, s) = civil(now_epoch());
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

fn civil(epoch: u64) -> (u64, u64, u64, u64, u64, u64) {
    let days = epoch / 86_400;
    let rem = epoch % 86_400;
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// Pure parse of `date +%H:%M`'s output. Tolerates a trailing newline; rejects anything
/// out of range or unparseable rather than guessing.
fn parse_hour_minute(s: &str) -> Option<(u32, u32)> {
    let trimmed = s.trim();
    let (h, m) = trimmed.split_once(':')?;
    let hour: u32 = h.parse().ok()?;
    let minute: u32 = m.parse().ok()?;
    (hour <= 23 && minute <= 59).then_some((hour, minute))
}

/// Shells out to the POSIX-guaranteed `date +%H:%M`, the only place `std`'s missing timezone
/// database gets filled in. `tz` sets the child's `TZ` when given; `None` leaves the child to
/// inherit the host's own zone. Any failure — spawn, exit status, or unparseable stdout —
/// comes back as `None` rather than a guess.
fn local_hour_minute(tz: Option<&str>) -> Option<(u32, u32)> {
    let mut command = Command::new("date");
    command.arg("+%H:%M");
    if let Some(value) = tz {
        command.env("TZ", value);
    }
    let out = command.output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_hour_minute(&String::from_utf8_lossy(&out.stdout))
}

/// The pure fallback seam: `reading` when the spawn succeeded, otherwise the UTC hour/minute
/// this process already knows from [`civil`]. Callable directly with a literal `None`, so the
/// fallback path is unit-testable without forcing `date` itself to fail.
fn local_or_utc(reading: Option<(u32, u32)>) -> (u32, u32) {
    reading.unwrap_or_else(|| {
        let (_, _, _, h, mi, _) = civil(now_epoch());
        (h as u32, mi as u32)
    })
}

/// The host-local `(hour, minute)`, falling back to UTC when `date` cannot be read.
pub fn now_local_hour_minute() -> (u32, u32) {
    local_or_utc(local_hour_minute(None))
}

/// A unique scratch directory under the system temporary directory. Test scaffolding —
/// `tests/topology.rs` keeps `std::fs` and `std::env` out of every other module, so the
/// tests that need a throwaway clone ask here. The caller removes what it creates.
///
/// Uniqueness comes from a process-global counter rather than wall-clock time: two tests
/// spawning concurrently can land inside the same nanosecond, and the loser's
/// [`remove_tree`] then deletes the winner's fixture mid-write.
///
/// Creation is exclusive with a retry on `AlreadyExists`: the counter restarts at zero
/// every process, pids get reused, and a test that panics skips its [`remove_tree`] — so
/// a stale tree from an aborted run can sit exactly where this name regenerates. Adopting
/// it would seed the new fixture with foreign files; advancing the sequence cannot.
#[cfg(test)]
pub fn temp_dir(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SCRATCH_SEQ: AtomicU64 = AtomicU64::new(0);
    loop {
        let path = std::env::temp_dir().join(format!(
            "grind-test-{tag}-{}-{}",
            std::process::id(),
            SCRATCH_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::create_dir(&path) {
            Ok(()) => return path,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => panic!("a scratch directory: {err}"),
        }
    }
}

/// The inverse of [`temp_dir`]: best-effort removal of a scratch tree.
#[cfg(test)]
pub fn remove_tree(path: &Path) {
    let _ = fs::remove_dir_all(path);
}

/// Test-only environment mutation. Exists so a test in another module (`tools`'s
/// credential-scrubbing test) can set up its fixture without naming `std::env` itself —
/// `world` stays the sole namer even from test code, the same reason [`temp_dir`] exists rather
/// than a test elsewhere calling `std::env::temp_dir` directly.
#[cfg(test)]
pub fn set_var_for_test(name: &str, value: &str) {
    unsafe { std::env::set_var(name, value) };
}

/// The inverse of [`set_var_for_test`].
#[cfg(test)]
pub fn remove_var_for_test(name: &str) {
    unsafe { std::env::remove_var(name) };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(argv: &[&str]) -> Vec<String> {
        argv.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_child_that_exits_normally_is_unaffected_by_the_bound() {
        let out = run_bounded(
            &words(&["sh", "-c", "echo out; echo err >&2; exit 7"]),
            None,
            &[],
            Duration::from_secs(5),
        );
        assert_eq!(out.code, Some(7));
        assert_eq!(out.stdout, "out\n");
        assert_eq!(out.stderr, "err\n");
    }

    #[test]
    fn a_child_that_outlives_the_deadline_is_killed_and_reported_as_a_loud_failure() {
        let out = run_bounded(
            &words(&["sh", "-c", "sleep 5"]),
            None,
            &[],
            Duration::from_millis(200),
        );
        assert_eq!(
            out.code, None,
            "a killed child never reports a real exit code"
        );
        assert!(
            out.stderr.contains("deadline"),
            "the model must be able to tell this was a timeout, not a crash: {}",
            out.stderr
        );
    }

    #[test]
    fn a_child_reading_stdin_hangs_forever_without_the_null_but_is_still_killed_on_time() {
        let out = run_bounded(&words(&["cat"]), None, &[], Duration::from_millis(500));
        assert!(out.code == Some(0) || out.code.is_none(), "{out:?}");
    }

    #[test]
    fn output_larger_than_a_pipe_buffer_does_not_deadlock_the_wait() {
        let out = run_bounded(
            &words(&[
                "sh",
                "-c",
                "yes out | head -c 1000000; (yes err | head -c 1000000) 1>&2",
            ]),
            None,
            &[],
            Duration::from_secs(10),
        );
        assert_eq!(out.code, Some(0));
        assert_eq!(out.stdout.len(), 1_000_000);
        assert_eq!(out.stderr.len(), 1_000_000);
    }

    #[test]
    fn a_backgrounded_grandchild_outliving_the_deadline_does_not_hang_the_call() {
        let start = Instant::now();
        let out = run_bounded(
            &words(&["sh", "-c", "sleep 30 & echo started"]),
            None,
            &[],
            Duration::from_secs(2),
        );
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(8),
            "run_bounded must return in bounded time even when the child backgrounds a \
             process that outlives it; took {elapsed:?}"
        );
        assert_eq!(
            out.code,
            Some(0),
            "the direct child (`sh`) exits cleanly: {out:?}"
        );
    }

    #[test]
    fn a_missing_program_still_comes_back_as_a_spawn_failure_not_a_timeout() {
        let out = run_bounded(
            &words(&["/no/such/program-grind-test"]),
            None,
            &[],
            Duration::from_secs(5),
        );
        assert_eq!(out.code, None);
        assert!(!out.stderr.contains("deadline"), "{}", out.stderr);
    }

    #[test]
    fn read_bytes_round_trips_what_was_written() {
        let dir = temp_dir("read-bytes");
        let path = dir.join("evidence.bin");
        let raw = b"\x00\x01\xffbytes".to_vec();
        fs::write(&path, &raw).expect("a scratch file");
        assert_eq!(read_bytes(&path), Ok(raw));
        remove_tree(&dir);
    }

    #[test]
    fn a_missing_path_is_an_error_naming_the_path() {
        let found = read_bytes(Path::new("/nowhere/that/exists/evidence.bin"));
        let Err(said) = found else {
            panic!("a missing file is an Err");
        };
        assert!(said.contains("/nowhere/that/exists/evidence.bin"), "{said}");
    }

    #[test]
    fn parse_hour_minute_tolerates_a_trailing_newline_and_rejects_garbage() {
        assert_eq!(parse_hour_minute("07:05\n"), Some((7, 5)));
        assert_eq!(parse_hour_minute("24:00"), None);
        assert_eq!(parse_hour_minute(""), None);
        assert_eq!(parse_hour_minute("resets 5pm"), None);
    }

    fn shifted(epoch: u64, offset_minutes: i64) -> (u32, u32) {
        let (_, _, _, h, mi, _) = civil(epoch);
        let total = ((h as i64) * 60 + mi as i64 + offset_minutes).rem_euclid(1440);
        ((total / 60) as u32, (total % 60) as u32)
    }

    /// Whether the `date` binary can be spawned at all here. Lets the zone tests below skip
    /// only on an environment that lacks `date`, rather than on any `None` from
    /// `local_hour_minute` — a `None` after a successful spawn is a real regression, not an
    /// environment gap, and must fail loudly.
    fn date_binary_available() -> bool {
        Command::new("date").arg("+%H:%M").output().is_ok()
    }

    #[test]
    fn local_hour_minute_reads_the_injected_zone_not_utc() {
        if !date_binary_available() {
            return;
        }
        let e1 = now_epoch();
        let got = local_hour_minute(Some("GRD-3"))
            .expect("`date` spawned; a non-zero exit or unparseable stdout is a regression");
        let e2 = now_epoch();
        assert!(
            got == shifted(e1, 180) || got == shifted(e2, 180),
            "{got:?} vs e1={:?} e2={:?}",
            shifted(e1, 180),
            shifted(e2, 180)
        );
    }

    #[test]
    fn local_hour_minute_at_zero_offset_pins_only_the_zone_moving() {
        if !date_binary_available() {
            return;
        }
        let e1 = now_epoch();
        let got = local_hour_minute(Some("UTC0"))
            .expect("`date` spawned; a non-zero exit or unparseable stdout is a regression");
        let e2 = now_epoch();
        assert!(
            got == shifted(e1, 0) || got == shifted(e2, 0),
            "{got:?} vs e1={:?} e2={:?}",
            shifted(e1, 0),
            shifted(e2, 0)
        );
    }

    #[test]
    fn local_or_utc_falls_back_to_the_civil_utc_reading() {
        let e1 = now_epoch();
        let got = local_or_utc(None);
        let e2 = now_epoch();
        assert!(
            got == shifted(e1, 0) || got == shifted(e2, 0),
            "{got:?} vs e1={:?} e2={:?}",
            shifted(e1, 0),
            shifted(e2, 0)
        );
    }
}
