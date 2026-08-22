---
name: Grind
last_updated: 2026-08-22
---

# Grind Strategy

## Target problem

Plans that are already good enough to act on sit idle whenever the human cannot spend a
supervised session on them — meetings, travel, sleep. The crux is not starting work
unattended, which is easy; it is that what comes back arrives as a pile of findings and open
questions, so picking it up in the morning can cost more than the unattended run saved.

## Our approach

Grind has owned its pipeline since ADR-0015 — superseding ADR-0001's reserved escape clause,
on Run 4's evidence (`docs/findings/0004`): enqueue, unattended dispatch, supervision of a run
that dies, the handback, and now the ten-stage ladder itself, walked one Attempt per stage from
Plan through Ship. Every capability lags supervised local use until it stops needing correction,
and a Run stops at an open PR without ever asserting the work is ready — so the gate stays
downstream of Grind, where it already was: an agent-run review, and the human's merge decision
over the PR's record.

Leg 1 exists. The four things shipped as one accumulating build on the Rust base — #76
spec'd it, PR #79 landed it 2026-08-20 — with the review sweep (#86) and the transcript
live-view fix (#82) on top. Run 3, Grind dispatched at itself (`docs/findings/0003`), scored
handback fidelity 5 of 5 with the record true on every claim.

## Who it's for

**Primary:** The author, at the seam — holding a branch they have already enriched and
cannot afford a supervised session to finish. They are hiring Grind to turn it into an open
PR that costs as little as possible to pick up.

Single-user and opinionated on purpose: no auth, no multi-tenancy, no configuration surface
for other people's repo conventions. If a colleague wants this, they fork their own — which
is affordable precisely because the base is one thin compiled binary over its own stage skills.

## Key metrics

- **Handback fidelity** — of the Handback's five claims about the world (the verdict, plus the
  four observations that decide it: PR open, tree clean, commits ahead, no check pending), how
  many the human had to check for themselves before they could act. The primary metric;
  minimise it. Hand-counted from the Handback beside the PR, and nothing instruments it. Run 1
  scored 0 of 5, Run 2 scored 3 of 5 — and Run 3, against Grind itself, 5 of 5 — and Run 2's
  morning cost was disbelief rather than decisions, which is the cost the metric below could
  not see. Distinct from *self-diagnosable
  failures*: that one is about explaining a death, this one about trusting a terminal fact.
- **Morning decisions per run** — count of findings and open questions that require a decision
  from the human before work can continue. Secondary, and minimised only alongside fidelity:
  on its own it rewards a Run that says nothing, and every Run is now asked to narrate (issue
  #55). **Measured from the Record — the PR and its narrative — never from the handback**, which
  carries no findings and depends on the narrative for nothing.
- **Unattended completion rate** — share of dispatched Runs reaching an open PR with no
  mid-run intervention. Measured from run state.
  Standing after three Runs: 3 of 3 reached an open PR, and the record says 3 of 3
  (`docs/findings/0003`).
- **Weekly-limit cost per run** — session and weekly-limit consumption a Run spends. Kept as
  the instrument that would show the limit becoming binding, not because it already is: the
  scarce input is refined plans, and they arrive slower than the limit refills. Measured from
  run state plus `claudefuel`.
- **Self-diagnosable failures** — of Runs that died, the share the human could explain from
  run state and the Record alone, without re-dispatching or reading raw transcripts. This
  is what makes the record debuggable by a day session rather than an archive.

## Tracks

### Enqueue

Everything at the seam: the Job issue in the target repo, the Handoff SHA, the anchor
artifact, the decomposability admission check, and the frozen provenance (binary version plus
`skills_hash`). Enqueue is also
where the Dispatch is offered — the trigger is a push closing this step, never a watcher
observing anything (ADR-0012).

It ships as `skills/enqueue/`, a globally-loaded skill invoked from the session that prepared
the branch (#69); its Job table is a parser contract, tested by `tests/enqueue_template.rs`.

_Why it serves the approach:_ Enqueue is the last moment a human is present, so it is the
only place a badly shaped job can be caught before it fails unattended and expensively.

### The supervisor

Run-now before any schedule, a ten-stage ladder walked one Attempt per stage with re-entry at
the stage that died, limit handling by sleeping and re-entering rather than pre-flight quota
checks, and run state on gitignored local disk that

is structured enough for a day session to read.

A reboot re-enters what was cut off through a boot-time one-shot calling `grind resume
--all` (ADR-0011); nothing owns the supervisor while the host is up.

_Why it serves the approach:_ It is the resilience layer, so it is **not an agent** — a
supervisor built from the thing that gets rate-limited loses its state exactly when that
matters. That argues against an agent and nothing more: the base is a compiled Rust binary
(ADR-0005), which serves the same reasoning better than a script does.

### The record

The open PR together with the branch behind it — the Record — carrying what happened,
faithfully, in the one place a human actually reads. Everything else that shows a Run is a
projection of it and may compress.

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
- A cross-run digest or cockpit — nothing does the "needs you" job in leg 1, and that is the
  ruling rather than a gap: the human looks, and is never reached. A projection over run
  state is an hour's work if a week of runs shows it is wanted.
- Gating a PR on review findings — the gate is downstream of Grind and Grind does not own
  it. See ADR-0003.
- Proving new capabilities headlessly — new lenses, an adversarial pass, deeper rungs of the
  review tier table and guidelines checking all earn their way in from supervised use. See
  ADR-0002.
- Giving the runner its own Claude plan — still deferred, but no longer for the reason first
  written. Dispatch is human-managed because Grind never selects a Job, not because the human
  is rationing the weekly limit. The credentials half is already settled: a remote host needs
  its own `setup-token` regardless (issue #7).
- A central cross-repo queue — a central one would orphan each issue from its code. The
  justification used to be that `gh search issues --label <x> --assignee @me` already _is_ the
  queue view; ADR-0012 took the label away, so the queue view is now whatever the human's own
  tracker gives them and Grind cannot see it either way. The ruling is unchanged; only its
  example was.
