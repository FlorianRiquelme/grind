//! `OmpAdapter::run` against a real child (#197): the argv it builds, the transcript it
//! harvests, and the fallback arm that exists because the harness is inconsistent.
//!
//! `src/omp.rs`'s own unit tests are all pure helpers over literal JSONL. Nothing there
//! constructs an adapter or spawns anything, so the half of the module that talks to a process
//! — the argv, `harvest` and its `strayed_after` fallback, the pre-run line count — was green
//! by never being run. Two of the last three omp commits were cost-channel corrections found
//! by reading rather than by a test failing; #194 named this fake a prerequisite for trusting
//! any change to that path.
//!
//! The seam is the binary path, and it is the only seam here — `RunRecord.omp_bin` is
//! snapshotted per Run (ADR-0017), so `tests/fakes/bin/omp` needs no injection, exactly as
//! `tests/fakes/bin/claude` needs none. Only a real process replays a real exit code the
//! parent did not choose, a real separate stderr file, and a session file the harness wrote
//! where *it* felt like writing it rather than where the flag said.
//!
//! This adds tests and a fake and changes no adapter behaviour. Unlike `tests/end_to_end.rs`,
//! which spawns the binary and can hand it `.env("HOME", …)`, the adapter is driven in-process
//! here, so `strayed_after`'s `world::home()` reads *this* process's `$HOME`. That is a
//! process-global, so every test in this file takes [`ENV_LOCK`] for its whole body — and it is
//! set for every test, not only the two that read it, because the alternative is a stray walk
//! of the developer's own `~/.omp/agent/sessions/`.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use grind::attempt::{self, Attempt, Mode, Transcript};
use grind::claude;
use grind::job::Job;
use grind::observe::{Observed, Reason};
use grind::rung;
use grind::runner::{self, ClassRoutes, ModelClass, OmpAdapter, RunSpec, StageModel, StageRunner};

/// `$HOME` is process-global; the whole file serialises on it.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// The stable per-(run, stage) session identity the adapter names its session directory after.
const SESSION_ID: &str = "omp-fake-work";

/// The filename `flat_clean.sh` writes, and the `<timestamp>_<uuid>` shape `resume_suffix`
/// splits on.
const FLAT_FILE: &str = "2026-01-02T03-04-05-000Z_11111111-2222-4333-8444-555555555555.jsonl";
const FLAT_SUFFIX: &str = "11111111-2222-4333-8444-555555555555";

/// The filename `strayed.sh` writes into the encoded-cwd bucket instead.
const STRAY_FILE: &str = "2026-01-02T03-04-05-000Z_99999999-8888-4777-8666-555555555555.jsonl";

/// The snapshotted binary path this Run is pinned to — the whole seam.
fn fake_omp() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fakes/bin/omp")
}

/// One test's world: a temp `$HOME` holding the Run directory, the worktree the child runs in,
/// and the fake's own scratch. Dropping it releases the lock; the tree is left for a post-mortem
/// and cleared by the next run of the same test.
struct Sandbox {
    _lock: MutexGuard<'static, ()>,
    root: PathBuf,
    run_dir: PathBuf,
    cwd: PathBuf,
}

impl Sandbox {
    /// Take the lock, build an empty tree, lay down the scenario the fake reads its shapes
    /// from, and point `$HOME` at it.
    fn new(name: &str, shapes: &[&str]) -> Sandbox {
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root =
            std::env::temp_dir().join(format!("grind-omp-fake-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let run_dir = root.join(".grind/runs/omp-fake-run");
        let cwd = root.join("worktree");
        std::fs::create_dir_all(run_dir.join("fake")).expect("the Run directory");
        std::fs::create_dir_all(&cwd).expect("the worktree");
        std::fs::write(run_dir.join("fake/scenario"), shapes.join("\n") + "\n")
            .expect("write the scenario");
        unsafe { std::env::set_var("HOME", &root) };
        Sandbox {
            _lock: lock,
            root,
            run_dir,
            cwd,
        }
    }

    /// Where the adapter puts this stage's own session directory, `run_dir/sessions/<sid>`.
    fn stage_dir(&self) -> PathBuf {
        self.run_dir.join("sessions").join(SESSION_ID)
    }

    /// What the adapter passes as `--session-dir`, trailing slash and all.
    fn session_flag(&self) -> String {
        format!("{}/", self.stage_dir().display())
    }

    /// The tree `strayed_after` walks when `--session-dir` was ignored.
    fn stray_bucket(&self) -> PathBuf {
        self.root.join(".omp/agent/sessions")
    }

    /// The argv one attempt was handed, as the fake logged it — `argv[1..]`, since
    /// `world::spawn_recorded` splits the program off before spawning.
    fn argv(&self, attempt_n: usize) -> Vec<String> {
        let log = std::fs::read_to_string(self.run_dir.join("fake/argv.log"))
            .expect("the fake logged an argv");
        let mut current: Vec<String> = Vec::new();
        for line in log.lines() {
            match line.strip_prefix("--- attempt ") {
                Some(n) if n.trim() == attempt_n.to_string() => return current,
                Some(_) => current.clear(),
                None => current.push(line.to_string()),
            }
        }
        panic!("the fake never logged attempt {attempt_n}:\n{log}");
    }

    /// The copy `harvest` left in the Run's evidence tree, by name.
    fn harvested(&self, filename: &str) -> String {
        std::fs::read_to_string(self.stage_dir().join(filename))
            .unwrap_or_else(|e| panic!("no harvested copy of {filename}: {e}"))
    }
}

/// One attempt through the real seam, built the way `supervisor.rs` builds a stage attempt and
/// `tests/sse_native.rs` already mirrors: a literal Job, `claude::stage_invocation` for the
/// prompt and mode, `attempt::denied_for` for the globs.
fn drive(
    sandbox: &Sandbox,
    attempt_n: usize,
    mode: Mode,
    model: &StageModel,
    fast_model: Option<&str>,
    strong_model: Option<&str>,
) -> Attempt {
    let job = Job {
        issue: 197,
        url: "https://github.com/FlorianRiquelme/grind/issues/197".to_string(),
        title: "omp has no fake".to_string(),
        labels: Vec::new(),
        target_repo: "FlorianRiquelme/grind".to_string(),
        branch: "test/197-omp-fake".to_string(),
        handoff_sha: "13ceb5500000000000000000000000000000000f".to_string(),
        agent: None,
        anchor: "Drive the omp adapter against a real process.".to_string(),
        intent: None,
        model: None,
        done_predicate: "PR is open".to_string(),
        base_branch: "main".to_string(),
        verify_entrypoint: "just verify".to_string(),
        declared_hot_paths: Vec::new(),
    };
    let conditions = attempt::StageConditions {
        claude_bin: "claude",
        run_id: "omp-fake-run",
    };
    let stages_dir = sandbox.run_dir.join("stages").display().to_string();
    let worktree = sandbox.cwd.display().to_string();
    let ctx = attempt::StageContext {
        stage: rung::Stage::Work,
        skill_text: "Do the assigned stage work.",
        stages_dir: &stages_dir,
        worktree: &worktree,
        job: &job,
        model: None,
        notes: None,
        landed: None,
    };
    let invocation = claude::stage_invocation(&conditions, &ctx, mode, None);
    let denied_globs = attempt::denied_for(rung::Stage::Work);
    let spec = RunSpec {
        invocation: &invocation,
        cwd: &sandbox.cwd,
        run_dir: &sandbox.run_dir,
        attempt_n,
        session_id: SESSION_ID,
        worktree: &worktree,
        model,
        denied_globs: &denied_globs,
        file_label: runner::FileLabel::Attempt,
    };
    OmpAdapter {
        bin: fake_omp().display().to_string(),
        fast_model: fast_model.map(str::to_string),
        strong_model: strong_model.map(str::to_string),
        routes: ClassRoutes::default(),
    }
    .run(&spec)
}

/// The common case: an undeclared strong class, so no `--model` and nothing to disambiguate.
fn dispatch(sandbox: &Sandbox, attempt_n: usize, mode: Mode) -> Attempt {
    drive(
        sandbox,
        attempt_n,
        mode,
        &StageModel::Class(ModelClass::Strong),
        None,
        None,
    )
}

#[test]
fn a_flat_session_is_harvested_and_the_dispatch_argv_is_literal() {
    let sandbox = Sandbox::new("flat", &["flat_clean"]);
    let attempt = dispatch(&sandbox, 1, Mode::Dispatch);

    assert_eq!(
        sandbox.argv(1),
        vec![
            "-p".to_string(),
            "--mode".to_string(),
            "json".to_string(),
            "--auto-approve".to_string(),
            "--session-dir".to_string(),
            sandbox.session_flag(),
        ],
        "the whole dispatch argv, so a flag that moved or lost its value is not green"
    );

    assert_eq!(attempt.exit_code, Some(0));
    assert!(attempt.parse_ok);
    assert!(!attempt.is_error);
    assert!(!attempt.rate_limited);
    assert!(attempt.done_promise);
    assert_eq!(attempt.num_turns, Some(2));
    assert_eq!(attempt.stop_reason.as_deref(), Some("endTurn"));
    assert!(
        (attempt.total_cost_usd.expect("a spend channel") - 0.07).abs() < 1e-9,
        "the child's own per-message usage rows reach classify: {:?}",
        attempt.total_cost_usd
    );

    assert_eq!(
        attempt.transcript,
        Transcript::Recorded(format!("sessions/{SESSION_ID}/{FLAT_FILE}"))
    );
    assert!(
        sandbox.harvested(FLAT_FILE).contains("scout the module"),
        "the harvested copy is the child's own bytes"
    );
    assert_eq!(
        attempt.fanout,
        Observed::Present((1, 1)),
        "one `task` spawn, paired to its completion"
    );
}

#[test]
fn a_pinned_model_rides_the_argv_and_an_undeclared_class_omits_the_flag() {
    let pinned = Sandbox::new("pinned", &["flat_clean"]);
    drive(
        &pinned,
        1,
        Mode::Dispatch,
        &StageModel::Pinned("z-ai/glm-5.3-flash".to_string()),
        None,
        None,
    );
    assert_eq!(
        pinned.argv(1),
        vec![
            "-p".to_string(),
            "--mode".to_string(),
            "json".to_string(),
            "--auto-approve".to_string(),
            "--model".to_string(),
            "z-ai/glm-5.3-flash".to_string(),
            "--session-dir".to_string(),
            pinned.session_flag(),
        ],
        "the pin crosses verbatim, between `--auto-approve` and `--session-dir`"
    );
    drop(pinned);

    let undeclared = Sandbox::new("undeclared", &["flat_clean"]);
    drive(
        &undeclared,
        1,
        Mode::Dispatch,
        &StageModel::Class(ModelClass::Fast),
        None,
        Some("deepseek/deepseek-chat-v3.1"),
    );
    assert!(
        !undeclared.argv(1).iter().any(|arg| arg == "--model"),
        "an undeclared fast class omits the flag rather than borrowing strong's id: {:?}",
        undeclared.argv(1)
    );
}

#[test]
fn a_resumed_attempt_carries_the_suffix_and_slices_off_the_pre_run_lines() {
    let sandbox = Sandbox::new("resume", &["flat_clean", "resume_append"]);
    let first = dispatch(&sandbox, 1, Mode::Dispatch);
    assert_eq!(first.fanout, Observed::Present((1, 1)));

    let second = dispatch(&sandbox, 2, Mode::Resume);
    assert_eq!(
        sandbox.argv(2),
        vec![
            "-p".to_string(),
            "--mode".to_string(),
            "json".to_string(),
            "--auto-approve".to_string(),
            "--session-dir".to_string(),
            sandbox.session_flag(),
            "--resume".to_string(),
            FLAT_SUFFIX.to_string(),
        ],
        "resume rides the dispatch argv plus the uuid `newest_stage_file` found on disk"
    );

    assert!(second.parse_ok);
    assert_eq!(
        second.transcript,
        Transcript::Recorded(format!("sessions/{SESSION_ID}/{FLAT_FILE}")),
        "a resume appends to the stage's own file rather than allocating a second"
    );
    assert_eq!(
        second.fanout,
        Observed::Present((1, 0)),
        "only the lines this attempt appended count — attempt 1's paired spawn is below the floor"
    );
    assert!(
        sandbox.harvested(FLAT_FILE).contains("second wave"),
        "the harvested copy carries both attempts"
    );
}

#[test]
fn a_strayed_session_is_found_under_the_encoded_cwd_bucket() {
    let sandbox = Sandbox::new("strayed", &["strayed"]);
    let attempt = dispatch(&sandbox, 1, Mode::Dispatch);

    assert!(
        sandbox.stray_bucket().is_dir(),
        "the shape must actually reproduce the bucket for this test to mean anything"
    );
    assert_eq!(
        attempt.transcript,
        Transcript::Recorded(format!("sessions/{SESSION_ID}/{STRAY_FILE}")),
        "`--session-dir` was ignored and the fallback arm found it anyway"
    );
    assert!(
        sandbox.harvested(STRAY_FILE).contains("strayed scout"),
        "the strayed bytes were copied into the Run's own evidence tree"
    );
    assert_eq!(attempt.fanout, Observed::Present((1, 0)));
}

#[test]
fn a_stray_session_older_than_the_attempt_is_never_harvested_as_ours() {
    let sandbox = Sandbox::new("stale", &["wrote_nothing"]);
    let other_run = sandbox.stray_bucket().join("some-other-runs-bucket");
    std::fs::create_dir_all(&other_run).expect("another Run's bucket");
    let stale =
        other_run.join("2020-01-01T00-00-00-000Z_deadbeef-0000-4000-8000-000000000000.jsonl");
    std::fs::write(&stale, "{\"type\":\"agent_end\",\"messages\":[]}\n")
        .expect("a stale transcript");
    let touched = std::process::Command::new("touch")
        .arg("-t")
        .arg("202001010000.00")
        .arg(&stale)
        .status()
        .expect("run touch");
    assert!(touched.success(), "the stale mtime is the whole point");

    let attempt = dispatch(&sandbox, 1, Mode::Dispatch);

    assert!(stale.is_file(), "the other Run's transcript is untouched");
    assert_eq!(
        attempt.transcript,
        Transcript::WroteNone,
        "the time floor rejects a transcript that predates this attempt"
    );
    assert!(
        matches!(attempt.fanout, Observed::Unobservable(_)),
        "an unharvestable transcript is loud, never a silent zero: {:?}",
        attempt.fanout
    );
    assert!(
        !sandbox
            .stage_dir()
            .join(stale.file_name().expect("a name"))
            .exists(),
        "another Run's transcript never lands in this Run's evidence tree"
    );
}

#[test]
fn no_session_file_anywhere_is_loud_in_the_attempt() {
    let sandbox = Sandbox::new("none", &["wrote_nothing"]);
    let attempt = dispatch(&sandbox, 1, Mode::Dispatch);

    assert!(attempt.parse_ok, "the stream itself was fine");
    assert_eq!(attempt.transcript, Transcript::WroteNone);
    assert_eq!(
        attempt.fanout,
        Observed::Unobservable(Reason::saying(
            "no stage transcript to read: the child allocated no session transcript"
        ))
    );
}

#[test]
fn rate_limit_prose_on_a_real_stderr_with_a_nonzero_exit_is_a_wall() {
    let sandbox = Sandbox::new("wall", &["rate_limited"]);
    let attempt = dispatch(&sandbox, 1, Mode::Dispatch);

    assert_eq!(attempt.exit_code, Some(1));
    assert!(!attempt.parse_ok, "the stream was cut mid-frame");
    assert!(attempt.is_error);
    assert!(
        attempt.rate_limited,
        "the needle is folded over the child's own stderr file, not a literal"
    );
    assert!(!attempt.result_tail.is_empty(), "the raw tail is kept");
}

#[test]
fn the_version_probe_answers_without_spending_a_shape() {
    let sandbox = Sandbox::new("version", &["flat_clean"]);
    let probed = std::process::Command::new(fake_omp())
        .arg("--version")
        .output()
        .expect("probe the fake");
    assert!(probed.status.success());
    assert_eq!(
        String::from_utf8_lossy(&probed.stdout).lines().next(),
        Some("omp/18.0.6"),
        "the first line with a leading `omp/` stripped is what the dispatch snapshot records"
    );
    assert!(
        !sandbox.run_dir.join("fake/counter").exists(),
        "a probe that spent a shape would put every attempt-indexed assertion off by one"
    );

    let attempt = dispatch(&sandbox, 1, Mode::Dispatch);
    assert_eq!(
        attempt.transcript,
        Transcript::Recorded(format!("sessions/{SESSION_ID}/{FLAT_FILE}")),
        "attempt 1 still gets the first shape"
    );
}
