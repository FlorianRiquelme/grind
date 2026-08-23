---
name: fixes
description: Grit's fix stage — rung 9 of the ladder. Applies only what Validate confirmed, re-running the Job's verify entrypoint each round.
---

# Fixes

Everything this stage touches has already survived a blind adversarial pass. That is what makes
it safe to spend turns on: churn was killed upstream, at Validate, not here.

## The return

Write `<stages-dir>/fixes.return.json` containing exactly `{"status": "complete"}` or
`{"status": "incomplete"}` — no other key. Artifacts (the round-by-round narration, the
before/after counts) live under `<stages-dir>/fixes/`, never in the return.

## What is eligible, and what is not

Read `<stages-dir>/validate/findings-validated.json`. Only rows marked **Confirmed** are eligible
at all — Refuted and Unfounded rows are not yours to touch; they are the Record's narrative, not
a queue.

Among Confirmed rows, the `autofix_class` decides how:

- **`gated_auto`** — apply directly. The validator already walked the failure; there is nothing
  left to weigh.
- **`manual`** — apply with care: read the surrounding code before editing, and prefer the
  smallest change that removes the confirmed defect over a rewrite of the region.
- **`advisory`** — **never apply.** An advisory row is a note for the human, not a fix for you to
  make. Applying it anyway is exactly the shape ADR-0003 forbids: turning a description into an
  enforced outcome.

## Rounds

Bounded by `FIX_ROUNDS` — 2 at T2, more at T3, absent (0) at T0/T1 where Validate itself is
absent or thin. Each round:

1. Apply every eligible Confirmed finding not yet applied.
2. Re-run the Job's own **verify entrypoint** (`job.verify_entrypoint` — the same invocation
   Work used, the same one CI-babysit will use later). Read its output; do not guess at pass/fail.
3. Narrate the round in `<stages-dir>/fixes/`: how many Confirmed findings existed, how many were
   applied, how many were skipped and why (advisory, or a `manual` fix that a re-run of verify
   showed made things worse and was reverted), and the verify entrypoint's outcome.

A finding a fix round could not actually clear — verify still fails on it, or the fix itself
introduced a new failure and was reverted — stays open into the next round if rounds remain, or
becomes a residual once rounds are exhausted.

## Exhaustion is not a stop

`FIX_ROUNDS` running out is an ordinary, described outcome, never a Blocker and never a reason to
hold the diff back. Write the residuals into the narration — which Confirmed findings never got
applied, and why — and let the ladder proceed to Ship. Ship renders those residuals into the PR
body; the human reads them there. ADR-0003 again: a Run that stops at a diff it could not fully
scrub is a gate, and Grind never gates.

## Never

- **Never weaken, trim, or remove an assertion to make verify pass.** A test that stopped
  checking what it used to check is a worse outcome than a fix round that ran out — a gutted
  check is a lie the Record cannot see, and a residual is an honest one it can.
- Never apply an advisory finding.
- Never re-open a finding Validate refuted or left Unfounded — those are not yours to act on
  here; they are calibration data and Record narrative respectively.
- Never let this stage decide the diff is "good enough" — it has no such verdict to give
  (ADR-0006's prohibited shapes name exactly this: a summary boolean over the fix queue is a
  gate one line from `if !ok { return }`).

*Fix what survived validation; exhaustion leaves residuals in the Record, not a stopped Run.*
