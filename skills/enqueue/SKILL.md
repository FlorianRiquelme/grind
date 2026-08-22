---
name: enqueue
description: Turns a prepared branch into a Grind Job — drafts the whole Job issue in the target repo, files it once the human has read it, and closes by offering a detached Dispatch. Use when the user says enqueue, asks to file or queue a Grind Job, or has just finished a plan they want run headlessly by `grind run`.
---

# Enqueue

The single conversational step, with the human present, that turns a prepared branch into a Job.
Run it **from the session that prepared the branch** — the plan is in context, the worktree is the
cwd. A cold invocation is the same steps asking more questions, never a second mode.

Grind reads the Job back with `job::from_issue_json` (`src/job.rs:139`). The table in
[JOB-TEMPLATE.md](JOB-TEMPLATE.md) is that parser's contract, and `tests/enqueue_template.rs`
parses the template's own example table through that parser — so a required row renamed on either
side turns `just verify` red. A change to either still belongs in the same diff: the test catches
a rename, not a meaning that drifted.

## 1. Derive everything you can

```sh
gh repo view --json nameWithOwner -q .nameWithOwner        # target repo
git branch --show-current                                   # branch
gh repo view --json defaultBranchRef -q .defaultBranchRef.name
ls ~/.claude/plugins/cache/compound-engineering-plugin/compound-engineering | sort -V | tail -1
test -f justfile && grep -m1 '^verify' justfile             # verify entrypoint, first guess
test -f package.json && jq -r '.scripts.verify // empty' package.json
```

The **Anchor artifact** is the plan document this session just wrote, as a repo-relative path.
Show it; don't ask for it.

**Always latest** for the plugin: write the newest installed version as a literal `x.y.z`. Never
write a bare `name@marketplace` — `PluginPin::parse` refuses it, which is what keeps `Latest`
unspellable.

**Base branch** derives from `defaultBranchRef` above — write it unless the human names another
merge target in the session.

**Verify entrypoint** derives from the repo the same way `VERIFY_CONTRACT` does: a `justfile`
recipe first, then a `package.json` script. When neither exists, **ask the human** rather than
inventing one — a Job naming no runnable command is an enqueue-time refusal waiting to happen,
not a guess worth writing down.

**Done predicate** is not derivable. Draft it from what this session knows the work to be, and
state it so a machine could grade it: *`just verify` is green and the new endpoint returns 404
for an unknown id* is gradable; *the feature works well* is not — nobody, human or Run, can
check it against evidence.

**Declared hot paths** is asked-for, never derived: **Grind does not classify a path as hot**
(ADR-0012), so this row exists only when the human names one in the session. Leave it out rather
than guessing from a diff or a directory name.

## 2. Refuse a Job on the default branch

If the derived branch **is** the repo's default branch, stop. Do not file. `job::validate_branch`
accepts `main` without a word, so such a Job dispatches cleanly and puts a Run on the default
branch — the one derivation whose failure is silent and unrecoverable.

Offer the repair in the same breath: create `<type>/<issue>-<slug>` off the current HEAD and
continue from there.

## 3. Confirm the Handoff SHA — the only row you ask about

Put **both candidates on screen** and let the human pick:

```sh
git rev-parse HEAD                                # the branch tip
git fetch origin --quiet && git rev-parse origin/<default>   # the default branch tip
```

Real Jobs have used each. Do not guess: the wrong one is invisible until a Run has committed onto
it.

## 4. Run the three advisory checks

Warnings the human may override — never refusals.

```sh
git branch -r --contains <handoff-sha>            # empty → on no remote, so un-dispatchable off this box
test -f <anchor>                                  # missing → Dispatch will refuse
test -d ~/.claude/plugins/cache/<marketplace>/<name>/<version>
```

The third runs against **the host named at the offer**, not this laptop — that is where Grind
resolves the plugin, and always-latest widens the window where a host's cache has not caught up.

Do **not** check the Anchor's *shape* — no R-IDs, no readiness field. It requires none.

## 5. Draft the whole body

Fill [JOB-TEMPLATE.md](JOB-TEMPLATE.md) from what this session already knows: the table, and prose
for the work, the definition of done, what to watch, and the decomposability admission check. Ask
about a section rather than emitting an empty heading, and **omit** what has no answer.

Draft the admission check as the human's words to approve or rewrite. **Never resolve it** — no
"passed", no verdict. Whether a Job is ready is theirs.

Show the whole body and file nothing they have not read.

## 6. File it, and write nothing else

```sh
gh issue create --repo <target-repo> --title "Job: <title>" --body-file <draft>
```

**No label, no assignee, no project board, no milestone** — on any repo, ever. The Queue is
whatever the human's tracker already does; if they want a marker they apply it themselves.

## 7. Offer the Dispatch

Print the command **always**, whatever they answer — declining has to cost nothing, because
declining is what leaves the Job on the Queue.

```
grind run <issue-url>
```

On an explicit yes: **ask which host** if they did not name one. Then detach, because a supervisor
that dies with the pane presents as a Run that got most of the way:

```sh
nohup grind run <url> > ~/.grind/dispatch-<issue>.log 2>&1 &                      # this host
ssh <host> 'nohup grind run <url> > ~/.grind/dispatch-<issue>.log 2>&1 </dev/null &'   # elsewhere
```

Print what you ran, then read the log — a dispatch-time refusal lands in it within seconds, and it
is the only thing that will tell them.

## Never

- **Choose.** Not which Job, not whether a Job is ready, not which host.
- **Gate.** Every check here is coherence or advice; none blocks on a judgement about the work.
- **Classify.** Grind adds and never classifies, and that binds this skill as much as the binary.
