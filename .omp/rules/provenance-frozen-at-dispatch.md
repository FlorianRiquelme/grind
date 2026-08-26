---
description: "Fires near provenance()/skills_hash() call sites in the supervisor — the dispatch-time freeze of binary version and stage-skill hash."
globs:
  - "src/supervisor.rs"
condition:
  - "provenance\\(|skills_hash\\("
interruptMode: never
---

# Provenance freezes at dispatch

binary_version and skills_hash resolve ONCE, in the fresh-Dispatch RunRecord construction (supervisor::provenance(), single call site). Every Attempt, every --resume and every stage prompt reads the RECORDED values; the pair moves only via a fresh Dispatch reading whatever is installed at that instant.

NEVER call provenance()/skills_hash() again inside the Run loop or refresh on resume: an hours-long, multi-attempt Run silently changes what executes mid-flight. Reading skill TEXT via skills_root() per stage stays legitimate — the freeze covers the hash/version pair, not the bytes.

Source: AGENTS.md provenance constraint; ADR-0002 (fourth amendment), ADR-0015; #42/#50/#69/#98.
