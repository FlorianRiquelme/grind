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
  doing             Coherence review returned clean — zero findings. Waiting on…
  progress          newest write 28s ago
  fan-out           1 agent: feasibility reviewer  (newest write 28s ago)
```

- **`doing`** is the transcript's last assistant message, one line — the direct answer to
  *what is it doing right now*. Read fresh from the session transcript on every call;
  mid-attempt, this is the field that moves.
- **`progress`** is seconds since the newest write across the parent transcript **and every
  fan-out subagent transcript**. A quiet parent during a fan-out is healthy, not stuck.
- A `?` means *could not observe*, never a verdict. Report it as blindness, not as a state
  of the work.

### On a native Run

The block above is a claude-code Run. A Run dispatched on the `native` backend (ADR-0017)
answers the same fields off grind's own `messages-N.jsonl` transcripts, with three
differences worth knowing before you read one:

```
  now               work
  doing             bash {"command": "just verify"}
  progress          newest write 4s ago
  fan-out           ?
```

- **`now`** is the *rung*, not a plugin skill path — the name the stage prompt's own
  frontmatter declared (`work`, `ship`, …). There is no `attributionSkill` to read.
- **`doing`** is the last thing the model itself authored, and on a native Run that is
  usually a **tool call** rather than prose: an attempt's assistant turns are tool calls
  until the final one. A tool *result* never fills this field — that is the world talking.
  Tool results do appear in `last words`, which is the two sides interleaved.
- **`fan-out`** is always `?`, and the reason on the value says why: the native loop has no
  tool to spawn a subagent with. It is a standing fact about the loop, not an unwritten
  reader — and not `none`, which in this view would claim spawns that all returned.

## Going deeper

The view's trailing block names the resolved transcript path (`transcript`) and the record
(`run state`). Tail the transcript directly when one line is not enough; subagent transcripts
live under `<transcript-stem>/subagents/*.jsonl`.

On a native Run, `transcript` names the **newest-written** `messages-N.jsonl` under the Run's
own directory — the attempt in flight. Earlier attempts' files sit beside it, and `progress`
spans all of them.

## Boundaries

- **Status is pull-only and writes nothing.** Never save what `grind status` loads; the
  supervisor is the only writer of `run.json`, and a read path that persists can erase
  `attempts[]` while the human watches the dashboard (issues #12, #27).
- **Verdict language describes what happened, never quality** (ADR-0003). A completed Run
  means the pipeline finished. Nothing in the view gates anything.
