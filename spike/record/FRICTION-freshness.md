# Friction log — observation freshness

Task: `grind status` shows observations with no indication of when they were
taken. Add a "last observed at" timestamp to the record, update it on
observe, display an age string on the read path, and demo it in `main()`.

## What changed

- `src/types.rs`: `Observation` gains `observed_at_epoch: u64`
  (`#[serde(default)]`), sitting next to the existing `observed_at: String`.
  Kept both rather than replacing the string: `observed_at` is an
  ISO-ish string the existing demo/fixtures already pass around verbatim
  (`main.rs` calls `simulate_observe(.., "2026-08-04T12:00:00+00:00")`), and
  changing its meaning or type would have meant touching every call site
  and the fixture. Adding a second, purely-numeric field alongside it was
  the smaller diff and matches the task's explicit steer ("plain unix-epoch
  integer... no date/time crate").
- `src/lib.rs`: `simulate_observe` now sets `observed_at_epoch` from a new
  `unix_now()` helper (`SystemTime::now().duration_since(UNIX_EPOCH)`),
  independent of the `at: &str` argument callers pass — so every call to
  the shared observe function gets a real wall-clock timestamp, not the
  fixed demo strings. Also added `pub fn freshness(observed_at_epoch: u64)
  -> String`, a free function (`"observed {age}s ago"`) rather than a
  method, since both `Observation` (in `types.rs`) and `RunView` (in
  `view.rs`) needed to reach it and neither module owns the other.
- `src/view.rs`: `RunView::observation_freshness(&self) -> Option<String>`
  — the read path's own display method, delegating to
  `crate::freshness`. `None` when there's no `observed` block yet.
- `src/main.rs`: extended the existing walkthrough with a new
  `=== 4. observation freshness ===` section, and added one line to
  section 1 printing `view.observation_freshness()` against the real
  fixture (which predates the new field, so it defaults to 0 and prints a
  deliberately enormous age — an honest answer, not a crash).

## Every build/run invocation, in order

1. `cargo build -p record` — clean, no errors, no warnings. First attempt.
2. `cargo run -p record` — ran clean, all four sections printed, both
   assertions in `main.rs` passed.
3. Added the `view.observation_freshness()` line to section 1 of
   `main.rs`.
4. `cargo build -p record` — clean again, no errors, no warnings.
5. `cargo run -p record` — clean, printed `observed 1785855914s ago` for
   the legacy fixture (expected: `observed_at_epoch` defaults to 0 there,
   so age = current unix time).

**Compile-error iterations before first green: 0.** Both builds compiled on
the first try. The only "iteration" was adding one more print line after
the first successful run, done for coverage of the `RunView` method
specifically (see below), not to fix anything broken.

Total wall clock: about 10 minutes, most of it reading `lib.rs` / `types.rs`
/ `view.rs` / `supervisor.rs` / `main.rs` to understand the existing
read/write split before touching anything, plus ~2s of that spent inside
the program's own deliberate `thread::sleep(2s)` demo pause.

## Where the existing design almost pushed back, and what I decided instead

Nothing resisted at the type level — no borrow-checker fight, no trait
needed, no visibility change. The one real design fork was interpretive,
not mechanical, and worth recording because the choice isn't obviously
forced by the code:

**What does "observe again and show it advanced" mean, given that `observe`
always stamps the *current* wall clock?** If every call to
`simulate_observe` sets `observed_at_epoch = now`, then two successive
observes are each "0s ago" the instant they're taken — the *age* never
"advances" across two observes back-to-back; only the *timestamp value*
advances (monotonically). I considered three readings:

1. Fake the clock — pass an override `now` into a `freshness_at(epoch, now)`
   variant so the demo could show canned "45s ago" output deterministically,
   without an actual sleep. Rejected: it would hide the actual mechanism
   (real `SystemTime::now()`) behind a parameter that production code never
   needs, purely to make a demo prettier. Would also have meant a second
   entry point for the same one-line calculation.
2. Only show freshness immediately after each observe (always "0s ago"
   twice) and call that compliant with the letter of the requirement.
   Rejected: it wouldn't actually demonstrate "a human running status twice,
   a minute apart, sees the freshness advance" — the whole point of the
   feature — since nothing in the printed output would ever show a number
   other than 0.
3. (What I did.) Real `thread::sleep(2s)`, and show the *same* observation's
   freshness both immediately and after the sleep (0s -> 2s, genuinely
   advancing), then observe again to show the timestamp moving forward with
   an explicit `assert!(obs2.observed_at_epoch >= obs1.observed_at_epoch)`.
   This uses the real `SystemTime` path end to end (no override, no fake
   clock) and is the most honest reproduction of the staleness scenario the
   task describes, at the cost of the demo taking 2 real seconds longer to
   run.

I went with option 3. It's the only one that exercises the exact code path
production would exercise (no test-only clock hook) while still visibly
demonstrating both halves of the requirement: the age growing while a
reading sits unrefreshed, and a fresh observe resetting it.

## Considered and rejected

- **Putting `freshness` on `Observation` as `impl Observation { fn
  freshness(&self) -> String }`** instead of a free function taking
  `observed_at_epoch: u64`. Rejected because `RunView::observation_freshness`
  needed the exact same one-line computation and taking `&self.observed_at_epoch`
  directly (not needing the rest of `Observation`) kept the free function
  usable from both `types.rs` and `view.rs` without either module importing
  the other's impl block. Also makes the "what is 'now'" question explicit
  at the call site rather than buried in a method.
- **Replacing `observed_at: String` with the epoch integer** and dropping
  the string entirely. Rejected — out of scope (the string is read
  elsewhere, e.g. printed in the fixture, and the task asked to "gain" a
  timestamp, not replace one), and would have been a larger, less
  reversible diff for no requirement gain.
- **Formatting age as `Xm Ys` for large values** instead of a flat seconds
  count. Rejected as unnecessary polish beyond what four requirements ask
  for; `"observed {age}s ago"` is legible even at `1785855914s` (ugly, but
  correctly conveys "this is old data" — arguably the point).

## What I'd have done in Python or TypeScript instead

Same shape, but with far less type-level scaffolding to write around: add
`observed_at_epoch = int(time.time())` (Python) or
`Date.now()` (TS, as ms) into whatever dict/object `observe()` returns, then
a `age = now() - observed_at_epoch` one-liner wherever status prints. No
`#[serde(default)]` ceremony for backward compatibility — `dict.get(
"observed_at_epoch", 0)` or `obj.observedAtEpoch ?? 0` is the same idea with
less syntax. The interesting part of this task in Rust wasn't the timestamp
itself (identical effort in any language) — it was confirming that adding a
field to a `#[serde(default)]`-guarded struct doesn't force a fixture
migration or a serialize-side default, which cost zero extra code here
because the pattern was already established by `hostname`/`claude_bin` in
the same file. In Python that safety would have been implicit and unchecked
(a `KeyError` at runtime on an old record, not a compile-time non-issue).
