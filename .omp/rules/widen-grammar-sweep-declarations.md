---
description: "Changing a transcript/file-naming/layout grammar in runner/native/serve/page/attempt — sweep every mention and amend the declaring ADR in-branch."
globs:
  - "src/runner.rs"
  - "src/native.rs"
  - "src/serve.rs"
  - "src/page.rs"
  - "src/attempt.rs"
condition:
  - "messages-N|one file per attempt|file-naming|file naming|naming grammar|ADR-0017"
interruptMode: never
---

# Widened grammar: sweep every mention, amend its declaring ADR same-branch

If this edit widens or renames a transcript / file-naming / layout grammar: grep the OLD shape across *.rs doc comments AND docs/adr/ — every mention, not just cited hunks — and amend the declaring ADR section in the SAME branch (ADR-0017 documents messages-N-{K}.jsonl).

Proof of cost: two same-day run-156 ledger entries record Review → Validate → Fixes paying an entire round because src/runner.rs kept "one file per attempt" after the grammar widened elsewhere. Validate is hunk-scoped: out-of-scope remnants survive it; only a pre-work sweep does not.

Sources: docs/ledger/2026-08-26-run-156-* entries; ADR-0017.
