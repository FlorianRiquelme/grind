# Blocker clearance notes: carry the human's "what changed" into the resumed Attempt

The Blocker loop is half-built. Detection (`policy::blocker` — the same denial on two consecutive
working Attempts with no progress), the stop (`State::Blocked`), the rendering (*a Blocker: X must
be cleared*), and hand `resume` all exist — but re-entry composes the fixed `REENTRY_PROMPT`
(`src/attempt.rs`), so the resumed Attempt learns nothing about what changed in the world. Only
the human who cleared the wall knows what they did. This Job adds the missing half: a recorded
clearance note that rides the re-entry prompt. It serves *unattended completion rate* (a resumed
Attempt that knows the wall moved stops re-probing it) and *morning decisions per run* (the
Record then carries what was cleared, not merely that something was).

## Requirements

- **R1** — `grind cleared <run-id> <note>` appends a dated clearance row to the named Run's
  state. Refusing coherently: unknown run-id, empty note, or a Run whose state is not `Blocked`
  lands in the **incoherent-input register** (exit 2) naming the actual state — the same shape
  as `resume` refusing a Completed Run, never a health verdict.
- **R2** — A Resume-mode invocation whose Run carries a clearance note composes it into the
  prompt after `REENTRY_PROMPT`'s text — roughly *"since you stopped, the human reports: …"*;
  final wording is the Run's. **Dispatch and CiBabysit prompts are unchanged**: CiBabysit's
  prompt bounds itself to one reaction and must not grow a second subject.
- **R3** — Clearances accumulate; the **latest** note rides every later Resume invocation (a
  fact about the world does not expire), and all of them stay in the record.
- **R4** — Surfaces: `grind status <run-id>` shows the latest note; the Handback and the
  Job-issue comment carry it in the trailing block **only when one exists** (#16 discipline —
  a clearance decides nothing). The blocked verdict line may name the two-step repair
  (`grind cleared`, then `grind resume`) beside what must be cleared.
- **R5** — Writer ruling: `grind cleared` writes run state as a one-shot supervisor process,
  exactly as `grind resume` already does. CONTEXT.md's *the supervisor is its only writer*
  holds because the CLI verbs **are** supervisor processes.
- **R6** — `just verify` green. New tests only where a safety property exists: the composed
  prompt reaches **Resume argv only**, never Dispatch or CiBabysit; the non-Blocked refusal;
  render prints nothing when no note exists.

## Settled here, not open to the Run

- The note is tied to a stop: it exists because a Blocker stopped the Run. It is **not** a
  general human→Run messaging channel and must not become one.
- Two verbs, not one: `cleared` records, `resume` spends. Grind never chooses to spend an
  Attempt, so the acts stay separate even though the common case runs them back to back.
- `Blocked` stays a supervisor state; no verdict variant, no gate (ADR-0006, ADR-0003).
- Blocked Runs stay excluded from `resume --all`; only the hand clears and only the hand
  re-enters them.

## Non-goals

No new module, no directories under `src/` — the verb parses in `cli`, the row writes through
`supervisor`, the composition lives in `attempt`, the strings in `render`. No change to
`DENIED_TOOLS`. No scheduling, no auto-resume-after-clearance.

## What to watch

- Prompt composition is the risk surface: pin with a test that the note reaches Resume-mode
  argv and nothing else, in the spirit of `no_built_argv_on_any_of_the_three_paths…`.
- The note is human-typed prose that reaches the Job-issue comment. Render it as-is — the
  human typed it for the Run — but add no new exposure beyond the fields already rendered.
- Re-block after a clearance must work: clear, resume, blocked again, clear again — latest
  note wins, both rows survive.
