//! The dispatch lock, exercised against real processes.
//!
//! These live here rather than in a `#[cfg(test)]` module for a mechanical reason: killing a
//! holder needs a second process, which would name `std::process` inside `src/` and trip
//! `tests/topology.rs`. Integration tests are separate crates, so the conflict dissolves.

use grind::supervisor::{lock_key, lock_path, take_lock};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const HOME_FOR_HOLDER: &str = "GRIND_TEST_LOCK_HOME";
const REPO: &str = "FlorianRiquelme/snapper";
const BRANCH: &str = "feat/28-slice-1b-agent-surface-screensource-seam";

fn scratch_home(name: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!("grind-lock-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(home.join(".grind").join("locks")).expect("a scratch home");
    home
}

/// Re-exec this very test binary, running only the ignored holder below. It acquires the lock
/// and then sits on it, exactly like a supervisor inside its loop.
fn spawn_a_holder(home: &Path) -> Child {
    let child = Command::new(std::env::current_exe().expect("this test binary"))
        .args([
            "--exact",
            "a_held_lock_outlives_this_call",
            "--ignored",
            "--nocapture",
        ])
        .env(HOME_FOR_HOLDER, home)
        .spawn()
        .expect("spawn a second process");
    let acquired = home.join("acquired");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !acquired.exists() {
        assert!(
            Instant::now() < deadline,
            "the holder never acquired the lock"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    child
}

/// Not a test. The holder half of the two-process scenarios, run only when named explicitly.
#[test]
#[ignore = "the holder half of the lock scenarios; spawned by them, never run on its own"]
fn a_held_lock_outlives_this_call() {
    let home = PathBuf::from(std::env::var(HOME_FOR_HOLDER).expect("a home to lock under"));
    let _held = take_lock(&home, REPO, BRANCH).expect("the holder must acquire");
    fs::write(home.join("acquired"), "yes").expect("signal readiness");
    std::thread::sleep(Duration::from_secs(120));
}

#[test]
fn two_worktrees_of_one_repo_on_one_branch_collide() {
    let home = scratch_home("collide");
    let mut holder = spawn_a_holder(&home);

    let refused = take_lock(&home, REPO, BRANCH).expect_err("the second Dispatch must be refused");
    assert!(
        refused.to_string().contains("already holds"),
        "a collision must read as a collision: {refused}"
    );
    assert!(!refused.to_string().contains("another Run"), "{refused}");

    let _ = holder.kill();
    let _ = holder.wait();
}

#[test]
fn a_second_dispatch_while_the_first_supervisor_is_inside_its_loop_is_refused() {
    let home = scratch_home("in-flight");
    let mut holder = spawn_a_holder(&home);
    assert!(take_lock(&home, REPO, BRANCH).is_err());
    let _ = holder.kill();
    let _ = holder.wait();
}

#[test]
fn the_kernel_releases_the_lock_when_its_holder_is_killed() {
    let home = scratch_home("sigkill");
    let mut holder = spawn_a_holder(&home);
    assert!(take_lock(&home, REPO, BRANCH).is_err(), "held while alive");

    holder.kill().expect("kill the holder");
    holder.wait().expect("reap the holder");

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if take_lock(&home, REPO, BRANCH).is_ok() {
            break;
        }
        assert!(Instant::now() < deadline, "the lock was never released");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_branch_full_of_slashes_locks_as_one_file() {
    let home = scratch_home("slashes");
    let held = take_lock(&home, REPO, BRANCH);
    assert!(
        held.is_ok(),
        "a slash must never become a path separator: {held:?}"
    );
    let path = lock_path(&home, REPO, BRANCH);
    assert!(path.exists());
    assert_eq!(path.parent().unwrap(), home.join(".grind").join("locks"));
    assert!(!lock_key(REPO, BRANCH).contains('/'));
}

#[test]
fn a_lock_that_cannot_be_opened_is_could_not_determine_and_never_a_collision() {
    let home = scratch_home("undetermined");
    let blocked = lock_path(&home, REPO, BRANCH);
    fs::create_dir_all(&blocked).expect("put a directory where the lock file goes");

    let refused = take_lock(&home, REPO, BRANCH).expect_err("a directory cannot be locked");
    let said = refused.to_string();
    assert!(said.contains("could not determine"), "{said}");
    assert!(
        !said.contains("already holds"),
        "could-not-determine must never read as a collision: {said}"
    );
}

#[test]
fn a_refused_dispatch_reads_as_incoherent_input_and_carries_no_quality_language() {
    let home = scratch_home("register");
    let mut holder = spawn_a_holder(&home);
    let refused = take_lock(&home, REPO, BRANCH)
        .expect_err("refused")
        .to_string();
    for banned in [
        "bad", "invalid", "wrong", "fail", "error", "reject", "should",
    ] {
        assert!(
            !refused.to_lowercase().contains(banned),
            "a refused Dispatch is incoherent input, not a judgement: {refused}"
        );
    }
    let _ = holder.kill();
    let _ = holder.wait();
}

#[test]
fn different_branches_of_one_repo_do_not_collide() {
    let home = scratch_home("distinct");
    let _first = take_lock(&home, REPO, "feat/28-one").expect("first branch");
    let second = take_lock(&home, REPO, "feat/29-two");
    assert!(
        second.is_ok(),
        "two Runs on two branches are independent: {second:?}"
    );
}
