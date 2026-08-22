//! The argument shapes, the doctor driver, and the only thing that writes to stdout.
//!
//! **The exit code reports observability, never health.** Status exiting non-zero on an
//! unhealthy Run is the idiom every CLI has ever followed, and it is precisely how Grind grows
//! a gate through the back door: something downstream acts on the code, and a finding starts
//! blocking. There is deliberately no conversion from a verdict in existence anywhere in this
//! module (ADR-0006's convention mode, aimed at the surface most exposed to it).
//!
//! Nothing here invokes an agent. A view built out of the thing that gets rate-limited is
//! unavailable during exactly the stall it exists to explain.

use crate::decide;
use crate::job::{self, Check, Depth, Refusal};
use crate::observe::{self, Observed, Outcome};
use crate::render::{self, DoctorLine, SingleRun};
use crate::supervisor;
use crate::view::{self, Lookup};
use crate::world;
use std::path::{Path, PathBuf};

/// Whether the command could answer the question it was asked. **Not how the Run is doing.**
enum Observability {
    Answered,
    CouldNotAnswer,
}

impl Observability {
    fn code(self) -> i32 {
        match self {
            Observability::Answered => 0,
            Observability::CouldNotAnswer => 3,
        }
    }
}

/// A refused Dispatch, a refused resume and a failed host check all leave in the same register:
/// **incoherent input**, never a health verdict and never a gate.
const INCOHERENT_INPUT: i32 = 2;

/// What the shipped templates in `dist/` call themselves. The check asks the service manager
/// what it has **loaded** under these names, never the filesystem what is on disk under them.
const BOOT_ONE_SHOT_LABEL: &str = "com.grind.resume-all";
const BOOT_ONE_SHOT_UNIT: &str = "grind-resume-all.service";

pub fn run() -> i32 {
    let args = world::args();
    let rest: Vec<&str> = args.iter().map(String::as_str).collect();
    match rest.as_slice() {
        ["--version"] | ["-V"] => {
            print(&format!("grind {}\n", env!("CARGO_PKG_VERSION")));
            0
        }
        ["--help"] | ["-h"] | [] => {
            print(USAGE);
            0
        }
        ["run", issue] => finish(supervisor::dispatch(issue)),
        // **Before** the generic arm: slice patterns match by position, so that one would
        // otherwise bind `run_id = "--all"` and dispatch it as a run id.
        ["resume", "--all"] => resume_all(),
        ["resume", run_id] => finish(supervisor::resume(run_id)),
        // The rest of argv joined is the note, so an unquoted multi-word note works. An
        // empty rest still matches and refuses in `supervisor`, where the state check lives.
        ["cleared", run_id, note @ ..] => cleared(run_id, &note.join(" ")),
        ["status"] => status_roster(),
        ["status", run_id] => status_one(run_id),
        ["doctor"] => doctor(),
        // **Before** the generic fallthrough, for the same reason `resume --all` is: slice
        // patterns match by position, so unparsed flags would otherwise be read as tokens of
        // an unknown command instead of parsed.
        ["serve", rest @ ..] => serve_dashboard(rest),
        // No `list`. A bare status that resolves to one Run is Grind selecting, and the repair
        // *"pick the one in flight"* would pick a zombie.
        other => refuse(&format!("unknown command: {}", other.join(" "))),
    }
}

const USAGE: &str = "grind — dispatch and supervise headless `lfg` Runs against a Job.

    grind run <issue>       dispatch a Job now (issue number or URL)
    grind resume <run-id>   re-enter a Run that died
    grind resume --all      re-enter every Run on this host a restart cut off
    grind cleared <run-id> <note>   record what changed on a Run a Blocker stopped
    grind status [run-id]   roster when bare; one Run's live view when named
    grind doctor            check the provisioned-host list
    grind serve [--bind <addr>] [--port <n>]   serve the dashboard — pull-only; writes nothing
    grind --version         which copy of the binary is this
";

// --- run and resume ---------------------------------------------------------------------------

fn finish(outcome: Result<supervisor::Outcome, Refusal>) -> i32 {
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(refusal) => {
            print_err(&render::refusal(&refusal.to_string()));
            return INCOHERENT_INPUT;
        }
    };
    if outcome.already_completed {
        print(&format!("run already {}\n", outcome.state));
    }
    // The Handback is composed from a **fresh** read, not from anything the loop was holding.
    let Some(home) = world::home() else {
        return INCOHERENT_INPUT;
    };
    // The verdict the Handback prints and the verdict the process exits on are **one
    // computation**, gathered by the same function the supervisor's terminal comment uses. It
    // used to be computed twice, moments apart, and spent on an exit code while the projection
    // printed the recorded state instead.
    //
    // A record that cannot be found or read here is itself a failure to observe — the Handback
    // this command exists to produce never got composed, so a bare `0` would tell a caller
    // checking `$?` that everything answered when nothing did.
    let Some(facts) = view::gather(&home, &outcome.run_id) else {
        return Observability::CouldNotAnswer.code();
    };
    print(&render::handback(&facts));
    if matches!(facts.verdict, decide::Verdict::Unobserved(_)) {
        Observability::CouldNotAnswer.code()
    } else {
        Observability::Answered.code()
    }
}

/// Record a clearance on a Blocked Run. Recording only: `resume` is the separate act that
/// spends, so success points at it rather than performing it — Grind never chooses to
/// spend an Attempt.
fn cleared(run_id: &str, note: &str) -> i32 {
    match supervisor::cleared(run_id, note) {
        Ok(()) => {
            print(&format!(
                "clearance recorded for {run_id} — re-enter with `grind resume {run_id}`"
            ));
            0
        }
        Err(refusal) => {
            print_err(&render::refusal(&refusal.to_string()));
            INCOHERENT_INPUT
        }
    }
}

/// **Reports what it started, never what any Run concluded.** `finish` is built around one
/// `Outcome` and one Handback; N detached children have neither a single outcome nor a single
/// verdict-derived exit code, and inventing one would be a summary over N Runs.
fn resume_all() -> i32 {
    let report = match supervisor::resume_all() {
        Ok(report) => report,
        Err(refusal) => {
            print_err(&render::refusal(&refusal.to_string()));
            return INCOHERENT_INPUT;
        }
    };
    if report.started.is_empty() && report.skipped.is_empty() {
        print("no Run on this host was cut off.\n");
        return Observability::Answered.code();
    }
    for run_id in &report.started {
        print(&format!("re-entered {run_id}"));
    }
    for (run_id, why) in &report.skipped {
        print(&format!("skipped    {run_id}: {why}"));
    }
    Observability::Answered.code()
}

// --- status ------------------------------------------------------------------------------------

/// Bare `grind status` prints the roster and **never resolves to a single Run**.
fn status_roster() -> i32 {
    let Some(home) = world::home() else {
        print_err(&render::refusal("$HOME is unset"));
        return INCOHERENT_INPUT;
    };
    let rows = view::roster(&home);
    let blind = rows
        .iter()
        .any(|row| matches!(row.supervisor_here, Observed::Unobservable(_)));
    print(&render::roster(&hostname(), &rows));
    if blind {
        Observability::CouldNotAnswer
    } else {
        Observability::Answered
    }
    .code()
}

fn status_one(run_id: &str) -> i32 {
    let Some(home) = world::home() else {
        print_err(&render::refusal("$HOME is unset"));
        return INCOHERENT_INPUT;
    };
    match view::load(&home, run_id) {
        // A Run on another box sends the operator to its Job issue rather than looking like a
        // typo, and answering that *is* answering.
        Lookup::NotHere => {
            print(&render::not_here(run_id, &hostname()));
            Observability::Answered.code()
        }
        Lookup::Unreadable(reason) => {
            print_err(&render::refusal(&reason.to_string()));
            Observability::CouldNotAnswer.code()
        }
        Lookup::Here(found) => {
            let observation = observe_for(&found);
            let signals = decide::signals_of(&observation);
            let promised = found.attempts.last().is_some_and(|a| a.done_promise);
            let verdict = decide::verdict(&signals, promised);
            let live = view::live(
                &view::transcript_path(&home, &found.worktree, &found.session_id),
                world::now_epoch(),
            );
            let here = view::supervisor_here(
                found.supervisor_identity.as_deref(),
                &observe::process_start_stamp(&world::ps_start_stamp(found.supervisor_pid)),
            );
            print(&render::run_view(&SingleRun {
                found: &found,
                observation: &observation,
                live: &live,
                verdict: &verdict,
                contract: &view::verify_contract_of(&found.worktree),
                furthest: decide::furthest_stage(&observation),
                supervisor_here: &here,
                cleared: found.clearances.last(),
                run_state: &view::record_path(&home, run_id),
            }));
            // Derived from what could be observed. There is no path from the verdict to here.
            if matches!(verdict, decide::Verdict::Unobserved(_)) {
                Observability::CouldNotAnswer.code()
            } else {
                Observability::Answered.code()
            }
        }
    }
}

fn observe_for(found: &view::RunView) -> observe::Observation {
    view::observe_fresh(
        Path::new(&found.worktree),
        &found.job.handoff_sha,
        &found.job.branch,
        &found.job.base_branch,
        world::now_iso(),
    )
}

// --- serve -------------------------------------------------------------------------------------

/// Serve the dashboard. The kernel prints its own startup line; `Ok` here only means it
/// answered until the operator stopped it, and a bind failure leaves in the could-not-answer
/// register — never a verdict about any Run.
fn serve_dashboard(rest: &[&str]) -> i32 {
    let (host, port) = match serve_flags(rest) {
        Ok(parsed) => parsed,
        Err(said) => return refuse(&said),
    };
    let Some(home) = world::home() else {
        print_err(&render::refusal("$HOME is unset"));
        return INCOHERENT_INPUT;
    };
    match crate::serve::serve(&home, &host, port) {
        Ok(()) => Observability::Answered.code(),
        Err(said) => {
            print_err(&said);
            Observability::CouldNotAnswer.code()
        }
    }
}

/// Parse `--bind <addr>` and `--port <n>` in any order, each taking one value. Anything else
/// is incoherent input, and the refusal names what was incoherent.
fn serve_flags(rest: &[&str]) -> Result<(String, u16), String> {
    let mut host = "127.0.0.1".to_string();
    let mut port: u16 = 7800;
    let mut flags = rest.iter();
    while let Some(flag) = flags.next() {
        match *flag {
            "--bind" => match flags.next() {
                Some(addr) => host = (*addr).to_string(),
                None => return Err("--bind needs an address".to_string()),
            },
            "--port" => match flags.next() {
                Some(value) => match value.parse::<u16>() {
                    Ok(parsed) => port = parsed,
                    Err(_) => return Err(format!("--port must be a port number, not `{value}`")),
                },
                None => return Err("--port needs a port number".to_string()),
            },
            other => return Err(format!("unknown flag: {other}")),
        }
    }
    Ok((host, port))
}

/// The shared refusal path: stderr, USAGE, exit 2.
fn refuse(said: &str) -> i32 {
    print_err(&render::refusal(said));
    print(USAGE);
    INCOHERENT_INPUT
}

// --- doctor -------------------------------------------------------------------------------------

/// The driver: walk `job`'s item list, call `world` per item, hand the raw triples to `observe`,
/// and pass each item's name and depth mark **alongside** its classified result to `render`.
/// That is what keeps `render` composition-only, with no edge to the list.
fn doctor() -> i32 {
    let Some(home) = world::home() else {
        print_err(&render::refusal("$HOME is unset"));
        return INCOHERENT_INPUT;
    };
    let clones = declared_clones(&home);
    let results: Vec<(&'static str, &'static str, Observed<Outcome>)> = job::host_items()
        .iter()
        .map(|item| {
            (
                item.name,
                mark_of(item.depth),
                check(&home, &clones, item.check),
            )
        })
        .collect();
    let lines: Vec<DoctorLine> = results
        .iter()
        .map(|(name, mark, outcome)| DoctorLine {
            name,
            mark,
            outcome: outcome.clone(),
        })
        .collect();
    print(&render::doctor(&hostname(), &lines));

    let unmet = results.iter().any(|(_, _, outcome)| {
        matches!(
            outcome,
            Observed::Present(Outcome::Unsatisfied(_))
                | Observed::Unobservable(_)
                | Observed::Absent
        )
    });
    // A failed host check is incoherent input, not a judgement. It is also not a gate: nothing
    // downstream consults this code.
    if unmet { INCOHERENT_INPUT } else { 0 }
}

fn mark_of(depth: Depth) -> &'static str {
    match depth {
        Depth::Dispatch => "dispatch",
        Depth::Doctor => "doctor",
        Depth::Step => "step",
    }
}

/// Every `~/.grind/repos/<owner>/<name>` the host declares. The path *is* the declaration, so
/// each clone's own path is what its `origin` is checked against — doctor takes no Job.
fn declared_clones(home: &Path) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    for owner in world::list_dir(&job::grind_dir(home).join("repos")) {
        let Some(owner_name) = owner.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        for clone in world::list_dir(&owner) {
            if let Some(name) = clone.file_name().and_then(|n| n.to_str())
                && world::is_dir(&clone)
            {
                found.push((format!("{owner_name}/{name}"), clone.clone()));
            }
        }
    }
    found
}

fn check(home: &Path, clones: &[(String, PathBuf)], check: Check) -> Observed<Outcome> {
    match check {
        Check::DeclaredClone => {
            let Some((declared, path)) = clones.first() else {
                return observe::declared_clone(false, None, "any repo");
            };
            let origin = world::run(&words(&["git", "remote", "get-url", "origin"]), Some(path));
            observe::declared_clone(true, Some(&origin), declared)
        }
        Check::OneClonePerRepo => {
            let paths: Vec<String> = clones
                .iter()
                .map(|(_, p)| p.display().to_string())
                .collect();
            match clones.first() {
                Some((declared, _)) => observe::one_clone_per_repo(&paths, declared),
                None => observe::one_clone_per_repo(&[], "any repo"),
            }
        }
        Check::ClaudeBinary => {
            let binary = job::claude_bin(home);
            let resolved = world::resolve_link(&binary);
            observe::claude_binary(
                world::is_executable(&binary),
                resolved
                    .as_deref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .as_deref(),
            )
        }
        Check::OnPath(tool) => observe::on_path(tool, &resolves(tool)),
        Check::GitVersionFloor => observe::git_version_floor(
            &world::run(&words(&["git", "--version"]), None),
            job::GIT_VERSION_FLOOR,
        ),
        Check::SkillsPresent => {
            let root = job::grind_dir(home).join("skills").join("run");
            let names: Vec<String> = world::list_dir(&root)
                .into_iter()
                .filter(|p| world::is_dir(p))
                .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
                .collect();
            observe::skills_present(&names)
        }
        // Spelled `cfg!` rather than `std::env::consts::OS`, which is the idiomatic runtime
        // spelling: `tests/topology.rs` string-matches the literal `std::env` and asserts only
        // `world` names it, so the idiom would turn `just verify` red on a carrier that must
        // pass unrelaxed.
        Check::BootOneShot => {
            let asked = if cfg!(target_os = "macos") {
                Some((
                    format!("launchctl print gui/$(id -u)/{BOOT_ONE_SHOT_LABEL}"),
                    observe::Fires::AtLogin,
                ))
            } else if cfg!(target_os = "linux") {
                // **Conjunctive.** `systemctl --user is-enabled` returns enabled purely from the
                // symlink, with or without linger — and without `loginctl enable-linger` a
                // `--user` unit's systemd instance does not start until first login, which on a
                // headless box is never. Asking only the first half is the same silent-pass
                // shape this check was added to prevent, one layer down.
                Some((
                    format!(
                        "systemctl --user is-enabled {BOOT_ONE_SHOT_UNIT} >/dev/null 2>&1 && \
                         [ \"$(loginctl show-user \"$(id -un)\" -p Linger --value)\" = yes ]"
                    ),
                    observe::Fires::AtBoot,
                ))
            } else {
                None
            };
            match asked {
                // Read-only on both platforms: doctor never performs a write to prove a step.
                Some((command, fires)) => observe::boot_one_shot(
                    &world::run(&words(&["sh", "-c", &command]), None),
                    fires,
                ),
                None => observe::unchecked("no restart one-shot is defined for this platform"),
            }
        }
        Check::GhAuthStore => {
            observe::gh_auth_store(&world::run(&words(&["gh", "auth", "status"]), None))
        }
        Check::SshKeyPassphraseless => {
            let key = config("user.signingkey");
            let named = key.stdout.trim().to_string();
            if named.is_empty() {
                return observe::ssh_key_passphraseless(None, None);
            }
            // A read, never a write: doctor performs no write to prove a step.
            let probe = world::run(&words(&["ssh-keygen", "-y", "-P", "", "-f", &named]), None);
            observe::ssh_key_passphraseless(Some(&named), Some(&probe))
        }
        Check::SshKeyBothTypes => {
            observe::ssh_keys_both_types(&world::run(&words(&["gh", "ssh-key", "list"]), None))
        }
        Check::SigningConfig => observe::signing_config(
            &config("gpg.format"),
            &config("user.signingkey"),
            &config("commit.gpgsign"),
        ),
        Check::CommitterIdentity => {
            observe::committer_identity(&config("user.name"), &config("user.email"))
        }
        Check::OriginOverSsh => match clones.first() {
            Some((_, path)) => observe::origin_over_ssh(&world::run(
                &words(&["git", "remote", "get-url", "origin"]),
                Some(path),
            )),
            None => observe::unchecked("no declared clone to read an origin from"),
        },
        Check::NoBoolean => observe::unchecked(
            "performed during provisioning; every available check would be a guess",
        ),
    }
}

fn config(key: &str) -> world::Completed {
    world::run(&words(&["git", "config", "--global", "--get", key]), None)
}

/// `command -v` is a shell builtin, so it needs a shell. `which` is not guaranteed anywhere the
/// rest of this list is.
fn resolves(tool: &str) -> world::Completed {
    world::run(&words(&["sh", "-c", &format!("command -v {tool}")]), None)
}

fn words(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

fn hostname() -> String {
    world::hostname().unwrap_or_else(|| "this host".to_string())
}

fn print(text: &str) {
    world::print_line(text.trim_end_matches('\n'));
}

fn print_err(text: &str) {
    world::print_error(text.trim_end_matches('\n'));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_exit_code_reports_whether_status_could_answer_and_never_how_the_run_is_doing() {
        assert_eq!(Observability::Answered.code(), 0);
        assert_ne!(Observability::CouldNotAnswer.code(), 0);
        // Neither of these takes a verdict, and there is no conversion from one in existence.
        // An unhealthy Run whose every signal was observed answers, and answering is a zero.
    }

    #[test]
    fn a_refusal_leaves_in_the_incoherent_input_register_and_not_the_observability_one() {
        assert_ne!(INCOHERENT_INPUT, Observability::Answered.code());
        assert_ne!(INCOHERENT_INPUT, Observability::CouldNotAnswer.code());
    }

    #[test]
    fn the_surface_is_eight_shapes_and_none_of_them_is_list() {
        // A bare status that resolves to one Run is Grind selecting, and the repair
        // "pick the one in flight" would pick a zombie.
        assert!(!USAGE.contains("grind list"));
        assert!(!USAGE.contains("latest if omitted"));
        for shape in [
            "grind run <issue>",
            "grind resume <run-id>",
            "grind resume --all",
            "grind cleared <run-id> <note>",
            "grind status [run-id]",
            "grind serve [--bind <addr>] [--port <n>]",
            "grind doctor",
            "grind --version",
        ] {
            assert!(USAGE.contains(shape), "the surface must name {shape}");
        }
        assert!(USAGE.contains("roster when bare"));
        assert!(USAGE.contains("pull-only"));
        // Not a boot verb — that names a command after its caller. Not a bare `resume` either:
        // a typo would mutate every branch on the host.
        assert!(!USAGE.contains("grind boot"));
        assert!(!USAGE.contains("grind resume\n"));
    }

    #[test]
    fn serve_flags_default_to_loopback_and_7800() {
        assert_eq!(serve_flags(&[]), Ok(("127.0.0.1".to_string(), 7800)));
    }

    #[test]
    fn serve_flags_parse_in_any_order() {
        assert_eq!(
            serve_flags(&["--port", "8000"]),
            Ok(("127.0.0.1".to_string(), 8000))
        );
        assert_eq!(
            serve_flags(&["--bind", "0.0.0.0"]),
            Ok(("0.0.0.0".to_string(), 7800))
        );
        assert_eq!(
            serve_flags(&["--port", "8000", "--bind", "0.0.0.0"]),
            Ok(("0.0.0.0".to_string(), 8000))
        );
        assert_eq!(
            serve_flags(&["--bind", "0.0.0.0", "--port", "8000"]),
            Ok(("0.0.0.0".to_string(), 8000))
        );
    }

    #[test]
    fn serve_flags_refuse_the_incoherent_and_name_the_flag() {
        assert!(serve_flags(&["--verbose"]).is_err());
        assert!(serve_flags(&["runs.json"]).is_err());
        let said = serve_flags(&["--port", "not-a-port"]).unwrap_err();
        assert!(
            said.contains("--port"),
            "the refusal must name the flag: {said}"
        );
        assert!(serve_flags(&["--port"]).is_err());
        assert!(serve_flags(&["--bind"]).is_err());
    }

    #[test]
    fn every_depth_mark_has_a_word_and_it_is_the_documents() {
        assert_eq!(mark_of(Depth::Dispatch), "dispatch");
        assert_eq!(mark_of(Depth::Doctor), "doctor");
        assert_eq!(mark_of(Depth::Step), "step");
    }

    #[test]
    fn the_driver_answers_for_every_item_on_the_list() {
        // Without this wiring the host requirements have no home: `job` ships the list and its
        // classifiers, and nothing else walks it.
        let home = Path::new("/nowhere/that/exists");
        for item in job::host_items() {
            let outcome = check(home, &[], item.check);
            if item.depth == Depth::Step {
                assert!(
                    matches!(outcome, Observed::Present(Outcome::Unchecked(_))),
                    "`{}` is marked *step* and must carry no boolean",
                    item.name
                );
            }
        }
    }
}
