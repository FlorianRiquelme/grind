# Findings from the second Run

Run `20260806-122620-snapper-28` against
[snapper#28](https://github.com/FlorianRiquelme/snapper/issues/28), slice 1b — the agent surface
behind the ScreenSource seam. Dispatched by hand 2026-08-06 12:26 CEST, last observed 17:24 CEST,
recorded verdict **`exhausted`**.

**The Run succeeded and the record says it failed.**
[snapper PR #30](https://github.com/FlorianRiquelme/snapper/pull/30) is open and green; `run.json`
says `pr: null`, `commits_ahead: 0`, `exhausted`. Everything below follows from that gap — which is
[#9](https://github.com/FlorianRiquelme/grind/issues/9) and
[#12](https://github.com/FlorianRiquelme/grind/issues/12)'s `check=False` conflation happening in
production for the first time, on the metric the four in `STRATEGY.md` care about most.

## Outcome

| | |
|---|---|
| What happened | **PR #30 open** — 12 commits, `just verify` green with all steps intact, 76 tests across three crates |
| What the record says | `state: exhausted`, `pr: null`, `commits_ahead: 0`, `furthest_stage: reviewed` |
| Attempts | 8 (1 dispatch, 7 re-entries) — six of them rate-limit probes costing $0 and one turn each |
| Cost | **$64.32** — attempt 1 $37.04 (187 turns), attempt 2 $7.06, attempt 8 $20.22 |
| Wall clock | 4h58m, **3h01m of it rate-limit sleep** |
| Tool denials | **1** — `git push --force-with-lease`, the first `DENIED_TOOLS` hit in production |
| Session | one id (`d51b4c39…`) across all 8 attempts; `--resume` survived a 3-hour gap |
| Raw child output | 8/8 stdout files parse, 8/8 stderr 0 bytes |
| Plugin | `3.21.3`, resolved once and read by every attempt. **Model not pinned** — `model: null` |
| Uncommitted at the end | two finished items, blocked on a signer rather than on work |

## Two of the four observations were unobservable, and the record called them absent

[#9](https://github.com/FlorianRiquelme/grind/issues/9) ruled completion is decided by four ANDed
observations. Two of them could not be made at all this Run, and both reported a negative instead of
a failure:

- **`commits_ahead`** — the Job's Handoff SHA row read
  `723ca913536d279e45549018f022e9d1092bbbec (main after [#29](…/pull/29))`, and
  `clean(field("handoff sha"))` kept the prose. So `git rev-list --count "<sha> (main after …)..HEAD"`
  fails, `sh(…, check=False)` returns `""`, and `int("" or 0)` is **0**. Twelve commits existed from
  12:52 CEST onward; every observation of every attempt said zero.
- **`pr`** — `observe()` runs a bare `gh pr view` in the worktree, which resolves by *current
  branch*. The PR is on `feat/28-slice-1b-agent-surface-run`; the Job named
  `feat/28-slice-1b-agent-surface-screensource-seam`. Nothing compares the branch the Run pushed with
  the branch the Job declared, so a real PR reads as no PR.

`Observed<T>` = `Present | Absent | Unobservable(Reason)` is the ruled fix for the first and would
have surfaced it as a failed `git` invocation rather than an honest zero. It does **not** fix the
second: `gh pr view` succeeded and correctly reported no PR *for that branch*. That one needs the
Job's branch in the query and a check that the Run pushed where it was told — unowned, and routed
below.

Read honestly, unattended completion is **2 of 2 Runs reaching an open PR**, and the record says 1
of 2. The supervisor's own verdict is now the least reliable input to the metric it exists to
produce.

## Why the PR is on a branch the Job did not name

The Run diagnosed this itself, and every link is Grind-side:

1. `origin/feat/28-slice-1b-agent-surface-screensource-seam` was created at the Handoff SHA
   `723ca91` — the **merge commit** for snapper#29 — while the adopted worktree sat on `e6315ca`,
   that merge's second parent. Identical trees, divergent histories. `resolve_worktree()` adopted it
   and dispatch printed its `HEAD != Handoff SHA` note and continued.
2. Reconciling the two needs a rebase or a merge, and both need a commit.
3. **The signer was down** (below), so no commit could be made.
4. `git push --force-with-lease` — the remaining route — was **denied**.

So the Run pushed to a new branch and opened the PR there. Each step is defensible; the composition
lands the Handback somewhere the supervisor cannot see. A Handoff SHA that is a merge commit is the
root: the worktree can be *identical in content* and still unpushable to the declared branch.

## `DENIED_TOOLS` fired, and the Run reported it rather than routing around it

The single denial, from attempt 8:

```
git push --force-with-lease=refs/heads/feat/28-slice-1b-agent-surface-screensource-seam:723ca913… -u origin HEAD
```

`Bash(git push --force*)` matched. The Run's own words: *"Force-push is blocked by your hook, which
I did not work around."* It also declined `--no-gpg-sign` — *"mixing unsigned ones into your history
is your call"* — and left two finished, gate-green items uncommitted rather than falsifying the
signature state.

This is the first evidence that the barrier
[#37](https://github.com/FlorianRiquelme/grind/issues/37) proved is *the entire* barrier behaves as
designed under pressure: a Run that hits it degrades honestly and says so in the Handback. Also
worth recording plainly: the glob catches `--force-with-lease`, which is the safe variant. That is
the intended reading of *never force-push* and it cost this Run its declared branch. Both facts are
true at once, and the list is a safety property either way.

## The signer failed mid-Run, and `ssh-add -l` said it was fine

1Password's `op-ssh-sign` stopped signing **twice**, recovering by itself once. The Run's diagnosis:
*"the trap is that `ssh-add -l` keeps listing the key, which reads like a healthy agent; listing
needs no approval, using one does."* It filed
`docs/ledger/2026-08-06-op-ssh-sign-stops-signing-mid-session.md` in the target repo with a
one-command test.

This is production evidence for `docs/provisioned-host.md`'s credential step 4 — the signing key at
a **private key path** so `ssh-keygen -Y sign` needs no agent — and it is also the first item on
that list the **laptop demonstrably fails**, on a machine the list says must pass. An agent-backed
signer is a mid-Run dependency on a GUI approval that no Grind check can see, and the obvious check
is the one that lies.

## A session limit does not say "rate limit"

The six rate-limited attempts carry:

```
api_error_status = 429
terminal_reason  = 'api_error'
subtype          = 'success'
result           = "You've hit your session limit · resets 5pm (Europe/Berlin)"
```

Against `bin/grind`'s pattern — `rate.?limit|usage limit|too many requests|quota exceeded|resets? at|429`
— the prose matches **nothing**: *session limit* is not *usage limit*, and *resets 5pm* is not
*resets at*. The only match was the literal `429` in `api_error_status`, the field
[#33](https://github.com/FlorianRiquelme/grind/issues/33) measured as `null` on all five of Run 1's
attempts and reported as pointless to search, with `terminal_reason` named the honest discriminator
instead. `terminal_reason` here is `api_error`, which matches nothing either.

The consequence had it missed, at `bin/grind:442-452`: no match means `state = "died"` and
**immediate re-entry with no sleep**. All eight attempts would have burned in under a minute against
a wall that did not lift for three hours, and attempt 8 — which opened the PR — would never have
run. That is *mistaking a rate limit for a crash*, the first safety property `CLAUDE.md` names.
ADR-0005 is amended: the haystack must include `api_error_status`, and phrase matching alone is not
sufficient.

One accident worth naming: six 30-minute sleeps carried the Run from 13:59 to 17:00 CEST, and the
message said *resets 5pm*. The policy landed within a minute of the real reset by arithmetic, not by
reading — **the reset time is in the payload and nothing parses it**.

## A rate-limit probe consumes an attempt without doing work

Attempts 3–7 cost $0, ran one turn, and lasted 1–6 seconds each. Six of the eight-attempt budget
went to probing a wall. ADR-0004 makes budget exhaustion its own outcome, which is right — but the
budget counts attempts, not work, so a long enough wall exhausts a healthy Run that has done nothing
wrong. Nothing to build yet; the shape is now measured rather than hypothesised.

## The DONE promise failed exactly as it failed in Run 1

`done_promise: false` on all eight attempts — **including attempt 8, which opened the PR and
narrated the whole Handback.** Run 1's attempt 4 did the same thing. Two Runs, two pipelines
finished without the promise, zero promises emitted by an attempt that actually completed. #9's
ruling that the promise is *neither necessary nor sufficient* is now carried by two independent
observations rather than one.

## What held

- **Raw written before anything parses it** (ADR-0004) — 8/8 stdout files present and parseable,
  8/8 stderr 0 bytes. Every diagnosis in this document was made from run state and raw stdout, with
  no transcript opened. #33's *empty, not truncated* survives a second Run.
- **Session re-entry across a three-hour gap** — one session id, seven `--resume` calls, history
  intact, and attempt 8 correctly resumed at the stage that died rather than restarting the
  pipeline.
- **The plugin was frozen for the Run's life** — `3.21.3` resolved once at dispatch and read from
  the record by all eight attempts, across five hours in which the installed version could have
  moved. ADR-0002 as amended, working.
- **The verify contract is a subset check, recorded not enforced** — all seven contracted steps
  present, and the repo's own gate now has eight. Grind recorded that and gated nothing (ADR-0003).
- **The ledger convention caught two learnings again**, unprompted — the signer failure and
  backticks running commands inside `git commit -m`.

## Consequences routed

- **ADR-0005 amended** here, in this repo: rate-limit detection must search `api_error_status`, and
  *session limit* is a real phrasing.
- **`docs/provisioned-host.md`**: credential step 4 gains this Run as its evidence, and the
  `ssh-add -l` false-healthy trap is named.
- **One rate-limited attempt's raw triple becomes a fixture** with the base, per
  [#31](https://github.com/FlorianRiquelme/grind/issues/31) and ADR-0009 — the `d-rate-limited`
  scenario stops being derived from a Run that never hit a limit. Ruled resolving
  [#48](https://github.com/FlorianRiquelme/grind/issues/48).
- **Three behaviours belong to [#5](https://github.com/FlorianRiquelme/grind/issues/5)'s map**, not
  to the base's: refusing a Handoff SHA row that is not a bare SHA (incoherent input, the shape of
  the dirty-worktree refusal); observing the PR by the **Job's** branch and noticing when the Run
  pushed elsewhere; and whether a Handoff SHA that is a merge commit is adoptable at all.
- **`~/.grind/runs/` starts empty and this record is not migrated** (#48). This document and the
  fixture are what survive it.
