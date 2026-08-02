# Findings from the first Run

Run `20260802-105828-snapper-21` against [snapper#21](https://github.com/FlorianRiquelme/snapper/issues/21),
slice 1a. Dispatched by hand 2026-08-02 10:58 CEST, completed 19:02 CEST.

This is the run `BRAINSTORM.md` § Next step asks for. What follows is what it answered that no
further design would have.

## Outcome

| | |
|---|---|
| Result | **[PR #23](https://github.com/FlorianRiquelme/snapper/pull/23) open** — 64 files, +12,744 lines |
| `just verify` | **exits 0**, all seven contracted steps present |
| Attempts | 5 (1 dispatch, 4 re-entries) |
| Cost | **$40.90** |
| Wall clock | 8h04m, much of it the laptop asleep |
| Commits | 3 · 7 ADRs · `CONTEXT.md` · `docs/ledger/` with 1 real entry · `docs/manual-tests.md` |
| Tool denials | 0 |
| Residual review findings | 0 |

The Job's definition of done was `just verify` passing. It passes, and the supervisor's
independent contract check agrees with the Run's own claim — nothing was trimmed to get there.

## First data for the four metrics

Every metric in `STRATEGY.md` read zero data before this. Now each reads one run.

- **Morning decisions per run** (primary) — **two**, both pre-framed with the argument and
  explicitly marked strikeable: whether to keep `plutil -lint` inside the build assertion, and
  whether to keep `bundle.targets: ["app"]`. Both are things the Run added *beyond* the anchor
  and flagged rather than folded in silently. A third soft one: whether the degraded plan review
  (below) warrants a re-review. That is a low morning cost, and the shape is right — the PR does
  the arguing, the human only rules.
- **Unattended completion rate** — 1/1 reached an open PR, with four mid-run re-entries and
  **zero human interventions**.
- **Weekly-limit cost per run** — $40.90 for a slice that was deliberately boring. Attempt 1
  alone was $23.51 across 36 turns; planning was the expensive half.
- **Self-diagnosable failures** — every death was diagnosable from run state alone. `run.json`
  carried the subtype, turn count, cost and result tail per attempt, and the cause
  (`Connection closed mid-response`) was legible without opening a transcript.

## The design record was wrong about re-entry

`BRAINSTORM.md` § A run says *"`lfg` returns `run_id`, `plan_checkpoint`, `unit_receipts` and
`recovery_path`. A dead run re-enters at the stage it died on."*

It does not. Those are what `ce-work` returns **to** `lfg` at its step 2 — internal to the run
(`lfg` SKILL.md step 2 GATE). `lfg`'s only caller-visible terminal signal is
`<promise>DONE</promise>`.

Re-entry therefore rides Claude Code's own session resume: the supervisor picks the session id
up front with `--session-id` and re-enters with `--resume`. Stage is inferred from durable
artifacts — plan file, commits ahead of the Handoff SHA, PR, residual findings — rather than
read from a return value.

## The failure mode is a dropped connection, not a rate limit

| Attempt | Mode | Turns | Wall clock | Cost | Ended with |
|---|---|---|---|---|---|
| 1 | dispatch | 36 | 5h39m | $23.51 | `API Error: Connection closed mid-response` |
| 2 | resume | 3 | 15m | $2.35 | same |
| 3 | resume | 1 | 15m | $0.12 | same |
| 4 | resume | 18 | 2h16m | $11.74 | completed the pipeline, no DONE promise |
| 5 | resume | 3 | 2m | $3.18 | **DONE** |

The cause was the laptop sleeping (`pmset -g log` confirms repeated sleep/wake cycles). **The
supervisor re-entered each time and work continued each time** — the resilience layer doing the
one job it exists for, against a real failure rather than a synthetic one.

The important detail: a dropped connection presents as `subtype: success`, `is_error: false`,
`stop_reason: stop_sequence`. It is **not** an error. The rate-limit branch correctly declined
to match it; the generic *no DONE promise → re-enter* path is what caught it, and that path
turned out to be the load-bearing one. Limit handling was designed for first; connection loss is
what actually happened, four times, and no rate limit was hit at all.

**Consequence:** an unattended Run on a laptop dies on every sleep. Re-entry is not a nicety
here, it is the difference between a Run existing and not.

## The DONE promise is a fragile completion signal

**Attempt 4 finished the entire pipeline** — PR open, `just verify` green, tree clean, branch
pushed — and returned `done_promise: false`. Its narration ends *"Stopping at the open PR as
instructed — not merging"* with no promise tag.

The supervisor could not distinguish *finished* from *died*, so it burned attempt 5 ($3.18)
re-entering a completed Run. Attempt 5 did the right thing — *"the interruption landed after the
pipeline had already completed"*, verified on the committed HEAD rather than assumed, then
emitted DONE — so the cost was small and the outcome correct. But the mechanism is wrong.

**Fix:** treat completion as corroborated rather than declared. The supervisor already observes
PR-open plus an intact verify contract plus a clean tree; those should be able to conclude a Run
on their own, with the promise as one signal among several rather than the only one.

Open question this raises: the dispatch prompt ends *"Stop at an open PR. Do not merge it."* It
is plausible that wording suppressed `lfg`'s step-10 promise by making the Run think stopping
*was* the instruction. Worth testing on the next Run by varying only that line.

## The review fan-out degraded silently in headless

From the PR body, disclosed by the Run itself:

> The document-review fan-out over the *plan* largely failed — one reviewer of five returned
> (scope-guardian, 3 findings, no P0/P1); the other four were interrupted or died on a
> connection error, and I ran those lenses in-thread, which is not independent corroboration.

The *diff* then got a genuinely independent review that found no blocking issues and re-derived
the three load-bearing claims itself rather than trusting the Run's word — injecting an unused
variable to prove clippy's `-D warnings` fires, injecting a type error in `vite.config.ts` to
prove the double-`tsc` is load-bearing, and confirming `eslint .` really reaches `src/`.

Two things follow. The honest self-report is exactly what ADR-0003 wants — verdict language
describing what happened, not asserting quality. But **a review stage can half-fail and the run
still completes**, which is the failure ADR-0002 exists to worry about. The supervisor sees
nothing of this today; it is only visible because the Run wrote it into the PR.

## Two pins were missing

- **The model was not pinned.** A Job pins its plugin version so Runs stay comparable
  (ADR-0002); the model was left to whatever the dispatching shell defaulted to. Fixed: a Job
  carries an optional `Model` row, `GRIND_MODEL` overrides, recorded in run state.
- **`claude` on PATH resolves to cmux's wrapper shim**, which injects that host's session hooks
  into the Run, so a Run silently depended on the terminal it was dispatched from. Fixed: the
  supervisor resolves the real binary.

## A context-ceiling worry that did not materialise

The whole pipeline shares one context window — every stage is a nested Skill call in the same
session. At 13 minutes the parent was at ~124k tokens and compaction looked near-certain against
an assumed 200k ceiling.

**The assumption was wrong.** The parent passed **~270,000 tokens with zero compactions**. The
ceiling is well above 200k. `lfg` pushing heavy work into subagents — each with its own window —
is a real part of why the parent stays manageable. The model pin is still worth having for
reproducibility; the compaction risk was mine, not measured.

## The handoff was rich enough

The gathered findings docs were consumed as intended: the Run read
`docs/research/0004-verify-entrypoint.md` and `docs/prototype/0008-…` directly before planning,
and the plan cites them rather than re-deriving them. It also used `0008`'s run-loop measurements
to justify `bundle.targets`, and `0008`'s `SecurityAgent` finding to explain why the signing cert
must not be touched during an unattended run.

The plan (`docs/plans/2026-08-02-001-…`, 592 lines, 16 requirements, 10 KTDs, 8 units) passed
`lfg`'s step-1 gate cleanly and stated *"Product Contract preservation: unchanged. No scope was
added, removed, or reinterpreted"* — slice 1a's transcription character survived planning.

All seven `just verify` steps survived, several hardened rather than weakened: `tsc --noEmit`
twice because one invocation silently misses `vite.config.ts`; `plutil -lint` on `Info.plist` as
the first line of the build assertion; `vitest run --passWithNoTests` so an empty suite passes
honestly rather than vacuously.

## Planning found a hazard by experiment, not by reasoning

A `Plan` subagent replicated the worktree layout in `/tmp` and ran the real `create-tauri-app`
against it, because **a worktree's `.git` is a gitlink file, not a directory**, and the Job
scaffolds into a non-empty worktree root. It found that `--force` does not merge — it empties
the target directory, deletes the gitlink, and marks the worktree `prunable`.

Result was KTD1: scaffold into a scratch dir, then `rsync -a --exclude=.git --exclude=.gitignore`
— never `--force`. This is the class of failure that would have killed the Run silently.

## The anchor artifact wants CE's own shape

`ce-plan` classified the anchor as a *"legacy-shape requirements artifact"* — not
`ce-unified-plan/v1`. Harmless, since the step-1 gate applies to the plan `ce-plan` writes, not
its input. But it is a direct answer to the snapper map's open item *"What a Snapper Job's Anchor
artifact looks like — shape emerges from the first slice."* It emerged: CE would rather be handed
its own artifact contract.

## MCP resolves inside a headless Run

The research subagent used Exa; `mcp-exa-web_search_advanced_exa-*.txt` tool results are on disk.
Relevant to `BRAINSTORM.md` § Still open #3's general question about what the headless environment
can reach.

## `--disallowedTools` is a dependable constraint

Settles the first of § Still open #3. Verified 2026-08-02: denials **are** inherited by
subagents, and `bypassPermissions` does **not** override them. Denials surface in
`permission_denials` in the result JSON even when they occur inside a subagent. This is why the
supervisor's deny list is a constraint rather than a request. Zero denials fired this run.

## Watch item 1 — confirmed, and the ledger covers it

No `ce-compound` learning was filed, exactly as predicted: `lfg` does not reference it anywhere.
But `docs/ledger/` caught the learning instead, which suggests **the ledger convention is the
answer for headless** rather than something needing a Grind-side fix. Nothing to build here yet;
revisit if a Run ever produces a learning too general for a repo's own ledger.

## Watch item 2 — answered, and well

The Job created the ledger it was supposed to write into, and then wrote into it. The entry is in
`#6`'s exact four-line format:

> **Hit while:** copying a `create-tauri-app` scaffold from a scratch dir into the repo root with
> `rsync -a --exclude='.git' --exclude='.gitignore'`.
> **Symptom:** `src-tauri/.gitignore` was missing afterwards. The root `.gitignore` was correctly
> preserved, *which made it look like the exclude had worked.*
> **Cause:** an rsync exclude pattern containing no slash matches the basename at **every** depth.
> **Do instead:** anchor it — `--exclude='/.gitignore'`.

A real bug, found by using it, with a root fix rather than a workaround, and a verification
recipe attached. The Run's own summary: *"The Run turned out to be the ledger's first real user,
as the anchor predicted."*

## Deviation: the PR is not a draft

`BRAINSTORM.md` § Surfaces says "Draft PR". `lfg` opened a normal PR (`isDraft: false`). Either
the surfaces table should say "PR", or the Job needs to ask for a draft explicitly — `lfg` has no
opinion Grind can rely on here. Cosmetic, but it is a design-record inaccuracy of the same kind
as the re-entry one, and those are the ones worth fixing on sight.

## Wanted: a way to check on a Run without asking an agent

Surfaced by use. Over one Run the author asked for a status update six times, each costing a
session turn, and each answered by a script reading run state plus the session transcript.

**This is not the cockpit `STRATEGY.md` cut.** That was a *cross-run* digest with a "needs you"
section, and its re-entry condition — *"if a week of runs shows it is wanted"* — has not been met
and should not be treated as met. What is wanted here is **live progress on one in-flight Run**,
which the record already holds.

Cheapest form: fold what the session's throwaway `peek.sh` did into `grind status` — furthest
stage, artifacts, context size, process liveness, transcript tail — so `watch -n 30 grind status`
is the dashboard and nothing new is built. Now designable against a complete record.
