---
date: 2026-08-30
run: supervised-session
paths: [src/observe.rs, src/policy.rs, src/decide.rs, src/render.rs, src/world.rs, src/supervisor.rs, tests/denied_tools.rs, tests/topology.rs, tests/end_to_end.rs]
statement: A full sweep for tautological tests — tests that pass whether or not the code they claim to guard works — found fourteen across the base (five deleted, four rewritten, five trimmed of dead assertions inside otherwise-live tests); codified the shapes in AGENTS.md.
status: candidate
---

The sweep audited every inline and integration test module against the production code it
sits beside, asking one question per test: what breaks if the guarded code is deleted or
reverted? Anything whose honest answer is "nothing" is a tautology, and a tautology is worse
than a coverage gap — it reads as a guard while guarding nothing.

Deleted outright (no intent to preserve):

- `tests/denied_tools.rs::the_declared_length_matches_the_globs_actually_listed` — parsed the
  declared `[&str; N]` length and compared it to the literal's own element count. rustc
  rejects a mismatch; no compilable state can fail it.
- `tests/topology.rs::a_module_nested_under_a_shared_parent_is_caught` — asserted
  `contains('/')` on its own literals: stdlib behavior, tested against the test's own data.
- `src/observe.rs::every_arm_is_constructed_somewhere_in_this_module` — built an array
  literal and asserted its own length.
- `src/supervisor.rs::a_blocked_run_is_resumable_and_a_completed_or_exhausted_one_is_not` —
  looped over a list the test constructed and asserted its members are not two states the
  list doesn't contain. The real gate (`resume`'s terminal-state short-circuit) is
  exercised end-to-end by scenario G.
- `src/supervisor.rs::the_attempt_list_can_only_grow` — proved `Vec::push` appends.

Rewritten where the intent was real but the mechanism couldn't fail:

- `tests/end_to_end.rs::a_run_whose_log_cannot_be_written_still_exits_on_its_real_verdict` —
  its fault (unwritable `supervisor.log`) was never reached: it resumed an already-completed
  Run, which short-circuits before any log write. Rewritten as
  `a_run_whose_log_cannot_be_written_still_supervises_to_its_verdict`: a Blocked Run is
  cleared and re-enters supervision with the log replaced by a directory, so the resilience
  `say()` actually promises — a log write failure never abandons a Run — is exercised on the
  path that writes.
- `src/supervisor.rs::an_error_ending_takes_precedence_over_a_spoken_done_promise` — its doc
  claimed reverting the `push_stage_entry` seam substitution would fail it, but it called
  `reflect_status` directly, so a revert stayed green. Now a source carrier:
  `include_str!("supervisor.rs")` greps for the exact seam expression and for the promise
  never feeding it, plus the helper's own precedence pins. (Self-match trap: a negative grep
  over `include_str!` of the test's own file matches its own source line — split the literal.)
- `src/world.rs::a_child_reading_stdin_hangs_forever_without_the_null_but_is_still_killed_on_time`
  — `run_bounded` nulls stdin itself, so `cat` reads EOF and exits 0; the deadline is never
  exercised, and the assertion accepted both outcomes. Rewritten as
  `run_bounded_nulls_stdin_so_a_reading_child_reads_eof_instead_of_hanging`, pinning
  `Some(0)` — which fails if the null-stdin contract or the kill path breaks either way.
- `src/supervisor.rs::skills_hash_of_no_files_is_stable_and_not_a_special_case` — a pure
  function compared to itself. Now pins the known answer: the empty hash is the FNV offset
  basis, `cbf29ce484222325`.

Trimmed dead assertions out of otherwise-live tests (kept the real parts):

- `src/policy.rs::a_blocker_is_a_stop_and_never_a_verdict_variant` — the trailing
  `matches!(Stop::Blocked(..), Stop::Blocked(_))` constructed and matched its own value; the
  Debug-name scan over `Verdict` variants stays.
- `src/decide.rs` T3-roster uniqueness block — `panel` builds from a unique-candidates
  literal + filter + take, so duplicates are structurally impossible (the same shape ledger
  165 recorded). Security-count assertions stay; test renamed
  `t3_roster_carries_security_exactly_when_the_diff_hits`.
- `src/decide.rs::red_ci_lands_on_the_verdict_line_and_does_not_hold_the_verdict_open` —
  re-asserted the value the test itself set.
- `src/render.rs` — two trailing fixture-self assertions (a fresh `observation()` matched
  against its own variants; `record.clearances.len()` re-stating the fixture builder).
- `src/supervisor.rs::the_key_is_the_repo_and_the_branch_and_never_a_filesystem_path` —
  first line compared `lock_key` to itself; the two `assert_ne!` lines carry the signal.

Added where a mirror had been hiding a missing direct test:

- `src/world.rs::civil_converts_known_epochs_to_their_utc_fields` — `civil` had zero direct
  coverage; two neighboring tests computed expectations *through* `civil`, so any bug there
  escaped both sides. Known-answer epochs pinned (epoch 0, day boundary, leap day, a
  familiar large epoch), verified against the host `date -u -r`.

Defensible and left alone: the same-file asset pins in `style.rs`/`script.rs` (they
falsify accidental future drift of the shipped CSS/JS even though the expected literals were
transcribed from the constant), the external-`date`-anchored zone tests, and the documented
convention carriers (`tests/topology.rs`, `tests/tier_grader.rs`, the AGENTS.md deny-list
mirror). The rule going forward is in AGENTS.md beside the negative-assertions paragraph:
before writing a test, name what breaks if the guarded code breaks — if nothing does, delete
the test or the guard.
