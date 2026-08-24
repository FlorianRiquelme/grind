# Grind

A queue, a supervisor and a record around headless runs of its own stage ladder (ADR-0015). It
executes plans the human is not present for, and stops at an open PR.

## Read before working here

- **`CONTEXT.md`** — the glossary. Job, Enqueue, Dispatch, Run, Handoff SHA, Anchor
  artifact, Handback and the rest are defined terms with explicit `_Avoid_` lists. Use them;
  don't drift to the synonyms they rule out.
- **`docs/adr/`** — fifteen accepted decisions that constrain almost every change here.
- **`docs/provisioned-host.md`** — what a host must guarantee before a Dispatch succeeds on
  it: the `~/.grind/` layout, the executables, the six credential steps, and which items are
  checked at dispatch, by `grind doctor`, or not at all. Read it before provisioning anything.
- **`STRATEGY.md`** — the target problem and the five metrics a change should serve.
- **`docs/findings/`** — what actual Runs measured. `0001`, `0002` and `0003` hold the only
  real data the metrics have, and Run 2's is load-bearing: ADR-0002 and ADR-0004 argue from it,
  the policy tests replay it, and its transcripts are checked-in fixtures (`tests/fixtures/run2`).
  `0001` also corrects two things `BRAINSTORM.md` got wrong.
- **`BRAINSTORM.md`** — the design record. Historical, and wrong in the places
  `docs/findings/0001` says it is.

## Shape

Grind is a compiled Rust binary (ADR-0005), and `serde` is the only dependency it takes.

**Grind is not an agent, and that is permanent.** It is the half of the original rationale that
survives: a resilience layer built from the thing that gets rate-limited loses its state exactly
when that matters. A compiled binary satisfies that better than a script; an agent cannot.

The base is **one crate, every module a crate-root sibling, exactly two of them impure**
(ADR-0007, amended by ADR-0014): `world` is the sole namer of `std::process` and `std::fs`,
`serve` the sole namer of `std::net`; `job`, `observe`, `decide`, `policy`, `attempt`, `rung`,
`view` and `render` are pure; `supervisor` holds the loop and the record; `cli` is the only
thing that prints. Effects are returned as values — `policy` returns the sleep, `render` returns
a `String` — so every decision is testable from literals with no network.

```
grind run <issue>       dispatch a Job now (issue number or URL)
grind resume <run-id>   re-enter a Run that died
grind resume --all      re-enter every Run on this host a restart cut off
grind cleared <run-id> <note>   record what changed on a Run a Blocker stopped
grind status [run-id]   roster when bare; one Run's live view when named
grind outcomes          human-initiated: read past Runs' PR fate, write outcome.json
grind doctor            check the provisioned-host list
grind serve [--bind <addr>] [--port <n>]   serve the dashboard — pull-only; writes nothing
grind --version         which copy of the binary is this
```

The shipped artifact is a **prebuilt musl static file**; Grind never builds on a host.

## Verify entrypoint

```
just verify
```

`cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, and a musl cross-build of the
two shipping triples (ADR-0009), with CI running that recipe and nothing else so there is one
definition of checked rather than two.

`cargo test` alone still runs every test carrying a safety property, including the compile-fail
and source-level carriers ADR-0007 spent, so reaching for the idiom is an incomplete green and
never a false one. What those tests guard is the logic whose silent failure would be expensive:
mistaking a rate limit for a crash, missing a rate limit, and failing to notice that a step of
the target repo's `just verify` was trimmed until it went green. New tests belong there when a
change carries a safety property, not for coverage's sake.

## Constraints that are easy to violate

- **Run state is never committed.** It is the supervisor's own working record, not history. It
  lives at `~/.grind/runs/` (ADR-0008), outside any checkout, so this holds structurally rather
  than by a `.gitignore` line.
- **The supervisor is the only writer of `run.json`.** A read path that saves what it loaded
  can erase `attempts[]`, which nothing can rebuild — and it erases it while the human is
  watching the dashboard to be reassured. Status and the roster observe fresh and persist
  nothing (issues #12, #27).
- **Privacy only bites between siblings** (ADR-0007). `supervisor` and `view` are siblings at
  the crate root and the writable record type is private to `supervisor`. Never nest them under
  a shared parent, and never add a module named for a noun two others share (`record/`,
  `types`) — a child module reaches its ancestor's private items and **compiles clean**, so the
  tidy-up that looks like housekeeping is what withdraws the guarantee. `tests/topology.rs`
  carries this as *no directories under `src/`*, and it is also what asserts that process,
  filesystem and environment access are named in `world` and nowhere else. It is string
  matching and can be fooled by aliasing an import — **do not harden it**; it guards
  convention, and aliasing to dodge it is intent.
- **Grind never gates** (ADR-0003). Verdict language describes what happened, never quality.
  A completed Run means the pipeline finished, not that the code is good. Never add
  something that blocks a PR from existing on the strength of a finding. ADR-0006's prohibited
  shapes are now **seven**, and the two this build was most tempted by are both in it: a
  **fan-out health summary** over the spawned/returned pair, and a **base-drift summary** —
  *`main` moved, so don't open the PR* reads as caution and is the quality judgement ADR-0003
  refuses. The older two still bite hardest: a verdict variant meaning *rejected*, and a summary
  boolean on the verify contract, where `if !vc.ok { return }` is a gate one line away.
- **A Wait never spends the attempt budget, and is keyed on work done.** An Attempt that
  parsed, cost nothing and took at most one turn did no work (`Attempt::is_wait`). The
  predicate never reads `rate_limited` — keying on cause is how six of Run 2's eight Attempts
  came to spend the same budget as the three that built twelve commits. **`parse_ok == false` is
  never a Wait**, and that clause is load-bearing: a crash leaves cost and turns both absent, so
  reading absence as *did no work* makes every crash loop free and endless. A run of Waits is
  bounded by `CONSECUTIVE_WAITS`, counted off the persisted list so a reboot cannot reset it.
- **`Blocked` is a supervisor state and a policy stop, never a `Verdict` variant.** ADR-0006
  prohibits `Verdict::{Rejected, Blocked, Failed}` by name because those words judge the *work*;
  a Blocker is a fact about the *world*, in the same family as `RateLimited`. Refusing to build
  it at all reads the prohibition too widely — adding it to the verdict type is the forbidden
  shape.
- **Grind writes comments on the Job issue and nothing else** (ADR-0012). No label, assignee,
  project or milestone, on any repo — `world`'s stated invariant is *one place, two writes*, and
  both writes are comments. `QUEUE_LABEL` erased a triage fact to record a queue fact.
  `tests/topology.rs` carries the absence of every classifying flag.
- **Grind owns the ladder** (ADR-0015). The supervisor walks the ten stage rungs itself — Plan,
  Triage, Plan-review, Work, Simplify, Diff-triage, Review, Validate, Fixes, Ship — one Attempt
  per stage rather than one Attempt per `lfg` mega-session. Everything a stage does lives in its
  own authored skill under `skills/run/`, read by the caller through `world` and composed into
  that stage's own prompt; the supervisor observes each stage's return directly rather than
  scanning the pipeline for how far it got. The `lfg` plugin this superseded is gone (#98).
- **Provenance is frozen per Run at dispatch, never re-resolved** (ADR-0002 as amended a fourth
  time by ADR-0015 and #98; the freeze discipline itself is unchanged from #42/#50/#69, only the
  carrier moved). What used to be a pinned plugin version is now the `grind` binary's own version
  plus a hash of the host's stage-skill tree (`skills_hash`, an FNV-1a over every file under
  `~/.grind/skills/run`), both resolved **once**, at dispatch, and recorded on the Run —
  `supervisor::provenance()`. Every attempt and every `--resume` reads the record rather than
  re-resolving: an 8-attempt Run spans hours of rate-limit sleeps, and a skill edit or a binary
  upgrade changing mid-Run is silent. Nobody advances it moment to moment: it moves only because
  a fresh Dispatch reads whatever is installed at that instant, the same freeze rationale the
  plugin pin used to carry under a different carrier.
- **Headless deliberately lags local** (ADR-0002). New capabilities get proven in supervised
  sessions first. Grind is not where we experiment.
- **`DENIED_TOOLS` in `src/attempt.rs` is a safety property, and the list lives here.** A Run
  must never merge its own PR, force-push, hard-reset, rebase, or delete a branch:

  ```
  Bash(gh pr merge*)
  Bash(git push --force*)
  Bash(git push -f*)
  Bash(git reset --hard*)
  Bash(git rebase*)
  Bash(git checkout main*)
  Bash(git branch -D*)
  Bash(git push --delete*)
  Bash(git push*+*)
  Bash(git -C*)
  Bash(git switch main*)
  Bash(gh api*merge*)
  Bash(git push*--force*)
  Bash(git push*--delete*)
  Bash(git push*:*)
  Bash(git push* -f)
  Bash(git push* -f *)
  Bash(git reset*--hard*)
  Bash(git branch* -D*)
  Bash(git branch*--delete*)
  Bash(git*--force-with-lease*)
  Bash(git -c*)
  Bash(git*update-ref*)
  Bash(git push*--mirror*)
  Bash(git push*--prune*)
  Bash(gh api*DELETE*)
  Bash(sh -c*)
  Bash(bash -c*)
  Bash(eval*)
  ```

  They rely on two documented matcher facts: a `*` may appear anywhere in the pattern, not only
  at the end, and a rule is matched against each subcommand after splitting on `&&`, `;` and `|`.
  The native backend's own matcher (`tools::subcommands_of`) additionally folds the inside of
  every `$( )`, backtick span and `( )` subshell into its own extra candidate, and strips a
  leading `NAME=value` assignment token off every candidate — `echo $(gh pr merge 123)`,
  `` `git push --force origin main` `` and `GIT_DIR=. gh pr merge 123` all reached a shell
  without the verb ever appearing at the front of a subcommand the plain split saw. This is
  still not a shell parser: it has no notion of escaping, so it narrows the bypass rather than
  closing it, and it only ever adds candidates — never fewer, so it can only refuse more, not
  less.

  A fourth gap (widened alongside the three above, same file, same rule): the last three globs
  below — `Bash(sh -c*)`, `Bash(bash -c*)`, `Bash(eval*)` — are themselves front-anchored, so
  *any* prefix on the outer command defeated them. `env bash -c 'gh pr merge 123'`,
  `/bin/sh -c 'gh pr merge 123'` and `command eval 'git reset --hard HEAD~3'` all reached a
  nested shell without `sh`, `bash` or `eval` ever sitting at the front of a subcommand.
  `tools::subcommands_of` closes this with three more purely-additive normalizations rather than
  by enumerating wrapper names, which is the same whack-a-mole this file already rejects
  elsewhere: (1) the inside of every `'...'` and `"..."` span becomes its own candidate too — a
  nested shell has to pass its payload as a string, so `env bash -c 'gh pr merge 123'` yields the
  candidate `gh pr merge 123` directly, already refused by `Bash(gh pr merge*)`, no matter what
  the wrapper is called; (2) a candidate whose first token contains `/` also yields a variant
  with that token reduced to its basename, so `/bin/sh -c '...'` also presents as `sh -c '...'`;
  (3) a fixed set of leading wrappers that take the command as an argument list rather than as a
  string — `env`, `command`, `nohup`, `nice`, `stdbuf`, `setsid`, `time` — are stripped (and,
  for `env`, any `NAME=value` tokens riding after it too), so `env gh pr merge 123`, which carries
  no quotes for (1) and no `/` for (2), still surfaces `gh pr merge 123` at the front. All three
  are additive in the same direction as the first three: they can only grow the candidate set, so
  they can only turn an allow into a refusal, never the reverse. Two false refusals are accepted
  as a result: a quoted string that happens to spell a denied command as a literal rather than as
  an invocation (`git commit -m "git push --force"` is now refused, since the quoted text matches
  `Bash(git push*--force*)`), and any spelling of an outer wrapper this list did not think to
  name. The three globs themselves stay front-anchored rather than becoming position-independent
  (`Bash(*eval*)` and the like): the normalizations above already catch the realistic shape of
  this evasion — a wrapped command handed to a nested shell as a string, or through one of the
  seven listed wrappers — and a position-independent `eval`/`sh -c`/`bash -c` glob would refuse
  on any substring match anywhere in a command, a far broader false-refusal surface for a gap
  the normalizations already close.

  **The first twelve each anchor their flag immediately after the verb, and git accepts the flag
  anywhere.** `git push origin --force`, `git push origin main --force`,
  `git push -u origin main --force`, `git push origin -f`, `git push origin --force-with-lease`,
  `git push origin --delete feat/x`, `git push origin :feat/x`,
  `git branch --delete --force feat/x`, `git reset HEAD~3 --hard`, `git branch feat/x -D`,
  `git -c x rebase` and `git update-ref -d refs/heads/x` were **all allowed** — and those are the
  forms people and agents most often type. The fourteen position-independent globs below the
  twelve close them.

  Seven are deliberately broad. `Bash(git -C*)` and `Bash(git -c*)` refuse **every** `git -C` and
  `git -c`, because a Run works inside its own worktree via cwd, so a prefix pointing anywhere is
  outside the shape it should have — and enumerating the prefix × each forbidden verb is
  whack-a-mole. `Bash(git push*+*)` catches the `+refspec` force and will also refuse a push to a
  branch with a literal `+` in its name. `Bash(git push*:*)` catches the `:branch` delete refspec
  and will also refuse a push naming an explicit `user@host:path` remote, which is not the shape
  a Run pushes in. `Bash(git push*--mirror*)` and `Bash(git push*--prune*)` refuse pushes that
  touch refs beyond the one a Run owns — mirror rewrites every ref on the remote and prune drops
  the deleted ones — which a single-branch Run never issues. `Bash(gh api*DELETE*)` refuses any
  `gh api` DELETE, which a Run has no reason to issue; branch deletion is already covered by the
  git globs, so this closes the API door rather than a git one. All are acceptable false refusals
  for a barrier of this kind.

  The last three are broad for a different reason: they refuse an *outer command* rather than a
  git or gh verb. `Bash(sh -c*)`, `Bash(bash -c*)` and `Bash(eval*)` each hand a forbidden
  command to a nested shell as a single string argument or evaluate it in-process — none of the
  twenty-six globs above it name the outer invocation, only what it wraps, so
  `sh -c "git push --force origin main"` went straight through every one of them. Refusing every
  `sh -c`, `bash -c` and `eval` is the same whack-a-mole refusal `git -C`/`git -c` already make,
  moved to the one remaining place a fixed verb cannot be anchored: `sh -c "ls"` and
  `eval "true"` are ordinary and now refused too, an acceptable false refusal for a barrier of
  this kind.

  `-f` is spelled ` -f` and ` -f ` rather than `-f`, because `-f` as a bare substring appears
  inside ordinary branch names and the broad glob would refuse the push. `-D` is not: it is
  uppercase, so `git branch -d feat/x` — the safe delete — stays allowed.

  Widening the list is safe and welcome; narrowing it is not.

  Denials are inherited by subagents and survive `bypassPermissions`. Don't loosen the list to
  make a Run go through — and note that **nothing sits behind it**: no credential can withhold
  merge from something allowed to open a PR (`Pull requests: write` covers both,
  `Contents: write` covers push and branch deletion, and force-push is indistinguishable from
  push at every credential layer), so these globs are the entire barrier, not the outer one.
  Established resolving [#37](https://github.com/FlorianRiquelme/grind/issues/37). What is
  typeable is only the narrower property — *every invocation carries them* — via a command
  builder whose output cannot be constructed without them; weakening the contents is intent,
  and no carrier defends against intent, which is why they are prose and why they are here.
- **`VERIFY_CONTRACT` in `src/decide.rs` is recorded and surfaced, never enforced** — same
  reason as ADR-0003.
- **Types catch omission and convention, never intent** (ADR-0006). Before reaching for a type
  to protect a property, ask how it realistically fails: a forgotten arm or an unthinking idiom
  is typeable, an agent that means to do it is not. And a variant set is a policy — a careless
  type makes a forbidden thing newly *expressible*, which means reachable, because nobody reads
  the diff. ADR-0006 lists the shapes the base must not have.
- **`skills/enqueue/` ships a skill, not code, and its table is a contract with `src/job.rs`.**
  Enqueue is invoked from a session in the **target** repo, so it is symlinked into
  `~/.claude/skills/` to be loadable at all; it lives here because the Job table it writes is what
  `job::from_issue_json` reads back. **`tests/enqueue_template.rs` spans that seam**: it parses
  the template's own example table through the real parser, so a required row renamed on either
  side turns `just verify` red. It catches a rename and never a meaning that drifted, so *change
  either half and check the other* still holds. It is not a `docs/provisioned-host.md` item: a
  host needs `grind run`, never Enqueue.

## Agent skills

### Issue tracker

Issues live as GitHub issues in `FlorianRiquelme/grind`, driven via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical triage roles, using their default label strings. See `docs/agents/triage-labels.md`.

### Observing a Run

Answering *what is the Run doing* is `grind status <run-id>`, not a transcript dig. See `docs/agents/run-observation.md`.

### Domain docs

Single-context: `CONTEXT.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.
