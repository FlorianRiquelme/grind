// SNEAK-PAST ATTEMPT — this one COMPILES. It is the hole in the design.
//
// The type-level guard only holds if the status call site actually reaches
// for `RunView`. Nothing stops a future edit — someone adding a field to
// display, reaching for the type that already has the getter they want —
// from importing `RunRecord` (the SUPERVISOR's type) into the status path
// instead. `RunRecord` has `save` by design, because the supervisor needs
// it. A status command built against `RunRecord` reproduces the exact
// original bug: load, observe, save-what-you-loaded.
//
// This is not caught by the compiler. It is caught only by code review
// noticing "why does `cmd_status` import the writable type" — a much
// smaller, more visible thing to notice than the original bug (a `save`
// buried inside a function named `load`-and-`observe`), but not a
// impossibility.

use record::{simulate_observe, RunRecord};
use std::path::Path;

fn cmd_status(path: &Path) {
    let mut record = RunRecord::load(path).unwrap(); // should have been RunView::load
    let obs = simulate_observe(record.observed(), 0, "now");
    record.set_observed(obs);
    record.save(path).unwrap(); // <-- compiles. erases anything appended since the load.
}

fn main() {
    cmd_status(Path::new("../../fixtures/run.json"));
}
