# Grind

A queue, a supervisor and a record around headless `lfg` runs. It executes plans the
human is not present for, and stops at an open PR.

## Language

**Job**:
One unit of queued work, filed as a labelled GitHub issue in the repo the work happens
in. Names a target repo, a branch, a Handoff SHA and an Anchor artifact.
_Avoid_: ticket, task, plan

**Enqueue**:
The single conversational step, with the human present, that turns a prepared branch into
a Job.
_Avoid_: file, submit, schedule

**Queue**:
The Jobs waiting, seen as a label query over GitHub issues rather than held anywhere.
Grind never selects from it; a human does, by naming a Job. Dispatch dequeues by removing
the label.
_Avoid_: backlog, pipeline, list

**Dispatch**:
Starting a Run for a Job, always by a human naming that Job. Grind never selects. A
schedule can only delay a Dispatch a human already chose, never choose one.
_Avoid_: launch, trigger, kick off

**Run**:
One supervised execution of `lfg` against a Job. Restartable, and re-enterable at the
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
The subagents one Attempt spawns to work in parallel. Claude Code substrate, never an `lfg`
stage — which is what lets the supervisor count it without observing the pipeline. How many were
spawned and how many returned is readable from outside; what any of them concluded is not, and
the difference is a fact about processes rather than a judgement about the work.
_Avoid_: stage, review pass, swarm

**Handoff SHA**:
The commit at which the human stopped and the Run begins. Context is everything behind
it; reviewable output is everything in front of it.
_Avoid_: base, head, start commit

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
observations that decide it — and everything else it carries is cost or a pointer; it never
re-counts what the Record already shows. Two surfaces carry it, one on the host and one on the
Job issue, over a single set of facts, differing only in where they send the human to look. Its
shape is what the morning costs. The Record is its durable half; the rest is a projection and may
compress.
_Avoid_: digest, report, results, morning

**Verify entrypoint**:
A repo's own generic answer to "how do I check this", adopted rather than invented, and
shared with CI so a repo has one definition of checked instead of two.
_Avoid_: verify command, test command, gate

**Promotion**:
Moving a capability from supervised local sessions into Grind, once it has stopped
needing correction. Enacted by changing Grind — no longer by advancing a pinned plugin
version, which floats with the host (ADR-0002 as amended).
_Avoid_: rollout, enablement, release
