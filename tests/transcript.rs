//! Progress is the newest write across the parent transcript **and every fan-out subagent
//! transcript**.
//!
//! Here rather than in a `#[cfg(test)]` module because setting an mtime names `std::fs`, which
//! inside `src/` would trip `tests/topology.rs`. Git carries no mtimes either, so the fixture's
//! times are set at run time — which is the honest way to test a freshness reading anyway.

use grind::view::newest_write;
use std::fs::{self, FileTimes};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const SESSION: &str = "8f2c1a70-4b3d-4e51-9c02-6a7d5e8b1f43";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/transcript/fanout")
}

/// A private copy of the fan-out fixture, so the times set below never touch the checked-in one.
fn scratch_fanout(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("grind-transcript-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let subagents = root.join(SESSION).join("subagents");
    fs::create_dir_all(&subagents).expect("a scratch transcript tree");
    fs::copy(
        fixtures().join(format!("{SESSION}.jsonl")),
        root.join(format!("{SESSION}.jsonl")),
    )
    .expect("copy the parent transcript");
    for agent in ["agent-01.jsonl", "agent-02.jsonl"] {
        fs::copy(
            fixtures().join(SESSION).join("subagents").join(agent),
            subagents.join(agent),
        )
        .expect("copy a subagent transcript");
    }
    root
}

fn set_mtime(path: &Path, at: SystemTime) {
    fs::File::options()
        .write(true)
        .open(path)
        .expect("open for a time change")
        .set_times(FileTimes::new().set_modified(at))
        .expect("set the mtime");
}

fn parent_of(root: &Path) -> PathBuf {
    root.join(format!("{SESSION}.jsonl"))
}

fn subagent_of(root: &Path, agent: &str) -> PathBuf {
    root.join(SESSION).join("subagents").join(agent)
}

#[test]
fn a_subagent_writing_more_recently_than_the_parent_wins() {
    // A fan-out makes the parent go quiet while subagents work. Reading the parent alone
    // misreads the quietest healthy phase of a pipeline as a stall, and sends the operator to
    // kill a working Run.
    let root = scratch_fanout("subagent-newest");
    let base = SystemTime::now() - Duration::from_secs(600);
    set_mtime(&parent_of(&root), base);
    set_mtime(
        &subagent_of(&root, "agent-01.jsonl"),
        base + Duration::from_secs(120),
    );
    let newest_subagent = base + Duration::from_secs(540);
    set_mtime(&subagent_of(&root, "agent-02.jsonl"), newest_subagent);

    let found = newest_write(&parent_of(&root)).expect("a newest write");
    let drift = found
        .duration_since(newest_subagent)
        .or_else(|_| newest_subagent.duration_since(found))
        .expect("a comparable time");
    assert!(
        drift < Duration::from_secs(1),
        "the newest subagent write must win over the parent's"
    );
}

#[test]
fn the_parent_wins_when_it_is_the_newest() {
    let root = scratch_fanout("parent-newest");
    let base = SystemTime::now() - Duration::from_secs(600);
    let parent_at = base + Duration::from_secs(590);
    set_mtime(&parent_of(&root), parent_at);
    set_mtime(&subagent_of(&root, "agent-01.jsonl"), base);
    set_mtime(
        &subagent_of(&root, "agent-02.jsonl"),
        base + Duration::from_secs(60),
    );

    let found = newest_write(&parent_of(&root)).expect("a newest write");
    let drift = found
        .duration_since(parent_at)
        .or_else(|_| parent_at.duration_since(found))
        .expect("a comparable time");
    assert!(drift < Duration::from_secs(1));
}

#[test]
fn a_session_that_never_fanned_out_still_reads_its_own_write() {
    // Most sessions never fan out, so a missing subagents directory is not an error.
    let root = scratch_fanout("no-fanout");
    fs::remove_dir_all(root.join(SESSION)).expect("drop the fan-out");
    assert!(newest_write(&parent_of(&root)).is_some());
}

#[test]
fn a_transcript_that_is_not_there_yields_no_time_rather_than_a_zero() {
    // Zero would render as *written in 1970*, which reads as a fact. Nothing is the honest
    // answer, and the view spells it as could-not-observe.
    assert!(newest_write(Path::new("/nowhere/that/exists/none.jsonl")).is_none());
}
