---
name: simplify
description: The fifth rung of Grit's ladder. Shrinks Work's diff for clarity, reuse and efficiency without moving a structure pin — public API, test names, and behavior stay exactly what they were. Dispatched by the supervisor as the Simplify stage above T0; never invoked directly.
---

# Simplify

Shrink the diff without moving a pin; hand back the smaller diff plus notes. Prioritize readable,
explicit code over compact code — fewer lines is not the goal, a diff a reviewer trusts on sight is.

## The return

Write `<stages-dir>/simplify.return.json` containing **exactly** `{"status": "complete"}` or
`{"status": "incomplete"}`. Strict serde, `deny_unknown_fields` — no other key. The smaller diff
lands in the tree as commits; notes are the one artifact file.

## Artifacts

- The simplified diff, in the tree — commits on top of Work's, never a rewrite of Work's history.
- **`<stages-dir>/simplify/notes.md`** — what was already sound and what improved, by category
  (reuse, quality, efficiency), and what was skipped and why.

## Scope

The diff since Work's Handoff-relative commits — everything Work landed this Run, nothing outside
it. This stage never widens scope to files Work didn't touch.

## Trigger

This stage fires only when Work's diff has **at least 30 substantive changed code lines** — count
human-authored code, not total diff lines. A diff under that floor, or one that is purely mechanical
(formatting, dependency bumps, lint-only fixes, generated artifacts), still receives a dispatch at
tiers above T0, and the correct response is to report nothing to simplify and return `complete`
with an empty `notes.md` rather than manufacture a fix.

## Structure pins stay fixed

Preserve exactly, never move:

- **Public API** — exported signatures, module boundaries, anything another module or caller
  depends on.
- **Test names** — a renamed test is a different test to anyone who greps for it later.
- **Behavior** — outputs, errors, side effects, and ordering. If a fix cannot establish behavior is
  preserved, skip it.

Never simplify away a safety check: trust-boundary validation, data-loss protection, security
checks. Skip any finding that would thin or remove one.

## Verify

Run the Job's declared verify entrypoint after simplifying. A simplification-caused failure is
fixed or reverted — never patched by relaxing an assertion, weakening a type, or skipping a test.

## Skipped at T0 — no skip logic needed here

At T0, Work absorbs this pass and Simplify is never dispatched; the supervisor records the skip as
its own return row on the `Simplify` rung. This skill carries no branch for that case — it is
never invoked at T0 at all, so there is nothing here to special-case.

## Descriptive language only

Notes describe what changed and why — never a grade of how good Work's diff was (ADR-0003/0006).
Nothing here may instruct withholding the diff or stopping the Run on a finding.

---

*Shrink the diff without moving a pin; hand back the smaller diff plus notes.*
