# Amend #76's named Run 2 replay test to the claim it supports

Enactment of the ruling on
[#78](https://github.com/FlorianRiquelme/grind/issues/78): #76's named test cannot hold —
`docs/findings/0002` records Run 2's single denial on **attempt 8**, the Attempt that opened
the PR, so no denial-keyed predicate fires at two working Attempts. Leg 1 shipped the honest
substitute; this Job corrects the spec surfaces to match what shipped.

## Requirements

- **R1** — #76's named-test list no longer claims *"Replaying Run 2's attempt shapes reaches
  a Blocker at two working Attempts rather than exhaustion at eight."* It carries the
  Wait-arithmetic claim: replaying the eight attempt shapes leaves **five Waits and three
  working Attempts**, does not reach exhaustion on the attempt count, and **does not reach a
  Blocker**. The evidence cited is the single attempt-8 denial in `docs/findings/0002`.
- **R2** — The correction lives on issue #76 itself — a body edit or a correcting comment on
  the named-test list — referencing #78's ruling.
- **R3** — `docs/plans/2026-08-15-001-feat-leg-1-map-rulings-plan.md`'s deferred Open
  Question (`#76's named Run 2 replay test does not survive its own evidence`) records both
  rulings: Q1 amended and enacted by this branch; Q2 — a second, non-denial Blocker trigger —
  stays fog on [#5](https://github.com/FlorianRiquelme/grind/issues/5), with Run 3 as the
  instrument.
- **R4** — `just verify` green, and one open PR against `main` carrying the plan edit and the
  Run's own narrative.

## Non-goals

No source change, no second Blocker trigger, no new tests. The twenty-two-test count in the
plan's Definition of Done already reflects the substitution and does not move.
