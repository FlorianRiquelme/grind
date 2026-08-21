# Observing a Run

How to answer *what is the Run doing* without digging through session transcripts by hand.
Grind already reads them for you; the answer is one command away.

## The two commands

```sh
grind status              # roster: every Run on this host, with run ids
grind status <run-id>     # one Run's live view
```

Run state does not travel between hosts. A run id this host has never held answers
*not here* and points at the Job issue, which carries the pointer to the host that holds it.

## Reading the live view

The observation block, top to bottom, is the answer to *how is it going*:

```
  verdict           unobserved — …        # what happened, never quality (ADR-0003)
  furthest stage    dispatched
  now               compound-engineering:ce-work
  live              Coherence review returned clean — zero findings. Waiting on…
  progress          newest write 28s ago
  fan-out           1 agent: feasibility reviewer  (newest write 28s ago)
```

- **`live`** is the transcript's last assistant message, one line — the direct answer to
  *what is it doing right now*. Read fresh from the session transcript on every call;
  mid-attempt, this is the field that moves.
- **`progress`** is seconds since the newest write across the parent transcript **and every
  fan-out subagent transcript**. A quiet parent during a fan-out is healthy, not stuck.
- A `?` means *could not observe*, never a verdict. Report it as blindness, not as a state
  of the work.

## Going deeper

The view's trailing block names the resolved transcript path (`transcript`) and the record
(`run state`). Tail the transcript directly when one line is not enough; subagent transcripts
live under `<transcript-stem>/subagents/*.jsonl`.

## Boundaries

- **Status is pull-only and writes nothing.** Never save what `grind status` loads; the
  supervisor is the only writer of `run.json`, and a read path that persists can erase
  `attempts[]` while the human watches the dashboard (issues #12, #27).
- **Verdict language describes what happened, never quality** (ADR-0003). A completed Run
  means the pipeline finished. Nothing in the view gates anything.
