# Findings from the fourth Run

Run `20260821-170246-grind-87` against
[grind#87](https://github.com/FlorianRiquelme/grind/issues/87) — "Blocker clearance notes —
carry 'what changed' into the resumed Attempt", the second Run Grind has dispatched at itself.
Dispatched by hand 2026-08-21 19:02 CEST, last observed 2026-08-22 10:12 CEST, recorded verdict
**`uncorroborated`**.

**The Run completed. The record is honest about the one thing it could not corroborate, and
everything else in it is true.** [PR #89](https://github.com/FlorianRiquelme/grind/pull/89) is
open against `main` with CI green, closes #87, and narrates the whole Handback. `run.json` does
not say `completed` — it says `uncorroborated(["tree clean"])`, because the worktree carried an
unrelated in-flight feature (the `grind serve` web UI, plan 005) the Run correctly left untouched
rather than committed. This is not the Run 2 gap: nothing here is false, one observation is
honestly withheld. What this Run is actually evidence for is a cost story, not a resilience one —
the resilience machinery worked exactly as designed, and what it had to resuscitate is the finding.

## Outcome

| | |
|---|---|
| What happened | **PR #89 open** on the declared branch, closes #87, CI green — 3 commits |
| What the record says | `uncorroborated`, `pr` found, `commits_ahead: 3`, tree clean **not** corroborated (unrelated feature work sat in the worktree) |
| Attempts | 8 of 8 — one that did the work, six free Waits, one that finished it |
| Cost | **$132.98** total (attempt 1: $84.85 · attempts 2–7: $0.00 · attempt 8: $48.14) |
| Wall clock | ~15h10m dispatch to completion; ~10h49m of that was attempt 7 ending to attempt 8 starting |
| Tool denials | 0 (one *permission* denial in attempt 1 — a read-only `git -C … diff --stat` check, caught by the broad `Bash(git -C*)` glob) |
| Session | one id (`aaf044e0…`) across all 8 attempts |
| Cache-read tokens | ~79.6M across the two working attempts (49.9M + 29.7M, from each attempt's `usage.cache_read_input_tokens`) |

## Attempt 1 hit the session limit mid-review, at $84.85

149 turns, ending:

```
api_error_status = 429
terminal_reason  = api_error
subtype          = success
result           = "You've hit your session limit · resets 12am (Europe/Paris)"
```

`fanout: { present: [12, 12] }` — twelve reviewers spawned, twelve returned. The supervisor's
furthest-stage inference still read `implemented` (two commits, no `PR open`, no `tree clean`),
which is consistent with the review fan-out having finished and its synthesis — reading twelve
findings files, bucketing them, writing them back into the plan/PR narrative — being what the
session limit cut off. The Job's own Intent row said *"the shape is decided, the prompt wording is
not"* — a small, five-module supervisor feature — and it still earned the full lfg fan-out, because
lfg has no notion of a diff small enough to skip it.

$84.85 is more than Run 3's entire Job ($38.18) and close to Run 2's ($64.32), for planning plus
implementation plus most of a fixed-size review pass that does not know how to be smaller.

## Six free Waits, then the supervisor itself did not survive the night

```
| Attempt | Turns | Cost  | Wall clock | Ended with |
|---|---|---|---|---|
| 2 | 1 | $0.00 | 6s | same 429, same "resets 12am" |
| 3 | 1 | $0.00 | 7s | same |
| 4 | 1 | $0.00 | 7s | same |
| 5 | 1 | $0.00 | 5s | same |
| 6 | 1 | $0.00 | 6s | same |
| 7 | 1 | $0.00 | 6s | same |
```

Each parsed, cost nothing, and took one turn — `Attempt::is_wait`'s predicate exactly, and none of
them spent the budget. Each slept the policy's fixed 1800s and re-entered on schedule, 30 minutes
apart, from 18:32 to 21:03 CEST. The reset time named in the refusal — *resets 12am* — was never
close: probing every 30 minutes against a wall stated to lift at midnight guaranteed six of the
seven overnight attempts would do nothing, which is a policy observation rather than a defect —
nothing here reads cause, and a Wait never claims to.

`supervisor.log` ends mid-sleep after attempt 7, with no further line until attempt 8 begins the
next morning under what is observably a fresh invocation. Consistent with ADR-0011 — nothing owns
the supervisor while the host is up, so a host going down between 21:03 and 07:52 CEST simply ends
the process holding that sleep, and the Run picks back up wherever the record says it stopped.

## Attempt 8: twenty minutes, $48.14, and most of it was re-priming

Resumed 09:52:43 CEST, finished 10:12:23 CEST — 19m40s wall clock, 51 turns, `done_promise: true`,
`terminal_reason: completed`. Its own result narrates real, small work: a plan review with two
applied fixes, one feature landed across two commits, six reviewers plus a validator confirming
four findings on the *diff* (three applied, one filed as
[#88](https://github.com/FlorianRiquelme/grind/issues/88) with a run-report comment), `just verify`
green — fmt, clippy `-D warnings`, 328 tests, both musl cross-builds. None of that is expensive
work. The cost is `cache_read_input_tokens: 29,681,562` — the price of resuming a session that had
already accumulated 149 turns and ~50M tokens of its own context the night before, re-primed cold
before the first useful token of the morning could be produced. Twenty minutes of finishing cost
more than Run 3's whole Job.

## The interpretation: resilience worked, shape didn't

Read the four numbers together: $84.85 to a session limit, six $0 Waits that spent nothing, a
~10h49m gap the supervisor did not survive, and a $48.14 re-entry that did twenty minutes of real
work. Every one of those is the resilience layer behaving exactly as findings 0001–0003 said it
should — a Wait costs nothing, a re-entry picks the Run back up rather than losing it, and the Run
that dies overnight is still the Run that opens the PR the next day. **The Run was never lost.**
That is not this finding's complaint.

The complaint is the shape underneath it. lfg runs one mega-session per Attempt, so "re-enter" and
"re-prime the entire accumulated context" are the same operation — there is no seam between them.
A twelve-reviewer fan-out fired on a diff whose own Intent row said the shape was decided, because
lfg has no cheaper lane to route a small Job into. And the thing that made attempt 8 expensive was
never the twenty minutes of work it did; it was the fifty minutes' worth of context it had to read
before it could start. $132.98 for a five-module feature is the two failures compounding: a fixed
review size that does not scale down, and a session shape where every re-entry pays for the whole
Run's history again.

## What held

- **Waits stayed free and did not spend the budget** — six of eight attempts cost $0 and one turn
  each, exactly `is_wait`'s definition, across a wall the policy could not have shortened by
  reading the reset time (it isn't parsed).
- **Re-entry survived a process death, not just a rate limit** — the gap between attempt 7 and 8 is
  not a sleep, it is the supervisor itself stopping and starting again, and the Run resumed at the
  stage it was at rather than restarting.
- **`DENIED_TOOLS` fired on a read-only command and held anyway** — the one permission denial in
  attempt 1 was a `git -C … diff --stat` check, not a push or a reset; the broad `Bash(git -C*)`
  glob (CLAUDE.md's own acknowledged false-positive cost) caught it and the Run continued around
  it without incident.
- **The record told the truth about what it could not observe** — `uncorroborated(["tree
  clean"])` rather than a false `completed`, because an unrelated feature sat in the worktree. Run
  2's gap — succeeded while the record said failed — does not recur here in the other direction
  either: nothing in this record overclaims.

## Consequences routed

- **`docs/findings/0004` is the motivating evidence for the superseding ADR** (grind#92): the
  numbers above — $132.98 total, ~79.6M cache-read tokens across two working attempts, a
  twelve-reviewer fan-out on a Job that named its own shape as decided — are what argue for cutting
  the mega-session into separately resumable stages rather than for anything about resilience,
  which this Run already had.
- **This is the second Run dispatched at Grind itself**, after `20260821-065705-grind-80`
  (`0003`). The findings numbering (Run 1 = `snapper-21`, Run 2 = `snapper-28`, Run 3 =
  `grind-80`) is canonical; this is Run 4, not "Run 1" of anything.
