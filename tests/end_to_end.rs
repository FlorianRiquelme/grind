//! The whole supervisor loop, end to end, with **no `claude`, no network and no target repo**.
//!
//! What replaces *exercised by hand against a scratch repo* once no human is doing it. The
//! binary is spawned as a subprocess with a temp `$HOME`, so this covers `cli` and the argv
//! rather than only the loop — and an in-process test would need a process-global environment
//! variable, which is racy under parallel tests and `unsafe` in Rust 2024.
//!
//! **`PATH` is replaced, not prepended to.** It holds the fakes and a toolbox of symlinks to
//! the real `git` and the shell utilities the fakes need — nothing else. Grind comments on the
//! Job issue, so a fall-through to a real `gh` would mutate a real GitHub issue from a routine
//! `just verify`. Hermeticity here is structural rather than asserted. `origin` is a bare repo
//! on disk, so the fetch Dispatch performs is real git that still reaches no network.
//!
//! Every fake substitutes **raw stdout, stderr and exit code**, never a domain value. That is
//! the only fidelity that can express a truncated parse or replay a dropped connection.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const OWNER: &str = "FlorianRiquelme";
const NAME: &str = "snapper";
const BRANCH: &str = "feat/28-slice-1b-agent-surface-screensource-seam";
const ISSUE: &str = "28";
const MARKETPLACE: &str = "compound-engineering-plugin";
const PLUGIN: &str = "compound-engineering";
const VERSION: &str = "3.21.3";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

// --- the sandbox --------------------------------------------------------------------------

struct Sandbox {
    home: PathBuf,
    path: String,
    handoff_sha: String,
}

impl Sandbox {
    fn fake(&self) -> PathBuf {
        self.home.join(".fake")
    }

    fn scenario(&self, shapes: &[&str]) -> &Self {
        fs::write(self.fake().join("scenario"), shapes.join("\n") + "\n")
            .expect("write a scenario");
        self
    }

    /// The attempt at which `gh pr view` starts reporting an open PR. The world moves
    /// underneath a Run, and a PR that exists from second zero would complete it on attempt one.
    fn pr_appears_at(&self, attempt: usize) -> &Self {
        fs::write(self.fake().join("gh/pr_from"), attempt.to_string()).expect("write pr_from");
        self
    }

    /// Make `gh pr view` unreachable, the way it is in the window after a laptop wake.
    fn gh_cannot_be_reached(&self) -> &Self {
        for kind in ["pr", "rollup"] {
            fs::write(
                self.fake().join(format!("gh/{kind}.stderr")),
                "error connecting to api.github.com\n",
            )
            .expect("write a gh failure");
            fs::write(self.fake().join(format!("gh/{kind}.code")), "1").expect("write a code");
            let _ = fs::remove_file(self.fake().join(format!("gh/{kind}.stdout")));
        }
        self
    }

    fn spawn(&self, args: &[&str]) -> Child {
        Command::new(env!("CARGO_BIN_EXE_grind"))
            .args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", &self.path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn the binary")
    }

    fn run(&self, args: &[&str]) -> (String, String, Option<i32>) {
        let out = self
            .spawn(args)
            .wait_with_output()
            .expect("wait for the binary");
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.code(),
        )
    }

    fn record(&self) -> serde_json::Value {
        let runs = self.home.join(".grind/runs");
        let dir = fs::read_dir(&runs)
            .unwrap_or_else(|e| panic!("no runs under {}: {e}", runs.display()))
            .flatten()
            .map(|e| e.path())
            .next()
            .expect("exactly one Run");
        let raw = fs::read_to_string(dir.join("run.json")).expect("read the record");
        serde_json::from_str(&raw).expect("the record parses")
    }

    /// Make `gh issue comment` fail, the way GitHub being down at 04:00 does.
    fn gh_cannot_comment(&self) -> &Self {
        fs::write(self.fake().join("gh/comment.code"), "1").expect("write a comment failure");
        self
    }

    fn clone_path(&self) -> PathBuf {
        self.home.join(".grind/repos").join(OWNER).join(NAME)
    }

    /// Rewrite the Job issue's `Handoff SHA` row.
    fn handoff_becomes(&self, sha: &str) -> &Self {
        let path = self.fake().join("gh/issue.json");
        let raw = fs::read_to_string(&path).expect("the Job issue");
        fs::write(&path, raw.replace(&self.handoff_sha, sha)).expect("rewrite the Job issue");
        self
    }

    /// Rewrite the Job issue's `Anchor artifact` row.
    fn anchor_becomes(&self, path: &str) -> &Self {
        let issue = self.fake().join("gh/issue.json");
        let raw = fs::read_to_string(&issue).expect("the Job issue");
        fs::write(&issue, raw.replace("docs/plans/a-plan.md", path)).expect("rewrite it");
        self
    }

    /// Stage a record as a Run in some state with some supervisor, so `resume --all` has a
    /// roster to sort. Built from a real record, so every field the base forces at construction
    /// is present and nothing here is a hand-written shape the reader would refuse.
    fn stage(&self, template: &serde_json::Value, run_id: &str, state: &str, pid: u32) {
        let mut record = template.clone();
        record["run_id"] = serde_json::json!(run_id);
        record["state"] = serde_json::json!(state);
        record["supervisor_pid"] = serde_json::json!(pid);
        record["supervisor_identity"] = serde_json::json!(identity_of(pid));
        record["attempts"] = serde_json::json!([]);
        let dir = self.home.join(".grind/runs").join(run_id);
        fs::create_dir_all(&dir).expect("a staged run directory");
        fs::write(
            dir.join("run.json"),
            serde_json::to_string_pretty(&record).expect("serialise") + "\n",
        )
        .expect("stage a record");
    }

    /// Rewrite one Run's recorded state word, leaving its attempt list alone. The shape a
    /// resume guard keyed on the word cannot tell apart from the one it means to allow.
    /// Replace the toolbox's `ps` with one that cannot answer — busybox does not implement
    /// `-p <pid> -o lstart=`, and Grind ships as a musl static binary aimed at hosts that run
    /// it. A shape, not a mock: the binary sees a real exec with a real non-zero exit.
    fn ps_cannot_answer(&self) -> &Self {
        let ps = self.home.join(".fake/toolbox/ps");
        let _ = fs::remove_file(&ps);
        fs::write(
            &ps,
            "#!/bin/sh\necho 'ps: unrecognized option: p' >&2\nexit 127\n",
        )
        .expect("write a refusing ps");
        let mut mode = fs::metadata(&ps).expect("stat").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
        fs::set_permissions(&ps, mode).expect("make it executable");
        self
    }

    fn state_becomes(&self, run_id: &str, state: &str) {
        let path = self.home.join(".grind/runs").join(run_id).join("run.json");
        let raw = fs::read_to_string(&path).expect("a record");
        let mut record: serde_json::Value = serde_json::from_str(&raw).expect("it parses");
        record["state"] = serde_json::json!(state);
        fs::write(
            &path,
            serde_json::to_string_pretty(&record).expect("serialise") + "\n",
        )
        .expect("rewrite the state");
    }

    fn state_of(&self, run_id: &str) -> String {
        let raw = fs::read_to_string(self.home.join(".grind/runs").join(run_id).join("run.json"))
            .expect("a staged record");
        let record: serde_json::Value = serde_json::from_str(&raw).expect("it parses");
        record["state"].as_str().expect("a state").to_string()
    }

    fn run_dir(&self) -> PathBuf {
        fs::read_dir(self.home.join(".grind/runs"))
            .expect("runs")
            .flatten()
            .map(|e| e.path())
            .next()
            .expect("a Run")
    }

    /// The argv of each attempt, in order, as the fake actually received it.
    fn argvs(&self) -> Vec<Vec<String>> {
        split_log(&self.fake().join("argv.log"), "--- attempt")
    }

    /// Every `gh` invocation, in order. The assertion on what Grind writes is on what was
    /// actually posted, never on the absence of an error.
    fn gh_calls(&self) -> Vec<Vec<String>> {
        split_log(&self.fake().join("gh.log"), "--- gh")
    }
}

/// A fake's argument log, split into one `Vec` per invocation.
fn split_log(path: &Path, separator: &str) -> Vec<Vec<String>> {
    let Ok(log) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut all = Vec::new();
    let mut current = Vec::new();
    for line in log.lines() {
        if line.starts_with(separator) {
            all.push(std::mem::take(&mut current));
        } else {
            current.push(line.to_string());
        }
    }
    all.push(current);
    all.retain(|call: &Vec<String>| !call.is_empty());
    all
}

/// Every comment body Grind posted on the Job issue, in order. A body carries newlines and the
/// log is one line per argument, so everything after `--body` is rejoined.
fn comments_on_the_job_issue(box_: &Sandbox) -> Vec<String> {
    box_.gh_calls()
        .into_iter()
        .filter(|call| {
            call.first().is_some_and(|a| a == "issue")
                && call.get(1).is_some_and(|a| a == "comment")
        })
        .map(|call| {
            let at = call
                .iter()
                .position(|a| a == "--body")
                .expect("a --body argument");
            call[at + 1..].join("\n")
        })
        .collect()
}

/// A process's start stamp, exactly as `world::process_start_stamp` reads it. A pid nothing is
/// running under yields nothing, which is what *stale* looks like after a reboot.
fn identity_of(pid: u32) -> Option<String> {
    let out = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .output()
        .expect("ps");
    let stamp = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !stamp.is_empty()).then_some(stamp)
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn which(tool: &str) -> PathBuf {
    let out = Command::new("/usr/bin/env")
        .args(["sh", "-c", &format!("command -v {tool}")])
        .output()
        .expect("resolve a tool");
    let found = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(
        !found.is_empty(),
        "`{tool}` must exist to build the sandbox"
    );
    PathBuf::from(found)
}

fn sandbox(name: &str) -> Sandbox {
    let home = std::env::temp_dir().join(format!("grind-e2e-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&home);

    let fake = home.join(".fake");
    fs::create_dir_all(fake.join("gh")).expect("fake gh");
    fs::create_dir_all(fake.join("shapes")).expect("fake shapes");
    fs::create_dir_all(home.join(".grind/bin")).expect("the layout's bin");
    fs::create_dir_all(home.join(".grind/repos").join(OWNER)).expect("the layout's repos");
    fs::create_dir_all(
        home.join(".claude/plugins/cache")
            .join(MARKETPLACE)
            .join(PLUGIN)
            .join(VERSION),
    )
    .expect("the pinned plugin");
    // The ten stage skill directories (ADR-0015) — `refuse_unless_host_ready` now checks their
    // presence at every Dispatch, legacy path and ladder alike. Presence only: empty directories
    // are enough for the check, and the mega-session scenarios below never read their contents.
    for stage in [
        "plan",
        "plan-review",
        "work",
        "simplify",
        "review",
        "validate",
        "fixes",
        "ship",
        "babysit",
        "reflect",
    ] {
        let dir = home.join(".grind/skills/run").join(stage);
        fs::create_dir_all(&dir).expect("a stage skill dir");
        fs::write(dir.join("SKILL.md"), format!("# {stage}\n")).expect("a stage skill file");
    }

    // The fakes are checked in executable. A copy that loses the bit fails as *could not
    // observe* rather than as a test failure, which is the confusing shape to avoid.
    let fakes_bin = home.join(".fake/bin");
    fs::create_dir_all(&fakes_bin).expect("a fakes bin");
    for tool in ["gh", "claude"] {
        let source = repo_root().join("tests/fakes/bin").join(tool);
        assert_executable(&source);
        fs::copy(&source, fakes_bin.join(tool)).expect("copy a fake");
    }
    for shape in fs::read_dir(repo_root().join("tests/fakes/shapes"))
        .expect("shapes")
        .flatten()
    {
        let path = shape.path();
        assert_executable(&path);
        fs::copy(&path, fake.join("shapes").join(path.file_name().unwrap())).expect("copy a shape");
    }
    // `~/.grind/bin/claude` is where the layout says the binary Grind spawns lives.
    fs::copy(fakes_bin.join("claude"), home.join(".grind/bin/claude")).expect("place claude");
    // Run 2's real triple, for the rate-limit shape to replay verbatim.
    fs::copy(
        repo_root().join("tests/fixtures/run2/rate-limited.stdout.json"),
        fake.join("rate-limited.stdout.json"),
    )
    .expect("place the recorded triple");

    // A real clone, with real `git` against it — real git output is the point.
    let clone = home.join(".grind/repos").join(OWNER).join(NAME);
    fs::create_dir_all(&clone).expect("a clone");
    git(&clone, &["init", "-b", "main", "-q"]);
    git(&clone, &["config", "user.email", "run@example.invalid"]);
    git(&clone, &["config", "user.name", "Grind Test"]);
    git(&clone, &["config", "commit.gpgsign", "false"]);
    // A bare repo on disk stands in for `origin`. Dispatch fetches before it asks anything
    // about the worktree, so `origin` has to be reachable — and a local one keeps *no network*
    // structural rather than merely unexercised.
    let origin = fake.join("origin.git");
    // `-b main` explicitly: without it the bare repo takes the machine's `init.defaultBranch`,
    // its HEAD points at a branch nothing ever pushes, and `remote set-head -a` cannot resolve
    // `origin/HEAD` — which base drift reads. A green laptop and a red CI, from one config
    // difference.
    git(
        &home,
        &[
            "init",
            "--bare",
            "-q",
            "-b",
            "main",
            origin.to_str().expect("utf-8"),
        ],
    );
    git(
        &clone,
        &["remote", "add", "origin", origin.to_str().expect("utf-8")],
    );
    fs::write(clone.join("README.md"), "the human's context\n").expect("seed a file");
    git(&clone, &["add", "-A"]);
    git(&clone, &["commit", "-q", "-m", "the human stops here"]);
    let handoff_sha = git(&clone, &["rev-parse", "HEAD"]);
    git(&clone, &["push", "-q", "origin", "main"]);
    git(&clone, &["remote", "set-head", "origin", "-a"]);
    git(&clone, &["checkout", "-q", "-b", BRANCH]);
    fs::create_dir_all(clone.join("docs/plans")).expect("a plan directory");
    fs::write(clone.join("docs/plans/a-plan.md"), "# a plan\n").expect("a plan");
    git(&clone, &["add", "-A"]);
    git(&clone, &["commit", "-q", "-m", "the Run's first commit"]);
    git(&clone, &["checkout", "-q", "main"]);

    fs::write(
        fake.join("gh/issue.json"),
        serde_json::json!({
            "number": 28,
            "title": "Slice 1b: the agent surface",
            "url": format!("https://github.com/{OWNER}/{NAME}/issues/28"),
            "state": "OPEN",
            "labels": [{"name": "ready-for-agent"}],
            "body": format!(
                "| Field | Value |\n|---|---|\n\
                 | Target repo | {OWNER}/{NAME} |\n\
                 | Branch | {BRANCH} |\n\
                 | Handoff SHA | {handoff_sha} |\n\
                 | Anchor artifact | docs/plans/a-plan.md |\n\
                 | Pinned plugin version | `{PLUGIN}@{MARKETPLACE}` {VERSION} |\n\
                 | Done predicate | `just verify` is green |\n\
                 | Base branch | main |\n\
                 | Verify entrypoint | `just verify` |\n\
                 | Budget ceiling | $12.50 |\n"
            ),
        })
        .to_string(),
    )
    .expect("the Job issue");

    fs::write(
        fake.join("gh/pr.stdout"),
        serde_json::json!({
            "number": 30,
            "url": format!("https://github.com/{OWNER}/{NAME}/pull/30"),
            "state": "OPEN",
            "isDraft": false,
            "headRefName": BRANCH,
            "baseRefName": "main",
        })
        .to_string(),
    )
    .expect("the PR answer");
    fs::write(
        fake.join("gh/rollup.stdout"),
        r#"{"statusCheckRollup":[{"status":"COMPLETED","conclusion":"SUCCESS"}]}"#,
    )
    .expect("the checks answer");
    fs::write(fake.join("gh/pr_from"), "1").expect("pr_from");

    // Replaced, never prepended to: the fakes plus a toolbox holding exactly what the fakes and
    // the real `git` need. No real `gh` is reachable at all.
    let toolbox = home.join(".fake/toolbox");
    fs::create_dir_all(&toolbox).expect("a toolbox");
    for tool in ["git", "sh", "cat", "sed", "dirname", "uname", "ps", "mkdir"] {
        let real = which(tool);
        let _ = std::os::unix::fs::symlink(&real, toolbox.join(tool));
    }
    let path = format!("{}:{}", fakes_bin.display(), toolbox.display());

    Sandbox {
        home,
        path,
        handoff_sha,
    }
}

fn assert_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path).expect("a fake").permissions().mode();
    assert!(
        mode & 0o111 != 0,
        "{} must be checked in executable — a copy that loses the bit fails as \
         *could not observe* rather than as a test failure",
        path.display()
    );
}

/// Read the child's line-buffered output until it says something, then stop it. The supervisor
/// announces its sleep before taking it, so the assertion is on the announced duration rather
/// than on elapsed time — nothing can shorten a recorded 1800s from outside.
fn wait_for_line(child: &mut Child, needle: &str, patience: Duration) -> String {
    let stdout = child.stdout.take().expect("piped stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                return;
            }
        }
    });
    let deadline = Instant::now() + patience;
    let mut seen = String::new();
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                seen.push_str(&line);
                seen.push('\n');
                if line.contains(needle) {
                    return seen;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = child.kill();
    panic!("never saw `{needle}`. Output so far:\n{seen}");
}

// --- the six scenarios -----------------------------------------------------------------------

#[test]
fn scenario_a_a_real_run_shape_with_the_literal_argv_of_every_attempt() {
    // Run 1's shape: three deaths, a clean invocation that had not finished, then the promise.
    let box_ = sandbox("a-real-run-shape");
    box_.scenario(&[
        "half_json",
        "subtle_error",
        "silent",
        "success_no_done",
        "success_done",
    ])
    .pr_appears_at(5);

    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(code, Some(0), "stdout:\n{out}\nstderr:\n{err}");

    let record = box_.record();
    assert_eq!(record["state"], "completed", "stdout:\n{out}");
    let attempts = record["attempts"].as_array().expect("attempts");
    assert_eq!(attempts.len(), 5, "five attempts, as Run 1 took");

    let all_argvs = box_.argvs();
    // Six: the five real Attempts plus the one Reflect session a Completed terminal
    // observation now dispatches (idempotent, budget-exempt — it lands no `Attempt` row, but
    // the fake still logs the invocation it received).
    assert_eq!(
        all_argvs.len(),
        6,
        "the fake saw every attempt plus reflect"
    );
    let (argvs, reflect) = all_argvs.split_at(5);
    let session = &record["session_id"]
        .as_str()
        .expect("a session id")
        .to_string();

    // The first invocation opens the session id; every later one resumes the same one.
    assert!(argvs[0].contains(&"--session-id".to_string()));
    assert!(!argvs[0].contains(&"--resume".to_string()));
    for (n, argv) in argvs.iter().enumerate() {
        let at = argv
            .iter()
            .position(|a| a == "--session-id" || a == "--resume")
            .unwrap_or_else(|| panic!("attempt {} carries neither flag: {argv:?}", n + 1));
        assert_eq!(
            &argv[at + 1],
            session,
            "attempt {} used another session",
            n + 1
        );
        if n > 0 {
            assert_eq!(argv[at], "--resume", "attempt {} must resume", n + 1);
            assert!(!argv.contains(&"--session-id".to_string()));
        }
        // The denials ride every one of them.
        let denials = argv
            .iter()
            .position(|a| a == "--disallowedTools")
            .unwrap_or_else(|| panic!("attempt {} carries no denials: {argv:?}", n + 1));
        assert_eq!(
            &argv[denials + 1..],
            &[
                "Bash(gh pr merge*)",
                "Bash(git push --force*)",
                "Bash(git push -f*)",
                "Bash(git reset --hard*)",
                "Bash(git rebase*)",
                "Bash(git checkout main*)",
                "Bash(git branch -D*)",
                "Bash(git push --delete*)",
                "Bash(git push*+*)",
                "Bash(git -C*)",
                "Bash(git switch main*)",
                "Bash(gh api*merge*)",
                "Bash(git push*--force*)",
                "Bash(git push*--delete*)",
                "Bash(git push*:*)",
                "Bash(git push* -f)",
                "Bash(git push* -f *)",
                "Bash(git reset*--hard*)",
                "Bash(git branch* -D*)",
                "Bash(git branch*--delete*)",
                "Bash(git*--force-with-lease*)",
                "Bash(git -c*)",
                "Bash(git*update-ref*)",
                "Bash(git push*--mirror*)",
                "Bash(git push*--prune*)",
                "Bash(gh api*DELETE*)",
            ]
        );
        // No spend ceiling on any of them, and the Job issue still carries the row (ADR-0010).
        assert!(!argv.contains(&"--max-budget-usd".to_string()));
        assert!(argv.contains(&"bypassPermissions".to_string()));
        // A fresh Dispatch walks the ladder (ADR-0015), and a stage invocation names no
        // plugin — the pin retires the moment nothing left invokes it (unit D deletes the
        // record field and the resolution; this Run still carries both, unread by any argv).
        assert!(!argv.contains(&"--plugin-dir".to_string()));
    }

    // Reflect's own session, never the Run's: a fresh `--session-id`, no `--plugin-dir` (a
    // stage-shaped invocation never names one), and the base denials still ride it.
    assert_eq!(reflect.len(), 1);
    assert!(reflect[0].contains(&"--session-id".to_string()));
    assert!(!reflect[0].contains(&"--plugin-dir".to_string()));
    assert!(
        reflect[0].iter().any(|a| a.ends_with("-reflect")),
        "{:?}",
        reflect[0]
    );

    // Every attempt's raw landed on disk, including the ones that died and the silent one.
    for n in 1..=5 {
        let raw = box_.run_dir().join(format!("attempt-{n}.stdout.json"));
        assert!(raw.exists(), "attempt {n}'s raw stdout must be on disk");
        assert!(
            box_.run_dir()
                .join(format!("attempt-{n}.prompt.txt"))
                .exists()
        );
        assert!(
            box_.run_dir()
                .join(format!("attempt-{n}.stderr.log"))
                .exists()
        );
    }
    // Zero bytes is itself a recorded fact, not a lost one.
    let silent = fs::metadata(box_.run_dir().join("attempt-3.stdout.json")).unwrap();
    assert_eq!(silent.len(), 0);

    // `subtype` reads "success" on the attempts that died, so it is not the outcome.
    assert_eq!(attempts[1]["subtype"], "success");
    assert_eq!(attempts[1]["is_error"], true);
    assert_eq!(attempts[4]["done_promise"], true);

    assert!(out.contains("run state"), "the Handback prints:\n{out}");
}

#[test]
fn scenario_b_a_chaotic_parse_keeps_the_tail_and_the_loop_continues() {
    let box_ = sandbox("b-chaos-parse");
    box_.scenario(&["half_json", "success_done"])
        .pr_appears_at(2);

    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(code, Some(0), "{out}\n{err}");

    let record = box_.record();
    let attempts = record["attempts"].as_array().expect("attempts");
    assert_eq!(
        attempts.len(),
        2,
        "the loop continued past the unparseable response"
    );
    assert_eq!(attempts[0]["parse_ok"], false);
    assert_eq!(attempts[0]["subtype"], "unparseable-output");
    let tail = attempts[0]["result_tail"].as_str().expect("a tail");
    assert!(
        tail.contains("Connection closed mid-resp"),
        "the tail is kept: {tail}"
    );
    assert_eq!(record["state"], "completed");
}

#[test]
fn scenario_c_a_child_that_kills_itself_is_recorded_and_the_loop_re_enters() {
    // `kill -9 $$` — no chance to close stdout cleanly or set an exit code the parent chose.
    let box_ = sandbox("c-sigkilled");
    box_.scenario(&["sigkilled", "success_done"])
        .pr_appears_at(2);

    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(code, Some(0), "{out}\n{err}");

    let record = box_.record();
    let attempts = record["attempts"].as_array().expect("attempts");
    assert_eq!(
        attempts.len(),
        2,
        "a killed child costs one attempt, not the Run"
    );
    assert!(
        attempts[0]["exit_code"].is_null(),
        "the kill signal ate the exit code: {}",
        attempts[0]
    );
    assert_eq!(attempts[0]["parse_ok"], false);
    // Whatever reached the pipe before the kill landed is on disk.
    let raw = fs::read_to_string(box_.run_dir().join("attempt-1.stdout.json")).expect("the raw");
    assert!(
        raw.starts_with('{'),
        "partial bytes must survive the kill: {raw:?}"
    );
    assert_eq!(record["state"], "completed");
}

#[test]
fn scenario_d_a_rate_limit_announces_the_recorded_sleep_rather_than_burning_the_budget() {
    // Run 2's real triple. Its prose matches none of the script's phrases; only the 429
    // classifies it. Had that missed, eight attempts would have burned in under a minute
    // against a three-hour wall.
    let box_ = sandbox("d-rate-limited");
    box_.scenario(&["rate_limited"]).pr_appears_at(99);

    let mut child = box_.spawn(&["run", ISSUE]);
    // The fixture's own prose names a stated reset time (`resets 5pm (Europe/Berlin)`), so
    // decision 5's parse fires and the announced duration is no longer the fixed 1800s — it is
    // whatever hour is 5pm from wherever the test happens to run, capped at 12h. The property
    // this scenario still pins is that *something* bounded is announced and slept on, not a
    // literal figure that would make the assertion depend on the wall clock at test time.
    let seen = wait_for_line(&mut child, "rate limited", Duration::from_secs(30));
    let seconds: u64 = seen
        .split("sleeping ")
        .nth(1)
        .and_then(|s| s.split('s').next())
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no announced sleep duration:\n{seen}"));
    assert!(
        seconds > 0 && seconds <= 12 * 3600,
        "the announced sleep must be bounded: {seconds}s\n{seen}"
    );
    child.kill().expect("stop the supervisor mid-sleep");
    let _ = child.wait();

    let record = box_.record();
    assert_eq!(record["state"], "rate_limited");
    let attempts = record["attempts"].as_array().expect("attempts");
    assert_eq!(attempts.len(), 1, "it slept rather than burning attempts");
    assert_eq!(attempts[0]["rate_limited"], true);
    assert_eq!(attempts[0]["api_error_status"], "429");
    // And it did no work, so it is a Wait: it parsed, cost nothing and took one turn. Nothing
    // records that as a field — the predicate is derived from these three.
    assert_eq!(attempts[0]["parse_ok"], true);
    assert_eq!(attempts[0]["total_cost_usd"], 0.0);
    assert_eq!(attempts[0]["num_turns"], 1);
    assert_eq!(
        attempts[0]["subtype"], "success",
        "subtype is not the outcome"
    );
}

#[test]
fn scenario_e_attempts_exhausted_is_its_own_outcome_and_not_a_death() {
    let box_ = sandbox("e-exhausted");
    box_.scenario(&["subtle_error"]).pr_appears_at(99);

    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(code, Some(0), "{out}\n{err}");

    let record = box_.record();
    assert_eq!(record["state"], "exhausted", "distinct from `died`");
    // Grit's arithmetic (the ten-Attempt fullest walk plus four re-entries of headroom).
    assert_eq!(record["attempts"].as_array().expect("attempts").len(), 14);
    assert_eq!(record["attempt_budget"], 14);
}

#[test]
fn scenario_f_an_unobservable_run_pauses_before_looking_again_and_spends_no_attempt() {
    // A fault in Grind's own eyes must never cost an attempt, and three retries fired within
    // milliseconds of each other cannot span the transient this pause exists for (the window
    // after a laptop wake), so this asserts on the *announced* pause and kills the child
    // mid-sleep — exactly scenario d's technique for the 1800s rate-limit sleep. Nothing here
    // waits out the three real fifteen-second pauses it would otherwise take to walk this Run
    // all the way to `unobserved`; that terminal transition is `policy`'s own
    // `re_observation_spent_stops_as_unobserved_rather_than_as_a_death`.
    let box_ = sandbox("f-unobservable");
    box_.scenario(&["silent"]).gh_cannot_be_reached();

    let mut child = box_.spawn(&["run", ISSUE]);
    let seen = wait_for_line(
        &mut child,
        "sleeping 15s before looking again",
        Duration::from_secs(30),
    );
    assert!(
        seen.contains("could not be observed"),
        "a fault in Grind's own eyes, not a death:\n{seen}"
    );
    child.kill().expect("stop the supervisor mid-pause");
    let _ = child.wait();

    let record = box_.record();
    assert_eq!(
        record["attempts"].as_array().expect("attempts").len(),
        1,
        "re-observing must not cost attempts"
    );
}

#[test]
fn scenario_g_a_repeated_denial_with_no_progress_stops_for_a_human_and_resumes() {
    // A Run refused the same operation twice over stops instead of spending its remaining
    // budget against it — and it never spent the budget, so it re-enters where it stopped once
    // the human has cleared the obstacle. The world changed, not the number.
    let box_ = sandbox("g-blocked");
    box_.scenario(&["denied", "denied", "denied", "success_done"])
        .pr_appears_at(4);

    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(code, Some(0), "{out}\n{err}");

    let record = box_.record();
    assert_eq!(record["state"], "blocked", "stdout:\n{out}");
    let attempts = record["attempts"].as_array().expect("attempts").len();
    assert_eq!(attempts, 3, "it stopped at once rather than spending eight");
    assert!(record["attempt_budget"].as_u64().expect("a budget") > attempts as u64);
    assert!(
        out.contains("git push --force-with-lease"),
        "the Handback names what must be cleared:\n{out}"
    );

    let run_id = record["run_id"].as_str().expect("a run id").to_string();

    // The stop names the two-step repair in order: `cleared` records, `resume` spends.
    assert!(out.contains(&format!("grind cleared {run_id}")), "{out}");
    assert!(out.contains(&format!("grind resume {run_id}")), "{out}");

    // An unknown run id and an empty note both leave in the incoherent-input register.
    let (_, err, code) = box_.run(&["cleared", "nope", "the wall moved"]);
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("no Run `nope` on this host"), "{err}");
    let (_, err, code) = box_.run(&["cleared", &run_id]);
    assert_eq!(code, Some(2), "an empty note clears nothing:\n{err}");
    assert_eq!(
        box_.record()["state"],
        "blocked",
        "a refusal records nothing"
    );

    // The clearance: a multi-word unquoted note, joined from the rest of argv. It records
    // and does not spend — the state stays blocked until the hand re-enters.
    let (out, err, code) =
        box_.run(&["cleared", &run_id, "the", "deploy", "key", "was", "rotated"]);
    assert_eq!(code, Some(0), "{out}\n{err}");
    assert!(out.contains(&format!("grind resume {run_id}")), "{out}");
    let record = box_.record();
    assert_eq!(record["state"], "blocked");
    assert_eq!(
        record["clearances"][0]["note"],
        "the deploy key was rotated"
    );
    assert!(
        record["clearances"][0]["cleared_at"]
            .as_str()
            .is_some_and(|at| !at.is_empty()),
        "the row is dated"
    );

    let (out, err, code) = box_.run(&["resume", &run_id]);
    assert_eq!(code, Some(0), "{out}\n{err}");
    assert!(
        !out.contains("run already"),
        "a Blocked Run re-enters rather than short-circuiting:\n{out}"
    );
    assert_eq!(box_.record()["state"], "completed");

    // The note rode the resumed Attempt's prompt — and only Resume-mode prompts: the
    // dispatch prompt on disk stays exactly what it was (R6's safety property, pinned on
    // the files the supervisor wrote rather than on the builder alone).
    let resumed_prompt = fs::read_to_string(box_.run_dir().join("attempt-4.prompt.txt"))
        .expect("the resumed attempt's prompt");
    assert!(
        resumed_prompt.contains("Since you stopped, the human reports (recorded "),
        "{resumed_prompt}"
    );
    assert!(
        resumed_prompt.contains("the deploy key was rotated"),
        "{resumed_prompt}"
    );
    let dispatch_prompt = fs::read_to_string(box_.run_dir().join("attempt-1.prompt.txt"))
        .expect("the dispatch prompt");
    assert!(
        !dispatch_prompt.contains("the deploy key was rotated"),
        "{dispatch_prompt}"
    );

    // The note reaches the Handback and the terminal comment — the Record carries what was
    // cleared, not merely that something was.
    assert!(out.contains("the deploy key was rotated"), "{out}");
    let comments = comments_on_the_job_issue(&box_);
    let terminal = comments.last().expect("a terminal comment");
    assert!(
        terminal.contains("the deploy key was rotated"),
        "{terminal}"
    );

    // A Run that finished has no Blocker to clear anymore: the actual state is named.
    let (_, err, code) = box_.run(&["cleared", &run_id, "again"]);
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("completed"), "{err}");
}

#[test]
fn scenario_h_a_ladder_walk_completes_plan_then_triage_before_plan_review_ever_dispatches() {
    // A full ten-rung fake is disproportionate (per the plan's own escape valve): this walks
    // the first two rungs honestly — Plan, an [S] session, and Triage, a zero-token [R] pass —
    // then lets PlanReview's own stage session re-enter itself under `rung::next`'s earliest-
    // absent-return contract until the budget is spent, which this asserts deterministically
    // (no killed-mid-flight race, unlike the sleep-blocked scenarios above: nothing here
    // blocks between one rung and the next). `plan_writes_return.sh` plays Plan honestly: it
    // writes `plan-facts.json` and its own return under the stages directory the context block
    // names, exactly what a real stage session's structured return would do; every attempt
    // after it — PlanReview's, since its own shape never writes back — replays it too, which
    // is harmless (Plan's return already exists) and lets the same one shape drive the whole
    // scenario.
    let box_ = sandbox("h-ladder-walk");
    box_.scenario(&["plan_writes_return"]).pr_appears_at(99);

    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(code, Some(0), "{out}\n{err}");

    let record = box_.record();
    let run_id = record["run_id"].as_str().expect("a run id").to_string();
    assert_eq!(
        record["state"], "exhausted",
        "plan-review never writes back, so the budget is spent re-entering it:\n{out}\n{err}"
    );
    assert_eq!(record["attempts"].as_array().expect("attempts").len(), 14);

    let stages = record["stages"].as_array().expect("stages");
    assert_eq!(stages[0]["name"], "plan");
    assert_eq!(stages[0]["status"], "complete");
    assert_eq!(
        stages[0]["session_id"], record["session_id"],
        "a new record's Run-level session_id is the Plan stage's own (decision 4)"
    );
    assert_eq!(
        stages[0]["session_id"].as_str().unwrap(),
        format!("{run_id}-plan")
    );

    // Triage: zero-token, no session, costing none of Plan's or plan-review's attempts.
    assert_eq!(stages[1]["name"], "triage");
    assert_eq!(stages[1]["session_id"], "[R]");
    assert_eq!(stages[1]["status"], "complete");
    assert_eq!(stages[1]["cost_usd"], 0.0);
    assert_eq!(stages[1]["turns"], 0);

    // Every remaining row is PlanReview, re-entering **its own** session — never Plan's again,
    // the earliest-absent-return contract `rung::next` states.
    let plan_review_rows = &stages[2..];
    assert_eq!(plan_review_rows.len(), 13, "14 attempts minus Plan's one");
    for row in plan_review_rows {
        assert_eq!(row["name"], "plan-review");
        assert_eq!(
            row["session_id"].as_str().unwrap(),
            format!("{run_id}-plan-review")
        );
    }

    // Triage's own decision landed on disk.
    let decision = fs::read_to_string(
        box_.run_dir()
            .join("stages")
            .join("triage")
            .join("decision.json"),
    )
    .expect("triage's decision.json");
    assert!(decision.contains("\"tier\""), "{decision}");

    // The argv shape: Plan's session opens fresh, and every plan-review attempt after the
    // first resumes the same one rather than re-opening it.
    let argvs = box_.argvs();
    assert_eq!(
        argvs.len(),
        14,
        "one per real Attempt — the [R] pass logged none"
    );
    assert!(argvs[0].contains(&"--session-id".to_string()));
    let first_review_at = argvs[1]
        .iter()
        .position(|a| a == "--session-id" || a == "--resume")
        .expect("a session flag");
    assert_eq!(argvs[1][first_review_at], "--session-id");
    assert_eq!(
        argvs[1][first_review_at + 1],
        format!("{run_id}-plan-review")
    );
    for argv in &argvs[2..] {
        let at = argv
            .iter()
            .position(|a| a == "--session-id" || a == "--resume")
            .expect("a session flag");
        assert_eq!(
            argv[at], "--resume",
            "plan-review re-enters, never re-opens"
        );
        assert_eq!(argv[at + 1], format!("{run_id}-plan-review"));
    }
}

// --- surviving a reboot -------------------------------------------------------------------------

/// A completed Run's record, to stage cut-off and stopped Runs from.
fn a_real_record(box_: &Sandbox) -> serde_json::Value {
    box_.scenario(&["success_done"]).pr_appears_at(1);
    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(code, Some(0), "{out}\n{err}");
    let record = box_.record();
    let _ = fs::remove_dir_all(box_.home.join(".grind/runs"));
    record
}

/// A pid nothing is running under. After a reboot every recorded pid is stale by construction.
const GONE: u32 = 999_999;

#[test]
fn resume_all_re_enters_the_cut_off_and_never_the_stopped() {
    // Two records staged as cut off and two as stopped re-enter exactly two Runs.
    let box_ = sandbox("resume-all");
    let template = a_real_record(&box_);
    for (run_id, state) in [
        ("r-dispatched", "dispatched"),
        ("r-limited", "rate_limited"),
        ("r-died", "died"),
        ("r-blocked", "blocked"),
        ("r-unobserved", "unobserved"),
        ("r-uncorroborated", "uncorroborated"),
    ] {
        box_.stage(&template, run_id, state, GONE);
    }

    let (out, err, code) = box_.run(&["resume", "--all"]);
    assert_eq!(code, Some(0), "{out}\n{err}");
    for cut_off in ["r-dispatched", "r-limited", "r-died"] {
        assert!(out.contains(&format!("re-entered {cut_off}")), "{out}");
    }
    for stopped in ["r-blocked", "r-unobserved", "r-uncorroborated"] {
        assert!(
            !out.contains(stopped),
            "a stopped Run is a deliberate decision:\n{out}"
        );
    }
}

#[test]
fn resume_all_re_enters_no_run_whose_recorded_supervisor_is_alive() {
    let box_ = sandbox("resume-all-alive");
    let template = a_real_record(&box_);
    // This test process is unambiguously alive, and its start stamp matches itself.
    let alive = std::process::id();
    assert!(identity_of(alive).is_some(), "ps must answer for this pid");
    box_.stage(&template, "r-alive", "dispatched", alive);
    box_.stage(&template, "r-gone", "dispatched", GONE);

    let (out, err, code) = box_.run(&["resume", "--all"]);
    assert_eq!(code, Some(0), "{out}\n{err}");
    assert!(out.contains("re-entered r-gone"), "{out}");
    assert!(!out.contains("r-alive"), "{out}");
}

#[test]
fn a_cut_off_run_whose_worktree_is_dirty_is_skipped_and_the_skip_is_recorded() {
    // A machine that just rebooted is exactly where someone was mid-edit, and this is the one
    // path that starts an agent with nobody present. A skip rather than a refusal, because one
    // unre-enterable Run must not stop the others.
    let box_ = sandbox("resume-all-dirty");
    let template = a_real_record(&box_);
    box_.stage(&template, "r-dirty", "died", GONE);
    box_.stage(&template, "r-clean", "died", GONE);
    let worktree = PathBuf::from(template["worktree"].as_str().expect("a worktree"));
    fs::write(worktree.join("uncommitted.txt"), "work the human left\n").expect("dirty it");

    let (out, err, code) = box_.run(&["resume", "--all"]);
    assert_eq!(code, Some(0), "{out}\n{err}");
    assert!(out.contains("skipped    r-dirty"), "{out}");
    assert!(
        out.contains("skipped    r-clean"),
        "both share one worktree:\n{out}"
    );
    let log = fs::read_to_string(box_.home.join(".grind/runs/r-dirty/supervisor.log"))
        .expect("the skip is recorded");
    assert!(log.contains("skipped at boot"), "{log}");
}

#[test]
fn a_skipped_run_does_not_stop_the_others_from_re_entering() {
    let box_ = sandbox("resume-all-partial");
    let template = a_real_record(&box_);
    box_.stage(&template, "r-good", "died", GONE);
    // A record nothing can read. There is deliberately no migration read path, and a record
    // written before this build lacks fields the base forces at construction.
    let old = box_.home.join(".grind/runs/r-ancient");
    fs::create_dir_all(&old).expect("a staged run directory");
    fs::write(
        old.join("run.json"),
        r#"{"run_id":"r-ancient","state":"died","attempts":[]}"#,
    )
    .expect("stage a pre-build record");

    let (out, err, code) = box_.run(&["resume", "--all"]);
    assert_eq!(code, Some(0), "never a panic:\n{out}\n{err}");
    assert!(out.contains("re-entered r-good"), "{out}");
    assert!(out.contains("skipped    r-ancient"), "{out}");
}

#[test]
fn fan_out_is_recorded_per_attempt_and_never_cumulatively() {
    // Three Attempts of one Run, each fanning out to two subagents that both return. The Run's
    // transcript is **one append-only file** — the session id is fixed at dispatch and every
    // later Attempt resumes it — so reading the whole file on Attempt N counted Attempts 1..N,
    // and `render::fanout_totals` then summed those pairs: (2,2), (4,4), (6,6) published as
    // *12 spawned, 12 returned* for a Run that spawned six. R51 says per Attempt.
    let box_ = sandbox("fanout-per-attempt");
    box_.scenario(&["fanout", "fanout", "fanout", "success_done"])
        .pr_appears_at(4);
    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(code, Some(0), "{out}\n{err}");

    let record = box_.record();
    let attempts = record["attempts"].as_array().expect("attempts");
    let fanouts: Vec<&serde_json::Value> = attempts.iter().take(3).map(|a| &a["fanout"]).collect();
    for (n, fanout) in fanouts.iter().enumerate() {
        assert_eq!(
            fanout["present"],
            serde_json::json!([2, 2]),
            "attempt {} recorded {fanout} rather than its own two: a cumulative read would give \
             (2,2), (4,4), (6,6)",
            n + 1
        );
    }
    // And the Handback, which sums those pairs, says six rather than twelve — as does the
    // comment posted on the Job issue, which is where a wrong number leaves the host.
    assert!(
        out.contains("6 spawned, 6 returned"),
        "the Handback:\n{out}"
    );
    let posted = comments_on_the_job_issue(&box_);
    let terminal = posted.last().expect("a terminal comment");
    assert!(terminal.contains("6 spawned, 6 returned"), "{terminal}");
}

#[test]
fn resume_all_re_enters_nothing_on_a_host_whose_ps_cannot_answer() {
    // The reading `resume --all` acts on. A `ps` that cannot spawn used to yield no stamp, and
    // *no stamp* collapsed into *the supervisor is gone* — so every Run on the host read as cut
    // off and every Run was re-entered at boot, with nobody watching. The safe direction is to
    // decline, and to say so rather than skip in silence.
    let box_ = sandbox("resume-all-blind-ps");
    let template = a_real_record(&box_);
    box_.stage(&template, "r-dispatched", "dispatched", GONE);
    box_.stage(&template, "r-died", "died", GONE);
    box_.ps_cannot_answer();

    let (out, err, code) = box_.run(&["resume", "--all"]);
    assert_eq!(code, Some(0), "{out}\n{err}");
    for run_id in ["r-dispatched", "r-died"] {
        assert!(
            !out.contains(&format!("re-entered {run_id}")),
            "a blind reading must not re-enter {run_id}:\n{out}"
        );
        assert!(
            out.contains(run_id),
            "and the skip is reported rather than silent:\n{out}"
        );
    }
    assert!(out.contains("could not be read"), "{out}");
}

#[test]
fn the_supervisors_resume_all_spawns_outlive_it_and_it_exits_on_what_it_started() {
    // `resume --all` reports which Runs it started and exits on that, never on any Run's
    // verdict. The children keep going after it is gone.
    let box_ = sandbox("resume-all-detached");
    let template = a_real_record(&box_);
    box_.stage(&template, "r-detached", "died", GONE);
    box_.scenario(&["success_done"]).pr_appears_at(1);

    let started = Instant::now();
    let (out, err, code) = box_.run(&["resume", "--all"]);
    assert_eq!(code, Some(0), "{out}\n{err}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "it spawns and exits rather than waiting on a Run"
    );
    assert!(out.contains("re-entered r-detached"), "{out}");
    assert!(
        !out.contains("Verdict"),
        "it never reports what a Run concluded:\n{out}"
    );

    // The child is still alive after its parent is gone, and finishes the Run.
    let deadline = Instant::now() + Duration::from_secs(30);
    while box_.state_of("r-detached") != "completed" {
        assert!(
            Instant::now() < deadline,
            "the spawned supervisor must outlive `resume --all`; it left the Run at {}",
            box_.state_of("r-detached")
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn resume_all_is_not_parsed_as_a_run_id_named_all() {
    let box_ = sandbox("resume-all-arm");
    let (out, err, code) = box_.run(&["resume", "--all"]);
    assert_eq!(code, Some(0), "{out}\n{err}");
    assert!(
        !out.contains("no Run `--all`") && !err.contains("--all"),
        "slice patterns match by position, so the generic arm would bind it:\n{out}\n{err}"
    );
    assert!(out.contains("no Run on this host was cut off"), "{out}");
}

// --- the exit code reports observability, never health ----------------------------------------

/// Dispatch a Run that stops short of completion, and hand back its run id.
fn a_run_that_did_not_finish(box_: &Sandbox) -> String {
    box_.scenario(&["subtle_error"]).pr_appears_at(99);
    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(code, Some(0), "{out}\n{err}");
    box_.record()["run_id"]
        .as_str()
        .expect("a run id")
        .to_string()
}

#[test]
fn an_unhealthy_but_fully_observed_run_exits_zero() {
    // The idiom every CLI has ever followed is *non-zero means bad*, and following it here is
    // how Grind grows a gate through the back door. This Run is exhausted with no PR — as
    // unhealthy as it gets — and status answered, so status exits zero.
    let box_ = sandbox("status-unhealthy");
    let run_id = a_run_that_did_not_finish(&box_);

    let (out, err, code) = box_.run(&["status", &run_id]);
    assert_eq!(
        code,
        Some(0),
        "an answered question is a zero:\n{out}\n{err}"
    );
    assert!(
        out.contains("exhausted"),
        "and it is plainly unhealthy:\n{out}"
    );
    assert!(out.contains("incomplete"), "{out}");
}

#[test]
fn a_run_whose_signals_could_not_be_observed_exits_non_zero() {
    let box_ = sandbox("status-blind");
    let run_id = a_run_that_did_not_finish(&box_);
    box_.gh_cannot_be_reached();

    let (out, err, code) = box_.run(&["status", &run_id]);
    assert_ne!(
        code,
        Some(0),
        "a question it could not answer:\n{out}\n{err}"
    );
    assert_ne!(
        code,
        Some(2),
        "and not the incoherent-input register either"
    );
    // The blind signals render as could-not-observe rather than as facts.
    assert!(out.contains("unobserved"), "{out}");
}

#[test]
fn bare_status_prints_the_roster_and_never_a_single_runs_view() {
    let box_ = sandbox("status-roster");
    let run_id = a_run_that_did_not_finish(&box_);

    let (out, err, code) = box_.run(&["status"]);
    assert_eq!(code, Some(0), "{out}\n{err}");
    assert!(out.contains("this host only"), "{out}");
    assert!(out.contains(&run_id), "the roster lists it:\n{out}");
    // Nothing from the single-Run view: a bare status that resolves to one Run is Grind
    // selecting, and the repair "pick the one in flight" would pick a zombie.
    assert!(!out.contains("last words"), "{out}");
    assert!(!out.contains("furthest stage"), "{out}");
}

#[test]
fn status_reads_and_never_writes() {
    // Watching a Run to be reassured must not destroy the one field nothing can rebuild.
    let box_ = sandbox("status-read-only");
    let run_id = a_run_that_did_not_finish(&box_);
    let record = box_.run_dir().join("run.json");
    let before = fs::read_to_string(&record).expect("the record");

    for _ in 0..3 {
        let (_, _, code) = box_.run(&["status", &run_id]);
        assert_eq!(code, Some(0));
    }
    let after = fs::read_to_string(&record).expect("the record");
    assert_eq!(before, after, "status must leave the record byte-identical");
    let parsed: serde_json::Value = serde_json::from_str(&after).unwrap();
    assert_eq!(parsed["attempts"].as_array().expect("attempts").len(), 14);
}

#[test]
fn resume_on_a_completed_run_prints_the_handback_and_starts_nothing() {
    let box_ = sandbox("resume-completed");
    box_.scenario(&["success_done"]).pr_appears_at(1);
    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(code, Some(0), "{out}\n{err}");
    let run_id = box_.record()["run_id"]
        .as_str()
        .expect("a run id")
        .to_string();
    let attempts_before = box_.record()["attempts"]
        .as_array()
        .expect("attempts")
        .len();

    let (out, err, code) = box_.run(&["resume", &run_id]);
    assert_eq!(code, Some(0), "{out}\n{err}");
    assert!(out.contains("run already completed"), "{out}");
    assert!(
        out.contains("run state"),
        "the Handback, not a re-entry:\n{out}"
    );
    assert_eq!(
        box_.record()["attempts"]
            .as_array()
            .expect("attempts")
            .len(),
        attempts_before,
        "a mistyped command must not restart finished work"
    );
}

#[test]
fn resume_on_an_exhausted_run_prints_the_handback_and_starts_nothing() {
    // `supervise` attempts before it ever consults `policy::next`, so without this guard a
    // resume of an exhausted Run would spend one more attempt against its recorded budget.
    // The Handback names the state it actually found, so an exhausted Run reads as
    // exhausted rather than borrowing the word `completed` from its sibling short-circuit.
    let box_ = sandbox("resume-exhausted");
    let run_id = a_run_that_did_not_finish(&box_);
    let record = box_.record();
    assert_eq!(record["state"], "exhausted");
    let attempts_before = record["attempts"].as_array().expect("attempts").len();
    assert_eq!(attempts_before, 14);

    let (out, err, code) = box_.run(&["resume", &run_id]);
    assert_eq!(code, Some(0), "{out}\n{err}");
    assert!(out.contains("run already exhausted"), "{out}");
    assert!(
        out.contains("run state"),
        "the Handback, not a re-entry:\n{out}"
    );
    assert_eq!(
        box_.record()["attempts"]
            .as_array()
            .expect("attempts")
            .len(),
        attempts_before,
        "resuming an exhausted Run must not spend one more attempt against its recorded budget"
    );
}

#[test]
fn resume_at_the_attempt_budget_starts_nothing_whatever_the_state_word_says() {
    // `Uncorroborated` and `Unobserved` are deliberately resumable: neither stopped because the
    // budget ran out. But a Run can land one *at* its recorded budget — and `supervise` runs
    // `run_one_attempt` before it ever consults `policy::next`, so a guard keyed on the state
    // word walks that Run into one more attempt with no policy check standing in front of it,
    // stops in the same state, and does it again on the next resume. A recorded Attempt in this
    // project costs $7–$37.
    let box_ = sandbox("resume-at-budget");
    let run_id = a_run_that_did_not_finish(&box_);
    let attempts_before = box_.record()["attempts"]
        .as_array()
        .expect("attempts")
        .len();
    assert_eq!(attempts_before, 14);

    for state in ["uncorroborated", "unobserved"] {
        box_.state_becomes(&run_id, state);
        let (out, err, code) = box_.run(&["resume", &run_id]);
        assert_eq!(code, Some(0), "{out}\n{err}");
        assert!(out.contains(&format!("run already {state}")), "{out}");
        assert!(
            out.contains("run state"),
            "the Handback, not a re-entry:\n{out}"
        );
        assert_eq!(
            box_.record()["attempts"]
                .as_array()
                .expect("attempts")
                .len(),
            attempts_before,
            "a Run at its recorded budget must not spend one more attempt because its state word is resumable"
        );
    }
}

// --- the sandbox's own guarantees ------------------------------------------------------------

#[test]
fn an_unimplemented_gh_subcommand_fails_loudly_rather_than_escaping() {
    // Dispatch removes a label and comments on a real Job issue. A fall-through would mutate
    // GitHub from a routine `just verify`, so the fake refuses anything it does not implement.
    let box_ = sandbox("gh-loud");
    let out = Command::new("gh")
        .args(["pr", "merge", "30"])
        .env_clear()
        .env("HOME", &box_.home)
        .env("PATH", &box_.path)
        .output()
        .expect("the fake gh");
    assert_eq!(out.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unimplemented"));
}

#[test]
fn no_real_gh_is_reachable_from_the_sandbox() {
    let box_ = sandbox("no-real-gh");
    let out = Command::new("/usr/bin/env")
        .args(["sh", "-c", "command -v gh"])
        .env_clear()
        .env("HOME", &box_.home)
        .env("PATH", &box_.path)
        .output()
        .expect("resolve gh");
    let resolved = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(
        resolved.starts_with(box_.home.to_str().expect("a utf-8 home")),
        "`gh` must resolve inside the sandbox, not to a real one: {resolved}"
    );
}

#[test]
fn a_dirty_worktree_refuses_and_nothing_is_dispatched_onto_it() {
    let box_ = sandbox("dirty");
    box_.scenario(&["success_done"]);
    let worktree = box_
        .home
        .join(".grind/repos")
        .join(OWNER)
        .join(NAME)
        .join(".claude/worktrees")
        .join(format!("grind-{}", BRANCH.replace('/', "-")));
    // Dispatch once so the worktree exists, then dirty it and dispatch again.
    let _ = box_.run(&["run", ISSUE]);
    fs::write(worktree.join("uncommitted.txt"), "work the human left\n").expect("dirty it");
    let _ = fs::remove_dir_all(box_.home.join(".grind/runs"));

    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(
        code,
        Some(2),
        "a refusal is incoherent input:\n{out}\n{err}"
    );
    assert!(err.contains("dirty"), "{err}");
    assert!(
        !box_.home.join(".grind/runs").exists(),
        "nothing is dispatched onto a dirty worktree"
    );
}

#[test]
fn a_complete_run_issues_no_gh_issue_edit_at_all() {
    // Grind adds and never classifies (ADR-0012). `ready-for-agent` is one of the five canonical
    // triage roles, so removing it erased a triage fact to record a queue fact.
    let box_ = sandbox("no-issue-edit");
    box_.scenario(&["success_done"]).pr_appears_at(1);
    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(code, Some(0), "{out}\n{err}");

    let calls = box_.gh_calls();
    assert!(!calls.is_empty(), "the fake saw `gh` at all");
    for call in &calls {
        assert_ne!(
            call.first()
                .map(String::as_str)
                .zip(call.get(1).map(String::as_str)),
            Some(("issue", "edit")),
            "no path applies or removes a label: {call:?}"
        );
        for classifying in [
            "--add-label",
            "--remove-label",
            "--add-assignee",
            "--remove-assignee",
            "--milestone",
            "--project",
        ] {
            assert!(
                !call.iter().any(|a| a == classifying),
                "`{classifying}` reached `gh`: {call:?}"
            );
        }
    }
}

#[test]
fn the_dispatch_comment_still_carries_the_run_id_and_the_hostname() {
    // Half of the only off-host surface leg 1 has, kept byte-identical.
    let box_ = sandbox("dispatch-comment");
    box_.scenario(&["success_done"]).pr_appears_at(1);
    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(code, Some(0), "{out}\n{err}");

    let record = box_.record();
    let run_id = record["run_id"].as_str().expect("a run id").to_string();
    let host = record["hostname"].as_str().expect("a hostname").to_string();
    let posted = comments_on_the_job_issue(&box_);
    let body = posted.first().expect("the dispatch comment is the first");
    assert!(body.contains(&run_id), "{body}");
    assert!(body.contains(&host), "{body}");
    assert!(body.contains("Dispatched as Run"), "{body}");
    assert!(body.contains("~/.grind/runs/"), "{body}");
}

#[test]
fn a_completed_run_leaves_a_supervisor_log_beside_its_record() {
    // What the supervisor said is the only account of a Run between the dispatch comment and a
    // terminal state, and it died with the terminal it was said to.
    let box_ = sandbox("supervisor-log");
    box_.scenario(&["success_done"]).pr_appears_at(1);
    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(code, Some(0), "{out}\n{err}");

    let log = fs::read_to_string(box_.run_dir().join("supervisor.log")).expect("a supervisor log");
    assert!(log.contains("attempt 1 (dispatch)"), "{log}");
    assert!(log.contains("plugin pinned to"), "{log}");
    for said in log.lines() {
        assert!(
            out.contains(said),
            "the log carries the lines that reached stdout; `{said}` did not:\n{out}"
        );
    }
}

#[test]
fn the_supervisor_log_is_appended_across_a_resume_rather_than_truncated() {
    let box_ = sandbox("supervisor-log-append");
    box_.scenario(&["denied", "denied", "denied", "success_done"])
        .pr_appears_at(4);
    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(code, Some(0), "{out}\n{err}");
    let log = box_.run_dir().join("supervisor.log");
    let before = fs::read_to_string(&log).expect("a supervisor log");

    let run_id = box_.record()["run_id"]
        .as_str()
        .expect("a run id")
        .to_string();
    let (out, err, code) = box_.run(&["resume", &run_id]);
    assert_eq!(code, Some(0), "{out}\n{err}");
    let after = fs::read_to_string(&log).expect("a supervisor log");
    assert!(
        after.starts_with(&before),
        "a resume appends rather than truncating:\n{after}"
    );
    assert!(after.len() > before.len(), "and it wrote something new");
}

#[test]
fn a_run_whose_log_cannot_be_written_still_exits_on_its_real_verdict() {
    // A log that cannot be written is not worth abandoning a Run over.
    let box_ = sandbox("supervisor-log-unwritable");
    box_.scenario(&["success_done"]).pr_appears_at(1);
    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(code, Some(0), "{out}\n{err}");

    // Replace the log with a directory, which nothing can append a line to, and re-enter.
    let log = box_.run_dir().join("supervisor.log");
    fs::remove_file(&log).expect("drop the log");
    fs::create_dir(&log).expect("put something unwritable in its place");
    let run_id = box_.record()["run_id"]
        .as_str()
        .expect("a run id")
        .to_string();
    let (out, err, code) = box_.run(&["resume", &run_id]);
    assert_eq!(code, Some(0), "{out}\n{err}");
    assert!(out.contains("run already completed"), "{out}");
}

#[test]
fn a_completed_run_posts_exactly_one_terminal_comment_on_the_job_issue() {
    // Everything only the supervisor knows survives the host. Between the dispatch comment and
    // nothing, the human's only instrument was SSH.
    let box_ = sandbox("terminal-comment");
    box_.scenario(&["success_done"]).pr_appears_at(1);
    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(code, Some(0), "{out}\n{err}");

    let posted = comments_on_the_job_issue(&box_);
    assert_eq!(
        posted.len(),
        2,
        "the dispatch comment and one more: {posted:?}"
    );
    let terminal = &posted[1];
    let record = box_.record();
    assert!(
        terminal.contains(record["run_id"].as_str().expect("a run id")),
        "{terminal}"
    );
    assert!(
        terminal.contains(record["hostname"].as_str().expect("a host")),
        "{terminal}"
    );
    assert!(terminal.contains("completed"), "{terminal}");
    assert!(terminal.contains("| attempts |"), "{terminal}");
    assert!(terminal.contains("| spend |"), "{terminal}");
    assert!(terminal.contains("| run state |"), "{terminal}");
    assert!(terminal.contains("verify contract present"), "{terminal}");
    assert!(terminal.contains("verify contract missing"), "{terminal}");
}

#[test]
fn a_blocked_run_resumed_to_completion_posts_two_terminal_comments() {
    // Append, never edit. A Run that reaches a terminal state twice leaves two comments, and
    // two comments are the honest account.
    let box_ = sandbox("two-terminal-comments");
    box_.scenario(&["denied", "denied", "denied", "success_done"])
        .pr_appears_at(4);
    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(code, Some(0), "{out}\n{err}");
    assert_eq!(box_.record()["state"], "blocked");
    assert_eq!(comments_on_the_job_issue(&box_).len(), 2);

    let run_id = box_.record()["run_id"]
        .as_str()
        .expect("a run id")
        .to_string();
    let (out, err, code) = box_.run(&["resume", &run_id]);
    assert_eq!(code, Some(0), "{out}\n{err}");
    assert_eq!(box_.record()["state"], "completed");
    let posted = comments_on_the_job_issue(&box_);
    assert_eq!(posted.len(), 3, "dispatch, blocked, completed: {posted:?}");
    assert!(posted[2].contains("completed"), "{}", posted[2]);
}

#[test]
fn a_gh_that_fails_on_issue_comment_still_exits_on_the_runs_real_verdict() {
    // Best-effort. A Run that finished must not become `unobserved` because GitHub was down.
    let box_ = sandbox("comment-fails");
    box_.scenario(&["success_done"])
        .pr_appears_at(1)
        .gh_cannot_comment();

    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(code, Some(0), "the Run's real verdict:\n{out}\n{err}");
    assert_eq!(box_.record()["state"], "completed");
    assert!(
        out.contains("could not post the terminal comment"),
        "logged, never raised:\n{out}"
    );
}

#[test]
fn a_job_whose_anchor_artifact_is_not_on_disk_refuses() {
    // A Run handed a path to nothing invents requirements, satisfies them, and opens a green PR.
    let box_ = sandbox("anchor-absent");
    box_.scenario(&["success_done"])
        .anchor_becomes("docs/plans/a-plan-that-was-never-written.md");

    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(
        code,
        Some(2),
        "a refusal is incoherent input:\n{out}\n{err}"
    );
    assert!(
        err.contains("docs/plans/a-plan-that-was-never-written.md"),
        "the refusal names the path:\n{err}"
    );
    let lowered = err.to_lowercase();
    for quality in ["bad", "invalid", "wrong", "reject"] {
        assert!(!lowered.contains(quality), "no quality word:\n{err}");
    }
    assert!(!box_.home.join(".grind/runs").exists());
}

#[test]
fn an_anchor_artifact_that_is_present_but_empty_proceeds() {
    // Presence, never shape. An admission check must not arrive through the back door of an
    // admission rule, so nothing here reads R-IDs or a readiness field.
    let box_ = sandbox("anchor-empty");
    let clone = box_.clone_path();
    git(&clone, &["checkout", "-q", BRANCH]);
    fs::write(clone.join("docs/plans/empty.md"), "").expect("an empty Anchor");
    git(&clone, &["add", "-A"]);
    git(
        &clone,
        &["commit", "-q", "-m", "an Anchor with nothing in it"],
    );
    git(&clone, &["checkout", "-q", "main"]);

    box_.scenario(&["success_done"])
        .anchor_becomes("docs/plans/empty.md")
        .pr_appears_at(1);

    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(code, Some(0), "{out}\n{err}");
    assert_eq!(box_.record()["state"], "completed");
}

#[test]
fn a_worktree_behind_the_handoff_sha_refuses_at_second_zero() {
    // Run 2's opening condition. The string comparison this replaces printed the same note here
    // as it printed on a worktree that was harmlessly ahead, and the Run proceeded — the signer
    // outage, the denied force-push and five hours of `pr: null` are all downstream of it.
    let box_ = sandbox("behind-the-handoff");
    box_.scenario(&["success_done"]);

    // A commit the branch does not have, reachable as an object and nothing else.
    let clone = box_.clone_path();
    git(&clone, &["checkout", "-q", BRANCH]);
    fs::write(clone.join("docs/plans/later.md"), "# the human moved on\n").expect("a later plan");
    git(&clone, &["add", "-A"]);
    git(&clone, &["commit", "-q", "-m", "the human moved on"]);
    let ahead = git(&clone, &["rev-parse", "HEAD"]);
    git(&clone, &["reset", "--hard", "-q", "HEAD~1"]);
    git(&clone, &["checkout", "-q", "main"]);
    box_.handoff_becomes(&ahead);

    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(
        code,
        Some(2),
        "a refusal is incoherent input:\n{out}\n{err}"
    );
    assert!(
        err.contains("fast-forward"),
        "the one refusal that names its repair:\n{err}"
    );
    assert!(
        !box_.home.join(".grind/runs").exists(),
        "nothing is dispatched onto a worktree that does not contain the Handoff SHA"
    );
}

#[test]
fn a_handoff_sha_off_this_worktrees_history_refuses() {
    let box_ = sandbox("handoff-elsewhere");
    box_.scenario(&["success_done"]);

    // An unrelated root commit: neither an ancestor nor a descendant of the branch.
    let clone = box_.clone_path();
    git(&clone, &["checkout", "-q", "--orphan", "elsewhere"]);
    fs::write(clone.join("README.md"), "another history entirely\n").expect("seed it");
    git(&clone, &["add", "-A"]);
    git(&clone, &["commit", "-q", "-m", "elsewhere"]);
    let elsewhere = git(&clone, &["rev-parse", "HEAD"]);
    git(&clone, &["checkout", "-q", "main"]);
    box_.handoff_becomes(&elsewhere);

    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(code, Some(2), "{out}\n{err}");
    assert!(err.contains("not in the history"), "{err}");
    assert!(
        !err.contains("fast-forward"),
        "there is nothing to fast-forward to:\n{err}"
    );
}

#[test]
fn the_handoff_sha_the_job_names_is_what_commits_are_counted_from() {
    let box_ = sandbox("handoff");
    assert_eq!(box_.handoff_sha.len(), 40);
    box_.scenario(&["success_done"]).pr_appears_at(1);
    let (out, err, code) = box_.run(&["run", ISSUE]);
    assert_eq!(code, Some(0), "{out}\n{err}");
    assert_eq!(box_.record()["job"]["handoff_sha"], box_.handoff_sha);
    assert!(
        out.contains("commits ahead     1"),
        "one commit in front of the SHA:\n{out}"
    );
}
