---
name: plan
description: The first rung of Grit's ladder. Writes the anchor-plan from the Job's Anchor artifact — steps with stable requirement IDs, the files-touched forecast, the declared base branch and verify entrypoint, readiness frontmatter, and a falsifiable done predicate. Dispatched by the supervisor as the Plan stage; never invoked directly.
---

# Plan

Author the plan Plan review's checklist will grade. Everything this stage needs — the Job issue, its
template rows, and any lessons or target-repo notes injected at composition time — arrives already
in the dispatch prompt. **Never go hunting for it**: no separate fetch of the issue, no re-deriving
a row the prompt already states.

## The return

The dispatch prompt names the Run's stages directory (`<stages-dir>`). On finishing, write
`<stages-dir>/plan.return.json` containing **exactly**:

```json
{"status": "complete"}
```

or `{"status": "incomplete"}` when the stage could not finish. The parser is strict serde with
`deny_unknown_fields`: any key beyond `status` makes the return unparseable and the stage does not
count complete, no matter what the artifacts on disk say. Every other thing this stage produces is
an artifact file, never a return key.

## Artifacts

Both under `<stages-dir>/plan/`:

- **`anchor-plan.md`** — the plan itself.
- **`plan-facts.json`** — the facts Triage sizes the rest of the walk from.

### `anchor-plan.md`

Written from the Anchor artifact: the Job issue, its template rows, and whatever lessons or
target-repo notes the dispatch prompt injected. Contains:

- **Steps with stable requirement IDs** — assigned once, never renumbered as the plan is revised;
  Plan review and Work cite them.
- **Files-touched forecast** — repo-relative paths, each with an existing parent at the Handoff
  SHA.
- **Declared base branch** and **verify entrypoint invocation** — copied verbatim from the Job's
  `Base branch` and `Verify entrypoint` rows. Never invented, never re-derived: the Job already
  states both.
- **Readiness frontmatter** — `readiness: implementation-ready` only when every step, path and the
  done predicate are actually stated well enough for Work to start from the plan alone. Any other
  value (or a progress-like one — `active`, `in_progress`, `done`) means the checklist below fails
  and the stage is `incomplete`, never a plan half-marked ready to make progress look further along
  than it is.
- **A falsifiable done predicate per feature-bearing step**, refined from the Job's own `Done
  predicate` row into something a machine could grade: *`just verify` is green and the new endpoint
  returns 404 for an unknown id* is gradable; *the feature works well* is not.
- **Test-file paths** for every feature-bearing step — the path Work's evidence strategy will use,
  named now rather than discovered mid-Work.

## The checklist this plan is graded by

Plan review runs this checklist first, before any lens fires — the same six items its own
skill states, so the two halves are a contract: change either and check the other. It checks:

1. The plan file exists.
2. `readiness:` parses and equals `implementation-ready`.
3. The done predicate is present and stated so a machine could grade it.
4. Every referenced path is repo-relative with an existing parent at the Handoff SHA.
5. Every feature-bearing step names test-file paths.
6. The declared base branch is present and the Handoff SHA sits on it.

Write toward this list directly — it is the actual bar, not a description of taste.

### `plan-facts.json`

Matches `src/decide.rs`'s `PlanFacts` **byte-for-key** — `deny_unknown_fields`, so no key beyond
these four:

```json
{
  "step_count": 0,
  "forecast_paths": ["path/one.rs"],
  "new_module_count": 0,
  "declared_hot_paths": ["path/two.rs"]
}
```

- `step_count` — the number of steps in the plan just written.
- `forecast_paths` — the files-touched forecast, same list as the plan.
- `new_module_count` — how many of those forecast paths are new modules, not edits to existing
  ones.
- `declared_hot_paths` — copied verbatim from the Job's optional `Declared hot paths` row. This
  stage never classifies a path as hot on its own; the row is human-declared or absent (ADR-0012).

## Descriptive language only

This plan's steps, notes and any open question describe what the work is and what it will do —
never a grade of how good the work is (ADR-0003/0006). Nothing written here may instruct
withholding the PR or stopping the Run on the strength of a finding; that judgment belongs to
Plan review and, downstream of Grind entirely, to the human.

---

*Author the plan Plan review's checklist will grade; state done so a machine could grade it.*
