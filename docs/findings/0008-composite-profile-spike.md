# Findings from the composite-profile spike Runs

Two throwaway Runs dogfooded the composite-profile grammar (#185): the P0
two-backend leg (Job #187, branch `feat/185-p0-spike`) and the P3
compliance-pin leg (Job #189, branch `feat/185-p3-compliance`), both
dispatched from the brief
[docs/plans/2026-08-28-001-composite-profile-spike-brief.md](../plans/2026-08-28-001-composite-profile-spike-brief.md).
Each doc below is written from that Run's own record at Work time (`run.json`
and `stages/triage/decision.json` under `~/.grind/runs/<run-id>/`). Only
figures are quoted and summarized; no file from `~/.grind/runs/` is copied
into the repo (ADR-0008).

## Leg 1 — the two-backend composite (`20260828-212910-grind-187`)

The Run drives one throwaway Job through a ladder whose stages are deliberately
split across two adapters by the `opus-plan` class binding. Figures quoted from
the Run's own record: `run.json` and `stages/triage/decision.json` under
`~/.grind/runs/20260828-212910-grind-187/`.

### Dispatch binding

Verbatim from `run.json` `class_routes`: `fast` routes to backend `omp` with
model id `openrouter/z-ai/glm-5.3-flash`; `strong` routes to backend
`claude-code` with model id `claude-opus-5`. The Run's line backend is `omp`.
`omp_version` reads `18.0.9`; `claude_version` reads `2.1.250 (Claude Code)`.
`provenance.binary_version` is `0.1.0` and `provenance.skills_hash` is
`b7bea3223c7fa7a5`.

### Per-stage backends

One row per ladder rung, in ladder order. A stage whose `stages[]` entry
carries no `backend` key reads `absent` (the field is
`skip_serializing_if = "Option::is_none"` on `StageEntry`, `src/rung.rs`); a
stage with no entry yet reads `unobserved`.

| Stage | Class | Backend | Model id | Turns | Cost USD | Source |
|---|---|---|---|---|---|---|
| Plan | strong | claude-code | claude-opus-5 | 24 | 1.694055 | run.json stages[] (figures); class routed strong (src/supervisor.rs:1606-1607) |
| Triage | strong | absent | claude-opus-5 | 3 | 0.321835 | run.json stages[] (figures); class from strong fallback (src/supervisor.rs:1614-1617) |
| Plan-review | strong | claude-code | claude-opus-5 | 24 | 2.462641 | run.json stages[] (figures); class from triage decision (src/supervisor.rs:1614-1617) |
| Work | fast | unobserved | unobserved | unobserved | unobserved | triage decision (not yet executed) |
| Simplify | fast | unobserved | unobserved | unobserved | unobserved | triage decision (not yet executed) |
| Diff-triage | strong | unobserved | unobserved | unobserved | unobserved | triage decision (not yet executed) |
| Review | fast | unobserved | unobserved | unobserved | unobserved | triage decision (not yet executed) |
| Validate | strong | unobserved | unobserved | unobserved | unobserved | triage decision (not yet executed) |
| Fixes | fast | unobserved | unobserved | unobserved | unobserved | triage decision (not yet executed) |
| Ship | fast | unobserved | unobserved | unobserved | unobserved | triage decision (not yet executed) |

`stages[]` carries three entries at write time: `plan` (backend
`claude-code`), `triage` (no `backend` key at all), and `plan-review` (backend
`claude-code`). The classes for the unobserved rows come from
`stages/triage/decision.json`'s `model_per_stage`, which assigns `fixes`,
`review`, `ship`, `simplify`, and `work` to `fast` and `plan`, `plan-review`,
and `validate` to `strong`. Triage and Diff-triage have no `model_per_stage`
key, so they fall to `strong` via the fallback (`src/supervisor.rs:1614-1617`);
Plan is always routed `strong` before any Decision exists
(`src/supervisor.rs:1606-1607`).

### Which adapter executed which stage

The `opus-plan` binding intends Plan to ride the foreign `claude-code` route
while the workhorse stages ride `omp`. What the record shows at write time:

- **Plan** — entry present, `backend: claude-code`, model `claude-opus-5`,
  24 turns, `$1.694055`. The strong route executed it, as the binding intends.
- **Triage** — entry present, but it carries **no `backend` key**. Its adapter
  is absent from the record; the doc records that absence rather than inferring
  an adapter from the class route. The entry does name model `claude-opus-5`
  (the strong model), 3 turns, and `$0.321835`.
- **Plan-review** — entry present, `backend: claude-code`, model
  `claude-opus-5`, 24 turns, `$2.462641`. Again the strong route.

No stage has yet been observed executing under the `omp` fast route: the fast
stages of the ladder (Work, Simplify, Review, Fixes, Ship) are all unobserved —
their entries land when each stage returns, and `stages[]` is append-only, so a
Run writing at Work time observes `plan`, `triage`, and `plan-review` and
nothing after itself. Work itself is among the unobserved. No adapter claim in
this doc goes beyond what the entries state.

### Evidence-tree coherence across the adapters

Per adapter that executed at least one observed stage. Triage's adapter is
absent from the record, so it cannot be listed as its own adapter; its entry's
figures are covered under the claude-code check's caveats below.

### claude-code (Plan, Plan-review)

- **Per-stage return file:** `stages/plan.return.json` and
  `stages/plan-review.return.json` both exist under the run dir and parse as
  JSON. Present.
- **Stage artifacts:** `stages/plan/` holds `anchor-plan.md` and
  `plan-facts.json`; `stages/plan-review/` holds `findings.json` and
  `revision.md`. Present.
- **Turns and cost:** `turns` and `cost_usd` are present and populated on both
  entries (24 / `$1.694055` and 24 / `$2.462641`). Present.

### Triage (adapter absent from the record)

- **Per-stage return file:** `stages/triage.return.json` exists and parses as
  JSON. Present.
- **Stage artifacts:** `stages/triage/` holds `decision.json` and
  `grade.json`. Present.
- **Turns and cost:** `turns` (3) and `cost_usd` (`$0.321835`) are present and
  populated on the entry. Present — but the entry names no adapter, so which
  adapter produced the spend is not stated by the record.

### Transcript/stdout per attempt

`attempt-1.prompt.txt`, `attempt-1.stderr.log`, `attempt-1.stdout.json`,
`attempt-2.prompt.txt`, `attempt-2.stderr.log`, and `attempt-2.stdout.json`
all exist at the run-dir root, and `run.json` records two completed attempts
(n = 1 and n = 2, both `exit_code: 0`). Present.

These rows are the first observation of this Run's cost figures. They are not
an answer to the open question findings/0007 left: that audit weighed
claude-code against **native** and never names `omp` at all (`native` and
`omp` are separate `runner::Backend` variants, `src/runner.rs:88`), and its
unpriced channel was native's. Whether `omp` records spend is not yet
observed — no `fast`-routed stage has returned, and Work, the first of them,
is unobserved at write time — so this Run's rows are the first data that
could bear on the question, recorded as that rather than as an answer to
0007's; 0007 never measured `omp`.

### What the observation cannot cover

- **`stages[]` is append-only.** Downstream stages are unobserved at Work
  time; the tail of the table is necessarily partial, and a second commit from
  a later stage is the human's call downstream of this Run.
- **Which stages ride which adapter is a Triage output, not a ladder
  property.** Plan is always `strong` (`src/supervisor.rs:1606-1607`); every later
  stage's class comes from Diff-triage's decision when present, else Triage's,
  else `strong` (`src/supervisor.rs:1614-1617`). This doc records the Triage
  decision it actually read.

Record provenance: figures quoted and summarized from
`~/.grind/runs/20260828-212910-grind-187/run.json` and
`~/.grind/runs/20260828-212910-grind-187/stages/triage/decision.json`, read at
Work time; evidence-tree contents listed from the run directory itself. No
record file is copied into the repo.

## Leg 2 — the compliance pin (`20260828-222129-grind-189`)

### Compliance-leg observations

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

## Post-terminal addendum (human-verified, 2026-08-29)

The Work-time tables above are partial by construction (`stages[]` is
append-only; see "What the observation cannot cover"). Final record states,
read after both Runs reached terminal:

- **Leg 1 (`…187`)**: terminal `completed`, 8 attempts, 4 commits, PR open,
  `reflected = true`. Full ladder: plan/plan-review/review/validate =
  claude-code ($1.69/$2.46/$3.06/$0.86), work/simplify/fixes/ship = omp,
  diff-triage = `[R]` (backend absent, honest). Reflect metered $6.64.
- **Leg 2 (`…189`)**: terminal `completed`, 8 attempts, 3 commits, PR open,
  `reflected = true`. Every rung claude-code (Job pin `claude-code` outranked
  the repo binding `opus-plan`); `class_routes` absent, `omp_bin`/`omp_version`
  absent. Metered total $10.16.
- **omp cost channel**: the unmetered omp rows above are a channel fact, not a
  zero-spend fact — no `total_cost_usd`, no usage frame, and
  `advisor_cost_changed` carries no payload on merge-gateway/glm-5.3-flash
  (omp v18.0.9). A metered channel is the prerequisite for any ADR-0019
  default flip (verdict recorded on issue #189).
