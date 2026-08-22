---
status: accepted
date: 2026-07-29
---

# Grind consumes `lfg` wholesale; it is a scheduler, not a pipeline

Grind does not implement a multi-stage implement/verify/review/fix pipeline. It
invokes the compound-engineering `lfg` skill headlessly and supplies only the four things
`lfg` has no opinion about: a queue, unattended dispatch, supervision of a run that dies,
and a record of what happened. The pipeline `lfg` already runs is more careful than the
one we designed — structured returns with `run_id` and `plan_checkpoint`, a
`verification_evidence` contract gating on how behaviour was protected, report-only review
with caller-applied fixes, bounded CI repair that may never weaken an assertion, and an
autonomous residual handoff that files durable records without prompting.

## Considered options

| Option | Verdict | Trade-off |
|---|---|---|
| **Invoke `lfg` wholesale** | **Chosen** | Everything above for free, plus upstream improvements. **Cost:** we inherit its opinions — no `depth:full` on its review step, no extra review lens, no control over its fix-round budget, and no gate of our own between stages. |
| Compose the same `ce-*` skills ourselves | Rejected for the PoC | Full control over every seam. **Cost:** `lfg`'s step-2 gate logic alone is more careful than anything we would write now, and we would re-derive it and then drift from it on every plugin update. |
| Build the pipeline from scratch | Rejected | This was the round-3 design. It reimplements a mature pipeline badly. |

## Consequences

- The plugin version is **pinned per job**, so a plugin update cannot change run behaviour
  mid-experiment and invalidate comparison across runs.
- Grind owns no shared state: it reads issues, writes run state to gitignored local
  disk, comments on the Job issue at dispatch and at every terminal state, and opens PRs.
  *(Superseded 2026-08-06 by ADR-0008: run state lives at `~/.grind/runs/`, outside any
  checkout, so it is never committed structurally rather than by a `.gitignore` line.)*
- If `lfg` turns out to be too opinionated, composing later is a mechanical decomposition
  of a known-good sequence rather than a redesign — and by then we will know which of its
  opinions actually cost us something.

> **Superseded 2026-08-22 by ADR-0015
> ([#92](https://github.com/FlorianRiquelme/grind/issues/92)).** This clause is exercised, not
> overridden: `docs/findings/0004` is the "by then we will know," and the opinion that cost
> something was the mega-session shape. Grind now owns a ten-stage ladder decomposed from the same
> known-good sequence this ADR chose to consume wholesale.
