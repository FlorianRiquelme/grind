# Findings from weighing both backends' run records

The evidence audit behind epic [#135](https://github.com/FlorianRiquelme/grind/issues/135)'s P4
verdict ([ADR-0019](../adr/0019-both-adapters-stay-and-deletion-needs-priced-parity.md)). Every
Run record grind owns was inventoried (`~/.grind/runs/`, thirteen directories), then the seven
with real outcomes were read in full: two pre-seam claude-code Runs and the five-run native batch
of [findings/0006](0006-five-run-native-batch.md). One further native Run
(`20260826-150041-grind-161`, `z-ai/glm-5.3-flash`) supplies the adapter's first recorded
non-zero spend after [#140](https://github.com/FlorianRiquelme/grind/pull/140) wired
`usage.cost` into `total_cost_usd`.

## The ledger

| | claude-code — #80 | claude-code — #87 | native batch × 5 (ox-alpha) |
|---|---|---|---|
| Record | `20260821-065705` | `20260821-170246` | `20260826-0559xx` |
| PR | [#84](https://github.com/FlorianRiquelme/grind/pull/84) **merged** | [#89](https://github.com/FlorianRiquelme/grind/pull/89) **merged** | 5/5 **merged**: [#159](https://github.com/FlorianRiquelme/grind/pull/159), [#160](https://github.com/FlorianRiquelme/grind/pull/160), [#162](https://github.com/FlorianRiquelme/grind/pull/162), [#163](https://github.com/FlorianRiquelme/grind/pull/163), [#164](https://github.com/FlorianRiquelme/grind/pull/164) |
| Attempts | 3 of 8 | 8 of 8 | 14 each (budget) |
| Total turns | 63 + 4 + 4 = **71** | 149 + 6×1 + 51 = **206** | **1,822** |
| Total spend | **$38.18** | **$132.98** | **$0.00 recorded** (unwired; see below) |
| Wall clock | ≈ 27 min | ≈ 14 h 50 m, ~11 h of it 429-sleeping | hours, concurrent |
| Human touch after the fact | merge click only (merge tools denied to the agent) | PR finalized by hand; issue closed ~6 h after last attempt | three branches pushed, CodeRabbit-triaged and merged by hand |

## What the comparison says

**Spend polarity.** Real money exists only on claude-code rows: `$38.1751582 + $132.9842778 =
$171.16` for two Jobs, i.e. **$0.54/turn (#80)** and **$0.65/turn (#87)**. The native batch
recorded zero because its dispatch-time binary predates
[#140](https://github.com/FlorianRiquelme/grind/pull/140) (`accumulate_usage` folds each turn's
reported `usage.cost`; pinned by tests in `src/native.rs`). Post-fix, run
`20260826-150041-grind-161` recorded **$0.26302** — proof the field carries true charge now, but
a different model slug and a partial run, so **no per-turn native price can be quoted yet**.

**Turn density differs sharply, and Jobs differ too.** Native averaged ~364 turns/Job against
claude-code's ~139 — 2.6×. But the Job mixes are not comparable: #80 was a test-amendment, #87 a
notes feature whose attempt 1 alone burned 149 turns and $84.85 against a 13-wide fan-out; the
native five were defects in grind itself, each wading through
[#157](https://github.com/FlorianRiquelme/grind/issues/157)-era 32-turn walls that inflated
re-entry churn until [#167](https://github.com/FlorianRiquelme/grind/pull/167) recalibrated the
ceilings. Density gap is directionally interesting and numerically unactionable.

**Landing reliability converges; autonomy does not.** Seven Jobs in, seven merged PRs out —
both backends produce landable work. But the human's role differed: claude-code Runs ended
cleanly enough that a merge click sufficed (though #87's tree-clean disagreement still forced a
manual finalize), while three native Runs exhausted *after* their fixes were provably correct and
required branch salvage. Findings/0006's sentence stands: salvage is real, works, and is the
batch's largest hidden cost.

**Failure asymmetry.** claude-code's pain is upstream quota idleness (six consecutive
$0.00/1-turn 429 resumes spanning ~11 h — wasted wall time, but the deliverable survived).
Native's pain is budget exhaustion mid-ladder — wasted attention, because a human must read the
record before touching the branch. Neither failure mode corrupts work; they merely move labor
between the machine and the operator.

## What these numbers are not evidence for

- **No cost-per-quality claim across backends.** Native spend during the batch is unrecorded;
  the sole priced native run is a different model on a different Job class. That channel is now
  open: future native Runs go out on `z-ai/glm-5.3-flash`, whose $0.075/M input and $0.25/M
  output are a **50% promotional rate** (Z.ai list price: $0.15/$0.50) expiring 24:00 on
  September 9, 2026 UTC+8 — so the next batch produces priced rows this audit could only wish
  for, and any batch after the promo ends prices itself at double.
- **n = 2 vs n = 5, different Jobs, different models, different nights.** Nothing here normalizes
  across those axes.
- **claude-code was never dogfooded post-seam.** Its two records predate the `StageRunner` seam,
  use the pre-seam layout (no `stages/*/return.json`), and were selected from exactly the two
  claude-code-era records that survive on disk. If a fair priced comparison ever matters, it
  requires new dispatches — deliberate spend, separately approved.
- **The tier signal was degenerate** for the whole native batch
  ([#166](https://github.com/FlorianRiquelme/grind/issues/166)), so neither backend's numbers
  answer "does effort scale with risk?" yet.

Record provenance: both claude-code rows quoted from `run.json`, `supervisor.log` boundary lines,
and PR caches (#84, #89); native rows from
[findings/0006](0006-five-run-native-batch.md); the priced spot-check from
`~/.grind/runs/20260826-150041-grind-161/run.json`.
