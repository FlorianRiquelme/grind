# Spike notes — wayfinder #33, "Whether Rust survives Grind's awkward core"

PROTOTYPE. Throwaway. Branch `prototype/33-rust-awkward-core`. Not a translation source.

Per-crate findings live in `<crate>/FINDINGS.md`. This file holds what the driver established
directly, against the **real** `claude` binary and the **real** Run 1 record — the two things no
fake child can tell us.

---

## 1. Re-entry works from Rust, with zero dependencies

The load-bearing mechanic: Run 1 died and was resumed five times in eight hours. Proven from a
~110-line stdlib-only Rust probe (`std::process::Command`, no crates):

- spawn `~/.local/bin/claude -p --output-format json --session-id <uuid>`, prompt on stdin,
  stdout redirected to a file, stderr to a separate file;
- `Child::kill()` (SIGKILL) mid-response;
- re-spawn with `--resume <same uuid>`;
- **history intact** — a codeword planted in turn 1 came back in turn 2. Verdict `YES`.

`std::process` covers the whole shape: argv, cwd, stdin pipe, two separate output redirections,
kill, wait, exit status. Nothing here wants a crate.

**The real binary is not the one on PATH.** `which -a claude` puts cmux's shim first
(`/var/folders/.../cmux-cli-shims/claude`); the real binary is `~/.local/bin/claude`. This
confirms `resolve_claude_bin()`'s reason for existing, and it is a live hazard on this host today,
not a historical one.

## 2. A killed child leaves stdout EMPTY, not truncated — the ticket's premise was wrong here

This corrects an assumption worth correcting, because it moves work off the critical path.

First run of the probe killed at 14s and got **1644 bytes of valid, complete JSON** — because
haiku had already finished (`duration_api_ms: 10701`) and we were killing an idle process. That
proved nothing. Re-killed at 4s, genuinely mid-response:

```
attempt 1  SIGKILL after 4.00188s
attempt 1  exit: ExitStatus(unix_wait_status(9))
attempt 1  raw stdout on disk: 0 bytes  (parses as JSON: false)
transcript 8 lines
```

`--output-format json` emits **one document at the end**. So the child's stdout is
**empty-or-complete**, never half a JSON object. Two consequences:

- The degrading-parse problem does **not** apply to `attempt-N.stdout.json`. Its failure mode is
  the empty string, which is one `if raw.trim().is_empty()` branch, not a tolerant parser.
- It **does** apply to the transcript, which is written incrementally and is the only thing that
  survived the kill (8 lines of real progress while stdout had nothing).

Corroborated against Run 1's real record: **all five `attempt-*.stdout.json` parse cleanly**, and
every `stderr.log` is 0 bytes. Across one real 8-hour run the Python's `json.JSONDecodeError`
fallback never fired. Truncated child JSON is a hazard we invented.

## 3. The real death shape: `subtype: "success"` on a dead Run

From `.grind/runs/20260802-105828-snapper-21`, all five attempts:

| attempt | is_error | subtype | stop_reason | terminal_reason | api_error_status | DONE | cost |
|---|---|---|---|---|---|---|---|
| 1 | **true** | `success` | stop_sequence | `api_error` | null | no | $23.51 |
| 2 | **true** | `success` | stop_sequence | `api_error` | null | no | $2.35 |
| 3 | **true** | `success` | stop_sequence | `api_error` | null | no | $0.12 |
| 4 | false | `success` | end_turn | `completed` | null | **no** | $11.74 |
| 5 | false | `success` | end_turn | `completed` | null | yes | $3.18 |

Attempts 1–3 all end with the same prose in `result`:
`API Error: Connection closed mid-response. The response above may be incomplete.`

Four things the base must be built knowing:

1. **`subtype` is `"success"` on a Run that died.** It is `"success"` in all five attempts,
   including the three that failed. Any typed model that reads `subtype` as the outcome reads a
   death as a success. This is Grind's signature failure mode already latent in the child's own
   output format.
2. **`terminal_reason` is the honest discriminator** — `api_error` vs `completed`, and it agrees
   with `is_error` five times out of five. It is also the field the Python treats as merely one of
   four rate-limit haystacks rather than as the outcome.
3. **`api_error_status` is `null` on all five attempts**, including the three genuine API errors.
   `is_rate_limited()` searches it anyway. In real data it contributes nothing — the signal, if it
   ever arrives, is prose inside `result`.
4. **Attempt 4 finished with a PR open and never promised DONE**, ending "PR is moving — run
   `/ce-babysit-pr` …". This is the concrete evidence behind #9's *completion is observed, never
   declared*: the promise is absent on a Run that got all the way there. Only attempt 5 promised.

## 4. The transcript is heterogeneous in ways no guess would cover

The 19-line transcript from one 4-second killed haiku session already contains **six line types**
— `queue-operation` (4), `attachment` (7), `user` (3), `assistant` (3), `last-prompt` (1),
`mode` (1). A separate real transcript carries 40+ distinct top-level keys and both `session_id`
*and* `sessionId`.

**`isSidechain` is not the fan-out signal, and #12's heartbeat depends on knowing that.** Counted
across every transcript on this host:

| file class | lines carrying `isSidechain` | value |
|---|---|---|
| parent (`<dir>/<uuid>.jsonl`) | 88,763 | `false` — **never** true |
| subagent (`<dir>/<uuid>/subagents/agent-*.jsonl`) | 37,588 across 514 files | `true` — **never** false |

It is a constant per file *class*, not a discriminator within a file. So "newest mtime across the
fan-out's subagent transcripts" is a **filesystem** operation — glob `<uuid>/subagents/*.jsonl`
alongside the parent and take the newest mtime — not a field test. 111 such `subagents/`
directories exist here. Reading `isSidechain` to find fan-out activity would scan every parent
line and always conclude there was none, i.e. exactly the false "stuck" reading that sends the
human to kill a healthy Run.

Every line was valid JSON and the file ended with a clean newline, including after SIGKILL.
(An earlier reading of "4 of 19 lines invalid" was a measurement bug in the driver's shell — zsh
`echo` interpreting backslash escapes in the JSON — not corruption. Re-measured with `json.loads`
per line: 0 invalid. Recorded because a false finding here would have justified a tolerant parser
for the wrong reason.)

So: the tolerant parse is needed for **unknown and shifting keys**, not for **broken bytes**.
Those want different solutions, and only the first is real.

## 5. Rate-limit detection without regex is BETTER than the regex, not worse

`supervise/FINDINGS.md` concludes that dropping `regex` buys "a worse detector, not an equal
one", having tried a list of literal needles (`rate limit`, `ratelimit`, `rate-limit`, …). That
conclusion is wrong, and the fix is one line: **normalize the haystack instead of enumerating
separators.** Strip every non-alphanumeric character, lowercase, then match `ratelimit`,
`usagelimit`, `toomanyrequests`, `quotaexceeded`, `resetat`, `resetsat`, `429`.

Measured against the current Python regex
(`rate.?limit|usage limit|too many requests|quota exceeded|resets? at|429`):

| case | Python regex | literal needles | normalized |
|---|---|---|---|
| `Claude AI usage limit reached` | ✓ | ✓ | ✓ |
| `rate-limit exceeded` | ✓ | ✓ | ✓ |
| `rate_limit exceeded` | ✓ | **✗** | ✓ |
| `rate:limit exceeded` | ✓ | **✗** | ✓ |
| `rate  limit exceeded` (two spaces) | **✗** | ✗ | ✓ |
| `resets at 3pm` | ✓ | **✗** | ✓ |
| `429 Too Many Requests` | ✓ | ✓ | ✓ |
| Run 1's real death: `API Error: Connection closed mid-response.` | ✗ | ✗ | ✗ |
| decoy: `connection reset by peer` | ✗ | ✗ | ✗ |

Normalization is a **superset** of the regex: it matches everything the regex matches and also
catches `rate  limit`, which `.?` cannot (it permits zero or one character, not two). Both
correctly refuse Run 1's actual death text and the `connection reset by peer` decoy — the
false-positive direction that would sleep 30 minutes on a crash.

So the `regex` crate is not needed here, and the honest framing is the reverse of the spike's:
zero-dependency detection is an upgrade. (The spike's write-up also claims `"resets at"` matches
its needle list "by luck" by containing `"reset at"` — it does not; that row is a genuine miss.)

## 6. Line-buffering: the Python workaround has no Rust equivalent

`main()`'s `sys.stdout.reconfigure(line_buffering=True)` exists because CPython block-buffers
the moment stdout is not a tty, which is exactly the detached-supervisor case. Rust's `println!`
flushes per line through a pipe regardless — measured live through `| cat` with per-line
timestamps, showing the real ~154ms sleep gap rather than one block release at exit. One
carrier from #32 dissolves rather than porting.

## 7. Cost of the probe

Two haiku invocations, $0.011 + $0.023. Run 1's five real opus attempts totalled ~$41, of which
the first attempt alone was $23.51 — which is the number that makes "does re-entry work" worth
proving before the rewrite rather than after.
