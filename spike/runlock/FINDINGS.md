# runlock — an flock keyed on (repo, branch), std-only

Run with:

```
cargo run -p runlock -- demo
```

from `spike/`. One command; the binary forks itself to run every scenario
below and prints pids, paths, and outcomes as it goes. Individual pieces
are also reachable directly: `key <repo> <branch>`, `hold <repo> <branch>
<seconds>`, `try <repo> <branch>`, `undetermined-probe`.

## Is std's flock sufficient? Yes, zero dependencies.

`std::fs::File::try_lock() -> Result<(), std::fs::TryLockError>` (stable
since 1.89; this machine runs 1.95) is enough. `TryLockError` is an enum —
`WouldBlock` (someone else holds it) vs `Error(io::Error)` (something else
went wrong) — which is exactly the distinction grind needs and gets for
free. `runlock/Cargo.toml` has an empty `[dependencies]` table; the crate
compiles and runs against nothing but std.

## The SIGKILL property — verbatim evidence

This is the entire reason the ruling is "flock, not a state check": there
is no `running` state in `run.json`, so a SIGKILLed supervisor leaves a run
`dispatched` forever, and any state-based check would then refuse dispatch
onto a branch nothing is actually touching. An OS-held flock has no such
failure mode — the kernel drops it when the holding process dies, killed
or not. Output from an actual `cargo run -p runlock -- demo` run,
unedited:

```
=== 3. SIGKILL releases the lock (the whole point) ===
[pid 92789] attempting lock: /var/folders/.../grind-runlock/72cff6156e520b89__story_33-foo.lock
[pid 92789] ACQUIRED, holding for 9999s
  lock path : /var/folders/.../grind-runlock/72cff6156e520b89__story_33-foo.lock
  outcome   : REFUSED -- held by another pid (pid 92914 did not get it)
while victim is alive, a fresh dispatch attempt: refused, as expected
kill -9 92789 ...
  lock path : /var/folders/.../grind-runlock/72cff6156e520b89__story_33-foo.lock
  outcome   : ACQUIRED by pid 92997
after SIGKILL, a fresh dispatch attempt: ACQUIRED CLEANLY -- flock released by the OS on process death
```

pid 92789 was `kill -9`'d — no chance to run a destructor, close a file
descriptor cleanly, or write anything. The very next `try_lock` from a
brand-new process (92997) still acquires it. A run-state check reading
`run.json` at this point would see `"state": "dispatched"` forever and
refuse — permanently — even though nothing is running. That's the
scenario this mechanism is built to not have.

## Two processes, second refuses (scenario 2)

```
[pid 92019] attempting lock: .../72cff6156e520b89__story_33-foo.lock
[pid 92019] ACQUIRED, holding for 2s
[pid 92163] attempting lock: .../72cff6156e520b89__story_33-foo.lock
[pid 92163] REFUSED -- held by another process
second process (via sibling worktree path) exit code: 1 (refused, as expected)
[pid 92019] releasing (process exit)
```

Two genuinely separate OS processes (the binary re-invokes itself via
`std::process::Command`, no threads). The second call was made passing a
*different* filesystem path (a sibling worktree of the same repo) — it
still collides on the same lock file, which is the point of the key
derivation below.

## Held-by-other vs could-not-determine (scenario 4)

`TryLockError::WouldBlock` (someone else has it) and
`TryLockError::Error(io::Error)` (couldn't even try — permissions, missing
directory, whatever) surface as distinct `Outcome` variants in
`src/main.rs` and are never merged. Proof, from the same run:

```
=== 4. held-by-other vs could-not-determine must not be confused ===
...
held-by-other reported as exit code 1 (contract: 1)
probing a path that cannot be opened: /nonexistent-grind-runlock-dir/x/lock
  correctly reported UNDETERMINED: No such file or directory (os error 2)
```

Exit code 1 means "someone else is running this branch." Exit code 2 means
"I don't know, and neither should you assume it's a refusal." A caller
that collapses those two into one "can't dispatch" branch reproduces
exactly the failure grind already has with observation (a failed read
looking identical to a negative result) — just relocated to the lock.

## Key derivation: repo *identity* + branch, not repo *path*

Lock files live in `$TMPDIR/grind-runlock/<hash>__<branch>.lock` — an
OS-wide scratch location, not inside any repo's `.grind/`. `.grind/` is
explicitly host-local, per-checkout scratch that's gitignored; it exists
*inside* one worktree. The lock must be visible across every worktree of a
repo, so it cannot live inside any one of them.

The harder decision is the hash's input. Naively hashing the `repo` path
argument is **wrong**: `resolve_worktree` in `bin/grind` can hand back a
path that is *itself* a worktree, and the CLAUDE.md preferences on this
machine describe running ~10 parallel worktrees of the same origin repo at
once. Two dispatches that land on different worktree directories of the
same underlying repo, targeting the same branch, must still collide — and
a lock keyed on the literal directory string would let them past each
other silently.

Instead `repo_identity()` shells out to `git rev-parse --git-common-dir`,
which returns the *one* `.git` directory shared by every worktree of a
repo, canonicalizes it, and hashes that. Scenario 1 in the demo proves
this directly by building a real repo with two worktrees and showing the
derived lock path is identical from both:

```
=== 1. key derivation ===
lock path derived from repo path            : .../grind-runlock/72cff6156e520b89__story_33-foo.lock
lock path derived from sibling worktree path : .../grind-runlock/72cff6156e520b89__story_33-foo.lock
same key despite different filesystem paths : true
```

Branch names are sanitized (`[^A-Za-z0-9.-]` → `_`) before going into the
filename, since `story/33-foo` contains a path separator.

Fallback: if `git rev-parse --git-common-dir` fails (not a git repo, or
`git` missing from `PATH`), `repo_identity()` falls back to
`canonicalize(repo)` — i.e. degrades to path-keying. This is a known,
documented gap, not a silent one (see below).

## What this does NOT protect against

- **The path fallback.** If git-common-dir resolution fails for any
  reason, the lock degrades to keying on the literal path passed in, which
  is exactly the bug this whole exercise exists to avoid. In practice
  `bin/grind` always operates on a git repo, so this should be rare, but
  it is a real gap in this prototype, not a theoretical one — it should be
  a hard error in production, not a silent fallback.
- **Runs dispatched before this mechanism existed.** A worktree adopted by
  a run that started before any `runlock`-style locking was wired into
  `bin/grind` was never holding a lock in the first place. Introducing
  this after the fact protects every *new* dispatch, but a still-running
  old-style run and a first new-style dispatch onto the same branch will
  not collide — the old run never took the flock. This only becomes airtight
  once every code path that can dispatch or resume acquires the lock.
- **Platform: flock semantics differ between macOS and Linux**, and this
  runs on both (a laptop and an ephemeral Linux host per the target
  environment). Both `try_lock` on stable Rust use `flock(2)` on both BSD
  and Linux, so the *advisory, per-open-file-description* semantics are
  the same in the case that matters here (one process, one open, cooperating
  callers). The corner case that differs: on Linux, `flock()` is associated
  with the *open file description*, so locks are shared across `dup()`'d
  fds and released the moment all copies of that fd close, which matches
  what SIGKILL demonstrated. macOS/BSD flock has the same open-file-description
  semantics (unlike POSIX `fcntl` locks, which are process-associated and have
  well-known do-not-use pitfalls with `fork`/`dup` and multi-threaded closes —
  this prototype deliberately does not touch `fcntl` locks for that reason).
- **NFS.** `flock(2)` over NFS is historically unreliable — some NFS
  clients/servers don't implement it at all (silently succeeding without
  actually locking), others need `lockd` running and can still misbehave
  across network partitions. If `.grind`'s host, or `$TMPDIR`, is ever on a
  network filesystem shared between hosts, this lock's core guarantee
  (mutual exclusion) may quietly stop holding. Grind's stated deployment
  (a laptop and an ephemeral single-tenant Linux host, one filesystem each)
  avoids this, but it is not enforced by the code.
- **A leaked lock file on disk is harmless but not free.** The lock *file*
  itself is never cleaned up (this prototype leaves them in
  `$TMPDIR/grind-runlock/`); only the advisory lock *on* it is released
  with the process. An accumulating pile of zero-byte files is not a
  correctness problem — `try_lock` on an unlocked file always succeeds
  regardless of the file's history — but it is disk litter a real
  implementation should periodically sweep, and `$TMPDIR` itself can be
  cleared by the OS between reboots, which is fine for this file (it's
  advisory and recreated on demand) but means it must never be treated as
  a durable record.
- **This only serializes *dispatch*, nothing about what happens inside a
  Run.** It answers "can a second supervisor start writing into this
  worktree" — it says nothing about, and is not meant to replace, ADR-0003
  (grind never gates) or anything about the quality or content of what a
  Run produces.
