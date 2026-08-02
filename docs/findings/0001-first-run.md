# Findings from the first Run

Run `20260802-105828-snapper-21` against [snapper#21](https://github.com/FlorianRiquelme/snapper/issues/21),
slice 1a. Dispatched by hand 2026-08-02 10:58 CEST. **Living document — the Run is still in
flight.**

This is the run `BRAINSTORM.md` § Next step asks for. What follows is what it answered that no
further design would have.

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

Three attempts, all ending identically:

| Attempt | Turns | Wall clock | Cost | Ended with |
|---|---|---|---|---|
| 1 dispatch | 36 | 5h39m | $23.51 | `API Error: Connection closed mid-response` |
| 2 resume | 3 | 15m | $2.35 | same |
| 3 resume | 1 | 15m | $0.12 | same |

The cause was the laptop sleeping (`pmset -g log` confirms repeated sleep/wake cycles). **The
supervisor re-entered each time and work continued each time** — the resilience layer doing the
one job it exists for, against a real failure rather than a synthetic one.

The important detail: a dropped connection presents as `subtype: success`, `is_error: false`,
`stop_reason: stop_sequence`. It is **not** an error. The rate-limit branch correctly declined
to match it; the generic *no DONE promise → re-enter* path is what caught it, and that path
turned out to be the load-bearing one. Limit handling was designed for first; connection loss
is what actually happened.

**Consequence:** an unattended Run on a laptop dies on every sleep. Re-entry is not a nicety
here, it is the difference between a Run existing and not.

## Two pins were missing

- **The model was not pinned.** A Job pins its plugin version so Runs stay comparable
  (ADR-0002); the model was left to whatever the dispatching shell defaulted to. Fixed: a Job
  carries an optional `Model` row, `GRIND_MODEL` overrides, recorded in run state.
- **`claude` on PATH resolves to cmux's wrapper shim**, which injects that host's session hooks
  into the Run, so a Run silently depended on the terminal it was dispatched from. Fixed: the
  supervisor resolves the real binary.

## A context-ceiling worry that did not materialise

The whole pipeline shares one context window — every stage is a nested Skill call in the same
session. At 13 minutes the parent was at ~124k tokens and compaction looked near-certain
against an assumed 200k ceiling.

**The assumption was wrong.** The parent passed **~270,000 tokens with zero compactions**. The
ceiling is well above 200k. `lfg` pushing heavy work into subagents — each with its own window —
is a real part of why the parent stays manageable.

## The handoff was rich enough

The gathered findings docs were consumed as intended: the Run read
`docs/research/0004-verify-entrypoint.md` and `docs/prototype/0008-…` directly before planning,
and the plan cites them rather than re-deriving them.

The plan (`docs/plans/2026-08-02-001-…`, 592 lines, 16 requirements, 10 KTDs, 8 units) passed
`lfg`'s step-1 gate cleanly and stated *"Product Contract preservation: unchanged. No scope was
added, removed, or reinterpreted"* — slice 1a's transcription character survived planning.

All seven `just verify` steps survived, several hardened rather than weakened: `tsc --noEmit`
twice because one invocation silently misses `vite.config.ts`; `plutil -lint` on `Info.plist`
as the first line of the build assertion; `vitest run --passWithNoTests` plus one real smoke
test so an empty suite passes honestly rather than vacuously.

## Planning found a hazard by experiment, not by reasoning

A `Plan` subagent replicated the worktree layout in `/tmp` and ran the real
`create-tauri-app` against it, because **a worktree's `.git` is a gitlink file, not a
directory**, and the Job scaffolds into a non-empty worktree root. Result was KTD1: scaffold
into a scratch dir, then `rsync -a --exclude=.git --exclude=.gitignore` — never
`create-tauri-app --force`. In its own words, the two excludes are "the whole safety property".

This is the class of failure that would have killed the Run silently.

## The anchor artifact wants CE's own shape

`ce-plan` classified the anchor as a *"legacy-shape requirements artifact"* — not
`ce-unified-plan/v1`. Harmless, since the step-1 gate applies to the plan `ce-plan` writes, not
its input. But it is a direct answer to the snapper map's open item *"What a Snapper Job's
Anchor artifact looks like — shape emerges from the first slice."* It emerged: CE would rather
be handed its own artifact contract.

## MCP resolves inside a headless Run

The research subagent used Exa; `mcp-exa-web_search_advanced_exa-*.txt` tool results are on
disk. Relevant to `BRAINSTORM.md` § Still open #3's general question about what the headless
environment can reach.

## `--disallowedTools` is a dependable constraint

Settles the first of § Still open #3. Verified 2026-08-02: denials **are** inherited by
subagents, and `bypassPermissions` does **not** override them. Denials surface in
`permission_denials` in the result JSON even when they occur inside a subagent. This is why the
supervisor's deny list is a constraint rather than a request.

## Open — the ledger is still empty

`docs/ledger/` has no entries, despite the Run visibly tripping over
`eslint-plugin-react-hooks` v7 having moved its flat config (it introspected the package with
`node -e` to find the real export shape). That is textbook ledger material.

The silence is explainable rather than damning: the ledger convention and its `CLAUDE.md`
write-trigger rule are **deliverables of this very Job**, so the rule does not exist in the repo
yet for the Run to obey. The real test is whether an entry appears after deliverable 5 lands.
Watch item 2 from the Job is not yet answered.

## Wanted: a way to check on a Run without asking an agent

Surfaced by use. Over one Run the author asked for a status update five times, each costing a
session turn, and each answered by a script reading run state plus the session transcript.

**This is not the cockpit `STRATEGY.md` cut.** That was a *cross-run* digest with a "needs you"
section, and its re-entry condition — *"if a week of runs shows it is wanted"* — has not been
met and should not be treated as met. What is wanted here is **live progress on one in-flight
Run**, which the record already holds.

Cheapest form: fold what the session's throwaway `peek.sh` did into `grind status` — furthest
stage, artifacts, context size, process liveness, transcript tail — so `watch -n 30 grind
status` is the dashboard and nothing new is built. Deferred until this Run finishes, so it is
designed against a complete record rather than a partial one.
