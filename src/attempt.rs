//! One `claude` invocation: the argv the denials ride on, and the raw triple that hits disk
//! before anything reads it.
//!
//! The rule's one asterisk — a pure builder and a pure classifier around two `world` calls,
//! neither cleanly pure nor cleanly I/O (ADR-0007).

use crate::job::Job;
use crate::observe::{Observed, Reason};
use crate::rung;
use crate::world;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A Run must never merge its own PR, discard the human's history, or rewrite a pushed branch.
/// Denials are inherited by subagents and are **not** overridden by `bypassPermissions`
/// (verified 2026-08-02), so this is a dependable constraint rather than a request.
///
/// **Nothing sits behind it.** No credential at any tier can withhold merge from something
/// allowed to open a PR — `Pull requests: write` covers both, `Contents: write` covers push and
/// branch deletion, and force-push is indistinguishable from push at every credential layer. So
/// these globs are the entire barrier, not the outer one.
///
/// Weakening the list is **intent**, and no carrier defends against intent. What is typeable is
/// the narrower, omission-shaped property below: every invocation carries them. The contents
/// stay prose, in `CLAUDE.md`, where they already are.
pub const DENIED_TOOLS: [&str; 26] = [
    "Bash(gh pr merge*)",
    "Bash(git push --force*)",
    "Bash(git push -f*)",
    "Bash(git reset --hard*)",
    "Bash(git rebase*)",
    "Bash(git checkout main*)",
    "Bash(git branch -D*)",
    // Deletes a branch through `push`, the sibling of `git branch -D` above.
    "Bash(git push --delete*)",
    // The `+refspec` force. Also refuses a push to a branch with a literal `+` in its name —
    // an acceptable false refusal for a barrier of this kind.
    "Bash(git push*+*)",
    // Every `git -C` invocation, not just the dangerous ones. A Run works inside its own
    // worktree via cwd, so `git -C` pointing anywhere is outside the shape it should have —
    // enumerating `-C` × each forbidden verb is the whack-a-mole this glob avoids.
    "Bash(git -C*)",
    // The sibling of `git checkout main` above.
    "Bash(git switch main*)",
    // Merge through the API rather than `gh pr merge`.
    "Bash(gh api*merge*)",
    // --- the same operations with the flag off the front ---------------------------------
    //
    // Every glob above anchors its flag immediately after the verb, and **git accepts the flag
    // anywhere**: `git push origin --force`, `git push -u origin main --force`,
    // `git reset HEAD~3 --hard` and `git branch --delete --force feat/x` are the forms people
    // and agents most often type, and all four were allowed. These eleven are the same
    // operations, position-independent.
    "Bash(git push*--force*)",
    "Bash(git push*--delete*)",
    // `git push origin :feat/x` deletes a branch through the refspec. Also refuses a push
    // naming an explicit `user@host:path` remote, which is not the shape a Run pushes in —
    // it pushes to the `origin` its worktree already has.
    "Bash(git push*:*)",
    // `-f` as its own argument, in the two positions it can take. Deliberately **not**
    // `git push*-f*`: a branch named `fix/PROJ-1 -form` is unlikely, but `-f` as a bare
    // substring appears inside ordinary branch names and that glob would refuse the push.
    "Bash(git push* -f)",
    "Bash(git push* -f *)",
    "Bash(git reset*--hard*)",
    "Bash(git branch* -D*)",
    "Bash(git branch*--delete*)",
    // Force-push under its safest-sounding spelling. It is still a force-push, and
    // `--force-with-lease` reads like a concession a stuck Run would reach for.
    "Bash(git*--force-with-lease*)",
    // The lowercase sibling of `Bash(git -C*)`. `git -c x rebase` moves the verb off the front
    // exactly as `-C` does, and glob matching is byte-exact, so the uppercase glob never saw it.
    "Bash(git -c*)",
    // Branch deletion and history rewriting one layer below the porcelain:
    // `git update-ref -d refs/heads/x` deletes, `git update-ref refs/heads/main <sha>` rewrites.
    "Bash(git*update-ref*)",
    // Mirror push: force-updates (and can delete) **every** ref on the remote, not one.
    "Bash(git push*--mirror*)",
    // Prune push: deletes every remote ref with no local counterpart — remote branch
    // deletion by side effect of an ordinary-looking push.
    "Bash(git push*--prune*)",
    // Branch deletion one door over: `gh api -X DELETE repos/o/r/git/refs/heads/x` removes a
    // remote branch through the API the `gh pr merge`/`gh api*merge*` globs leave open.
    "Bash(gh api*DELETE*)",
];

/// Write-capable Bash forms denied on the two fan-out panels (`Review`, `Validate`) and on
/// Reflect: none of the three ever touches a worktree, and denying `Write`/`Edit` alone does not
/// reach a shell command that mutates one the same way. `git push*` is denied outright — a panel
/// or Reflect never pushes.
const PANEL_BASH_FORMS: [&str; 10] = [
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

/// The denial set for one of the ten ladder stages. **The base list always, verbatim, never
/// filtered** — widening only, never narrowing. Report-only stages (`PlanReview`, `Review`,
/// `Validate`) additionally deny `Write` and `Edit`; the two fan-out panels (`Review`,
/// `Validate`) further deny the write-capable Bash forms above, since a panel session's sandbox
/// — not its prompt — is what makes "touches nothing" true. `Plan`, `Triage`, `Work`,
/// `Simplify`, `DiffTriage`, `Fixes` and `Ship` carry the base list only: they are the stages
/// that write the worktree (or, for the two in-process `[R]` passes, write nothing at all).
pub fn denied_for(stage: rung::Stage) -> Vec<String> {
    use rung::Stage::{PlanReview, Review, Validate};
    let mut denials: Vec<String> = DENIED_TOOLS.iter().map(|s| s.to_string()).collect();
    if matches!(stage, PlanReview | Review | Validate) {
        denials.push("Write".to_string());
        denials.push("Edit".to_string());
    }
    if matches!(stage, Review | Validate) {
        denials.extend(PANEL_BASH_FORMS.iter().map(|s| s.to_string()));
    }
    denials
}

/// Reflect is not a rung on the ladder — [`rung::Stage`] has no variant for it — and it is
/// report-only like the two panels, but it must still write its own artifacts under
/// `<stages-dir>/reflect/`, which a `Write`/`Edit` denial would block. Its worktree protection is
/// instead the write-capable Bash-form denials above (plus `git push*` outright), backed by
/// dispatching it with the *run* directory as cwd rather than the *worktree* (unit C's job, noted
/// here since this is where the sandbox story is decided) — so there is no repo tree under this
/// session for `Write`/`Edit` to touch in the first place.
pub fn denied_for_reflect() -> Vec<String> {
    let mut denials: Vec<String> = DENIED_TOOLS.iter().map(|s| s.to_string()).collect();
    denials.extend(PANEL_BASH_FORMS.iter().map(|s| s.to_string()));
    denials
}

/// `<run>-<stage>`, the session id a stage's own Attempt dispatches or resumes. Re-entry resumes
/// the dying *stage's* session, never the Run's old mega-session.
pub fn stage_session_id(run_id: &str, stage: rung::Stage) -> String {
    format!("{run_id}-{stage}")
}

/// What a stage invocation needs beyond a session id and a binary path: the stage itself, its
/// skill text (read by the caller through `world` and handed in as `&str`, so this stays pure),
/// where its returns and artifacts go, the worktree it runs in, the Job rows it is dispatched
/// against, and the model this Run's tier resolved for it. `notes` is Plan-only injected
/// notes/lessons text; every other stage leaves it `None` and it is never rendered.
#[derive(Debug, Clone, Copy)]
pub struct StageContext<'a> {
    pub stage: rung::Stage,
    pub skill_text: &'a str,
    pub stages_dir: &'a str,
    pub worktree: &'a str,
    pub job: &'a Job,
    pub model: Option<&'a str>,
    pub notes: Option<&'a str>,
}

/// The two facts a stage invocation needs that [`Conditions`] does not carry the right shape
/// for: a stage invocation names no plugin (ADR-0015 retired the pin once nothing was left to
/// invoke it), so this is not `Conditions` with a field ignored — it is the narrower thing a
/// stage argv actually is.
#[derive(Debug, Clone, Copy)]
pub struct StageConditions<'a> {
    pub claude_bin: &'a str,
    pub run_id: &'a str,
}

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
    let session_id = format!("{}-reflect", conditions.run_id);
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
    Invocation {
        argv,
        prompt: skill_text.to_string(),
        mode,
    }
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
    Invocation { argv, prompt, mode }
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

/// Which of the three shapes an invocation is. Recorded per attempt, so a spent CI budget is
/// visible as itself rather than as an ordinary re-entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Dispatch,
    Resume,
    CiBabysit,
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let word = match self {
            Mode::Dispatch => "dispatch",
            Mode::Resume => "resume",
            Mode::CiBabysit => "ci-babysit",
        };
        write!(f, "{word}")
    }
}

/// One clearance the human recorded on a Blocked Run: when, and what changed in the world.
///
/// It lives here beside [`Attempt`] because this module is already the shared vocabulary
/// between the record's private writer and its read-only reader — and because the re-entry
/// composition consumes the note here. A shared-noun module for it would pull the writer and
/// the readers back under one roof, which `tests/topology.rs` exists to refuse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clearance {
    pub cleared_at: String,
    pub note: String,
}

/// What the one surviving [`Conditions`] user — [`ci_babysit`] — is built from, all of it read
/// from the record rather than the environment so re-entering an in-flight Run never changes it
/// mid-stage.
#[derive(Debug, Clone, Copy)]
pub struct Conditions<'a> {
    pub claude_bin: &'a str,
    pub session_id: &'a str,
    pub model: Option<&'a str>,
}

/// A built invocation. **Private fields, and `build` is the only constructor** — so an argv
/// that does not carry the denials is not a value this program can hold. That is the
/// omission-shaped half of the property; the contents of the list are prose, because weakening
/// them is intent.
#[derive(Debug, Clone)]
pub struct Invocation {
    argv: Vec<String>,
    prompt: String,
    mode: Mode,
}

impl Invocation {
    pub fn argv(&self) -> &[String] {
        &self.argv
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }
}

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
    Invocation {
        argv,
        prompt: CI_BABYSIT_PROMPT.to_string(),
        mode: Mode::CiBabysit,
    }
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

/// One attempt as the record holds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attempt {
    pub n: usize,
    pub mode: Mode,
    pub started_at: String,
    pub ended_at: String,
    pub exit_code: Option<i32>,
    pub is_error: bool,
    /// Whether the child's stdout parsed at all. An unparseable response is a record that says
    /// so, not an aborted supervisor.
    pub parse_ok: bool,
    /// `Some("unparseable-output")` when `parse_ok` is false, `Some("result-field-missing")`
    /// when the payload parsed but its `result` key did not arrive, the payload's own subtype
    /// otherwise. Two distinct synthetic values rather than one, so a renamed field never reads
    /// the same as garbage that never parsed.
    pub subtype: Option<String>,
    pub stop_reason: Option<String>,
    pub api_error_status: Option<String>,
    pub terminal_reason: Option<String>,
    pub num_turns: Option<u64>,
    pub total_cost_usd: Option<f64>,
    pub usage: Option<serde_json::Value>,
    pub permission_denials: Vec<serde_json::Value>,
    pub done_promise: bool,
    pub rate_limited: bool,
    pub result_tail: String,
    /// **Spawned and returned, per Attempt.** A fan-out degrading on attempt 3 of a Run that
    /// finishes on attempt 8 leaves something durable.
    ///
    /// Three-valued, because two bare `Option<u64>` fields would collapse *no fan-out* and
    /// *could not read the transcript*. **No summary, boolean or health word sits over the two
    /// integers**: a count of processes must never become an assertion about a review
    /// (ADR-0006's sixth prohibited shape).
    pub fanout: Observed<(u64, u64)>,
}

impl Attempt {
    /// The fan-out arithmetic, gathered before the Attempt is pushed — `RunRecord.attempts` is
    /// append-only with no mutating accessor, so it cannot be filled in afterwards (KTD9).
    pub fn with_fanout(mut self, fanout: Observed<(u64, u64)>) -> Self {
        self.fanout = fanout;
        self
    }

    /// **A Wait is an Attempt that did no work**, and it is keyed on work done rather than on
    /// cause — this predicate never reads `rate_limited`. Six of Run 2's eight Attempts cost $0
    /// and ran one turn each, probing a wall, and spent the same budget as the three that built
    /// twelve commits.
    ///
    /// **Presence, not absence, of the two fields decides.** Run 2's real Waits carry explicit
    /// `total_cost_usd: 0.0` and `num_turns: 1`; a payload that parsed but whose cost/turn
    /// fields were renamed away (the recorded `result-field-missing` drift) must not read as
    /// *did no work* — that is the same failure mode the next clause guards, one level
    /// shallower. Absence spends the budget, which is the safe direction.
    ///
    /// **An Attempt whose stdout did not parse is never a Wait, and that clause is
    /// load-bearing.** A child that dies before emitting parseable JSON leaves both the cost and
    /// the turn count absent, so a predicate reading absence as *did no work* would make every
    /// crash loop free: no budget spent, no rate-limit match, immediate re-entry, forever, with
    /// `attempt N of M` reporting the Run as barely started. Absence of evidence is not evidence
    /// of no work, and `parse_ok` is the field that already separates the two.
    ///
    /// Derived, never persisted: the fields it reads are already on the record, so there is no
    /// migration and no reader mirror to keep in step.
    pub fn is_wait(&self) -> bool {
        self.parse_ok
            && self.total_cost_usd.is_some_and(|cost| cost <= 0.0)
            && self.num_turns.is_some_and(|turns| turns <= 1)
    }
}

/// How many of these Attempts did work. **The attempt budget counts these and no others**, on
/// every surface that prints *attempt N of M*.
pub fn working(attempts: &[Attempt]) -> usize {
    attempts.iter().filter(|a| !a.is_wait()).count()
}

/// The run of Waits at the end of the list — the only bound on a Run that never does work,
/// since Waits by design spend no attempt budget.
///
/// Counted from the **recorded** list rather than held loop-local, because the bound has to
/// survive a restart: `resume --all` re-enters rate-limited and died Runs at boot, so a
/// loop-local count would hand a permanently-walled Run a fresh allowance at every reboot and
/// never terminate.
pub fn trailing_waits(attempts: &[Attempt]) -> usize {
    attempts.iter().rev().take_while(|a| a.is_wait()).count()
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

/// A field as text, whether the child sent a string, a number or a boolean. `api_error_status`
/// arrives as a JSON **number** in Run 2's recorded triple and as a string elsewhere.
fn text_at(value: &serde_json::Value, key: &str) -> Option<String> {
    match value.get(key)? {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

fn tail(text: &str, characters: usize) -> String {
    let count = text.chars().count();
    text.chars()
        .skip(count.saturating_sub(characters))
        .collect()
}

/// Rate limits, from a **normalised haystack**.
///
/// Lowercasing and stripping non-alphanumerics is what makes `rate  limit` with two spaces
/// match; including the API error status field is what makes a bare `429` match with no
/// matching prose anywhere. Run 2's six limited attempts read *"You've hit your session limit ·
/// resets 5pm"*, which matched none of the script's phrases — only the status code classified
/// them, and had it missed, eight attempts would have burned in under a minute against a
/// three-hour wall.
///
/// No regex crate: normalising detects rate limits more broadly than a pattern does.
pub fn is_rate_limited(value: &serde_json::Value) -> bool {
    if !value
        .get("is_error")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return false;
    }
    let mut haystack = String::new();
    for key in ["result", "terminal_reason", "api_error_status", "subtype"] {
        if let Some(text) = text_at(value, key) {
            haystack.push_str(&text);
            haystack.push(' ');
        }
    }
    let normalised = normalise(&haystack);
    mentions_limit(&normalised)
}

/// The needle set over an already-normalised haystack — shared by the payload path above and
/// by `classify`'s fold over the raw streams, so both ask exactly one question.
fn mentions_limit(normalised: &str) -> bool {
    const NEEDLES: [&str; 8] = [
        "ratelimit",
        "usagelimit",
        "sessionlimit",
        "toomanyrequests",
        "quotaexceeded",
        "resetsat",
        "resetat",
        "429",
    ];
    NEEDLES.iter().any(|needle| normalised.contains(needle))
}

fn normalise(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN2_RATE_LIMITED: &str = include_str!("../tests/fixtures/run2/rate-limited.stdout.json");
    const RUN1_ATTEMPT_1: &str = include_str!("../tests/fixtures/run1/attempt-1.stdout.json");
    const RUN1_ATTEMPT_2: &str = include_str!("../tests/fixtures/run1/attempt-2.stdout.json");
    const RUN1_ATTEMPT_3: &str = include_str!("../tests/fixtures/run1/attempt-3.stdout.json");
    const TRUNCATED: &str = include_str!("../tests/fixtures/run1/degraded-truncated.stdout.json");
    const GARBAGE: &str = include_str!("../tests/fixtures/run1/degraded-garbage.stdout.json");
    const EMPTY: &str = include_str!("../tests/fixtures/run1/degraded-empty.stdout.json");
    const DEGRADED_RENAMED: &str =
        include_str!("../tests/fixtures/run1/degraded-renamed.stdout.json");

    fn job() -> Job {
        Job {
            issue: 28,
            url: "https://github.com/FlorianRiquelme/snapper/issues/28".to_string(),
            title: "Slice 1b".to_string(),
            labels: vec![],
            target_repo: "FlorianRiquelme/snapper".to_string(),
            branch: "feat/28-slice-1b".to_string(),
            handoff_sha: "9d1f4c7a".to_string(),
            anchor: "docs/plans/a.md".to_string(),
            intent: None,
            model: None,
            done_predicate: "just verify is green".to_string(),
            base_branch: "main".to_string(),
            verify_entrypoint: "just verify".to_string(),
            declared_hot_paths: vec![],
        }
    }

    fn conditions(model: Option<&str>) -> Conditions<'_> {
        Conditions {
            claude_bin: "/home/op/.grind/bin/claude",
            session_id: "d51b4c39-ce1d-449b-8366-04b9b1aa6573",
            model,
        }
    }

    fn denials_of(invocation: &Invocation) -> Vec<String> {
        let argv = invocation.argv();
        let at = argv
            .iter()
            .position(|a| a == "--disallowedTools")
            .expect("--disallowedTools");
        argv[at + 1..].to_vec()
    }

    #[test]
    fn ci_babysit_carries_every_denial_no_plugin_flag_and_resumes_its_own_session() {
        let conditions = conditions(Some("claude-opus-5"));
        let invocation = ci_babysit(&conditions);
        assert_eq!(
            denials_of(&invocation),
            DENIED_TOOLS.to_vec(),
            "ci-babysit must carry every denial"
        );
        assert!(invocation.argv().contains(&"--resume".to_string()));
        assert!(!invocation.argv().contains(&"--session-id".to_string()));
        let at = invocation
            .argv()
            .iter()
            .position(|a| a == "--resume")
            .unwrap();
        assert_eq!(invocation.argv()[at + 1], conditions.session_id);
        assert!(!invocation.argv().contains(&"--plugin-dir".to_string()));
        assert!(!invocation.argv().contains(&"--max-budget-usd".to_string()));
        assert!(
            invocation
                .argv()
                .windows(2)
                .any(|w| w[0] == "--model" && w[1] == "claude-opus-5")
        );
    }

    #[test]
    fn ci_babysit_with_no_model_carries_no_model_flag() {
        let invocation = ci_babysit(&conditions(None));
        assert!(!invocation.argv().contains(&"--model".to_string()));
    }

    /// A minimal reimplementation of the two facts CLAUDE.md's `DENIED_TOOLS` section states
    /// about Claude Code's own deny-glob matcher: `*` may appear anywhere in the pattern (start,
    /// middle or end), and matching is per-subcommand after splitting the full command line on
    /// `&&`, `||`, `;`, `|`, `|&`, `&` and newlines. **This tests our own understanding of that
    /// matcher, not the matcher itself** — the real one lives in Claude Code and cannot be
    /// imported here, so a bug in this reimplementation would pass silently. Keep it dumb:
    /// full-string match, `*` the only wildcard, no other glob syntax.
    fn glob_matches(pattern: &str, candidate: &str) -> bool {
        fn rec(p: &[u8], c: &[u8]) -> bool {
            match p.first() {
                None => c.is_empty(),
                Some(b'*') => rec(&p[1..], c) || (!c.is_empty() && rec(p, &c[1..])),
                Some(head) => c.first() == Some(head) && rec(&p[1..], &c[1..]),
            }
        }
        rec(pattern.as_bytes(), candidate.as_bytes())
    }

    fn subcommands_of(command: &str) -> Vec<String> {
        let mut pieces = vec![command.to_string()];
        for separator in ["|&", "&&", "||", ";", "|", "&", "\n"] {
            pieces = pieces
                .iter()
                .flat_map(|p| p.split(separator).map(str::to_string).collect::<Vec<_>>())
                .collect();
        }
        pieces.into_iter().map(|p| p.trim().to_string()).collect()
    }

    fn is_denied(command: &str) -> bool {
        subcommands_of(command).iter().any(|sub| {
            DENIED_TOOLS.iter().any(|glob| {
                let pattern = glob
                    .strip_prefix("Bash(")
                    .and_then(|g| g.strip_suffix(')'))
                    .expect("every DENIED_TOOLS glob is Bash(...)");
                glob_matches(pattern, sub)
            })
        })
    }

    #[test]
    fn each_forbidden_operation_has_a_glob_that_refuses_it_under_the_documented_matcher() {
        // Table of forbidden operation -> **every spelling that performs it**, flag-first and
        // flag-last. This is the coverage the membership test above cannot give, and one
        // spelling per row is what let the whole list read complete while
        // `git push origin --force` went straight through: git accepts the flag anywhere, and
        // a table that only ever types it in one position never asks.
        let table: [(&str, &[&str]); 18] = [
            (
                "merge via gh pr merge",
                &["gh pr merge 123 --squash", "gh pr merge --squash 123"],
            ),
            (
                "force push with --force",
                &[
                    "git push --force origin feat/x",
                    "git push origin --force",
                    "git push origin main --force",
                    "git push -u origin main --force",
                ],
            ),
            ("force push with -f", &["git push -f", "git push origin -f"]),
            (
                "force push with --force-with-lease",
                &[
                    "git push --force-with-lease origin feat/x",
                    "git push origin --force-with-lease",
                ],
            ),
            (
                "hard reset",
                &["git reset --hard HEAD~3", "git reset HEAD~3 --hard"],
            ),
            (
                "rebase",
                &["git rebase main", "git rebase --onto main feat/x"],
            ),
            (
                "checkout main",
                &["git checkout main", "git checkout main --force"],
            ),
            (
                "branch delete",
                &[
                    "git branch -D feat/x",
                    "git branch feat/x -D",
                    "git branch --delete --force feat/x",
                    "git branch --force --delete feat/x",
                ],
            ),
            (
                "branch delete via push",
                &[
                    "git push --delete origin feat/x",
                    "git push origin --delete feat/x",
                    "git push origin :feat/x",
                ],
            ),
            (
                "the +refspec force",
                &[
                    "git push origin +main",
                    "git push origin +refs/heads/main:refs/heads/main",
                ],
            ),
            (
                "the -C prefix moving the verb off the front",
                &[
                    "git -C /tmp/other push --force",
                    "git -C /tmp/other rebase main",
                ],
            ),
            (
                "the -c prefix moving the verb off the front",
                &["git -c core.pager=cat rebase main", "git -c x push --force"],
            ),
            (
                "switch onto main",
                &["git switch main", "git switch main --force"],
            ),
            (
                "merge through the api",
                &[
                    "gh api repos/o/r/pulls/12/merge -X PUT",
                    "gh api --method PUT repos/o/r/pulls/12/merge",
                ],
            ),
            (
                "branch delete and history rewrite through update-ref",
                &[
                    "git update-ref -d refs/heads/feat/x",
                    "git update-ref refs/heads/main abc123",
                ],
            ),
            (
                "mirror push force-updating every ref on the remote",
                &["git push --mirror origin", "git push origin --mirror"],
            ),
            (
                "prune push deleting remote refs with no local counterpart",
                &["git push --prune origin", "git push origin --prune"],
            ),
            (
                "branch delete through the api",
                &[
                    "gh api -X DELETE repos/o/r/git/refs/heads/feat/x",
                    "gh api --method DELETE repos/o/r/git/refs/heads/feat/x",
                ],
            ),
        ];
        for (name, spellings) in table {
            for candidate in spellings {
                assert!(is_denied(candidate), "{name}: {candidate:?} must be denied");
            }
        }
        // Denials are per-subcommand after splitting on shell operators, so a prefix like `cd`
        // ahead of the forbidden verb must not let it through.
        assert!(is_denied("cd /tmp && git push --force origin main"));
    }

    #[test]
    fn ordinary_operations_the_barrier_must_not_catch_are_not_denied() {
        for allowed in [
            "git push origin feat/x",
            "git push -u origin feat/x",
            // A branch name carrying `-f` or `-D` as a substring is ordinary, and the
            // position-independent globs are written to leave it alone.
            "git push -u origin fix/PROJ-1-form-fields",
            "git push -u origin feat/PROJ-2-Dashboard",
            "git status",
            "git branch -d feat/x",
            "gh pr view 12",
            "gh pr create --fill",
            "git checkout feat/x",
            "git fetch origin",
            "git log --oneline",
        ] {
            assert!(!is_denied(allowed), "{allowed:?} must not be denied");
        }
    }

    #[test]
    fn nothing_observes_the_narrative_or_the_closing_keyword() {
        // Asserted as an absence, over the two modules that could grow a reader for either.
        // `include_str!` rather than the filesystem: reading a file at run time from inside
        // `src/` is what `tests/topology.rs` forbids everywhere but `world`.
        const OBSERVE: &str = include_str!("observe.rs");
        const DECIDE: &str = include_str!("decide.rs");
        for (name, source) in [("observe", OBSERVE), ("decide", DECIDE)] {
            for reader in ["narrative", "Closes #", "closes #", "surprised"] {
                assert!(
                    !source.contains(reader),
                    "`{name}` must not read the Run's prose back; it names `{reader}`"
                );
            }
        }
    }

    #[test]
    fn a_rate_limit_survives_two_spaces_between_the_words() {
        let doubled = serde_json::json!({"is_error": true, "result": "hit a rate  limit"});
        assert!(is_rate_limited(&doubled));
    }

    #[test]
    fn a_bare_429_with_no_matching_prose_anywhere_is_a_rate_limit() {
        // Run 2's real triple. `api_error_status` is a JSON number here, and the prose matches
        // none of the script's phrases — only the status code classified these six attempts.
        let value: serde_json::Value = serde_json::from_str(RUN2_RATE_LIMITED).unwrap();
        assert_eq!(
            value.get("api_error_status").unwrap(),
            &serde_json::json!(429)
        );
        assert!(is_rate_limited(&value));

        let bare = serde_json::json!({"is_error": true, "api_error_status": 429, "result": "x"});
        assert!(is_rate_limited(&bare));
    }

    #[test]
    fn a_session_limit_that_never_says_rate_limit_is_a_rate_limit() {
        let session = serde_json::json!({
            "is_error": true,
            "result": "You've hit your session limit · resets 5pm (Europe/Berlin)",
        });
        assert!(is_rate_limited(&session));
    }

    #[test]
    fn a_successful_attempt_mentioning_a_limit_in_passing_is_not_rate_limited() {
        let mentions = serde_json::json!({"is_error": false, "result": "rate limit in passing"});
        assert!(!is_rate_limited(&mentions));
    }

    #[test]
    fn an_ordinary_crash_is_not_mistaken_for_a_rate_limit() {
        let crash = serde_json::json!({"is_error": true, "result": "TypeError: undefined"});
        assert!(!is_rate_limited(&crash));
        for raw in [RUN1_ATTEMPT_1, RUN1_ATTEMPT_2, RUN1_ATTEMPT_3] {
            let value: serde_json::Value = serde_json::from_str(raw).unwrap();
            assert!(
                !is_rate_limited(&value),
                "Run 1's dropped connections were crashes, not limits"
            );
        }
    }

    #[test]
    fn subtype_reads_success_on_the_attempts_that_died_so_it_is_not_the_outcome() {
        for (n, raw) in [
            (1, RUN1_ATTEMPT_1),
            (2, RUN1_ATTEMPT_2),
            (3, RUN1_ATTEMPT_3),
        ] {
            let found = classify(raw, "", Some(1), n, Mode::Resume, "start", "end");
            assert_eq!(found.subtype.as_deref(), Some("success"), "attempt {n}");
            assert!(found.is_error, "attempt {n} really did die");
            assert!(!found.done_promise, "attempt {n} promised nothing");
            assert_eq!(
                found.terminal_reason.as_deref(),
                Some("api_error"),
                "attempt {n}"
            );
        }
    }

    #[test]
    fn unparseable_stdout_becomes_a_record_that_says_so_and_keeps_the_tail() {
        // Expectations here are derived independently of `tail` rather than by calling it a
        // second time: `tail` is the production path's own helper, so re-invoking it would let
        // an off-by-one or wrong-end bug in `tail` reproduce identically on both sides of the
        // assertion and pass unnoticed.
        for raw in [TRUNCATED, GARBAGE, EMPTY] {
            let found = classify(raw, "", Some(1), 1, Mode::Dispatch, "start", "end");
            assert!(!found.parse_ok, "this fixture does not parse");
            assert_eq!(found.subtype.as_deref(), Some("unparseable-output"));
            assert!(found.is_error);
            assert!(
                found.result_tail.chars().count() <= 1500,
                "a tail is never longer than what it was asked to keep"
            );
            assert!(
                raw.ends_with(found.result_tail.as_str()),
                "a tail must be a suffix of what it came from"
            );
        }
        // The truncated fixture is 1998 characters, long enough to actually cross the
        // 1500-character boundary — the case that matters, where bytes arrived and then stopped.
        let truncated = classify(TRUNCATED, "", Some(1), 1, Mode::Dispatch, "start", "end");
        assert_eq!(truncated.result_tail.chars().count(), 1500);
        assert_ne!(
            truncated.result_tail, TRUNCATED,
            "the boundary was crossed, so the tail must actually have been cut"
        );
        // Both of these fixtures are under the boundary, so nothing was cut.
        for raw in [GARBAGE, EMPTY] {
            let found = classify(raw, "", Some(1), 1, Mode::Dispatch, "start", "end");
            assert_eq!(
                found.result_tail, raw,
                "under the 1500-character boundary, the tail is everything there was"
            );
        }
    }

    #[test]
    fn a_killed_childs_empty_stdout_is_recorded_rather_than_lost() {
        let killed = classify("", "", None, 4, Mode::Resume, "start", "end");
        assert!(!killed.parse_ok);
        assert_eq!(killed.exit_code, None);
        assert!(
            killed.result_tail.is_empty(),
            "zero bytes is itself a recorded fact"
        );
    }

    #[test]
    fn the_done_promise_is_read_from_the_result_and_nowhere_else() {
        let promised = serde_json::json!({
            "is_error": false,
            "subtype": "success",
            "result": "PR is open, stopping here. <promise>DONE</promise>",
        })
        .to_string();
        assert!(classify(&promised, "", Some(0), 5, Mode::Resume, "s", "e").done_promise);

        let unpromised = serde_json::json!({
            "is_error": false,
            "subtype": "success",
            "result": "Made progress but the pipeline has not reached an open PR yet.",
        })
        .to_string();
        assert!(!classify(&unpromised, "", Some(0), 4, Mode::Resume, "s", "e").done_promise);
    }

    #[test]
    fn a_result_key_renamed_to_output_is_recorded_as_missing_not_as_an_empty_promise() {
        // The fixture this wires in models a real drift: `claude` sent a well-formed payload
        // whose DONE text sits under `output` rather than `result`. Before this fix, that read
        // as `parse_ok: true, done_promise: false` — indistinguishable from a session that
        // genuinely said nothing.
        let found = classify(
            DEGRADED_RENAMED,
            "",
            Some(0),
            1,
            Mode::Dispatch,
            "start",
            "end",
        );
        assert!(found.parse_ok, "the payload itself is well-formed JSON");
        assert_eq!(
            found.subtype.as_deref(),
            Some("result-field-missing"),
            "distinct from `unparseable-output`: this payload parsed fine"
        );
        assert!(
            !found.done_promise,
            "nothing can be read from a key that never arrived"
        );
        assert!(
            found.result_tail.contains("<promise>DONE</promise>"),
            "the raw stdout still carries the promise text under its new name, so the fallback \
             to it keeps the diagnostic alive"
        );
    }

    #[test]
    fn a_recorded_denial_survives_onto_the_attempt() {
        let denied = serde_json::json!({
            "is_error": false,
            "result": "the push was refused",
            "permission_denials": [{"tool_name": "Bash", "tool_input": {"command": "git push --force"}}],
        })
        .to_string();
        let found = classify(&denied, "", Some(0), 1, Mode::CiBabysit, "s", "e");
        assert_eq!(found.permission_denials.len(), 1);
        assert_eq!(found.mode, Mode::CiBabysit);
    }

    #[test]
    fn the_ci_babysit_prompt_names_what_the_globs_will_refuse_anyway() {
        // Reacting to a red check is the one situation where the forbidden repairs are the
        // idiomatic ones, so an unwarned agent spends its single invocation on the barrier.
        for named in [
            "merge",
            "force-push",
            "rebase",
            "hard-reset",
            "delete the branch",
        ] {
            assert!(
                CI_BABYSIT_PROMPT.contains(named),
                "the prompt must name {named}"
            );
        }
        assert!(CI_BABYSIT_PROMPT.contains("one invocation"));
        assert!(CI_BABYSIT_PROMPT.contains("do not open a second PR"));
        assert!(CI_BABYSIT_PROMPT.contains("Never weaken, trim or skip a step of `just verify`"));
    }

    // --- the clearance note rides Resume and nothing else --------------------------------------

    fn a_clearance(note: &str) -> Clearance {
        Clearance {
            cleared_at: "2026-08-21T19:00:00+00:00".to_string(),
            note: note.to_string(),
        }
    }

    // --- the Wait predicate -------------------------------------------------------------------

    fn shaped(parse_ok: bool, cost: Option<f64>, turns: Option<u64>, limited: bool) -> Attempt {
        Attempt {
            n: 1,
            mode: Mode::Resume,
            started_at: "s".to_string(),
            ended_at: "e".to_string(),
            exit_code: Some(1),
            is_error: true,
            parse_ok,
            subtype: None,
            stop_reason: None,
            api_error_status: None,
            terminal_reason: None,
            num_turns: turns,
            total_cost_usd: cost,
            usage: None,
            permission_denials: vec![],
            done_promise: false,
            rate_limited: limited,
            result_tail: String::new(),
            fanout: Observed::Absent,
        }
    }

    #[test]
    fn an_attempt_with_real_cost_and_many_turns_is_never_a_wait() {
        // Keyed on work done, never on cause: the flag says rate-limited and the Attempt still
        // did work, so it still spends the budget.
        assert!(!shaped(true, Some(37.04), Some(187), true).is_wait());
        assert!(!shaped(true, Some(37.04), Some(187), false).is_wait());
    }

    #[test]
    fn an_attempt_with_explicit_zero_cost_and_one_turn_is_a_wait() {
        // Run 2's real Waits carry the fields explicitly: 0.0 and 1.
        assert!(shaped(true, Some(0.0), Some(1), true).is_wait());
        assert!(
            shaped(true, Some(0.0), Some(1), false).is_wait(),
            "a Wait is never keyed on the rate-limit flag"
        );
    }

    #[test]
    fn an_attempt_that_parsed_with_both_fields_absent_is_not_a_wait() {
        // The bug this defends: absence read as zero, so a payload whose cost/turn fields were
        // renamed away (the recorded `result-field-missing` drift) made every Attempt —
        // including a $37/187-turn one — a Wait, and the budget never spent. Absence spends
        // the budget; only explicit zero-and-one waits.
        assert!(!shaped(true, None, None, false).is_wait());
        assert!(!shaped(true, None, None, true).is_wait());
        assert!(!shaped(true, Some(37.04), None, false).is_wait());
        assert!(!shaped(true, None, Some(187), false).is_wait());
    }

    #[test]
    fn an_unparseable_attempt_is_never_a_wait_even_with_both_fields_absent() {
        // The load-bearing clause. A child that dies before emitting parseable JSON leaves both
        // fields absent, and reading that as *did no work* makes every crash loop free.
        assert!(!shaped(false, None, None, false).is_wait());
        assert!(!shaped(false, None, None, true).is_wait());
    }

    #[test]
    fn the_wait_arithmetic_reads_the_attempt_list_and_nothing_else() {
        let list = [
            shaped(true, Some(3.0), Some(40), false),
            shaped(true, Some(0.0), Some(1), true),
            shaped(false, None, None, false),
            shaped(true, None, None, true),
            shaped(true, Some(0.0), Some(0), true),
        ];
        assert_eq!(working(&list), 3);
        assert_eq!(trailing_waits(&list), 1);
        assert_eq!(trailing_waits(&[]), 0);
        assert_eq!(working(&[]), 0);
    }

    #[test]
    fn a_payload_amid_stdout_noise_still_classifies() {
        // Strict whole-string parsing would flip `parse_ok` false over the banner bytes and
        // turn a rate limit into a crash — an immediate Reenter against an hours-long wall.
        let payload = serde_json::json!({
            "is_error": true,
            "result": "You've hit your session limit · resets 5pm (Europe/Berlin)",
            "num_turns": 3,
            "total_cost_usd": 0.42,
        });
        let noisy = format!("WARNING: plugin cache stale\n{payload}\nretrying once\n");
        let found = classify(&noisy, "", Some(1), 2, Mode::Resume, "s", "e");
        assert!(found.parse_ok, "the payload amid noise is still a payload");
        assert!(found.rate_limited, "the limit must survive noise around it");
        assert_eq!(found.num_turns, Some(3));
        assert_eq!(found.total_cost_usd, Some(0.42));
    }

    #[test]
    fn noise_without_a_recoverable_payload_is_still_unparseable_output() {
        // Braces alone do not make a payload: nothing parses, so the record says
        // `unparseable-output` and keeps the raw tail exactly as before the fallback existed.
        let found = classify(
            "fatal: worker exited mid-write { see dump above }",
            "",
            Some(1),
            1,
            Mode::Dispatch,
            "s",
            "e",
        );
        assert!(!found.parse_ok);
        assert_eq!(found.subtype.as_deref(), Some("unparseable-output"));
        assert_eq!(
            found.result_tail,
            "fatal: worker exited mid-write { see dump above }"
        );
    }

    #[test]
    fn a_rate_limit_that_killed_the_child_before_any_json_is_read_off_stderr() {
        // The child never emitted stdout JSON, so the payload path cannot speak; the verdict
        // sits on stderr, which `world` already wrote to disk. Sleeping beats re-entering.
        let found = classify(
            "",
            "You've hit your session limit · resets 5pm (Europe/Berlin)",
            None,
            4,
            Mode::Resume,
            "s",
            "e",
        );
        assert!(!found.parse_ok);
        assert!(found.rate_limited, "a stderr-only limit must still be one");
    }

    #[test]
    fn an_ordinary_death_on_stderr_is_not_mistaken_for_a_rate_limit() {
        let found = classify(
            "",
            "thread 'main' panicked at 'out of memory', src/never.rs:3:5",
            Some(101),
            4,
            Mode::Resume,
            "s",
            "e",
        );
        assert!(!found.parse_ok);
        assert!(!found.rate_limited, "no needle in the prose, no limit");
    }

    #[test]
    fn a_successful_attempt_is_not_rate_limited_by_noise_on_its_stderr() {
        // The stderr fold is gated on the payload failing to speak or the exit being non-zero:
        // a healthy attempt whose stderr mentions limits in passing stays healthy.
        let payload = serde_json::json!({"is_error": false, "result": "done"}).to_string();
        let found = classify(
            &payload,
            "note: the rate limit docs have moved",
            Some(0),
            5,
            Mode::Dispatch,
            "s",
            "e",
        );
        assert!(found.parse_ok);
        assert!(!found.rate_limited);
    }

    #[test]
    fn the_mode_a_record_holds_round_trips() {
        for mode in [Mode::Dispatch, Mode::Resume, Mode::CiBabysit] {
            let text = serde_json::to_string(&mode).unwrap();
            assert_eq!(serde_json::from_str::<Mode>(&text).unwrap(), mode);
        }
        assert_eq!(
            serde_json::to_string(&Mode::CiBabysit).unwrap(),
            "\"ci-babysit\""
        );
    }

    // --- stage invocations ---------------------------------------------------------------------

    const ALL_STAGES: [rung::Stage; 10] = [
        rung::Stage::Plan,
        rung::Stage::Triage,
        rung::Stage::PlanReview,
        rung::Stage::Work,
        rung::Stage::Simplify,
        rung::Stage::DiffTriage,
        rung::Stage::Review,
        rung::Stage::Validate,
        rung::Stage::Fixes,
        rung::Stage::Ship,
    ];

    fn stage_conditions() -> StageConditions<'static> {
        StageConditions {
            claude_bin: "/home/op/.grind/bin/claude",
            run_id: "run-20260822-001",
        }
    }

    fn stage_ctx<'a>(
        stage: rung::Stage,
        job: &'a Job,
        model: Option<&'a str>,
        notes: Option<&'a str>,
    ) -> StageContext<'a> {
        StageContext {
            stage,
            skill_text: "# Work\n\nDo the work and write the return.",
            stages_dir: "/home/op/.grind/runs/run-20260822-001/stages",
            worktree: "/home/op/.grind/runs/run-20260822-001/worktree",
            job,
            model,
            notes,
        }
    }

    #[test]
    fn stage_session_ids_are_run_id_dash_stage_for_all_ten_stages() {
        for stage in ALL_STAGES {
            assert_eq!(
                stage_session_id("run-1", stage),
                format!("run-1-{stage}"),
                "{stage}"
            );
        }
        assert_eq!(stage_session_id("run-1", rung::Stage::Work), "run-1-work");
    }

    #[test]
    fn a_stage_dispatch_opens_that_stages_own_session_with_no_plugin_flag() {
        let job = job();
        let ctx = stage_ctx(rung::Stage::Work, &job, None, None);
        let invocation = stage_dispatch(&stage_conditions(), &ctx);
        let argv = invocation.argv();
        assert!(argv.contains(&"--session-id".to_string()));
        let at = argv.iter().position(|a| a == "--session-id").unwrap();
        assert_eq!(argv[at + 1], "run-20260822-001-work");
        assert!(!argv.contains(&"--resume".to_string()));
        assert!(!argv.contains(&"--plugin-dir".to_string()));
    }

    #[test]
    fn a_stage_resume_resumes_that_stages_own_session_with_no_plugin_flag() {
        let job = job();
        let ctx = stage_ctx(rung::Stage::Review, &job, None, None);
        let invocation = stage_resume(&stage_conditions(), &ctx, None);
        let argv = invocation.argv();
        assert!(argv.contains(&"--resume".to_string()));
        let at = argv.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(argv[at + 1], "run-20260822-001-review");
        assert!(!argv.contains(&"--session-id".to_string()));
        assert!(!argv.contains(&"--plugin-dir".to_string()));
    }

    #[test]
    fn a_stage_model_puts_the_model_flag_on_the_argv_and_absence_omits_it() {
        let job = job();
        let with_model = stage_ctx(rung::Stage::Ship, &job, Some("claude-opus-5"), None);
        let invocation = stage_dispatch(&stage_conditions(), &with_model);
        assert!(
            invocation
                .argv()
                .windows(2)
                .any(|w| w[0] == "--model" && w[1] == "claude-opus-5")
        );

        let without_model = stage_ctx(rung::Stage::Ship, &job, None, None);
        let invocation = stage_dispatch(&stage_conditions(), &without_model);
        assert!(!invocation.argv().contains(&"--model".to_string()));
    }

    #[test]
    fn stage_invocation_routes_dispatch_and_resume_and_refuses_ci_babysit() {
        let job = job();
        let ctx = stage_ctx(rung::Stage::Fixes, &job, None, None);
        let dispatched = stage_invocation(&stage_conditions(), &ctx, Mode::Dispatch, None);
        assert_eq!(dispatched.mode(), Mode::Dispatch);
        let resumed = stage_invocation(&stage_conditions(), &ctx, Mode::Resume, None);
        assert_eq!(resumed.mode(), Mode::Resume);
    }

    #[test]
    #[should_panic(expected = "babysit continues Ship's session")]
    fn stage_invocation_panics_on_ci_babysit() {
        let job = job();
        let ctx = stage_ctx(rung::Stage::Ship, &job, None, None);
        let _ = stage_invocation(&stage_conditions(), &ctx, Mode::CiBabysit, None);
    }

    #[test]
    fn a_stage_prompt_carries_the_skill_text_verbatim_and_the_bounded_context() {
        let job = job();
        let ctx = stage_ctx(rung::Stage::Work, &job, None, None);
        let prompt = stage_dispatch(&stage_conditions(), &ctx)
            .prompt()
            .to_string();
        assert!(prompt.contains(ctx.skill_text));
        assert!(prompt.contains(ctx.stages_dir));
        assert!(prompt.contains(ctx.worktree));
        assert!(prompt.contains(&job.branch));
        assert!(prompt.contains(&job.base_branch));
        assert!(prompt.contains(&job.verify_entrypoint));
        assert!(prompt.contains(&job.done_predicate));
        assert!(prompt.contains(&job.anchor));
    }

    #[test]
    fn a_stage_resume_prompt_carries_the_stage_reentry_paragraph_and_no_dispatch_does() {
        let job = job();
        let ctx = stage_ctx(rung::Stage::Work, &job, None, None);
        let resumed = stage_resume(&stage_conditions(), &ctx, None)
            .prompt()
            .to_string();
        assert!(resumed.contains("Re-read this stage's own return file"));

        let dispatched = stage_dispatch(&stage_conditions(), &ctx)
            .prompt()
            .to_string();
        assert!(!dispatched.contains("Re-read this stage's own return file"));
    }

    #[test]
    fn a_stage_resume_prompt_carries_the_latest_clearance_when_given_and_nothing_otherwise() {
        let job = job();
        let ctx = stage_ctx(rung::Stage::Fixes, &job, None, None);
        let note = "the CI runner was fixed";
        let cleared = a_clearance(note);
        let with_note = stage_resume(&stage_conditions(), &ctx, Some(&cleared))
            .prompt()
            .to_string();
        assert!(with_note.contains("Since you stopped, the human reports"));
        assert!(with_note.contains(note));

        let without_note = stage_resume(&stage_conditions(), &ctx, None)
            .prompt()
            .to_string();
        assert!(!without_note.contains("Since you stopped, the human reports"));
    }

    #[test]
    fn plan_alone_injects_notes_and_lessons_into_its_dispatch_prompt() {
        let job = job();
        let notes = "the last Run mistook a Wait for a crash; watch for that.";
        let plan_ctx = stage_ctx(rung::Stage::Plan, &job, None, Some(notes));
        let plan_prompt = stage_dispatch(&stage_conditions(), &plan_ctx)
            .prompt()
            .to_string();
        assert!(plan_prompt.contains(notes));

        // The same notes text, offered to a non-Plan stage, is never rendered — only Plan reads
        // `ctx.notes` at all.
        let work_ctx = stage_ctx(rung::Stage::Work, &job, None, Some(notes));
        let work_prompt = stage_dispatch(&stage_conditions(), &work_ctx)
            .prompt()
            .to_string();
        assert!(!work_prompt.contains(notes));
    }

    #[test]
    fn every_stage_denial_set_carries_the_full_base_list_verbatim() {
        for stage in ALL_STAGES {
            let denied = denied_for(stage);
            for glob in DENIED_TOOLS {
                assert!(
                    denied.iter().any(|g| g == glob),
                    "{stage} must carry {glob}"
                );
            }
        }
        let reflect = denied_for_reflect();
        for glob in DENIED_TOOLS {
            assert!(
                reflect.iter().any(|g| g == glob),
                "reflect must carry {glob}"
            );
        }
    }

    #[test]
    fn report_only_stages_deny_write_and_edit_and_the_writing_stages_do_not() {
        for stage in [
            rung::Stage::PlanReview,
            rung::Stage::Review,
            rung::Stage::Validate,
        ] {
            let denied = denied_for(stage);
            assert!(denied.contains(&"Write".to_string()), "{stage}");
            assert!(denied.contains(&"Edit".to_string()), "{stage}");
        }
        for stage in [rung::Stage::Work, rung::Stage::Fixes, rung::Stage::Ship] {
            let denied = denied_for(stage);
            assert!(!denied.contains(&"Write".to_string()), "{stage}");
            assert!(!denied.contains(&"Edit".to_string()), "{stage}");
        }
    }

    #[test]
    fn only_the_two_fan_out_panels_carry_the_write_capable_bash_forms() {
        for stage in [rung::Stage::Review, rung::Stage::Validate] {
            let denied = denied_for(stage);
            for glob in PANEL_BASH_FORMS {
                assert!(
                    denied.iter().any(|g| g == glob),
                    "{stage} must carry {glob}"
                );
            }
        }
        for stage in [
            rung::Stage::Plan,
            rung::Stage::Triage,
            rung::Stage::PlanReview,
            rung::Stage::Work,
            rung::Stage::Simplify,
            rung::Stage::DiffTriage,
            rung::Stage::Fixes,
            rung::Stage::Ship,
        ] {
            let denied = denied_for(stage);
            assert!(
                !denied.iter().any(|g| *g == "Bash(git commit*)"),
                "{stage} must not carry a panel-only Bash form"
            );
        }
    }

    #[test]
    fn reflect_carries_the_write_capable_bash_forms_but_not_write_or_edit() {
        let reflect = denied_for_reflect();
        for glob in PANEL_BASH_FORMS {
            assert!(
                reflect.iter().any(|g| g == glob),
                "reflect must carry {glob}"
            );
        }
        assert!(
            !reflect.contains(&"Write".to_string()),
            "reflect must still write its own artifacts"
        );
        assert!(!reflect.contains(&"Edit".to_string()));
    }
}
