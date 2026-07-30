---
status: accepted
date: 2026-07-29
---

# Headless deliberately lags the local session

Grind is always behind the capability of a hand-driven Claude Code session, on
purpose. A supervised session can be steered, corrected and learned from mid-flight; a
headless run cannot, so anything unproven that fails there fails silently and expensively.
A capability therefore earns its way into Grind only after it has stopped needing
correction in supervised use — the same evidence that justified automating `/ce-plan`,
which was never corrected.

## Consequences

- **Grind is not where we experiment.** New review lenses, an adversarial pass,
  guidelines checking, `depth:full`, and every other candidate get tried in local sessions
  first. This is also how the round-3 open question "the adversarial stage is unjustified"
  gets settled: measured locally, promoted if it earns it.
- **Promotion is mechanical.** Local sessions run the latest plugin; Grind pins a
  version per job. Advancing that pin *is* the act of promotion, which means promotion is
  reviewable and revertible rather than a vibe.
- A future reader will find Grind less capable than the setup it was built from and
  assume it is stale. It is not — the lag is the design.
