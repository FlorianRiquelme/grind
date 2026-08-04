// SNEAK-PAST ATTEMPT 2 — also compiles. The deeper hole.
//
// Even if the status path is disciplined about only ever importing
// `RunView`, Rust's type system has no concept of "this code is not allowed
// to touch this file." `RunView::load` and `RunRecord::save` both take a
// `&Path` that the CALLER constructed — the record crate does not own or
// gate the run.json path itself. Any code in the binary that knows the path
// (and every command does, since they all resolve `RUNS_DIR / run_id /
// "run.json"` to find the record at all) can `std::fs::write` over it
// directly, bytes it built however it likes, with no dependency on
// `RunView`, `RunRecord`, or serde at all.
//
// A capability-based fix would need the *path itself* to be a write-gated
// handle, not a plain `&Path` any caller can reconstruct — out of scope for
// this spike, and noted as the honest limit of the type-only design in
// FINDINGS.md.

use std::path::Path;

fn main() {
    let path = Path::new("../../fixtures/run.json");
    std::fs::write(path, b"{\"anything\": \"goes\"}").unwrap();
}
