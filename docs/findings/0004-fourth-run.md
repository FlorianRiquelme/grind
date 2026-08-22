# Findings from the fourth Run

Run `20260821-170246-grind-87` against
[grind#87](https://github.com/FlorianRiquelme/grind/issues/87) — blocker clearance notes, the
second Run Grind dispatched at itself. Dispatched by hand 2026-08-21 19:02 CEST; DONE promised
the next morning 10:12 CEST; recorded verdict **`uncorroborated`**.
[PR #89](https://github.com/FlorianRiquelme/grind/pull/89) merged the same afternoon.

**The record is true, and the money is the finding.** Every claim the Handback made holds
against the world, including the one that kept the verdict off `completed`. What this Run
measured that no prior Run could is what the mega-session *shape* costs when the resilience
machinery works exactly as designed — which it did.

## Outcome

| | |
|---|---|
| What happened | **PR #89 open** — 3 commits, +1,007/−34 over 10 files, `just verify` green, merged 2026-08-22 16:12 CEST |
| What the record says | `uncorroborated` — DONE promised, PR found, `commits_ahead: 3`, checks decided, **tree clean: false** |
| Attempts | 8 of 8 — two working, six Waits |
| Cost | **$132.99** ($84.85 + $48.14; the six Waits cost $0.00) |
| Tokens | ~79.6M cache-read, ~1.31M cache-write, ~194k output across the two working attempts |
| Wall clock | 15h10m, most of it the session limit and the host asleep |
| Tool denials | **1** — the first live `DENIED_TOOLS` refusal (below) |
| Fan-out | attempt 1: **12 spawned, 12 returned**; attempt 8: 1/1 |
| PR branch | `feat/blocker-clearance-notes` — **the branch the Job declared**, found by head commit |

## Handback fidelity: 5 of 5, and the verdict is the interesting row

| Claim | Handback says | World says | |
|---|---|---|---|
| verdict | `uncorroborated` | correct — see the tree-clean row | ✓ |
| PR | open, on `feat/blocker-clearance-notes` | same, found by head commit — Run 2's miss stayed fixed | ✓ |
| commits ahead | 3 | `git rev-list --count` = 3 | ✓ |
| tree clean | **false** | **false** — unrelated in-flight work sat in the working tree | ✓ |
| checks pending | decided | CI green, merged by hand hours later | ✓ |

`uncorroborated` is not the record being wrong; it is the record refusing to say `completed`
while one ANDed observation honestly failed. The tree was dirty because the Run shares its
checkout with supervised sessions, and the serve/web-UI feature (plan 005, ADR-0013/0014)
accumulated there while the Run sat suspended overnight. The Run excluded every foreign file
from its commits — the discipline held — but it cannot corroborate *tree clean* in a checkout
other work lives in. **Fidelity and completion are different measurements**: this is 5/5 on
the first and an honest miss on the second, and the miss is a fact about how the host is used,
not about the machinery. A Run that must corroborate its own completion needs a working tree
nothing else writes to.

## The cost of the shape

This is the load-bearing measurement. The timeline:

- **Attempt 1** (dispatch, 19:02–20:02 CEST): 149 turns, **$84.85**, ~49.9M cache-read tokens.
  Planned, implemented (2 commits), fanned out 12 reviewers, got all 12 back — and hit the
  session limit before the review synthesis landed. Classified rate-limited (429 + "session
  limit · resets 12am"), correctly: a limit, not a crash.
- **Attempts 2–7** (20:32–23:03 CEST): six Waits at 30-minute sleeps — one turn, $0.00, no
  budget spent. The exact shape Run 2's evidence bought, working as bought.
- The loop was cut after attempt 7 — the host went down for the night — and re-entered the
  next morning.
- **Attempt 8** (resume, 09:52–10:12 CEST): 51 turns, **$48.14**, ~29.7M cache-read and 725k
  cache-write tokens. Finished the synthesis — reviewers plus a validator confirmed four
  findings, three applied, the fourth filed as
  [#88](https://github.com/FlorianRiquelme/grind/issues/88) — re-ran `just verify` green,
  opened the PR, promised DONE.

Read the second working attempt closely: **$48 of it is re-entry, not work.** Twenty minutes of
actual finishing — synthesis, three fixes, a push — cost more than Run 3's entire Job, because
resuming a 149-turn session after a cold night means re-reading the whole accumulated context
before the first useful token. The mega-session survived its death precisely as designed, and
the price of surviving *as a mega-session* was a second bill more than half the size of the
first. That is the honest framing for any decomposition argument: the resilience machinery does
not need replacing — the shape it has to resuscitate does. One Run, one small supervisor
feature, **$132.99**.

Also in that bill: the 12-reviewer fan-out fired at full width on a five-module supervisor
feature whose Job's own Intent row says "the shape is decided." Nothing sized the review to
the diff; nothing yet can.

## What the Run surfaced about Grind

- **The first live `DENIED_TOOLS` refusal, and it was a false positive that worked as
  documented.** Attempt 1 had one denial: a read-only `git -C … diff --stat` folded into a
  compound command. `Bash(git -C*)` refuses every `git -C` on purpose — CLAUDE.md calls the
  false refusals acceptable for a barrier of this kind — and the agent routed around it and
  kept going. The barrier's cost model (broad glob, occasional harmless refusal, no
  enumeration) now has its first datapoint, and the datapoint is benign.
- **The limit sleep is fixed; the limit stated its reset time.** The result tail said "resets
  12am (Europe/Paris)"; the supervisor probed six times at 1800s intervals anyway, all before
  midnight, all guaranteed futile. Each probe was a free Wait, so nothing was lost — but a
  sleep that can read a stated reset time would have made zero probes instead of six. Watch
  item, not a ticket: the probes cost $0 and the bound (`CONSECUTIVE_WAITS = 12`) was never
  near, but six of eight Attempt slots in the record are noise a smarter sleep would not write.
- **Cross-model review degraded visibly only in prose.** The adversarial pass could not run
  (one peer over quota, one CLI unreachable) and fell back in-process. The Run said so — in
  the PR's run-report comment, which is the right place — but the record itself carries no
  structured trace of the degradation. If a panel's composition ever matters to a morning
  read, the Record is where it has to live.

## Morning decisions per run

One: merge or not. The PR narrative carried the three applied findings with reasons, the
fourth as a filed issue with a link, and the cross-model degradation note. The merge happened
the same afternoon without a transcript dig. **1 of 1 answerable from the Record** — with the
caveat that the `uncorroborated` verdict sent a reader here, to the run state, to learn it
meant "dirty shared checkout" and not "something failed." A verdict that names its failed
observation in the Handback would have saved that trip.
