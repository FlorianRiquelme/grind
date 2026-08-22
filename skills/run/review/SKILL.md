---
name: review
description: Grit's Review stage — spawns the tier's persona roster against the real diff, merges durable per-persona findings into one file, and returns a strict-serde stage result. Dispatched by the supervisor once Diff-triage has sized the tier; never invoked by a human directly.
---

# Review

One Attempt: a lead session that fans out `[F n ∈ 1..9]` persona sessions, one per roster seat, then
merges what they wrote. The dispatch prompt names `<stages-dir>` — everything this stage reads and
writes lives under it.

Grind computes the roster; this skill never re-derives one. Read `<stages-dir>/diff-triage/decision.json`
(`decide::Decision`) once, at the top: `tier` and `personas` are exactly the fan-out you run. If the
file is missing or fails to parse, that is a Blocker for the supervisor to clear, not a roster this
skill invents from reading the diff itself — Grind computes tier and roster over observable facts and
never classifies (ADR-0012), and a session picking its own panel from vibes is exactly the classifying
this stage must not do.

## 1. Assemble the context-economy bundle, once

Every persona session receives the same three things and nothing else:

- the diff (`git diff <handoff-sha>..HEAD`, or the range the dispatch prompt names)
- the relevant plan units from `<stages-dir>/plan/anchor-plan.md` (the units the diff's touched paths
  map to — pass the whole plan when the mapping is ambiguous, never trim to the point of guessing)
- that persona's own instruction file (`personas/<name>.md`)

**Never the Run transcript. Never another persona's findings file.** This is the contract, not
advice: a persona that has read a sibling's conclusion is no longer an independent seat, and a
persona re-reading the transcript is Run 4's $132.98 shape returning one stage at a time. The one
exception is a second fix round: on a second round, hand each reviewer the prior round's *Confirmed*
findings (from Validate) and instruct it to acknowledge rather than re-raise them — never the full
findings history, never Refuted or Unfounded rows.

## 2. Spawn the roster

One subagent session per persona in `decision.json`'s `personas` list, in the order the nine-persona
library lists them (Correctness, Security, Concurrency, Schema, Surface, Tests, Performance,
Consistency, Docs) — a stable order, never re-sorted by this stage. Each session runs report-only:
Write and Edit denied, and no write-capable Bash form available, so "touches nothing" is a property
of the sandbox and not a promise in the prompt.

Each conditional persona (every one but Correctness and Tests) states in its findings file the
one-line reason it fired: the tier Decision selected it, and the justification restates the
observable signal `decision.json`'s rationale rows already carry (e.g. "fired: risky_path_hits
includes auth" — never a rediscovery of why, since that was already computed and logged upstream).

## 3. Each persona writes its own file before it returns

Before returning, every persona session writes `<stages-dir>/review/<persona>/findings.json` —
durably, on disk, regardless of how much it found. A persona with zero findings still writes the
file with an empty list; a missing file is a dead or misbehaving child, not a clean pass. This is
what closes Run 1's silent 1-of-5 fan-out degradation: the lead observes which files exist against
which personas were spawned, and a dead subagent is respawned under ordinary Wait rules — never
inferred as "nothing to report."

**Finding schema**, one object per finding:

```json
{
  "file": "src/decide.rs",
  "lines": "1157-1163",
  "class": "correctness",
  "claim": "select_tier reads diff.risky_path_hits() before checking required_missing, so a Triage call with plan facts but a stray Some(diff) silently skips the fail-closed branch.",
  "proposed_fix": "guard the risky-path read behind the same required_missing check used for the fail-closed branch",
  "rule_id": "COR-6",
  "severity": "P1",
  "confidence": 75,
  "autofix_class": "manual"
}
```

- `rule_id` cites the exact checklist entry this finding is grounded in (`COR-1`..`COR-8`,
  `SEC-1`..`SEC-6`, and so on per persona file below). A finding with no `rule_id` is not a finding
  from this stage — citation tallies fall out of the findings files for free, and an uncited claim
  breaks that for nothing.
- `severity` is `P0`–`P3`. `confidence` is an anchored integer 0–100: **100 means provable from the
  diff text alone**, no interpretation; anything below **50 is suppressed and never written** —
  suppression happens in the persona session, not later, so a findings file never carries noise the
  lead has to filter.
- `autofix_class` is `gated_auto | manual | advisory` — a fact about how mechanically the fix could
  be applied, not a recommendation to apply it. Nothing here decides whether Fixes acts on it; that
  decision belongs to Validate and Fixes, downstream.

Findings describe what the diff does, in the cited lines, never whether the work is acceptable.
"This introduces an unbounded loop over `runs()`" is a finding; "this is bad code" is not one.

## 4. Merge, and count — never grade

Once every persona has returned (or a respawn has been exhausted under Wait rules), the lead reads
every `<stages-dir>/review/<persona>/findings.json` that exists and writes
`<stages-dir>/review/review.findings.json`: the concatenated findings, plus `spawned` and `returned`
as counts — facts about processes, exactly like `decision.json`'s roster, never folded into a health
summary (ADR-0006's sixth prohibited shape is precisely a boolean over this pair). A tier that fully
fired and returned nothing Confirmable is not a finding for this stage to make; that reading belongs
to whoever consumes `spawned`/`returned` downstream, never to the merge itself.

The merge touches nothing in the worktree. It is a read of N files and a write of one.

## The return

`<stages-dir>/review.return.json`, strict serde, `deny_unknown_fields`, exactly:

```json
{ "status": "complete" }
```

or `{ "status": "incomplete" }`. Nothing else belongs in this file — the merged findings, the
spawned/returned counts, and every persona's justification live in the artifact files above. The
supervisor observes this file's existence and shape, never a claim inside a transcript, to decide
the stage is done.

## Never

- **Re-pick the roster.** The tier Decision already computed it; this stage spawns it.
- **Let a persona see another persona's findings**, or the Run transcript. Context economy is the
  contract, not a suggestion this stage may relax under time pressure.
- **Fold `spawned`/`returned` into a boolean**, or write anything that reads as a verdict on the
  work. A finding describes the diff. It never recommends blocking, withholding, or "not shipping"
  anything — the gate stays downstream at human merge (ADR-0003), and this stage has no opinion
  about it.
- **Touch the worktree.** Report-only is a sandbox property here, not a prompt-level promise this
  skill is trusted to keep on its own.

Report what the diff breaks, cite your checklist ID, touch nothing.
