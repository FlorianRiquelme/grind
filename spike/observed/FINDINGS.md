# Findings: `Observed<T>` and Grind's silent-collapse failure mode

Wayfinder #33. Question: does Rust's type system stop "could not observe" from becoming
indistinguishable from "observed absent" — the exact bug in `bin/grind`'s `sh(...,
check=False)` returning `""` for both a real negative and a failed subprocess call?

## What compiled

- `src/main.rs`: `Observed<T>` (`Present(T)` / `Absent` / `Unobservable(String)`), the four
  ANDed completion signals (`pr_open`, `tree_clean`, `commits_ahead`, `ci_clear`) as
  `Observed<bool>`, and a `verdict()` that folds them through one exhaustive `match` into
  `Verdict::Completed | Incomplete(Vec<&str>) | Uncorroborated(Vec<String>)`.
- `fn main()` prints four scenarios end to end: all four present and satisfied, one absent
  (dirty tree), one unobservable (`gh pr view` timeout), two unobservable (laptop-wake
  scenario, two calls into `gh` both failing). Verified by running `cargo run -p observed`:
  the first scenario resolves to `Completed`, the second to
  `Incomplete(["tree_clean"])`, and both unobservable scenarios resolve to
  `Uncorroborated([...])` — never `Completed`, never `Incomplete`.
- `cargo test -p observed`: one test, `unobservable_always_wins_and_only_unobservable_produces_it`,
  enumerates all 3^4 = 81 combinations of the four signals' states and asserts the invariant
  holds in both directions — any `Unobservable` anywhere forces `Uncorroborated`, and
  `Uncorroborated` is reachable no other way. Passes: `test result: ok. 1 passed`.

## The won't-compile proof

Two files in `wont-compile/`, excluded from the crate build (not `mod`-referenced, outside
`src/`), each redefining a minimal local `Observed<T>` so they compile standalone with
`rustc` directly.

### (a) `forgot_unobservable.rs` — a match that forgets the third case

```
rustc --edition 2021 forgot_unobservable.rs -o /tmp/a
```

Verbatim:

```
error[E0004]: non-exhaustive patterns: `Observed::Unobservable(_)` not covered
  --> forgot_unobservable.rs:15:11
   |
15 |     match o {
   |           ^ pattern `Observed::Unobservable(_)` not covered
   |
note: `Observed<bool>` defined here
  --> forgot_unobservable.rs:8:6
   |
 8 | enum Observed<T> {
   |      ^^^^^^^^
...
11 |     Unobservable(String),
   |     ------------ not covered
   = note: the matched value is of type `Observed<bool>`
help: ensure that all possible cases are being handled by adding a match arm with a wildcard pattern or an explicit pattern as shown
   |
18 ~         Observed::Absent => "absent",
19 ~         Observed::Unobservable(_) => todo!(),
   |

error: aborting due to 1 previous error

For more information about this error, try `rustc --explain E0004`.
```

This is the load-bearing result: the exact shape of Grind's real bug — a caller who wrote
"yes" and "no" branches and never wrote one for "couldn't ask" — is a compile error, not a
code-review miss. `check=False` in Python has no equivalent failure mode: there is nothing
the compiler can refuse.

### (b) `silent_collapse_to_false.rs` — collapsing on purpose

```
rustc --edition 2021 silent_collapse_to_false.rs -o /tmp/b
```

**This one compiles.** Verbatim compiler output (warnings only, exit 0):

```
warning: field `0` is never read
  --> silent_collapse_to_false.rs:14:18
   |
14 |     Unobservable(String),
   |     ------------ ^^^^^^
   |     |
   |     field in this variant
   |
   = note: `#[warn(dead_code)]` (part of `#[warn(unused)]`) on by default
help: consider changing the field to be of unit type to suppress this warning while preserving the field numbering, or remove the field
   |
14 -     Unobservable(String),
14 +     Unobservable(()),
   |

warning: variant `Present` is never constructed
  --> silent_collapse_to_false.rs:12:5
   |
11 | enum Observed<T> {
   |      -------- variant in this enum
12 |     Present(T),
   |     ^^^^^^^

warning: 2 warnings emitted
```

Running it:

```
unobservable -> false
absent       -> false
```

**Say it loudly: this is the honest limit of what the type system buys.** The code is
`if let Observed::Present(true) = o { true } else { false }` — syntactically it *does* name
the `Unobservable` case, by falling into the `else` along with `Absent`. Nothing in the
type system objects, because nothing is unhandled; the mistake is semantic (routing a
distinct variant to the same outcome as another), and Rust's exhaustiveness checker has no
opinion on semantics. Clippy does not flag this either (`if let ... else` on a
non-`Option`/`Result` enum isn't one of its collapse lints). So the guarantee `Observed<T>`
actually gives is narrower than "you can't collapse could-not-observe into false" — it's
"you cannot let could-not-observe collapse *by omission*". A programmer who wants to
collapse it on purpose still can, in one line, with zero compiler resistance.

## What the type system genuinely prevents vs. what it only discourages

**Prevents (hard, at compile time):**
- Forgetting the `Unobservable` arm anywhere a `match` is written directly over
  `Observed<T>` — proof (a).
- A fourth state quietly creeping in unhandled: adding a variant to `Observed<T>` breaks
  every exhaustive `match` over it until each is updated — not tested here directly, but it
  falls out of the same exhaustiveness check as (a).
- `Verdict::Completed` being reachable through any code path that also touches
  `Unobservable` without routing through the `unobservable.push(...)` arm in `verdict()` —
  because that arm is the only place a `Verdict::Uncorroborated` value gets constructed,
  and the match that guards it is exhaustive.

**Only discourages (a human decision, not a compiler check):**
- Choosing to fold `Unobservable` into the same bucket as `Absent` once you've already
  named it in a match arm — proof (b). The type forces you to *write a line for it*; it
  does not force that line to do the right thing.
- Choosing the right classification for a real external signal (see cost section below) —
  e.g. deciding that `gh pr view` exiting 1 with empty stderr means "genuinely no PR" while
  exiting 1 with a stderr blob means "couldn't tell". That triage is domain knowledge Rust
  has no way to check; Python needed exactly the same judgment call and also didn't check
  it.

## Measured call-site cost

**Reading one signal, Python (current `bin/grind`, `observe()`):**

```python
pr = sh(["gh", "pr", "view", "--json", "number,url,state,isDraft"], cwd=wt, check=False)
obs["pr"] = json.loads(pr) if pr.startswith("{") else None
```
2 lines, 147 characters. Any failure of `gh` — timeout, auth error, network blip, or a
genuine "no PR exists" — produces the same empty string, then the same `None`.

**Producing the equivalent `Observed<bool>` in Rust, illustrative (not compiled — no
subprocess calls exist in this spike; shown for the cost measurement only):**

```rust
fn observe_pr_open(wt: &Path) -> Observed<bool> {
    match Command::new("gh").args(["pr", "view", "--json", "state"]).current_dir(wt).output() {
        Ok(out) if out.status.success() => match serde_json::from_slice::<Value>(&out.stdout) {
            Ok(v) => Observed::Present(v["state"] == "OPEN"),
            Err(e) => Observed::Unobservable(format!("gh returned unparseable json: {e}")),
        },
        Ok(out) if out.stderr.is_empty() => Observed::Absent, // gh's own "no PR" exit
        Ok(out) => Observed::Unobservable(String::from_utf8_lossy(&out.stderr).into_owned()),
        Err(e) => Observed::Unobservable(e.to_string()),
    }
}
```
11 lines, 652 characters — roughly **5x the lines, 4.4x the characters** of the Python
one-liner it replaces, for one signal.

**Where the cost actually is, and where it isn't:**

- The cost above is not the `Observed<T>` type or the exhaustive `match` — those add maybe
  4 lines total (the definition, in `src/main.rs:10-14`). The cost is the *classification
  logic*: deciding, from a raw exit code and stderr blob, which of the three buckets a
  real-world failure belongs in. Python's `sh(..., check=False)` never had to make that
  decision because it had nowhere to put a third answer — it threw everything into the
  string it returns. Rust doesn't remove that judgment call; it makes it a place you're
  required to write code, instead of a place you're allowed to skip.
- That classification cost is paid **per producer, not per consumer**. Once
  `Observed<bool>` values exist, `verdict()` in `src/main.rs` reads all four of them through
  one shared match inside a single loop — that cost does not grow with the number of call
  sites that need a verdict. A fifth completion signal added later pays the ~11-line
  producer cost once and gets folded through the existing loop for free.
- Net: **expensive at the edge where the world is observed, cheap everywhere the
  observation is consumed.** Grind's real bug lives at the edge (`sh()` collapsing a failed
  `gh` call), so this is the right place for Rust to be more expensive — the ceremony is
  concentrated exactly where the silent collapse used to happen.

## Bottom line

`Observed<T>` and an exhaustive `match` make Grind's actual historical bug (a forgotten
"couldn't ask" branch) a compile error instead of a 3am false-positive report. They do not
make it impossible to write the bug on purpose, or to misclassify a real subprocess result
— those remain human judgment calls, unaided by the compiler, exactly as they were in
Python. The type system converts one specific class of mistake (omission) into a hard
failure, at a real but one-time, per-producer cost; it does not touch the other class
(deliberate or misinformed collapse), which is the one worth remembering has no fix here.
