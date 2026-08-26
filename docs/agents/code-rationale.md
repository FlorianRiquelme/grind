# Code rationale

Inline comments were dropped from the source: prose beside code rots faster than code changes,
and the primary reader re-reads the code anyway. What remains in-source is doc comments
(`///`, `//!`) only. This file holds the knowledge that was worth keeping but does not belong
in a function description — invariants, external-system quirks, and bug history. Anchors name
the symbol each note was extracted beside; symbols move, so grep before trusting one.

New code ships comment-free (AGENTS.md → Agent skills → Code comments). Record such knowledge
here, or in an ADR when it decides something.

## src/supervisor.rs

- **take_lock**: The two lock failures must stay distinct refusals: WouldBlock names *another
  Run* holding the branch, while an unreadable identity reads as "could not determine".
  Collapsing the two sends the human hunting the roster for a collision that does not exist.
- **resume**: Refusing to re-enter an already-terminal Run is keyed on the recorded attempt
  number, never the state word. Each further manual resume deliberately spends one more
  attempt and stops again, so the guard survives state-word drift between versions.
- **maybe_dispatch_reflect**: Reflect's cwd is the run directory, never the worktree — a
  string handed as worktree is not proof of where the child ran. The native path goes through
  the same seam as ladder attempts, and build_reflect never passes `--model`, so routing stays
  with the declared strong model.
- **skills_hash**: FNV-1a inputs are concatenated WITH a separator so ("ab","c") and
  ("a","bc") hash differently. Identity check only, not a security boundary — deliberately
  dependency-free per ADR-0005.
- **adopt_or_create_worktree** (#81): Jobs declaring a brand-new branch need
  `git worktree add -b`; plain `add` refuses with "fatal: invalid reference" because the
  branch exists nowhere yet. `-b` binds to the NEXT argument, so argument order distinguishes
  `add -b <branch> <path>` from `add <path> <branch>`.
- **adopt_or_create_worktree**: Git reports worktree paths canonicalised (macOS:
  `/private/var` vs `/var`), so adopt-vs-create comparison must go through the filesystem
  (same-file check), never string equality of paths.
- **run_ship_babysit_attempt** (#106): the CI-babysit round must ride resolve_stage_model's
  stage-resolved model, not the raw Job pin — an unpinned Job's round once ran the harness
  default while its session had switched models mid-flight.
- **resume_all**: Process start-stamp liveness (`ps -p <pid> -o lstart=`) can be
  unimplementable or unreadable on a host; that reads as "could not tell" and fails toward
  declining re-entry. A host where every reading is blind would silently never re-enter —
  accepted as the safe direction.
- **dispatch**: The agent-backend declaration (ADR-0017) is snapshotted once, before any other
  host-readiness check, into the record; later stages read the record, never the environment.
  The Run-level session_id field survives only so pre-ladder records render; new records give
  Plan its own derived per-stage session UUID.
- **tests::serde drift carrier**: The duplicated-field-name carrier lives in this module's
  tests because `view` cannot name the private RunRecord and the compiler is blind to exactly
  this drift. Writer fields use skip_serializing_if, so fixtures MUST set optional fields
  explicitly or the drift the test exists for hides behind the attribute.

## src/job.rs

- **split_hot_paths**: Split hot-path cells on the FIRST interior `|` only: values may carry
  escaped or bare pipes, and splitting on every pipe turned such a row into three
  silently-dropped cells, so dispatch refused on data that was there (row shape from
  docs/findings/0002).
- **reachability**: A fetch that could not be made leaves every ancestor answer "unobserved"
  — an unanswered question is neither a clean bill of health nor a refusal. This is the row
  the whole table exists to get right.
- **host_items**: The host-item table arrives via include_str!, never a runtime file read:
  reading it at run time would name std::fs inside src/, which tests/topology.rs forbids.
- **extract_handoff_sha**: Six hex characters is not a commit: yielding an abbreviated sha
  hands `handoff..HEAD` a revision that resolves to something else or to nothing. Full
  40-char shas only.
- **HostItem classifiers (tests)**: Membership alone cannot catch a mis-marked item — one
  quietly demoted from *dispatch* to *doctor* stops running before every Dispatch with nothing
  saying so. That demotion is the only failure mode this list has, hence the dedicated
  classifier test.

## src/cli.rs

- **run**: The `clear <id> …`, `resume --all`, and boot arms must precede the generic arm:
  slice patterns match by position, so otherwise `--all` binds as run_id and unparsed flags
  become tokens of an "unknown command". There is intentionally NO `list` verb — picking "the
  one in flight" would pick a zombie.
- **finish**: The Handback's printed verdict and the process exit code are ONE computation
  (they used to be computed twice moments apart and disagreed). An unreadable record exits
  nonzero: a bare 0 would tell a caller checking `$?` that everything answered when nothing
  did.
- **check_with_probe**: `systemctl --user is-enabled` answers enabled purely from the symlink,
  with or without linger — without `loginctl enable-linger` a --user unit never starts on a
  headless box. The check is therefore conjunctive; asking only the first half is the
  silent-pass shape it was added to prevent.
- **check**: Doctor's OS row uses cfg! rather than std::env::consts::OS because
  tests/topology.rs string-matches the literal `std::env` and asserts only `world` names it —
  the idiom would break `just verify`.
- **probe_declared_endpoint**: Probe ONLY the DECLARED base URL (default when undeclared),
  never probe the hardcoded default regardless: the base-url token exists for self-hosting and
  probing the default defeats it. An unreadable/unparseable ~/.grind/agent means no probe at
  all — doctor must never guess.
- **status_one**: "Doing" in the status panel is the last thing the assistant authored — a
  tool CALL. The tool RESULT is not it: that is the world talking.

## src/world.rs

- **run_bounded**: Reap-before-signal ordering IS the safety argument: once wait() reaps the
  child its pid is free for reuse, so signalling the group afterwards can SIGKILL whatever
  process group inherited that number — a successful kill of the wrong thing, silently. The
  clean-exit path deliberately cannot do this (try_wait already reaped; a succeeding command
  has not earned having its background children killed).
- **run_bounded**: One deadline is shared across BOTH pipes (fixed grace, independent of
  limit): the case that spends it is a clean exit whose backgrounded grandchild holds the
  write ends, where per-pipe graces would stack into double the wait. Reader threads hand
  bytes over channels and are never joined unconditionally — an unbounded join after a
  grandchild keeps the pipe open hangs forever; likewise >64KiB on both pipes deadlocks any
  sequential read-stdout-then-read-stderr design (kernel pipe buffer ≈64KiB).

## src/view.rs

- **observe_fresh**: Grit-surface readers follow observe_fresh's discipline: read world via
  world::*, persist nothing; an absent or unparseable artifact folds into "nothing to show",
  never a crash.
- **Live**: Live once lived in claude.rs while it was the only transcript reader, forcing
  cli/serve to construct a claude::Live for native Runs; it moved beside RunView because both
  claude::live and native::live produce it. Nothing in this module reads a transcript itself —
  the adapters do.

## src/observe.rs

- **checks**: GitHub's statusCheckRollup is present-and-null when a PR has no checks
  configured at all; the classifier treats that as nothing-pending AND nothing-red (both
  Present(false)) — an observation, not blindness.
- **checks**: STARTUP_FAILURE (required check's job never ran its steps) and STALE (GitHub
  retired the result) must classify as red: neither is pending, and without naming them red,
  completion could proceed over a required check that never produced a verdict.
- **diff_facts**: `git diff --numstat` spells binary files as `-<TAB>-<TAB><path>`; such rows
  contribute no line count and are skipped along with lockfile/generated churn.
- **process_start_stamp**: Test fixtures arrive via include_str!, never the filesystem at run
  time: naming std::fs inside src/ is forbidden by tests/topology.rs.
- **boot_one_shot**: See the conjunctive linger check under src/cli.rs → check_with_probe;
  doctor going green on a unit that will never run at boot is precisely the failure this check
  exists to catch.
- **repo_of_remote**: Remote URLs are parsed to owner/name with everything before the host
  dropped first, so a credential-bearing HTTPS origin (embedded token on a misprovisioned
  host) is discarded before the pairs are read; the raw URL never leaves the function.
- **RunOutcome**: ADR-0009 put clippy in the verify recipe, but under a library target a `pub`
  enum no longer raises the dead-code warning, so an exhaustive match over the enum stands in
  for it — a representable-state statement enforced by test instead of lint.

## src/decide.rs

- **signals_of**: The head-commit PR lookup asks with `--state all` deliberately: a
  human-closed PR stays Observed::Present (it belongs in the Handback), but completion ANDs
  pr_open, so a closed PR holds the Run Incomplete — otherwise a four-for-four Run reported
  Completed printing the closed PR's URL on the Job issue.
- **verify_contract**: A verify step trimmed to green but left behind as a justfile comment
  (`# cargo clippy ...`) must read as missing — the exact trim the contract exists to catch
  (#82). Stripping `#`-to-EOL segments risks only noisy "missing" reports, never a false
  green; package.json scripts carry no comments and are untouched.
- **tier calibration tests**: First tier-calibration datapoints (facts from findings docs +
  `gh pr view`): snapper#23 Tauri scaffold — DeploySurface alone reaches T3; snapper#30
  ($64.32 is the design's T1/T2 cost-band reference) reached T2 via surface_delta +
  Concurrency; grind#84 docs-only still cleared loc_t1 → T1 ("any ambiguity rounds up");
  grind#89 +1007/-34 across 10 files → T2. Docs-only does not exempt a diff from the size
  signal.

## src/render.rs

- **job_comment**: outcome and calibration are chronologically always None here: `grind
  outcomes` collects both only after this comment posts at the Run's terminal moment. They are
  bound-and-unused (not `_`) so a caller who changes that ordering meets a live binding instead
  of silent absence.
- **fanout_line**: fanout returns Absent once every spawn has paired with a tool_result, so
  the old `Present(agents) if agents.is_empty() => "none"` arm was unreachable — Absent and
  Unobservable share negative_mark rather than ever printing "none".
- **cleared_twice (tests)**: Clearance rendering rules by requirement id: R3/R4 — the latest
  note rides all three surfaces while older rows survive in the record unseen; R6 — no surface
  prints anything when no clearance exists (a permanent negative never prints).

## src/page.rs

- **board**: Columns are filed under the recorded state string but labeled only when rendered
  — keying a display label where the record says "completed" loses every card to a silent
  miss.
- **board**: The hold lane always renders its second slot even when empty: it names the repair
  path (`grind cleared`, then resume).
- **proposal_queue_section**: Reflect proposal paths render as plain text, deliberately not
  links: the raw-evidence route whitelists six fixed names (R5) and an artifact's own path is
  not among them.
- **waterfall**: Gaps between consecutive attempts render as hatched sleep bands when the gap
  is a bounded re-entry sleep — ADR-0004 refuses to hide the wall the Run slept against.
- **run_page**: The following-log pane is also on the full page so deep links land with the
  log already attached.

## src/policy.rs

- **reset_time_sleep**: If arithmetic lands exactly on the reset hour (zero seconds away), a
  full 24h cycle is returned instead of a zero-length sleep that would spin — the caller
  handles it as its usual sleep-then-reenter.
- **next**: Red CI does not hold a Completed verdict open — it buys exactly one CiBabysit
  invocation, then stops. An Uncorroborated verdict stops immediately because a
  self-declared-finished session would re-emit its promise until the budget ran out.
- **the_consecutive_wait_bound_survives_a_re_entry (tests)**: The consecutive-Wait counter
  derives from the persisted Attempt list so restarts cannot reset it — `resume --all`
  re-enters rate-limited Runs at boot by design, so a loop-local counter would never terminate.
- **replaying_run_2s_eight_attempt_shapes…(tests)**: Fixtures replay docs/findings/0002
  (Run 2's real attempt costs, $37.04/187 turns down to $0 waits); three working Attempts
  against a recorded budget of eight is not exhaustion.

## src/tools.rs

- **resolves_within**: RootDir/Prefix path components are unreachable after the leading-slash
  strip but treated defensively as contained rather than panicking — a gate must never be what
  crashes a Run.
- **real_denied_tools_entries_refuse_their_shell_commands (tests)**: All DENIED_TOOLS entries
  are gated against realistic evasive spellings because a prior gap left most globs exercised
  only by membership checks; nested-shell coverage exists because no glob names sh/bash/eval
  itself.
- **write_file_is_refused_under_report_only_denials…(tests)**: Cross-backend naming: the
  native toolkit calls its writer `write_file`, so report-only denial sets must carry that
  name itself, not just claude-code's `Write`/`Edit`.
- **DENIED_TOOLS matcher history**: see AGENTS.md → "DENIED_TOOLS … safety property" for the
  full story (position-independent globs, `-f` inside branch names, sh/bash/-c/-eval outer
  refusals, suffix-candidate generation).

## src/serve.rs

- **parse_request**: One tolerated leading CRLF is stripped: clients sometimes send a stray
  one after pipelining.
- **evidence_allowed**: The numbered-evidence whitelist grew twice behind adapter changes and
  both times dashboard links resolved to 404s — native writes messages-N.jsonl and reflect
  writes a reflect- pair through the seam; any new adapter artifact needs a route entry or its
  evidence links break.
- **http_date**: Weekday index is `(days mod 7 + 4) % 7` because 1970-01-01 was a Thursday
  with Sunday = 0.
- **serve**: One thread per connection so a dead client can never stall the accept loop.

## src/attempt.rs

- **tests::each_forbidden_operation_has_a_glob_that_refuses_it…**: The tests import
  glob_matches/subcommands_of from crate::tools rather than keeping local copies: a previously
  hand-maintained twin drifted silently while `just verify` stayed green, and tools' copy is
  the native backend's entire enforcement barrier.
- **DENIED_TOOLS glob specifics**: see AGENTS.md → "DENIED_TOOLS … safety property".

## src/native.rs

- **NativeAdapter::run (transcript.truncate)**: Truncating messages-N.jsonl before any log
  call is load-bearing for resume (#23): a crashed attempt N was never recorded, so a resume
  recomputing the same n must start the file empty instead of appending after the dead
  attempt's partial content.
- **scan_latch**: Latch transcripts are ordered by parsed attempt number, never lexicographic
  filename sort — messages-10.jsonl sorts before messages-2.jsonl lexicographically, which
  would let the wrong ProtocolSelected win.
- **next_action**: finish_reason="length" must back off, never latch Text protocol: a
  max-tokens cutoff names nothing about tool capability, and latching on it would pin the
  whole Run to text mode off one long reply (P1 regression).
- **synthesize**: Synthesized attempts must carry honest total_cost_usd Some(0.0), not None:
  None makes Attempt::is_wait false for every native Attempt regardless of num_turns (P1),
  breaking the Wait/budget accounting.
- **live**: Newest-transcript pick relies on Option mtime ordering: None sorts below Some, so
  a file whose mtime cannot be read still wins when it is the only one present but never beats
  a timestamped one; the path breaks second-level ties deterministically.

## src/net.rs

- **SseAssembler (finish_reason handling)**: OpenRouter maps upstream provider failures to
  finish_reason="stop" with an empty delta, stashing the real cause in native_finish_reason —
  only "stop" and "tool_calls" count as legitimate endings, and reading continues past finish
  because usage chunks follow.
- **classify_sse_line**: Colon-prefixed SSE lines are comments/keepalives — OpenRouter emits
  ": OPENROUTER PROCESSING" mid-stream; they must classify as Keepalive, not data.
- **read_sse_stream**: The MAX_SSE_LINE cap is per-line, enforced by a fresh take() per read
  call rather than a cumulative stream bound, so long healthy streams are never cut off
  mid-way. EOF without [DONE] is legal (the done flag exists for parity only).

## src/claude.rs

- **ClaudeCodeAdapter::run**: Unrecoverable pre-spawn IO failures (prompt write, spawn) are
  deliberately routed through the normal classifier instead of panicking: parse_ok comes back
  false, which Attempt::is_wait requires, so the failure can never be mistaken for a zero-cost
  Wait looping forever.
- **classify**: Absent vs present-and-empty on the `result` key are distinct facts (value.get,
  not unwrap_or_default): folding them once made a DONE promise under a renamed key read as
  done_promise:false, indistinguishable from genuine silence — the reason subtype carries a
  second synthetic value "result-field-missing" beside "unparseable-output".

## tests/end_to_end.rs

- **sandbox()**: The sandbox's `origin` is a local bare repo because Dispatch fetches before
  inspecting the worktree, so origin must be reachable for "no network" to stay structural. It
  is inited with explicit `-b main`: otherwise HEAD follows init.defaultBranch,
  `remote set-head -a` cannot resolve origin/HEAD (which base drift reads), giving green
  laptop / red CI from one config difference.
- **scenario_d_a_rate_limit_announces_the_recorded_sleep…**: The rate-limit fixture's prose
  names a stated reset time ("resets 5pm (Europe/Berlin)"), so decision 5 parses it and the
  announced sleep is location-dependent, capped at 12h. The scenario deliberately pins only
  that something bounded is announced, never a literal figure.
- **resume_all_re_enters_no_run_whose_recorded_supervisor_is_alive**: A `ps` that cannot spawn
  used to yield no start stamp, and "no stamp" collapsed into "supervisor gone", re-entering
  every Run on the host at boot with nobody watching. The safe direction chosen is to decline
  loudly (exit 2) rather than skip in silence.
- **an_unhealthy_but_fully_observed_run_exits_zero**: Deliberate refusal of the universal CLI
  idiom "non-zero means bad": following it would grow a health gate through the back door. An
  exhausted Run whose status answered observably still exits 0; exit codes report
  observability, never health.
- **fan_out_is_recorded_per_attempt_and_never_cumulatively** (#51): the Run transcript is one
  append-only file resumed per Attempt, so reading the whole file on Attempt N counted Attempts
  1..N and summed pairs published "12 spawned, 12 returned" for six actual spawns. Fanout
  totals must be per-Attempt.
- **resume_at_the_attempt_budget_starts_nothing_whatever_the_state_word_says**: `supervise`
  runs one attempt BEFORE consulting policy::next, so a guard keyed on the state word walks a
  Run sitting exactly at its recorded budget into another attempt, forever.
  Uncorroborated/Unobserved are deliberately resumable, hence the budget check; a recorded
  Attempt in this project costs $7–$37.
- **a_worktree_behind_the_handoff_sha_refuses_at_second_zero**: This check replaced a string
  comparison that printed the same "behind" note for a harmlessly-ahead worktree and proceeded
  anyway; the signer outage, denied force-push, and five hours of `pr: null` were downstream
  of it. Refusal at second zero is intentional.

## tests/sse_native.rs

- **abnormal_native_finish_fails_the_attempt_without_rate_limit**: OpenRouter masked-failure
  shape: finish_reason says "stop" while the real cause hides in native_finish_reason (e.g.
  "network_error") with an empty delta. An abnormal finish while tools were sent counts as a
  tools-array rejection and latches Text mode immediately.
- **a_completed_stage_without_the_sentinel_promises_nothing** (#139): a clean stage ending is
  a fact about the loop, not the work; synthesizing a done-promise from it forced the
  Run-level verdict after stage one and terminated the ladder as Uncorroborated. Endings are
  never synthesized into promises.

## tests/compile_fail.rs

- **a_read_path_reaching_the_writable_record_type_does_not_compile**: Assertions match
  diagnostic CODES (E0603), never message text — messages move between rustc releases.
  Absence of a "help: consider making" fix is asserted too: rustc offering no repair is what
  makes the privacy wall undecidable rather than merely inconvenient.
- **the_unmodified_crate_compiles_the_same_way**: The baseline control is load-bearing:
  without compiling the unmodified scratch crate, a crate failing for an unrelated reason
  reads as both negative cases passing.

## tests/lock.rs

- **two_worktrees_of_one_repo_on_one_branch_collide**: Collision messages must never say
  "another Run": for `resume`/`cleared` the holder may be the named Run's own supervisor, and
  sending the human hunting for a phantom second Run is wrong. Also: there is no `running`
  state in the record, so a SIGKILLed supervisor would leave a Run dispatched forever — the
  OS-held lock exists precisely because state-based checks have that failure mode;
  could-not-determine and collision are never folded together.

## tests/transcript.rs

- **the_transcript_slug_follows_the_symlink_the_way_claude_records_it** (#82): Claude slugs
  the RESOLVED clone path, so on macOS `~/.grind/repos/<owner>/<name>` resolves to
  /private/var/... vs /var/... and a raw-string slug named a nonexistent file. A worktree that
  no longer canonicalizes falls back to the raw string, keeping old records' transcript paths
  computable.

## tests/topology.rs

- **no_path_in_src_classifies_an_issue**: Each gh namespace needs both flag spellings because
  `gh issue edit` takes --add-/--remove- while `gh issue create` takes the bare flag, and
  substring matching means "--add-project" does NOT contain "--project" — the bare-flag list
  alone let the edit spelling through.

## tests/denied_tools.rs

- **every_built_argv_carries_all_base_denials_regardless_of_stage**: DENIED_TOOLS is `[&str;
  N]` on purpose: the fixed length makes adding a glob without bumping N a compile error, and
  the source-parsing assertion guards against the parser silently missing a line.
