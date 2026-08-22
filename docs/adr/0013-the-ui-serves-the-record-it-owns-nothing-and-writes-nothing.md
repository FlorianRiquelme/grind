---
status: accepted
date: 2026-08-21
---

# The UI serves the record; it owns nothing and writes nothing

`grind serve` is a one-shot, operator-launched process that reads Run state from
`~/.grind/runs/` and serves it as web pages until the human closes it. It binds loopback by
default, holds no lock, keeps no state of its own, and dies without consequence — because it
was never responsible for anything. There is no write route, no action button, and no
second writer of `run.json`.

Decided in session on 2026-08-21, when the operator asked for a way to see and inspect the
Runs currently in flight, and chose a local webserver over a TUI. The target is a server
host, reached over an SSH tunnel; the laptop remains the same host it always was.

## A reader needs nothing resident

The reason this needs no new machinery is that everything worth showing is already durable,
by earlier rulings:

- **The record is written atomically** — `world::write_atomic` lands `run.json` as
  tmp + fsync + rename, so a reader polling mid-write sees the old file or the new one and
  never a torn one. No read lock is needed, and `view.rs` correctly takes none.
- **Raw output precedes parsing** (ADR-0004), so the attempt files on disk are the live
  evidence, not a post-hoc summary. An Attempt in flight is visible as the files appearing.
- **Liveness is computed, not stored** — recorded pid plus `lstart`, already answered by
  `world::ps_start_stamp` and `view::supervisor_here`. There is no `running` state and none
  is owed.
- **Narration is line-buffered to `supervisor.log`** (ADR-0004), precisely so that an
  observer is never fooled by a working Run that looks dead. The log tail is therefore a
  faithful pulse, and a byte-offset tail of it is cheap.

Against that, polling a KB-scale file once a second for one to five viewers is nothing. The
disk is the event source; the server is a lens on it.

## An observer is not an owner

[ADR-0011](0011-nothing-owns-the-supervisor-while-the-host-is-up.md) rejected a resident
service, and the rejection stands. What it rejected was **ownership**: pid management, a
restart policy, backoff — machinery someone must hold in their head at 2am, attached to a
process whose death would matter. `grind serve` acquires none of it. It can be killed,
crash, or be forgotten on a dead SSH session, and the effect on any Run is exactly nil. A
ruling against owners does not forbid lenses.

Two things would change this, and they are named so the revisit is cheap:

- **Push while away** — wanting to be *told* a Run stopped, rather than looking. That needs
  something resident holding a connection, and would amend this ADR with the carve-out:
  resident, owns nothing, restartable without consequence.
- **Multi-host aggregation** — wanting one page across hosts. Run state does not travel
  ([ADR-0008](0008-the-host-is-declared-by-its-layout.md)), so this needs a design of its
  own, not a bigger loop.

Neither has a measured example. Both stay out of scope.

## Why read-only is a definition, not a limitation

A button that resumes a Run or records a clearance is a second writer wearing a browser.
[#23](https://github.com/FlorianRiquelme/grind/issues/23) settled the discipline the moment
a Blocker exists: *only the hand clears, and only the hand re-enters* — deliberate acts,
typed by a human who knows what changed. Even Grind's own second writer (`grind cleared`)
needs load-re-load-under-lock choreography to keep "the supervisor is the only writer" true.
A web route would repeat that machinery and put it behind a mis-click. Every mutation stays
a typed verb.

## Loopback by default

Run state is host-local; the roster itself says *this host only*. The server therefore
binds `127.0.0.1` unless `--bind` says otherwise — the access path is an SSH tunnel, and
exposing Run state (which contains prompts, transcripts and narration) to a LAN is an
explicit act, not a default. Authentication is out of scope until a surface demands it;
the tunnel is the auth boundary for now.

## Consequences

- **`grind serve` joins the surface as an eighth shape.** Like bare `status`, it is
  pull-only and writes nothing; unlike `resume --all`, a typo'd invocation of it cannot
  mutate anything.
- **The read side of the house is now three consumers deep** — `status`, the Handback, and
  the dashboard — all over the same `view.rs` projection. Reader drift is a test question,
  not a hope: the dashboard's wording inherits the same discipline the terminal surfaces
  are held to.
- **Restart churn is tolerated, not solved.** `std` sets no `SO_REUSEADDR`, so an immediate
  re-bind after killing the server can refuse; the server retries briefly and exits in the
  could-not-answer register rather than growing socket options to fight it.
- **The dashboard is allowed to compress.** It is a projection like the Handback: it may
  summarize, and it adds no claim the record does not carry.

## Explicitly out of scope

Actions from the browser (resume, cleared, kill), authentication, multi-host views, and a
resident watcher are all out of scope. The first three are ruled out above; the fourth is
ruled out by [ADR-0011](0011-nothing-owns-the-supervisor-while-the-host-is-up.md) and
revisitable only through its tripwires. A JSON API is likewise out of scope — the pages
are the API, and adding a second serialization of the same facts is a second thing to keep
honest.
