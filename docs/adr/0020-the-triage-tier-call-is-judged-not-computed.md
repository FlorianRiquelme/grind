---
status: accepted
date: 2026-08-28
---

# The Triage tier call is judged, not computed

**Amends ADR-0015.** Triage keeps its static tier table as a recorded prior, but the Triage tier
itself is now decided by a one-turn LLM grader reading the Job rows and Plan facts. This replaces
the escalation-only tier computation at that one call and is the remedy for the degenerate
`template_record` routing documented in the five-run native batch (`docs/findings/0006`). Decided
resolving [#166](https://github.com/FlorianRiquelme/grind/issues/166).

## The evidence: five different Jobs, one identical tier receipt

In the 2026-08-26 native batch, all five Runs' Triage stages selected **T3** and emitted the same
byte-for-byte rationale row:

```json
{"signal":"template_record","value":"0 reverted of 10 runs, 1 unattended completions","weight":"-> t3"}
```

The Jobs ranged from a two-line render fix to a parser-plus-consumer feature, yet every one
received T3's cold-started sessions and strong-model plan review. `floor_from_plan` was T0 on all
five; the single historical template record outvoted every per-Job fact. A signal meant to scale
review by risk had collapsed to a constant, and the one static track-record term could always
defeat the evidence the tier table existed to weigh.

The owner direction recorded in [#166](https://github.com/FlorianRiquelme/grind/issues/166) is:

> triage is just a rust call right? maybe we need an llm classifier
>
> lets hold this for now, but i do lean towards llm grading it. I dont think we can produce any meaningful judging of complexity with static code without me interfering all the time

## The ruling

At Triage, after the normal Plan facts are collected, the supervisor dispatches one additional
strong-routed one-turn grader session. Its input is the Job rows, the Plan facts, the static tier,
and the static rationale as the recorded prior. Its output is `stages/triage/grade.json` with
`{tier, rationale[]}`, parsed strictly with `deny_unknown_fields`; rationale rows use the existing
signal/value/weight receipt shape. When the grade is readable, its tier **replaces** the static
tier at Triage, its rationale rows are recorded first, and the static rows remain after them as
the prior — the record shows why the winner won without erasing what it overcame. When the grade
is absent or unreadable, the supervisor fails closed to the unchanged static decision; missing
grader facts fall back to T2. The grader session's cost, turns, and session identity are recorded
on the Triage StageEntry as real values, not as a zero-token `[R]` row.

This seat is implemented as a `reflect`-style precedent: one judged judgment inside an existing
ladder stage, not a new rung and not a fan-out.

## The amendment, scoped exactly

ADR-0015 states that tier selection is deterministic, escalation-only, and fails closed: *"a diff
can only raise its own tier, never lower the plan's."* That sentence remains true from **Diff-triage
onward**. It no longer describes the initial Triage tier call.

The scope is narrow because the defect is narrow. A floor the grader cannot lower is not a safety
property here; it is a receipt-chaser that cannot lose. If the prior says T3 and the grader may
only agree or raise, the five-Run evidence would reproduce exactly: the grader becomes another
voter under a term that has already won. Graded-down is the point. The static table survives as a
prior so its reasoning and its fallback behavior remain auditable, not as a minimum the judgment
must respect.

Fail-closed semantics are unchanged: no grade or an unreadable grade yields the static decision,
and missing facts yield T2. The grader changes who decides a successful Triage call, not what
happens when the evidence needed for that call is unavailable.

## What does not change

- **ADR-0003** — the grade never gates. It sizes review bought; it never withholds a PR or blocks
  Ship on the strength of a finding.
- **ADR-0012** — the grader emits a tier choice plus rationale, never quality booleans or a
  taxonomy label beyond the existing T0–T3 vocabulary.
- **ADR-0006** — receipts remain signal/value/weight rows; statistics stay statistics and never
  become classification prose.
- **ADR-0010** — spend remains recorded, never bounded. The grader shapes what a Run spends by
  changing the tier; it does not cap any stage's cost.
- **Diff-triage** — still computes from the diff, still maxes with the floor, and remains a
  zero-token Rust pass.

## Consequences

- **Cost symmetry.** A Run pays one strong-routed grader turn at Triage. Against that, four of the
  five Runs in `docs/findings/0006` would have been cheaper at a lower tier even with the turn
  ceiling and re-entry costs included; preventing hours of T3 overtreatment is the intended trade.
- **`skills_hash` now covers the grader.** The grade skill is authored under `skills/run/` and
  provisioned with the other stage skills, so its text is frozen into the Run's provenance at
  dispatch like every other skill the Run can execute.
- **Drift tests must cover the new carrier.** The grade skill's documented schema, the supervisor's
  `GraderVerdict` handling, and the static-prior receipt order are contract surface; topology and
  golden-receipt tests must fail when the file format, fallback, or receipt ordering drifts.
- **The static table remains the fallback, not a stale decision.** If the grader is unavailable,
  Triage records the prior and continues; the ladder does not stop waiting for a judgment that
  failed to arrive.
