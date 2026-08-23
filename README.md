# Grind

A queue, a supervisor and a record around headless runs of its own stage ladder
([ADR-0015][adr15]).

Grind executes plans the human is not present for: a Run turns a prepared branch into an open PR
plus a Handback, then stops. It is not a gate — a completed Run means the pipeline finished, never
that the code is good — and it is not team infrastructure: a single-user proof of concept with no
auth, no multi-tenancy, and no configuration surface for anyone else's repo conventions.

## How it works

**Enqueue** ([`skills/enqueue/`](skills/enqueue/SKILL.md)). Invoked from a session already working
in the target repo, Enqueue drafts the whole Job issue from what that session knows and files
nothing the human has not read. It closes by offering the Dispatch; declining leaves the Job on
the Queue.

**Dispatch**. Always by a human naming the Job — nothing watches the Queue and nothing fires on a
schedule:

```
grind run <issue>
```

At dispatch, provenance is frozen onto the Run: the binary's version plus a hash over every file
under `~/.grind/skills/run`. Every Attempt and every resume reads the record rather than
re-resolving, so a skill edit or a binary upgrade cannot change mid-Run.

**The ladder** ([ADR-0015][adr15]). Ten stages, each one Attempt with its own session and its own
authored skill text:

Plan, Triage, PlanReview, Work, Simplify, DiffTriage, Review, Validate, Fixes, Ship.

Triage and DiffTriage are pure Rust passes costing zero tokens: they size later stages into tiers
T0–T3 from observable facts about the plan and the diff — escalation-only, fail-closed to T2.
Stage completion is supervisor observation of durable return files (`stages/<name>.return.json`),
never the agent's claim; advancement is the total function `rung::next`; a death re-enters only
the stage that died.
Reflect is a post-run pass, deliberately not an eleventh rung — it drafts follow-up Jobs and
skill diffs into a proposal queue readable on the dashboard. The compound-engineering `lfg`
plugin this replaced is retired (#92, #98).

**Handback**. What a finished Run leaves: the Record — the open PR together with the branch
narrative behind it — and a terminal comment on the Job issue carrying the supervisor's five
claims about the world. While Runs are in flight or afterwards, `grind serve` projects run state
onto a read-only dashboard; it writes nothing and owns nothing.

## Using it

| Command | Description |
| --- | --- |
| `grind run <issue>` | dispatch a Job now (issue number or URL) |
| `grind resume <run-id>` | re-enter a Run that died |
| `grind resume --all` | re-enter every Run on this host a restart cut off |
| `grind cleared <run-id> <note>` | record what changed on a Run a Blocker stopped |
| `grind status [run-id]` | roster when bare; one Run's live view when named |
| `grind doctor` | check the provisioned-host list |
| `grind serve [--bind <addr>] [--port <n>]` | serve the dashboard — pull-only; writes nothing |
| `grind outcomes` | human-initiated: read past Runs' PR fate, write outcome.json |
| `grind --version` | which copy of the binary is this |

A host must be provisioned before a Dispatch succeeds on it: the `~/.grind/` layout, the stage
skills copied to `~/.grind/skills/run`, the executables, the six credential steps, and which items
are checked at dispatch, by `grind doctor`, or not at all — all listed in
[docs/provisioned-host.md](docs/provisioned-host.md).

Building from source needs Rust 1.89+ and `cargo build`; `serde` is the only dependency. The
shipped artifact, however, is a prebuilt musl static binary — Grind never builds on a host.

## Installing it

A host owes the list in [docs/provisioned-host.md](docs/provisioned-host.md); the short version,
in order:

1. **Binary** — never build on a host. Cross-build both musl triples from a checkout
   (`cargo zigbuild --release --target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl`,
   or just let `just verify` do it) and copy the one matching the host's arch somewhere on its
   `PATH`. `grind --version` answers *which copy is this* if in doubt.
2. **Stage skills** — from the same checkout:

   ```
   just provision-skills <ssh-host>
   ```

   an `rsync --delete` of `skills/run/` onto the host's `~/.grind/skills/run/`, so Dispatch finds
   all ten stages and freezes provenance against exactly this tree (#103).
3. **Restart one-shot** — the launchd/systemd templates in [`dist/`](dist/) so a reboot re-enters
   cut-off Runs; linux needs the `loginctl enable-linger` step or the unit is enabled and never
   fires.
4. **Credentials** — the six steps in docs/provisioned-host.md; step 6, a real push, is the only
   one that proves the other five.
5. **Check** — `grind doctor` on the host. Every checkable item should print `ok`; the remaining
   rows are the ones no boolean can honestly answer.

On the laptop that dispatches, also point `~/.claude/skills/enqueue` at this repo's copy so
Enqueue tracks the checkout you edit:

```
ln -s <path-to-this-repo>/skills/enqueue ~/.claude/skills/enqueue
```

## Repository map

- `src/` — one crate. Pure modules return effects as values (`policy` returns the sleep, `render`
  returns a `String`), so decisions test from literals with no network; `world` and `serve` are the
  only impure pair, owning process/filesystem and network respectively. `supervisor` holds the loop
  and the record; `cli` is the only thing that prints.
- [`skills/run/`](skills/run/) — one authored skill per ladder stage, plus the persona checklists
  behind Review and PlanReview.
- [`skills/enqueue/`](skills/enqueue/SKILL.md) — the Enqueue skill; its Job table is a parser
  contract with `src/job.rs`, tested by `tests/enqueue_template.rs`.
- [`tests/`](tests/) — every safety property, including the compile-fail carrier, which shells out
  to `rustc` rather than taking a dev-dependency.
- [`docs/adr/`](docs/adr/) — fifteen accepted decisions constraining almost every change here.
- [`docs/findings/`](docs/findings/) — four dogfood Runs' measurements, `0001`–`0004`.
- [`docs/provisioned-host.md`](docs/provisioned-host.md) — what a host owes before a Dispatch
  succeeds on it.
- [`CLAUDE.md`](CLAUDE.md) and [`CONTEXT.md`](CONTEXT.md) — contributor constraints, and the
  glossary of defined terms this file borrows without redefining.

`just verify` is the one definition of checked: fmt, clippy, tests, and musl cross-builds, run by
CI and nothing else.

## Status

Single-user proof of concept, used by its author against their own repos. Four dogfood Runs are
recorded under `docs/findings/`; handback fidelity, morning decisions per run, unattended
completion rate, cost, and self-diagnosable failures are tracked in
[STRATEGY.md](STRATEGY.md). Not looking for contributors — if a colleague wants this, they fork
their own.

[adr15]: docs/adr/0015-grind-owns-its-pipeline-a-ladder-replaces-lfgs-mega-session.md
