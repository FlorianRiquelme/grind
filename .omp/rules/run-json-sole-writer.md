---
description: "Fires whenever an edit touches run.json reads/writes, RunRecord or record paths across supervisor, view and cli."
globs:
  - "src/supervisor.rs"
  - "src/view.rs"
  - "src/cli.rs"
condition:
  - "run\\.json"
  - "RunRecord|record_path"
interruptMode: never
---

# run.json has exactly one writer

supervisor is the sole writer of `~/.grind/runs/<id>/run.json`; `RunRecord` is private to `src/supervisor.rs`. Every read path — roster/facts in view, `grind outcomes` in cli, the serve dashboard — observes fresh and persists nothing.

NEVER save back a loaded record from a read path: re-saving what you loaded erases `attempts[]`, which nothing can rebuild, and it happens while the human is watching the dashboard to be reassured (#12, #27). outcome.json (`grind outcomes`) is deliberately separate. Do not harden `tests/topology.rs`.

Source: AGENTS.md → sole-writer constraint; issues #12, #27.
