---
status: accepted
date: 2026-08-04
---

# The supervisor re-enters rather than predicts, and writes raw before it parses

Four behaviours of `bin/grind` are findings rather than implementation detail. Each was paid
for — three by Run 1, one by reasoning that Run 1 then confirmed — and none of them can be
carried by a test, so they are recorded here. A rewrite that drops any of them is a
regression, not a simplification.

Recorded resolving [#32](https://github.com/FlorianRiquelme/grind/issues/32), whose comment
holds the full inventory: what survives as a test, what is an accident free to drop, and
which of the script's claims are already superseded.

> **Amended 2026-08-05 by [#34](https://github.com/FlorianRiquelme/grind/issues/34).** Three
> behaviours remain rules of this ADR. The fourth — raw written before anything parses it — is
> now carried by a **type** (`RawAttempt`) under ADR-0006, because a type was not an option
> #32 had when it sent all four here. Its section below keeps the evidence and no longer
> states the rule: one carrier per finding, or the finding drifts.

## Sleep long and re-enter; never a pre-flight quota check

The supervisor does not ask whether there is enough budget before starting a stage. It
starts, and if the stage dies against a limit it sleeps and re-enters. **Even a perfectly
informed supervisor would be wrong about what a stage costs** — the cost of an `lfg` stage is
not knowable before it runs, so a pre-flight check trades a certain wrong prediction for an
uncertain one.

Run 1 hardened this from the other direction: no rate limit was hit at all, and the four
deaths were dropped connections that a quota check would not have seen coming
(`docs/findings/0001-first-run.md`). The generic *ended without completing → re-enter* path
is the load-bearing one; limit handling is the narrow special case.

## The attempt budget is bounded, and its exhaustion is its own outcome

The number of attempts is capped, running out is a distinct terminal state rather than a
failure, and the count is surfaced to the human
([#12](https://github.com/FlorianRiquelme/grind/issues/12) prints `attempt N of 8`). The
*number* carries almost no evidence — 8 is arbitrary and Run 1 used 5 — so it is free to
change; the shape is not.

The policy is a thing that changes on its own:
[#23](https://github.com/FlorianRiquelme/grind/issues/23) will decide whether a no-progress
re-entry costs an attempt at all.

## Raw child output is written before anything parses it — carried by a type since #34

**The rule now lives in the type, not here.** `RawAttempt` has private fields and is returned
only by `write_raw`, so parsing before writing is uncallable and the escape is `E0603`
(ADR-0006). What this section keeps is why the invariant was worth a carrier at all.

The prompt, stdout and stderr of every attempt are written to disk *first*; parsing happens
afterwards, over bytes that are already durable. This is why `docs/findings/0001` could report
that **every death was diagnosable from run state alone** — the cause
(`Connection closed mid-response`) was legible without opening a transcript.

It is also a precondition for two later rulings, which is the real reason it must not be
reordered: [#31](https://github.com/FlorianRiquelme/grind/issues/31) makes Run 1's five
attempt files checked-in fixtures, and requires that fakes substitute raw stdout, stderr and
exit code rather than domain values. Neither is possible if the only copy of a response has
already been through a parser.

Parsing itself degrades and never aborts: unreadable output becomes a record that says so and
keeps the tail, rather than an exception. #33 narrowed where that leniency is owed: a killed
child leaves stdout **empty, not truncated**, so the tolerant parse is the transcript's problem
rather than the child's.

## Standard output is line-buffered

The caller does not sit in front of a Run, so the supervisor's output reaches a pipe, a log
file or a journal rather than a terminal. Block-buffered output makes a working Run look dead,
and the human's response to a Run that looks dead is to kill it — the same false negative
[#12](https://github.com/FlorianRiquelme/grind/issues/12) ruled against for the fan-out's
mtime.

This holds regardless of how the supervisor is started, and gets *more* load-bearing if it
becomes a service. Some languages give it for free — Rust's `std::io::Stdout` is a
`LineWriter` — in which case the finding survives as a prohibition: do not wrap it in a
buffered writer for throughput a supervisor does not need.

## What this ADR deliberately does not say

**That nothing is above the supervisor to re-enter it.** True of the script, and the reason
`&` will not do and the caller has to `tmux new -d`. But that is an open defect rather than
preserved knowledge — `.grind/` survives a reboot, so a rebooted host today leaves a Run that
is perfectly re-enterable sitting at `died` until a human notices. The supervisor's process
model is being decided on
[map #5](https://github.com/FlorianRiquelme/grind/issues/5); enshrining the workaround here
would outlive the constraint that produced it.
