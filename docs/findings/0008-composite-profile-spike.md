# Findings from the compliance-pin leg of the composite-profile spike

Run #189, the P3 leg of the throwaway composite-profile spike (issue
[#185](https://github.com/FlorianRiquelme/grind/issues/185)), whose Anchor artifact is
[`docs/plans/2026-08-28-001-composite-profile-spike-brief.md`](../plans/2026-08-28-001-composite-profile-spike-brief.md).
This leg is the compliance pin: the Job carried an `Agent` pin of `claude-code`, and the leg
exists to show that pin outranks the repo binding (`opus-plan`) the spike's P0 leg exercises on
this same repo.

## Compliance-leg observations

- **Per-stage backend.** `run.json`'s `stages[]` records the same backend for each of the three
  rungs this leg ran:
  - Plan → `claude-code`
  - Triage → `claude-code`
  - Plan-review → `claude-code`
- **Which tier won and why.** The Job's `Agent` pin (`claude-code`) executed all three rungs,
  which is the pin outranking the repo binding this Run's repo would otherwise have applied
  (`opus-plan`, per the spike's P0 leg).
- **Evidence-tree coherence.** All three rungs completed under a single adapter with no
  adapter-boundary crossing: each has a distinct `session_id`, and turn/cost data is present for
  all three (Plan: 17 turns, $1.09; Triage: 2 turns, $0.29; Plan-review: 6 turns, $3.06). The
  contrast is with a two-backend leg of the same spike, which would show the same question asked
  across an adapter boundary — this leg does not.
