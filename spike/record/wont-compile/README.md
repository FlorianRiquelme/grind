# wont-compile/

Excluded from the workspace build on purpose (no `mod` reference, no
`[[bin]]` entry — cargo never sees these). Each file is compiled by hand
against the built `record` rlib to get a real, verbatim compiler error.
Verbatim output for each is in `FINDINGS.md`.

Reproduce:

```
cargo build -p record
cd wont-compile
RLIB=$(ls ../../target/debug/deps/librecord-*.rlib | head -1)
rustc --edition 2021 --crate-type bin -L "$(dirname "$RLIB")" --extern record="$RLIB" 01_status_calls_save.rs -o /tmp/a1
```

## Files that do NOT compile (the bug is unrepresentable)

- **`01_status_calls_save.rs`** — transcribes `cmd_status`'s `load() ->
  observe() -> save()` literally: load a `RunView`, then call `.save()` on
  it. `RunView` has no `save` method. `error[E0599]: no method named `save`
  found for struct `RunView``.
- **`02_status_serializes_view.rs`** — sidesteps the missing method, tries
  `serde_json::to_string(&view)` directly. `RunView` never derives
  `Serialize`. `error[E0277]: the trait bound `RunView: serde::Serialize` is
  not satisfied`.

## `escapes/` — files that DO compile (the holes in this design)

Reported loudly, not hidden, per the brief. See FINDINGS.md § "Holes I found
in my own design" for the full writeup.

- **`escapes/01_status_picks_the_writable_type.rs`** — nothing stops a status
  call site from importing `RunRecord` (the supervisor's type, which does
  have `save`) instead of `RunView`. Reproduces the original bug exactly.
  Compiles.
- **`escapes/02_status_writes_raw_bytes.rs`** — nothing stops any code that
  knows the `run.json` path from `std::fs::write`-ing over it directly, with
  no dependency on this crate's types at all. Compiles.
