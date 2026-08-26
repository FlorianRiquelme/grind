---
description: "Guards the tool-denial barrier. Fires when an edit touches DENIED_TOOLS or the subcommand matcher in src/attempt.rs / src/tools.rs."
globs:
  - "src/attempt.rs"
  - "src/tools.rs"
condition:
  - "DENIED_TOOLS"
  - "(?i)disallowedTools"
  - "(?i)(glob_matches|subcommands_of)"
interruptMode: never
---

# The denial barrier narrows only by intent

`DENIED_TOOLS` (`src/attempt.rs`) plus `tools::glob_matches` / `tools::subcommands_of` are the entire barrier between a Run and merging its own PR, force-pushing or deleting a branch (#37) — no credential layer sits behind them.

NEVER remove or weaken a glob (including the deliberately broad `git -C*`, `git -c*`, `sh -c*`, `bash -c*`, `eval*`, mirror/prune/DELETE entries), and keep the candidate generation purely additive — candidates only ever accumulate, so widening may refuse more and can never refuse less.

A narrowing edit that bumps `[&str; N]` and updates the tables and tests compiles green: `just verify` cannot catch intent (ADR-0006), which makes this reminder the last surface that sees it. Review persona CST-4 treats any narrowing as a P0 finding regardless of stated rationale.

Widening is always safe. Background: AGENTS.md → the DENIED_TOOLS constraint.
