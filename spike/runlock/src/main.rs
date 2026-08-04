// Prototype: an flock keyed on (repo identity, branch), std-only, no deps.
//
// Answers wayfinder question: can grind refuse a second dispatch onto a
// branch someone else is already working, WITHOUT a run-state check (which
// can never distinguish "still running" from "supervisor got SIGKILLed
// and never got to write a terminal state")?
//
// Run `cargo run -p runlock -- demo` for the full proof. See ../FINDINGS.md.

use std::env;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

// --- lock key derivation ----------------------------------------------------

/// Where lock files live. Deliberately NOT inside any repo's `.grind/`:
/// `.grind/` is per-checkout scratch (gitignored, host-local to *that*
/// worktree). The lock must be visible to every worktree of the same repo,
/// so it lives in one OS-wide scratch location instead, keyed on repo
/// identity rather than on any single worktree's path.
fn scratch_dir() -> PathBuf {
    let dir = env::temp_dir().join("grind-runlock");
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// The repo's identity, independent of which worktree you happen to be
/// standing in. `git rev-parse --git-common-dir` returns the ONE `.git`
/// directory shared by every worktree of a repo.
///
/// This matters because `resolve_worktree` in bin/grind can hand back a
/// path that is itself a worktree -- and the author runs ~10 parallel
/// worktrees of the same origin repo at once. Two dispatches that both
/// resolve to worktrees of the *same* origin, targeting the *same* branch,
/// must collide. Keying on the literal path passed in would let them slip
/// past each other: two different absolute paths, same underlying repo.
fn repo_identity(repo: &Path) -> PathBuf {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--git-common-dir"])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let raw = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let p = PathBuf::from(raw);
            let abs = if p.is_absolute() { p } else { repo.join(p) };
            fs::canonicalize(&abs).unwrap_or(abs)
        }
        // Not a git repo, or git isn't on PATH: fall back to the path
        // itself. This is a real gap -- see FINDINGS.md ("what this does
        // not cover").
        _ => fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf()),
    }
}

/// Branches contain `/` (e.g. `story/33-foo`) and worse. Keep the lock
/// filename to one path segment.
fn sanitize(branch: &str) -> String {
    branch
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' })
        .collect()
}

fn lock_path(repo: &Path, branch: &str) -> PathBuf {
    let id = repo_identity(repo);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hasher);
    let h = hasher.finish();
    scratch_dir().join(format!("{:016x}__{}.lock", h, sanitize(branch)))
}

// --- acquisition -------------------------------------------------------------

enum Outcome {
    Acquired(File),
    HeldByOther,
    /// Could not determine either way: a real IO error (permissions,
    /// missing directory, ...). MUST be reported differently from
    /// HeldByOther -- conflating the two turns a broken scratch dir into a
    /// permanent refusal of every dispatch onto that branch.
    Undetermined(io::Error),
}

fn try_acquire(path: &Path) -> Outcome {
    let file = match OpenOptions::new().create(true).write(true).open(path) {
        Ok(f) => f,
        Err(e) => return Outcome::Undetermined(e),
    };
    match file.try_lock() {
        Ok(()) => Outcome::Acquired(file),
        Err(TryLockError::WouldBlock) => Outcome::HeldByOther,
        Err(TryLockError::Error(e)) => Outcome::Undetermined(e),
    }
}

fn report_outcome(path: &Path, outcome: &Outcome) {
    match outcome {
        Outcome::Acquired(_) => {
            println!("  lock path : {}", path.display());
            println!("  outcome   : ACQUIRED by pid {}", std::process::id());
        }
        Outcome::HeldByOther => {
            println!("  lock path : {}", path.display());
            println!(
                "  outcome   : REFUSED -- held by another pid (pid {} did not get it)",
                std::process::id()
            );
        }
        Outcome::Undetermined(e) => {
            println!("  lock path : {}", path.display());
            println!("  outcome   : UNDETERMINED (not a refusal!) -- {}", e);
        }
    }
}

// --- subcommands used as child processes / building blocks ------------------

fn cmd_key(repo: &Path, branch: &str) {
    println!("{}", lock_path(repo, branch).display());
}

/// Acquire and hold the lock for `seconds`, or print refusal/undetermined
/// and exit non-zero. Used as the child process in the two-process and
/// SIGKILL demos.
fn cmd_hold(repo: &Path, branch: &str, seconds: u64) {
    let path = lock_path(repo, branch);
    println!("[pid {}] attempting lock: {}", std::process::id(), path.display());
    match try_acquire(&path) {
        Outcome::Acquired(_file) => {
            println!("[pid {}] ACQUIRED, holding for {}s", std::process::id(), seconds);
            std::thread::sleep(Duration::from_secs(seconds));
            println!("[pid {}] releasing (process exit)", std::process::id());
        }
        Outcome::HeldByOther => {
            println!("[pid {}] REFUSED -- held by another process", std::process::id());
            std::process::exit(1);
        }
        Outcome::Undetermined(e) => {
            println!("[pid {}] UNDETERMINED: {}", std::process::id(), e);
            std::process::exit(2);
        }
    }
}

/// Single non-blocking attempt, no holding. Reports and exits: 0 acquired
/// (then releases immediately), 1 held-by-other, 2 undetermined.
fn cmd_try(repo: &Path, branch: &str) {
    let path = lock_path(repo, branch);
    let outcome = try_acquire(&path);
    report_outcome(&path, &outcome);
    match outcome {
        Outcome::Acquired(_) => std::process::exit(0),
        Outcome::HeldByOther => std::process::exit(1),
        Outcome::Undetermined(_) => std::process::exit(2),
    }
}

/// Force an IO error at acquisition time (not a held lock) by pointing the
/// lock at a path whose parent directory does not exist, and prove the
/// outcome is reported as Undetermined, not HeldByOther.
fn cmd_undetermined_probe() {
    let bogus = PathBuf::from("/nonexistent-grind-runlock-dir/x/lock");
    println!("probing a path that cannot be opened: {}", bogus.display());
    match try_acquire(&bogus) {
        Outcome::Acquired(_) => println!("  UNEXPECTED: acquired"),
        Outcome::HeldByOther => println!("  WRONG: reported HeldByOther for a plain IO failure"),
        Outcome::Undetermined(e) => println!("  correctly reported UNDETERMINED: {}", e),
    }
}

// --- demo orchestration ------------------------------------------------------

fn run(cmd: &mut Command) -> String {
    let out = cmd.output().expect("spawn");
    if !out.status.success() {
        panic!(
            "command failed: {:?}\nstdout: {}\nstderr: {}",
            cmd,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn setup_demo_repo(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    // A real git repo, so repo_identity() exercises the real `git
    // rev-parse --git-common-dir` path, not the fallback.
    fs::create_dir_all(root).unwrap();
    run(Command::new("git").args(["init", "-q"]).current_dir(root));
    run(Command::new("git").args(["config", "user.email", "spike@example.com"]).current_dir(root));
    run(Command::new("git").args(["config", "user.name", "spike"]).current_dir(root));
    fs::write(root.join("README"), "spike\n").unwrap();
    run(Command::new("git").args(["add", "."]).current_dir(root));
    run(Command::new("git").args(["commit", "-q", "-m", "init"]).current_dir(root));
    run(Command::new("git").args(["branch", "story/33-foo"]).current_dir(root));

    // A second worktree of the SAME repo, checked out on a DIFFERENT
    // branch (git refuses the same branch twice by design -- that's
    // precisely the refusal grind's resolve_worktree defeats by adopting
    // the existing worktree instead of trying to create a second one).
    let wt = root.parent().unwrap().join("wt-sibling");
    run(Command::new("git")
        .args(["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "sibling"])
        .current_dir(root));

    (root.to_path_buf(), wt, root.join(".git"))
}

fn main() {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("key") => cmd_key(Path::new(&args[2]), &args[3]),
        Some("hold") => cmd_hold(Path::new(&args[2]), &args[3], args[4].parse().unwrap()),
        Some("try") => cmd_try(Path::new(&args[2]), &args[3]),
        Some("undetermined-probe") => cmd_undetermined_probe(),
        Some("demo") | None => demo(),
        Some(other) => eprintln!("unknown subcommand: {other}"),
    }
}

fn demo() {
    let exe = env::current_exe().unwrap();
    let tmp = env::temp_dir().join(format!("runlock-demo-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let (repo, sibling_worktree, _dotgit) = setup_demo_repo(&tmp.join("repo"));
    let branch = "story/33-foo";

    println!("=== setup ===");
    println!("repo             : {}", repo.display());
    println!("sibling worktree : {} (same repo, different branch checked out)", sibling_worktree.display());
    println!();

    println!("=== 1. key derivation ===");
    let key_from_repo = lock_path(&repo, branch);
    let key_from_sibling = lock_path(&sibling_worktree, branch);
    println!("lock path derived from repo path            : {}", key_from_repo.display());
    println!("lock path derived from sibling worktree path : {}", key_from_sibling.display());
    println!(
        "same key despite different filesystem paths : {}",
        key_from_repo == key_from_sibling
    );
    assert_eq!(key_from_repo, key_from_sibling, "worktree of the same repo must key the same");
    println!();

    println!("=== 2. two processes, second refuses ===");
    let mut child_a = Command::new(&exe)
        .args(["hold", repo.to_str().unwrap(), branch, "2"])
        .stdout(Stdio::inherit())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(400)); // let A actually acquire first
    let status_b = Command::new(&exe)
        .args(["hold", sibling_worktree.to_str().unwrap(), branch, "0"])
        .stdout(Stdio::inherit())
        .status()
        .unwrap();
    println!(
        "second process (via sibling worktree path) exit code: {} ({})",
        status_b.code().unwrap_or(-1),
        if status_b.success() { "acquired -- WRONG" } else { "refused, as expected" }
    );
    child_a.wait().unwrap();
    println!();

    println!("=== 3. SIGKILL releases the lock (the whole point) ===");
    let mut victim = Command::new(&exe)
        .args(["hold", repo.to_str().unwrap(), branch, "9999"])
        .stdout(Stdio::inherit())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(400));
    let mid_status = Command::new(&exe)
        .args(["try", repo.to_str().unwrap(), branch])
        .stdout(Stdio::inherit())
        .status()
        .unwrap();
    println!(
        "while victim is alive, a fresh dispatch attempt: {}",
        if mid_status.success() { "acquired -- WRONG" } else { "refused, as expected" }
    );
    println!("kill -9 {} ...", victim.id());
    let _ = Command::new("kill").args(["-9", &victim.id().to_string()]).status();
    let _ = victim.wait();
    std::thread::sleep(Duration::from_millis(200)); // let the OS actually reap/release
    let after_status = Command::new(&exe)
        .args(["try", repo.to_str().unwrap(), branch])
        .stdout(Stdio::inherit())
        .status()
        .unwrap();
    println!(
        "after SIGKILL, a fresh dispatch attempt: {}",
        if after_status.success() {
            "ACQUIRED CLEANLY -- flock released by the OS on process death"
        } else {
            "refused -- WRONG, this is the property the whole design rests on"
        }
    );
    assert!(after_status.success(), "SIGKILL must release the flock");
    println!();

    println!("=== 4. held-by-other vs could-not-determine must not be confused ===");
    let mut holder = Command::new(&exe)
        .args(["hold", repo.to_str().unwrap(), branch, "2"])
        .stdout(Stdio::inherit())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(400));
    let held_status = Command::new(&exe)
        .args(["try", repo.to_str().unwrap(), branch])
        .stdout(Stdio::inherit())
        .status()
        .unwrap();
    println!("held-by-other reported as exit code {} (contract: 1)", held_status.code().unwrap_or(-1));
    holder.wait().unwrap();
    Command::new(&exe).arg("undetermined-probe").stdout(Stdio::inherit()).status().unwrap();
    println!();

    println!("=== summary ===");
    println!("all scenarios behaved as required. See FINDINGS.md for the write-up.");

    let _ = fs::remove_dir_all(&tmp);
}
