# Grind

A queue, a supervisor and a record around headless `lfg` runs. It executes plans the human
is not present for, and stops at an open PR.

## Read before working here

- **`CONTEXT.md`** — the glossary. Job, Enqueue, Dispatch, Run, Handoff SHA, Anchor
  artifact, Handback and the rest are defined terms with explicit `_Avoid_` lists. Use them;
  don't drift to the synonyms they rule out.
- **`docs/adr/`** — eight accepted decisions that constrain almost every change here.
- **`docs/provisioned-host.md`** — what a host must guarantee before a Dispatch succeeds on
  it: the `~/.grind/` layout, the executables, the six credential steps, and which items are
  checked at dispatch, by `grind doctor`, or not at all. Read it before provisioning anything.
- **`STRATEGY.md`** — the target problem and the four metrics a change should serve.
- **`docs/findings/`** — what actual Runs measured. `0001-first-run.md` is the only real
  data the metrics have; it also corrects two things `BRAINSTORM.md` got wrong.
- **`BRAINSTORM.md`** — the design record. Historical, and wrong in the places
  `docs/findings/0001` says it is.

## Shape

`bin/grind` is a single Python 3 script, stdlib only. **It is being replaced by a compiled
Rust binary** (ADR-0005): stdlib-only, no-package-manager and no-build-step are withdrawn, and
`serde` is the only dependency the base takes. The script stays reference and evidence — never a
translation source.

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
grind status [run-id]   print run state (latest if omitted)
grind list              list known Runs
```

## Verify entrypoint

```
python3 tests/test_grind.py
```

Exits non-zero on failure and prints per-check lines. Pure functions only — no network, no
`claude` invocations, no pytest. `tests/test_grind.py` guards the logic whose silent failure
would be expensive: mistaking a rate limit for a crash, missing a rate limit, and failing to
notice that a step of the target repo's `just verify` was trimmed until it went green. New
tests belong there when a change carries a safety property, not for coverage's sake.

## Constraints that are easy to violate

- **Run state is never committed.** It is the supervisor's own working record, not history. It
  lives at `~/.grind/runs/` (ADR-0008), outside any checkout, so this holds structurally rather
  than by a `.gitignore` line. The script's `<checkout>/.grind/` is the old location and cannot
  survive a shipped binary — `GRIND_ROOT` derives from `__file__`.
- **The supervisor is the only writer of `run.json`.** A read path that saves what it loaded
  can erase `attempts[]`, which nothing can rebuild — and it erases it while the human is
  watching the dashboard to be reassured. Status and the roster observe fresh and persist
  nothing (issues #12, #27).
- **Privacy only bites between siblings** (ADR-0007). `supervisor` and `view` are siblings at
  the crate root and the writable record type is private to `supervisor`. Never nest them under
  a shared parent, and never add a module named for a noun two others share (`record/`,
  `types`) — a child module reaches its ancestor's private items and **compiles clean**, so the
  tidy-up that looks like housekeeping is what withdraws the guarantee.
- **Grind never gates** (ADR-0003). Verdict language describes what happened, never quality.
  A completed Run means the pipeline finished, not that the code is good. Never add
  something that blocks a PR from existing on the strength of a finding. Two shapes carry
  this in the base and are prohibited (ADR-0006): a verdict variant meaning *rejected*, and
  a summary boolean on the verify contract — `if !vc.ok { return }` is a gate one line away.
- **Grind is a scheduler, not a pipeline** (ADR-0001). Everything between plan and open PR
  belongs to `lfg`. Don't reimplement stages it already runs.
- **The plugin version is frozen per Run, not pinned per Job** (ADR-0002 as amended by #42).
  The Job names the plugin; the host names the version. `resolve_plugin_dir()` runs **once**, at
  dispatch, and the resolved path goes into the record — every attempt and every `--resume` reads
  the record. Never re-resolve per attempt: an 8-attempt Run spans hours of rate-limit sleeps, and
  a version changing mid-Run is silent. Promotion is now enacted by changing Grind, not by
  advancing a pin.
- **Headless deliberately lags local** (ADR-0002). New capabilities get proven in supervised
  sessions first. Grind is not where we experiment.
- **`DENIED_TOOLS` in `bin/grind` is a safety property.** A Run must never merge its own PR,
  force-push, hard-reset, rebase, or delete a branch. Denials are inherited by subagents and
  survive `bypassPermissions`. Don't loosen the list to make a Run go through — and note that
  **nothing sits behind it**: no credential can withhold merge from something allowed to open a PR
  (`Pull requests: write` covers both, `Contents: write` covers push and branch deletion, and
  force-push is indistinguishable from push at every credential layer), so these globs are the
  entire barrier, not the outer one. Established resolving
  [#37](https://github.com/FlorianRiquelme/grind/issues/37).
- **`VERIFY_CONTRACT` is recorded and surfaced, never enforced** — same reason as ADR-0003.
- **Types catch omission and convention, never intent** (ADR-0006). Before reaching for a type
  to protect a property, ask how it realistically fails: a forgotten arm or an unthinking idiom
  is typeable, an agent that means to do it is not. And a variant set is a policy — a careless
  type makes a forbidden thing newly *expressible*, which means reachable, because nobody reads
  the diff. ADR-0006 lists the shapes the base must not have.

## Agent skills

### Issue tracker

Issues live as GitHub issues in `FlorianRiquelme/grind`, driven via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical triage roles, using their default label strings. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `CONTEXT.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.
