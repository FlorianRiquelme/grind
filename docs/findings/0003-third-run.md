# Findings from the third Run

Run `20260821-065705-grind-80` against
[grind#80](https://github.com/FlorianRiquelme/grind/issues/80) — the first Run Grind dispatched
**at itself**, and the first on this host after Leg 1. Dispatched by hand 2026-08-21 08:57 CEST,
completed 09:2x CEST, recorded verdict **`completed`**.

**The record is true.** [PR #84](https://github.com/FlorianRiquelme/grind/pull/84) is open
and green on the declared branch; the verdict says so. The gap that defined Run 2 — succeeded
while the record said failed — did not recur.

## Outcome

| | |
|---|---|
| What happened | **PR #84 open** — 3 commits, `just verify` green in CI, issue #76 corrected in body and comment |
| What the record says | `completed`, PR found, `commits_ahead: 3`, `tree clean: true`, `checks pending: false` |
| Attempts | 3 of 8 — two non-working attempts before the completing one |
| Cost | **$38.18** (API pricing) |
| Tool denials | 0 |
| Base drift | **2 commits on `origin/main` since the Handoff SHA** — surfaced, not enforced; both were PR #83 (the #81 worktree fix) landing mid-Run |
| PR branch | `feat/78-amend-named-run2-replay-test` — **the branch the Job declared** |

## Handback fidelity: 5 of 5

Scored by the ruling of `0001` and `0002` — the verdict and the four ANDed observations, each
checked against the world by hand:

| Claim | Handback says | World says | |
|---|---|---|---|
| verdict | `completed` | PR #84 open, both `verify` checks SUCCESS | ✓ |
| PR | open, on `feat/78-amend-named-run2-replay-test` | same, found **by head commit** — Run 2's fatal miss is fixed | ✓ |
| commits ahead | 3 | `git rev-list --count` = 3 | ✓ |
| tree clean | true | worktree clean | ✓ |
| checks pending | false | CI finished green | ✓ |

What moved it: Leg 1's Phase E/F exactly where it aimed. The PR is located by the commit the Run
pushed rather than the branch the Job named (the observation Run 2 could not make), the fresh
verdict renders in the top position, and `commits_ahead` reads through `Observed` rather than
reporting an honest zero for a failed command. Unattended completion now reads **3 of 3 Runs
reaching an open PR, and the record says 3 of 3**.

## What the Run surfaced about Grind — two tickets in one morning

Dogfooding paid before the Run even finished:

- **#81** — dispatch refuses any Job declaring a brand-new branch: `git worktree add` never
  receives `-b`, and git ≥ 2.50 requires the ref. Both prior Runs avoided it by accident. Found
  by a dispatch refusal; unblocked by hand (`git branch <branch> <handoff-sha>`); fixed by PR
  #83 landing **while this Run was in flight** — which is also what made the base-drift row's
  first live appearance.
- **#82** — mid-attempt, the live view is blind to the session transcript that answers *what is
  it doing now*, and its transcript pointer resolves through the `~/.grind` symlink to a path
  Claude does not write. The answer existed on disk the whole time; reading it took an LLM
  session digging by hand.

## Watch item: the good path is no longer flat

#16 ruled five claims, everything else non-zero-only. The completed Handback carried four rows
that are neither: `tool denials | 0`, `fan-out | ?` (unobserved, shown anyway), and the
verify-contract present/missing pair rendered in full. None is wrong; together they are the
beginning of the drift #77 was told to watch — the third category doing work on the good path.
Not a ticket yet; noted for the next Handback change.

## Morning decisions per run

From the Record alone, no Handback digging: the PR narrative states the three decisions taken
(body edit *and* comment, Open Question amended in place, idempotent issue writes across
Attempts), each with its reason, plus two drifts noticed and deliberately left alone. A morning
read is one decision — merge or not — and the narrative supports it. **1 of 1 answerable from
the Record.**

## Cost note

$38.18 for a docs-only Job whose diff is two plan files and an issue edit. The overhead is the
machinery exercising itself — planning skill, fan-out review, idempotent retry logic — on the
smallest possible work. That is the point of a first dogfood; it is not the steady-state price
of a Job.
