//! Grind-defined tools for the native adapter (R7): the registry, the pre-execution
//! gate, and first-party execution. Owned by the wave-1 tools slice; this skeleton
//! carries only the frozen surface.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::world::{self, Completed};

/// Tool output larger than this is truncated before it re-enters the context.
pub const MAX_TOOL_OUTPUT: usize = 8 * 1024;

/// The standard toolkit, in definition order.
const STANDARD_NAMES: [&str; 3] = ["bash", "read_file", "write_file"];

/// Provider credentials the supervisor's own environment carries so it can call the model at
/// all (`runner::Endpoint`). The native backend hands the `bash` tool's shell to that same
/// model, which can read files and receive prompt injection from the target repo, so these two
/// are scrubbed from the child before it spawns rather than trusted not to be echoed back.
const CREDENTIAL_ENV_VARS: [&str; 2] = ["OPENROUTER_API_KEY", "OPENAI_API_KEY"];

/// Wall-clock bound on one `bash` tool call. A stage legitimately runs the target repo's whole
/// `just verify` (or equivalent) as part of its work, which can take minutes for a real build,
/// so this has to be generous enough that a real build is never cut off — it exists to catch a
/// hung child (an accidental `tail -f`, a stalled network call), not to enforce a time budget.
/// 600s is a chosen default, not a measured one, and cheap to change.
const BASH_TIMEOUT: Duration = Duration::from_secs(600);

/// OpenAI function-schema description of one tool.
#[derive(Clone, Debug)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: serde_json::Value,
}

/// The tools a native attempt may call, bound to its working directory.
pub struct ToolRegistry {
    workdir: PathBuf,
}

impl ToolRegistry {
    /// bash / read_file / write_file — the standard stage toolkit.
    pub fn standard(workdir: PathBuf) -> Self {
        Self { workdir }
    }

    /// OpenAI `tools` array entries, POC-verbatim schemas.
    pub fn defs(&self) -> Vec<ToolDef> {
        let _ = &self.workdir;
        vec![
            ToolDef {
                name: "bash",
                description: "Run a shell command inside the working directory. Use for building, testing, inspecting.",
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {"command": {"type": "string"}},
                    "required": ["command"],
                }),
            },
            ToolDef {
                name: "read_file",
                description: "Read a file from the working directory.",
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                    "required": ["path"],
                }),
            },
            ToolDef {
                name: "write_file",
                description: "Create or overwrite a file in the working directory.",
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "content": {"type": "string"},
                    },
                    "required": ["path", "content"],
                }),
            },
        ]
    }

    /// Execute an ALREADY-GATED call. Tool-level failures are string outcomes fed back
    /// to the model, never Rust errors.
    pub fn execute(&self, call: &RawCall) -> ToolOutcome {
        let parsed: Value = serde_json::from_str(&call.arguments_json).unwrap_or(Value::Null);
        let field = |key: &str| parsed[key].as_str().unwrap_or_default().to_string();
        let (full, exit) = match call.name.as_str() {
            "bash" => {
                let completed = world::run_bounded(
                    &["sh".to_string(), "-c".to_string(), field("command")],
                    Some(self.workdir.as_path()),
                    &CREDENTIAL_ENV_VARS,
                    BASH_TIMEOUT,
                );
                let exit = completed.code;
                (format_completed(completed), exit)
            }
            "read_file" => {
                let path = resolve(&self.workdir, &field("path"));
                match crate::world::read_to_string(&path) {
                    Ok(s) => (s, None),
                    Err(e) => (format!("read failed: {e}"), None),
                }
            }
            "write_file" => {
                let path = resolve(&self.workdir, &field("path"));
                match crate::world::write(&path, &field("content")) {
                    Ok(()) => ("ok".to_string(), None),
                    Err(e) => (format!("write failed: {e}"), None),
                }
            }
            other => (format!("unknown tool {other}"), None),
        };
        ToolOutcome {
            output: truncate_output(&full),
            truncated: full.len() > MAX_TOOL_OUTPUT,
            exit,
        }
    }

    pub fn names(&self) -> Vec<&'static str> {
        STANDARD_NAMES.to_vec()
    }
}

#[derive(Clone, Debug)]
pub struct RawCall {
    pub id: String,
    pub name: String,
    /// Raw JSON arguments text as received.
    pub arguments_json: String,
}

/// Which first-party layer refused the call (R7: denials are structured events naming
/// the gating layer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateLayer {
    UnknownTool,
    InvalidArgs,
    DeniedGlob,
    PathEscape,
}

#[derive(Clone, Debug)]
pub struct GateReport {
    pub layer: GateLayer,
    pub tool: String,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub enum GateDecision {
    Allowed(RawCall),
    Denied(GateReport),
}

/// Gate BEFORE execution against this stage's denied-tool globs (the same
/// `attempt::denied_for` list the claude-code adapter passes to --disallowedTools).
///
/// Matching mirrors Claude Code's own deny-glob matcher as encoded in
/// `attempt::DENIED_TOOLS`: a `Bash(<pattern>)` entry matches shell commands of the
/// bash tool — per subcommand after splitting on shell separators, `*` the only
/// wildcard — while a bare entry matches tool names directly.
pub fn gate(denied_globs: &[String], raw: RawCall) -> GateDecision {
    // 1. The tool must exist at all.
    if !STANDARD_NAMES.contains(&raw.name.as_str()) {
        return GateDecision::Denied(GateReport {
            layer: GateLayer::UnknownTool,
            tool: raw.name.clone(),
            reason: format!("no such tool {}", raw.name),
        });
    }
    // 2. Arguments must be a JSON object.
    let args: Value = match serde_json::from_str(&raw.arguments_json) {
        Ok(v) => v,
        Err(_) => {
            return GateDecision::Denied(GateReport {
                layer: GateLayer::InvalidArgs,
                tool: raw.name.clone(),
                reason: "tool arguments were not valid JSON".to_string(),
            });
        }
    };
    if !args.is_object() {
        return GateDecision::Denied(GateReport {
            layer: GateLayer::InvalidArgs,
            tool: raw.name.clone(),
            reason: "tool arguments were not a JSON object".to_string(),
        });
    }
    // 3. Denied-tool globs.
    let command = || args["command"].as_str().unwrap_or_default();
    for glob in denied_globs {
        let denied = match glob.strip_prefix("Bash(").and_then(|g| g.strip_suffix(')')) {
            // Shell-shaped glob: applies to what the bash tool would run.
            Some(pattern) => {
                raw.name == "bash"
                    && subcommands_of(command())
                        .iter()
                        .any(|sub| glob_matches(pattern, sub))
            }
            // Bare name glob (`Write`, `Edit`, ...): matches the tool name itself.
            None => glob_matches(glob, &raw.name),
        };
        if denied {
            return GateDecision::Denied(GateReport {
                layer: GateLayer::DeniedGlob,
                tool: raw.name.clone(),
                reason: format!("tool call matched denied glob `{glob}`"),
            });
        }
    }
    // 4. Path-carrying tools must stay inside the working directory.
    if matches!(raw.name.as_str(), "read_file" | "write_file")
        && !resolves_within(args["path"].as_str().unwrap_or_default())
    {
        return GateDecision::Denied(GateReport {
            layer: GateLayer::PathEscape,
            tool: raw.name.clone(),
            reason: "path resolves outside the working directory".to_string(),
        });
    }
    GateDecision::Allowed(raw)
}

#[derive(Clone, Debug)]
pub struct ToolOutcome {
    pub output: String,
    pub truncated: bool,
    pub exit: Option<i32>,
}

/// Format a finished child POC-verbatim, plus the one case the POC never had to render:
/// `world::run_bounded` folds both *never started* (a spawn failure) and *started but killed on
/// its deadline* into the same `code: None` shape, with the reason in `stderr` either way. The
/// wording here is generic over both rather than "failed to spawn" — which would read as a
/// contradiction for a command the model can see printed output for — so a model reading a
/// timeout result can tell its command was killed on a deadline rather than that it never ran.
fn format_completed(completed: Completed) -> String {
    match completed.code {
        Some(code) => format!(
            "exit: {code}\nstdout:\n{}\nstderr:\n{}",
            completed.stdout, completed.stderr
        ),
        None => format!(
            "did not complete: {}\nstdout:\n{}",
            completed.stderr, completed.stdout
        ),
    }
}

/// Resolve a model-supplied path against the working directory: strip any leading
/// `/`, then join. Mirrors the POC's `resolve`.
fn resolve(workdir: &Path, raw: &str) -> PathBuf {
    let stripped = raw.trim_start_matches('/');
    workdir.join(stripped)
}

/// Lexical containment check for a model-supplied path: after leading-`/` strip,
/// does it still climb out of the working directory with `..`?
fn resolves_within(raw: &str) -> bool {
    let stripped = raw.trim_start_matches('/');
    let mut depth: usize = 0;
    for component in Path::new(stripped).components() {
        match component {
            std::path::Component::Normal(_) => depth += 1,
            std::path::Component::CurDir => {}
            // More `..` than real components means we are above the workdir root.
            std::path::Component::ParentDir => match depth.checked_sub(1) {
                Some(d) => depth = d,
                None => return false,
            },
            // Unreachable after the strip; treat defensively as contained.
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {}
        }
    }
    true
}

/// Claude Code's deny-glob matcher, as documented at `attempt::DENIED_TOOLS`:
/// full-string match, `*` the only wildcard, byte-exact otherwise.
///
/// `pub(crate)` because this is now the **only** copy: `attempt`'s test module used to carry a
/// hand-written duplicate to check the forbidden-spelling table against, and nothing bound the
/// two, so they could drift while `just verify` stayed green. `attempt`'s tests import this one
/// instead, so the table validates the matcher grind actually runs.
pub(crate) fn glob_matches(pattern: &str, candidate: &str) -> bool {
    fn rec(p: &[u8], c: &[u8]) -> bool {
        match p.first() {
            None => c.is_empty(),
            Some(b'*') => rec(&p[1..], c) || (!c.is_empty() && rec(p, &c[1..])),
            Some(head) => c.first() == Some(head) && rec(&p[1..], &c[1..]),
        }
    }
    rec(pattern.as_bytes(), candidate.as_bytes())
}

/// Split a compound shell command the way the documented matcher does, before per-subcommand
/// glob matching — then widen the candidate set past what a plain separator split sees.
///
/// Splitting on `&&`/`;`/`|`/etc. alone misses three shapes that still reach a shell:
/// `echo $(gh pr merge 123)` and `` `git push --force origin main` `` hide the forbidden verb
/// inside a substitution or subshell, and `GIT_DIR=. gh pr merge 123` hides it behind a leading
/// `NAME=value` assignment so the candidate no longer *starts with* `gh`. Both gaps are closed
/// by adding candidates, never by removing one: the inside of every `$( )`, backtick span and
/// `( )` subshell becomes its own candidate (recursively re-run through this same pipeline, so
/// a substitution nested inside another is still found), and each candidate additionally has any
/// leading assignment tokens stripped. More candidates can only mean more refusals, never a
/// false allow, which is the direction widening is allowed to move in.
///
/// **This is not a shell parser and is not trying to become one.** It has no notion of quoting,
/// escaping or comments, so a quoted separator inside `sh -c '...'` can still fragment a
/// candidate early — the glob still matches the fragment carrying the verb, but a determined
/// adversary constructing shell syntax by hand has more room here than the matcher closes. It
/// narrows the bypass surface documented above; it does not claim to eliminate it.
pub(crate) fn subcommands_of(command: &str) -> Vec<String> {
    let mut queue = vec![command.to_string()];
    let mut seen: Vec<String> = Vec::new();
    let mut candidates = Vec::new();
    while let Some(current) = queue.pop() {
        if seen.contains(&current) {
            continue;
        }
        seen.push(current.clone());
        let mut pieces = vec![current.clone()];
        for separator in ["|&", "&&", "||", ";", "|", "&", "\n"] {
            pieces = pieces
                .iter()
                .flat_map(|p| p.split(separator).map(str::to_string).collect::<Vec<_>>())
                .collect();
        }
        for piece in pieces {
            let trimmed = piece.trim().to_string();
            queue.extend(extract_spans(&trimmed));
            // The plain trimmed piece stays a candidate regardless — stripping only adds a
            // second candidate on top, so a future glob with no fixed prefix cannot lose a
            // match this already had.
            let stripped = strip_leading_assignments(&trimmed);
            candidates.push(trimmed.clone());
            if stripped != trimmed {
                candidates.push(stripped);
            }
        }
    }
    candidates
}

/// The inside of every `$( ... )`, backtick `` `...` `` and bare `( ... )` span in `text`,
/// outermost only (an inner span reaches the queue on its own next pass in [`subcommands_of`]).
/// Unbalanced or unterminated spans are skipped rather than guessed at.
fn extract_spans(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && bytes.get(i + 1) == Some(&b'(') {
            match balanced_parens(text, i + 1) {
                Some((inner, end)) => {
                    spans.push(inner);
                    i = end;
                    continue;
                }
                None => i += 1,
            }
        } else if bytes[i] == b'(' {
            match balanced_parens(text, i) {
                Some((inner, end)) => {
                    spans.push(inner);
                    i = end;
                    continue;
                }
                None => i += 1,
            }
        } else if bytes[i] == b'`' {
            match text[i + 1..].find('`') {
                Some(offset) => {
                    let end = i + 1 + offset;
                    spans.push(text[i + 1..end].to_string());
                    i = end + 1;
                    continue;
                }
                None => i += 1,
            }
        } else {
            i += 1;
        }
    }
    spans
}

/// `open` must index the `(` byte itself. Returns the text strictly between that `(` and its
/// matching `)` (nesting counted, not quote-aware), plus the byte index just past the `)`.
fn balanced_parens(text: &str, open: usize) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    debug_assert_eq!(bytes.get(open), Some(&b'('));
    let mut depth: i32 = 0;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((text[open + 1..i].to_string(), i + 1));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Strip leading `NAME=value` assignment tokens (`GIT_DIR=. gh pr merge 123` ->
/// `gh pr merge 123`), so a prefix-anchored glob still sees the verb at the front.
fn strip_leading_assignments(command: &str) -> String {
    let mut rest = command.trim_start();
    loop {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let token = &rest[..end];
        if is_assignment_token(token) {
            rest = rest[end..].trim_start();
        } else {
            break;
        }
    }
    rest.to_string()
}

/// `NAME=...`: a non-empty leading identifier (letters, digits, underscore; not starting with a
/// digit) followed by `=`. Shell assignment syntax, not general `=` use — `gh api -X DELETE` is
/// untouched because `-X` is not an identifier.
fn is_assignment_token(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Char-boundary-safe truncation with a `…[truncated N bytes]` suffix.
pub fn truncate_output(s: &str) -> String {
    if s.len() <= MAX_TOOL_OUTPUT {
        return s.to_string();
    }
    let mut end = MAX_TOOL_OUTPUT;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n…[truncated {} bytes]", &s[..end], s.len() - end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, arguments_json: &str) -> RawCall {
        RawCall {
            id: "t0".to_string(),
            name: name.to_string(),
            arguments_json: arguments_json.to_string(),
        }
    }

    fn allowed_layer(decision: GateDecision) -> Option<RawCall> {
        match decision {
            GateDecision::Allowed(raw) => Some(raw),
            GateDecision::Denied(report) => panic!("unexpected denial: {report:?}"),
        }
    }

    fn denied_layer(decision: GateDecision) -> GateLayer {
        match decision {
            GateDecision::Allowed(raw) => panic!("unexpectedly allowed: {:?}", raw.name),
            GateDecision::Denied(report) => report.layer,
        }
    }

    fn denied_globs(globs: &[&str]) -> Vec<String> {
        globs.iter().map(|g| g.to_string()).collect()
    }

    // --- gating matrix ------------------------------------------------------------------

    #[test]
    fn unknown_tool_is_refused_before_anything_else_is_examined() {
        let decision = gate(&denied_globs(&["bash"]), call("rm_rf", r#"{"path": "x"}"#));
        assert_eq!(denied_layer(decision), GateLayer::UnknownTool);
    }

    #[test]
    fn non_json_and_non_object_arguments_are_refused_as_invalid_args() {
        for arguments_json in ["not json", "[1,2]", "\"bash\"", "42"] {
            let decision = gate(&[], call("bash", arguments_json));
            assert_eq!(
                denied_layer(decision),
                GateLayer::InvalidArgs,
                "{arguments_json}"
            );
        }
    }

    #[test]
    fn invalid_args_beats_denied_glob_in_the_gate_order() {
        let decision = gate(
            &denied_globs(&["Bash(git push -f*)"]),
            call("bash", "not json"),
        );
        assert_eq!(denied_layer(decision), GateLayer::InvalidArgs);
    }

    #[test]
    fn real_denied_tools_entries_refuse_their_shell_commands() {
        // All 29 entries of `attempt::DENIED_TOOLS`, each against the exact evasion spelling
        // its own comment names as the form people and agents most often type — flag-first and
        // flag-last, verb-first and verb-displaced. A prior gap here left 16 of 26 globs
        // exercised only by the membership test below, never against a real shell string.
        for (glob, command) in [
            ("Bash(gh pr merge*)", "gh pr merge 123"),
            ("Bash(gh pr merge*)", "gh pr merge --squash 7"),
            ("Bash(git push --force*)", "git push --force origin main"),
            ("Bash(git push -f*)", "git push -f origin main"),
            ("Bash(git reset --hard*)", "git reset --hard HEAD~3"),
            ("Bash(git rebase*)", "git rebase main"),
            ("Bash(git checkout main*)", "git checkout main"),
            ("Bash(git branch -D*)", "git branch -D feat/x"),
            (
                "Bash(git push --delete*)",
                "git push --delete origin feat/x",
            ),
            ("Bash(git push*+*)", "git push origin +feat/x:main"),
            ("Bash(git -C*)", "git -C /elsewhere status"),
            ("Bash(git switch main*)", "git switch main"),
            (
                "Bash(gh api*merge*)",
                "gh api repos/o/r/pulls/1/merge -X PUT",
            ),
            ("Bash(git push*--force*)", "git push origin --force"),
            ("Bash(git push*--force*)", "git push origin main --force"),
            ("Bash(git push*--force*)", "git push -u origin main --force"),
            (
                "Bash(git push*--delete*)",
                "git push origin --delete feat/x",
            ),
            ("Bash(git push*:*)", "git push origin :feat/x"),
            ("Bash(git push* -f)", "git push -u origin fix -f"),
            ("Bash(git push* -f)", "git push origin -f"),
            ("Bash(git push* -f *)", "git push -u origin fix -f now"),
            ("Bash(git reset*--hard*)", "git reset HEAD~3 --hard"),
            ("Bash(git branch* -D*)", "git branch feat/x -D"),
            (
                "Bash(git branch*--delete*)",
                "git branch --delete --force feat/x",
            ),
            (
                "Bash(git*--force-with-lease*)",
                "git push origin --force-with-lease",
            ),
            ("Bash(git -c*)", "git -c x rebase"),
            ("Bash(git*update-ref*)", "git update-ref -d refs/heads/x"),
            ("Bash(git push*--mirror*)", "git push --mirror origin"),
            ("Bash(git push*--prune*)", "git push --prune origin"),
            (
                "Bash(gh api*DELETE*)",
                "gh api -X DELETE repos/o/r/git/refs/heads/x",
            ),
            // The shell-indirection trio from fix 2: none of the 26 globs above name `sh`,
            // `bash` or `eval` themselves, so a nested shell was the one gap a wider matcher
            // could not close and only a new glob could.
            ("Bash(sh -c*)", "sh -c 'git push --force origin main'"),
            ("Bash(bash -c*)", "bash -c 'gh pr merge 123'"),
            ("Bash(eval*)", "eval 'git reset --hard HEAD~3'"),
        ] {
            let decision = gate(
                &denied_globs(&[glob]),
                call(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string(),
                ),
            );
            assert_eq!(
                denied_layer(decision),
                GateLayer::DeniedGlob,
                "{glob} must refuse `{command}`"
            );
        }
    }

    #[test]
    fn substitutions_subshells_and_leading_env_assignments_no_longer_hide_the_verb() {
        // The three constructions fix 2 names: command substitution, a backtick span, and a
        // leading `NAME=value` assignment token in front of the forbidden verb. Gated against
        // the real `attempt::DENIED_TOOLS` list, not a single hand-picked glob, since the
        // point is that the *matcher* now finds these, not that some glob happens to.
        let all_denials: Vec<String> = crate::attempt::DENIED_TOOLS
            .iter()
            .map(|g| g.to_string())
            .collect();
        for command in [
            "echo $(gh pr merge 123)",
            "echo `git push --force origin main`",
            "GIT_DIR=. gh pr merge 123",
        ] {
            let decision = gate(
                &all_denials,
                call(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string(),
                ),
            );
            assert_eq!(
                denied_layer(decision),
                GateLayer::DeniedGlob,
                "`{command}` must be denied"
            );
        }
    }

    #[test]
    fn benign_commands_pass_the_real_denied_tools_entries() {
        let globs: Vec<String> = crate::attempt::DENIED_TOOLS
            .iter()
            .map(|g| g.to_string())
            .collect();
        for command in [
            "git push -u origin fix/PROJ-1-form-fields",
            "git status",
            "gh pr view 135",
            "cargo test --lib runner::tools",
            "echo done",
        ] {
            let decision = gate(
                &globs,
                call(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string(),
                ),
            );
            allowed_layer(decision);
        }
    }

    #[test]
    fn compound_shell_commands_are_matched_per_subcommand() {
        let globs = denied_globs(&["Bash(git push --force*)"]);
        for command in [
            "echo hi && git push --force origin main",
            "echo hi; git push --force origin main",
            "echo hi || git push --force origin main",
            "echo hi & git push --force origin main",
            "echo hi | git push --force origin main",
            "echo hi\ngit push --force origin main",
        ] {
            let decision = gate(
                &globs,
                call(
                    "bash",
                    &serde_json::json!({ "command": command }).to_string(),
                ),
            );
            assert_eq!(
                denied_layer(decision),
                GateLayer::DeniedGlob,
                "subcommand split must catch `{command}`"
            );
        }
    }

    #[test]
    fn shell_shaped_globs_never_touch_non_bash_tools() {
        // `Bash(git push -f*)` denies the bash tool running git, not read_file.
        let decision = gate(
            &denied_globs(&["Bash(git push -f*)"]),
            call("read_file", r#"{"path": "notes.md"}"#),
        );
        allowed_layer(decision);
    }

    #[test]
    fn write_file_is_refused_under_report_only_denials_and_left_alone_at_work() {
        // `Bash(...)` globs never apply to `write_file` — this walks the bare-name branch,
        // matched straight against `raw.name`. `write_file` is the native toolkit's own name
        // for the writer `Write`/`Edit` name on the claude-code side, so the real
        // `attempt::denied_for` list for a report-only stage must carry `write_file` itself
        // (not just `Write`/`Edit`) for this to refuse under `backend: native`.
        let review_denials = crate::attempt::denied_for(crate::rung::Stage::Review);
        let decision = gate(
            &review_denials,
            call("write_file", r#"{"path": "a.md", "content": "x"}"#),
        );
        assert_eq!(denied_layer(decision), GateLayer::DeniedGlob);

        // A worktree-writing stage's own denial set carries no `Write`/`Edit`/`write_file`
        // entry at all, so the same call stays allowed there.
        let work_denials = crate::attempt::denied_for(crate::rung::Stage::Work);
        let decision = gate(
            &work_denials,
            call("write_file", r#"{"path": "a.md", "content": "x"}"#),
        );
        allowed_layer(decision);

        // A grind-side bare glob over our own names also works, wildcards included —
        // independent of what `denied_for` happens to push.
        for glob in ["write_file", "write_*"] {
            let decision = gate(
                &denied_globs(&[glob]),
                call("write_file", r#"{"path": "a.md", "content": "x"}"#),
            );
            assert_eq!(denied_layer(decision), GateLayer::DeniedGlob, "{glob}");
        }
        let decision = gate(
            &denied_globs(&["write_*"]),
            call("read_file", r#"{"path": "a.md"}"#),
        );
        allowed_layer(decision);
    }

    // --- path resolution / escape ---------------------------------------------------------

    #[test]
    fn path_escapes_are_refused_for_path_carrying_tools_only() {
        for (name, arguments_json) in [
            ("read_file", r#"{"path": "../outside.txt"}"#),
            (
                "write_file",
                r#"{"path": "../../etc/passwd", "content": "x"}"#,
            ),
            ("read_file", r#"{"path": "a/../../b"}"#),
            ("read_file", r#"{"path": ".."}"#),
            ("read_file", r#"{"path": "/../abs"}"#),
        ] {
            let decision = gate(&[], call(name, arguments_json));
            assert_eq!(
                denied_layer(decision),
                GateLayer::PathEscape,
                "{name} {arguments_json}"
            );
        }
        // Inside-the-workdir climbs are fine, and bash has no path to escape with.
        for (name, arguments_json) in [
            ("read_file", r#"{"path": "a/../b.txt"}"#),
            ("read_file", r#"{"path": "./x"}"#),
            ("read_file", r#"{"path": "/work/sub/file"}"#),
            ("write_file", r#"{"path": "nested/dir/f", "content": "x"}"#),
            ("bash", r#"{"command": "cat ../../etc/passwd"}"#),
        ] {
            allowed_layer(gate(&[], call(name, arguments_json)));
        }
    }

    // --- truncation ------------------------------------------------------------------------

    #[test]
    fn output_at_or_under_the_cap_passes_through_untouched() {
        assert_eq!(truncate_output(""), "");
        assert_eq!(truncate_output("hello"), "hello");
        let exact = "x".repeat(MAX_TOOL_OUTPUT);
        assert_eq!(truncate_output(&exact), exact);
    }

    #[test]
    fn oversized_ascii_output_is_truncated_with_a_byte_count() {
        let s = "y".repeat(MAX_TOOL_OUTPUT + 10);
        let out = truncate_output(&s);
        assert!(out.starts_with(&"y".repeat(MAX_TOOL_OUTPUT)));
        assert!(out.ends_with(&format!(
            "\n…[truncated {} bytes]",
            s.len() - MAX_TOOL_OUTPUT
        )));
        assert!(out.len() > MAX_TOOL_OUTPUT); // suffix rides on top, POC verbatim
    }

    #[test]
    fn truncation_never_splits_a_multi_byte_character() {
        // 'é' is 2 bytes, '€' 3, '🦀' 4: fill to MAX-1 so the cut point at MAX
        // lands strictly inside the first multibyte char and the walk-back runs.
        for unit in ["é", "€", "🦀"] {
            let mut s = "a".repeat(MAX_TOOL_OUTPUT - 1);
            s.push_str(&unit.repeat(2));
            let out = truncate_output(&s);
            let (kept, suffix) = out.split_once("\n…").expect("truncation suffix present");
            assert_eq!(kept.len(), MAX_TOOL_OUTPUT - 1, "{unit}: cut walked back");
            assert!(s.starts_with(kept));
            assert!(kept.is_char_boundary(kept.len()));
            assert_eq!(
                out,
                format!("{kept}\n…[truncated {} bytes]", s.len() - kept.len())
            );
            assert!(suffix.ends_with("]"));
        }
    }

    #[test]
    fn emoji_straddling_the_cut_survives_without_panicking() {
        // 4-byte crab repeated so MAX lands mid-crab.
        let s = "🦀".repeat(MAX_TOOL_OUTPUT / 4 + 1);
        let out = truncate_output(&s);
        let kept = out.split_once("\n…").unwrap().0;
        assert!(kept.chars().all(|c| c == '🦀'));
        assert_eq!(kept.len() % 4, 0);
        assert!(out.ends_with(&format!("\n…[truncated {} bytes]", s.len() - kept.len())));
    }

    // --- execution ---------------------------------------------------------------------------

    struct TempWorkdir(PathBuf);

    impl TempWorkdir {
        fn new(tag: &str) -> Self {
            // `world::temp_dir` is the sanctioned scratch seam — `tests/topology.rs`
            // keeps `std::fs` and `std::env` out of this module.
            Self(crate::world::temp_dir(&format!("runner-tools-{tag}")))
        }
    }

    impl Drop for TempWorkdir {
        fn drop(&mut self) {
            crate::world::remove_tree(&self.0);
        }
    }

    /// Sets one environment variable for the test's duration and clears it on drop, even on
    /// panic — via `world::set_var_for_test`/`remove_var_for_test` rather than `std::env`
    /// directly, so `std::env` stays named in `world` alone (`tests/topology.rs`).
    struct EnvVarGuard(&'static str);

    impl EnvVarGuard {
        fn set(name: &'static str, value: &str) -> Self {
            crate::world::set_var_for_test(name, value);
            Self(name)
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            crate::world::remove_var_for_test(self.0);
        }
    }

    #[test]
    fn bash_never_sees_the_supervisors_provider_credentials() {
        // The concrete leak fix 3 closes: a provider key sits in the supervisor's own
        // environment so it can call the model at all, and the native `bash` tool used to
        // hand that whole environment to a shell the model drives.
        let _guard = EnvVarGuard::set("OPENROUTER_API_KEY", "sk-leak-me-not");
        let wd = TempWorkdir::new("scrub-env");
        let registry = ToolRegistry::standard(wd.0.clone());
        let outcome = registry.execute(&call(
            "bash",
            r#"{"command": "printenv OPENROUTER_API_KEY"}"#,
        ));
        assert!(
            !outcome.output.contains("sk-leak-me-not"),
            "the credential must never reach the child: {}",
            outcome.output
        );
        assert_eq!(
            outcome.exit,
            Some(1),
            "printenv exits non-zero when the named var is unset: {}",
            outcome.output
        );
    }

    #[test]
    fn bash_reports_exit_stdout_stderr_poc_verbatim() {
        let wd = TempWorkdir::new("bash");
        let registry = ToolRegistry::standard(wd.0.clone());
        let outcome = registry.execute(&call(
            "bash",
            r#"{"command": "echo out; echo err >&2; exit 3"}"#,
        ));
        assert_eq!(outcome.output, "exit: 3\nstdout:\nout\n\nstderr:\nerr\n");
        assert_eq!(outcome.exit, Some(3));
        assert!(!outcome.truncated);
    }

    #[test]
    fn bash_runs_inside_the_workdir() {
        let wd = TempWorkdir::new("cwd");
        let registry = ToolRegistry::standard(wd.0.clone());
        let outcome = registry.execute(&call("bash", r#"{"command": "pwd"}"#));
        assert!(
            outcome.output.contains(&wd.0.to_string_lossy().to_string()),
            "{}",
            outcome.output
        );
    }

    #[test]
    fn write_then_read_round_trips_through_the_workdir_root() {
        let wd = TempWorkdir::new("rw");
        let registry = ToolRegistry::standard(wd.0.clone());
        // POC-verbatim `fs::write`: no parent-directory creation, so a root-level file.
        let outcome = registry.execute(&call(
            "write_file",
            r#"{"path": "note.txt", "content": "héllo 🦀"}"#,
        ));
        assert_eq!(outcome.output, "ok");
        assert_eq!(outcome.exit, None);

        let outcome = registry.execute(&call("read_file", r#"{"path": "note.txt"}"#));
        assert_eq!(outcome.output, "héllo 🦀");

        // Leading-slash strip: an absolute-looking model path lands in the workdir.
        let outcome = registry.execute(&call("read_file", r#"{"path": "/note.txt"}"#));
        assert_eq!(outcome.output, "héllo 🦀");
    }

    #[test]
    fn tool_failures_are_string_outcomes_not_rust_errors() {
        let wd = TempWorkdir::new("fail");
        let registry = ToolRegistry::standard(wd.0.clone());
        let outcome = registry.execute(&call("read_file", r#"{"path": "missing.txt"}"#));
        assert!(
            outcome.output.starts_with("read failed: "),
            "{}",
            outcome.output
        );
        assert_eq!(outcome.exit, None);

        // Writing under a regular file fails too, still as a string.
        registry.execute(&call("write_file", r#"{"path": "plain", "content": "x"}"#));
        let outcome = registry.execute(&call(
            "write_file",
            r#"{"path": "plain/child", "content": "x"}"#,
        ));
        assert!(
            outcome.output.starts_with("write failed: "),
            "{}",
            outcome.output
        );
    }

    #[test]
    fn unknown_tool_execution_yields_a_string_outcome() {
        let wd = TempWorkdir::new("unknown");
        let registry = ToolRegistry::standard(wd.0.clone());
        let outcome = registry.execute(&call("web_search", r#"{"query": "x"}"#));
        assert_eq!(outcome.output, "unknown tool web_search");
    }

    #[test]
    fn oversized_bash_output_comes_back_truncated() {
        let wd = TempWorkdir::new("trunc");
        let registry = ToolRegistry::standard(wd.0.clone());
        let outcome = registry.execute(&call(
            "bash",
            r#"{"command": "printf 'z%.0s' $(seq 1 10000)"}"#,
        ));
        assert!(outcome.truncated);
        assert!(outcome.output.contains("…[truncated "));
        let kept = outcome.output.split_once("\n…").unwrap().0;
        assert_eq!(kept.len(), MAX_TOOL_OUTPUT);
        assert!(outcome.exit.is_some());
    }

    // --- registry surface ---------------------------------------------------------------------

    #[test]
    fn standard_registry_defines_exactly_the_poc_three_tools_with_verbatim_schemas() {
        let registry = ToolRegistry::standard(PathBuf::from("/tmp/whatever"));
        assert_eq!(registry.names(), vec!["bash", "read_file", "write_file"]);
        let defs = registry.defs();
        assert_eq!(defs.len(), 3);
        assert_eq!(defs[0].name, "bash");
        assert_eq!(
            defs[0].parameters,
            serde_json::json!({
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "required": ["command"],
            })
        );
        assert_eq!(defs[1].name, "read_file");
        assert_eq!(defs[2].name, "write_file");
        for def in &defs {
            assert!(!def.description.is_empty());
            assert_eq!(def.parameters["type"], "object");
        }
    }
}
