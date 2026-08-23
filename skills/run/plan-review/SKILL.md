---
name: plan-review
description: The third rung of Grit's ladder. Runs coherence and feasibility lenses (and, at higher tiers, additional reviewer sessions) against the anchor-plan before a single implementation token burns; findings land in findings.json, an optional bounded revision round lands revision.md and a revised plan checkpoint. Dispatched by the supervisor as the PlanReview stage; never invoked directly.
---

# Plan review

Find contradictions and unstartable plans before Work spends. Mandatory on every Run — only its
depth adapts by tier, from one combined-lens pass at T0/T1 up to a cross-model seat at T3. This
stage never touches the target repo's worktree; it reads the plan and the repo and writes only
under `<stages-dir>/plan-review/`.

## The return

Write `<stages-dir>/plan-review.return.json` containing **exactly**:

```json
{"status": "complete", "revised": false}
```

`status` is `"complete"` or `"incomplete"`, same contract as every other stage. `revised` is this
stage's one addition: `true` when the bounded revision round ran and changed the plan checkpoint,
`false` when it did not. The parser is strict serde with `deny_unknown_fields` — no key beyond
these two, ever. All other output is an artifact file.

## Artifacts

Under `<stages-dir>/plan-review/`:

- **`findings.json`** — every finding from every reviewer session that fired, each carrying
  **anchored confidence**: `100` means provable from the plan's own text (quote two passages that
  disagree, or a cited symbol absent from the codebase); confidence below `50` is suppressed
  entirely rather than written. A finding is a description of what the text says or doesn't say,
  never a verdict on the plan's quality.
- **`revision.md`** — present only when a revision round ran: what changed and why, citing the
  finding IDs it addressed.

The revised plan checkpoint itself lands back at `<stages-dir>/plan/anchor-plan.md` — Work reads
that path always, whether or not a revision happened, so it never has to know which version it got.

## The plan checklist, first

Before any lens fires, grade the plan against the six-item checklist plan/SKILL.md tells its
author to write toward — the two skills state the same list, so the halves are a contract:
change either and check the other.

1. The plan file exists.
2. `readiness:` parses and equals `implementation-ready`.
3. The done predicate is present and stated so a machine could grade it.
4. Every referenced path is repo-relative with an existing parent at the Handoff SHA.
5. Every feature-bearing step names test-file paths.
6. The declared base branch is present and the Handoff SHA sits on it.

Each miss is a finding in `findings.json` like any other — described, bucketed, never a gate
(ADR-0003).

## The six checks

None of these is a gate. Each produces findings, never a pass/fail:

1. **Coverage** — a pairing table, every Job requirement against the plan step(s) that address it;
   an unmatched row is a finding, not a failure.
2. **Ground truth** — every symbol and file the plan cites, grepped at the Handoff SHA; a cited
   module that doesn't exist there is a finding.
3. **Verification plan** — each behavior-changing step's stated protection, reported as `present` /
   `missing` lists, never a boolean.
4. **Blast radius** — the plan's forecast paths against the risky-path list, feeding Diff-triage's
   tier floor.
5. **Base discipline** — the declared base branch is correct and the Handoff SHA sits on it.
6. **Ledger fit** — which of the target repo's existing ledger entries this plan applies, cited by
   ID, and where any new lesson this plan implies would land.

## Personas

Two lenses, always: `personas/coherence.md` and `personas/feasibility.md`. Read the full file and
pass it verbatim to the dispatched reviewer session — never paraphrase a persona from memory. At
T2 a second independent reviewer session runs the same two lenses; at T3 a third seat runs on a
second model family.

Findings are bucketed `act-on | consider | noted | dismissed` by a steward session with rationale
for each. Only `act-on` findings feed the revision round.

## The bounded revision round

`PLAN_REVISIONS=1`. When any finding is bucketed `act-on`, one reviser session applies it and
writes `revision.md` plus the revised `anchor-plan.md` checkpoint; `revised` is `true` in the
return. `consider` and `noted` findings survive into the Record as-is — a proceeded-and-flagged
conflict reaches the human's morning read rather than blocking anything now. `dismissed` findings
keep their rationale in `findings.json`.

An invalidating contradiction — a decided constraint with no workable resolution — is a Blocker,
never a workaround and never a revision-round patch job.

## Descriptive language only

Every finding, every bucket and every line of `revision.md` describes what the plan's text says,
not how good the plan is (ADR-0003/0006). Nothing here may instruct withholding the PR or stopping
the Run on a finding — that stays downstream of Grind, at the human's merge decision.

---

*Find contradictions and unstartable plans before Work spends; findings, not failures.*
