# Fixtures

**A recorded artifact is not the supervisor's live record.** Run state is never committed
(ADR-0008) — it lives at `~/.grind/runs/`, outside any checkout. What is checked in here is
*evidence*: raw bytes two real Runs produced, kept so a classifier can be tested against what
actually happened rather than against what someone imagined. Checking these in does not violate
that constraint, and the distinction is written down here so it survives the next tidy-up.

**Do not normalise, prettify or re-serialise a recorded fixture.** A reformatted fixture stops
being evidence. The authored ones are marked below.

| path | origin |
|---|---|
| `run1/attempt-{1..5}.{stdout.json,stderr.log}` | **Recorded.** Run 1 (`docs/findings/0001-first-run.md`), five attempts, three of which died. Every `stderr.log` is 0 bytes — that is the measurement, not a missing file. |
| `run1/degraded-*.stdout.json` | **Recorded, then damaged deliberately** in the spike: the empty, garbage, renamed-field and truncated shapes a child's stdout can arrive in. |
| `run2/rate-limited.{stdout.json,stderr.log}` | **Recorded.** Run 2's attempt 3 — the only copy of the session-limit shape. `api_error_status` is `429`, `terminal_reason` is `api_error`, `subtype` is `success`, `is_error` is true, and the prose reads *"You've hit your session limit · resets 5pm (Europe/Berlin)"*, which matches none of the old script's phrases. The 0-byte stderr is part of the evidence. |
| `record/day-one.json` | **Authored**, in the base's own record shape — one attempt of each recorded outcome (died, unparseable, rate-limited, CI-babysit-and-done). Run 1's own `run.json` is deliberately *not* here: it is the script's record, it lacks six fields the base forces at construction, and parsing it at all would be the migration read path there is deliberately none of. |
| `gh/auth-failure.{stdout,stderr,code}` | **Authored.** Empty stdout, a non-zero exit and an auth message on stderr — the case where *observed absent* and *could not observe* have to be separable from three values alone. |
| `transcript/{empty,renamed-field,type-changed}.jsonl` | **Recorded, then damaged**, from the transcript spike. The same file changes field names and field types between its own lines; that is why the transcript is read tolerantly and the child's stdout strictly. |
| `transcript/fanout/8f2c1a70-*.jsonl` and `8f2c1a70-*/subagents/*` | **Authored** — a parent transcript beside two subagent transcripts, spelling the **former** fan-out tool name (`Task`). Synthetic on purpose: the real fan-out session is verbatim conversation content from an unrelated project and is not in git anywhere. Git carries no mtimes, so the freshness test sets them at run time. |
| `transcript/fanout/spelling-agent.jsonl` | **Authored** — the same shape spelling the **current** tool name (`Agent`). Support, not proof: an authored fixture asserts the matcher against itself, which is exactly how the rename went unnoticed for 203 spawns. The load-bearing assertion is the row below. |
| `transcript/fanout/no-recognised-spawn.jsonl` | **Authored** — tool-use blocks and **zero** recognised spawns. This is the one authoring cannot fake: whatever the matcher is taught to recognise, a transcript naming something else must read *could not observe* with the tool-call count in the reason, never `Absent`. Remove the negative-recognition arm and this goes red. |
