---
date: 2026-08-26
run: 20260826-055920-grind-156
paths: [src/native.rs, docs/adr/0017-the-agent-backend-is-declared-by-layout-and-snapshotted-at-dispatch.md]
statement: When a fix changes the layout a recorded decision declares — file-naming grammar, directory shape, wire format — amend the ADR section in the same branch; Review's consistency persona will find the divergence, and a record that no longer describes what the code writes is a defect Validate can Confirm.
status: candidate
---

A fix that widens what the code *writes* also widens what the code's accepted record
*declares*. This Run's diff moved the native transcript grammar from `messages-N.jsonl` to
`messages-N-{K}.jsonl` for same-slot retries across writer (`src/native.rs`), server
(`src/serve.rs`) and renderer (`src/page.rs`) — but left ADR-0017's transcript section saying
"one file per attempt". Review's consistency persona flagged it, Validate Confirmed it, Fixes
amended the section as its single manual finding. The lesson is not that ADRs go stale; it is
that the ladder has a dedicated detector for exactly this staleness, so the cheap move is to
sweep the declaring sections of every touched decision before Work starts, not after Review
bills a round for it.
