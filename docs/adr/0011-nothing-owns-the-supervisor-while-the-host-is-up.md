---
status: accepted
date: 2026-08-09
---

# Nothing owns the supervisor while the host is up; a boot one-shot re-enters what was cut off

There is no Grind daemon. One supervisor runs one Run, in the foreground, and nothing watches it.
The only thing the platform's service manager does is fire **once at boot** — `RunAtLoad` on
darwin, `Type=oneshot` on linux — calling `grind resume --all`, which re-enters every Run on the
host that was **cut off** rather than stopped.

Decided resolving [#38](https://github.com/FlorianRiquelme/grind/issues/38), which asked how the
supervisor becomes a long-lived process given that nothing is above it to re-enter it. The answer
is that it does not become one.

## What was actually missing

Two gaps were run together in the question, and only one of them was open.

**Keeping the process alive on a live host** is solved and proven. Run 1 survived four laptop
sleeps *because the supervisor stayed alive across them*, and
[#10](https://github.com/FlorianRiquelme/grind/issues/10) already weighed the alternatives.

**The reboot gap** had no owner. `~/.grind/` survives a restart — worktree, branch, PR and
`run.json` all intact — so a rebooted host leaves a Run that is perfectly re-enterable sitting at
`died` until a human notices.

And Grind already knows where it was cut off: the record holds `attempts[]`, the furthest stage,
and the supervisor's pid with its `lstart`, and `resume` re-enters at the stage that died — which
Run 2 exercised across a three-hour gap. **Nothing needed to learn anything.** What was missing was
something to ask it after a reboot, which is a trigger, not a supervisor-of-the-supervisor.

## Why not a daemon

A resident service owning the supervisor's lifetime brings pid management, a restart policy and
backoff — and every one of those is a thing the operator has to hold in their head, understand
when it misfires, and reason about at 2am. The driver's objection is the decisive one: *the whole
point of Grind is not having to micro-manage it.* A daemon trades one kind of babysitting for
another.

The test every option was held to: **after provisioning, does a Dispatch require anything other
than `grind run <issue>`?** A boot one-shot answers no. It has no restart policy, nothing watching
anything, and no state of its own — it fires, spawns, and exits.

## Why not nothing at all

This is the honest alternative and it nearly won. **The reboot gap has zero measured examples**
across two Runs, and this map's own discipline elsewhere is *revisit once it has actually
happened*. This decision is therefore made against no evidence, which is worth stating plainly
rather than leaving for someone to discover.

What tipped it: the failure is not a lost Run, it is a Run sitting at `died` for however long it
takes a human to look — which is the micro-management this decision exists to remove, arriving by
a different door.

## What counts as cut off

`resume` by hand accepts anything that is not `Completed` or `Exhausted`. The boot path is
**narrower**, because two different things produce a non-terminal record:

- **Cut off** — `Dispatched`, `RateLimited`, `Died`, *and* the recorded supervisor is not alive.
  The process vanished between one thing and the next. Boot re-enters these.
- **Stopped** — `Uncorroborated`, `Unobserved`, `Blocked`. The supervisor reached a decision and
  wrote it. Boot re-enters **none** of them.

Re-entering a stopped Run overrides a deliberate decision at the one moment nobody is watching.
`Uncorroborated` stops precisely because *"a session that believes it finished would re-emit the
promise until the budget was gone"*. `Unobserved` is the arguable one — a reboot may well have
cleared the fault — but re-entering it automatically means a blind Run mutating a branch, which is
what `Next::Reobserve` exists to prevent. `Blocked` is excluded by
[#23](https://github.com/FlorianRiquelme/grind/issues/23): a Blocker is defined by needing a human.

The liveness half needs nothing new. After a reboot every recorded pid is stale by construction —
gone, or holding a different `lstart` — so `world::process_start_stamp` already answers it. The
edge: a fast reboot plus a pid collision plus a colliding `lstart` second reads alive, and the
failure is *declining to re-enter*, which is the safe direction.

## Consequences

- **`grind resume --all`** is the surface. Not a `grind boot` verb, which would name a command
  after its caller; not bare `grind resume`, because bare `grind status` is pull-only and writes
  nothing while bare `resume` would mutate every branch on the host from a typo; and **not** a
  shell loop over `grind status`, whose format is deliberately undocumented and *degrades rather
  than fails* ([#12](https://github.com/FlorianRiquelme/grind/issues/12)) — a plist parsing it is a
  silent breakage waiting for the one boot that matters.
- **Re-entry is concurrent, never serial.** Serial is ordering, and ordering is the human's act
  (#10); a boot that runs Run A before Run B is Grind selecting.
  [#11](https://github.com/FlorianRiquelme/grind/issues/11) put no ceiling on Runs in flight and
  this must not add one through the back door. **The mechanism detail this leaves:** the one-shot
  spawns N supervisors and exits, and a `Type=oneshot` exiting takes its cgroup with it by default
  — so every Run dies seconds after boot, silently, unless the unit says otherwise. It is the one
  place this decision still has machinery in it.
- **The supervisor writes `run_dir/supervisor.log`.** Its narration had no durable home — the
  record and the raw attempt files land in `run_dir`, the narration only ever reached stdout. Left
  to the service manager it becomes a per-platform question, and ADR-0008's posture is that
  per-platform divergence in where things live is how you get two internally-consistent wrong
  answers. ADR-0004's line-buffering rule is unchanged and now has a destination that does not
  depend on who started the process.
- **Two platform files, and the laptop carries one.** `docs/provisioned-host.md`'s rule is one
  definition that the laptop must pass, so this is a launchd plist *and* a systemd unit, maintained
  forever, against a gap with no measured examples. Marked *doctor*: an uninstalled unit is
  perfectly silent, and you find out one reboot later. It is the first `doctor` check that
  branches on platform, and it must verify **loaded**, not merely present — a plist on disk that
  was never bootstrapped is the likeliest way this fails.

  The item is recorded in `docs/provisioned-host.md`'s *Lifetime* section **without a mark**, because
  the marks there are bound to `job::host_items()` by
  `every_item_carries_the_mark_the_document_gives_it` and this item has no `Check` behind it yet.
  The decision is made; the carrier and the check land together, or neither does.

> **Landed 2026-08-21.** The paragraph above is superseded: the item carries its *doctor* mark
> in `docs/provisioned-host.md`, `job::host_items()` carries the entry, and `Check::BootOneShot`
> is implemented in `cli.rs`. Carrier and check landed together, as this section required.

## Explicitly out of scope

**A supervisor killed on a host that stays up** — OOM, a stray `kill`, a crash. Neither the current
mechanism nor a boot one-shot closes it: nothing fires, and the Run sits exactly as it would after
a reboot. Closing it needs something resident and watching, which is the daemon this ADR rejected.
Zero measured examples. Written down so nobody assumes the boot path covers it.
