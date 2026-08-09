---
status: accepted
date: 2026-08-09
---

# Spend is recorded and surfaced, never bounded

A Run has no cost ceiling. Grind records what a Run cost, prints it, and hands it to the human in
the Handback — and nothing anywhere stops a Run for being expensive.

Decided resolving [#23](https://github.com/FlorianRiquelme/grind/issues/23), which asked whether a
Run should be bounded by spend instead of, or alongside, an Attempt count.

## What this removes

A Job could declare a `budget ceiling` row, which `job::spend_cap` turned into `--max-budget-usd`
on the `claude` invocation. That row is gone, along with the flag and the function.

It also had a defect worth naming, because it is *not* the reason for this decision: the cap was
applied to **each** invocation, so an 8-Attempt Run could spend 8× what the Job declared. Run 2
cost **$64.32**. The obvious fix was to make the row a Run total and derive each invocation's flag
as the remainder. This ADR is the decision not to do that.

## Why

**ADR-0004 already rules out the prediction a ceiling requires.** Its first section says the
supervisor never runs a pre-flight quota check, because *"even a perfectly informed supervisor
would be wrong about what a stage costs"* — the cost of an `lfg` stage is not knowable before it
runs. `--max-budget-usd` is exactly that prediction. Moving it earlier in time and handing it to a
human at Enqueue does not make it knowable; it makes it a guess by someone with strictly less
information than the supervisor would have had.

**Jobs are not the same size.** A $200 Run can be as valid as a $10 one, and which one a Job is
cannot be read off the issue that files it. A ceiling set at Enqueue can therefore only ever do one
thing: kill a Run mid-work for being larger than guessed — abandoning finished work at the exact
moment it was most expensive to abandon. Run 2 is the pattern already: it opened its PR on the last
Attempt its *other* bound allowed.

**`VERIFY_CONTRACT` is the standing precedent.** Grind records the verify contract and surfaces it,
and enforces nothing (ADR-0003). Spend is the same kind of fact.

**Nothing is left unbounded by removing it.** This is the part that makes the decision safe rather
than merely principled, and it only became true with #23. A **Wait** — an Attempt that did no work
— costs nothing and spends no budget, so spend without work is impossible. Spend *with* work is
bounded by the working-Attempt count. A $200 Run is eight Attempts that each did real work on a
large Job, which is the Job being large, not Grind running away.

**It was never exercised.** Run 2 declared `none`. The ceiling has never once bounded a real Run.

## Consequences

- **The Job format loses a row.** `budget ceiling` is not read, and a Job carrying one is not an
  error — it is ignored, the way any unknown row is.
- **`job::spend_cap` and its tests go**, and `Conditions::spend_cap` with them. No invocation
  carries `--max-budget-usd`.
- **Cost stays in the record and in the view.** `Attempt.total_cost_usd` is unchanged and
  load-bearing for something else now: it is half the predicate that makes an Attempt a Wait
  (#23), so removing the ceiling does not make cost optional to observe. `grind status` keeps
  showing dollars as the API-pricing counterfactual
  ([#12](https://github.com/FlorianRiquelme/grind/issues/12)).
- **A runaway Run is a human's problem, and the human has the tools.** `grind status` shows spend
  live and pull-only; killing a Run is what a human does about it. That is a deliberate trade: the
  failure this ADR accepts is *an expensive Run nobody stopped*, and the failure it refuses is *a
  correct Run abandoned mid-work for being expensive*. Only the second one destroys work.

## What would falsify this

A Run whose spend runs away **without doing work** — because that is the case the Wait predicate is
supposed to make impossible, and it is the only case a ceiling would have caught that the
Attempt count does not. If that happens, the repair is the predicate in #23, not a ceiling.
