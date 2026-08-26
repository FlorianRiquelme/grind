---
description: "Adding/removing/moving files under an indexed directory (docs/adr, docs/findings) or editing counted/ranged prose anywhere."
globs:
  - "docs/adr/**"
  - "docs/findings/**"
condition:
  - "(?i)\\b(eighteen|seventeen|nineteen|twenty)\\b"
  - "(?i)(rung|stage)s?\\b.*\\b(list|map|count)"
  - "docs/(adr|findings)/"
interruptMode: never
---

# Index lines follow their directories

README.md carries prose that counts or ranges directories ("eighteen accepted decisions" over docs/adr/, the findings range `0001`–`NNNN`). When this change adds, removes or moves a file there, correct EVERY index line naming that directory in the SAME change — including sibling lines and neighboring bullets, not only the touched one.

Proof of cost: Job #138 existed solely because an ADR count lagged three behind docs/adr/ (docs/findings/0005); ledger entry 2026-08-25-grind-138-index-lines-follow-their-directories.md names the pattern.
