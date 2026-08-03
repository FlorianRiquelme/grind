# Grind

A queue, a supervisor and a record around headless `lfg` runs. It executes plans the human
is not present for, and stops at an open PR.

## Read before working here

- **`CONTEXT.md`** — the glossary. Job, Enqueue, Dispatch, Run, Handoff SHA, Anchor
  artifact, Handback and the rest are defined terms with explicit `_Avoid_` lists. Use them;
  don't drift to the synonyms they rule out.
- **`docs/adr/`** — three accepted decisions that constrain almost every change here.
- **`STRATEGY.md`** — the target problem and the four metrics a change should serve.
- **`docs/findings/`** — what actual Runs measured. `0001-first-run.md` is the only real
  data the metrics have; it also corrects two things `BRAINSTORM.md` got wrong.
- **`BRAINSTORM.md`** — the design record. Historical, and wrong in the places
  `docs/findings/0001` says it is.

## Shape

`bin/grind` is a single Python 3 script, stdlib only — no dependencies, no package manager,
no build step. It is a script rather than an agent on purpose: a resilience layer built from
the thing that gets rate-limited loses its state exactly when that matters. Keep it that way.

```
grind run <issue>       dispatch a Job now (issue number or URL)
grind resume <run-id>   re-enter a Run that died
grind status [run-id]   print run state (latest if omitted)
grind list              list known Runs
```

## Verify entrypoint

```
python3 tests/test_grind.py
```

Exits non-zero on failure and prints per-check lines. Pure functions only — no network, no
`claude` invocations, no pytest. `tests/test_grind.py` guards the logic whose silent failure
would be expensive: mistaking a rate limit for a crash, missing a rate limit, and failing to
notice that a step of the target repo's `just verify` was trimmed until it went green. New
tests belong there when a change carries a safety property, not for coverage's sake.

## Constraints that are easy to violate

- **`.grind/` is Run state and is never committed.** Gitignored deliberately — it is the
  supervisor's own working record, not history.
- **The supervisor is the only writer of `run.json`.** A read path that saves what it loaded
  can erase `attempts[]`, which nothing can rebuild — and it erases it while the human is
  watching the dashboard to be reassured. Status and the roster observe fresh and persist
  nothing (issues #12, #27).
- **Grind never gates** (ADR-0003). Verdict language describes what happened, never quality.
  A completed Run means the pipeline finished, not that the code is good. Never add
  something that blocks a PR from existing on the strength of a finding.
- **Grind is a scheduler, not a pipeline** (ADR-0001). Everything between plan and open PR
  belongs to `lfg`. Don't reimplement stages it already runs.
- **The plugin version is pinned per Job** (ADR-0001, ADR-0002). Advancing that pin is the
  act of promotion; it is reviewable and revertible. Never resolve "latest" at dispatch.
- **Headless deliberately lags local** (ADR-0002). New capabilities get proven in supervised
  sessions first. Grind is not where we experiment.
- **`DENIED_TOOLS` in `bin/grind` is a safety property.** A Run must never merge its own PR,
  force-push, hard-reset, rebase, or delete a branch. Denials are inherited by subagents and
  survive `bypassPermissions`. Don't loosen the list to make a Run go through.
- **`VERIFY_CONTRACT` is recorded and surfaced, never enforced** — same reason as ADR-0003.

## Agent skills

### Issue tracker

Issues live as GitHub issues in `FlorianRiquelme/grind`, driven via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical triage roles, using their default label strings. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `CONTEXT.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.
