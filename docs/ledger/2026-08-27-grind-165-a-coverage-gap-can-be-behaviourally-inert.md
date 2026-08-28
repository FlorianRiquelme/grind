---
date: 2026-08-27
run: 20260827-063817-grind-165
paths: [src/observe.rs]
statement: A validated "branch never exercised by any test" finding can still be inert — check whether the branch changes output before writing a test for it, or the test asserts nothing a passing test wouldn't already assert.
status: candidate
---

Validate confirmed a TST-1 finding on `disk_headroom`'s `seen_lines` dedup guard: no test ever
supplied two readings with an identical `df -Pk` data line, so the `continue` arm was never
exercised. The finding was real — the branch is genuinely uncovered — but Fixes demoted it to a
residual-risk note instead of writing the obvious test, because the guard turned out to be dead
weight: `tightest.is_none_or(|(_, t)| gib < t)` is a strict `<`, and two readings sharing a data
line necessarily parse to an equal `gib`, which the comparison already rejects with or without
`seen_lines`. A test supplying a duplicate line would pass whether or not the guard existed,
buying no signal. The lesson generalizes past this one guard: before treating an uncovered-branch
finding as actionable, trace what the branch actually changes about output, not just whether a
test path reaches it — an uncovered branch that changes nothing is not a coverage gap worth a
test, it is a guard worth deleting (or a downstream comparison worth loosening so it stops being
redundant).
