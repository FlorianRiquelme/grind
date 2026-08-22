---
readiness: implementation-ready
---

# Grit stage machinery — the supervisor walks the ladder

Issue [#98](https://github.com/FlorianRiquelme/grind/issues/98). Builds on #92 (pure core),
#94 (Job rows), #96 (authored skills). ADR-0015 is the ruling; this plan is the wiring.

**Done predicate:** `just verify` green; a replayed end-to-end scenario walks a T0 ladder from
Dispatch to a terminal observation with per-stage `StageEntry` rows in the record; the two new
completion observations fold into `verdict` such that a missing fifth/sixth signal is `E0063`
at every constructor; pre-cutover fixture records still resume via `rung::from_furthest`; no
path in `src/` names the plugin after unit D.

## Decided constraints (from the design, restated for the implementer)

- `policy.rs` semantics are untouched: `Next`, Wait rules, `CONSECUTIVE_WAITS`, blocker logic,
  `SpendCiBudget` all stay. What changes is what an Attempt *executes*, not what one costs.
- The supervisor stays the sole writer of `run.json`; atomic tmp+fsync+rename stays; the
  attempt list stays append-only. `stages` entries follow the same append-only discipline.
- Nothing gates (ADR-0003): the two new PR observations are ANDed *completion* signals — a
  mismatch is `Uncorroborated`, never a withheld PR. No new boolean summaries (ADR-0006):
  the signals ride `RawSignals` as `Observed<bool>` fields so the named-struct/E0063 carrier
  covers them.
- Spend stays unbounded (ADR-0010): tiers route models and size panels; nothing caps cost.
- Skills are prompt text read from the host skill root; returns are files (ADR-0015).

## Decisions this plan makes (not in the design's words)

1. **Host skill root is `~/.grind/skills/`** (ADR-0008: the host declares itself by layout).
   Provisioning copies `skills/run/*` there; `skills_hash` is a sorted SHA-256 over relative
   path + file bytes of that tree, computed at dispatch through `world`. Hand-rolled hash is
   overkill — but no new dependency (ADR-0005), so it is a simple FNV-1a over the same input,
   named `skills_hash` and documented as an identity check, not a security boundary.
2. **Model routing classes resolve at invocation**: `strong` → the Job's `Model` row when
   present, else the harness default (no `--model` flag); `fast` → `claude-sonnet-5` unless
   the Job's `Model` row names something, which then pins **every** stage (the freeze beats
   the routing — a Job that names a model means it). The resolved model lands in the
   `StageEntry` row, so receipts stay per-stage honest.
3. **Stage returns are read fresh from disk on every loop pass** (`world::read_to_string` per
   `stages/<name>.return.json`, parsed by `rung::StageReturn`); a malformed return reads as
   *not satisfied* — the stage re-enters, which is the fail-closed direction. A helper
   `rung::returns_from(...)` assembles `StageReturns` from ten `Option<&str>` slots so the
   parse stays pure and literals-testable.
4. **`entry_mode` goes per-stage**: fresh stage session (no transcript for `<run>-<stage>`)
   dispatches the stage; an existing transcript resumes it. The Run-level `session_id` field
   stays for pre-cutover records; new records leave it as the Plan stage's id.
5. **The reset-time sleep** parses `resets <time>` from the refusal text (`policy` gets the
   pure parse + the sleep computation; `supervisor` supplies now). Unparseable → the fixed
   1800s. Never sleeps longer than 12h (a garbled parse must not park a Run for a week).
6. **Pre-cutover records**: a record with no `stages` rows and a nonempty attempt list resumes
   at `rung::from_furthest(decide::furthest_stage(observation))` — it keeps its mega-session
   semantics for that one resume (Mode::Resume against the old session), and the ladder
   applies from the next Run. No migration of old records.

## Units

**A — observation surface (`observe.rs`, `decide.rs`; pure + parsers).**
`observe::diff_facts()` over `git diff --numstat` + name-only output (lockfile/generated
subtraction here, kinds from the compiled risky-path/content lists), `pr_head_matches_job_branch()`,
`pr_base_matches_declared()`, `skills_present()`. `RawSignals` gains the two PR fields
(`Observed<bool>`, E0063/E0027 carriers force the fold); `decide::verdict` ANDs them; missing
observations surface in `Uncorroborated`/`Unobserved` lists. `Persona`/`Tier` gain
`#[serde(rename_all = "kebab-case")]` (lowercase `t0..t3`) so `decision.json` matches the
skills' vocabulary before it ever lands on disk. Tests from literals + fixture diffs.
→ verify: `cargo test decide observe`, topology.

**B — invocation surface (`attempt.rs`; pure builders).**
`Invocation` gains stage composition: session id `<run>-<stage>`, the stage skill's text
embedded in the prompt (read by the caller through `world`, passed in as `&str` — builders
stay pure), the Anchor/context bundle per stage, per-stage `DENIED_TOOLS` (base list always;
report-only stages add Write/Edit; panel stages add write-capable Bash forms), the resolved
model per decision 2. Plan-stage composition injects `notes.md` + lessons lines passed in by
the caller. `Mode` unchanged. The existing lfg dispatch/resume builders stay until unit D
deletes them. Tests: every stage's argv from literals; the denial sets asserted per stage.
→ verify: `cargo test attempt`, `tests/denied_tools.rs` extended per stage.

**C — the loop (`supervisor.rs`, `cli.rs`; the risky one, sequential after A+B).**
`RunRecord` gains `stages: Vec<StageEntry>` (append-only accessor pair like `attempts`),
`provenance { binary_version, skills_hash }` (frozen at dispatch), `#[serde(default)]` on both
so pre-cutover records parse. Constants: `ATTEMPT_BUDGET = 14`, `PLAN_REVISIONS = 1`,
`FIX_ROUNDS = 2` (the latter two snapshotted into the record). `supervise()` walks:
read returns → `rung::next` → if `Triage`/`DiffTriage`, run the [R] pass in-process
(`select_tier`, write `decision.json` + return, loop — no Attempt consumed, a `StageEntry`
with cost 0 recorded) → else compose the stage invocation (B) and `run_one_attempt` it →
existing observe/verdict/policy tail unchanged, plus decision 5's reset-time sleep. Terminal
observation dispatches Reflect once (`reflected` flag, one re-entry bound, budget-exempt,
`post-run` tag on its StageEntry). The verdict path adds the fifth/sixth signals from A.
Fixtures: `day-one.json` keeps parsing (defaults); one new fixture with stages rows; the
end-to-end suite gains a T0 ladder walk scenario and keeps the pre-cutover resume scenario.
→ verify: full `just verify`.

**D — the cutover tail (phase 2b; sequential after C).**
Delete `PluginPin`, `Conditions.plugin`, `plugin_dir()`, `observe::plugin_installed()`,
`Check::PluginInstalled` (→ `Check::SkillsPresent`), the template's pinned-plugin row, the
enqueue skill's derivation and always-latest rule, the lfg dispatch builders, and their tests —
grep-verified gone. `docs/provisioned-host.md` swaps the plugin items for the skills-present
item; ADR-0002 amendment note (pin re-seated on binary version + skills hash), ADR-0009
executables note, CONTEXT.md Fan-out entry rewritten, CLAUDE.md Shape section rewritten (the
follow-up ADR-0015 names), USAGE in `cli.rs`.
→ verify: full `just verify` + `grep -ri plugin src/ skills/ | wc -l` = 0.

Order: A ∥ B, then C, then D — every unit leaves `just verify` green and dispatch working.
