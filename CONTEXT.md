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
The supervisor's own working record of a Run, held on local disk and never committed.
_Avoid_: artifacts, journal, log

**Handback**:
What a finished Run leaves for the human to pick up — the open PR, the seeded feature
channel, and the findings and residuals inside them. Its shape is what the morning costs.
_Avoid_: digest, report, results, morning

**Feature channel**:
The Buzz room that carries a feature's history — seeded by the Run, continued by the
human and their day agents, archived by hand when the feature is done.
_Avoid_: thread, room, digest

**Verify entrypoint**:
A repo's own generic answer to "how do I check this", adopted rather than invented, and
shared with CI so a repo has one definition of checked instead of two.
_Avoid_: verify command, test command, gate

**Promotion**:
Moving a capability from supervised local sessions into Grind, once it has stopped
needing correction. Enacted by advancing the pinned plugin version.
_Avoid_: rollout, enablement, release
