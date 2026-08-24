---
status: accepted
date: 2026-08-23
---

# The agent backend is declared by layout and snapshotted at dispatch

ADR-0008 declared `~/.grind/` the host's Grind directory with **its layout as the declaration** —
no config file, no format to parse, no environment override. Epic
[#135](https://github.com/FlorianRiquelme/grind/issues/135) adds a fact of exactly that kind: which
agent backend a Run executes through (`claude-code`, today's hardwired path, or `native`, the
grind-owned loop). Left to accumulate, it would have grown a config file mapping runs to backends
or an environment override per dispatch — the same drift toward per-fact mechanisms ADR-0008
opened with.

They are one decision. **The backend is one more line in the layout, and the RunRecord snapshot
taken at dispatch is the only thing a Run ever executes under.**

Recorded resolving the selection row of the #135 plan
(`docs/plans/2026-08-23-001-feat-agent-harness-adapters-plan.md`, R3).

## The grammar

One file, one line, no format beyond it:

```
~/.grind/agent          <backend>[ <base-url>]
```

- `<backend>` is `claude-code` or `native`. **Absent file = `claude-code`** — R2's
  byte-for-byte preservation is the default falling out of the layout, not a branch.
- The optional `<base-url>` token exists for `native` only: it is the test and self-host seam,
  pointing the loop at any OpenAI-compatible endpoint without touching the default
  `https://openrouter.ai/api/v1`. `claude-code` takes no extra token; a second token after
  `claude-code` is an unknown token.
- **Unknown tokens fail loud.** Read errors and parse errors are `Err`s, not defaults with a
  warning — the same refusal ADR-0008 records for a mistyped symlink target. A silently
  misparsed backend dispatches a Run under the wrong executor, and nothing downstream can see
  the difference.
- An empty file (or empty line) is the default. The file names a deviation, nothing else.

## Credentials stay env-only

The file names an endpoint, never a key. API credentials resolve from the environment —
`OPENROUTER_API_KEY`, then `OPENAI_API_KEY` — and the resolved `Endpoint{base_url, api_key,
model}` is **never serialized anywhere**: not into the record, not into a transcript, not into a
log line. This is the ADR-0008 shape applied to secrets: the environment is the only variable,
the same way `$HOME` is, and a Run record that leaked a bearer token would turn every `run.json`
into a credential store with worse retention than any vault.

The record therefore snapshots **backend + optional `endpoint_override`** and nothing else. A
re-entry under a different environment is visible (the key check fails loudly in doctor, R9)
rather than silently substituted.

## The snapshot is the Run's constitution

At dispatch, the supervisor resolves the selection once and writes `backend` and
`endpoint_override` into the RunRecord. From that moment the file is irrelevant to that Run:

- **Resume proceeds on the snapshot and never re-reads `~/.grind/agent`.** The plan's draft
  wording was *refuse on mid-run backend change*; the ruling here is sharper and simpler — there
  is nothing to refuse, because the Run's record already names what it runs under, and a record
  that re-consulted the filesystem on resume would be a Run whose identity depends on when you
  look at it. Mid-run switching is not approximated and not detected; it is *incoherent* —
  session identity, transcript location and denial semantics differ per backend — and the
  snapshot is what makes the incoherence unrepresentable rather than guarded against.
- This is the same move as ADR-0002's plugin version frozen per Run at dispatch, extended to the
  executor itself: every policy knob the Run will ever need is in the record by the time the
  first attempt starts.
- **The mirror obligation is explicit.** `RunView` deserializes the same JSON under
  `deny_unknown_fields` (ADR-0007's writer/reader wall). Every field added to `RunRecord` must
  be mirrored into `RunView` in the same change, or every *new* `run.json` fails deserialization
  on read. This is a load-bearing inconvenience: the wall that keeps the writer and reader
  blind to each other also keeps them honest about shape, and the mirror is the tax it charges.

## Grind-owned transcripts land in the run dir

The native adapter appends `messages-N.jsonl` — one file per attempt, mirroring the
`attempt-N.*` convention — into the Run's own directory under `~/.grind/runs/<run-id>/`. Lines
are `{"event": "<snake_name>", "value": {...}}`, identical shape to the POC's log: the variants
are `AssistantToolCalls`, `ToolResult`, `Usage`, `ProtocolNudge`, `ProtocolSelected`, `Final`
(see [ADR-0018](0018-tool-calling-is-capability-adaptive.md) for the protocol semantics the
events carry).

No code outside the claude-code adapter may derive `~/.claude/projects/` paths. Claude Code's
transcripts stay adapter #1's private input; observation of native Runs reads grind transcripts
from grind's own layout. A Run's evidence therefore lives in one directory tree regardless of
backend — which is what makes the P4 deletion verdict a one-PR cutover rather than an
archaeology project.

## Costs

- A second file joins `~/.grind/`, and with it a second thing provisioning must not get wrong.
  Like `bin/claude`, it is optional by default: its absence *is* the default backend.
- The one-line grammar has no room for comments, multiple backends, or per-stage routing. That
  is deliberate — tier routing is P4's verdict, decided by evidence, not a config feature
  smuggled in ahead of it.
- `endpoint_override` in the record is a URL, not a secret, but it is still topology: it
  reproduces in every resume. Changing the file mid-run changes nothing about running Runs —
  the point — which also means fixing a typo requires either finishing or abandoning the Run.
- The RunView mirror must be repeated for every future record field, by hand, forever. The
  compile-fail-style test that both types parse the same fixture is the carrier ADR-0007
  already named; forgetting the mirror fails at first read of a new record, not at review.

## ADR-0008 is extended, not amended

Nothing ADR-0008 ruled is withdrawn: `~/.grind/agent` is a new entry in the same layout
declaration, credentials ride the same "environment is the only variable" rule as `$HOME`, and
the loud-failure stance on unknown tokens is the same one applied to missing repos directories.
The extension is that the layout now declares *policy* (which executor) in addition to *host
facts* (where the binary, the repos, the locks live) — and that the declaration is consumed by
snapshot, so the filesystem states the intent once and the record owns the consequence.

The transport this selection feeds is [ADR-0016](0016-the-agent-harness-takes-a-vetted-sync-http-stack.md)'s
decision; the per-Run protocol behavior it gates is
[ADR-0018](0018-tool-calling-is-capability-adaptive.md)'s.

## Amendment (2026-08-24): a model *class* crosses the seam, and the grammar grows keys

Epic #135's own dogfooding surfaced a defect this ADR's original grammar had no way to avoid.
Grind routes every `fast`-classified stage to a hardwired literal — `"claude-sonnet-5"` — and
handed that string straight through to whichever backend the Run declared. On claude-code that
string is exactly right: it is the harness's own `--model` alias. On the native backend it is
wrong in every case: the native wire's `model` field is OpenRouter's `vendor/model` namespace,
so every `fast`-routed stage sent an Anthropic alias to an OpenAI-compatible endpoint and burned
three retries discovering the 400/404, on every attempt, with no way for an operator to avoid it
short of never using `fast` at all.

**The fix: a model *class* crosses the seam, never a concrete id.** A model id is a provider
fact — `vendor/model` on the native wire, a plain alias on claude-code's harness. The class
(`fast` or `strong`) is grind's own routing intent, decided the same way it always was
(`decide::Decision::model_per_stage`), and it is *that* — a `StageModel::Pinned(id)` for a
Job's verbatim `model:` pin, or `StageModel::Class(Fast | Strong)` for grind's own routing —
that now crosses from `supervisor::resolve_stage_model` into the seam. Each adapter resolves
its own concrete id from the class: claude-code maps `Class(Fast)` to the `claude-sonnet-5`
alias and `Class(Strong)` to no `--model` flag at all (the harness default, unchanged from
before this amendment — R2's byte-for-byte claim holds because the claude-code mapping is
exactly the literal the old code hardwired); the native adapter maps each class to the model id
the host declared for it, falling back to the same `DEFAULT_MODEL` constant it already used when
nothing was declared. A Job's pin still crosses verbatim to both adapters — the freeze still
beats the routing — but grind's own `fast`/`strong` intent no longer masquerades as a provider
fact it was never entitled to assert.

**The grammar grows to carry the declaration**, still one line, still no format beyond it:

```
<backend> [<base-url>] [key=value ...]
```

Keys: `base-url`, `model`, `fast`, `strong`, `proto`. `model=<id>` sets both classes at once;
an explicit `fast=`/`strong=` overrides one individually — an operator naming one class without
committing to the same id for the other. The bare base-url positional token — no `=` in it —
still parses exactly as it did before this amendment; a token containing `=` is a key/value.
Every failure mode stays loud, on the same register `Backend::parse` already set for an unknown
backend: an unknown key, a duplicate key, a `key=` with an empty value, or a `proto=` value that
is neither `native` nor `text`, all refuse rather than silently defaulting or taking the first.

**`proto=` exists because probing is not free, and one upstream proved the probe is not even
informative.** R5's default is to probe once per Run and latch what the probe found. The
epic's own proof-of-concept ran `stealth/ox-alpha` and found it "unable to execute native tool
calls at all" — not flaky, not occasionally rejecting `tools`, but never able to use them. Every
Run against a host permanently declared to that model wastes one failed request per Run
discovering a fact the operator already knows. `proto=native` / `proto=text` names the wire mode
outright and skips the probe entirely, latching from the declaration before the first request
goes out — the same latch shape R5 already uses for a resumed attempt's inherited protocol, just
sourced from the layout instead of from a transcript. The transcript still carries a
`ProtocolSelected` event naming the declaration as its reason, so nothing about *how the mode
was chosen* becomes unobservable, and `scan_latch`'s resume path keeps working unchanged: a
declared run always re-derives the same latch on every attempt, so a resumed attempt reading an
earlier attempt's transcript agrees with a fresh one reading the declaration directly.

**The mirror obligation, again.** `fast_model_override`, `strong_model_override` and
`proto_override` join `backend`/`endpoint_override` on `RunRecord`, snapshotted at dispatch from
the same selection read, `#[serde(default, skip_serializing_if = "Option::is_none")]` for the
same reason: undeclared is the honest answer for every record written before the grammar grew
these keys, never a blank one. Each needs the same read-only mirror on `view::RunView` this
ADR already obligates for every future record field — left for the module that owns `view.rs` to
land, since the writer and the reader are deliberately blind to each other (ADR-0007's wall) and
this amendment's author does not own that side of it.
