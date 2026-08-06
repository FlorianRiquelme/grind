---
status: accepted
date: 2026-08-06
---

# Checked is one recipe, and CI runs nothing else

Grind checks a seven-step `VERIFY_CONTRACT` against every target repo and reports *a step trimmed
until it went green* as the failure that costs most. Grind itself has no `justfile`, no `.github/`,
and one entrypoint — `python3 tests/test_grind.py`, 107 lines over two pure functions. `CONTEXT.md`
defines a **Verify entrypoint** as *"a repo's own generic answer to 'how do I check this', adopted
rather than invented, and shared with CI so a repo has one definition of checked instead of two."*
Grind has never had one.

This is it. Recorded resolving [#36](https://github.com/FlorianRiquelme/grind/issues/36), whose
comment holds the full derivation.

Nothing here is built yet: there is no Rust crate in this repo, so a recipe running `cargo fmt`
would reference nothing. This ADR is the target the base arrives with.

## The premise that deflated: `just` is not a host dependency

[#36](https://github.com/FlorianRiquelme/grind/issues/36) framed the entrypoint's name as a trade
against host weight — *"note that `just` would become a host dependency
([#30](https://github.com/FlorianRiquelme/grind/issues/30))"*. It is wrong twice, and the framing
was mixing two machines that share no code path:

- **Grind's verify entrypoint never runs on a provisioned host.** #30 ships a prebuilt binary and
  forbids building there; ADR-0008's Executables list is `git`, `gh`, `bin/claude` and the `lfg`
  plugin, with no Rust toolchain. The entrypoint runs on a laptop where someone is editing Grind,
  and in CI. Those are the only two places.
- **`just` is already required on the host, for the Run.** `bin/grind:344` tells every Run
  *"Definition of done: `just verify` passes"*, and the Run executes it in the target repo. `just`
  is exactly as load-bearing there as `git` — and it is **missing from ADR-0008's list**, which is a
  gap in that document rather than a cost of this decision.

So the choice of name costs nothing on any host, and reduces to whether the symmetry is worth
having.

## Considered options: the entrypoint's shape

| Option | Verdict | Trade-off |
|---|---|---|
| **`just verify`, one recipe, CI runs it** | **Chosen** | `.github/` holds no knowledge of what checked means, so a step cannot exist in CI and not locally. **Cost:** `just` is a fourth tool a machine editing Grind needs, and `cargo test` — the idiom an agent reaches for unprompted — is not the entrypoint. |
| `cargo` native, CI as a list of steps | Rejected | Nothing extra to install, and it is the adopted answer for a Rust crate. **Cost:** the definition of checked splits in two — CI's lives in `.github/`, the local one in an agent's habits — which is the divergence `tests/test_grind.py` exists to catch in *other* repos, relocated into this one. |
| `just verify` as a thin alias for fmt+clippy+test | Rejected | Symmetry for one line. **Cost:** buys the name without the property; the recipe has to hold the cross-build and the odd tests or it is decoration. |

## What `just verify` runs

| step | why it is in the recipe |
|---|---|
| `cargo fmt --check` | Convention, and free. |
| `cargo clippy -D warnings` | ADR-0005 recorded a dead-code warning on `Observed::Absent` because nothing constructed it. On a type whose whole purpose is a state that must be representable, an unused-variant warning is a statement about test coverage. Warnings are not free here, so they are errors. |
| `cargo test` | Everything with a safety property, including the two carriers below. |
| `cargo zigbuild --target {x86_64,aarch64}-unknown-linux-musl --release` | The ship check. See below. |

## The two odd tests live inside `cargo test`, under `tests/`

ADR-0007 spent two carriers on this ticket before it had ruled: a **compile-fail test** shelling out
to `rustc` to assert `E0603` (the thing that closes the one-keyword `pub(crate)` repair its
one-crate topology accepted), and **source-level tests** asserting `std::env` is named in one module
and `std::process`/`std::fs` only in `world` (the only carrier for *only dispatch reads the
environment*, and what makes *"`world` is the only untested code"* checked rather than aspirational).

Both run inside `cargo test`, as integration tests under `tests/` — not as sibling `just` recipes.
Two reasons, and the second was not anticipated:

- **Otherwise `cargo test` is a false green.** ADR-0006's **convention** mode is the ecosystem's
  default idiom applied without deciding anything, and for a Rust crate that idiom is `cargo test`.
  Putting the repo's two most load-bearing tests only behind `just verify` means an agent that runs
  `cargo test`, sees green and stops has skipped precisely the tests carrying ADR-0006 and ADR-0007.
  That is ADR-0006's own failure mode aimed at Grind's entrypoint.
- **`tests/` is the only placement that does not self-contradict.** The compile-fail test spawns
  `rustc`, so it names `std::process`. In `src/` as a `#[cfg(test)]` unit test it would trip the
  source-level assertion that `std::process` appears only in `world` — the test closing ADR-0007's
  topology hole breaking the test that makes ADR-0007's central claim checkable. Integration tests
  are separate crates, so the assertion globs `src/**` and the conflict dissolves. It needs no
  exemption list, which matters: an exemption list is a thing an agent widens by one entry without
  deciding anything.

## The shipped artifact is musl static, and there is no mac target

#30 verified **both** fixes for glibc skew — pin the floor (`--target …gnu.2.17`) or link musl
static — and picked neither, so *"build the shipping triples"* had no referent. Picked here, because
the recipe cannot be written without it:

**musl static, `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`.** #30 measured that
musl changed nothing Grind exercises — process spawn, JSON parse, mtime, DNS and `flock` identical
across glibc 2.26, glibc 2.36 and Alpine — for +7–9% binary size, which is meaningless for a
supervisor. Static linking **deletes** the glibc-floor question rather than pinning it.

**Darwin is a development platform, not a shipping target.** Nothing is ever shipped for macOS; the
laptop runs whatever it built. This does not conflict with ADR-0008's *the laptop must pass the
provisioned-host definition* — those items are about the `~/.grind/` layout and are exercisable
against a locally built binary.

**The cross-build is in the recipe because it is the ship check, and it is the one step that makes
`verify` more than an alias.** *Compiles on Linux* and *the shipped artifact builds* are different
claims. The glibc floor flag exists only in the zigbuild path, and the failure #30 actually
reproduced was a binary that **refused to start** on `amazonlinux:2` and Debian 10 — so a repo green
on a native Linux build can still ship something that will not launch on the host, which is the only
place it matters. #30 measured 2–4s for all four triples with no Docker; the real cost is that any
machine running `just verify` needs `zig` and `cargo-zigbuild`, plus a one-time ~36s zig cache per
triple.

## CI: one Ubuntu job, running the recipe

- **`ubuntu-latest`, one job, `just verify` and nothing else.** If a step joins the recipe, CI gets
  it without an edit; if CI is green, `just verify` was green. `just` comes from `apt`, not a
  marketplace action — a third-party action is silent trust in the one repo whose thesis is that
  silent trust is expensive.
- **No macOS runner.** Grind has one shipping target and Darwin is not it. Darwin-native compilation
  breaking is loud — every local run is on it — while Linux musl breaking is silent, which is why
  that one is in the recipe. macOS runners also bill at 10× against a private repo's allowance
  where Linux bills at 1×.
- **Push on every branch, plus `pull_request`, with `concurrency: cancel-in-progress`.** Grind's
  working pattern is agents pushing branches no human reads; a mark per push is the point, and
  cancellation stops three pushes in a minute burning three runs.

## `DENIED_TOOLS` does not cover CI

The repo's central safety property — a Run must never merge its own PR, force-push, hard-reset,
rebase or delete a branch — is a set of `claude` tool globs, and
[#37](https://github.com/FlorianRiquelme/grind/issues/37) established those globs are the **entire**
barrier rather than the outer one, because no credential can withhold merge from something allowed
to open a PR. A GitHub Actions job is not `claude`, so `DENIED_TOOLS` never sees it: a
write-scoped `GITHUB_TOKEN` reaches all five operations without touching the barrier at all.

`default_workflow_permissions` on this repo is `read`, with `can_approve_pull_request_reviews:
false` — verified, so the hole is theoretical today. **But it is fine in the one place nothing can
review.** That default is a repo *setting*: it appears in no file, no diff and no `git log`, it is
one toggle from `write`, and a later workflow can request `permissions: contents: write` explicitly
regardless of it. [#6](https://github.com/FlorianRiquelme/grind/issues/6)'s *nobody reads the diff*
is bad enough; a control living outside every diff is strictly worse.

So **`permissions: contents: read` is pinned at the workflow's top level**, redundantly with the
setting, because one line moves the control into a file.

## Hermeticity is structural, not policed

Nothing in CI may invoke `claude`, dispatch a Run, or touch a target repo. The carrier is that **CI
holds no secrets** — no `claude` binary, no `~/.grind/`, no OAuth token, no signing key — so a job
attempting it dies at the first step. This is ADR-0008's move: Run state sits outside every checkout
so *never committed* holds structurally rather than by a `.gitignore` line.

No test polices this, deliberately. An agent wiring secrets into CI is **intent**, and ADR-0006
establishes that no carrier defends against intent. A test there would be theatre.

## ADR-0003 does not forbid this

Said explicitly, because the misreading is cheap and has happened before. ADR-0003 is **Grind never
gates a target repo's PR** — verdict language describes what happened, never quality, and nothing
may block a PR from existing on the strength of a finding. **Grind's own CI blocking Grind's own
merge is a different thing and is fine.** #32 found the script still printing *"human review is the
gate"* long after #6 corrected it; *"never gates"* read as *"no CI"* is the same class of drift.

## What stands in for hand-exercising

`tests/test_grind.py` records that the dispatch and re-entry paths *"are exercised by hand against a
scratch repo"*. With agents writing the base, that is a step that quietly stops happening. What
replaces it already exists and has been proven once: `spike/supervise` runs the **whole supervisor
loop end to end with no `claude`, no network and no target repo** — every child a shell script
reproducing one death shape recorded in Run 1's `run.json`, six scenarios, and scenario A asserting
the **literal argv for all five attempts** (first `--session-id`, every later one `--resume`, same
session id throughout). `sigkilled.sh` does `kill -9 $$` on itself and the harness still captures
every byte that reached the pipe.

Promoted from the spike's `fn main()` into `cargo test`, so it sits inside `just verify` like
everything else:

- **Fake `claude` at ADR-0008's declared `~/.grind/bin/claude`.** #35 kept *the binary path* as the
  `claude` seam precisely because only a real process replays real SIGKILL and real
  empty-not-truncated stdout. A shell script there needs no trait and no injection — the seam exists.
- **Fake `gh` earlier on `PATH`, real `git` against a scratch repo.** `gh` resolves from `PATH`
  (ADR-0008), so this is a seam that changes no production code. #31 ruled real `git` output is the
  point.
- **The binary spawned as a subprocess with a temp `$HOME`**, via `CARGO_BIN_EXE_grind`. ADR-0008
  made `$HOME` the only variable, and an in-process test would need `std::env::set_var` — which is
  process-global, racy under parallel tests, and `unsafe` in Rust 2024. Spawning also makes it a
  genuine end-to-end, covering `cli` and argv rather than only the loop, and keeps `std::env` out of
  the test.

**What this green means, stated narrowly:** the loop handles the death shapes that have been
*recorded*. Not that the loop handles `claude`. The fakes are derived from Run 1 rather than invented
— #31's rule is that fakes substitute raw stdout + stderr + exit code, never domain values — which
is what stops them being fiction, but a death shape nobody has seen yet is invisible to them.

## Consequences

- **ADR-0007's last consequence is amended: argv on the short-lived side is covered after all.** It
  named the fix — *"fake `gh`/`git` executables on a temp `PATH`"* — and declined it as a third
  **code** seam. The end-to-end test does exactly that without a code seam: no trait, no production
  change, nothing `pub`. What ADR-0007 declined stays declined; what it named as uncovered is now
  covered by a different mechanism.
- **The base has three uncovered areas, not two.** `world`'s own syscalls (untested by construction,
  ADR-0007), the real `claude`/`gh` contracts, and any failure mode Run 1 did not exhibit. A green
  `just verify` says nothing about any of them, and the only thing that produces new death shapes is
  a real Run — [#19](https://github.com/FlorianRiquelme/grind/issues/19), not this ADR.
- **`just`, `zig` and `cargo-zigbuild` join the tools a machine editing Grind needs**, on top of the
  Rust toolchain. Three `install` lines, and the entrypoint fails loudly without them rather than
  skipping the step.
- **`apt`'s `just` version floats.** No floor is pinned, because Grind uses one recipe and no
  recent syntax; an invented floor is a precondition that fails for no reason (ADR-0008's reasoning
  about `gh`).
- **`docs/provisioned-host.md` is missing `just`** from its Executables list, found here rather than
  decided here. The Run runs `just verify` in the target repo, so it belongs beside `git` and `gh`.
- **CI never exercises the literal ship command.** #30's mechanism is *cross-compile from Darwin
  arm64*; CI proves *cross-compile from Linux*. Zig supplies the libc and the linker, so the artifact
  is host-independent — but the real command is covered only by being run on a laptop.

## What this ADR deliberately does not say

**That `cargo test` and `just verify` are interchangeable.** They are not: `verify` adds fmt, clippy
and the cross-build. The property claimed is narrower and is the one that matters — `cargo test`
alone runs every test carrying a safety property, so reaching for the idiom is never a *false* green,
only an incomplete one.

**That the source-level and compile-fail tests are hard to fool.** ADR-0007 already accepted they are
string matching and a `rustc` invocation, defeated by `use std::env as e`. That was accepted on the
ground that they guard **convention**; aliasing an import to dodge a test is intent. Do not harden
them into something cleverer.

**That CI is a gate on anything a Run produces.** It gates Grind's own merges and nothing else. See
ADR-0003 above.
