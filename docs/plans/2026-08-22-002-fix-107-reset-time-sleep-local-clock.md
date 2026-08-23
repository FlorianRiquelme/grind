# Fix #107 — the reset sleep is computed against the wrong clock

## Problem

`policy::reset_time_sleep` (`src/policy.rs:152`) is pure and correct as designed: it parses the
stated reset hour out of a rate-limit message and takes `now: (u32, u32)` from its caller. The
caller, `supervisor::now_hour_minute()` (`src/supervisor.rs:1003`), supplies UTC. A limit
message states a wall-clock reset in the account's zone — Run 4's said
`"resets 12am (Europe/Paris)"` while the host clock was CEST — so the computed sleep is off by
the zone offset in both directions: an undershoot wakes into one more free Wait (harmless), an
overshoot parks the Run up to the offset past the actual reset (spends nothing, wastes wall
clock).

Issue [#107](https://github.com/FlorianRiquelme/grind/issues/107) weighed two options and
leans to the first: compare in host-local time. This is right whenever the host's zone matches
the account's, which is the standing case for this fleet. The compiled zone table (option 2)
is out of scope.

## Requirements

- **R1** — The `(hour, minute)` handed to `policy::reset_time_sleep` is the host's local
  wall-clock reading, not UTC.
- **R2** — The local read is named in `world` and nowhere else (ADR-0007: `world` is the sole
  namer of `std::process`, `std::fs` and `std::env`). `policy` stays pure; its signature does
  not change.
- **R3** — No new dependency (ADR-0005: serde is the only one). `std` has no timezone
  database, so the read must come from what the host already provides; the plan chooses the
  mechanism and says why.
- **R4** — The 12-hour cap and the garbled-parse fallback to the recorded fixed sleep are
  unchanged, and the existing tests carrying them stay green.
- **R5** — The record's timestamps stay UTC (`world::now_iso` and `now_stamp` untouched); only
  the reset comparison moves to local time.

## Done predicate

`just verify` is green, and a test demonstrates that the reading handed to
`reset_time_sleep` comes from the local-time seam rather than from the UTC path — stated at
the seam so it grades without wall-clock flakiness.
