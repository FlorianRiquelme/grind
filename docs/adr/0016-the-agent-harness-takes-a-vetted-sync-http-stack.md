---
status: accepted
date: 2026-08-23
---

# The agent harness takes a vetted sync HTTP stack

Epic [#135](https://github.com/FlorianRiquelme/grind/issues/135) adds a grind-owned agent loop
(`NativeAdapter`) speaking an OpenAI-compatible `/chat/completions` endpoint directly, instead of
delegating every turn to the foreign `claude` binary. That loop makes network I/O a load-bearing
path of the supervisor for the first time: every stage of every attempt is now a streaming HTTPS
POST, and misreading its failures misclassifies Runs exactly where the base has already been
burned once — ADR-0005's 2026-08-06 amendment records *mistaking a rate limit for a crash* burning
eight attempts in under a minute.

Three facts had accumulated with nothing owning them — the dependency ruling says JSON only, the
module map says `std::net` lives in `serve`, and a proof-of-concept crate
(`~/Repos/mine/grind-poc`, outside this repo) had recorded a failure taxonomy no amount of design
could have derived. They are one decision: **`src/net.rs` owns a vetted, synchronous,
connection-pooled HTTP stack (`ureq` 2.x, rustls), and failure classification of the wire itself
lives at that layer.**

Recorded resolving the transport row of the #135 plan
(`docs/plans/2026-08-23-001-feat-agent-harness-adapters-plan.md`), which named this ADR before the
evidence below was written down.

## What this amends, and how far

**ADR-0005 is amended for the harness module only.** Its ruling was "`serde` is the only
dependency", chosen when every byte Grind touched came from `std::process` pipes. A native agent
loop cannot be built on `std`: there is no HTTPS client in std, and hand-rolling TLS would be the
opposite of vetted. The amendment is scoped deliberately: `net.rs` may depend on `ureq`;
nothing outside that one module gains a dependency budget. `serde_json` remains the only dependency
the rest of the crate needs, and the ruling's rationale — silent failure made unrepresentable —
is inherited, not relaxed: the point of the stack choice below is that upstream lies become loud
`NetError`s rather than clean stops.

**ADR-0007 is amended by addition.** Its cut sanctioned exactly one network module, `serve`, as
sole namer of `std::net`. `net.rs` becomes the **second sanctioned network region**, same
rule, different socket: serve listens, net dials. Until `net` first needs to name `std::net`
directly — today `ureq` keeps the spelling out of the tree entirely, which is the stronger
fact — the naming test is untouched; when that day comes, the test gains a second allowed
site by the same amendment mechanism that put `serve` on its list, never an exemption entry.
Tool subprocesses and file effects inside the adapters go through the existing `world`
functions; they do not become network regions and net does not grow filesystem hands.

## Considered options

| Option | Verdict | Trade-off |
|---|---|---|
| **`ureq` 2.x, sync, rustls (64 new crates)** | **Chosen** | Blocking API matches the supervisor's own shape — `policy` returns `Next::SleepThenReenter(Duration)` precisely so the loop is the only thing that blocks, and a blocking client keeps that true without an executor. Connection pooling amortizes TLS across turns. **Cost:** the dependency weight ADR-0005 ruled out, now paid once and fenced inside one module. |
| Subprocess `curl` per request | Rejected | Zero dependencies, and evidence killed it: the POC's turn loop is one HTTPS round trip per model turn, so curl-per-request pays a fresh TCP + TLS handshake *every turn* of *every stage* — a fixed tax on each of dozens of calls, plus argv-quoting of the request body through a shell surface for no gain. Pooling is the whole point of owning the socket. |
| Async runtime (tokio + reqwest/hyper) | Rejected | An executor, a timer, and a task ecosystem to stream SSE that one blocking loop consumes line by line. Nothing in grind is concurrent at this seam — one stage, one stream, sequential turns — so async buys latency grind cannot use and adds a second concurrency model beside the thread-based supervisor. |

## The failure taxonomy decides where classification lives

The POC ran real write→compile→run→verify stages end-to-end through OpenRouter on two models and
recorded how providers actually fail. Three signatures were observed, none derivable:

1. **Masked upstream failure.** OpenRouter maps provider-side faults to
   `finish_reason:"stop"` with an empty delta and stashes the real cause in
   `native_finish_reason:"network_error"`. Read `finish_reason` alone and a dead turn parses as a
   clean completion — the exact class ADR-0005's Python script died of, arriving through a
   different door.
2. **Deterministic tools-array rejection.** Some upstreams (`stealth/ox-alpha` via OpenRouter)
   fail *every* request carrying a native `tools` array — an HTTP-level rejection with nothing
   intermittent about it, invisible until the first tool-bearing turn.
3. **Empty stub responses.** Streams that end legitimately, `[DONE]` and all, having produced no
   content and no tool calls.

This is why `NetError` is an enum — `Http{status, body}`, `Stream(String)`,
`AbnormalFinish{finish, native}`, `EmptyResponse` — and why the validation rules live in
`net.rs` rather than in the adapter: `finish_reason` must be one of `stop`/`tool_calls`,
`native_finish_reason` if present must be one of `stop`/`tool_calls`/`end_turn`/`stop_sequence`,
and anything else is `AbnormalFinish`. An empty guard after the loop catches signature 3. The
adapter above net therefore receives classified failures and maps them onto the loud-failure rule
(R6): failed Attempts with `terminal_reason` set, never clean stops. Classification below the
seam would hand every caller the raw stream and re-derive the taxonomy per consumer.

Two structural rules ride with it:

- **net carries no retry logic.** Per-turn retries with bounded backoff are the caller's policy;
   a client that retries internally hides the flake boundary from the transcript and the nudge
   accounting of [ADR-0018](0018-tool-calling-is-capability-adaptive.md).
- **The SSE assembly is pure and tested inline**: split-name chunks, out-of-order tool-call
  indices, comment keepalives (`: OPENROUTER PROCESSING`), abnormal native finish reasons,
  mid-stream error chunks, missing `[DONE]`, empty responses. Reading past `finish_reason` is
  legal — usage follows the finish — and EOF without `[DONE]` is legal too.

`probe_endpoint(ep) -> bool` — GET `{base}/models`, true iff connection-level success at any
status, 5-second timeout — exists for doctor's reachability check (R9), not for dispatch-time
probing.

## Costs

- The lockfile went from 12 packages to 76 — 64 new crates, including `ring`, `rustls`, the
  ICU/idna tree, `flate2` and a build-time `cc`, all linked into a prebuilt musl static binary
  — once, permanently, even if `native` is never selected. The fence is the mitigation:
  `cargo tree` shows the weight, the module shows the surface.
- `ureq`'s pooled agent holds sockets open across turns. A stage that dies between turns leaks
  the pool into process exit — harmless today, worth remembering if adapters ever share a
  long-lived process differently.
- Sync means one slow stream blocks the whole supervisor thread for its duration. True today by
  construction (one dispatch lock, one attempt at a time); revisiting requires a concurrency
  decision grind has not made, not a runtime swap.
- The finish-reason allowlists encode observed OpenRouter behavior. A provider that surfaces
  legitimate finishes outside `{stop, tool_calls}` / `{end_turn, stop_sequence, …}` will fail
  loud and need the list extended — deliberate: unknown endings are errors until seen, which is
  the only safe reading while masked failures exist.
- `probe_endpoint` accepts *any* HTTP status as reachable, so a 401-ing endpoint reads green at
  the connection layer. Key presence is checked separately (see
  [ADR-0017](0017-the-agent-backend-is-declared-by-layout-and-snapshotted-at-dispatch.md)'s
  env-only credentials); the two checks together are doctor's honest answer, not a guarantee.

## Amendments recorded alongside this

[ADR-0017](0017-the-agent-backend-is-declared-by-layout-and-snapshotted-at-dispatch.md) places the
selection and credential rules this stack serves;
[ADR-0018](0018-tool-calling-is-capability-adaptive.md) consumes `NetError`'s taxonomy as the
trigger for protocol latching. Separate decisions, same ticket (#135); all three are terms of the
one epic.
