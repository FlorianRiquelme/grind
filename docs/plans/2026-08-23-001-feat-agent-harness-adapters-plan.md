---
title: "feat: agent harness adapters — grind owns the loop, pluggable backends"
date: 2026-08-23
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
execution: code
origin: session 2026-08-23 (user-directed: non-Claude models via an owned harness; OpenRouter and OpenAI concrete; never other agent CLIs)
deciding_adrs:
  - docs/adr/0005-the-base-is-a-compiled-rust-binary.md
  - docs/adr/0007-side-effects-live-in-one-module.md
  - docs/adr/0008-the-host-is-declared-by-its-layout.md
  - docs/adr/0010-spend-is-recorded-never-bounded.md
planned_adrs:
  - ADR-0016 the agent harness takes a vetted sync HTTP stack (amends the serde-only reading of 0005 for the harness module)
  - ADR-0017 the agent backend is declared by layout and snapshotted at dispatch (extends 0008)
  - ADR-0018 tool calling is capability-adaptive (native function-calling with a text-protocol fallback)
---

# Agent harness adapters — grind owns the loop

**Product Contract preservation:** ADR-0001 (grind is a scheduler, not a pipeline) and
ADR-0004 (the supervisor re-enters rather than predicts) rule unchanged. This plan adds a
second executor *behind* the supervisor's existing re-entry mechanics; it does not turn
grind into an interactive agent.

## Summary

Today every stage executes through one hardwired path: `attempt.rs` builds `claude -p`
argv, `world::spawn_recorded` runs it, and observation reads Claude Code's transcripts out
of `~/.claude/projects/`. This plan extracts that path behind one internal seam
(`StageRunner`), keeps today's behavior as the first adapter (`ClaudeCodeAdapter`), and
adds a second adapter — a **grind-owned agent loop** speaking any OpenAI-compatible
endpoint (OpenRouter first, OpenAI by base-url swap) with grind-defined tools, grind-owned
transcripts, and grind-enforced permissions. Selection is declared by layout
(`~/.grind/agent`), snapshotted into the RunRecord at dispatch like every other policy
knob, and honored verbatim on resume.

Evidence basis: a throwaway proof-of-concept crate (`~/Repos/mine/grind-poc`, outside this
repo) ran a real write→compile→run→verify stage end-to-end through OpenRouter on two
models — `deepseek/deepseek-chat-v3.1` via native function calling (~56 s) and
`stealth/ox-alpha` via text-protocol fallback after its upstream proved unable to execute
native tool calls at all. All failure taxonomy in this plan is observed there, not assumed.

---

## Problem Frame

Grind's model freedom ends where Claude Code begins. The stages are billed, routed and
classified against a foreign binary whose lifecycle, transcript format and permission
model grind neither owns nor sees. Three costs follow: (1) no non-Anthropic models on any
tier; (2) permission denials arrive as *recorded history* to be interpreted (policy.rs)
instead of gateable events; (3) the transcript schema — the thing `view.rs` slugs out of
`~/.claude/projects/` and matches field-by-field — is undocumented and can change under
us. An owned loop fixes all three but must not regress what Claude Code provides for free
(harness prompt maturity, compaction, edge-case hardening); hence adapters behind one
seam, evidence before any default flips, and deletion only by verdict.

---

## Requirements

- **R1 — One seam.** Exactly one internal boundary decides how a stage executes. Downstream
  consumers (supervisor, policy, observe, serve, render) are backend-blind: no `if
  backend == …` branch anywhere outside the adapter modules.
- **R2 — Today's behavior preserved.** With no `~/.grind/agent` file, dispatch, resume,
  denial handling, rate-limit classification, spend recording and doctor output are
  byte-for-byte today's behavior. The default backend is and stays `claude-code`.
- **R3 — Backend selection by layout.** `~/.grind/agent` holds one line naming the backend
  (`claude-code` or `native`). Absent file = default. The choice is snapshotted into the
  RunRecord at dispatch and resume honors the snapshot; mid-run backend switching is
  incoherent (session identity, transcript location and denial semantics differ) and is
  refused, not approximated.
- **R4 — Grind-owned transcripts.** The native adapter appends `messages.jsonl` under the
  Run's own directory with grind-defined events (`assistant_tool_calls`, `tool_result`,
  `usage`, `protocol_nudge`). Observation reads grind transcripts for native Runs; no code
  outside the claude-code adapter may derive `~/.claude/projects` paths.
- **R5 — Capability-adaptive tool calling.** The native adapter supports two wire modes:
  `native` (OpenAI `tools` parameter) and `text` (tools described in the system prompt,
  `<tool>{…}</tool>` invocations parsed from content, `<done>` sentinel as the only legal
  termination). Mode is selectable and defaults to probing once then latching per backend.
- **R6 — Loud failure.** Upstream provider failures must never classify as clean stops.
  Validated on both `finish_reason` and `native_finish_reason`; empty responses (no
  content, no calls) are errors; each turn retries with bounded backoff before the attempt
  fails toward the could-not-answer register.
- **R7 — First-party enforcement.** In the native adapter, tool calls are gated *before*
  execution and denials are structured events naming the gating layer — feeding policy.rs
  as first-class facts rather than post-hoc transcript archaeology.
- **R8 — Spend recorded, never bounded (ADR-0010 carries over).** Usage from every API
  response lands in the transcript and the Attempt, exactly as `total_cost_usd` does today.
- **R9 — Doctor knows both.** Per-backend host checks: `Check::ClaudeBinary` remains for
  claude-code; the native backend adds key-present and endpoint-reachable checks. Both
  green = both selectable; a selected-but-unready backend refuses dispatch in the
  could-not-answer register.

---

## Key Technical Decisions

| Decision | Ruling | Trade-off accepted |
|---|---|---|
| Adapter seam shape | One `StageRunner` boundary: `run(stage_prompt, model_profile, proto_mode, session) -> Attempt` plus a normalized event stream. Normalization happens **inside** adapters; downstream sees one schema forever. | Two adapters exist during the transition; the alternative (dual parsers in view.rs) is the known maintenance death. |
| Transport | `ureq` (sync, rustls, ~a dozen crates) inside one network module behind a test-injectable seam. **Supersedes the serde-only reading of ADR-0005 for the harness module only** — ADR-0016 records the amendment with the observed failure taxonomy (masked upstream failures, flaky network errors, broken native tools on stealth/ox-alpha) and connection-reuse economics vs subprocess curl. | Dependency weight; rejected curl-per-request because fresh TLS handshakes per turn tax every stage call. |
| Wire format | One surface: OpenAI-compatible `/chat/completions`. No per-provider trait zoo — OpenRouter and OpenAI differ by `{base_url, key_env, model_id}` only. | Anthropic-native features ride OpenRouter's compatibility skin or don't happen. |
| Tool protocols | `native` + `text` (R5). `<done>` mirrors the existing `<promise>DONE</promise>` convention; prose without a tag triggers exactly one corrective nudge per turn budget. | Text mode spends tokens on protocol prose; native mode is unusable on some upstreams — duality is the point. |
| Termination & completion | Explicit sentinels only (`<done>`, native `stop` with content). Empty responses are failures (R6), mirroring the ADR-0005 lesson that silent-completion failure modes must be unrepresentable. | — |
| Subagents | Fan-out becomes a grind-defined tool mapping onto grind's own run machinery (uniform observation). Replaces scraping `FANOUT_TOOLS=["Agent","Task"]` from foreign transcripts. | Later unit; not in the first native cut. |
| Context management v0 | Append-only messages, truncate oversized tool outputs, no compaction. Stages are bounded; revisit when evidence shows window pressure. | Long stages may hit windows earlier than Claude Code would. |

## High-Level Technical Design

```text
                 ┌─ ClaudeCodeAdapter   argv builders + payload classifier + CC transcript
                 │                       reading (moved, not rewritten) → grind events
StageRunner ─────┤
                 └─ NativeAdapter       agent loop → ureq client → tools registry →
                                         ~/.grind/runs/<id>/messages.jsonl
                                   │
     supervisor · policy · observe · serve · render   (backend-blind)
```

Module placement follows existing seams: adapters in new `src/runner/` (claude.rs,
native.rs, mod.rs holding the trait); the HTTP client isolated in `src/runner/net.rs`
(ADR-0007 spirit: side effects — now including network — live in one module); transcript
event types shared in `src/runner/events.rs`.

## Implementation Units

### P1. The seam (behavior-preserving)

Extract `StageRunner`; move `build_stage/build_reflect/ci_babysit`, the result-payload
classifier (`classify`/`parse_payload`), `is_rate_limited` needles and the
`view::transcript_path` discovery + JSONL matchers behind `ClaudeCodeAdapter`.
`RunRecord` gains a snapshotted backend field defaulting to `claude-code`. All existing
tests pass untouched; hermetic fake-claude scenarios unchanged. **No `~/.grind/agent`
handling yet.**

### P2. The native adapter (default-off)

`NativeAdapter`: turn loop, `ureq`-based streaming client (SSE line parser, index-keyed
`tool_calls` delta assembly, native-finish-reason validation, empty-response guard,
bounded retries), tools `bash` / `read_file` / `write_file` (+ `glob`/`grep` when stages
demand), both proto modes (R5), `messages.jsonl` (R4), `~/.grind/agent` selection +
snapshot + resume-refusal (R3), doctor checks (R9). Tests: scripted-SSE mock server as a
hermetic fixture alongside `tests/fakes/bin` fakes (a fake `curl`-shaped endpoint shim);
identical scenario shapes executed against **both** adapters asserting equivalent
observable outcomes — drift is caught by CI, not archaeology.

### P3. Dogfood and measure

Real issues dispatched on both backends across the stage ladder; collect per-stage outcome
quality, cost (usage recorded per R8), wall time, retry/nudge rates. Cheap-strong models
first (`deepseek/deepseek-chat-v3.1` class); ox-alpha-class models exercise the text
protocol.

### P4. Verdict

Evidence decides, in this order of preference: keep both with tier routing (`strong` →
claude-code, `fast`/classification → native), flip the default and keep claude-code
selectable, or delete `ClaudeCodeAdapter` (one-PR cutover — P1 already isolated it).
Deletion requires the native adapter matching or beating the baseline on the dogfooded
stage mix, recorded here.

## Non-goals

- No interactive agent, no TUI chat (ADR-0001 stands).
- No config file format; selection extends the `~/.grind` layout declaration (ADR-0008).
- No MCP/third-party tool servers in this pass (candidate for a later ADR once the built-in
  registry proves itself).
- No context compaction in v0; no embedding/vector memory; no multi-endpoint failover
  within a single attempt.

## What to watch

- **Provider flakiness asymmetry**: the POC showed per-upstream failure signatures differ
  (masked `network_error`, empty stubs, deterministic tools rejection). The adapter must
  keep failing loud and specific, or policy will mis-sleep on rate limits — the exact bug
  class ADR-0005's amendment warns about.
- **Text-protocol drift**: models wrap tags in fences, emit prose mid-task, or invent
  fields. The nudge-and-budget mechanic bounds this; log every `protocol_nudge` so drift
  rates are comparable across models in P3.
- **Resume coherence**: a native Run resumed after a backend file change must proceed on
  its snapshotted backend or refuse — never silently translate sessions.
