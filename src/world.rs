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
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

// --- children -------------------------------------------------------------------------

/// One short-lived child (`git`, `gh`, `ps`). Concrete, no trait: a trait here would be a
/// hypothetical seam with one production impl and one test impl, both in Rust, and the actual
/// spawn path exercised by neither (ADR-0007).
pub fn run(argv: &[String], cwd: Option<&Path>) -> Completed {
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
        // Dropping stdin closes it, so the child sees EOF.
    }

    child
        .wait()
        .map(|status| status.code())
        .map_err(|e| e.to_string())
}

// --- the filesystem -------------------------------------------------------------------

pub fn read_to_string(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn write(path: &Path, contents: &str) -> Result<(), String> {
    fs::write(path, contents).map_err(|e| format!("{}: {e}", path.display()))
}

/// Write via a temporary file in the same directory, then rename over the target. A crash
/// between the two leaves the **old** `run.json` intact, because the temp file is the only
/// thing that can be half-written. A plain write truncates the real file instead.
pub fn write_atomic(path: &Path, contents: &str) -> Result<(), String> {
    let scratch = path.with_extension("tmp");
    fs::write(&scratch, contents).map_err(|e| format!("{}: {e}", scratch.display()))?;
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

// --- the process and its environment --------------------------------------------------

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

/// The supervisor's identity beside its pid: a pid alone is reused, and a reused pid reporting
/// a dead Run as alive is the thing the split exists to stop.
pub fn process_start_stamp(pid: u32) -> Option<String> {
    let out = run(
        &[
            "ps".to_string(),
            "-p".to_string(),
            pid.to_string(),
            "-o".to_string(),
            "lstart=".to_string(),
        ],
        None,
    );
    let stamp = out.stdout.trim().to_string();
    (out.code == Some(0) && !stamp.is_empty()).then_some(stamp)
}

/// Here rather than in `cli` because `std::env` is named in one module, and `cli` parses argv
/// by hand.
pub fn args() -> Vec<String> {
    std::env::args().skip(1).collect()
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

// --- the wall clock -------------------------------------------------------------------
//
// Civil-time arithmetic lives here, beside the clock read, rather than in a producer. The
// alternatives are worse: two producers would duplicate it, and a module named for a noun two
// others share is exactly what ADR-0007 forbids. It is proleptic-Gregorian and UTC-only,
// which is all a record needs.

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
    // Howard Hinnant's civil_from_days, shifted to a March-based year so the leap day is last.
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
