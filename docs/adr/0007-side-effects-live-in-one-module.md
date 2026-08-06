---
status: accepted
date: 2026-08-06
---

# Side effects live in one module, and privacy only bites between siblings

ADR-0006 admitted **visibility** as a carrier in its own right and then handed
[#35](https://github.com/FlorianRiquelme/grind/issues/35) exactly one constraint — *the record's
writer and the record's readers may not share a module* — deciding nothing else about layout,
because module privacy is per-module and the carrier does not exist until the seams do.

This is the layout. Ten modules, one of them impure, and a privacy rule that is sharper than
ADR-0006 assumed.

Recorded resolving [#35](https://github.com/FlorianRiquelme/grind/issues/35), whose comment holds
the full derivation.

## The finding that shapes the rest: privacy only bites between siblings

ADR-0005 recorded that restricting the writable record type to the supervisor's own module makes
the read path's escape `error[E0603]: struct RunRecord is private` — verified. What was not
checked is **which** module arrangement produces that error. Verified now, on rustc 1.95.0:

| arrangement | result |
|---|---|
| `RunRecord` private in `supervisor`; `view` a **sibling** | `error[E0603]: struct RunRecord is private`, and rustc offers **no fix** in the diagnostic |
| `RunRecord` private in `record`; `view` a **child** of `record` | **compiles clean** — the child calls the private `load()` and the private `save()`, one unrelated dead-code warning |

Descendants see their ancestors' private items. So the idiomatic tidy-up — *"`supervisor.rs` and
`view.rs` are both about the record, nest them under `record/`"* — **silently withdraws the
carrier**, and it is an act of housekeeping rather than a decision. That is precisely ADR-0006's
**convention** mode, aimed at the one property ADR-0006 says only visibility can carry.

Two rules follow, and they are the load-bearing part of this ADR:

- **The writer and the readers are siblings at the crate root.** `RunRecord` is private to
  `supervisor` — not `pub(crate)`, not re-exported. Never a shared parent.
- **No module may be named for a noun two other modules share.** A `record/` parent, or a `types`
  module, is an attractor that pulls the writer and the reader back under one roof, and the
  compiler will not object when they arrive.

The second rule is why types live with their producer rather than in a common `types` module:
`Observed<T>` and `Observation` in `observe`, `Attempt` in `attempt`, `Job` in `job`, `Verdict` in
`decide`, `RunView` in `view`, `RunRecord` in `supervisor`.

## Considered options: one crate or several

| Option | Verdict | Trade-off |
|---|---|---|
| **One crate, modules only** | **Chosen** | One build, no manifest plumbing, and the friction tax across module boundaries — still unmeasured — is not raised further. **Cost:** the repair for `E0603` is `pub(crate)`, one keyword, and it compiles. Closed by a compile-fail test rather than by a wall. |
| A Cargo workspace of crates | Rejected | The same repair needs `pub` **and** a new dependency edge in a manifest — an import is not a decision, a `Cargo.toml` line is. **Cost:** crates invite `pub` at every seam just to be usable, withdrawing the carrier by another door, and they raise an unmeasured cost rather than probing it. |
| The record as its own crate, the rest in one | Rejected | Buys the wall exactly where it matters most. **Cost:** the asymmetry needs its own justification forever, and the compile-fail test is needed anyway for the properties a crate wall cannot carry. |

The compile-fail test already exists in prototype form: the spike's
`record/wont-compile/escapes/01_status_picks_the_writable_type.rs`, compiled by hand against the
built rlib, no dependency. Promoting it to a test that shells out to `rustc` and asserts
non-compilation is the second carrier ADR-0006 demands.

## The cut

| module | pure? | what it lets a caller stop knowing |
|---|---|---|
| `world` | no — **the only one** | how the world is reached. Sole namer of `std::process` and `std::fs`; holds no branching |
| `job` | yes | how a Job reference becomes a dispatch plan — field table, plugin pin, worktree choice, repo path, `claude` binary |
| `observe` | yes | how raw bytes become `Present` / `Absent` / `Unobservable(Reason)`. Owns `Observed<T>` and `Observation` |
| `decide` | yes | which signals corroborate what — `furthest_stage`, and #9's four ANDed observations |
| `policy` | yes | when to re-enter, when to sleep, when to stop |
| `attempt` | pure but for two calls | how one `claude` invocation happens — session id vs `--resume`, the denials that ride on every argv, `RawAttempt` written before anything parses it |
| `supervisor` | no | that a Run is a loop at all. **Sole writer.** Holds the private `RunRecord`, the dispatch lock, the loop |
| `view` | yes | reading a Run without being able to damage it. `RunView`, the roster |
| `render` | yes | how a Run reads to a human — returns `String`, never prints |
| `cli` | no | argument shapes; the only thing that writes to stdout |

`job` absorbs host resolution — repo path, worktree adoption, plugin directory, `claude` binary —
because all four are one act, turning a Job reference into everything a dispatch needs, and all
four are pure once [#31](https://github.com/FlorianRiquelme/grind/issues/31)'s rule is applied:
**the fix for _"nothing that touches git is tested"_ is pure parse functions over output text.**

## Two seams at the world, and only one of them is a seam

[#31](https://github.com/FlorianRiquelme/grind/issues/31) ruled that fakes substitute **raw stdout
+ stderr + exit code, never domain values**. That does not imply a `Runner` trait, and the spike
did not build one — it took `fake_child: &Path` and ran `Command::new(fake_child)` against seven
shell scripts, one per real death shape.

- **Short-lived (`gh`, `git`) — no seam.** `run(argv, cwd) -> Completed { stdout, stderr, code }`,
  concrete, with every caller's logic in a pure parse function over the text. A trait here would be
  a hypothetical seam: one production impl, one test impl, both in Rust, and the actual spawn path
  exercised by neither.
- **Long-lived (`claude`) — a real seam, and it is the binary path.** `resolve_claude_bin()` /
  `GRIND_CLAUDE_BIN` already is one, and it delivers fidelity a trait cannot: real SIGKILL, real
  empty-stdout-not-truncated, real separate stderr file, a real exit code the parent did not
  choose. Substituting a Rust impl fakes away the exact mechanics
  [#33](https://github.com/FlorianRiquelme/grind/issues/33) proved matter.

They are split because their interfaces have almost nothing in common. The long-lived one is deep —
it hides the session id, the `--resume` fork, redirect-raw-to-disk-before-parsing, the denials, and
death survival. The short-lived one hides nearly nothing. Behind one interface the deep one goes
shallow and the shallow one carries ceremony it has no use for; `RawAttempt`'s invariant is
meaningless for `git rev-parse`.

## Effects are returned as values, never performed in place

The same move appears three times, and it is the reason the base is testable without a network:

- `policy` **returns** `Next::SleepThenReenter(Duration)`; the loop is the only thing that blocks.
  A `thread::sleep` inside the policy makes *"a rate limit asks for 1800 seconds"* an assertion you
  have to wait 1800 seconds for. Returned, it is an equality check on a literal.
- `render` **returns** `String`; `cli` prints. This is what makes #12's *status degrades, never
  fails* — and its distinction between `—` (observed absent) and `?` (could not observe) — an
  assertion rather than an intention.
- `observe` classifies `Completed → Observed<T>` **away from the spawn**. ADR-0006 puts the second
  carrier exactly there: a test that *this call site*, given empty stdout and a non-zero exit and an
  auth message on stderr, yields `Unobservable` and not `Absent`. Beside the spawn that test needs a
  process; at this seam it needs three string literals.

`world` is therefore **deliberately shallow**, inverting the usual rule. It is not a pass-through
wrapping an abstraction — it is the irreducible I/O edge, and the only untested code in the base.
Shrinking it is the goal, so its shallowness is the design working.

`policy` takes `&[Attempt]` rather than the record, and `budget: Budget` as a parameter rather than
reading `MAX_ATTEMPTS`. `RawSignals` is a named struct rather than loose arguments, so a new signal
is `E0063` at its construction and `E0027` at both folds — ADR-0006's forced-site rule applied to
the completion test, closing the gap the spike logged in `FRICTION-fifth-signal.md`.

## Visibility carries two of ADR-0006's three properties, not three

ADR-0006 lists three properties as *who may call, not what may exist*. Two are carried by the
sibling walls above. The third — **only dispatch reads the environment** — cannot be carried by
privacy at all: privacy gates `crate::` paths, and nothing stops a render function calling
`std::env::var`.

Its carrier is a **source-level test**: `std::env` is named in exactly one module. The same test
shape asserts `std::process` and `std::fs` are named only in `world`, which is what makes *"`world`
is the only untested code"* a checked claim rather than an aspiration.

Source-level tests are string matching over one's own source, and they can be fooled
(`use std::env as e`). Accepted: the failure mode they guard is **convention**, and an agent
aliasing an import to dodge a test has crossed into **intent**, which ADR-0006 establishes no
carrier defends against.

## The cut, tested against decisions not yet made

[#35](https://github.com/FlorianRiquelme/grind/issues/35) set its own test: take an unresolved
decision and name the **one** module it lands in.

- [#13](https://github.com/FlorianRiquelme/grind/issues/13), fan-out health → **`observe`**. `world`
  globs `<uuid>/subagents/*.jsonl`, `observe` classifies newest-mtime into an `Observed<_>` signal.
  Adding it forces `E0063` at `RawSignals` and `E0027` at both folds. `decide`, `policy` and
  `render` change only if they choose to read it.
- [#16](https://github.com/FlorianRiquelme/grind/issues/16), the Handback's shape → **`render`**. It
  reshapes composition from an `Observation` and a `RunView`, and composition is pure. Delivery, if
  the Handback outgrows the terminal, is `world`.

## Consequences

- **[#36](https://github.com/FlorianRiquelme/grind/issues/36) inherits two obligations.** Grind's
  verify entrypoint must run **compile-fail tests** (shelling out to `rustc`, asserting `E0603`) and
  **source-level tests** over module contents. Both are carriers this ADR spends before that ticket
  has decided what `checked` means.
- **Ten modules for 591 lines is a lot of files**, and the friction tax across module boundaries is
  still unmeasured — ADR-0005 is explicit that 0–1 compile cycles on small additive changes says
  nothing about refactors that span modules. This spends an unknown, not a known-small cost.
- **`RunRecord` and `RunView` deserialize the same JSON in two modules that cannot see each other**,
  so field names are duplicated by design and can drift. The carrier is a test that both parse the
  same fixture — not the compiler, which is blind to it precisely because the wall is working.
- **`attempt` is the rule's one asterisk** — a pure builder and a pure classifier around two `world`
  calls, neither cleanly pure nor cleanly I/O.
- **Argv on the short-lived side is uncovered.** `parse_pr_view` can be perfect while the command
  was built with a wrong flag, and the three worktree call sites #31 flagged as the risky ones live
  exactly there. Covering it needs fake `gh`/`git` executables on a temp `PATH` — a third seam,
  declined here and named rather than forgotten.

## What this ADR deliberately does not say

**That the module count is minimal.** It is the smallest cut that keeps one rule per module and
keeps every pure thing testable from literals. A smaller cut exists; it buys fewer files and pays in
modules that do two things, which is the shape `bin/grind` already is.

**That crate walls are unnecessary in general.** They are stronger than module privacy, and the
argument against them here is cost against an unmeasured friction tax — not that they would fail. If
the friction tax turns out to be low and the `pub(crate)` escape is taken in practice, promoting
`supervisor` and `view` to crates is the reversible response, and this ADR is where that gets
revisited.
