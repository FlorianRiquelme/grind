# Friction log: adding a fifth completion signal

Task: add `verify_reported: Observed<bool>` — "did the target repo's `just verify`
report a readable outcome" — as a fifth ANDed signal alongside `pr_open`, `tree_clean`,
`commits_ahead`, `ci_clear`.

## What I changed, in order

1. Added `verify_reported: Observed<bool>` to `struct Signals`.
2. Added `("verify_reported", &signals.verify_reported)` to the `named` array in
   `verdict()` and bumped its type annotation from `[..; 4]` to `[..; 5]`.
3. Added a `verify_reported` line to `print_scenario`'s output.
4. Ran `cargo build -p observed`.
5. Fixed the 4 errors it produced (below).
6. Added a 5th scenario to `main()` — `verify_reported` unobservable — so the new signal
   actually shows up doing something, not just as a constant `Present(true)` everywhere.
7. Updated the test: `states()` loop gained a fifth nested `for verify_reported in
   states("verify_reported")`, the `any_unobservable` array gained the fifth reference,
   the `Signals { .. }` literal inside the loop gained `verify_reported: ...`, and the
   final `assert_eq!(checked, 3*3*3*3)` became `3*3*3*3*3`.
8. Ran `cargo build`, `cargo run`, `cargo test` — all clean.

## Every `cargo` invocation, in order

| # | Command | Outcome |
|---|---------|---------|
| 1 | `cargo build -p observed` | **E0063 x4** — see below |
| 2 | `cargo build -p observed` | clean (3 pre-existing dead-code warnings, unrelated) |
| 3 | `cargo run -p observed` | clean, correct output |
| 4 | `cargo test -p observed` | clean, 1 passed |

**One compile-error iteration before first green.** (Four errors, but they were all the
same error class, reported in one `cargo build` invocation, fixed in one edit pass.)

## The E0063 errors, verbatim (truncated to one, the other three are identical shape)

```
error[E0063]: missing field `verify_reported` in initializer of `Signals`
  --> observed/src/main.rs:95:9
   |
95 |         Signals {
   |         ^^^^^^^ missing `verify_reported`
```

One of these fired for each of the four `Signals { ... }` literals in `fn main()`
(the four pre-existing scenarios). What was wrong: I'd added the field to the struct
and to `verdict()`'s `named` array, but hadn't touched any of the call sites that
construct a `Signals` value. What I changed: added `verify_reported: Observed::Present(true)`
(or `Unobservable(...)` for the new fifth scenario) to each literal.

## Did the compiler find the call sites, or did I have to find them myself?

**The compiler found every one of them, unprompted, in a single build.** I added the
field to `struct Signals` and to `verdict()`'s exhaustive-by-construction `named` array
myself — those aren't "found," they're the definition of the feature. But every place
that *constructs* a `Signals` value and would otherwise have silently defaulted
`verify_reported` to some unspecified state is a struct literal, and Rust has no field
defaulting: a struct literal missing a field is `E0063`, full stop, regardless of
whether that struct participates in a `match`. I did not manually grep for `Signals {`
sites — `cargo build` enumerated all four in one pass, with line numbers, before I fixed
anything.

The `verdict()` function's `named` array is a different mechanism (not exhaustiveness —
it's just an array literal I sized `4` then `5`), so the compiler did *not* force me to
add `verify_reported` there; I could have left the array at length 4 and it would have
compiled fine, silently excluding the new signal from the fold. That's the one place in
this task where correctness rode on me, not the type system — the fix was the addition
of one array entry, not something `rustc` complained about.

The `print_scenario` output line was pure cosmetics (a `println!`) — nothing enforces it
either; the task asked for it explicitly (requirement 3) and I added it by hand, unforced.

So: **partial compiler force.** The struct-literal sites (call sites) were fully and
automatically enumerated by the compiler — that's the E0063 story, and it's the strong
half of the claim in `FINDINGS.md`. The fold-in-`verdict()` and the print line are not
struct-literal completeness questions; they're "did I remember to wire up the new
variable," and Rust's exhaustiveness checking has nothing to say about an array literal
or a `println!` — those needed a human (me) reading the diff, same as Python would.

## Did the existing test catch anything I got wrong?

Yes, functionally by design rather than by surprise: after widening the test's nested
loop to 5 dimensions and the `Signals` literal inside it, `cargo test` passed
immediately — no bug surfaced. I did *not* make a classification mistake it would have
caught (I only added `Present`/`Unobservable` symmetrically, same as the other four
signals, so there was no room for a collapse-style bug in this change). Its value here
was structural, not diagnostic: it forced the same "you added a field, now touch this
literal" discovery as `E0063` did — I got `assert_eq!` failing until I updated
`checked, 3*3*3*3` to `3*3*3*3*3`... actually no, I updated both in the same edit, so it
never actually failed. Honest report: the test did not catch a mistake this round, it
just needed a mechanical extension parallel to the ones the compiler already forced.

## Total wall-clock to first green

Under 2 minutes of actual command time (three `cargo` invocations, none of which
rebuilt from scratch — incremental compiles). The bulk of the task's time was writing
the five edits, not iterating on compiler feedback — there was exactly one
build-fail-fix cycle.

## TypeScript or Python: faster, or riskier?

Faster to type, certainly — a Python `dataclass` or TS interface with a fifth optional
field wouldn't force me to touch four call sites; I could add the field, wire it into
one aggregation function, and ship without ever visiting the scenario constructors,
which is strictly less typing.

Riskier, specifically in the way this spike is about: nothing would have told me if I
forgot to add `verify_reported` to one of the four existing `Signals`-equivalent
dict/object literals in `main()`. In Python, a missing key on a dict either raises at
first *access* (if I wrote `.get("verify_reported")` with no default, an easy way to
avoid it) or, more likely given this codebase's habits, silently defaults to `None`/falsy
and gets folded into "absent" — which is exactly the collapse `FINDINGS.md` is about,
just one level up: not "gh call failed" collapsing into "false," but "forgot to update
this call site" collapsing into "signal reads as not-present." TypeScript with a
required (non-optional) field on an `interface` would catch the same four sites at
`tsc` time, similarly to Rust — but only if the field is written as required and the
four literals are type-checked as that interface, which is a discipline call, not a
language guarantee the way E0063 is for a `struct` literal in Rust. Rust's win here
wasn't ergonomics, it was that omission at any of the four call sites was not a choice
I could make by accident.
