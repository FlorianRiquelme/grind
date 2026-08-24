# Findings from the first native-backend Runs

Two Runs against [grind#138](https://github.com/FlorianRiquelme/grind/issues/138) — "the ADR count
in README.md and CLAUDE.md is three behind docs/adr/", a deliberately trivial docs Job filed to
carry the first Dispatch through the `native` backend added by
[PR #136](https://github.com/FlorianRiquelme/grind/pull/136). Same Job, same ladder, same three
tools, two different models:

| | Run A | Run B |
|---|---|---|
| Run id | `20260824-155324-grind-138` | `20260824-160445-grind-138` |
| Declaration | `native` (bare) | `native model=stealth/ox-alpha` |
| Model resolved | `deepseek/deepseek-chat-v3.1` (`DEFAULT_MODEL`) | `stealth/ox-alpha` |
| Recorded state | `died` (killed by hand mid-attempt 2) | `uncorroborated` |
| Plan stage | `incomplete`, 32 turns | **`complete`, 14 turns** |
| True spend | $0.048594 | $0.000000 (uncharged while cloaked) |

**Everything between the wire and one completed stage works. The ladder does not, and the cause is
one boolean.** The transport, the streaming parser, capability probing, tool calling, the layout
declaration, the dispatch snapshot, the provenance freeze, the stage-return contract and the
`view::Live` reader all did exactly what they were built to do, on a backend that had never run
before. Then the Run terminated after its first rung, because the native adapter reports a
finished *stage* as a promise that the whole *Run* is done. This is a P2 gap that P2's own tests
could not see: they exercise one stage at a time, and the defect only exists between two.

## Outcome

| | |
|---|---|
| What happened | Run A livelocked in Plan and was killed; Run B completed Plan, then terminated one rung into a ten-rung ladder |
| What the record says | A: `died`, `plan: incomplete` · B: `uncorroborated(["PR open", "commits ahead", "PR head matches Job branch", "PR base matches declared branch"])`, `plan: complete` |
| PR opened | none, by either Run |
| Commits | none, by either Run — both worktrees left clean |
| Attempts | A: 1 recorded of 14 (a second was in flight at the kill) · B: 1 of 14 |
| Wire protocol | `native` on both, latched by probe (`"probe succeeded: endpoint accepted the tools array"`) |
| Provenance | `binary_version 0.1.0`, `skills_hash ef17554580a19b6a`, frozen at dispatch on both |
| Tool denials | 0 |
| Fan-out | `could not observe` on both — correct; the native loop has no tool that spawns a subagent |

## The blocker: a finished stage is reported as a finished Run

`src/native.rs`, in `synthesize`:

```rust
let (exit_code, is_error, done_promise, spoken) = match facts.ending {
    Ending::Completed(text) => (Some(0), false, true, text),
    Ending::Failed(reason)  => (Some(1), true, false, reason),
};
```

Every native Attempt whose loop ends normally sets `done_promise: true`. But `done_promise` does
not mean *this Attempt finished*; it means *the Run claims to be done*. The claude-code adapter
requires the claim to be spoken outright — `src/claude.rs`:

```rust
done_promise: result.contains("<promise>DONE</promise>"),
```

A literal sentinel, which the agent emits or does not; `attempt.rs`'s own test is named
`the_done_promise_is_read_from_the_result_and_nowhere_else`, and a recorded Run 1 fixture carries
the marker verbatim. A Claude Code stage that merely returns promises nothing, and that is
precisely what lets the supervisor walk ten rungs: each stage returns, `rung::next` reads the
returns off disk, and the next rung starts.

The native adapter has no such gate. `Ending::Completed` means *the loop reached its end*, which
is true at the end of every stage, and it is being read as the claim.

Run B's log is the whole defect in three lines:

```
[2026-08-24T16:04:47+00:00] plan attempt 1 (dispatch) …
    -> DONE promised | stage=dispatched | commits=0 | cost=$0.00 | Uncorroborated([...])
  [reflect] attempt 1 (dispatch) …
```

Plan completed, wrote `{"status": "complete"}` to `stages/plan.return.json` and produced both of
its artifacts (`stages/plan/anchor-plan.md`, `stages/plan/plan-facts.json`). Then, because the
last Attempt "promised DONE", `decide::verdict` was asked whether the Run was finished, found no
PR and no commits — correctly, Plan does not produce either — and returned `Uncorroborated`, which
is terminal. Reflect ran post-terminal and the Run ended.

**Nothing about the ladder machinery is broken.** `plan.return.json` was on disk and `rung::next`
would have returned `Stage::Triage` had it been asked. It was never asked, because the Run had
already been declared over.

The repair follows from the asymmetry: **the native loop should read the same sentinel out of its
own final text** rather than synthesising a promise from the ending. One expression, the same
semantics on both adapters, and the stage skills already in `~/.grind/skills/run/` need no change
because they were authored against that sentinel to begin with. Run completion then arrives the
way it does on claude-code — `rung::next` returning `None` plus the observation tail, a path the
supervisor loop already has (`None => true`, falling through to the same verdict tail the legacy
loop uses).

Worth stating as a property rather than a patch, because a future adapter will face the same
question: **a Run-level promise must be spoken by the agent, never inferred from an adapter's
control flow.** An ending is a fact about the loop; a promise is a claim about the work.

## The cost field is zero on purpose, and that is the trap

`grind status` reported `spend $-0.00` for both Runs. The record explains why, twice over:

| | Run A |
|---|---|
| `attempts[0].total_cost_usd` — what the roster and status sum | `0.0` |
| `attempts[0].usage.cost` — what OpenRouter charged | `0.03183387` |

`synthesize` hardcodes `total_cost_usd: Some(0.0)`, and the comment above it is right about why it
is not `None`:

> `None` here would make `Attempt::is_wait()` false for every native Attempt regardless of
> `num_turns`, which lets a first-turn rate limit spend the attempt budget and keeps
> `trailing_waits` permanently at 0 — the Run 2 failure ADR-0002/0004 exist to prevent.

So the field is deliberately `Some(_)` and deliberately not absent. What it is not is *true*. Any
fix has to carry the real `usage.cost` while staying `Some(_)` — "wire the cost up" and "make the
field optional" are different changes and only the first one is safe.

This is the finding that blocks P3 arithmetic rather than P3 execution: spend per model is one of
the four things P3 exists to measure, and the ladder currently cannot report it even though every
number needed is already in the transcript.

## The dispatch banner describes the wrong backend

Both Runs printed, before their first request:

```
  model (session default — unpinned)
  claude /Users/florianriquelme/.grind/bin/claude
```

Neither line is true of a native Run. Run B had `fast_model_override` and `strong_model_override`
both set to `stealth/ox-alpha` in its own record while the banner called the model unpinned, and
no `claude` binary was involved in either Run. Cosmetic in the sense that nothing downstream reads
it, and not cosmetic at all in practice: this banner is the first thing a human sees after a
Dispatch, and during Run A it was read as evidence that backend selection had silently failed.
The record had to be opened to disprove the banner.

## Malformed tool calls are consumed silently

Run A's 49 assistant turns produced:

| call name | count |
|---|---|
| `bash` | 57 |
| *(empty name)* | **17** |
| `read_file` | 4 |
| `write_file` | 2 |

Seventeen calls arrived as `{"name": "", "arguments": "\"\""}`. The loop consumed each as a turn,
against a 32-turn budget, and emitted no `ProtocolNudge` — the mechanism that exists for exactly
this (a reply that did not conform, corrected once and logged so drift rates stay measurable).
Run B produced zero of them, so this is a weak-model amplifier rather than a defect that bites
every backend; but a third of one attempt's turn budget spent on nothing, invisibly, is worth a
nudge and a transcript line.

## The two models, with the harness held constant

This is the comparison P3 asks for, and it is unusually clean: identical Job, identical stage
skills, identical three-tool registry, identical prompts, one variable.

**`deepseek/deepseek-chat-v3.1` livelocked in Plan.** It ran
`git merge-base --is-ancestor 33c450f… main` **sixteen times**, identically. The command answers
only through its exit status, and grind surfaces exactly that — the tool result the model received
was `exit: 1\nstdout:\n\nstderr:\n`. It then tried `echo $?` four times, which cannot work: each
`bash` call is its own process, so `$?` reports the fresh shell's status and never the previous
call's. It never wrote a plan, hit `turn budget exhausted (32)`, and the supervisor re-entered at
the stage that died — correctly — whereupon the second attempt began repeating the first. Killed
by hand after $0.049. 94% of that spend was prompt tokens (214,991 prompt against 2,024
completion): a 32-turn loop re-sends the whole conversation every turn, so on a small context
window the cost is dominated by re-reading, not by thinking.

**`stealth/ox-alpha` completed Plan in 14 turns**, 13 of which carried tool calls, with zero
repeats and zero malformed calls. It met the same `exit: 1` ambiguity on the same command and
handled it by making the exit status visible to itself —

```
git merge-base --is-ancestor 33c450f main && echo "Handoff SHA is on main" || echo "NOT ..."
git rev-parse origin/main | cat; git merge-base --is-ancestor 33c450f origin/main && echo ...
```

— correctly suspecting that the local `main` was stale relative to the Handoff SHA, which it was.
It then went straight at the work: the README region, the head of CLAUDE.md, and
`grep -rn "fifteen\|sixteen\|seventeen\|eighteen"`.

**The harness was never the weak link.** That is the load-bearing conclusion, and it is why Run
A's livelock is a model datapoint rather than a bug report.

### One recorded assumption is now stale

Epic #135 and ADR-0018's rationale both rest on a POC observation that `stealth/ox-alpha` was
"deterministically unable to execute native tool calls", masked as `finish_reason: "stop"` with a
hidden `native_finish_reason: "network_error"`. As of 2026-08-24 it is not: two direct probes and
one full stage returned well-formed `tool_calls` with `native_finish_reason: tool_calls`, and the
in-loop probe latched `native` unprompted.

This does not withdraw anything. `proto=` was justified by a real observation, capability-adaptive
tool calling does not depend on this one slug, and a stealth slug whose behavior changed once can
change again — which is itself the argument for leaving the probe to decide rather than declaring
`proto=` for a model that drifts. What it does mean is that the epic's evidence basis should not
be cited for ox-alpha's capabilities without re-measuring.

## What these Runs are not evidence for

- **No P3 cost datapoint.** Run B was free and Run A never finished a stage, so neither says
  anything about what a ten-rung native ladder costs. That measurement cannot be taken until the
  `done_promise` blocker and the `total_cost_usd` gap are both fixed.
- **No quality judgement.** Neither Run produced a diff, so nothing here speaks to whether native
  output is good — only that one model can complete a stage and one cannot.
- **Nothing about the nine rungs past Plan.** Work and Ship are where a three-tool registry
  standing in for Claude Code's full set is most likely to strain, and no Run has reached them.

## Provenance and mechanics worth recording

- `~/.grind/agent`'s extended grammar works as ADR-0017 describes it. `native` bare resolved to
  `DEFAULT_MODEL`; `native model=stealth/ox-alpha` set both routing classes and snapshotted both
  into the record as `fast_model_override` / `strong_model_override`.
- `grind doctor`'s two new rows behaved exactly as specified: `not met` with no key present, and
  `?` — unobservable rather than red — for the endpoint probe that could not be attempted without
  one. Both went `ok` once a key was in the environment.
- The dispatch lock is keyed on repo + branch through `File::try_lock`, which releases on process
  exit, so re-dispatching the same Job after killing a supervisor needed no cleanup.
- `view::Live` answered for both Runs throughout, including during Reflect — whose transcript is
  named `reflect-messages-1.jsonl` rather than `messages-N.jsonl` and is still selected correctly
  as the newest write.
- The second Dispatch adopted the first's worktree. `~/.grind/repos/FlorianRiquelme/grind` is a
  symlink into a development checkout on this host, so the two Runs' recorded worktree paths differ
  only in whether the symlink was resolved.
