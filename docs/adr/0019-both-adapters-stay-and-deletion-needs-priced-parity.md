---
status: accepted
date: 2026-08-26
---

# Both adapters stay selectable, and deletion needs priced parity

Epic [#135](https://github.com/FlorianRiquelme/grind/issues/135) reached its P4 question: after
P1's seam and P2's native adapter, P3's evidence exists —
[findings/0005](../findings/0005-native-backend-dogfood.md),
[findings/0006](../findings/0006-five-run-native-batch.md), and the two-backend audit of
[findings/0007](../findings/0007-both-backends-weighed.md). The plan named three verdicts in
order of preference: tier routing across backends, flipping the default, or deleting
`ClaudeCodeAdapter` outright.

The decision: **no backend is deleted, the default stays `claude-code`, native remains opt-in
via `~/.grind/agent`, and cross-backend tier routing is deferred until native spend is priced.**
The one-PR cutover survives because P1 contained the legacy adapter; it stays available precisely
so that a future flip remains one attribute move.

Recorded resolving P4 of the #135 plan
(`docs/plans/2026-08-23-001-feat-agent-harness-adapters-plan.md`).

## Why each rejected verdict fails on today's evidence

**Deletion requires parity on the dogfooded stage mix — not met, not close.** The parity test was
spend-and-quality comparability. Native spend during the measured batch is unrecorded (dispatch
binary predates [#140](https://github.com/FlorianRiquelme/grind/pull/140)); claude-code shows
$0.54–$0.65/turn across its surviving records. Quality: both backends landed every PR they
attempted (7/7 merges), but three of five native Runs needed human branch salvage while the
claude-code Runs did not. A backend whose autonomy ledger reads 2-of-5 unattended completions has
not earned being the only backend.

**Flipping the default spends trust before price.** The default decides what happens when
`~/.grind/agent` is absent — the configuration of every operator who never opted in. Native's
only priced datapoint is $0.26 on a different model slug and Job class
(`20260826-150041-grind-161`). Its turn density ran ~364 turns/Job against claude-code's ~139,
even granting that the Job mixes are incomparable. Flip criteria are stated below rather than
guessed at.

**Tier routing has a real seam but no calibration to route by.** The machinery is ready:
every stage's model crosses `supervisor::resolve_stage_model`, triage's tier already reaches the
adapter as `max_turns_for`, and rung ordering guarantees Work knows triage's decision. But the
tier signal itself degenerated to a constant for the whole native batch
([#166](https://github.com/FlorianRiquelme/grind/issues/166) — five-for-five identical triage
rationales), and routing *classes* per stage inside one backend already exists via
`fast=`/`strong=` in the agent grammar. Routing tiers across *backends* would automate a choice
grounded in no priced comparison. Deferred, not dismissed.

## What would change this verdict

Stated as falsifiable conditions, so revisiting is evidence review rather than relitigation:

- **Flip default**: at least one fully charged native batch (≥5 runs) on Jobs comparable to a
  known claude-code baseline, recording real `total_cost_usd`, matching or beating claude-code's
  unattended-completion rate (baseline: 2/2 clean records, one manual finalize).
- **Cross-backend tier routing**: first fix the signal it would key on (#166); then show the
  strong-tier stages carry spend where pricing beats claude-code per-turn.
- **Deletion**: all of the above plus the salvage loop becoming unnecessary across a batch —
  the cutover stays a one-PR move (`#[default]` relocation plus removing the branch behind
  `runner_for`, the codebase's single backend switch).

## Consequences

- `~/.grind/agent` remains the sole selection surface (ADR-0017 stands). Absent file = claude-code.
- Every future Run record prices itself through `usage.cost`; findings docs can quote real spend
  from this day forward. The operator has fixed the measurement problem's input side: all future
  native Runs go out on `z-ai/glm-5.3-flash` ($0.075/M input, $0.25/M output per
  [OpenRouter](https://openrouter.ai/z-ai/glm-5.3-flash#providers)) — no more uncharged rides on
  the stealth slug behind every $0.00 batch row. Combined with
  [#140](https://github.com/FlorianRiquelme/grind/pull/140)'s wired cost, the next native batch
  is the first whose flip criteria above can be evaluated as written.
- grind no longer needs more native evidence to stay as-is; it needs charged evidence to change.

The verdict is recorded with the evidence that exists, deliberately short of certainty: nothing
here predicts which backend wins a priced comparison — it only ensures that when the comparison
happens, both candidates are still selectable and the result is one attribute move away either
way.
