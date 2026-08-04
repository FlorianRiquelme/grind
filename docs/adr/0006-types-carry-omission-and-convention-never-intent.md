---
status: accepted
date: 2026-08-05
---

# Types carry omission and convention, never intent — and a variant set is a policy

The base is Rust (ADR-0005) because the failure modes whose silent failure is expensive should
be unrepresentable. This decides *which* of Grind's safety properties actually get that
treatment, and what carries the rest.

Recorded resolving [#34](https://github.com/FlorianRiquelme/grind/issues/34), whose comment
holds the full table. [#32](https://github.com/FlorianRiquelme/grind/issues/32) supplied the
ordering this refines, and [#33](https://github.com/FlorianRiquelme/grind/issues/33) supplied
the measurements it rests on.

## The two rulings that looked contradictory

#32 ruled **one carrier per finding**, because a duplicated finding drifts — explicitly,
*anything that becomes a type should stop being a test rather than gaining one*. #33 ruled that
**every invariant carried by a type needs a second carrier**, because an agent handed a feature
whose naive implementation was the forbidden bug did not break the invariant — it silently
narrowed the feature until it fit, shipped something demo-true and CLI-false, and reported that
nothing resisted.

They do not collide, because they are about different propositions:

> **A type makes a state unrepresentable. A test pins the classifier that decides which state
> you are in.**

The finding *"a failed observation must not read as a negative one"* has one carrier —
`Observed<T>` — and no test asserts it, because it cannot be written. The second carrier is not
a copy: it is a test that *this* call site, given empty stdout and a non-zero exit and an auth
message on stderr, produces `Unobservable` and not `Absent`. That proposition is falsifiable,
which is exactly why the type cannot carry it. #33's own numbers say the same thing from the
other end: `Observed<T>` costs ~11 lines per producer and nothing per consumer, and all of the
ceremony is classification.

Drift between them is impossible because they assert different things — and the hollowing-out
failure #33 found was a classifier failure, not an invariant failure.

## Three failure modes, and only two of them are typeable

The admission test is not *how important is this property* but *how does it realistically fail*.

- **Omission** — a forgotten match arm, a forgotten struct field, a forgotten call site. The
  bug class that actually happens to an agent editing this. Typeable.
- **Convention** — the ecosystem's default idiom, applied without deciding anything. Typeable,
  because a type can remove the idiomatic path while leaving the deliberate one visible.
- **Intent** — an agent deciding. Not typeable, at all, ever: the deciding agent edits the enum,
  the const and the newtype in the same commit.

Convention matters more here than either of the others, because **every one of Grind's
distinctive rules is anti-idiomatic**:

| Grind's rule | the idiom it contradicts |
|---|---|
| the exit code reports observability, never health (#12) | non-zero means bad |
| never gate on a finding (ADR-0003) | `if !ok { return }` |
| never select a Job (#10) | a queue has a `next()` |
| a failed observation is not a negative one (#9, #31) | `.ok()`, `unwrap_or_default()` |
| the pin never resolves `latest` (ADR-0001, ADR-0002) | resolvers resolve |

An agent writing `if verdict != Completed { exit(1) }` is neither forgetting nor deciding. It is
doing what every CLI it has ever seen does. That is the failure this ADR exists to name, and it
is why *"never gates"* — which looks like pure policy — turns out to be typeable after all.

## Four carriers, not three

#32's ordering was `test > type > ADR`. That is a **durability** ranking, and it is stage two.
Stage one is **capability**: ask which carrier *can* work, from the failure mode, before asking
which fires earliest. A type fires at build, a test in CI, prose when a human reads it — and
nobody reads the diff ([#6](https://github.com/FlorianRiquelme/grind/issues/6)), so prose is
genuinely last.

The fourth carrier is **visibility**, and it earns its own name because Grind's only shipped
instance of this bug class is a **caller, not a state**. `cmd_status` calls `save()`, and a
whole-dict write from a read path can erase `attempts[]` — one of the two non-reconstructible
fields ([#8](https://github.com/FlorianRiquelme/grind/issues/8)) — while the human is watching
the dashboard to be reassured (#12,
[#27](https://github.com/FlorianRiquelme/grind/issues/27)). No `Observed<T>` prevents that and
no struct shape prevents it. What prevents it is that the writable type is not reachable from
the status module, which #33 verified rather than assumed:
`error[E0603]: struct RunRecord is private`.

Three properties have that shape — *who may call*, not *what may exist*:

| property | the wrong caller |
|---|---|
| only the supervisor writes `run.json` | `cmd_status` → `save()` — shipped and live in `bin/grind` |
| status never reaches an agent | none yet; a view built from the thing that gets rate-limited is unavailable during the stall it exists to explain |
| only dispatch reads the environment | `MAX_ATTEMPTS` is read at print time, so re-entering with a different `GRIND_MAX_ATTEMPTS` makes the record misreport its own budget |

All three are omission-shaped in the way that counts: the agent does not decide to corrupt the
record, it reaches for the obvious symbol and the language hands it over. `use RunRecord` is not
a decision, it is an import.

Module privacy is per-module, so a constraint falls out for
[#35](https://github.com/FlorianRiquelme/grind/issues/35): **the record's writer and the
record's readers may not share a module.**

## A variant set is a policy

Types do not only remove reachable states. **A careless type makes a forbidden thing newly
expressible, and expressible means reachable**, because nobody reads the diff. So the shapes
Grind's base must *not* have are as load-bearing as the ones it must:

| prohibited | why |
|---|---|
| any type for the Run's GitHub authority | [#37](https://github.com/FlorianRiquelme/grind/issues/37) ruled the Run inherits the credential *because there is no mechanism*, not as a preference: `GH_TOKEN=""` is a documented silent no-op (`go-gh` tests `!= ""` and falls through) and `GH_CONFIG_DIR` is plumbing a same-user child can unset. A type at that seam invites a `withhold()` that compiles, runs and does nothing — a control that appears to work and does not, which is the whole failure family. **Model nothing**, not even "inherited". |
| `enum PluginPin { Pinned(_), Latest }` | once `Latest` is spelled, resolve-at-dispatch is one match arm away, and advancing that pin is the act of promotion (ADR-0001, ADR-0002). Refusal must be the absence of a spelling, not a rejected case. |
| `VerifyContract { ok: bool }` | `present` and `missing` carry everything the handback needs. Add the boolean and `if !vc.ok { return }` is one line — a gate, in the exact place the constant says *recorded and surfaced, never enforced* (ADR-0003). |
| `Verdict::{Rejected, Blocked, Failed}` | every variant describes what happened. ADR-0003's *verdict language describes what happened, never quality* is enforceable as a variant set and nowhere else. |
| `Observed<T>` spelled `Result<Option<T>, E>` | #31 ruled `Result<T, E>` is two-valued in the same shape `sh(check=False)` is. The deeper reason is combinators: `.ok()`, `?` and `unwrap_or_default()` collapse three states into two *silently*, supplied free by the ecosystem. A dedicated enum has no such combinators, so every collapse must be written out where a reader could see it. |

## The fold, and the bypass rustc hands you

ADR-0005 records that *parallel arrays beside a struct are where the compiler goes quiet* —
reached because the spike folded its completion signals through a hand-sized
`[(&str, &Observed<bool>); 5]`, and adding a fifth signal did not force an entry there. Its
friction log calls that *the one place where correctness rode on me, not the type system*.

That gap was a design choice, not a language limit. Fold by **destructuring the struct with no
`..`** and the same omission is a compile error, at the cost of one line and no dependency:

```
error[E0027]: pattern does not mention field `verify_reported`
```

A new signal then has two forced sites — `E0063` at every constructor, `E0027` at the fold — and
neither needs grepping.

**The bypass is named because it has nothing behind it.** Verified: rustc's own `E0027` help
text offers `..` and `field: _` as fixes, and no clippy lint covers either. Unlike the
`if let Present(true) = o {…} else { false }` collapse #33 found, here the compiler *suggests*
the escape. Under the rules above that is a deliberate act, and deliberate acts are not
typeable — so it is recorded rather than patched. Chasing it regresses forever, and a reader who
believes the fold is airtight is the one who ships the collapse.

## Consequences

- **ADR-0004 is amended.** *Raw child output is written before anything parses it* stops being a
  rule stated in prose and becomes `RawAttempt` — private fields, obtainable only from
  `write_raw`, so "parse before write" is uncallable and the escape is `E0603`. #32 sent that
  behaviour to prose on the correct grounds that no test could carry it; this ticket owns the
  option #32 did not have. Under one-carrier, the ADR keeps the Run 1 evidence and hands the
  invariant to the type rather than duplicating it.
- **Ten properties become types, five shapes are prohibited, three become visibility, eight
  become tests, five stay prose.** The table is in
  [#34](https://github.com/FlorianRiquelme/grind/issues/34).
- **The claim to make downstream is narrow.** These carriers stop an agent forgetting and stop
  an agent reaching for the idiom. They stop nothing an agent means to do. ADR-0005 already says
  this about omission; the addition here is that convention is the mode most of Grind's rules
  actually fail in, and the one a reader is most likely to mistake for safety.

## What this ADR deliberately does not say

**That `DENIED_TOOLS` can be protected.** #37 established that the list is the *entire* barrier
— no credential at any tier can withhold merge from something permitted to open a PR, and
rulesets return `403 Upgrade to GitHub Pro` on this repo. That is settled, not a gap awaiting a
better credential. But weakening the list is **intent**: an agent under pressure to make a Run
go through deletes a glob, and it edits any type guarding the list in the same commit. What is
typeable there is only the narrower, omission-shaped property — *every `claude` invocation
carries the denials* — via a command builder whose output cannot be constructed without them.
The contents stay prose, in `CLAUDE.md`, where they already are.

**Which module anything lives in.** Visibility is only a carrier once the seams exist, and the
seams are [#35](https://github.com/FlorianRiquelme/grind/issues/35)'s. This ADR hands that
ticket one constraint and decides nothing else about layout.
