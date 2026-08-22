# Grind

A queue, a supervisor and a record around headless `lfg` runs. It executes plans the
human is not present for, and stops at an open PR.

## Language

**Job**:
One unit of queued work, filed as a GitHub issue in the repo the work happens in. Names a
target repo, a branch, a Handoff SHA, an Anchor artifact, a done predicate, a base branch and
a verify entrypoint, and may name a model, declared hot paths and an **Intent** — one line on
the work's nature, never a requirement. It carries no spend ceiling: ADR-0010 withdrew it. What
makes it a Job is what its body names, never a label — Grind applies none (ADR-0012).
_Avoid_: ticket, task, plan

**Enqueue**:
The single conversational step, with the human present, that turns a prepared branch into
a Job. It drafts the whole Job from what that session already knows — deriving what the repo
can tell it, asking only where deriving would guess — and files nothing the human has not read.
It closes by offering the Dispatch, which is what keeps filing and starting one act rather than
two; declining is what leaves the Job on the Queue. It writes no taxonomy of its own
(ADR-0012), so a Job left waiting is marked however its human already marks things.
_Avoid_: file, submit, schedule

**Queue**:
The Jobs filed and not yet started, found however the human's own tracker already finds
things. Grind has no part in it — it applies no label, runs no query, and cannot tell a
queued Job from any other issue (ADR-0012). Nothing waits here for a mechanism: a Job waits
only because its human has not yet chosen to start it.
_Avoid_: backlog, pipeline, list

**Dispatch**:
Starting a Run for a Job, always by a human naming that Job. Grind never selects — not the
Job, and not the host it runs on. Nothing watches the Queue and nothing fires on a schedule;
the act that files a Job is the act that may start it.
_Avoid_: launch, trigger, kick off

**Run**:
One supervised execution of a Job's ladder (ADR-0015). Restartable, and re-enterable at the
stage it died on. Runs are independent and any number may be in flight at once; the only
thing two of them share is the usage pool. The world moves underneath one — colleagues
and other Runs land commits while it works.
_Avoid_: build, execution, night

**Supervisor**:
The process that runs one Run — dispatching its Attempts, deciding re-entry, and the sole
writer of its Run state. One per Run and never resident: nothing watches it while the host
is up, and a host that restarts re-enters the Runs that were cut off rather than restarting
the supervisor itself.
_Avoid_: daemon, service, watcher

**Serve**:
The reader a human launches to watch Runs on this host — one process that serves the
current Run state as pages until it is closed. It holds no lock, owns no Run and writes
nothing; it dies without consequence because it was never responsible for anything. It is
not resident and nothing watches it: an observer is not an owner, which is why Serve is
not the daemon ADR-0011 refused (ADR-0013).
_Avoid_: ui server, monitor, web app

**Attempt**:
One invocation of the agent within a Run, and the unit the Run's budget counts. A Run is
bounded by how many Attempts do work, never by how long it takes or what it costs.
_Avoid_: try, retry, iteration

**Wait**:
An Attempt that did no work — it cost nothing and took at most one turn, because the world
was not ready for it. A Wait is not a failure and never spends the budget; a Run that only
waits is bounded separately, by how many it does in a row.
_Avoid_: probe, retry, no-op, free attempt

**Blocker**:
An obstacle a Run cannot clear itself and a human can. A Run that meets one stops at once
rather than spending Attempts against it, and resumes where it stopped once the human has
cleared it — the world changed, not the budget.
_Avoid_: failure, error, stuck

**Fan-out**:
The subagents one stage session spawns to work in parallel, counted per Attempt. Claude Code
substrate, distinct from the ladder's stages — which the supervisor observes directly rather
than by counting a fan-out — so a stage that spawns nothing is still a stage. How many were
spawned and how many returned is readable from outside; what any of them concluded is not, and
the difference is a fact about processes rather than a judgement about the work.
_Avoid_: review pass, swarm

**Handoff SHA**:
The commit at which the human stopped and the Run begins. It bounds **authorship**, not
visibility: everything in front of it is the Run's, which is what makes `handoff_sha..HEAD`
a reviewable diff. That diff exists only where the commit is **reachable from the worktree the
Run adopts**, so Dispatch refuses a Job whose branch does not already contain it: stopping on a
merge commit is ordinary, and adopting a branch that has not caught up with one is incoherent.
The Run may read past it — the default branch moves while it works — but only to avoid
colliding with the world, never to change what it builds.
_Avoid_: base, head, start commit

**Base drift**:
The target repo's default branch moving after the Handoff SHA. It is the standing condition
rather than a mode, was present with zero Runs in flight, and is invisible where it matters
most — two files claiming ADR-0001 have different names, so git merges clean and reports
nothing. Observed and surfaced when non-zero, never enforced.
_Avoid_: divergence, staleness, conflict

**Anchor artifact**:
The one file a Run is pointed at explicitly as the requirements it must satisfy —
enriched during the day and readiness-promoted by the Run. Everything else the Run needs
is discovered from the branch.
_Avoid_: spec, plan, requirements doc

**Run state**:
The supervisor's own working record of a Run, held on local disk and never committed. The
supervisor is its only writer — reading it never writes it — and it names the host holding
it, because it does not travel.
_Avoid_: artifacts, journal, log

**Record**:
The durable account of a Run and the one a human actually reads: the open PR together with the
branch behind it. It is written complete, carries the Run's own narrative on top — decisions
taken, the non-obvious, what surprised it — and is identified by the commit the Run pushed
rather than by the branch its Job named. Run state is the supervisor's working copy of the same
Run and does not travel; every other surface that shows a Run is a projection of the Record and
may compress. In leg 1 there is exactly one such projection that leaves the host — the
supervisor's terminal-state comment on the Job issue — and Grind seeds no room, channel or
second home for a feature's history.
_Avoid_: log, report, write-up, PR description

**Provisioned host**:
A machine a Dispatch can succeed on. One definition, and the laptop must meet it too — an
item it cannot satisfy is a wrong item, not a special machine. The host declares itself by
the layout of `~/.grind/` rather than by configuration, and what it owes is listed in
`docs/provisioned-host.md`.
_Avoid_: box, machine, runner, worker

**Handback**:
What a finished Run leaves for the human to pick up: the Record, plus the things only the
supervisor knows — how many Attempts did work, what it spent, what it was denied, and what could
not be observed at all. It makes exactly five claims about the world — the verdict, and the four
observations that decide it — and everything else it carries is cost, a pointer, or a fact that
decides nothing and appears only when non-zero; it never re-counts what the Record already shows. Two surfaces carry it, one on the host and one on the
Job issue, over a single set of facts, differing only in where they send the human to look. Its
shape is what the morning costs. The Record is its durable half; the rest is a projection and may
compress.
_Avoid_: digest, report, results, morning

**Dashboard**:
Run state projected onto a browser page by Serve — the roster of this host's Runs, and one
page per Run. It reads the same files `grind status` reads, may compress like any
projection, and adds no claim the record does not carry. Being read-only is its
definition, not a limitation; and it does not travel, because Run state does not
(ADR-0013).
_Avoid_: console, control panel, admin

**Verify entrypoint**:
A repo's own generic answer to "how do I check this", adopted rather than invented, and
shared with CI so a repo has one definition of checked instead of two.
_Avoid_: verify command, test command, gate

**Promotion**:
Moving a capability from supervised local sessions into Grind, once it has stopped
needing correction. Enacted by changing Grind — never by an unreviewed drift in provenance,
since a skill edit or a binary upgrade only ever lands on a Run through a frozen, recorded
hash and version, not by advancing silently the way the retired plugin pin once could
(ADR-0002 as amended a fourth time).
_Avoid_: rollout, enablement, release
