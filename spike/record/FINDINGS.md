# record — findings

Answers wayfinder #33 for one crate: whether Rust's type system can make
`bin/grind`'s known read-path-writes-back bug unrepresentable, and what that
costs an ordinary reader. Throwaway spike, not a translation source.

## The bug, restated

`bin/grind`'s `cmd_status`:

```python
state = load(run_id)   # read run.json
observe(state)          # mutates state["observed"] in place
save(state)             # writes the WHOLE dict back
```

`grind status` is a read command. If the supervisor appended `attempts[6]`
between the `load` and the `save` — plausible, since a human runs
`watch -n 30 grind status` precisely while a Run is still in flight — the
`save` overwrites `run.json` with the stale copy from before attempt 6,
erasing it. `attempts[]` is one of only two fields nothing can rebuild
(#8/#12/#27). The bug is not "status shows stale data" (fine, expected under
concurrent writers) — it's "status *destroys current* data".

## The design: read-only view vs. writable record, as two distinct types

Three designs were on the table:

1. **A read-only view type with no write method, vs. an owned writable
   record the supervisor alone constructs.** — chosen.
2. **A write capability/token** that only the dispatch path can obtain;
   `save` takes `&self, &WriteToken`, and `WriteToken` has no public
   constructor.
3. **Append-only `attempts`** such that a whole-document overwrite of that
   field specifically is inexpressible, independent of (1)/(2).

Picked **(1) + (3) together**: `RunView` (`src/view.rs`) derives
`Deserialize` only and has no method that writes; `RunRecord`
(`src/supervisor.rs`) derives `Deserialize` + `Serialize`, has `save`, and
makes `attempts` a private field mutable only through `push_attempt` (no
`set_attempts`, no `&mut Vec<_>` getter) plus `set_observed` for the sibling
field `observe()` also touches.

**Why not (2) alone, or on top of (1):** a token only gates the *method*,
not the *type*. It adds one more moving part (who is allowed to construct a
`WriteToken`?) without removing the type-level distinction (1) already
gives you for free — `RunView` still needs to not have `save`, or the token
gates nothing. Prototyped mentally, not in code: it would look like
`fn save(&self, _: &WriteToken)` on `RunRecord`, with `WriteToken`'s
constructor `pub(crate)`-restricted to the supervisor's own entry point. That
is real hardening for *within* `RunRecord`, but this spike gets the same
effect more cheaply by never giving `RunView` a `save` to gate in the first
place. Worth doing in the real rewrite if `RunRecord::load` itself needs to
be restricted (see the first hole below) — see "Holes I found," escape 1.

**Why (3) is layered on, not a replacement for (1):** (1) stops the *read*
path from writing at all. It does nothing about the *write* path replacing
its own history — `record.attempts = vec![]` would compile fine if
`attempts` were a public field, and that's a second way to lose `attempts[]`
(a bug the Python doesn't currently have, but the type design should not
introduce). Making the field private with only `push_attempt` closes that
without needing a second type.

**The tension the brief called out — `observe()` has two legitimate
callers** — status wants to *display* a fresh observation, the supervisor
wants to *persist* one: resolved by making `observe()` (`simulate_observe`
in `src/lib.rs`, standing in for the real git/gh-shelling version) a pure
function returning an owned `Observation`, independent of either record
type. Both callers get the same value from the same computation.
`RunView`'s copy is a local, ends when `main` drops it. `RunRecord`'s copy
goes through `set_observed` and is only durable once `save` runs. Neither
caller needed a special case; the fork is entirely in what each holder's
*type* lets it do with the value afterward.

## The proof: `wont-compile/`

Two transcriptions of the bug, compiled by hand against the built `record`
rlib (see `wont-compile/README.md` for the exact command):

**`01_status_calls_save.rs`** — `RunView::load`, then `.save()`:

```
error[E0599]: no method named `save` found for struct `RunView` in the current scope
  --> 01_status_calls_save.rs:14:10
   |
14 |     view.save(Path::new("../fixtures/run.json")).unwrap();
   |          ^^^^ method not found in `RunView`
```

**`02_status_serializes_view.rs`** — sidesteps the missing method, tries
`serde_json::to_string(&view)` directly:

```
error[E0277]: the trait bound `RunView: serde::Serialize` is not satisfied
    --> 02_status_serializes_view.rs:10:38
     |
  10 |     let body = serde_json::to_string(&view).unwrap();
     |                --------------------- ^^^^^ the trait `serde_core::ser::Serialize` is not implemented for `RunView`
```

Both are real, standalone `.rs` files, excluded from the workspace build
(no `mod` reference, no `[[bin]]`), compiled directly with `rustc` against
`librecord.rlib` to get the compiler's own verdict rather than a hand-written
approximation of one.

## Holes I found in my own design

Went looking for how a future agent — trying to ship a feature, not
preserve an invariant — would get past this. Two escapes, both in
`wont-compile/escapes/`, both **compile**:

**Escape 1 — pick the other type.** `RunRecord::load` is `pub`, because the
supervisor needs to call it. Nothing stops a status call site from importing
`RunRecord` instead of `RunView` — maybe because it wants a field `RunView`
doesn't expose, maybe by copy-paste from the supervisor code. Once it holds
a `RunRecord`, `.save()` exists, and the exact original bug is back:

```rust
fn cmd_status(path: &Path) {
    let mut record = RunRecord::load(path).unwrap(); // should have been RunView::load
    let obs = simulate_observe(record.observed(), 0, "now");
    record.set_observed(obs);
    record.save(path).unwrap(); // compiles. erases anything appended since the load.
}
```

This is the design's real limit: it makes the *accidental* version of the
bug (a `save` hiding inside something that reads-and-observes) impossible,
but it makes the *deliberate* version (choosing the writable type for a
read-only job) merely visible in a diff — `use record::RunRecord;` in a file
named `status.rs` is a five-second code-review catch, not a compiler error.
Closing this the rest of the way needs a capability/token (design 2, above)
restricted at `RunRecord::load`'s call site — e.g. `pub(crate)` visibility on
`RunRecord` itself with only the supervisor module allowed to see the type
at all, so a status module in a different file has nothing to `use` even by
mistake. Not implemented here because in this single-crate spike there is
only one `main.rs`, so module-privacy has nothing to bite on; in the real
multi-file rewrite this is worth doing for real.

**Escape 2 — skip the types entirely.** Even a disciplined status path that
only ever touches `RunView` cannot be stopped from writing the file some
other way, because Rust's type system has no notion of "this code may not
touch this path." Every command already knows `RUNS_DIR / run_id /
"run.json"` to find the record at all:

```rust
std::fs::write(path, b"{\"anything\": \"goes\"}").unwrap();
```

compiles and executes with zero dependency on this crate. A type-only
design can make the *ergonomic, would-write-this-by-accident* version of the
bug impossible; it cannot make writing to a path something a capability
system gates, because plain `&Path` is not a capability. Reported as the
honest ceiling of this approach rather than something a smarter type could
still close inside a single Rust crate — closing it needs either an
OS-level permission boundary (a supervisor-only-writable file, which
contradicts "the CLI runs as the same user for every command") or a
different architecture entirely (grind as a long-lived daemon that owns the
only file handle, `status` and `resume` talking to it over IPC instead of
opening `run.json` themselves) — worth flagging for the real rewrite, out of
scope for a spike whose job is to check whether the *type-level* half of
this problem is real, which it is.

## Strict serde + schema evolution

`run.json` is Grind's own format (unlike Claude Code transcripts, which are
someone else's format grind must be lenient about) — so strict, all-or-
nothing derive `Deserialize` is the right default: a record missing a field
genuinely required from day one (`run_id`, `state`, `job`, ...) should fail
loudly, because grind has no sensible behavior for a record it can't fully
account for.

The real risk this creates: a record written by an OLDER grind, read by a
NEWER one, after a field was added. Checked against the actual fixture
(`fixtures/run.json`, copied from a completed run in `.grind/runs/`):

```
claude_bin present: False
hostname present:   False
```

Both are true right now, not hypothetically — `claude_bin` predates this
fixture's dispatch and simply wasn't recorded that run; `hostname` is a
field `CONTEXT.md` (commit `4c10df5`, "the record has one writer, and it
names its host") describes but `bin/grind` doesn't populate yet. A strict
derive with no accommodation would hard-fail loading this exact,
real, on-disk record.

The cheapest honest handling, applied in `src/types.rs` / `src/view.rs` /
`src/supervisor.rs`: fields that were added after day one are `Option<T>`
with `#[serde(default)]` — they parse to `None` on an old record instead of
erroring, and the type still tells every caller they might be absent (no
`.unwrap()` silently assumes a value that was never guaranteed). Fields that
were required from day one stay bare, non-`Option`, non-defaulted — missing
one of those is not schema evolution, it's a corrupt or foreign record, and
should still be a hard `serde_json::Error`. This is a deliberate, per-field
call made at the moment a field is added — `#[serde(default)]` is not a
blanket policy, and a field that starts life optional but later becomes
load-bearing needs its own migration, not silently relying on the default
forever.

`main.rs` §1 loads the real fixture and prints `hostname=None
claude_bin=None` to confirm this is not hypothetical.

## Atomic write

The Python: `(d / "run.json").write_text(json.dumps(state, indent=2) + "\n")`
— one `write_text` call, which is not one atomic write; it's open, write N
bytes, close. A crash (OOM kill, `SIGKILL` from a supervising process,
power loss) between the first and last byte leaves a **truncated** file:
neither the old state nor the new one, just broken JSON — a destroyed
record, since `attempts[]` up to that point is now unparseable along with
everything after it.

`RunRecord::save` (`src/supervisor.rs`) writes to `run.json.tmp` in the same
directory, `sync_all`s it, then `std::fs::rename`s it over `run.json`.
`rename` on the same filesystem is a single directory-entry pointer swap —
POSIX guarantees no intermediate state is observable through the target
path; the reader sees either the fully-old or fully-new file, never a
partial one.

Proved, not just asserted, in `main.rs` §3: writes a deliberately truncated
body to `run.json.tmp` and stops — *simulating* the crash by simply not
calling `rename`, which is the one step a real crash would also skip —
then confirms the real `run.json` is byte-for-byte unchanged and still
parses as a valid `RunView`. Output:

```
real run.json unchanged by the crashed write: true
real run.json still parses as a valid record: true
```

This does not cover a crash *during* the `rename` itself, which POSIX still
treats as atomic (the metadata operation either completes or doesn't — it
is not a byte-stream write), nor does it cover the filesystem not honoring
`rename`'s atomicity guarantee at all (true of some network filesystems;
`.grind/` is local disk, so out of scope here).

## Ceremony cost for an ordinary reader

The tax this design imposes lands entirely on the *write* side, not the
read side — which is the point, since almost every call site (`status`,
`list`, a future dashboard) is a reader:

```rust
let view = RunView::load(path)?;   // one line, same as a plain deserialize
println!("{} attempts", view.attempts.len());
```

No token to acquire, no builder, no `Result`-wrapped capability check beyond
the ordinary I/O/parse `Result` any loader has. `src/main.rs` §1 is exactly
this: load, read fields, print — indistinguishable in line count from
`serde_json::from_str::<serde_json::Value>(&raw)?` with no types at all.

The write side pays for the safety property: the supervisor cannot do
`state["observed"] = obs; save(state)` in one expression the way the Python
does. It's `record.set_observed(obs)` (a field it's allowed to replace
wholesale — there's only ever one current observation) and
`record.push_attempt(attempt)` (a field it can only append to) as two named
calls instead of one dict-literal assignment, plus an explicit `save(path)`
at the end instead of it being implicit. Measured against `main.rs` §2: five
lines for the supervisor's whole load-mutate-persist sequence, versus three
in the Python (`state["attempts"].append(...); observe(state); save(state)`)
— a two-line tax, paid exactly once, by the one code path that is supposed
to be doing something deliberate.
