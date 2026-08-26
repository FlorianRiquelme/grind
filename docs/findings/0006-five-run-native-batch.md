# Findings from the five-run native batch

Five concurrent native-backend Runs, dispatched within one second of each other on 2026-08-26
(05:59:19–05:59:20 local), one against each of [grind#154](https://github.com/FlorianRiquelme/grind/issues/154),
[#155](https://github.com/FlorianRiquelme/grind/issues/155), [#156](https://github.com/FlorianRiquelme/grind/issues/156),
[#157](https://github.com/FlorianRiquelme/grind/issues/157) and [#158](https://github.com/FlorianRiquelme/grind/issues/158)
— five defects in grind itself, filed so that each Run fixed the defect its own issue describes.
Every Run declared `native` with `fast_model_override` and `strong_model_override` both
`stealth/ox-alpha` over OpenRouter, carried a 14-attempt budget, and walked the full ten-rung
ladder. This is the batch epic [#135](https://github.com/FlorianRiquelme/grind/issues/135) was
waiting for: **five full ladders through the pluggable native adapter, zero ladder changes** —
the architecture thesis holds under concurrency, not just once.

## Outcome

| Run | Issue | PR (merge) | Recorded state | Where it stopped | Turns |
|---|---|---|---|---|---|
| `20260826-055919-grind-158` | #158 | [#159](https://github.com/FlorianRiquelme/grind/pull/159) (`6e8dc9c`) | `completed` | Ship attempt 13 → Completed; Reflect ran post-terminal and died at the wall without blocking | 288 |
| `20260826-055920-grind-154` | #154 | [#162](https://github.com/FlorianRiquelme/grind/pull/162) (`38d1e50`) | `completed` | Ship attempt 13 → Completed; its Codex bot review was quota-blocked and carried forward by hand | 381 |
| `20260826-055920-grind-156` | #156 | [#160](https://github.com/FlorianRiquelme/grind/pull/160) (`7fb3d72`) | `exhausted` | Post-PR: Ship re-entered twice on `Incomplete(["no check pending"])`, budget died at attempt 14 | 343 |
| `20260826-055920-grind-157` | #157 | [#163](https://github.com/FlorianRiquelme/grind/pull/163) (`6d5758f`) | `exhausted` | Mid-validate: validate's third attempt completed at turn 25, but the budget was gone before Ship or Fixes could run | 388 |
| `20260826-055920-grind-155` | #155 | [#164](https://github.com/FlorianRiquelme/grind/pull/164) (`eafe29c`) | `exhausted` | Work: eleven consecutive attempts, every one ending incomplete at the 32-turn wall | 422 |

1,822 stage turns across the batch; every recorded `cost_usd` is `0.0` (the findings/0005
cost-field gap still stands — API pricing, uncharged locally). Four of five Runs terminated
without shipping themselves; all four branches were salvaged by hand and merged anyway. **All five
defects are fixed on main**: #159, #160, #162, #163, #164.

## The tier signal has degenerated to a constant

All five triage stages decided `t3`. All five rationales are byte-identical:

```json
{"signal":"template_record","value":"0 reverted of 10 runs, 1 unattended completions","weight":"-> t3"}
```

One template-record receipt — ten historical Runs, zero reversions — outvoted every per-Job input:
`floor_from_plan` was `t0` on all five, and the Jobs range from a two-line render fix (#158) to a
parser-plus-consumer feature (#157). The signal that exists to scale effort by risk currently
cannot say anything except "t3". Mid-run diff-triage agreed with itself the same way: the
supervisor logs show `[R] diff-triage decided t3 (floor t3)` on three of the five Runs. A tier
ladder whose every rung reads the same is not calibrating anything; this batch is the datapoint
that says the template-record weight needs a competitor or a cap.

## The 32-turn ceiling is the binding constraint of the whole batch

`MAX_TURNS = 32` (issue [#157](https://github.com/FlorianRiquelme/grind/issues/157)'s subject) was
hit **35 times** across the five Runs — 35 of 79 stage attempts ended `incomplete` at exactly the
wall:

| Stage | Wall deaths | Notes |
|---|---|---|
| work | 18 | incl. eleven straight in Run #155 |
| plan | 4 | Run #154 needed three plan attempts |
| simplify | 4 | |
| validate | 3 | |
| review | 2 | |
| reflect | 2 | both post-terminal; neither blocked completion |
| plan-review | 1 | |
| fixes | 1 | Run #154: died at 32, finished on resume in 13 |

Three *completes* also landed on exactly 32 — #154's plan-review, #156's plan-review and review —
stages finishing with zero margin. Where stages did finish cleanly they needed far less (validate
completes at 13, 15, 18, 25; most others under 30), which is the signature of a ceiling that is
wrong for a minority of slices rather than tight for everyone: the same stage class sometimes fits
and sometimes does not, and when it does not, the whole slice is thrown away and re-bought.

The price of throwing a slice away is not just the lost turns. Issue #157 measured it on the
preceding night's run:

> each slice boundary costs re-orientation (~300k prompt tokens per attempt tonight)

Run #155 is the worst case made visible. Its supervisor log shows the pattern — the walk ends
without its done promise, the supervisor honestly re-enters at the stage that died, and the fresh
attempt pays orientation again:

```
[2026-08-26T08:20:26+00:00] work attempt 7 (resume) …
    -> ended | stage=dispatched | commits=0 | cost=$0.00 | Incomplete(["PR open", "tree clean", ...])
    ended without a DONE promise — re-entering at the stage that died
```

…repeated through `work attempt 14`. Eleven work attempts × 32 turns = **352 of the run's 422
turns spent inside one stage**, with commits appearing only from attempt 11 onward. The fix itself
was complete — tree clean, 3 commits ahead at exhaustion — and PR
[#164](https://github.com/FlorianRiquelme/grind/pull/164) landed it verbatim. The ladder never
learned that, because the handoff never happened inside the budget.

## Exhaustion mid-validate strands proven findings

Run #157 built its entire feature (5 commits: the tiers.toml ceilings, the tolerant parser, the
`max_turns_for` consumer, two simplify passes) and reached Validate. Validate died at the wall
twice, completed on its third attempt at turn 25 — and that was attempt 14 of 14:

```
[2026-08-26T09:40:32+00:00] validate attempt 14 (resume) …
    -> ended | stage=implemented | commits=5 | cost=$0.00 | Incomplete(["PR open", ...])
```

Validate's findings existed; no attempt remained to let Fixes apply them. The record stops there,
and a human applied what validate had proved as `72dad00` ("fixes: apply what validate proved
before the budget died") on top of PR [#163](https://github.com/FlorianRiquelme/grind/pull/163).
That is the failure mode in one sentence: **a budget death after evidence is produced destroys the
evidence's only consumer.**

## Ship can starve waiting on checks that never settle

#156's own issue predicted it, and the run reproduced it live. After opening its PR, ship's done
predicate includes `no check pending`; the checks never resolved, so the supervisor re-entered ship
until the budget ran out:

```
[2026-08-26T09:42:51+00:00] ship attempt 13 (dispatch) …
    -> ended | stage=pr-open | commits=7 | cost=$0.00 | Incomplete(["no check pending"])
...
[2026-08-26T09:51:26+00:00] ship attempt 14 (resume) …
    -> ended | stage=pr-open | commits=7 | cost=$0.00 | Incomplete(["no check pending"])
```

State `exhausted`, deliverable already on a merged-bound branch. The contrast case shows the
predicate is not simply broken: #158 also hit `Incomplete(["no check pending"])` twice and then
corroborated on attempt 13 — because its checks actually settled. The difference between the two
Runs is one boolean on GitHub's side, and only one of them had the budget margin to survive it.
The repair is [#162](https://github.com/FlorianRiquelme/grind/pull/162): a pending-but-not-failed
check rollup corroborates once the deliverable exists.

## Salvage is manual by construction, and worked

A native Run cannot push: every record denies `Bash(git push*)` among other write paths to main.
So when a Run exhausts with landable work, a human must push the branch, then run the CodeRabbit
review-triage loop by hand until green, then merge. That loop ran four times in this batch and
produced four merges (#160, #162, #163, #164) plus the clean #159. One wrinkle: #162's Codex bot
review never ran —

> You have reached your Codex usage limits for code reviews.

— and the merge proceeded deliberately without it, quota-blocked review carried forward rather
than blocking a verified fix. The salvage path is real, but it is also the batch's largest hidden
cost: four hand-landings is four Runs' worth of human attention that the autonomy thesis is
supposed to buy back.

## This batch is the baseline, not the steady state

Because every defect these Runs hit was filed from earlier evidence and fixed by these very Runs,
the next dispatch is the first that benefits from all of: #162's check-wait corroboration, #163's
per-stage ceilings read from `docs/tiers.toml` instead of a compiled constant, and the calibration
shipped alongside this document (`work = 64`, `validate = 48`, `fixes = 96`). The measured needs
those values rest on:

- **work**: eighteen wall deaths in this batch alone, including #155's eleven-attempt stall and
  issue #157's measurement of the prior dogfood run (~40 turns: 32 + resume); 64 gives a
  two-slice budget where 32 did not finish one.
- **validate**: clean completes landed at 13–25 turns, but three of six validate attempts died at
  the 32 wall — including #157's, where the third attempt needed 25 and two full slices were
  burned first. 48 covers the observed worst slice plus headroom for one re-entry.
- **fixes**: the largest observed fixes need is issue #157's measurement of run
  `20260825-192521-grind-138`: ~112 turns (32/32/32/16) across four attempts, scaling with
  confirmed findings ∝ tier. 96 keeps the per-attempt ceiling near the measured single-run need;
  in this batch fixes completed in 12 and 20 turns once it got a turn at all.

These are data-derived, human-approved values; the absent stages keep reading the compiled
fallback of 32, and the `[max_turns.t2]` override stays untouched as the receipt that overrides
bind.

## What these Runs are not evidence for

- **Nothing about claude-code.** Every number above is native-backend-only; the legacy backend has
  its own turn dynamics and was not exercised.
- **No cost-per-quality claim.** With `cost_usd` hardcoded to 0.0 (findings/0005) nothing here
  prices a ladder, let alone compares quality per dollar.
- **n = 5, same repo, same model, same night.** Five concurrent Runs share one template record,
  one host, one disk, and one model slug. The tier-degeneracy and ceiling findings will replicate
  or not on the next batch; treat these as one data point with five samples of it.
- **No verdict on whether 64/48/96 are right.** They are measured-needs-derived and approved; the
  next batch measures again, and the ceilings are data precisely so this sentence can be revised
  in a reviewed diff.

## Provenance and mechanics worth recording

- Dispatch: five supervisors launched within one second (`created_at` 05:59:19–05:59:20 +00:00) on
  `Florians-MacBook-Pro-2.local`, grind binary `0.1.0`, `skills_hash ef17554580a19b6a` frozen in
  all five records.
- Operator-side, not in any Run record: host disk hit 98% full mid-batch (~3 GiB free at peak);
  four of the five verifies survived only after regenerable build caches were cleared by hand.
- The tier decision lives at `stages/triage/decision.json` in each record; the identical rationale
  quoted above appears in all five.
- Reflect ran post-terminal on both `completed` Runs and died at the wall both times without
  affecting state — consistent with findings/0005's note that reflect status is best-effort; the
  reflect-honesty question (#146) remains open.
- Records consulted for every number in this document: `~/.grind/runs/20260826-055919-grind-158/`,
  `~/.grind/runs/20260826-055920-grind-{154,155,156,157}/` — `run.json` (state, attempts, stage
  entries), `stages/*/return.json`, `supervisor.log`, plus PR threads #159/#160/#162/#163/#164 and
  issues #154–#158 on FlorianRiquelme/grind.
