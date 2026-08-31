---
description: "Fires when an edit touches the per-backend seam — adding a Backend variant, or a match on one outside src/runner.rs."
globs:
  - "src/runner.rs"
  - "src/cli.rs"
  - "src/serve.rs"
  - "src/page.rs"
  - "src/claude.rs"
  - "src/native.rs"
  - "src/omp.rs"
condition:
  - "Backend::(ClaudeCode|Native|Omp)"
  - "(?i)(denial_carrier|DenialCarrier|denied_globs)"
  - "(?i)(live_for|evidence_links|runner_for)"
interruptMode: never
---

# Every per-backend difference branches in src/runner.rs, and nowhere else

Four things vary per adapter and all four have exactly one match, in `src/runner.rs`: `runner_for` (execution), `runner::live` (live-progress observation), `runner::evidence` (which files an attempt wrote), `Backend::denials` (what carries the denied-tool globs).

Do NOT add a fifth `match backend` to `cli.rs`, `serve.rs`, `page.rs` or a renderer. Three of those matches lived there until #194 — `cli::live_for` and `serve::live_for` were byte-identical copies — and adapter #3 landed without either being widened for it in the same breath. Give the surface a function in `runner.rs` instead and call it.

`Backend::denials` is the one arm the compiler cannot check: it is a *claim* about what that adapter does, so a new adapter copying a neighbour's arm compiles green while claiming enforcement it does not have. `tests/denied_tools.rs::each_adapters_declared_denial_carrier_is_what_its_source_does` checks each declaration against the adapter's source. Widen the declaration; never the test.

This records what an adapter does — it grants nothing, gates nothing, and never narrows `DENIED_TOOLS` (see the denied-tools rule and ADR-0017).

Background: AGENTS.md → "Adding a backend is four arms in `src/runner.rs`"; issue #194.
