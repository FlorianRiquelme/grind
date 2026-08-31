---
date: 2026-08-31
run: supervised-session
paths: [src/cli.rs, src/render.rs, src/page.rs, tests/blocker_surfaces.rs]
statement: The third instance of the derive-from-the-record class was not a drifted fact but a projection that never reached the fact set — `cli::status_one` hand-built its own `SingleRun` list instead of calling `view::gather`, so `grind status <run-id>` had no `blocker` field to render and answered a blocked Run with silence. Closing an instance of that class means routing the bypassing call site through the one derivation, not correcting the value it computed.
status: candidate
---

`2026-08-26-grind-158-renderers-derive-from-the-record.md` named the class, and two instances
were closed under it: one model derivation serving both surfaces, and a handback naming the
model the record declares. Both fixed a *value* that two renderers computed differently. A
third instance stayed available because the class has a second shape — a renderer fed by a
fact set that was never `Facts` at all.

`SingleRun` overlapped `Facts` on seven of nine fields and lacked `blocker`, `coverage`, both
Decisions, `outcome` and `calibration`. Nothing about it was wrong on its own terms; it simply
could not carry a fact it had no field for. So a Blocker rendered three ways — the Handback
named it with the repair, the dashboard named it bare, and the command
`docs/agents/run-observation.md` points humans at named it not at all.

The fix deletes the second fact set rather than widening it: `render::run_view` now takes
`(&Facts, &Live, &Observed<bool>)`, the three arguments `page::run_page` already took, for the
same reason — `Live` is derived per backend and `supervisor_here` reads a live process, neither
of which `gather` can do. `render::blocker_note` is the one composition of *what must be
cleared* plus `repair_hint`, and all three surfaces call it.

The carrier is `tests/blocker_surfaces.rs`: one blocked record, three renderers, asserted
together because no single module's tests span `render` and `page`. Reverting any one surface
to its own composition turns it red.
