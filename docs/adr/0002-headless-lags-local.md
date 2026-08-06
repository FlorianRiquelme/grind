---
status: accepted
date: 2026-07-29
---

# Headless deliberately lags the local session

> **Amended 2026-08-06 by [#42](https://github.com/FlorianRiquelme/grind/issues/42).**
> **The plugin version is frozen per Run at dispatch, not pinned per Job.** The Job names the
> plugin; the host names the version, which is whatever is installed. *Promotion is mechanical*
> below therefore loses its reviewable half — advancing a version is now an auto-update, not an
> edit to a Job — and *never resolve latest at dispatch* is withdrawn as written. What that rule
> protected survives intact and by a different carrier: `resolve_plugin_dir()` runs **once**, at
> dispatch, and the resolved path lands in the record, so every attempt and every `--resume` reads
> the record rather than re-resolving. A Run's plugin version is fixed and knowable for the Run's
> whole life — [#32](https://github.com/FlorianRiquelme/grind/issues/32)'s *conditions read from
> the record, never the environment*, applied to the plugin. Without that, an 8-attempt Run spanning
> hours of rate-limit sleeps could start on one version and finish on another, resuming one session
> across the change, silently.
>
> Traded deliberately by the driver: a quieter queue, no Dispatch refused over an absent version,
> and no manual promotion step, against the loss of review before a version's first unattended use.
> The mirror risk — a box whose cache never refreshes drifting *old* — is accepted and left
> observable in the record rather than built for. The rest of this ADR stands: **headless still
> lags local**, because the capabilities it withholds are behavioural, not versioned.

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
