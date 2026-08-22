---
status: accepted
date: 2026-08-22
---

# Grind owns its pipeline: a ten-stage ladder replaces `lfg`'s mega-session

**Supersedes ADR-0001.** Grind stops consuming `lfg` wholesale and owns its own pipeline instead:
a ten-stage ladder — Plan, Triage, PlanReview, Work, Simplify, DiffTriage, Review, Validate,
Fixes, Ship — of separately resumable stage sessions, with two zero-token Rust triage passes
(Triage, DiffTriage) sizing how much review a Run buys against its own plan and diff, and
adversarial validation attacking every finding before a Fixes round spends on it. Decided
resolving [#92](https://github.com/FlorianRiquelme/grind/issues/92).

## ADR-0001 reserved exactly this move

ADR-0001 chose to invoke `lfg` wholesale over composing the same skills itself, and said why in
its own Consequences: *"If `lfg` turns out to be too opinionated, composing later is a mechanical
decomposition of a known-good sequence rather than a redesign — and by then we will know which of
its opinions actually cost us something."* That clause is not being overridden; it is being
exercised. `lfg` turned out to be too opinionated in exactly the way it named: one mega-session per
Attempt, one fixed fan-out size, no seam between "resume" and "re-read the whole Run's history."
`docs/findings/0004` is the "by then we will know" — the opinion that costs something is the shape,
not any single stage's logic, which is why the decomposition below keeps every stage's content
`lfg` already proved (ADR-0001's "known-good sequence") and only cuts where the sessions join.

## The evidence: a small Job cost $132.98 to a shape, not to its own work

Run `20260821-170246-grind-87` (`docs/findings/0004`) is the fourth dogfood Run and the case this
decision rests on. Its own Intent row said *"the shape is decided, the prompt wording is not"* — a
five-module supervisor feature. Attempt 1 spent $84.85 across 149 turns and a fixed twelve-reviewer
fan-out before hitting the session limit; six free Waits carried it through the night, exactly as
designed; attempt 8 resumed the next morning and did twenty real minutes of finishing work for
$48.14, because resuming lfg's single session meant re-priming ~79.6M cache-read tokens of
accumulated context before the first useful token. Total: $132.98.

**The Run completed. Nothing about it failed.** The Wait/resume machinery is not what this ADR is
about — `0001` through `0003` already established it works, and `0004` confirms it again. What
`0004` measures is that *lfg has no cheaper lane for a Job that named its own shape as decided*,
and that *a re-entry and a full re-read of the Run's history are the same operation* when every
stage lives in one session. Both are shape defects, not resilience defects, and shape is the one
lever ADR-0001 declined to pull.

## The ruling

Ten stages, each one Attempt with its own session — `Plan, Triage, PlanReview, Work, Simplify,
DiffTriage, Review, Validate, Fixes, Ship`. Triage and DiffTriage are pure Rust, cost nothing, and
size the rest of the walk from the plan's and then the real diff's observable facts (lines
changed, risky-path hits, surface deltas, prior template outcomes) rather than firing every
persona on every diff. Review and Validate scale from one lens at the smallest tier to a
matched panel with adversarial confirmation at the largest. A death re-enters the stage it died
on, never the whole Run, so a resume never again means re-reading everything that came before it.

This is the mechanical decomposition ADR-0001 reserved — the same sequence, cut at its seams
instead of rebuilt.

## The avoided-word framing this retires

`CONTEXT.md`'s **Fan-out** entry defines itself by contrast to a stage: *"Claude Code substrate,
never an `lfg` stage — which is what lets the supervisor count it without observing the pipeline,"*
and its own `_Avoid_` list names *stage* directly. That framing was correct for as long as "stage"
named something inside `lfg` that Grind could not see — counting a fan-out was safe precisely
because it stopped short of observing the pipeline stage it belonged to. That distinction is gone:
`rung::Stage` is now a Grind type, its ten variants are what the supervisor observes directly
(durable return files, not a fan-out count), and CLAUDE.md's line *"Everything between plan and
open PR belongs to `lfg`. Don't reimplement stages it already runs"* no longer describes the
system. Both pieces of vocabulary need a follow-up edit once the ladder itself lands — this ADR
states that the framing is retired; rewriting `CONTEXT.md`'s Fan-out entry and CLAUDE.md's Shape
section is Job 2's work, not this one's, since the words should change the day the type does.

## Consequences

- **Skills become in-repo authored artifacts, versioned with the binary.** `skills/run/*` replaces
  the `compound-engineering` plugin; no marketplace, no pinned version, no `Latest` to refuse
  spelling. The plugin pin retires in a later phase (the de-plugin cutover Job), re-seated on
  provenance we already own: binary version plus a hash of the skills directory, frozen per Run the
  same way the plugin version used to be.
- **Each stage is one Attempt with its own session.** A Run is still bounded by Attempts that did
  work, never by wall clock or cost (unchanged from `CONTEXT.md`'s Attempt entry) — the ladder
  changes what an Attempt executes, not what one costs the budget.
- **Stage completion is supervisor observation over durable return files, never the agent's
  claim.** A stage is complete when its strict-serde return exists with a completion status and its
  artifact is on disk — the same discipline ADR-0004 already established for `done_promise`, now
  applied per stage instead of once per Run.
- **Advancement is a total pure function.** `rung::Stage::next(&StageReturns) -> Option<Stage>`,
  tested from literals, carries the climb logic that used to live smeared through `supervise()`
  scanning for the furthest completed stage.
- **Tier selection is deterministic computation over observable facts, with receipts — computation,
  never classification.** ADR-0012 holds: nothing about sizing a Run's review depth from plan and
  diff facts applies a label, and every selection is logged as signal-value-weight rows a human can
  replay. Escalation-only — a diff can only raise its own tier, never lower the plan's — and
  fail-closed to T2 on any parse failure or missing fact.
- **Nothing gates.** ADR-0003 holds without qualification: a tier mismatch, a Refuted finding, or a
  base mismatch on the opened PR is always a described fact in the Record, never a withheld PR. A
  stage that mismatches its own precondition re-enters or Blocks; it never silently prevents Ship
  from running.
- **Spend stays recorded, never bounded.** ADR-0010 holds: tiers shape how much a Run spends by
  sizing what fires, they do not cap what any stage may cost. A T3 Run that legitimately spends more
  than `0004`'s $84.85 first attempt is the tail bought consciously, not a runaway.

## What this does not change

- **ADR-0002** — headless still deliberately lags local. Every authored skill is proven in
  supervised sessions before it is ever dispatched headless; only the pin's carrier moves, from a
  marketplace version to binary version plus skills hash.
- **ADR-0005** — the supervisor is still not an agent. Judgment that used to happen inside `lfg`'s
  own steward calls now runs in steward sessions whose context is assembled by Rust before
  dispatch, not by Grind reasoning about the work itself.
- **ADR-0007** — module topology is untouched. The ladder's new pure logic (`rung`, the extended
  `decide`) joins the existing pure modules; `world` remains the sole namer of process and
  filesystem, and the sibling-privacy discipline over the writable record is unchanged.
- **ADR-0008** — the host is still declared by its layout. Owned skills resolve under the same
  `~/.grind/` layout the plugin cache used to occupy; `docs/provisioned-host.md`'s check moves from
  a plugin-installed test to a skills-present one, not to a new kind of host requirement.

## What this ADR does not do

It does not describe the ladder's stage contents, the tier table, the review personas, or the
learning loop — those are `FINAL-design-grit-pipeline.md`'s scope and land as their own Jobs. This
ADR states the ruling and its rationale: that ADR-0001's reserved decomposition has been exercised,
on the evidence of one measured Run, and names what does and does not change as a result.
