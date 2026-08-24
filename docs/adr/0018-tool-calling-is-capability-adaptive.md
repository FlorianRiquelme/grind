---
status: accepted
date: 2026-08-23
---

# Tool calling is capability-adaptive

Epic [#135](https://github.com/FlorianRiquelme/grind/issues/135)'s native adapter must call tools
— grind-defined `bash`, `read_file`, `write_file`, gated first-party (R7) — through an
OpenAI-compatible chat endpoint. The POC (`~/Repos/mine/grind-poc`) found that "OpenAI-compatible"
does not mean *uniformly* capable: `deepseek/deepseek-chat-v3.1` executed native function calling
end to end in ~56 s, while `stealth/ox-alpha`'s upstream failed **every** request carrying a
`tools` array, deterministically. A single wire mode is therefore either unusable on some models
or second-class on all of them.

The decision: **the adapter speaks two wire modes and picks one per Run by probing the endpoint,
then holds it.** Mode is not configuration; it is a measured capability.

Recorded resolving the tool-protocols row of the #135 plan
(`docs/plans/2026-08-23-001-feat-agent-harness-adapters-plan.md`, R5).

## The two modes

**`native`** — the OpenAI `tools` parameter. The model returns structured `tool_calls`; results
go back as `role:"tool"` messages; a `stop` finish with content is completion. This is the
first-choice mode wherever the upstream actually supports it.

**`text`** — tools described in the system prompt; the model calls them by emitting exactly one
tag per reply:

```
<tool>{"name":"…","arguments":{…}}</tool>
```

Results return as user messages wrapped in `<tool_result call="…">`. Termination is **only** the
`<done>…</done>` sentinel, whose inner text becomes the final summary — deliberately mirroring
the existing `<promise>DONE</promise>` convention in `attempt.rs`: an explicit, greppable
termination token rather than inferred completion from silence or prose. Prose without a tag is
not a tool call and not a completion; it earns one corrective nudge (below).

The duality is the point: text mode spends tokens on protocol prose but works on any model that
can follow an instruction; native mode is cheap on the wire but provably absent on some
upstreams.

## Probe once, latch per run

The mode is determined once per Run, then latched:

1. On the first native-adapter attempt of a Run, scan the run dir's prior `messages-*.jsonl` for
   a `ProtocolSelected{mode, reason}` event. Found → honor it; the latch already exists.
2. Otherwise probe `native` once. Evidence for falling back to `text`: an HTTP-level rejection
   mentioning tools (the deterministic tools-array failure), or an abnormal finish while tools
   were present.
3. Whatever the outcome, log `ProtocolSelected{mode, reason}` as the first protocol event of the
   transcript. The latch then holds **for the rest of the Run** — every attempt, every turn.

Latching per Run rather than deciding per turn keeps a Run's evidence homogeneous: its
transcripts are comparable across attempts, its nudge and retry rates are attributable to one
protocol, and P3's dogfooding can measure drift per model per mode instead of untangling modes
mixed mid-Run. It also matches [ADR-0017](0017-the-agent-backend-is-declared-by-layout-and-snapshotted-at-dispatch.md)'s
snapshot rule — backend frozen at dispatch, protocol frozen at first contact; nothing about a
running Run re-consults the world mid-flight.

## Loud failure at the protocol layer too

The failure taxonomy [ADR-0016](0016-the-agent-harness-takes-a-vetted-sync-http-stack.md) places
in `net.rs` feeds directly into this ruling:

- **Empty response = error, not completion.** A stream that ends cleanly with no content and no
  tool calls is R6's loud-failure class, same as Claude Code's empty stdout was. The
  silent-completion failure mode stays unrepresentable: there is no path from an empty turn to
  `done_promise`.
- **Finish reasons are validated jointly.** `finish_reason ∈ {stop, tool_calls}` AND
  `native_finish_reason`, when present, `∈ {stop, tool_calls, end_turn, stop_sequence}`;
  anything else is `AbnormalFinish` and fails the attempt. Reading only the OpenAI-level field
  would have classified the POC's masked `network_error` turns as clean stops.
- **Turn budget is a failure.** Exhausting the 32-turn budget is a failed Attempt with
  `terminal_reason` set, never a clean stop. Per-turn network retries are bounded (3 attempts,
  2s × attempt backoff) before the attempt itself fails toward could-not-answer.

## One corrective nudge per occurrence, always logged

In text mode, a reply that is neither a `<tool>` tag nor a `<done>` sentinel gets exactly one
corrective nudge — a user message restating the protocol — per occurrence. Nudges do not stack
and are not escalated; the budget above bounds what repeated confusion can cost. Every nudge is
logged as `ProtocolNudge{assistant_text}` before the corrective message is sent.

The logging is not bookkeeping. Text-protocol drift — fenced tags, prose mid-task, invented
fields — is the known failure surface of this mode, and P3's verdict needs nudge *rates* per
model to decide whether the text protocol is a fallback or a liability. An unlogged nudge is a
signal discarded at the exact moment it was cheapest to record; the transcript is the
measurement instrument.

## Costs

- Two protocols to test. Mitigated by parity scenarios in CI running identical shapes against
  both adapters and asserting equivalent Attempt outcomes — drift is caught by the fixture, not
  archaeology.
- Text mode's parsing surface (tag extraction, fenced tags, one-tag-per-reply) is exactly where
  weaker models wobble, so the mode that rescues incapable upstreams demands more of them. The
  nudge budget converts that wobble from hangs into bounded, measurable friction.
- The probe costs one wasted request on upstreams that cannot do native tools — one HTTP round
  trip, once per Run. Charging it to every Run on healthy models buys the latch its evidence;
  per-model config tables were rejected as capability declarations going stale faster than
  endpoints change.
- The joint finish-reason allowlist hard-codes observed OpenRouter behavior (see ADR-0016's
  costs). A new legitimate native finish string fails loud until admitted — correct while
  masked failures exist.
- Latching means one bad probe condemns a whole Run to text mode even if the upstream's native
  support was flapping rather than absent. Accepted: a flapping upstream is indistinguishable
  from a broken one at probe time, and retrying probes per turn reintroduces mixed-mode Runs.

## Amendments recorded alongside this

[ADR-0017](0017-the-agent-backend-is-declared-by-layout-and-snapshotted-at-dispatch.md) owns where
these events live (`messages-N.jsonl` in the run dir) and why the latch can trust the run dir;
[ADR-0016](0016-the-agent-harness-takes-a-vetted-sync-http-stack.md) owns the transport and the
failure classification this ruling consumes. Separate decisions, same ticket (#135); all three
are terms of the one epic.
