---
date: 2026-08-28
run: 20260828-212910-grind-187
paths: [docs/findings/0008-composite-profile-spike.md]
statement: A provenance table's Source cell must attribute each column to the path that actually computes it — run.json stages[] carries the figures (model, turns, cost) but no Class field, so Class provenance belongs to the routing code, not to the row that happens to sit beside the numbers.
status: candidate
---

Review flagged the spike doc's per-stage table for labeling every column —
Class included — as sourced from `run.json stages[]`. Validate walked the
struct rather than restating the claim: `StageEntry`
(`src/rung.rs:157-170`) declares name, session_id, status, artifact_paths,
model, cost_usd, turns and backend — no class field under any spelling — so
no Class value can be read from a stages[] entry. The values actually come
from `resolve_stage_model` routing: Plan is hardcoded
`ModelClass::Strong` (`src/supervisor.rs:1606-1607`); every later stage's
class comes from Diff-triage's decision, else Triage's, else the `strong`
fallback (`src/supervisor.rs:1614-1617`). The fix split each Source cell
into "figures" (from the entry) and "class" (from its actual routing path),
matching what the doc's own next paragraph already said. The lesson
generalizes: a record row and the derivation that produced one of its
columns are different provenance, and a Source cell that merges them
asserts a field the schema does not have.
