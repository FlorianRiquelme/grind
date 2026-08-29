---
date: 2026-08-28
run: 20260828-212910-grind-187
paths: [docs/findings/0008-composite-profile-spike.md]
statement: A Run reading its own append-only record in flight must write unobserved stages as unobserved and absent fields as absent — inferring a missing value from a related one (the adapter from the class route) states more than the record does.
status: candidate
---

The spike's deliverable table was written at Work time, when `stages[]` held
only plan, triage and plan-review. The tail rows were marked `unobserved`
rather than projected, and Triage's entry — which names model
`claude-opus-5`, 3 turns and `$0.321835` but carries **no `backend` key**
(the field is `skip_serializing_if = "Option::is_none"` on `StageEntry`,
`src/rung.rs`) — had its adapter recorded as absent rather than inferred
from the strong route that dispatched it. The distinction is load-bearing
for a spike whose entire value is observing which adapter executed which
stage: a reader can audit absence, but an inference indistinguishable from
observation poisons the record the Run exists to produce. When a
`skip_serializing_if` field is unset, "the record does not say" is the
observation.
