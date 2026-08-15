---
status: accepted
date: 2026-08-15
---

# Grind adds, never classifies

Grind writes **comments on the Job issue and nothing else**. It never applies a label, removes
one, or requires one to be present; it never assigns, never moves an issue between states, and
never depends on a query over any of them. The target repo's issue vocabulary belongs to whoever
owns that repo.

Decided resolving [#39](https://github.com/FlorianRiquelme/grind/issues/39), which asked whether
putting a Job on the Queue should be the Dispatch signal. It cannot be, because after this
decision Grind cannot see a Queue at all.

## Add and classify are different acts

A **comment** is additive and ungoverned. It needs no permission beyond write, creates no
vocabulary, and takes nothing away from anyone. It is also authored by the human's own PAT
([#37](https://github.com/FlorianRiquelme/grind/issues/37), established by
[#15](https://github.com/FlorianRiquelme/grind/issues/15)) — so it is the human commenting on
their own issue rather than a bot decorating it.

A **label** is a shared namespace someone else governs. On a corporate tracker you may not be
permitted to create one, and the taxonomy is an org asset rather than a tool's scratch space.

The concrete case makes it sharper than the principle does. `QUEUE_LABEL` was `ready-for-agent`
— which is not Grind's label at all. It is one of the five canonical triage roles
(`docs/agents/triage-labels.md`), meaning *fully specified, ready for an AFK agent*, and it
exists in both repos for that purpose. So Grind was **erasing a triage fact to record a queue
fact**: after a Dispatch the issue is still fully specified and still AFK-ready, and the label
that said so is gone. The queue view was polluted in the other direction too — any issue triaged
AFK-ready appeared in it without being a Job, which is a different thing entirely (a Job names a
target repo, a branch, a Handoff SHA and an Anchor artifact).

Giving Grind its own label instead — `grind:queued` — fixes the overload and not the objection.
It still requires every target repo to carry a label because Grind wants one.

## What it costs

**The Queue stops being something Grind can see.** A labelless queue is not expressible as a
`gh search` query, and `STRATEGY.md` leaned on exactly such a query to rule a central cross-repo
queue out of scope. That ruling survives; its example does not.

This is a real cost paid for a principle, and the evidence says the cost is small. **The Queue
has had one member, ever.** snapper#21 was labelled `2026-08-02 09:41 CEST` and dispatched by
hand at `10:58` — a 77-minute wait, with the human awake, closed by that same human; the label
came off two days after the Run finished, by hand. snapper#28, Run 2's Job, was **never labelled
at all**. One of two Jobs used the Queue, and the reason is prior to any mechanism: refined plans
do not arrive fast enough for anything to queue behind anything, which is the map's own bet
([#11](https://github.com/FlorianRiquelme/grind/issues/11)), and #11 removed the only structural
reason a Job would wait by putting no ceiling on Runs in flight.

Nothing else is lost, because **the label was never a precondition**. `grind run <issue>` reads
the issue and dispatches; Run 2 proved the unlabelled path works end to end. The removal is
subtractive only.

## What survives

Both comments. The dispatch comment (*"Dispatched as Run `x` on `host`"*) and
[#56](https://github.com/FlorianRiquelme/grind/issues/56)'s terminal-state comment, which
[#15](https://github.com/FlorianRiquelme/grind/issues/15) promoted to **the** off-host
observability surface leg 1 has. Had this decision reached comments, #15 and #56 would both
reopen with nothing to replace them.

**The Run's PR is not covered by this.** `Closes #<job>` in the PR body
([#14](https://github.com/FlorianRiquelme/grind/issues/14)) changes issue state on merge, but it
is the Run authoring its own artifact and the **human's merge** that fires it — not Grind
reaching into someone's tracker. The line is who owns the surface being written, not whether a
write has consequences.

## Consequences

- **`world.rs`'s invariant becomes *one place, two writes*.** It was *two places, three writes*
  ([#14](https://github.com/FlorianRiquelme/grind/issues/14)): the label removal and the comment.
  With the label gone there is one place — the Job issue — and both writes on it are comments.
- **`QUEUE_LABEL` and the label half of `dequeue_and_point_at_this_host` are deleted**, and the
  function loses the *dequeue* from its name. Build item.
- **The Queue is the human's, and Grind never reads it.** Whatever finds your filed-but-unstarted
  Jobs — an assignee filter, a title prefix, a project board, a saved search — is your practice in
  your tracker. `CONTEXT.md`'s **Queue** entry is rewritten to say so, and **Job** loses
  *"labelled"*.
- **This forecloses the pull trigger permanently**, not just for now. A watcher needs a queryable
  claim on shared state, and Grind has ruled it may not create one. #39's *what claims a Job, and
  where does that claim live* has no answer available, which is the point: the claimant is a human
  and always was.

## Explicitly out of scope

**A tracker that is not GitHub.** GitHub is assumed for both personal and corporate work here —
`gh issue view` is how a Job is read, `gh` is a provisioned-host requirement, and
[#14](https://github.com/FlorianRiquelme/grind/issues/14) ruled the PR **is** the Record. This
decision constrains what Grind writes *to* GitHub; it does not begin abstracting over trackers,
which would be a different effort arriving as an adapter.
