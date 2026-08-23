---
name: run-reflect
description: Dispatched once, after a finished Run reaches a terminal observation — never a rung on the ladder, never counted against the Attempt budget. Mines the Run's own raw transcripts for lessons and routes each one to code, template, skill, or Rejected-with-reason.
---

# Reflect

Grit's ladder ends at Ship. This is not an eleventh stage — bolting it onto the ladder would
reopen the exact stage-count-vs-budget arithmetic the ladder was sized to close. It is a separate
dispatch the supervisor makes once a Run's Record reaches Completed or Uncorroborated, reading
only what that Run already left behind.

## Where you are

Unlike a ladder stage, this dispatch carries no context block — this skill text is the whole
prompt — so the paths are stated here instead. Your cwd is the finished Run's own directory
(`~/.grind/runs/<run-id>/`), never a worktree. `<stages-dir>` is `./stages`. The raw attempt
transcripts to mine are this run directory's own files. The proposal queue is two
subdirectories the dashboard reads by exactly these names (`view::proposals_in` — a contract:
change either half and check the other): `<stages-dir>/reflect/jobs/`, one file per drafted
follow-up Job, and `<stages-dir>/reflect/diffs/`, one file per proposed skill diff.

## The return

Write `<stages-dir>/reflect.return.json` containing exactly one of `{"status": "complete"}`,
`{"status": "skipped"}`, or `{"status": "incomplete"}` — no other key. A session that dies here
is re-entered once under ordinary Wait rules; if that re-entry also fails, the pass is recorded as
`skipped` rather than blocking the Run's own terminal state on anything. Reflect never blocks —
including its own Run's outcome. Artifacts live under `<stages-dir>/reflect/`.

## Skip first

Some Runs are not worth mining. State the reason in the return's artifacts rather than running the
lenses anyway:

- A **T0 Run with clean verify output** — nothing surprised the pipeline, so there is nothing a
  lens would find that the tier Decision did not already say.
- A **one-off anomaly an existing rule already covers** — a lesson that would just restate a row
  already in `docs/tiers.toml` or `~/.grind/learnings/lessons.tsv`.
- Anything the **target repo answers derivably** — if `view` can compute it from the Records on
  demand, writing it down here duplicates a store that should not exist twice.

## Three readonly lenses, blind to each other

Spawn three fresh subagent sessions over the finished Run's raw attempt transcripts
(this run directory — your cwd — read-only; none of these sessions writes to a tree or opens a
PR).
Each asks one question and returns structured candidates, never prose:

- **Judgment** — where in this Run did a stage substitute a guess for an observable fact it could
  have checked instead?
- **Tooling** — what did a stage do with a prompt sentence that should have been a Rust check —
  something `observe.rs` could answer mechanically and for free, forever, instead of being asked
  of every future Run?
- **Divergent** — what did every stage in this Run accept without comment that, read cold,
  deserved suspicion?

Each candidate a lens returns carries an **evidence pointer** — the attempt record's path and a
line range in its raw transcript — and nothing else. **Pointer, not prose.** A candidate that
cannot name where it came from is not a candidate; it is an opinion, and this pass does not deal in
those.

## One synthesizer, three more duties

A single session reads all three lenses' candidates and produces `Accepted / Rejected / Backlog`.

**The Rejected list is first-class**, not a wastebasket: every discarded candidate carries its
reason. This is what stops a twice-rejected lesson from being proposed a third time — the
synthesizer reads prior Reflect runs' Rejected rows before writing its own.

**The encode-in-structure check**, the most important routing decision here: any Accepted
candidate that would be better enforced by code than by a persona reading a sentence routes to
**Backlog** as a proposed `observe.rs` check or a proposed `JOB-TEMPLATE.md` row — never straight
into a persona prompt. Only a candidate that is genuinely prompt-shaped — a phrasing that misled,
an instruction a persona keeps missing — becomes proposed skill text. An `observe.rs` check that
lands costs zero tokens forever; a persona-prompt lesson costs a sentence on every future Run that
dispatches it. Route toward the cheaper one whenever the candidate allows it.

**The calibration row.** Replay this Run's own facts through `select_tier` and compare the tier it
produced against what Validate actually confirmed: a T1 whose thin review still produced multiple
Confirmed P0/P1s is a miss upward; a T3 whose full panel confirmed nothing is spend that wants
explaining. Write one row — this is statistics feeding the monthly audit, never a taxonomy of the
Run's quality (ADR-0012).

**Drafted follow-up Jobs.** Read the Run's residuals — Fixes rounds that ran out with Confirmed
findings still open, judgement calls Babysit drafted as comments, anything Reflect itself surfaces
that is real work and not a lesson — and draft each as a **complete issue body**: the JOB-TEMPLATE
rows filled, a done predicate stated so a machine could grade it, a Handoff SHA proposed. The
**Anchor artifact row is a path, never prose** — slash-separated segments of letters, digits,
`.`, `_` and `-`; `job::from_issue_json` refuses anything else, and dispatch refuses a path
absent from the worktree at the Handoff SHA. A row reading "this issue body (…)" is exactly what
makes a drafted Job undispatchable. A small Job that warrants no plan doc still needs a real
committed artifact here: anchor on the most specific committed file the work concerns. Park it
in `<stages-dir>/reflect/jobs/`, one file per draft. **Nothing dispatches it and nothing selects
it** — the human stays the only
trigger, same as every other Dispatch (ADR-0001, ADR-0012). What changes is the marginal cost of
the next unit of work: reading a draft instead of writing one from scratch.

## Never edit an installed skill in place

Every proposed change to a skill file — mechanical wording or substantive rewrite — is a **drafted
diff in `<stages-dir>/reflect/diffs/`**, never a write to the skill as it sits on disk. A Run's frozen
provenance names a skills-directory hash; an in-place edit forks what a host runs from what the
repo says, which is the same failure the retired plugin pin made when a cache moved out from under
a Run mid-flight. Land skill edits the ordinary way — a reviewed commit to this repo — never as a
side effect of a Reflect pass.

## Never

- Never propose a lesson without a resolvable evidence pointer.
- Never re-propose a candidate a prior Reflect pass already Rejected without naming why this time
  is different.
- Never write a candidate straight into a persona prompt when an `observe.rs` check or a
  JOB-TEMPLATE row would encode the same lesson for free.
- Never dispatch a drafted follow-up Job, and never mark one selected — the queue is a proposal,
  and the human is the only trigger.
- Never touch an installed skill file. A proposed edit is a diff in the queue, not a write.
- Never let this pass block the Run it followed — a Reflect that cannot complete is recorded
  `skipped`, and the Run's own terminal state stands regardless.

*Mine the Run's own transcripts; route every lesson to code, template, skill, or
Rejected-with-reason; leave the next Job drafted.*
