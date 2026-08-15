# Grind

A queue, a supervisor and a record around headless `lfg` runs. It executes plans the human
is not present for, and stops at an open PR.

## Read before working here

- **`CONTEXT.md`** — the glossary. Job, Enqueue, Dispatch, Run, Handoff SHA, Anchor
  artifact, Handback and the rest are defined terms with explicit `_Avoid_` lists. Use them;
  don't drift to the synonyms they rule out.
- **`docs/adr/`** — eleven accepted decisions that constrain almost every change here.
- **`docs/provisioned-host.md`** — what a host must guarantee before a Dispatch succeeds on
  it: the `~/.grind/` layout, the executables, the six credential steps, and which items are
  checked at dispatch, by `grind doctor`, or not at all. Read it before provisioning anything.
- **`STRATEGY.md`** — the target problem and the four metrics a change should serve.
- **`docs/findings/`** — what actual Runs measured. `0001-first-run.md` is the only real
  data the metrics have; it also corrects two things `BRAINSTORM.md` got wrong.
- **`BRAINSTORM.md`** — the design record. Historical, and wrong in the places
  `docs/findings/0001` says it is.

## Shape

Grind is a compiled Rust binary (ADR-0005), and `serde` is the only dependency it takes.

**Grind is not an agent, and that is permanent.** It is the half of the original rationale that
survives: a resilience layer built from the thing that gets rate-limited loses its state exactly
when that matters. A compiled binary satisfies that better than a script; an agent cannot.

The base is **one crate, ten modules, exactly one of them impure** (ADR-0007): `world` is the
sole namer of `std::process` and `std::fs`; `job`, `observe`, `decide`, `policy`, `attempt`,
`view` and `render` are pure; `supervisor` holds the loop and the record; `cli` is the only
thing that prints. Effects are returned as values — `policy` returns the sleep, `render` returns
a `String` — so every decision is testable from literals with no network.

```
grind run <issue>       dispatch a Job now (issue number or URL)
grind resume <run-id>   re-enter a Run that died
grind status [run-id]   roster when bare; one Run's live view when named
grind doctor            check the provisioned-host list
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
  something that blocks a PR from existing on the strength of a finding. Two shapes carry
  this in the base and are prohibited (ADR-0006): a verdict variant meaning *rejected*, and
  a summary boolean on the verify contract — `if !vc.ok { return }` is a gate one line away.
- **Grind is a scheduler, not a pipeline** (ADR-0001). Everything between plan and open PR
  belongs to `lfg`. Don't reimplement stages it already runs.
- **The plugin version is pinned per Job and frozen per Run** (ADR-0002 as amended by #42 and
  re-amended by #50). The Job names both the plugin and the version, and a reference without a
  literal `x.y.z` is refused at parse time — that shape is what makes `Latest` unspellable.
  `job::plugin_dir()` then runs **once**, at dispatch, and the resolved path goes into the record —
  every attempt and every `--resume` reads the record. Never re-resolve per attempt: an 8-attempt
  Run spans hours of rate-limit sleeps, and a version changing mid-Run is silent. Advancing the
  pin in a Job is the act of promotion, which keeps promotion reviewable.
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
  ```

  The last five close spellings the first seven miss, and they rely on two documented matcher
  facts: a `*` may appear anywhere in the pattern, not only at the end, and a rule is matched
  against each subcommand after splitting on `&&`, `;` and `|`. Two are deliberately broad.
  `Bash(git -C*)` refuses **every** `git -C`, because a Run works inside its own worktree via
  cwd, so `-C` pointing anywhere is outside the shape it should have — and enumerating `-C` ×
  each forbidden verb is whack-a-mole. `Bash(git push*+*)` catches the `+refspec` force and will
  also refuse a push to a branch with a literal `+` in its name, which is an acceptable false
  refusal for a barrier of this kind. Widening the list is safe and welcome; narrowing it is not.

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
  `job::from_issue_json` reads back, and **nothing tests that seam** — no test spans a skill and a
  parser. One repo means one diff, which is the whole of the mitigation. Change either half and
  check the other. It is not a `docs/provisioned-host.md` item: a host needs `grind run`, never
  Enqueue.

## Agent skills

### Issue tracker

Issues live as GitHub issues in `FlorianRiquelme/grind`, driven via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical triage roles, using their default label strings. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `CONTEXT.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.
