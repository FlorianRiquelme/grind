---
description: "Fires on edits to the Enqueue Job field table or its parser — the template ⇄ job::from_issue_json contract seam."
globs:
  - "skills/enqueue/JOB-TEMPLATE.md"
  - "src/job.rs"
  - "tests/enqueue_template.rs"
condition:
  - "from_issue_json"
  - "Handoff SHA|Anchor artifact|Done predicate|Base branch|Verify entrypoint"
  - "\\|\\s*\\*\\*"
interruptMode: never
---

# Enqueue template ⇄ job parser is one contract with two owners

The JOB field table in skills/enqueue/JOB-TEMPLATE.md is read back by job::from_issue_json (src/job.rs). Seven rows REQUIRED and refused at dispatch: target repo, branch, handoff sha, anchor artifact, done predicate, base branch, verify entrypoint. A value of none / - / n/a / empty IS missing — the refusal exists and the template documents it; never relax either half.

NO Budget ceiling row exists: ADR-0010 withdrew ceilings (spend is recorded, never bounded). intent / model / declared hot paths are OPTIONAL and human-declared — Grind classifies nothing (ADR-0012).

tests/enqueue_template.rs parses the template through the real parser: it catches renames on both sides and never meaning drift. Change either half, then reread the other.

Source: AGENTS.md enqueue constraint; ADR-0010, ADR-0012.
