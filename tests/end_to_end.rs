//! The whole supervisor loop, end to end, with **no `claude`, no network and no target repo**.
//!
//! What replaces *exercised by hand against a scratch repo* once no human is doing it. The
//! binary is spawned as a subprocess with a temp `$HOME`, so this covers `cli` and the argv
//! rather than only the loop — and an in-process test would need a process-global environment
//! variable, which is racy under parallel tests and `unsafe` in Rust 2024.
//!
//! **`PATH` is replaced, not prepended to.** It holds the fakes and a toolbox of symlinks to
//! the real `git` and the shell utilities the fakes need — nothing else. Dispatch removes a
//! label and comments on the Job issue, so a fall-through to a real `gh` would mutate a real
//! GitHub issue from a routine `just verify`. Hermeticity here is structural rather than
//! asserted.
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
        let Ok(log) = fs::read_to_string(self.fake().join("argv.log")) else {
            return Vec::new();
        };
        let mut all = Vec::new();
        let mut current = Vec::new();
        for line in log.lines() {
            if line.starts_with("--- attempt") {
                all.push(std::mem::take(&mut current));
            } else {
                current.push(line.to_string());
            }
        }
        all
    }
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
    git(
        &clone,
        &[
            "remote",
            "add",
            "origin",
            &format!("git@github.com:{OWNER}/{NAME}.git"),
        ],
    );
    fs::write(clone.join("README.md"), "the human's context\n").expect("seed a file");
    git(&clone, &["add", "-A"]);
    git(&clone, &["commit", "-q", "-m", "the human stops here"]);
    let handoff_sha = git(&clone, &["rev-parse", "HEAD"]);
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
    for tool in ["git", "sh", "cat", "sed", "dirname", "uname", "ps"] {
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

    let argvs = box_.argvs();
    assert_eq!(argvs.len(), 5, "the fake saw every attempt");
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
            ]
        );
        // Fixed at dispatch and read from the record on every attempt.
        assert!(
            argv.windows(2)
                .any(|w| w[0] == "--max-budget-usd" && w[1] == "12.50")
        );
        assert!(argv.contains(&"bypassPermissions".to_string()));
        assert!(
            argv.windows(2)
                .any(|w| w[0] == "--plugin-dir" && w[1].ends_with(VERSION))
        );
    }

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
    // The assertion is on the **announced** duration: nothing can shorten a recorded 1800s
    // from outside, because `$HOME` is the only variable and the field table has no row for it.
    let seen = wait_for_line(&mut child, "rate limited", Duration::from_secs(30));
    assert!(
        seen.contains("sleeping 1800s"),
        "the recorded limit sleep:\n{seen}"
    );
    child.kill().expect("stop the supervisor mid-sleep");
    let _ = child.wait();

    let record = box_.record();
    assert_eq!(record["state"], "rate_limited");
    let attempts = record["attempts"].as_array().expect("attempts");
    assert_eq!(attempts.len(), 1, "it slept rather than burning attempts");
    assert_eq!(attempts[0]["rate_limited"], true);
    assert_eq!(attempts[0]["api_error_status"], "429");
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
    assert_eq!(record["attempts"].as_array().expect("attempts").len(), 8);
    assert_eq!(record["attempt_budget"], 8);
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
    assert_eq!(parsed["attempts"].as_array().expect("attempts").len(), 8);
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
    // resume of an exhausted Run would spend a ninth attempt against a recorded budget of
    // eight. The Handback names the state it actually found, so an exhausted Run reads as
    // exhausted rather than borrowing the word `completed` from its sibling short-circuit.
    let box_ = sandbox("resume-exhausted");
    let run_id = a_run_that_did_not_finish(&box_);
    let record = box_.record();
    assert_eq!(record["state"], "exhausted");
    let attempts_before = record["attempts"].as_array().expect("attempts").len();
    assert_eq!(attempts_before, 8);

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
        "resuming an exhausted Run must not spend a ninth attempt against a recorded budget of eight"
    );
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
