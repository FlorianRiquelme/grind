//! One `claude` invocation: the argv the denials ride on, and the raw triple that hits disk
//! before anything reads it.
//!
//! The rule's one asterisk — a pure builder and a pure classifier around two `world` calls,
//! neither cleanly pure nor cleanly I/O (ADR-0007).

use crate::job::Job;
use crate::observe::{Observed, Reason};
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

/// Re-entry rides Claude Code's own session resume, not an `lfg` return value: `lfg` exposes no
/// structured return to its caller. Resuming the session restores which stage it was on; this
/// prompt only tells it not to redo finished work.
pub const REENTRY_PROMPT: &str = "You were interrupted mid-run and have just been resumed.

Re-read the working tree and `git log` to establish where the pipeline actually got to,
then continue `lfg` from the stage that did not complete. Do not restart stages that
already produced their artifact, and do not open a second PR.

Everything in the original instruction still applies — especially: never weaken, trim or
skip a step of `just verify` to make it pass.";

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

/// Everything an invocation is built from, all of it read from the record rather than the
/// environment — so re-entering an in-flight Run never changes its conditions mid-pipeline.
#[derive(Debug, Clone, Copy)]
pub struct Conditions<'a> {
    pub claude_bin: &'a str,
    pub session_id: &'a str,
    pub plugin_dir: &'a str,
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

/// The first attempt, which opens the session id.
pub fn dispatch(conditions: &Conditions, job: &Job) -> Invocation {
    build(conditions, Mode::Dispatch, dispatch_prompt(job))
}

/// Every later attempt, resuming the same session id. The latest clearance note rides this
/// prompt and only this one: Dispatch has no stop behind it, and CiBabysit bounds itself to
/// one reaction and must not grow a second subject (R2).
pub fn resume(conditions: &Conditions, cleared: Option<&str>) -> Invocation {
    build(conditions, Mode::Resume, reentry_prompt(cleared))
}

/// `REENTRY_PROMPT`, with the human's clearance note composed after it when one exists.
/// **Default is silence**, the same rule as `intent_line`: with no note the prompt is the
/// constant, exactly — nothing is rendered where nothing was recorded.
fn reentry_prompt(cleared: Option<&str>) -> String {
    match cleared {
        Some(note) => format!(
            "{REENTRY_PROMPT}\n\nSince you stopped, the human reports: {note}\n\nThat is \
             what changed in the world since your last observation. Trust it over what you \
             last saw of the obstacle, and do not spend turns re-probing what it says is \
             cleared."
        ),
        None => REENTRY_PROMPT.to_string(),
    }
}

/// The one bounded invocation a decided-and-failing CI buys. **The same builder** — there is no
/// second argv path, so the denials ride it by construction.
pub fn ci_babysit(conditions: &Conditions) -> Invocation {
    build(conditions, Mode::CiBabysit, CI_BABYSIT_PROMPT.to_string())
}

fn build(conditions: &Conditions, mode: Mode, prompt: String) -> Invocation {
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
    match mode {
        Mode::Dispatch => {
            argv.push("--session-id".to_string());
            argv.push(conditions.session_id.to_string());
        }
        Mode::Resume | Mode::CiBabysit => {
            argv.push("--resume".to_string());
            argv.push(conditions.session_id.to_string());
        }
    }
    argv.push("--plugin-dir".to_string());
    argv.push(conditions.plugin_dir.to_string());
    // No `--max-budget-usd`. ADR-0010: spend is recorded, never bounded — a number someone
    // guessed at Enqueue must not kill a Run mid-work for being larger than the guess.
    // The last thing appended, on every path, from the one builder.
    argv.push("--disallowedTools".to_string());
    argv.extend(DENIED_TOOLS.iter().map(|glob| glob.to_string()));
    Invocation { argv, prompt, mode }
}

/// The Run is told only what Grind can know.
///
/// Two constants went out of it. *No human present* invited the Run to ask a question and wait
/// for an answer; *unsupervised* says the same thing about attention without implying anyone is
/// there to be addressed. And *this slice is transcription, not design* is false of any Job
/// wider than a rewrite — the half that is true of every Job, **do not re-open decisions the
/// Anchor records**, survives the half that is not.
///
/// Nothing reads the narrative or the closing keyword back. They are asked for because the
/// human reads them, and a Grind that keyed a verdict on either would be grading prose.
fn dispatch_prompt(job: &Job) -> String {
    format!(
        "You are a Grind Run, executing unattended and unsupervised. Nobody is watching this
session and no question you ask will be answered, so decide and proceed.

Job:            {url}
Branch:         {branch}
Handoff SHA:    {handoff}
Anchor artifact: {anchor}
{intent}
The Handoff SHA bounds your **output**, not your **reading**. Everything you add sits in
front of it and is what gets reviewed; read as far around it as you need, including work
that landed after it.

Invoke the `lfg` skill against the Anchor artifact, resolving the skill name against the
available-skills list (it may be namespaced, e.g. `compound-engineering:lfg`):

    {anchor}

The Anchor artifact is the requirements you must satisfy. Everything else you need is
discoverable from this branch. Do not re-open decisions it records.

Before you create a file in a shared sequential namespace — a numbered ADR, a migration, a
changelog entry — read the current state of that namespace. Read it again on each attempt
rather than trusting a view you took earlier: other work lands while you run, and two files
claiming one number is a collision a human has to unpick by hand.

Definition of done: `just verify` passes.

If a step of `just verify` cannot be made green, say so plainly in the PR body and leave
the step intact. Never weaken, trim, skip or remove a step of the verify entrypoint to
make it pass — a gutted gate is worse than one that fails honestly.

Put a narrative in the PR body: the decisions you took, whatever was non-obvious, and
anything that surprised you. Those are categories and not a template — no headings, no
order, no required sections, and nothing at all where there is nothing to say.

Put `Closes #{issue}` in the PR body where this PR delivers the whole Job. Where the Job is
wider than the code, reference it without the keyword instead.

Stop at an open PR. Do not merge it.",
        url = job.url,
        branch = job.branch,
        handoff = job.handoff_sha,
        anchor = job.anchor,
        issue = job.issue,
        intent = intent_line(job.intent.as_deref()),
    )
}

/// **Default is silence.** The first `Option`-gated line in the prompt: saying nothing about
/// the work's nature is honest, and a wrong constant is not — which is exactly what *this slice
/// is transcription, not design* was.
fn intent_line(intent: Option<&str>) -> String {
    match intent {
        Some(said) => format!("Intent:         {said}\n"),
        None => String::new(),
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
    use crate::job::PluginPin;

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
            plugin: PluginPin::parse("compound-engineering@compound-engineering-plugin 3.21.3")
                .unwrap(),
        }
    }

    fn conditions(model: Option<&str>) -> Conditions<'_> {
        Conditions {
            claude_bin: "/home/op/.grind/bin/claude",
            session_id: "d51b4c39-ce1d-449b-8366-04b9b1aa6573",
            plugin_dir: "/home/op/.claude/plugins/cache/m/n/3.21.3",
            model,
        }
    }

    /// Every prompt change is asserted against the **built** prompt string, never against the
    /// constant it came from.
    fn built_dispatch_prompt() -> String {
        let built = dispatch(&conditions(None), &job());
        built.prompt().to_string()
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
    fn every_built_argv_carries_all_twelve_globs_on_all_three_paths() {
        let conditions = conditions(None);
        for invocation in [
            dispatch(&conditions, &job()),
            resume(&conditions, None),
            resume(&conditions, Some("the wall moved")),
            ci_babysit(&conditions),
        ] {
            assert_eq!(
                denials_of(&invocation),
                DENIED_TOOLS.to_vec(),
                "{:?} must carry every denial",
                invocation.mode()
            );
        }
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
    fn the_first_attempt_opens_a_session_and_every_later_one_resumes_it() {
        let conditions = conditions(None);
        let first = dispatch(&conditions, &job());
        assert!(first.argv().contains(&"--session-id".to_string()));
        assert!(!first.argv().contains(&"--resume".to_string()));

        for later in [resume(&conditions, None), ci_babysit(&conditions)] {
            assert!(later.argv().contains(&"--resume".to_string()));
            assert!(!later.argv().contains(&"--session-id".to_string()));
            let at = later.argv().iter().position(|a| a == "--resume").unwrap();
            assert_eq!(later.argv()[at + 1], conditions.session_id);
        }
    }

    #[test]
    fn the_argv_shape_is_the_one_two_runs_actually_used() {
        let invocation = dispatch(&conditions(Some("claude-opus-5")), &job());
        let argv = invocation.argv();
        assert_eq!(argv[0], "/home/op/.grind/bin/claude");
        assert_eq!(
            argv[1..6].to_vec(),
            vec!["-p", "--output-format", "json", "--permission-mode"]
                .into_iter()
                .chain(std::iter::once("bypassPermissions"))
                .map(String::from)
                .collect::<Vec<_>>()
        );
        assert!(
            argv.windows(2)
                .any(|w| w[0] == "--model" && w[1] == "claude-opus-5")
        );
        assert!(argv.windows(2).any(|w| w[0] == "--plugin-dir"));
    }

    #[test]
    fn no_built_argv_on_any_of_the_three_paths_carries_a_spend_ceiling() {
        // ADR-0010: spend is recorded, never bounded. A number someone guessed at Enqueue must
        // not kill a Run mid-work for being larger than the guess.
        let conditions = conditions(Some("claude-opus-5"));
        for invocation in [
            dispatch(&conditions, &job()),
            resume(&conditions, None),
            ci_babysit(&conditions),
        ] {
            assert!(
                !invocation.argv().contains(&"--max-budget-usd".to_string()),
                "{:?}",
                invocation.mode()
            );
        }
    }

    #[test]
    fn no_model_means_no_flag_for_it() {
        let invocation = dispatch(&conditions(None), &job());
        assert!(!invocation.argv().contains(&"--model".to_string()));
    }

    #[test]
    fn the_dispatch_prompt_carries_the_jobs_own_four_facts() {
        let invocation = dispatch(&conditions(None), &job());
        for fact in [
            "issues/28",
            "feat/28-slice-1b",
            "9d1f4c7a",
            "docs/plans/a.md",
        ] {
            assert!(invocation.prompt().contains(fact), "missing {fact}");
        }
        assert!(
            invocation
                .prompt()
                .contains("Stop at an open PR. Do not merge it.")
        );
    }

    #[test]
    fn the_dispatch_prompt_says_unsupervised_rather_than_alone() {
        // *No human present* invited the Run to ask a question and wait for an answer.
        let prompt = built_dispatch_prompt();
        assert!(prompt.contains("unsupervised"), "{prompt}");
        assert!(!prompt.contains("no human present"), "{prompt}");
        assert!(
            prompt.contains("executing unattended"),
            "the phrase that was right stays: {prompt}"
        );
    }

    #[test]
    fn the_dispatch_prompt_stops_calling_the_work_transcription() {
        // False of any Job wider than a rewrite, and this one is a slice with design left in it.
        let prompt = built_dispatch_prompt();
        assert!(!prompt.contains("transcription"), "{prompt}");
        assert!(
            prompt.contains("Do not re-open decisions it records"),
            "the half that is true of every Job survives: {prompt}"
        );
    }

    #[test]
    fn the_dispatch_prompt_bounds_output_rather_than_reading() {
        let prompt = built_dispatch_prompt();
        let at = prompt
            .find("Handoff SHA bounds")
            .expect("the Handoff SHA paragraph");
        let paragraph = &prompt[at..at + 200.min(prompt.len() - at)];
        assert!(paragraph.contains("output"), "{paragraph}");
        assert!(paragraph.contains("reading"), "{paragraph}");
    }

    #[test]
    fn the_dispatch_prompt_asks_for_a_narrative_by_category_and_not_by_template() {
        // Naming a structure is how a narrative Grind promised not to parse becomes one it
        // parses.
        let prompt = built_dispatch_prompt();
        for category in ["decisions you took", "non-obvious", "surprised you"] {
            assert!(prompt.contains(category), "missing {category}: {prompt}");
        }
        assert!(prompt.contains("no headings"), "{prompt}");
        assert!(prompt.contains("no required sections"), "{prompt}");
    }

    #[test]
    fn the_dispatch_prompt_asks_for_the_closing_keyword_and_licenses_declining_it() {
        let prompt = built_dispatch_prompt();
        assert!(prompt.contains("Closes #28"), "{prompt}");
        assert!(
            prompt.contains("without the keyword"),
            "a Job wider than its code closes nothing: {prompt}"
        );
    }

    #[test]
    fn the_dispatch_prompt_names_the_shared_sequential_namespaces() {
        let prompt = built_dispatch_prompt();
        for named in ["numbered ADR", "migration", "changelog entry"] {
            assert!(prompt.contains(named), "missing {named}: {prompt}");
        }
        assert!(
            prompt.contains("on each attempt"),
            "a per-Attempt read, not a pinned view: {prompt}"
        );
    }

    #[test]
    fn a_job_with_an_intent_row_puts_that_line_in_the_built_prompt() {
        let stated = Job {
            intent: Some("A settled plan transcribed into one module.".to_string()),
            ..job()
        };
        let prompt = dispatch(&conditions(None), &stated).prompt().to_string();
        assert!(
            prompt.contains("Intent:         A settled plan transcribed into one module."),
            "{prompt}"
        );
    }

    #[test]
    fn a_job_with_no_intent_row_puts_no_characterisation_of_the_work_in_the_prompt() {
        // Default is silence. Saying nothing about the work's nature is honest; a wrong
        // constant is not, which is exactly what *this slice is transcription, not design* was.
        let prompt = built_dispatch_prompt();
        assert!(!prompt.contains("Intent"), "{prompt}");
        for characterisation in [
            "transcription",
            "mechanical",
            "straightforward",
            "exploratory",
            "greenfield",
        ] {
            assert!(!prompt.contains(characterisation), "{prompt}");
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

    #[test]
    fn a_clearance_note_rides_the_resume_prompt_and_never_dispatch_or_ci_babysit() {
        // The safety property R6 names: the composed prompt reaches Resume invocations only.
        // Asserted against the **built** invocations, in the spirit of the argv tests above.
        let conditions = conditions(None);
        let note = "the deploy key was rotated; the push will go through now";
        let resumed = resume(&conditions, Some(note));
        assert!(
            resumed.prompt().starts_with(REENTRY_PROMPT),
            "the note composes after the re-entry text, never instead of it:\n{}",
            resumed.prompt()
        );
        assert!(
            resumed
                .prompt()
                .contains("Since you stopped, the human reports:"),
            "{}",
            resumed.prompt()
        );
        assert!(resumed.prompt().contains(note), "{}", resumed.prompt());
        for other in [dispatch(&conditions, &job()), ci_babysit(&conditions)] {
            assert!(
                !other.prompt().contains(note),
                "{:?} must not carry the note",
                other.mode()
            );
            assert!(
                !other.prompt().contains("the human reports"),
                "{:?} must not grow a clearance paragraph",
                other.mode()
            );
        }
    }

    #[test]
    fn no_note_composes_the_reentry_prompt_exactly() {
        // Render nothing when no note exists — the prompt is the constant, byte for byte,
        // so a Run that was never blocked re-enters exactly as it always has.
        assert_eq!(resume(&conditions(None), None).prompt(), REENTRY_PROMPT);
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
}
