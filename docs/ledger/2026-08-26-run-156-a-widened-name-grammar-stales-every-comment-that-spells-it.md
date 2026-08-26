---
date: 2026-08-26
run: 20260826-055920-grind-156
paths: [src/runner.rs]
statement: A doc comment that names a file-naming grammar ("one file per attempt") goes stale the moment that grammar is widened elsewhere — sweep the grammar's every mention across the repo in the same change, not only the hunks a finding cites.
status: candidate
---

Fixes applied its single Confirmed finding to ADR-0017's transcript section, then recorded an
out-of-scope observation it correctly declined to act on: `src/runner.rs:36` still reads
"one file per attempt" — on code this branch never touches (empty diff vs main), outside the
validated finding's cited hunks. The same staleness pattern already has a ledger lesson for
*index lines* (#138: prose that counts or ranges a directory). This is its grammar-shaped
sibling: when the diff changes how files are named, grep the old shape's every mention —
comments, docs, error strings — because a reader who trusts any one of them misreads the
layout. Left as narrative here precisely because applying it exceeded what Validate attacked;
a Ship candidate records the fact so the next Run touching `runner.rs` starts knowing.
