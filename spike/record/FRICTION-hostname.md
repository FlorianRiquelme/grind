# FRICTION — adding `hostname`

## What I found before touching anything

Read `FINDINGS.md`'s "Strict serde + schema evolution" section and the sources
before writing any code. `hostname` was already present:

- `src/view.rs`: `RunView.hostname: Option<String>` with `#[serde(default)]`,
  a doc comment explicitly citing this task's own premise ("Added after this
  fixture was written (docs/CONTEXT.md: 'it names the host holding it').
  Absent in the real fixture — proves the default.").
- `src/supervisor.rs`: `RunRecord.hostname: Option<String>` with
  `#[serde(default)]`, same shape.
- `src/main.rs` §1 (read path): already printed `hostname={:?}` when loading
  the real fixture.
- `fixtures/run.json`: already lacks the key, and `RunView::load` on it
  already returned `hostname: None` rather than erroring.

So requirements 1–3 of the task were already done — this crate's own
findings write-up (`FINDINGS.md`, "Strict serde + schema evolution") had used
`hostname` as its worked example for `Option<T>` + `#[serde(default)]`
schema evolution, ahead of this task actually asking for the field.

## What was actually missing

Requirement 4: "`fn main()` must print the hostname in both the read path
and the supervisor path." The read path (§1) printed it; the supervisor
path (§2, the `RunRecord::load` → `push_attempt` → `save` sequence) did not.

## The loop

1. `cargo build -p record` (baseline, before any edit): clean. No errors.
2. One edit: added a single line to `src/main.rs` §2 —
   `println!("  supervisor: hostname={:?}", record.hostname);` — right after
   the supervisor's `RunRecord::load`. `hostname` is a public field on
   `RunRecord`, so no getter was needed; this is a direct field read, same
   pattern as reading `record.attempts()`.
3. `cargo build -p record`: clean. No errors, no warnings.
4. `cargo run -p record`: succeeded, printed `supervisor: hostname=None`
   alongside the existing `hostname=None` in the read-path line, confirming
   both paths now render the field.

## Iteration count

**Zero compile-error iterations.** Both `cargo build` invocations were clean
on the first attempt — the pre-existing one (before my edit, to establish a
baseline) and the one after adding the print line. No `E0xxx` codes were
ever produced.

## Borrow-checker / ownership detours

None. The added line is a read of a `Copy`-free but `Debug`-derived
`Option<String>` field via `{:?}` — no move, no borrow conflict with the
subsequent `record.push_attempt(...)` / `record.set_observed(...)` /
`record.save(...)` calls, since the print happens before any of those and
only takes an immutable reference for the format call.

## Wall-clock

Under two minutes from opening `main.rs` to first (and only) green
`cargo run`. Almost all of the time went into reading `FINDINGS.md` and the
four source files to discover the field already existed, not into writing
or debugging code.

## Things I considered and backed away from

- **Making `hostname` private with a constructor-only setter**, mirroring
  the `attempts`/`push_attempt` treatment ("captured once, never changes
  afterward" as an enforced invariant, not just a comment). Backed away:
  this spike's `RunRecord` has no "create a new record at dispatch" path at
  all — `RunRecord::load` only ever loads existing JSON — so there is no
  real call site to attach a `set_hostname`-once guard to, and every other
  day-one field (`run_id`, `created_at`, `job`, ...) is equally `pub` and
  equally "supposed to never change after dispatch" without any enforcement
  in this codebase. Singling out `hostname` for privacy without a
  surrounding dispatch API would be inventing structure the rest of the type
  doesn't have, for a field that isn't special among its peers. In
  TypeScript I'd have reached for `Readonly<T>` on the whole dispatch-time
  slice of the record instead of hand-rolling one setter per field — Rust's
  per-field privacy makes that shape (one field singled out) mechanical but
  also makes "make the whole object immutable after construction" not a
  one-line thing the way `Readonly<>` is; I didn't need it here since
  nothing in this spike currently constructs a `RunRecord` from scratch.
- **Adding a `RunRecord::dispatch(...)` constructor** that would take a
  hostname (e.g. via `hostname::get()` or `std::env::var("HOSTNAME")`) and
  build a fresh record, to make "captured once at dispatch" something the
  code actually does rather than describes. Backed away: out of scope — the
  task asked for the field, its backward-compatible loading, and its
  display, not a new dispatch flow, and this crate doesn't model dispatch
  anywhere else either (`Job`, `run_id`, etc. all arrive pre-populated from
  a fixture, never constructed in Rust).
</content>
