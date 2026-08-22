---
name: run-ship
description: Grit's final rung — commits, pushes to the Job's declared branch, opens the PR against the declared base, and renders the Run's decision trail into the body. Then Mode::CiBabysit (see skills/run/babysit/SKILL.md) reacts to whatever CI says.
---

# Ship

Everything upstream of here produced a diff and a paper trail. Ship's job is to land the diff and
make the trail legible — never to judge it.

## The return

Write `<stages-dir>/ship.return.json` containing exactly `{"status": "complete"}` or
`{"status": "incomplete"}` — no other key. The PR itself, the ledger commit, and any working
notes are artifacts under `<stages-dir>/ship/`, never fields on the return.

## Commit

Name the files. Never `git add -A` and never `git add .` — the working tree may hold things this
Run never touched (a colleague's uncommitted work, a generated file), and a blind add rides them
into the commit. Stage exactly the paths this Run's diff produced, then commit against that same
list.

## Push and open, against the Job's own rows

Push to the branch the Job already named — `job.branch` — never a branch this session invents.
Open the PR against the branch the Job declared as its merge target — `job.base_branch` — never
whatever the repo's default happens to be. Both rows exist on the Job precisely so this step never
has to guess (ADR-0015). Use `gh pr create` with `--body-file`, not stdin — a body piped through
stdin can silently land empty.

If, after opening, the PR's head or base does not match what the Job declared — the repo moved
its default branch mid-Run, or a push landed somewhere the Job did not name — **describe it and
move on.** A described mismatch is a Record fact; a withheld PR is a gate, and ADR-0003 forbids
gating on any finding, including one about the Run's own shipping step.

## The PR body: an append-only decision trail

Render, in whatever order reads clearest, everything the ladder already decided rather than
re-deciding any of it:

- The **tier Decisions** — read `<stages-dir>/triage/decision.json` and
  `<stages-dir>/diff-triage/decision.json` if present — as rationale rows: signal, value, weight,
  the tier that came out of them. State what was selected, never what the diff is worth
  (ADR-0012 — computation over observable facts, not a grade).
- **Found-vs-applied counts** from `<stages-dir>/fixes/`: how many Confirmed findings existed, how
  many were applied, and the residuals a fix-round exhaustion left behind, plainly, not buried.
- The **done predicate's verdict**, stated descriptively against `job.done_predicate`: what the
  predicate said and what the Run observed against it — never a pass/fail word standing in for
  the observation itself.
- Whatever `job.intent` said the work's nature was, if it said anything.
- `Closes #<issue>` where this PR delivers the whole Job; reference it without the keyword where
  the Job is wider than this diff.

This is prose the human reads, not a schema Grind parses back — nothing here becomes a gate by
being written down.

## Ledger candidates, committed with the PR

Append bounded lesson candidates to the target repo's `docs/ledger/` and commit them in the same
push — lessons travel with the code that produced them, not in a side channel. Each candidate
carries frontmatter:

```yaml
date: <today>
run: <run-id>
paths: [<paths this Run touched that the lesson concerns>]
statement: <one sentence, the lesson itself>
status: candidate
```

`status: candidate` is not optional — a lesson Ship writes has not been reviewed by anyone yet;
promotion out of `candidate` is a human's or Reflect's later act, never this stage's.

## Never

- Never merge the PR, force-push, rebase, hard-reset, or delete a branch. `DENIED_TOOLS`
  (`CLAUDE.md`) refuses every spelling of these at the tool layer regardless of what this prompt
  says — but do not reach for them expecting the barrier to save you; the shape a Run pushes in
  is its own branch, plain push, nothing else.
- Never invent a base or branch this Run prefers over the Job's declared rows.
- Never let a base/head mismatch withhold the PR — describe it in the body instead.
- Never write `status:` anything but `candidate` on a ledger entry from this stage.

*Land the PR the Handback will swear about; carry the lessons with the code.*
