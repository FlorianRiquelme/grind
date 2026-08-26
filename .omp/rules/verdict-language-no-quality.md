---
description: "Fires on edits adding or changing verdict/status enums or ok-shaped gate flags in decide/policy/rung/render/cli."
globs:
  - "src/decide.rs"
  - "src/policy.rs"
  - "src/rung.rs"
  - "src/render.rs"
  - "src/cli.rs"
condition:
  - "enum Verdict|enum ReturnStatus|enum Stop"
  - "if !\\w+\\.ok\\b|ok:\\s*bool"
interruptMode: never
---

# Grind never gates (ADR-0003)

ADR-0006 prohibits `Verdict::{Rejected, Blocked, Failed}` by name, `{ ok: bool }` contract types, and every summary/gate shape (fan-out health summary, base-drift summary). `Blocked` lives only in `policy::Stop` and supervisor state — a fact about the world, not a judgement of the work. VERIFY_CONTRACT is recorded and surfaced, never enforced: no `if !contract.ok { return }` belongs here.

Verdict language describes what happened, never quality. Full rationale and the prohibited-shapes table: AGENTS.md "Constraints that are easy to violate", ADR-0003, ADR-0006.
