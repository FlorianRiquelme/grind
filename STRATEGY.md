---
name: Grind
last_updated: 2026-08-03
---

# Grind Strategy

## Target problem

Plans that are already good enough to act on sit idle whenever the human cannot spend a
supervised session on them — meetings, travel, sleep. The crux is not starting work
unattended, which is easy; it is that what comes back arrives as a pile of findings and open
questions, so picking it up in the morning can cost more than the unattended run saved.

## Our approach

Consume `lfg` wholesale and build only the four things it has no opinion about: enqueue,
unattended dispatch, supervision of a run that dies, and the handback. Every capability lags
supervised local use until it stops needing correction, and a Run stops at an open PR
without ever asserting the work is ready — so the gate stays downstream of Grind, where it
already was: an agent-run review, and the human's merge decision over the PR's record.

## Who it's for

**Primary:** The author, at the seam — holding a branch they have already enriched and
cannot afford a supervised session to finish. They are hiring Grind to turn it into an open
PR that costs as little as possible to pick up.

Single-user and opinionated on purpose: no auth, no multi-tenancy, no configuration surface
for other people's repo conventions. If a colleague wants this, they fork their own — which
is affordable precisely because the shell over `lfg` is thin.

## Key metrics

- **Morning decisions per run** — count of findings and open questions in the handback that
  require a decision from the human before work can continue. The primary metric; minimise
  it. Measured from the handback itself.
- **Unattended completion rate** — share of dispatched Runs reaching an open PR with no
  mid-run intervention. Measured from run state.
- **Weekly-limit cost per run** — session and weekly-limit consumption a Run spends. Kept as
  the instrument that would show the limit becoming binding, not because it already is: the
  scarce input is refined plans, and they arrive slower than the limit refills. Measured from
  run state plus `claudefuel`.
- **Self-diagnosable failures** — of Runs that died, the share the human could explain from
  run state, PR and channel alone, without re-dispatching or reading raw transcripts. This
  is what makes the record debuggable by a day session rather than an archive.

## Tracks

### Enqueue

Everything at the seam: the labelled issue in the target repo, the Handoff SHA, the anchor
artifact, the decomposability admission check, the pinned plugin version.

_Why it serves the approach:_ Enqueue is the last moment a human is present, so it is the
only place a badly shaped job can be caught before it fails unattended and expensively.

### The supervisor

Run-now before any schedule, re-entry at the stage that died, limit handling by sleeping and
re-entering rather than pre-flight quota checks, and run state on gitignored local disk that
is structured enough for a day session to read.

_Why it serves the approach:_ It is the resilience layer, so it is **not an agent** — a
supervisor built from the thing that gets rate-limited loses its state exactly when that
matters. That argues against an agent and nothing more: the base is a compiled Rust binary
(ADR-0005), which serves the same reasoning better than a script does.

### The record

Draft PR provenance and the seeded Buzz feature channel: what happened, faithfully, in the
two places the feature's history will continue to live.

_Why it serves the approach:_ Because Grind never gates, the record is the entire basis on
which a human decides what the Run's output is worth.

### Handback

The shape of what lands in the morning — what is worth surfacing, what is a durable residual,
what is noise, and how much of it needs the human at all.

_Why it serves the approach:_ This is the track that owns the primary metric, and the one
that earns trust. A Run that produces good code and an unreadable pile has still failed.

## Not working on

- Multi-user or team deployment — colleagues fork their own version and run it on their own
  infrastructure against their own Claude plan. Not only a scoping preference: one plan
  serving several people is against Anthropic's terms.
- A cross-run digest or cockpit — channel presence already does the "needs you" job, and a
  projection over run state is an hour's work if a week of runs shows it is wanted.
- Gating a PR on review findings — the gate is downstream of Grind and Grind does not own
  it. See ADR-0003.
- Proving new capabilities headlessly — new lenses, an adversarial pass, `depth:full` and
  guidelines checking all earn their way in from supervised use. See ADR-0002.
- Giving the runner its own Claude plan — still deferred, but no longer for the reason first
  written. Dispatch is human-managed because Grind never selects a Job, not because the human
  is rationing the weekly limit. The credentials half is already settled: a remote host needs
  its own `setup-token` regardless (issue #7).
- A central cross-repo queue — `gh search issues --label <x> --assignee @me` is the queue
  view, and a central one would orphan each issue from its code.
