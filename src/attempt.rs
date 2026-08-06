//! One `claude` invocation: the argv the denials ride on, and the raw triple that hits disk
//! before anything reads it.
//!
//! The rule's one asterisk — a pure builder and a pure classifier around two `world` calls,
//! neither cleanly pure nor cleanly I/O (ADR-0007).

use crate::job::Job;
use crate::observe::Reason;
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
pub const DENIED_TOOLS: [&str; 7] = [
    "Bash(gh pr merge*)",
    "Bash(git push --force*)",
    "Bash(git push -f*)",
    "Bash(git reset --hard*)",
    "Bash(git rebase*)",
    "Bash(git checkout main*)",
    "Bash(git branch -D*)",
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

/// Everything an invocation is built from, all of it read from the record rather than the
/// environment — so re-entering an in-flight Run never changes its conditions mid-pipeline.
#[derive(Debug, Clone, Copy)]
pub struct Conditions<'a> {
    pub claude_bin: &'a str,
    pub session_id: &'a str,
    pub plugin_dir: &'a str,
    pub model: Option<&'a str>,
    pub spend_cap: Option<&'a str>,
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

/// Every later attempt, resuming the same session id.
pub fn resume(conditions: &Conditions) -> Invocation {
    build(conditions, Mode::Resume, REENTRY_PROMPT.to_string())
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
    if let Some(cap) = conditions.spend_cap {
        argv.push("--max-budget-usd".to_string());
        argv.push(cap.to_string());
    }
    // The last thing appended, on every path, from the one builder.
    argv.push("--disallowedTools".to_string());
    argv.extend(DENIED_TOOLS.iter().map(|glob| glob.to_string()));
    Invocation { argv, prompt, mode }
}

fn dispatch_prompt(job: &Job) -> String {
    format!(
        "You are a Grind Run, executing unattended with no human present.

Job:            {url}
Branch:         {branch}
Handoff SHA:    {handoff}
Anchor artifact: {anchor}

Everything behind the Handoff SHA is context the human prepared for you — read it.
Everything you add in front of it is reviewable output.

Invoke the `lfg` skill against the Anchor artifact, resolving the skill name against the
available-skills list (it may be namespaced, e.g. `compound-engineering:lfg`):

    {anchor}

The Anchor artifact is the requirements you must satisfy. Everything else you need is
discoverable from this branch. Its contents are already decided — this slice is
transcription, not design. Do not re-open decisions it records.

Definition of done: `just verify` passes.

If a step of `just verify` cannot be made green, say so plainly in the PR body and leave
the step intact. Never weaken, trim, skip or remove a step of the verify entrypoint to
make it pass — a gutted gate is worse than one that fails honestly.

Stop at an open PR. Do not merge it.",
        url = job.url,
        branch = job.branch,
        handoff = job.handoff_sha,
        anchor = job.anchor,
    )
}

/// What a child left behind, **after** it landed on disk.
///
/// Private fields, and [`run`] is the only constructor. The invariant does not rest on that
/// alone: `world` redirects both streams to their files *before* the child is spawned and hands
/// back only an exit code, so the parent cannot see a byte of the child's output without
/// reading the file it already wrote. *Parse before write* is not a thing to remember.
pub struct RawAttempt {
    stdout: String,
    code: Option<i32>,
}

impl RawAttempt {
    pub fn classify(&self, n: usize, mode: Mode, started_at: &str, ended_at: &str) -> Attempt {
        classify(&self.stdout, self.code, n, mode, started_at, ended_at)
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
    // Read back what is already on disk, rather than what the parent buffered.
    let stdout = world::read_to_string(stdout_path).unwrap_or_default();
    Ok(RawAttempt { stdout, code })
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
}

/// The pure classifier over a raw triple.
///
/// **`subtype` is not the outcome.** It read `success` on all five of Run 1's attempts including
/// the three that died, and on all six of Run 2's rate-limited ones. `terminal_reason` and the
/// API error status are the discriminators.
pub fn classify(
    stdout: &str,
    code: Option<i32>,
    n: usize,
    mode: Mode,
    started_at: &str,
    ended_at: &str,
) -> Attempt {
    let parsed: Option<serde_json::Value> = serde_json::from_str(stdout).ok();
    let parse_ok = parsed.is_some();
    let value = parsed.unwrap_or(serde_json::Value::Null);

    let result = text_at(&value, "result").unwrap_or_default();
    let is_error = value
        .get("is_error")
        .and_then(|v| v.as_bool())
        .unwrap_or(!parse_ok);

    Attempt {
        n,
        mode,
        started_at: started_at.to_string(),
        ended_at: ended_at.to_string(),
        exit_code: code,
        is_error,
        parse_ok,
        subtype: if parse_ok {
            text_at(&value, "subtype")
        } else {
            Some("unparseable-output".to_string())
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
        rate_limited: is_rate_limited(&value),
        // The tail is kept whether or not the response parsed, so an unreadable child still
        // leaves something diagnosable.
        result_tail: tail(if parse_ok { &result } else { stdout }, 1500),
    }
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
            budget: Some("$12.50".to_string()),
            model: None,
            plugin: PluginPin::parse("compound-engineering@compound-engineering-plugin 3.21.3")
                .unwrap(),
        }
    }

    fn conditions<'a>(model: Option<&'a str>, cap: Option<&'a str>) -> Conditions<'a> {
        Conditions {
            claude_bin: "/home/op/.grind/bin/claude",
            session_id: "d51b4c39-ce1d-449b-8366-04b9b1aa6573",
            plugin_dir: "/home/op/.claude/plugins/cache/m/n/3.21.3",
            model,
            spend_cap: cap,
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
    fn every_built_argv_carries_all_seven_globs_on_all_three_paths() {
        let conditions = conditions(None, None);
        for invocation in [
            dispatch(&conditions, &job()),
            resume(&conditions),
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

    #[test]
    fn the_first_attempt_opens_a_session_and_every_later_one_resumes_it() {
        let conditions = conditions(None, None);
        let first = dispatch(&conditions, &job());
        assert!(first.argv().contains(&"--session-id".to_string()));
        assert!(!first.argv().contains(&"--resume".to_string()));

        for later in [resume(&conditions), ci_babysit(&conditions)] {
            assert!(later.argv().contains(&"--resume".to_string()));
            assert!(!later.argv().contains(&"--session-id".to_string()));
            let at = later.argv().iter().position(|a| a == "--resume").unwrap();
            assert_eq!(later.argv()[at + 1], conditions.session_id);
        }
    }

    #[test]
    fn the_argv_shape_is_the_one_two_runs_actually_used() {
        let invocation = dispatch(&conditions(Some("claude-opus-5"), Some("12.50")), &job());
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
        assert!(
            argv.windows(2)
                .any(|w| w[0] == "--max-budget-usd" && w[1] == "12.50")
        );
        assert!(argv.windows(2).any(|w| w[0] == "--plugin-dir"));
    }

    #[test]
    fn no_budget_ceiling_and_no_model_mean_no_flags_for_them() {
        let invocation = dispatch(&conditions(None, None), &job());
        assert!(!invocation.argv().contains(&"--max-budget-usd".to_string()));
        assert!(!invocation.argv().contains(&"--model".to_string()));
    }

    #[test]
    fn the_dispatch_prompt_carries_the_jobs_own_four_facts() {
        let invocation = dispatch(&conditions(None, None), &job());
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
            let found = classify(raw, Some(1), n, Mode::Resume, "start", "end");
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
        for raw in [TRUNCATED, GARBAGE, EMPTY] {
            let found = classify(raw, Some(1), 1, Mode::Dispatch, "start", "end");
            assert!(!found.parse_ok, "this fixture does not parse");
            assert_eq!(found.subtype.as_deref(), Some("unparseable-output"));
            assert!(found.is_error);
            assert_eq!(
                found.result_tail,
                tail(raw, 1500),
                "the tail is what was actually there"
            );
        }
        // The truncated one is the case that matters: bytes arrived and then stopped.
        let truncated = classify(TRUNCATED, Some(1), 1, Mode::Dispatch, "start", "end");
        assert!(!truncated.result_tail.is_empty());
    }

    #[test]
    fn a_killed_childs_empty_stdout_is_recorded_rather_than_lost() {
        let killed = classify("", None, 4, Mode::Resume, "start", "end");
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
        assert!(classify(&promised, Some(0), 5, Mode::Resume, "s", "e").done_promise);

        let unpromised = serde_json::json!({
            "is_error": false,
            "subtype": "success",
            "result": "Made progress but the pipeline has not reached an open PR yet.",
        })
        .to_string();
        assert!(!classify(&unpromised, Some(0), 4, Mode::Resume, "s", "e").done_promise);
    }

    #[test]
    fn a_recorded_denial_survives_onto_the_attempt() {
        let denied = serde_json::json!({
            "is_error": false,
            "result": "the push was refused",
            "permission_denials": [{"tool_name": "Bash", "tool_input": {"command": "git push --force"}}],
        })
        .to_string();
        let found = classify(&denied, Some(0), 1, Mode::CiBabysit, "s", "e");
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
