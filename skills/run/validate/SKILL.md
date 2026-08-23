---
name: validate
description: Grit's adversarial validator — rung 8 of the ladder. Dispatched once Review has written its merged findings; attacks every finding before Fixes is allowed to spend on it.
---

# Validate

Absent at T0 (Review itself is one lite reviewer with no fan-out to attack there). Wherever it
runs, its job is narrow: turn a persona's claim into a fact someone can check.

## The return

Your stages directory is named in the dispatch prompt. Write `<stages-dir>/validate.return.json`
containing exactly `{"status": "complete"}` — strict serde, deny-unknown-fields, no other key.
Everything else is artifact files under `<stages-dir>/validate/`: the durable finding-by-finding
verdicts belong there, not in the return.

## Inputs, and the one you must not read

Read `<stages-dir>/review/review.findings.json` (the merged set, with the spawned/returned
fan-out counts already folded in) and the per-persona files under `<stages-dir>/review/<persona>/`
for the `file`/`lines` a finding points at. Read the diff itself (`git diff <handoff-sha>..HEAD`,
or the hunk the finding cites) for what actually changed.

**Never read the authoring persona's reasoning.** A finding claims something about the diff; the
diff either bears that out or it does not, and reading the persona's own argument for it is how a
validator ends up re-deriving the same blind spot instead of checking it. Pass the child session
only the finding object and the diff hunk it cites — nothing else.

## One session per finding, deliberately blind

For every finding eligible for validation (skip only what Review itself already demoted — a
single-reviewer P2/P3 sourced solely from Tests or Consistency does not reach this stage; see
below), spawn one fresh subagent session. Its entire input is the finding and the hunk. Its job is
to walk the code the hunk shows and answer one question: does this claim hold.

The bar, and it is exact:

- **Confirmed** — the session cites the specific lines that make the claim true. It walked the
  failure: traced the input, the branch, the effect. A restatement of the finding's own prose is
  not a citation.
- **Refuted** — the session demonstrates the claim failing against the code as it stands: the
  guarded case is in fact guarded, the type already forbids the input, the path is unreachable.
- **Unfounded** — neither. The claim is unproven, not confirmed and not refuted, because the hunk
  alone does not settle it. Mark it Unfounded rather than rounding up to Confirmed — an unproven
  finding that reads as confirmed is worse than one that reads honestly as unproven, because Fixes
  spends on the first and not the second.

Never let a session return anything but one of these three plus its citations. A session that
hedges, or answers a different question than the one it was given, is Unfounded — the burden is on
the citation, not on your patience with the prose.

## After every child returns

1. **Agreement weighting.** When the same underlying claim was raised independently by two or more
   reviewers (not two personas restating the same line), and both readings validate, that is the
   strongest signal Validate can produce — mark it so in the record rather than letting it read
   identically to a single Confirmed.
2. **Demotion.** A weak P2/P3 sourced solely from the Tests or Consistency personas, once Confirmed
   but only barely — no user-facing effect, no violated contract — demotes to a residual-risk note
   rather than riding forward as an actionable finding. This is not a second validator pass; it is
   the same steward judgment ce-code-review applies before Fixes ever sees the queue.
3. **Nothing is dropped.** Refuted and Unfounded rows are not deleted — they ride into
   `findings-validated.json` beside the Confirmed ones, with their citations, because both are the
   Record's narrative for whoever reads it. A Refuted row is also next month's calibration data:
   a validator that refutes true findings tunes its bar; one that never refutes anything is not
   attacking.

Write `<stages-dir>/validate/findings-validated.json`: one row per finding, its verdict, its
citations, whether it was agreement-weighted or demoted, and the finding's own `file`, `lines`,
`severity` and `autofix_class` carried unchanged from Review. `autofix_class` especially: it is
the key Fixes' eligibility logic reads (`gated_auto` applies directly, `advisory` never applies),
so a row that drops it leaves Fixes guessing at a default instead of acting on what Review
recorded.

## Never

- Never apply a fix. This stage reports; Fixes spends.
- Never let a validator session see the persona's stated reasoning, the plan, or the Run's
  transcript — the diff and the finding are the whole of its world.
- Never let an absence of proof read as Confirmed. Unfounded exists so nobody has to round up.
- Never gate anything (ADR-0003). A fully-refuted review still leaves the diff standing and the
  Run walking toward an open PR; what Validate produces is description for Fixes and for the
  human, never a reason to stop.

*Attack every finding blind to its author's reasoning; mark unproven rather than round up.*
