---
name: work
description: The fourth rung of Grit's ladder. Implements exactly the revised plan — unit packets fan out to fresh worker contexts, the orchestrator owns every commit path-limited to its unit's declared files, and the Job's verify entrypoint runs before the stage returns. Dispatched by the supervisor as the Work stage; never invoked directly.
---

# Work

Implement exactly the revised plan; report how behavior got protected. This session is the
orchestrator. It reads the plan's steps as unit packets, fans each one out to a fresh worker
context, and is the only thing in this stage that ever runs `git commit`.

## The return

Write `<stages-dir>/work.return.json` containing **exactly** `{"status": "complete"}` or
`{"status": "incomplete"}`. Strict serde, `deny_unknown_fields` — no other key. Everything else this
stage produces is tree commits or an artifact file, never a return key.

## Artifacts

- **Working-tree commits** — one or more, each path-limited to the unit's own declared files. The
  orchestrator makes every commit; a worker never runs `git commit` itself.
- **`<stages-dir>/work/evidence.json`** — which behaviors got which protection, per unit. This is
  the data VERIFY_CONTRACT reads: recorded and surfaced, never enforced (ADR-0003).
- **`<stages-dir>/work/units/<unit-id>.json`** — one durable return per worker packet, written by
  the worker before it exits. The orchestrator reads these files, never a worker's own summary of
  itself.

## Unit packets and workers

Each step of the revised plan (`<stages-dir>/plan/anchor-plan.md`) is a unit packet: goal, declared
files, the test paths named in the plan, and the requirement ID it satisfies. Dispatch each packet
to a fresh worker context — workers see only their own packet plus what they read from the tree,
never the orchestrator's running context or another unit's transcript.

**Workers never commit.** A worker edits its declared files, runs the checks its evidence strategy
calls for, and returns its own `units/<unit-id>.json` with what it did and what it verified. The
orchestrator inspects the actual diff against the unit's declared files, then makes the commit
itself, path-limited to those files. A worker that touched a file outside its declared set is a
fact the orchestrator surfaces, never something it silently commits.

## Evidence strategy

For every unit that changes behavior, decide and report the evidence strategy **before** changing
production code — proof-first (a failing test exists or is added first) or characterization-first
(existing behavior is captured before it changes). State which one at the time: the choice is
unreconstructable afterward, so a unit that skipped this and reports it post hoc is reporting a
guess, not evidence.

- Existing test already fails for the intended behavior → use it as the red evidence; don't
  duplicate it.
- No test covers the behavior → add the smallest focused failing test, or a characterization test
  when the change isn't new behavior.
- A deliberate no-test exception (a trivial rename, pure config, pure styling) is recorded with its
  reason and whatever replacement verification stands in — never silently skipped.

`evidence.json` maps each behavior touched to its protection, so the next reader (VERIFY_CONTRACT,
Reflect, a human) sees what was actually checked rather than what was claimed.

## Verify entrypoint

Run the Job's declared verify entrypoint before this stage returns `complete`. A red result is
recorded in `evidence.json` — never as an extra key on the return, which stays exactly
`{"status": …}` — and it never silently blocks the return from being written, because Grind
never gates (ADR-0003); it is a fact the Record carries forward to Fixes.

## Idempotent re-entry

On re-entry, read this stage's own `units/` directory and the tree before doing anything else. A
unit with a durable return already on disk and matching commits in the tree is done — do not redo
it. Resume from the earliest unit lacking both.

## Descriptive language only

Every unit return and `evidence.json` entry states what was done and what protects it — never a
grade of how good the work is (ADR-0003/0006). Nothing here may instruct stopping the Run or
withholding a commit on the strength of a finding; that judgment belongs to Review, Validate and,
downstream of Grind, the human.

---

*Implement exactly the revised plan; report how behavior got protected.*
