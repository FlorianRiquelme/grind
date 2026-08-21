---
title: "Amend #76's named Run 2 replay test to the claim it supports - Plan"
type: fix
date: 2026-08-21
origin: docs/plans/2026-08-21-001-amend-leg-1-named-test.md
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: github-issue
execution: code
---

# Amend #76's named Run 2 replay test to the claim it supports - Plan

Enactment of the ruling on [#78](https://github.com/FlorianRiquelme/grind/issues/78), scheduled
as Job [#80](https://github.com/FlorianRiquelme/grind/issues/80). Leg 1 already shipped the
honest substitute test; this plan corrects the two spec surfaces that still carry the claim the
evidence cannot support. No source change.

## Summary

Issue #76's named-test list asserts *"Replaying Run 2's attempt shapes reaches a Blocker at two
working Attempts rather than exhaustion at eight."* That claim cannot hold: `docs/findings/0002`
records Run 2's **single** denial on **attempt 8** — the Attempt that opened the PR — so no
denial-keyed predicate fires at two working Attempts. Leg 1 shipped the substitute
(`replaying_run_2s_eight_attempt_shapes_leaves_five_waits_and_three_working_attempts`,
`src/policy.rs`), and #78's ruling settled both open questions: **amend** the named test to the
Wait-arithmetic claim, and **no second, non-denial Blocker trigger now** — that stays fog on
[#5](https://github.com/FlorianRiquelme/grind/issues/5) with Run 3 as the instrument. This plan
lands the correction on issue #76 itself and records both rulings in the leg-1 plan's deferred
Open Question.

## Requirements

Traced from the origin document (its R-IDs kept):

- **R1** — #76's named-test list no longer claims the Blocker-at-two-working-Attempts test. It
  carries the Wait-arithmetic claim: replaying the eight attempt shapes leaves **five Waits and
  three working Attempts**, does not reach exhaustion on the attempt count, and **does not reach
  a Blocker**. Evidence cited: the single attempt-8 denial in `docs/findings/0002`.
- **R2** — The correction lives on issue #76 itself — a body edit or a correcting comment on the
  named-test list — referencing #78's ruling.
- **R3** — `docs/plans/2026-08-15-001-feat-leg-1-map-rulings-plan.md`'s deferred Open Question
  (*#76's named Run 2 replay test does not survive its own evidence*) records both rulings: Q1
  amended and enacted by this branch; Q2 — a second, non-denial Blocker trigger — stays fog on
  #5, with Run 3 as the instrument.
- **R4** — `just verify` green, and one open PR against `main` carrying the plan edit and the
  Run's own narrative.

## Key Technical Decisions

- KTD1. **The #76 correction is a body edit of the one named-test bullet, plus one short
  correcting comment.** (session-settled: user-directed — chosen over keeping the original
  Blocker-at-two claim: `docs/findings/0002` shows the only denial fired on attempt 8, so the
  claim cannot hold.) R2 allows either surface; the body edit is what makes R1's *no longer
  claims* literally true — a comment alone leaves the list still asserting the dead claim — and
  the comment is what keeps an edit discoverable without relying on GitHub's edit history. The
  edited bullet carries the reference to #78's ruling inline; the comment names #78 and #80.
  Governs R1, R2.
- KTD2. **Q2 stays fog; nothing here designs a second Blocker trigger.** (session-settled:
  user-directed — chosen over designing a non-denial trigger in this Job: ruled fog by #78,
  needs Run 3 data.) The leg-1 plan edit records the ruling and points at #5; no other surface
  changes. Governs R3.
- KTD3. **No source change, no new tests.** (session-settled: user-directed — chosen over
  adjusting code or tests alongside the doc correction: leg 1 already shipped the honest
  substitute, so this Job is spec-surface correction only.) The twenty-two-test count in the
  leg-1 plan's Definition of Done already reflects the substitution and does not move. Governs
  R1, R3, R4.
- KTD4. **The leg-1 plan's Open Question entry is amended in place, not deleted.** The entry is
  the record of *why* the named test could not hold; erasing it would orphan KTD6's "See Open
  Questions" pointer at that plan's line 239 and the #78 issue body's citation of it. The edit
  replaces the *"What it does not do"* paragraph — the part that said both questions were still
  the spec's to settle — with the two rulings, and leaves the evidence paragraphs intact.
  Governs R3.

## Scope Boundaries

- **No source change** — `src/`, `tests/`, `skills/` untouched (origin non-goals).
- **No second Blocker trigger** — stays fog on #5, Run 3 the instrument (KTD2).
- **No test-count movement** — the leg-1 DoD's twenty-two named tests already reflect the
  substitution.
- **The PR tail** (branch, commit, push, PR) is owned by the pipeline, not by a unit here.

## Implementation Units

### U1. Record both rulings in the leg-1 plan's Open Question

- **Goal:** The deferred Open Question stops reading as undecided and records what #78 ruled.
- **Requirements:** R3
- **Dependencies:** —
- **Files:** `docs/plans/2026-08-15-001-feat-leg-1-map-rulings-plan.md`
- **Approach:**
  1. In the first `Open Questions` entry (the named Run 2 replay test), keep the evidence
     statement and the *"What this plan does"* paragraph unchanged.
  2. Replace the *"What it does not do"* paragraph with a **Ruled** paragraph recording both
     rulings from [#78](https://github.com/FlorianRiquelme/grind/issues/78): Q1 — the named test
     is **amended** to the Wait-arithmetic claim, enacted by
     [#80](https://github.com/FlorianRiquelme/grind/issues/80) on this branch; Q2 — a second,
     non-denial Blocker trigger stays fog on
     [#5](https://github.com/FlorianRiquelme/grind/issues/5), with Run 3 as the instrument.
  3. Keep the entry's *Deferred (non-blocking)* framing coherent with its new resolved state —
     the heading line may note the ruling, but the entry stays in place (KTD4).
  4. Idempotent across Attempts: if the entry already records both rulings (a prior Attempt
     landed the edit), treat this unit as done rather than re-applying it.
- **Patterns to follow:** the entry's own register — evidence first, decision second, links as
  `[#N](url)`.
- **Test scenarios:** Test expectation: none — docs-only edit; `just verify` guards nothing
  here beyond the repo staying green.
- **Verification:** The entry names both rulings, #78, #80 and #5; KTD6's *See Open Questions*
  pointer still resolves to a coherent entry.

### U2. Amend #76's named-test list and leave the correcting trail

- **Goal:** #76's named-test list carries the claim its own evidence supports.
- **Requirements:** R1, R2
- **Dependencies:** U1 (the repo record lands before the outward-facing edit cites it)
- **Files:** none — GitHub issue #76, written via `gh` (body edit + one comment)
- **Approach:**
  1. In #76's body, under `### The named test list`, replace the single bullet *"Replaying
     Run 2's attempt shapes reaches a Blocker at two working Attempts rather than exhaustion at
     eight."* with the Wait-arithmetic claim: replaying Run 2's eight attempt shapes leaves five
     Waits and three working Attempts, does not reach exhaustion on the attempt count, and does
     not reach a Blocker — with an inline parenthetical citing #78's ruling and the attempt-8
     single denial in `docs/findings/0002`. Every other byte of the body stays identical.
  2. Post one short comment on #76 stating the named-test list was amended per #78's ruling,
     enacted by #80 — the append-trail that makes the body edit discoverable.
  3. No label, assignee, project or milestone is touched — comments and a body edit only.
  4. Both writes are idempotent across Attempts: skip the body edit when the bullet already
     carries the Wait-arithmetic claim, and skip the comment when a correcting comment naming
     #78 and #80 already exists on #76.
- **Patterns to follow:** #78's ruling comment supplies the amended claim's wording almost
  verbatim; reuse it rather than paraphrasing.
- **Test scenarios:** Test expectation: none — external tracker edit; verified by reading the
  issue back.
- **Verification:** `gh issue view 76` shows the amended bullet and the correcting comment; the
  old claim appears nowhere in the current body.

## Verification Contract

- `just verify` green (R4) — unchanged in meaning; this plan adds no test and removes none.
- Issue #76's current body no longer contains *"reaches a Blocker at two working Attempts"*.
- The leg-1 plan's Open Question entry records both rulings.

## Definition of Done

- U1 and U2 landed; `just verify` green.
- One open PR against `main` carrying the plan edit and the Run's narrative, closing #80.

## Sources & Research

- [#78](https://github.com/FlorianRiquelme/grind/issues/78) — the ruling this plan enacts; its
  owner comment carries both rulings and the amended claim's wording.
- [#76](https://github.com/FlorianRiquelme/grind/issues/76) `### The named test list` — the
  bullet being amended.
- `docs/findings/0002-second-run.md` — the evidence: one denial, on attempt 8
  (`git push --force-with-lease`), attempts 3–7 at $0 and one turn.
- `src/policy.rs` —
  `replaying_run_2s_eight_attempt_shapes_leaves_five_waits_and_three_working_attempts`, the
  shipped substitute the amended claim describes.
- `docs/plans/2026-08-15-001-feat-leg-1-map-rulings-plan.md` — Open Questions (the entry U1
  amends), Assumptions (the Run 2 replay-input facts), Definition of Done (the twenty-two-test
  count that does not move).
