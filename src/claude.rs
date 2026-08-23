//! Adapter #1: today's `claude -p` behavior, moved verbatim behind the seam.
//!
//! Everything here is a **move, not a rewrite**: the argv builders and classifier came from
//! [`crate::attempt`], the transcript discovery/matchers from [`crate::view`] — same bodies,
//! same doc comments, only the module path changed. The types that stayed behind
//! ([`crate::attempt::Invocation`], [`crate::attempt::Attempt`], the denial lists) are the
//! shared vocabulary both adapters consume.

use crate::attempt::{
    Attempt, Clearance, Conditions, DENIED_TOOLS, Invocation, Mode, StageConditions, StageContext,
    denied_for, denied_for_reflect, is_rate_limited, mentions_limit, normalise, reflect_session_id,
    stage_session_id, text_at,
};
use crate::observe::{Observed, Reason};
use crate::rung;
use crate::runner::{Backend, RunSpec, StageRunner};
use crate::view::one_line;
use crate::world;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The one backend branch for this adapter (R1 lives in [`crate::runner::runner_for`]).
impl StageRunner for crate::runner::ClaudeCodeAdapter {
    fn backend(&self) -> Backend {
        Backend::ClaudeCode
    }

    /// Today's supervisor execution sequence, folded here byte-for-byte: prompt file first,
    /// then the recorded spawn, then read-back and classification, then the fan-out arithmetic
    /// over this adapter's own `home`. The stamps use the same `world::now_iso()` calls in the
    /// same order relative to the spawn; the pre-run line count is read while no child is
    /// running, exactly as the supervisor's `transcript_lines_for` did.
    fn run(&self, spec: &RunSpec) -> Attempt {
        let n = spec.attempt_n;
        let started_at = world::now_iso();
        // How much of this session's transcript exists right now — read while the child is not
        // running, so the file is quiescent and the last line is whole. A fresh session has
        // nothing to skip; that is zero lines.
        let already_written = transcript_lines(&self.home, spec.worktree, spec.session_id);
        let raw = run(
            spec.invocation,
            spec.cwd,
            &spec.run_dir.join(format!("attempt-{n}.prompt.txt")),
            &spec.run_dir.join(format!("attempt-{n}.stdout.json")),
            &spec.run_dir.join(format!("attempt-{n}.stderr.log")),
        )
        .unwrap_or_else(|reason| {
            // Unrecoverable local IO: today's supervisor turned this into a Refusal that
            // aborted the Run without recording an Attempt. The seam is infallible by design,
            // so the same unrecoverable shape surfaces loudly here instead.
            panic!("attempt {n}: {reason}")
        });
        raw.classify(n, spec.invocation.mode(), &started_at, &world::now_iso())
            .with_fanout(fanout_of(
                &self.home,
                spec.worktree,
                spec.session_id,
                already_written,
            ))
    }
}

/// How much of **one session's** transcript exists right now, in lines. A transcript that is
/// not there yet is zero rather than a refusal: a fresh session has nothing to skip, and that
/// is the same answer whether the session is the Run's old mega-session or one stage's own.
fn transcript_lines(home: &Path, worktree: &str, session_id: &str) -> usize {
    let transcript = transcript_path(home, worktree, session_id);
    match world::read_to_string(&transcript) {
        Ok(text) => text.lines().count(),
        Err(_) => 0,
    }
}

/// The fan-out arithmetic over one named session's transcript, over the lines appended since
/// `already_written`.
fn fanout_of(
    home: &Path,
    worktree: &str,
    session_id: &str,
    already_written: usize,
) -> Observed<(u64, u64)> {
    let transcript = transcript_path(home, worktree, session_id);
    match world::read_to_string(&transcript) {
        Ok(text) => fanout_since(&text, already_written),
        Err(said) => Observed::Unobservable(Reason::saying(&format!(
            "the transcript could not be read: {said}"
        ))),
    }
}

/// Write-capable Bash forms denied on the two fan-out panels (`Review`, `Validate`) and on
/// Reflect: none of the three ever touches a worktree, and denying `Write`/`Edit` alone does not
/// reach a shell command that mutates one the same way. `git push*` is denied outright — a panel
/// or Reflect never pushes.
pub(crate) const PANEL_BASH_FORMS: [&str; 10] = [
    "Bash(git commit*)",
    "Bash(git add*)",
    "Bash(git apply*)",
    "Bash(git stash*)",
    "Bash(mv *)",
    "Bash(cp *)",
    "Bash(rm *)",
    "Bash(tee *)",
    "Bash(sed -i*)",
    "Bash(git push*)",
];

/// Resumed mid-stage, told to pick up its own return rather than the Run's — the stage-level
/// sibling of [`REENTRY_PROMPT`]. Each stage owns its own session, so "the pipeline" of the old
/// prompt narrows to "this stage".
pub const STAGE_REENTRY_PROMPT: &str = "You were interrupted mid-stage and have just been resumed.

Re-read this stage's own return file and the working tree to establish what you had already
done, then continue this stage from where it left off. Do not restart work this stage already
completed, and do not redo a stage the ladder has already advanced past.

Everything in the original instruction still applies — especially: never weaken, trim or
skip a step of `just verify` to make it pass.";

/// The first Attempt for a stage, opening that stage's own session.
pub fn stage_dispatch(conditions: &StageConditions, ctx: &StageContext) -> Invocation {
    build_stage(conditions, ctx, Mode::Dispatch, stage_dispatch_prompt(ctx))
}

/// A later Attempt for a stage, resuming that stage's own session — never the Run's.
pub fn stage_resume(
    conditions: &StageConditions,
    ctx: &StageContext,
    cleared: Option<&Clearance>,
) -> Invocation {
    build_stage(
        conditions,
        ctx,
        Mode::Resume,
        stage_resume_prompt(ctx, cleared),
    )
}

/// The composition unit C's loop calls once it has decided a stage's next Attempt is Dispatch or
/// Resume. `Mode::CiBabysit` never routes here: Ship's babysit round continues `<run>-ship`'s own
/// session through the existing [`ci_babysit`] builder, not a fresh stage composition — there is
/// no stage-shaped babysit prompt to build.
pub fn stage_invocation(
    conditions: &StageConditions,
    ctx: &StageContext,
    mode: Mode,
    cleared: Option<&Clearance>,
) -> Invocation {
    match mode {
        Mode::Dispatch => stage_dispatch(conditions, ctx),
        Mode::Resume => stage_resume(conditions, ctx, cleared),
        Mode::CiBabysit => {
            unreachable!("babysit continues Ship's session via `ci_babysit`, never a fresh stage")
        }
    }
}

/// Reflect's first Attempt, opening `<run>-reflect`. Not a rung — [`rung::Stage`] has no
/// variant for it (the design's own words: *deliberately not an eleventh stage*) — so it never
/// goes through [`stage_invocation`]; the supervisor calls this directly once a terminal
/// observation lands. Dispatched with the **run directory** as cwd rather than the worktree
/// (unit C's job), so there is no repo tree under the session for a `Write`/`Edit` denial to
/// matter over — its worktree protection is [`denied_for_reflect`]'s write-capable Bash-form
/// denials instead.
pub fn reflect_dispatch(conditions: &StageConditions, skill_text: &str) -> Invocation {
    build_reflect(conditions, skill_text, Mode::Dispatch)
}

/// A later Attempt for Reflect, resuming `<run>-reflect` — bounded to one re-entry by the
/// supervisor, never by this builder.
pub fn reflect_resume(conditions: &StageConditions, skill_text: &str) -> Invocation {
    build_reflect(conditions, skill_text, Mode::Resume)
}

fn build_reflect(conditions: &StageConditions, skill_text: &str, mode: Mode) -> Invocation {
    let session_id = reflect_session_id(conditions.run_id);
    let mut argv = vec![
        conditions.claude_bin.to_string(),
        "-p".to_string(),
        "--output-format".to_string(),
        "json".to_string(),
        "--permission-mode".to_string(),
        "bypassPermissions".to_string(),
    ];
    match mode {
        Mode::Dispatch => {
            argv.push("--session-id".to_string());
            argv.push(session_id);
        }
        Mode::Resume | Mode::CiBabysit => {
            argv.push("--resume".to_string());
            argv.push(session_id);
        }
    }
    argv.push("--disallowedTools".to_string());
    argv.extend(denied_for_reflect());
    Invocation::build(argv, skill_text.to_string(), mode)
}

fn build_stage(
    conditions: &StageConditions,
    ctx: &StageContext,
    mode: Mode,
    prompt: String,
) -> Invocation {
    let session_id = stage_session_id(conditions.run_id, ctx.stage);
    let mut argv = vec![
        conditions.claude_bin.to_string(),
        "-p".to_string(),
        "--output-format".to_string(),
        "json".to_string(),
        "--permission-mode".to_string(),
        "bypassPermissions".to_string(),
    ];
    if let Some(model) = ctx.model {
        argv.push("--model".to_string());
        argv.push(model.to_string());
    }
    match mode {
        Mode::Dispatch => {
            argv.push("--session-id".to_string());
            argv.push(session_id);
        }
        Mode::Resume | Mode::CiBabysit => {
            argv.push("--resume".to_string());
            argv.push(session_id);
        }
    }
    // No `--plugin-dir`: a stage invocation names no plugin.
    // No `--max-budget-usd`: ADR-0010, spend is recorded, never bounded.
    argv.push("--disallowedTools".to_string());
    argv.extend(denied_for(ctx.stage));
    Invocation::build(argv, prompt, mode)
}

/// The skill text verbatim, then a bounded context block naming what the skill's own prose
/// cannot know until dispatch: where this Run's returns and artifacts go, which worktree it
/// runs in, and the Job rows the skill leans on (branch, base branch, verify entrypoint, done
/// predicate, Anchor). Plan alone also carries injected notes/lessons.
fn stage_dispatch_prompt(ctx: &StageContext) -> String {
    format!(
        "{skill}\n\n---\n\n{context}{notes}",
        skill = ctx.skill_text,
        context = stage_context_block(ctx),
        notes = plan_notes_block(ctx),
    )
}

/// The same bundle, with the stage-level re-entry paragraph (and, when one was recorded, the
/// latest clearance note) composed after the context block.
fn stage_resume_prompt(ctx: &StageContext, cleared: Option<&Clearance>) -> String {
    format!(
        "{skill}\n\n---\n\n{context}\n\n{reentry}{clearance}{notes}",
        skill = ctx.skill_text,
        context = stage_context_block(ctx),
        reentry = STAGE_REENTRY_PROMPT,
        clearance = stage_clearance_paragraph(cleared),
        notes = plan_notes_block(ctx),
    )
}

/// **Default is silence**, the same rule the mega-session's `reentry_prompt` follows: with no
/// note the paragraph renders as nothing, and the latest clearance is framed as an account to
/// check against what the stage now observes, never as current fact to trust blind.
fn stage_clearance_paragraph(cleared: Option<&Clearance>) -> String {
    match cleared {
        Some(clearance) => format!(
            "\n\nSince you stopped, the human reports (recorded {at}): {note}\n\nThat is their \
             account of what changed in the world, from the moment it was recorded. Check it \
             against what you now observe: do not spend turns re-probing an obstacle the note \
             says is cleared and the world confirms — but where the world in front of you \
             contradicts the note, trust what you observe and say so plainly.",
            at = clearance.cleared_at,
            note = clearance.note,
        ),
        None => String::new(),
    }
}

fn stage_context_block(ctx: &StageContext) -> String {
    format!(
        "Stage:             {stage}
Stages directory:  {stages_dir}
Worktree:          {worktree}
Job branch:        {branch}
Job base branch:   {base_branch}
Verify entrypoint: {verify}
Done predicate:    {done}
Anchor artifact:   {anchor}

Every return and artifact this stage writes belongs under the stages directory named above,
never elsewhere.",
        stage = ctx.stage,
        stages_dir = ctx.stages_dir,
        worktree = ctx.worktree,
        branch = ctx.job.branch,
        base_branch = ctx.job.base_branch,
        verify = ctx.job.verify_entrypoint,
        done = ctx.job.done_predicate,
        anchor = ctx.job.anchor,
    )
}

/// **Default is silence**, Plan-only: every other stage leaves `notes` `None` and this renders
/// nothing for it regardless of what is passed, since only Plan's dispatch prompt injects notes
/// and lessons the caller gathered ahead of composition.
fn plan_notes_block(ctx: &StageContext) -> String {
    match (ctx.stage, ctx.notes) {
        (rung::Stage::Plan, Some(notes)) => format!("\n\nNotes and lessons for this Run:\n{notes}"),
        _ => String::new(),
    }
}

/// The one prompt the script could not supply, because it has no CI-babysit path.
///
/// Reacting to a red check is the one situation where rebasing onto a moved base and
/// force-pushing an amended fix are the *idiomatic* repairs — so an unwarned agent spends its
/// single bounded invocation colliding with a barrier that will refuse it anyway. The
/// operations are named here for that reason, not because naming them is what stops them.
pub const CI_BABYSIT_PROMPT: &str = "The pipeline finished and the PR is open, but a check on it \
came back red. You have exactly one invocation to react to that and nothing else.

Read the failing checks on the PR for this branch, find the cause, fix it on this branch and
push. Do not redo finished work, do not open a second PR, and do not touch anything the failing
checks did not point at.

Never weaken, trim or skip a step of `just verify` to make a check go green — a gutted gate is
worse than one that fails honestly. If the check cannot be made green, say so plainly in the PR
body and leave the step intact.

Do not merge the PR, force-push, rebase, hard-reset or delete the branch. These are refused at
the tool layer and attempting them spends this invocation for nothing.";

/// The one bounded invocation a decided-and-failing CI buys, continuing whichever stage session
/// the caller names via `conditions.session_id` (Ship's, per `run_ship_babysit_attempt`).
pub fn ci_babysit(conditions: &Conditions) -> Invocation {
    let mut argv = vec![
        conditions.claude_bin.to_string(),
        "-p".to_string(),
        "--output-format".to_string(),
        "json".to_string(),
        "--permission-mode".to_string(),
        "bypassPermissions".to_string(),
    ];
    if let Some(model) = conditions.model {
        argv.push("--model".to_string());
        argv.push(model.to_string());
    }
    argv.push("--resume".to_string());
    argv.push(conditions.session_id.to_string());
    // No `--plugin-dir`: a stage invocation names no plugin (ADR-0015 retired the pin once
    // nothing was left to invoke it). No `--max-budget-usd`: ADR-0010, spend is recorded,
    // never bounded.
    argv.push("--disallowedTools".to_string());
    argv.extend(DENIED_TOOLS.iter().map(|glob| glob.to_string()));
    Invocation::build(argv, CI_BABYSIT_PROMPT.to_string(), Mode::CiBabysit)
}

/// What a child left behind, **after** it landed on disk.
///
/// Private fields, and [`run`] is the only constructor. The invariant does not rest on that
/// alone: `world` redirects both streams to their files *before* the child is spawned and hands
/// back only an exit code, so the parent cannot see a byte of the child's output without
/// reading the file it already wrote. *Parse before write* is not a thing to remember.
pub struct RawAttempt {
    stdout: String,
    /// Read back from disk alongside stdout, for the same reason: a child the rate limit
    /// killed before it emitted any JSON leaves its verdict on stderr, and classifying only
    /// stdout would record that death as an ordinary one.
    stderr: String,
    code: Option<i32>,
}

impl RawAttempt {
    pub fn classify(&self, n: usize, mode: Mode, started_at: &str, ended_at: &str) -> Attempt {
        classify(
            &self.stdout,
            &self.stderr,
            self.code,
            n,
            mode,
            started_at,
            ended_at,
        )
    }
}

/// Spawn the child, having already committed its output to disk. The prompt is written first
/// for the same reason: every death is diagnosable from Run state alone, without opening a
/// transcript.
pub fn run(
    invocation: &Invocation,
    cwd: &Path,
    prompt_path: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<RawAttempt, Reason> {
    world::write(prompt_path, invocation.prompt())
        .map_err(|e| Reason::saying(&format!("could not write the prompt: {e}")))?;
    let code = world::spawn_recorded(
        invocation.argv(),
        cwd,
        invocation.prompt(),
        stdout_path,
        stderr_path,
    )
    .map_err(|e| Reason::saying(&format!("could not spawn `claude`: {e}")))?;
    // Read back what is already on disk, rather than what the parent buffered. Both streams:
    // the classifier needs stderr to see a limit that killed the child before any JSON.
    let stdout = world::read_to_string(stdout_path).unwrap_or_default();
    let stderr = world::read_to_string(stderr_path).unwrap_or_default();
    Ok(RawAttempt {
        stdout,
        stderr,
        code,
    })
}

/// The pure classifier over a raw triple.
///
/// **`subtype` is not the outcome.** It read `success` on all five of Run 1's attempts including
/// the three that died, and on all six of Run 2's rate-limited ones. `terminal_reason` and the
/// API error status are the discriminators.
///
/// **The payload is recovered tolerantly, and the raw streams speak when it cannot.** Strict
/// whole-string parsing flips `parse_ok` false over a single stray byte around the payload,
/// and then a rate limit delivered amid noise classifies as a crash — an immediate Reenter
/// burning attempts against an hours-long wall. So [`parse_payload`] falls back before giving
/// up, and when the payload never rendered a verdict (`parse_ok` false) or the child exited
/// non-zero without the payload itself carrying the 429, the same normalised needle set folds
/// over the stdout tail and the stderr — where a limit that killed the child before any JSON
/// leaves its verdict. A false positive sleeps instead of burning attempts: the safe
/// direction, the same one that makes an unparseable Attempt never a Wait.
pub fn classify(
    stdout: &str,
    stderr: &str,
    code: Option<i32>,
    n: usize,
    mode: Mode,
    started_at: &str,
    ended_at: &str,
) -> Attempt {
    let parsed = parse_payload(stdout);
    let parse_ok = parsed.is_some();
    let value = parsed.unwrap_or(serde_json::Value::Null);

    // Absent is not the same fact as present-and-empty: `result.get` distinguishes a key that
    // never arrived (a renamed or dropped field in an otherwise well-formed payload) from one
    // that arrived null or empty. Folding the two together with `.unwrap_or_default()` is how
    // a finished Run's DONE promise, sitting under a renamed key, read as `done_promise: false`
    // indistinguishable from a session that truly said nothing.
    let result_present = value.get("result").is_some();
    let result = text_at(&value, "result").unwrap_or_default();
    let is_error = value
        .get("is_error")
        .and_then(|v| v.as_bool())
        .unwrap_or(!parse_ok);

    // The stdout half of the rate-limit haystack, computed once: the payload's `result` when
    // one arrived, the raw stdout otherwise — the same slice the record keeps as its tail,
    // so no second pass over the stream is needed.
    let stream_tail = tail(
        if parse_ok && result_present {
            &result
        } else {
            stdout
        },
        1500,
    );

    Attempt {
        n,
        mode,
        started_at: started_at.to_string(),
        ended_at: ended_at.to_string(),
        exit_code: code,
        is_error,
        parse_ok,
        // `subtype` already carries a synthetic value for one kind of drift
        // (`unparseable-output`, when the whole payload did not parse); a missing `result` key
        // gets its own synthetic value here rather than folding into that one. A single
        // `Attempt` has no room for a new field without breaking the record's shape everywhere
        // it is built by hand (ADR-0006's point about widening a record's vocabulary), and
        // collapsing both drifts into one string would make a payload that parsed cleanly but
        // renamed a field indistinguishable, in the operator-facing announce line, from a
        // payload that never parsed at all — its own loss of information.
        subtype: if !parse_ok {
            Some("unparseable-output".to_string())
        } else if !result_present {
            Some("result-field-missing".to_string())
        } else {
            text_at(&value, "subtype")
        },
        stop_reason: text_at(&value, "stop_reason"),
        api_error_status: text_at(&value, "api_error_status"),
        terminal_reason: text_at(&value, "terminal_reason"),
        num_turns: value.get("num_turns").and_then(|v| v.as_u64()),
        total_cost_usd: value.get("total_cost_usd").and_then(|v| v.as_f64()),
        usage: value.get("usage").cloned(),
        permission_denials: value
            .get("permission_denials")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default(),
        done_promise: result.contains("<promise>DONE</promise>"),
        // The payload's own verdict first. The raw-streams fold runs only when the payload
        // cannot speak — it never parsed, or the child exited non-zero without the payload
        // itself carrying the 429 — so a healthy attempt is never rate-limited by noise on
        // its stderr. `stream_tail` is the stdout half of that fold, the same tail the
        // record keeps for diagnosis.
        rate_limited: is_rate_limited(&value)
            || ((!parse_ok || code.is_some_and(|c| c != 0))
                && mentions_limit(&normalise(&format!("{stream_tail} {stderr}")))),
        // The tail is kept whether or not the response parsed, so an unreadable child still
        // leaves something diagnosable. A missing `result` key takes the same fallback as an
        // unparseable payload, for the same reason: there is nothing under that key to show
        // either way, and the raw stdout is the only thing left to look at.
        result_tail: stream_tail,
        // Could not observe until somebody reads the transcript, which is `supervisor`'s job
        // before it pushes the Attempt. A path that forgets records *could not observe* rather
        // than `(0, 0)`, which is the honest direction for an omission.
        fanout: Observed::Unobservable(Reason::saying("the transcript was not read")),
    }
}

/// The payload, recovered tolerantly.
///
/// Strict whole-string parsing first — the shape a healthy child emits. Around it, the
/// recorded failure mode is **noise**: a wrapper's banner, a warning line, a stray byte, and
/// then a rate limit delivered amid that noise would classify as a crash and burn attempts
/// against an hours-long wall. So before declaring the stream unparseable: retry on the
/// widest `{`..`}` span (the payload with junk around it), then take the last line that
/// parses as a JSON object (a payload after other output). Only a stream with no recoverable
/// object at all returns `None`, which `classify` records as `unparseable-output` with the
/// raw tail kept.
fn parse_payload(stdout: &str) -> Option<serde_json::Value> {
    if let Ok(value) = serde_json::from_str(stdout) {
        return Some(value);
    }
    if let (Some(start), Some(end)) = (stdout.find('{'), stdout.rfind('}'))
        && start < end
        && let Ok(value) = serde_json::from_str(&stdout[start..=end])
    {
        return Some(value);
    }
    stdout.lines().rev().find_map(|line| {
        serde_json::from_str(line.trim())
            .ok()
            .filter(serde_json::Value::is_object)
    })
}

fn tail(text: &str, characters: usize) -> String {
    let count = text.chars().count();
    text.chars()
        .skip(count.saturating_sub(characters))
        .collect()
}

// --- the live view, read from an undocumented format -----------------------------------------

/// One fanned-out subagent, as the transcript shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Fanout {
    pub description: String,
}

/// What the transcript can say. Five values, each degrading on its own — an unreadable
/// transcript costs these their values and never the whole command.
#[derive(Debug)]
pub struct Live {
    pub transcript: PathBuf,
    pub now_skill: Observed<String>,
    pub last_words: Vec<String>,
    /// The last assistant message, flattened to one line: the live answer to *what is it
    /// doing right now*, observed from the transcript. Never a verdict input — ADR-0003
    /// caps this field at describing what happened (issue #82).
    pub assistant_now: Observed<String>,
    pub fanout: Observed<Vec<Fanout>>,
    /// Seconds since the newest write across the parent transcript **and every subagent
    /// transcript**. The quietest healthy phase of a pipeline must not read as stuck.
    pub freshness: Observed<u64>,
}

/// Claude Code writes a session's transcript under a slug of the directory it ran in.
///
/// The record's worktree is the **declared** clone — on this host a symlink under
/// `~/.grind/repos/<owner>/<name>` — and the OS resolves a cwd through it, so Claude slugs the
/// **resolved** path (`/private/var/...` where the record says `/var/...`) and the pointer
/// named a file that was not there (#82). Resolving at read time matches what Claude records,
/// and heals records written before the fix with no migration: the slug is recomputed from the
/// same directory every read. A worktree that is gone cannot be canonicalised, and slugging
/// the raw string is then the only answer there is — the old behaviour, kept for that case.
pub fn transcript_path(home: &Path, worktree: &str, session_id: &str) -> PathBuf {
    let resolved = world::resolve_link(Path::new(worktree))
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| worktree.to_string());
    home.join(".claude")
        .join("projects")
        .join(project_slug(&resolved))
        .join(format!("{session_id}.jsonl"))
}

fn project_slug(worktree: &str) -> String {
    worktree
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

pub fn live(transcript: &Path, now_epoch: u64) -> Live {
    let text = world::read_to_string(transcript).ok();
    let newest = newest_write(transcript);
    Live {
        transcript: transcript.to_path_buf(),
        now_skill: match &text {
            Some(body) => now_skill(body),
            None => Observed::Unobservable(Reason::saying("the transcript could not be read")),
        },
        assistant_now: match &text {
            Some(body) => assistant_now(body),
            None => Observed::Unobservable(Reason::saying("the transcript could not be read")),
        },
        last_words: match &text {
            Some(body) => last_words(body, 3),
            // Still exactly three lines: the block's height is fixed so `watch` never jitters,
            // and an unreadable transcript must not change the shape of the view.
            None => vec![String::new(); 3],
        },
        fanout: match &text {
            Some(body) => fanout(body),
            None => Observed::Unobservable(Reason::saying("the transcript could not be read")),
        },
        freshness: match newest {
            Some(at) => Observed::Present(seconds_since(at, now_epoch)),
            None => {
                Observed::Unobservable(Reason::saying("no transcript write to read a time from"))
            }
        },
    }
}

/// The newest write across the parent transcript and `<uuid>/subagents/*.jsonl`.
///
/// A fan-out makes the **parent** go quiet while subagents work, so a parent-only mtime
/// misreads a healthy fan-out as a stall and sends the operator to kill a working Run.
pub fn newest_write(transcript: &Path) -> Option<SystemTime> {
    let mut newest = world::mtime(transcript);
    let Some(stem) = transcript.file_stem() else {
        return newest;
    };
    let subagents = transcript
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(stem)
        .join("subagents");
    for child in world::list_with_extension(&subagents, "jsonl") {
        if let Some(at) = world::mtime(&child) {
            newest = Some(match newest {
                Some(current) if current > at => current,
                _ => at,
            });
        }
    }
    newest
}

fn seconds_since(at: SystemTime, now_epoch: u64) -> u64 {
    let then = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now_epoch.saturating_sub(then)
}

/// Tolerant `serde_json::Value` lookups, line by line.
///
/// A derive against an undocumented format has to track it forever, and optional-with-default
/// still loses **every sibling field on a line** when one field's type is unexpected. The same
/// real file changes field names and field types between its own lines, so an unreadable line
/// costs its own values and nothing else.
pub fn now_skill(text: &str) -> Observed<String> {
    let mut last: Option<String> = None;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(skill) = value.get("attributionSkill").and_then(|s| s.as_str())
            && !skill.is_empty()
        {
            last = Some(skill.to_string());
        }
    }
    match last {
        Some(skill) => Observed::Present(skill),
        // The same *nothing recognised* rule the fan-out matcher carries. This field is not
        // currently broken; the rule is what keeps it from breaking silently the way the
        // fan-out one did.
        None => nothing_recognised(text, "attributionSkill"),
    }
}

/// The last assistant message, flattened to one line: the live answer to *what is it doing
/// right now* while the Run is still going (#82). It reads only assistant lines — unlike
/// `last_words`, which takes every message — because the operator asking *what is it doing*
/// means what Claude said, not what was said to it. Both role spellings are recognised, the
/// top-level `type` and `message.role`, because the real file carries the role inconsistently
/// between its own lines and a matcher over one spelling is the next silent-stale one. Like
/// everything in this view it describes what happened and is never a verdict input (ADR-0003).
pub fn assistant_now(text: &str) -> Observed<String> {
    let mut last: Option<String> = None;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let is_assistant = value.get("type").and_then(|t| t.as_str()) == Some("assistant")
            || value
                .get("message")
                .and_then(|m| m.get("role"))
                .and_then(|r| r.as_str())
                == Some("assistant");
        if is_assistant && let Some(said) = first_text(&value) {
            last = Some(one_line(&said));
        }
    }
    match last {
        Some(said) => Observed::Present(said),
        None => nothing_recognised(text, "assistant message"),
    }
}

/// The tool a fan-out spawn names. The CLI calls it `Agent`; `Task` is the former spelling, and
/// matching only that one printed `none` on every Run that fanned out — **203 spawns to 0**
/// across sixty transcripts. The fixture that should have caught it is authored, so it asserted
/// the matcher against itself and caught nothing.
pub const FANOUT_TOOLS: [&str; 2] = ["Agent", "Task"];

/// Every tool-use block in a transcript, whatever it named.
///
/// This is what separates *nothing recognised* from *nothing there*. A transcript full of tool
/// calls and no recognised spawn is a matcher that has gone stale, and reading it as `Absent`
/// is indistinguishable from a Run that genuinely fanned out to nobody.
pub fn tool_calls(text: &str) -> usize {
    let mut calls = 0usize;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(serde_json::Value::Array(parts)) =
            value.get("message").and_then(|m| m.get("content"))
        else {
            continue;
        };
        calls += parts
            .iter()
            .filter(|part| part.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
            .count();
    }
    calls
}

/// *Could not observe*, with the tool-call count in the reason — or `Absent` where there was
/// nothing in the transcript to recognise in the first place.
fn nothing_recognised<T>(text: &str, what: &str) -> Observed<T> {
    let calls = tool_calls(text);
    if calls == 0 {
        return Observed::Absent;
    }
    Observed::Unobservable(Reason::saying(&format!(
        "{calls} tool call{} in the transcript and no recognised `{what}`",
        if calls == 1 { "" } else { "s" }
    )))
}

/// The last-words block, fixed at exactly `wanted` lines so `watch -n 30` never jitters.
pub fn last_words(text: &str, wanted: usize) -> Vec<String> {
    let mut said: Vec<String> = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(text) = first_text(&value) {
            said.push(one_line(&text));
        }
    }
    let start = said.len().saturating_sub(wanted);
    let mut block: Vec<String> = said[start..].to_vec();
    while block.len() < wanted {
        block.push(String::new());
    }
    block
}

fn first_text(value: &serde_json::Value) -> Option<String> {
    let content = value.get("message")?.get("content")?;
    match content {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(parts) => parts
            .iter()
            .find_map(|p| p.get("text").and_then(|t| t.as_str()))
            .map(str::to_string),
        _ => None,
    }
}

/// Fan-out as a count with descriptions — *blocked on five agents, newest wrote forty seconds
/// ago* has to be an available answer.
///
/// Both spellings are recognised (`FANOUT_TOOLS`), and a transcript carrying tool-use blocks
/// with **zero** recognised spawns reads *could not observe* rather than `Absent`. Widening the
/// matcher alone would leave the next rename exactly as silent as this one was.
///
/// **A spawn paired to a `tool_result` anywhere in the same text is not running.** This is the
/// same `tool_use` → `tool_result` pairing [`fanout_counts`] assumes, and the same transcript:
/// the live view reads the whole append-only file, so listing every spawn listed finished work
/// as currently-running forever — attempt 1's three agents all returned, and `grind status`
/// kept saying *3 agents*. A spawn carrying no id cannot be paired, so it stays listed, the
/// same assumed-not-returned direction `fanout_counts` reads. When every spawn has paired
/// there is nothing running to observe, which is `Absent` — never `Present(vec![])`, an empty
/// list whose only consumer rendered as a word and whose non-empty case is the whole point.
///
/// A bare top-level `description` field is **not** a spawn. That field belongs to subagent
/// side-chain lines (`isSidechain: true` in `<session>/subagents/*.jsonl`), files this view
/// reads only for freshness ([`newest_write`]) — the parent transcript this function reads
/// never carries one, and matching it counted unrelated lines as spawns that could never pair
/// away.
pub fn fanout(text: &str) -> Observed<Vec<Fanout>> {
    let mut spawned: Vec<(Option<String>, Fanout)> = Vec::new();
    let mut returned: Vec<String> = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(serde_json::Value::Array(parts)) =
            value.get("message").and_then(|m| m.get("content"))
        else {
            continue;
        };
        for part in parts {
            let named = part
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or_default();
            if FANOUT_TOOLS.contains(&named) {
                spawned.push((
                    part.get("id").and_then(|i| i.as_str()).map(str::to_string),
                    Fanout {
                        description: part
                            .get("input")
                            .and_then(|i| i.get("description"))
                            .and_then(|d| d.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    },
                ));
            } else if part.get("type").and_then(|t| t.as_str()) == Some("tool_result")
                && let Some(paired) = part.get("tool_use_id").and_then(|i| i.as_str())
            {
                returned.push(paired.to_string());
            }
        }
    }
    if spawned.is_empty() {
        return nothing_recognised(text, "fan-out spawn");
    }
    let running: Vec<Fanout> = spawned
        .iter()
        .filter(|(id, _)| match id {
            Some(id) => !returned.iter().any(|seen| seen == id),
            None => true,
        })
        .map(|(_, fanout)| fanout.clone())
        .collect();
    if running.is_empty() {
        return Observed::Absent;
    }
    Observed::Present(running)
}

/// The fan-out in **the lines appended since** `already_written`, which is what *per Attempt*
/// means here (R51).
///
/// A Run's transcript is one append-only file for the Run's whole life: the session id is fixed
/// at dispatch and every later Attempt resumes that session. So counting the whole file on
/// Attempt N counts Attempts 1..N, and since `render` sums the per-Attempt pairs, a Run fanning
/// out to 2 agents on each of 3 attempts reported 12 spawned. The suffix is the fix, and it is a
/// suffix by line because the transcript is line-delimited JSON.
pub fn fanout_since(text: &str, already_written: usize) -> Observed<(u64, u64)> {
    fanout_counts(
        &text
            .lines()
            .skip(already_written)
            .collect::<Vec<&str>>()
            .join("\n"),
    )
}

/// **Spawned and returned, both read from the parent transcript** (KTD8). Spawns are the
/// tool-use blocks naming the fan-out tool; returns are the `tool_result` blocks that pair to
/// them by id. The subagent files on disk are the third source and are deliberately unused:
/// they have zero observed disagreements with these counts, so they add reading and no
/// information.
///
/// **No summary, boolean or health word sits over the two integers.** A count of processes must
/// never become an assertion about a review, and whether a returned subagent errored is
/// unproven across 203 observations and is not modelled.
///
/// The `tool_use` → `tool_result` pairing is **assumed**, not verified. Where a spawn carries no
/// id it cannot be paired, so it counts as spawned and never as returned — which reads low
/// rather than high, the safe direction for a number nobody should fold into a verdict.
pub fn fanout_counts(text: &str) -> Observed<(u64, u64)> {
    let mut spawned: Vec<String> = Vec::new();
    let mut unidentified = 0u64;
    let mut returned = 0u64;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(serde_json::Value::Array(parts)) =
            value.get("message").and_then(|m| m.get("content"))
        else {
            continue;
        };
        for part in parts {
            let named = part
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or_default();
            if FANOUT_TOOLS.contains(&named) {
                match part.get("id").and_then(|i| i.as_str()) {
                    Some(id) => spawned.push(id.to_string()),
                    None => unidentified += 1,
                }
                continue;
            }
            if part.get("type").and_then(|t| t.as_str()) == Some("tool_result")
                && let Some(paired) = part.get("tool_use_id").and_then(|i| i.as_str())
                && spawned.iter().any(|id| id == paired)
            {
                returned += 1;
            }
        }
    }
    let total = spawned.len() as u64 + unidentified;
    if total == 0 {
        return nothing_recognised(text, "fan-out spawn");
    }
    Observed::Present((total, returned))
}
