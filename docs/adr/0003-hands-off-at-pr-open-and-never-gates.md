---
status: accepted
date: 2026-07-29
---

# Grind hands off at an open PR and never gates

A Run produces an initial batch of code that has been planned, implemented, simplified,
reviewed, had eligible findings applied, and watched until CI is decided — then it stops.
It never asserts that work is ready, never blocks a PR from existing on the strength of a
review finding, and never treats a green check as a completion signal. The gate is
downstream of Grind and Grind does not own it: an agent-run review over the PR, and the
human's merge decision over its record. From the open PR onward the human owns the feature:
they check it, post findings in its channel, work with day agents, and either finish it or
re-file it as a new Job. Re-filing is the only feedback path into Grind.

This is also the seam `lfg` already draws — its pipeline-mode CI watch stops at "CI
decided", not merged, and it hands the interactive watch-to-merge back to a human.

## Considered options

Gating the Run's own output on review findings was considered and rejected. The argument
for it was that an unattended run has nobody to read an advisory at 03:00, so findings must
be able to stop a ship. The argument required a distinction between gating a human's push
and gating a robot's draft PR, and that distinction is unnecessary: the output already
lands behind a gate Grind does not own. "Blocking" was doing three separate jobs —
triggering more fix work, colouring the verdict, and preventing the PR from existing. The
first two are fine and internal; only the third is a gate, and only the third carries the
cost that a false positive destroys the whole run's output rather than costing one fix
round.

## Consequences

- Verdict language describes what happened, never quality. A completed run means the
  pipeline finished, not that the code is good.
- The dangerous direction is the inverse of the one we worried about: a green review must
  never be read as licence to mark work ready.
- Because feedback arrives only by re-filing, Grind tracks nothing about whether the
  human addressed anything. Each Run is a fresh batch off the queue.
- This keeps Grind clear of the consumer-side advisory/enforcing question owned by
  `svo-engineering-guidelines` (its ADR-0004): a system that never gates has no stake in it.

## Amended 2026-08-03 — what "the gate" is

The decision is unchanged; its stated reason was wrong. This ADR justified never gating with
*"Human review is the gate, as it already was."* Resolving
[#6](https://github.com/FlorianRiquelme/grind/issues/6) established that in the author's own
repos there is no human code review at all: review is delegated to agents end to end, and
the human reads the PR's narrative rather than the diff. Under a client compliance
obligation the author does hand-review, but that context is out of scope for leg 1.

Nothing above depends on the corrected reading — the false-positive argument stands on its
own, and is what the decision rests on. The correction matters because an accepted ADR
asserting a careful human reader will send anyone designing the handback at a target that
does not exist.
