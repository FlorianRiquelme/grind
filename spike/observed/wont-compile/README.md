# wont-compile

These files are not part of the `observed` crate's build (no `mod` statement references
them, and they live outside `src/`). `cargo build -p observed` never touches this
directory. They exist to be compiled individually, once, to capture real compiler output —
see `../FINDINGS.md` for the verbatim text.

Each file redefines a minimal `Observed<T>` locally so it can be run standalone with
`rustc`, independent of the crate:

```
rustc --edition 2021 forgot_unobservable.rs -o /tmp/a
rustc --edition 2021 silent_collapse_to_false.rs -o /tmp/b
```

## forgot_unobservable.rs

The collapse Python's `sh(..., check=False)` allows by construction: a caller who writes a
branch for "yes" and a branch for "no" and never wrote one for "I couldn't ask". **Does not
compile** — `match` on an enum in Rust must be exhaustive, so the missing
`Observed::Unobservable(_)` arm is a hard error, not a review comment.

## silent_collapse_to_false.rs

The same collapse, performed on purpose instead of by omission: an `if let Present(x) {
x } else { false }` that routes both `Absent` and `Unobservable` into the same `false`.
**Compiles and runs.** This is the honest limit of what `Observed<T>` buys: the type forces
you to *name* the Unobservable case somewhere, but nothing stops you from naming it and
then discarding the distinction anyway. The type system prevents the *silent* version of
the mistake (the one where you never noticed there was a third case); it does not prevent
the *deliberate* version.
