---
status: accepted
date: 2026-07-29
---

# Headless deliberately lags the local session

> **Amended a third time 2026-08-15 by [#69](https://github.com/FlorianRiquelme/grind/issues/69).**
> **Promotion is not the pin, and never was after #42.** *Promotion is mechanical* in the
> Consequences below — *"Grind pins a version per job. Advancing that pin is the act of
> promotion, which means promotion is reviewable and revertible"* — is **withdrawn for good**.
> Enqueue resolves the newest installed version at the moment it drafts the Job and writes that
> literal, so nobody advances anything: a pin moves because the local cache updated. The freeze
> survives untouched and is the whole of what the pin is for — one resolution, at Enqueue, landing
> in the record, so a Run's version is fixed and knowable for its whole life.
>
> This is **not** the host-side resolution #50 refused, and #50's argument survives rather than
> losing. #50 refused a Job spelling only `name@marketplace`, resolved by `plugin_dir()` on
> whatever box received it — `Latest` under another name, resolved with nobody watching. Enqueue
> resolves **once, with the human present, and writes the literal**, so the Job still cannot spell
> `Latest`, `PluginPin::parse` still refuses a reference without an `x.y.z`, and the resolution is
> visible in the Job body where a human reads it before filing.
>
> What it costs, stated: a Job's first unattended use of a version is no longer preceded by anyone
> choosing that version, and the local cache held **six** at the time of writing — `3.20.0` through
> `3.21.4`, with Run 1 on `3.21.0` and Run 2 on `3.21.3`. The lag this ADR is actually about is
> untouched, because it was never versioned: **headless still lags local**, and the capabilities it
> withholds are behavioural. `CONTEXT.md`'s **Promotion** entry already described this state and is
> correct as written; this ADR was the document that was behind.

> **Re-amended 2026-08-07 by [#50](https://github.com/FlorianRiquelme/grind/issues/50).**
> **The Job names the version too.** #50 story 5 requires the plugin reference to be refused
> unless it carries both `name@marketplace` and a literal `x.y.z`, and the base implements that:
> `PluginPin::parse` refuses a Job without one, and `job::plugin_dir()` builds the resolved path
> from that literal. Nothing scans the host. So *the host names the version, which is whatever is
> installed* — the #42 sentence directly below — is **withdrawn**, and with it the quieter-queue
> trade: a Dispatch **is** refused over an absent version, and *Promotion is mechanical* in the
> Consequences below stands as originally written.
>
> Why the reversal, rather than teaching `plugin_dir()` to resolve the newest installed version:
> the literal-`x.y.z` shape is the carrier that makes `Latest` **unspellable**. Host-side
> resolution reintroduces it under another name — a Job that says only `name@marketplace` means
> *whatever this box happens to have*, which is the same silent-drift risk #42's own mirror-risk
> paragraph accepted and left merely observable. Refusing at parse time is cheaper than observing
> after the fact.
>
> **#42's load-bearing half survives unchanged and is the part that matters:** resolution happens
> **once**, at dispatch, and the resolved path lands in the record, so every attempt and every
> `--resume` reads the record rather than re-resolving. A Run's plugin version is fixed and
> knowable for the Run's whole life. That was always the real content of #42; what this amendment
> withdraws is only where the version *comes from*, not when it is frozen.
>
> Established resolving [#51](https://github.com/FlorianRiquelme/grind/pull/51), where the base
> was found to implement the pre-#42 reading while this ADR and `CLAUDE.md` described the amended
> one — a contract no code held.

> **Amended 2026-08-06 by [#42](https://github.com/FlorianRiquelme/grind/issues/42).**
> **The plugin version is frozen per Run at dispatch, not pinned per Job.** The Job names the
> plugin; the host names the version, which is whatever is installed. *Promotion is mechanical*
> below therefore loses its reviewable half — advancing a version is now an auto-update, not an
> edit to a Job — and *never resolve latest at dispatch* is withdrawn as written. What that rule
> protected survives intact and by a different carrier: `job::plugin_dir()` runs **once**, at
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
