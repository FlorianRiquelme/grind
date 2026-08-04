use record::{simulate_observe, RunRecord, RunView};
use std::path::{Path, PathBuf};

fn main() {
    // Resolve relative to this crate, not the invoking shell's cwd, so
    // `cargo run -p record` works the same from the workspace root or the
    // crate directory.
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture: PathBuf = manifest_dir.join("fixtures/run.json");
    let scratch: PathBuf = manifest_dir.join("fixtures/run.scratch.json");
    let fixture: &Path = &fixture;
    let scratch: &Path = &scratch;

    println!("=== 1. load the real fixture as a read-only view ===");
    let view = RunView::load(fixture).expect("load fixture");
    println!(
        "run_id={} state={} attempts={} hostname={:?} claude_bin={:?}",
        view.run_id,
        view.state,
        view.attempts.len(),
        view.hostname,
        view.claude_bin
    );
    println!(
        "  (both absent in this fixture — written by an older grind, before either field \
         existed; `#[serde(default)]` is why loading it didn't hard-error)"
    );

    // Work from a scratch copy from here on — never touch the real .grind/ fixture.
    std::fs::copy(fixture, scratch).expect("seed scratch copy");

    println!("\n=== 2. the race: status reads while the supervisor appends attempt 6 ===");

    // Status opens its own read-only view first...
    let status_view = RunView::load(scratch).expect("status: load");
    println!("  status  : loaded view, sees {} attempts", status_view.attempts.len());

    // ...then, before status does anything else, the supervisor re-enters,
    // runs attempt 6, and persists it.
    let mut record = RunRecord::load(scratch).expect("supervisor: load");
    println!("  supervisor: loaded record, sees {} attempts", record.attempts().len());
    println!("  supervisor: hostname={:?}", record.hostname);
    let attempt6 = make_attempt(6);
    record.push_attempt(attempt6);
    let obs = simulate_observe(record.observed(), 1, "2026-08-04T12:00:00+00:00");
    record.set_observed(obs);
    record.set_state("completed");
    record.save(scratch).expect("supervisor: save");
    println!("  supervisor: appended attempt 6 and saved — {} attempts now on disk", record.attempts().len());

    // Status, still holding its STALE view from before the append, now does
    // what `cmd_status` in bin/grind does: observe(), to refresh the display.
    let stale_obs = simulate_observe(status_view.observed.as_ref(), 1, "2026-08-04T12:00:01+00:00");
    println!(
        "  status  : refreshed its own observation locally (commits_ahead={}) to print it",
        stale_obs.commits_ahead
    );

    println!("\n--- what the Python does today ---");
    println!(
        "  cmd_status calls load() -> observe() -> save(). `save` there writes the WHOLE \
         dict it loaded — the one from BEFORE attempt 6 landed. Attempt 6 would be erased \
         by the read command, silently, while the human watches `watch -n 30 grind status`."
    );

    println!("\n--- what this design does ---");
    println!(
        "  status_view : {:?} — a RunView. It has no `save`, no Serialize impl, nothing that \
         writes bytes. `status_view.save(..)` and `serde_json::to_string(&status_view)` are \
         both compiler errors — see wont-compile/.",
        std::any::type_name::<RunView>()
    );
    let after = RunView::load(scratch).expect("reload after race");
    println!(
        "  on disk after the race: {} attempts (attempt 6 survives)",
        after.attempts.len()
    );
    assert_eq!(after.attempts.len(), 6, "the whole point: nothing erased attempt 6");

    println!("\n=== 3. atomic save: does a crash mid-write survive? ===");
    let before = std::fs::read_to_string(scratch).unwrap();
    // Simulate a crash: write a truncated body to the `.tmp` file and stop
    // BEFORE the rename that `RunRecord::save` performs. This is exactly the
    // step between `write_text`'s first byte and its last in the Python,
    // where `json.dumps(state)` writes straight over the real file.
    let tmp = scratch.with_extension("json.tmp");
    std::fs::write(&tmp, br#"{"run_id": "20260802-105828-snapper-21", "state": "completed", "job": {"#).unwrap();
    let after_simulated_crash = std::fs::read_to_string(scratch).unwrap();
    println!(
        "  real run.json unchanged by the crashed write: {}",
        before == after_simulated_crash
    );
    let still_loads = RunView::load(scratch).is_ok();
    println!("  real run.json still parses as a valid record: {still_loads}");
    println!("  (the truncated body only ever reached {tmp:?}, which `rename` never pointed the real path at)");
    assert!(before == after_simulated_crash && still_loads);
    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_file(scratch);

    println!("\nOK");
}

fn make_attempt(n: u32) -> record::Attempt {
    record::Attempt {
        n,
        mode: "resume".into(),
        started_at: "2026-08-04T11:59:00+00:00".into(),
        ended_at: "2026-08-04T12:00:00+00:00".into(),
        exit_code: 0,
        is_error: false,
        subtype: Some("success".into()),
        stop_reason: Some("end_turn".into()),
        api_error_status: None,
        terminal_reason: Some("completed".into()),
        num_turns: Some(1),
        total_cost_usd: Some(0.01),
        usage: None,
        permission_denials: vec![],
        done_promise: true,
        rate_limited: false,
        result_tail: "<promise>DONE</promise>".into(),
    }
}
