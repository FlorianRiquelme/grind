---
name: run-babysit
description: The prompt half of Mode::CiBabysit (src/attempt.rs) — reacting to a red check or a review comment on the PR Ship just opened. The supervisor decides when this fires and how many rounds it gets; this skill states the discipline inside each round.
---

# Babysit

There is no `babysit.return.json`. This is not an eleventh rung — it is the Ship session,
resumed, reacting to what happened after the PR went up. Its artifacts land under
`<stages-dir>/ship/babysit/`; Ship's own return already stands.

## What you are reacting to, in order

Three streams can be waiting for you: a conflict with the base branch, a failing check, a review
comment. Handle them in this order, every round:

1. **Conflicts** — the base moved under the PR and it no longer merges cleanly. Resolve it on this
   branch, forward, never by rebasing or resetting onto the new base (`DENIED_TOOLS` refuses the
   tool calls that would try; do not spend a round discovering that).
2. **Failing checks** — read the failing check's own output, find the cause, fix it, push. Do not
   guess from the check's name; read what it actually printed.
3. **Comments** — review comments on the PR. Address these **while CI runs**, not after it settles
   green — waiting for CI to finish before starting on comments wastes the one thing this mode
   cannot get back, a round.

Within a round, do all three that are waiting rather than picking one and leaving the rest for the
next round — a round is bounded, not the amount of work inside it.

## Judgement calls become a drafted comment, never a question

Nobody is watching this session. A comment that asks a design question, or a check failure whose
fix requires a call this session should not make alone, does not get a question asked into the
void — it gets a **drafted reply, posted as a comment on the Job issue**. That is the only surface
this mode writes to besides the PR itself: no label, no assignee, no project, nothing but a
comment (ADR-0012 — Grind adds, never classifies, and the issue-comment surface is the whole of
what it may touch). State the judgement call plainly, the options as you see them, and what you
did in the meantime if anything had to move forward regardless.

## Rounds

The mode is bounded at **3 failed fix-push-recheck rounds**. The supervisor owns that count — it
reads the Run's own attempt history the same way `trailing_waits` does, so a restart cannot hand a
stuck PR a fresh allowance. This skill's job is to make each round count, not to track the bound
itself: assume any invocation might be your last, react completely, and leave a clean account of
what you did in `<stages-dir>/ship/babysit/` regardless of whether the check went green.

Exhaustion is a described outcome, same as a Fixes round running out — the PR stays open, the
residual checks or comments are named in the Job-issue comment, and nothing here decides the PR
should not exist.

## Never

- Never weaken, trim, or skip a step of the verify entrypoint to make a check go green. A gutted
  check is worse than one that fails honestly — say so in the drafted comment and leave the step
  intact.
- Never merge, force-push, rebase, hard-reset, or delete the branch. Reacting to a red check is
  exactly the situation where these look like the idiomatic repair, which is why `DENIED_TOOLS`
  names them there and refuses them at the tool layer regardless of what this prompt says.
- Never open a second PR.
- Never write anywhere but the PR and the Job-issue comment — no label, no assignee, no board.
- Never let a question sit unanswered in a comment. Turn it into a drafted reply and move on;
  nobody is coming back to answer it live.

*Watch CI within the bounded loop; escalate judgment calls as drafted replies, never questions.*
