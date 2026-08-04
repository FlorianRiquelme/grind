pub mod supervisor;
pub mod types;
pub mod view;

pub use supervisor::RunRecord;
pub use types::{Attempt, Job, Observation, Pr, VerifyContract};
pub use view::RunView;

/// Stand-in for the real `observe()`, which shells out to `git`/`gh` against
/// a worktree. The spike doesn't have a worktree to shell against, so this
/// takes the previous observation (if any) and a `commits_ahead` delta and
/// returns a fresh `Observation` — same shape as the real thing, same idea:
/// a pure computation from the world's current state, independent of what
/// gets DONE with the result afterwards. Both the status path and the
/// supervisor path call exactly this function; what differs is only whether
/// the caller is holding a type that can persist the result.
pub fn simulate_observe(previous: Option<&Observation>, commits_delta: i64, at: &str) -> Observation {
    let commits_ahead = previous.map(|o| o.commits_ahead).unwrap_or(0) + commits_delta;
    Observation {
        observed_at: at.to_string(),
        observed_at_epoch: unix_now(),
        commits_ahead,
        plan_files: previous.map(|o| o.plan_files.clone()).unwrap_or_default(),
        residual_findings: previous.map(|o| o.residual_findings.clone()).unwrap_or_default(),
        ledger_entries: previous.map(|o| o.ledger_entries.clone()).unwrap_or_default(),
        pr: previous.and_then(|o| o.pr.clone()),
        verify_contract: previous
            .map(|o| o.verify_contract.clone())
            .unwrap_or(VerifyContract { justfile: false, present: vec![], missing: vec![] }),
        furthest_stage: previous.map(|o| o.furthest_stage.clone()).unwrap_or_else(|| "dispatched".into()),
    }
}

/// Current wall clock as unix-epoch seconds. No date/time crate available in
/// this spike, so `SystemTime`/`UNIX_EPOCH` — the plain-integer timestamp the
/// task calls for.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before 1970")
        .as_secs()
}

/// Human-readable age of an observation, e.g. "observed 45s ago". Used by
/// the status/read path to show how stale a reading is; the freshness of a
/// just-taken observation and a five-minute-old one look identical without
/// this.
pub fn freshness(observed_at_epoch: u64) -> String {
    let age = unix_now().saturating_sub(observed_at_epoch);
    format!("observed {age}s ago")
}
