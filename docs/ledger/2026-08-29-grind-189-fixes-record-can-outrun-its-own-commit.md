---
date: 2026-08-29
run: 20260828-222129-grind-189
paths: [README.md]
statement: A fixes-round record describing an edit as applied is not proof the edit was committed — a died-and-resumed Run can carry a completed working-tree diff that a prior commit never picked up; Ship should diff the record's claim against git, not just against the record.
status: candidate
---

`fixes/round-1.md` described one edit clearing both `COR-1` and `DOC-5`: reclassifying
`docs/findings/0008-composite-profile-spike.md` in the README bullet at `README.md:117-119`
*and* correcting the Status line's "Seven dogfood Runs" to "Six" at `README.md:130`, calling both
part of the same hunk. The commit that actually landed (`189ab7e`) carried only the bullet; the
Status-line half sat as an uncommitted working-tree change when this Ship stage started, on a Run
whose `run.json` recorded `state: died` — the prior attempt exited before finishing the commit it
described as done. Ship caught it only because it ran `git status`/`git diff` before staging
rather than trusting the round's own prose. The lesson generalizes past this Run: after a died
Run resumes, a stage's own record of what it did is a claim, not a receipt — verify it against
the working tree before assuming the commit history matches the narrative.
