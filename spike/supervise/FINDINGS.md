# FINDINGS — supervise

Answers wayfinder #33's hardest question: can Rust supervise a `claude -p` child that dies
mid-run and be re-entered, the way `bin/grind`'s `invoke()`/`supervise()` do today, and
what does that cost versus the Python?

Never invokes the real `claude`. Every child is a shell script under `fake/` that
reproduces one death shape actually seen in
`.grind/runs/20260802-105828-snapper-21/run.json` (five attempts, three of which had
`is_error:true` and `exit_code:1` while `subtype` said `"success"`). Run it with
`cargo run -p supervise`.

`fn main()` runs six scenarios end to end and asserts on the outcome of each.

## What worked

- Raw before parse is a two-line invariant, not a design: `fs::write(stdout_path, &raw)`
  happens unconditionally, then parsing is attempted afterward as a separate statement.
- Proved it the hard way (scenario B): a parser that deliberately `.unwrap()`s instead of
  degrading gracefully panics, wrapped in `std::panic::catch_unwind`. The raw file on disk
  is still the full 136 bytes afterward, because the write already completed before the
  panicking closure ever ran.
- SIGKILL mid-write (scenario C) returns `exit_code: None` but still hands back every byte
  that made it down the pipe before the signal landed.
- The re-entry contract (attempt 1 `--session-id`, every later attempt `--resume`, same
  session id throughout) is a two-line `if`; scenario A asserts the literal argv for all 5
  attempts.
- Budget exhaustion is a genuine `Terminal::Exhausted` enum variant returned from
  `supervise()`, not an exception path.

## What Rust made harder

- The `Result`/panic split had to be decided up front: I needed both a graceful
  `Result`-returning parse path (production shape) and a panicking path (to prove the
  ordering claim), which meant a `chaos_parse` flag threading through `invoke()`.
- Killing a process from inside the test harness needs a real subprocess: `sigkilled.sh`
  does `kill -9 $$` on itself, since there's no lighter-weight way to interrupt a child
  mid-write from the test side alone.
- No regex in the dependency list — a real cost, detailed below.

## What Rust made easier

- `Attempt`/`Terminal` make the state machine exhaustive: `died`/`rate_limited`/`completed`
  per attempt, and `Completed`/`Exhausted` as the only two terminal states, enforced by the
  type checker rather than a string literal like Python's `state["state"] = "died"`.
- `wait_with_output()` bundles stdout, exit code, and (killed-or-not) status into one call.

## Rate-limit detection: regex-free, and it is a real behaviour change

`bin/grind` uses `re.compile(r"rate.?limit|usage limit|too many requests|quota
exceeded|resets? at|429", re.IGNORECASE)`. Regex is not in this spike's dependency list, so
`is_rate_limited` here is `str::contains` over a lowercased blob of the same four fields,
checked against 8 literal needles (`rate limit`, `ratelimit`, `rate-limit`, `usage limit`,
`too many requests`, `quota exceeded`, `reset at`, `429`).

This is not equivalent — it is narrower and more brittle:

- `rate.?limit` matches "rate" plus any single separator character or none, so
  `rate_limit`, `rate.limit`, `rate:limit` all match the regex. My needle list only covers
  the separators actually seen in this codebase's error strings (space, none, hyphen); a
  novel separator like `rate:limit` is a silent miss for `contains` that the regex would
  have caught. This is exactly the gap `.?` exists to close.
- `resets? at` matches "reset at" and "resets at" from one pattern. I only enumerated
  "reset at"; "resets at" still matches by luck (it contains "reset at" as a substring),
  not by design.
- Both approaches short-circuit at the first match anywhere in the blob — no difference
  there.
- The honest fix for a real rewrite: either accept the narrower needle list (what's here),
  or pull in the `regex` crate — small, no network calls, stdlib-adjacent enough not to
  undermine "no dependencies" the way a heavier crate would. `contains` is what "no
  dependency list" actually buys, and it buys a worse detector, not an equal one.

Per the spec's own framing, mistaking a rate limit for a crash burns the attempt budget on
something that just needed a sleep. A missed match here degrades to `died` (immediate
re-entry, no sleep) rather than burning the whole budget in one shot the way a false
negative on `is_error` would, but it can still cost more than one attempt across a run if
the model phrases the message differently each time.

## Line-buffered stdout: Rust gets it for free

Python needs `sys.stdout.reconfigure(line_buffering=True)` because CPython switches to
full block buffering the moment stdout is not a tty — a detached, piped supervisor is
exactly that case, and a live run looks dead until it exits without the explicit opt-in.

Measured empirically: ran `cargo run -q -p supervise 2>/dev/null | cat` and timestamped
every line as it arrived through the pipe:

```
16:52:33.635 === scenario D: rate-limited, then recovers on re-entry ===
16:52:33.645     rate limited -- sleeping 200ms (spike-shortened from the real 1800s), then re-entering
16:52:33.799   attempt 2 (resume) argv: [...]
16:52:33.822 === scenario E: ...
```

The "sleeping 200ms" line lands at `.645`; the next line lands at `.799` — a ~154ms gap
matching the `std::thread::sleep(Duration::from_millis(200))` between them, observed live
through `cat`, not released all at once at process exit (the binary keeps running for
another ~200ms after that point). Rust's `println!` flushes per call whether stdout is a
tty or a pipe; there is no block-buffering mode to opt out of. This is a genuine finding:
the Python's explicit line-buffering line has no Rust equivalent because the failure mode
it guards against doesn't exist here.

## Cost: lines of code

| | Python (`bin/grind`) | Rust (`supervise/src/main.rs`) |
|---|---|---|
| `is_rate_limited` | 9 | ~20 (incl. comment on the regex gap) |
| `invoke` | 61 | ~90 (incl. write-before-parse ordering and the extra chaos-parse branch this spec required) |
| `supervise` | 37 | ~35 |
| **supervision core total** | **~107** | **~145** |

The file is 419 lines total; everything past line 279 (`fn main()`) is the scenario
harness the spec asked for (six fake-child sequences, assertions, and per-attempt
diagnostics), not supervision logic. The core (`build_argv` + `is_rate_limited` + `invoke`
+ `supervise` + the `Attempt`/`Terminal` types) is ~145 lines, roughly 35-40% more than the
Python it replaces. The overhead is mostly: (1) the `catch_unwind` plumbing the panic-proof
required, which real code wouldn't need since a `Result`-returning parser never panics in
the first place; (2) spelling out struct fields Python gets for free from a plain `dict`;
(3) being explicit and commented about the regex-gap workaround.

## What felt like fighting the language

Nothing structural. The one real footgun: `Child::stdin` must be `.take()`n, written to,
and dropped (closing the pipe) before `wait_with_output()` is called, or a child that reads
its prompt to EOF (as `cat >/dev/null` does in every fake script here) blocks forever
waiting for a stdin close that never arrives. `Command::output()` doesn't let you write to
stdin at all, so this shape (`spawn()` + `take()` + `wait_with_output()`) is the one anyone
porting `invoke()` needs to reach for. One paragraph of stdlib documentation, not a
language fight.

## Left out

- No `--max-budget-usd` or other Job-derived argv fields — the spec only asked to prove the
  `--session-id` vs `--resume` distinction, not reproduce the whole Job structure
  `bin/grind` builds `cmd` from.
- No real 1800s rate-limit sleep — shortened to 200ms so scenario D finishes in the same
  run as everything else; the code path (`thread::sleep` + `continue`) is identical either
  way.
