---
status: accepted
date: 2026-08-04
---

# The base is a compiled Rust binary, and *not an agent* is the only part of the script rationale that survives

`CLAUDE.md` and `STRATEGY.md` justified `bin/grind`'s shape with one sentence: *a resilience
layer built from the thing that gets rate-limited loses its state exactly when that matters.*
Four properties were riding on it — **not an agent**, **stdlib only / no dependencies**, **no
build step**, and **one file**. Only the first is actually argued for by that rationale. By its
own logic a compiled binary is *more* resilient than a script, not less.

So the base is Rust, compiled, with dependencies and a build step. **Not an agent stays a hard
constraint.** The other three are withdrawn.

The language was chosen by the driver on 2026-08-03. What made it *safe* was measured
afterwards, in the spike on `prototype/33-rust-awkward-core`
([#33](https://github.com/FlorianRiquelme/grind/issues/33)) — five throwaway crates, one per
awkward bit, against the real `claude` binary and Run 1's real record.

## Considered options

| Option | Verdict | Trade-off |
|---|---|---|
| **Compiled Rust binary, `serde` the only dependency** | **Chosen** | The failure modes whose silent failure is expensive become unrepresentable-by-omission, and four of the five awkward bits need no dependency at all. **Cost:** a build step, a cross-compile story ([#30](https://github.com/FlorianRiquelme/grind/issues/30)), and a tolerant hand-written parse where the script got leniency free. |
| Keep the Python script | Rejected | No build step, and every accident it already survived stays survived. **Cost:** `check=False` makes a failed observation identical to a negative one, and two of the four completion signals fail toward `completed` — in the window right after the laptop wake that killed Run 1 four times. No test can make that class of bug unwritable. |
| TypeScript / any JavaScript runtime | Ruled out of scope with the language choice | Agents write it with less friction. **Cost:** a required field on an `interface` catches call sites at `tsc` time much as Rust does, but optionality is the default, and the guarantee is a discipline call rather than a language one. |
| Grind as an agent | Rejected, permanently | — This is the half of the original rationale that holds, and ADR-0001. |

## What the spike established

- **Re-entry works with zero dependencies.** `std::process` spawns the real `claude -p`, redirects
  stdout and stderr to separate files, SIGKILLs it mid-response and re-enters `--resume` on the
  same session id with history intact. This is the mechanic Run 1 performed five times.
- **`flock` is in std** (`File::try_lock`, stable 1.89), so [#25](https://github.com/FlorianRiquelme/grind/issues/25)
  needs no dependency. The property that makes it the right mechanism rather than a run-state
  check — the kernel releases it when the holder is SIGKILLed — is demonstrated, not assumed.
  Key it on `git rev-parse --git-common-dir`, never a worktree path, or two worktrees of one repo
  on one branch pass each other silently.
- **`serde` is the only dependency**, and only because JSON is. Regex is not needed: normalising
  the haystack (strip non-alphanumerics, then match) detects rate limits *more* broadly than the
  current `rate.?limit` regex, which cannot match `rate  limit`.
- **Strict `serde` for formats Grind owns; tolerant `Value` lookups for formats it does not.**
  `run.json` may be strict, with post-day-one fields `Option` + `#[serde(default)]`. Claude Code's
  transcripts may not: a derive struct must stay in sync with an undocumented format forever, and
  `Option` + `default` still loses **every sibling field on a line** when one field's *type* is
  unexpected. Rust's static-shape advantage does not apply to a format with no stable shape, and
  the honest cost there is the same per-field vigilance Python already paid.

## Consequences

- **Types catch omission, not intent.** A `match` that forgets `Unobservable` is `E0004`; a
  deliberate `if let Present(true) = o {…} else { false }` compiles clean with no clippy lint. A
  `save` hidden in a read-and-observe path is `E0599`; a status module that simply does
  `use RunRecord` gets `save()` back. Both were found independently, by separate crates. The
  claim to make downstream is *the compiler catches omission* — never *the compiler catches
  mistakes*.
- **A type-level invariant needs a second carrier.** Given an ordinary feature ticket whose naive
  implementation is the read-path-writes-back bug, an agent did not break the invariant — it
  silently narrowed the feature to fit, shipped something that works in a demo and does nothing
  as a CLI, and logged *"nothing resisted at the type level"*. Since nobody reads the diff
  (#6), that ships. Every invariant carried by a type needs a test that fails or a name that
  makes the impossibility loud.
- **Put data in shapes the compiler checks.** Adding a struct field forced all four construction
  sites (`E0063`) with no grepping. It forced neither the parallel array literal that folds the
  signals nor the `println!` that displays them. Parallel arrays beside a struct are where the
  compiler goes quiet.
- **Module privacy is worth using, and it works.** Restricting the writable record type to the
  supervisor's own module makes the read path's escape route `error[E0603]: struct RunRecord is
  private` — verified. Visibility is the difference between a compile error and a code-review
  catch.
- **Agent friction did not materialise at this scale.** Three Sonnet agents given feature tickets
  against the spike reached green in 0–1 compile-error cycles, with no borrow-checker detours.
  The map admitted this tax up front; it is not visible on small additive changes, which is not
  evidence about refactors across module boundaries.
- **`bin/grind` stops being the shape to preserve.** It remains reference and evidence. It is
  never a translation source.
