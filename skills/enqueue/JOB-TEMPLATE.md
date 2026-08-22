# The Job body

## The table — the parser's contract

`job::from_issue_json` (`src/job.rs:139`) reads `| key | value |` rows anywhere in the body:
exactly two cells, key lowercased, backticks and asterisks stripped, header and separator rows
dropped. Everything else in the body is prose Grind never reads.

```markdown
| Field | Value |
|---|---|
| **Target repo** | `owner/name` |
| **Branch** | `feat/28-slice-1b-agent-surface` |
| **Handoff SHA** | `723ca913536d279e45549018f022e9d1092bbbec` (`main` after #29) |
| **Anchor artifact** | `docs/plans/2026-08-05-002-slice-1b-plan.md` |
| **Pinned plugin version** | `compound-engineering@compound-engineering-plugin` **3.21.4** |
| **Done predicate** | `just verify` is green and the screensource seam has a passing test |
| **Base branch** | `main` |
| **Verify entrypoint** | `just verify` |
| **Declared hot paths** | `src/decide.rs, src/policy.rs` |
| **Intent** | A settled plan transcribed into one module; the shape is decided. |
```

**The first eight are required and refused at dispatch if missing.** A row reading `none`, `-`,
`n/a` or empty counts as missing, so never write a placeholder into a required row.

**`Done predicate`, `Base branch` and `Verify entrypoint` are the same absence-of-spelling
refusal as a missing Anchor** — blank is refused, not waved through as *nothing to check*.
`Done predicate` is stated so a machine could grade it; `Base branch` is the merge target the
PR opens against; `Verify entrypoint` is handed verbatim to the Run, the repo's own generic
answer to "how do I check this".

**`Declared hot paths` is optional**, and it is human-declared rather than Grind-classified
(ADR-0012): performance-sensitive paths the human already knows about. A row reading `none`,
`-`, `n/a` or empty is the same as no row, and the list is then empty.

`tests/enqueue_template.rs` parses this very table through `job::from_issue_json`, so a required
row renamed on either side turns `just verify` red. That is the whole of the seam.

**`Intent` is optional, and it is a statement about the work's *nature* — never a requirement.**
One line, written only when there is something true to say; the Anchor is where the work itself
is stated, and a second place stating it drifts. A row reading `none`, `-`, `n/a` or empty is the
same as no row, and the built prompt then carries no characterisation of the work at all.

**`Model` — silence was right on both real Jobs.**

There is no `Budget ceiling` row. ADR-0010 withdrew the ceiling and the parser no longer reads
one; a Run is bounded by Attempts that did work, never by spend.

Parenthetical context after a value is fine — it survives the strip and is how a Handoff SHA says
which commit it is.

## The prose

Grind reads none of it; the Run and the human read all of it. Ask about each section, draft it
from the session, and **omit any section with no answer** rather than emitting a thin one.

- **What this is** — one paragraph. Both real Jobs open with the slice and why it exists now.
- **The work** — what to move, build or change, and the shape it lands in. Point at the Anchor for
  the requirements rather than restating them; a second statement of the work drifts from it.
- **Definition of done** — the Verify entrypoint, and any step that must stay intact. Name the
  places where weakening a step will be tempting, and say **report it rather than weaken it**.
- **What to watch** — the risky item the Run should flag rather than assume. This is where
  snapper#28's value sat.
- **Decomposability admission check** — the human's own judgement that the Anchor decomposes into
  deliverables with a workable order. Draft it; never resolve it.
- **Scope recorded** — only when the Job narrows or widens something already decided elsewhere.

## What the Run is told separately

The dispatch prompt already tells every Run that the Anchor artifact is the requirements it must
satisfy, that everything else is discoverable from the branch, and not to re-open decisions the
Anchor records. Don't repeat those in the body.
