//! The source-level carrier: process, filesystem and environment access are named in `world`
//! and nowhere else, and the record's writer and its readers stay siblings at the crate root.
//!
//! This is string matching over the base's own source, and it can be fooled by
//! `use std::env as e`. **Do not harden it.** It guards *convention* — the ecosystem's default
//! idiom applied without deciding anything — and an agent aliasing an import to dodge a test
//! has crossed into intent, which ADR-0006 establishes no carrier defends against. Making it
//! cleverer buys nothing and regresses forever.
//!
//! It is an integration test rather than a `#[cfg(test)]` unit for a reason that is not
//! stylistic: integration tests are separate crates, so the glob over `src/**` needs **no
//! exemption list** — and an exemption list is what an agent widens by one entry without
//! deciding anything.

use std::fs;
use std::path::{Path, PathBuf};

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Every `.rs` file under `src/`, as (path-relative-to-src, contents).
fn sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
    collect(&src_dir(), &src_dir(), &mut out);
    out.sort();
    assert!(
        !out.is_empty(),
        "found no sources under src/ — the glob is broken, not the base"
    );
    out
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    for entry in fs::read_dir(dir).expect("read src/").flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let name = path
                .strip_prefix(root)
                .expect("under src/")
                .display()
                .to_string();
            out.push((name, fs::read_to_string(&path).expect("read source")));
        }
    }
}

/// Whole-line comments are dropped before matching, so a doc comment may *name* `std::fs`
/// while an import may not. This is not hardening — it is the difference between the test and
/// a ban on writing the rule down, and the rule has to be written down somewhere an editor
/// hits it.
fn code_only(contents: &str) -> String {
    contents
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The pure half, so the negative cases below are literals rather than a damaged checkout.
fn files_naming(sources: &[(String, String)], needle: &str) -> Vec<String> {
    sources
        .iter()
        .filter(|(_, contents)| code_only(contents).contains(needle))
        .map(|(name, _)| name.clone())
        .collect()
}

#[test]
fn process_access_is_named_in_world_only() {
    let offenders = files_naming(&sources(), "std::process");
    assert_eq!(
        offenders,
        vec!["world.rs".to_string()],
        "`std::process` must be named in src/world.rs only; found it in {offenders:?}"
    );
}

#[test]
fn filesystem_access_is_named_in_world_only() {
    let offenders = files_naming(&sources(), "std::fs");
    assert_eq!(
        offenders,
        vec!["world.rs".to_string()],
        "`std::fs` must be named in src/world.rs only; found it in {offenders:?}"
    );
}

#[test]
fn environment_access_is_named_in_exactly_one_module_and_it_is_world() {
    let offenders = files_naming(&sources(), "std::env");
    assert_eq!(
        offenders.len(),
        1,
        "`std::env` must be named in exactly one module; found it in {offenders:?}"
    );
    assert_eq!(
        offenders[0], "world.rs",
        "the one module naming `std::env` must be `world`; it is {}",
        offenders[0]
    );
}

/// Sockets join the named-here-only list as an amendment, not an exception: the listener is
/// `serve`'s essence (KTD3), and wrapping it in `world` would drag stream I/O through ceremony
/// without making anything testable (ADR-0014).
#[test]
fn socket_access_is_named_in_serve_only() {
    let offenders = files_naming(&sources(), "std::net");
    assert_eq!(
        offenders,
        vec!["serve.rs".to_string()],
        "`std::net` must be named in src/serve.rs only; found it in {offenders:?}"
    );
}

/// The UI never writes (ADR-0013; #23): the served surface — kernel, pages, and embedded
/// assets alike — may not name the record's write side, so a mutation route cannot arrive by
/// import and a save cannot hide inside a string constant.
#[test]
fn the_server_never_names_the_write_side() {
    let sources = sources();
    for banned in [
        "RunRecord",
        "push_attempt",
        "push_clearance",
        "::save",
        "dispatch(",
    ] {
        let offenders: Vec<String> = ["serve.rs", "page.rs", "style.rs", "script.rs"]
            .iter()
            .filter(|name| {
                sources
                    .iter()
                    .find(|(n, _)| n == *name)
                    .is_some_and(|(_, contents)| code_only(contents).contains(banned))
            })
            .map(|name| name.to_string())
            .collect();
        assert!(
            offenders.is_empty(),
            "`{banned}` is the write side; the served surface must never name it — \
             found in {offenders:?}"
        );
    }
}

/// **Grind adds, never classifies** (ADR-0012). A comment is additive and ungoverned; a label,
/// an assignee, a project and a milestone are shared namespaces the target repo's owner
/// governs. `QUEUE_LABEL` erased a triage fact — `ready-for-agent`, one of the five canonical
/// triage roles — to record a queue fact, and the repair is subtractive, so the carrier is the
/// absence of the spelling rather than a check on which label is applied.
#[test]
fn no_path_in_src_classifies_an_issue() {
    let sources = sources();
    for classifying in [
        "--add-label",
        "--remove-label",
        "--label",
        "--add-assignee",
        "--remove-assignee",
        "--assignee",
        "--add-project",
        "--remove-project",
        "--project",
        "--milestone",
    ] {
        let offenders = files_naming(&sources, classifying);
        assert!(
            offenders.is_empty(),
            "`{classifying}` must reach no argv Grind builds; found it in {offenders:?}"
        );
    }
}

#[test]
fn main_delegates_to_cli_and_does_nothing_else() {
    let main = fs::read_to_string(src_dir().join("main.rs")).expect("read src/main.rs");
    assert!(
        main_is_only_a_delegation(&main),
        "src/main.rs must hold nothing but a `fn main()` delegating to `cli`; it holds:\n{main}"
    );
}

/// The sibling wall, as a fact about the filesystem: **no directories under `src/`.** A child
/// module reaches its ancestor's private items and compiles clean, so nesting the record's
/// readers under its writer withdraws the carrier silently — by housekeeping, which is exactly
/// the failure mode nobody reviews. Every module is a crate-root sibling or it does not exist.
#[test]
fn every_module_is_a_crate_root_sibling() {
    let nested: Vec<String> = sources()
        .into_iter()
        .map(|(name, _)| name)
        .filter(|n| n.contains('/'))
        .collect();
    assert!(
        nested.is_empty(),
        "no module may live in a subdirectory of src/ — `supervisor` and `view` are siblings \
         and a shared parent compiles clean. Found: {nested:?}"
    );
}

/// The attractor nouns, banned by name. A `record/` parent or a `types` module pulls the
/// writer and the reader back under one roof, and the compiler will not object when they
/// arrive. This is why types live with their producer.
#[test]
fn no_module_is_named_for_a_noun_two_others_share() {
    let banned = ["record", "types", "state", "model"];
    for (name, _) in sources() {
        let stem = name.trim_end_matches(".rs");
        assert!(
            !banned.contains(&stem),
            "`{name}` is named for a noun more than one module shares, which is what pulls the \
             record's writer and its readers back under one roof. Types live with their producer."
        );
    }
}

/// The reset-sleep reading must come from the local-time seam, not a re-derived UTC path.
/// Reverting the supervisor's call-site edit alone — reintroducing `now_hour_minute` and
/// un-swapping the argument — must turn this test red; that falsifiability is the point.
#[test]
fn the_reset_sleep_reads_the_local_time_seam_rather_than_a_utc_path() {
    let supervisor =
        fs::read_to_string(src_dir().join("supervisor.rs")).expect("read src/supervisor.rs");
    let code = code_only(&supervisor);
    assert!(
        code.contains("world::now_local_hour_minute"),
        "supervisor.rs must feed `policy::reset_time_sleep` from `world::now_local_hour_minute`; \
         it does not"
    );
    assert!(
        !code.contains("fn now_hour_minute"),
        "the UTC-deriving `now_hour_minute` helper must be gone entirely, not shadowed or \
         renamed with the same body elsewhere"
    );
}

fn main_is_only_a_delegation(main: &str) -> bool {
    let code = code_only(main);
    let body: Vec<&str> = code
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    matches!(body.as_slice(), [open, delegation, close]
        if open.starts_with("fn main()") && *close == "}" && delegation.contains("cli"))
}

#[test]
fn a_second_module_naming_process_access_is_caught_and_named() {
    let damaged = vec![
        (
            "world.rs".to_string(),
            "use std::process::Command;".to_string(),
        ),
        (
            "job.rs".to_string(),
            "use std::process::Command;".to_string(),
        ),
    ];
    assert_eq!(
        files_naming(&damaged, "std::process"),
        vec!["world.rs", "job.rs"]
    );
}

#[test]
fn a_second_module_naming_environment_access_is_caught() {
    let damaged = vec![
        (
            "world.rs".to_string(),
            "std::env::var_os(\"HOME\")".to_string(),
        ),
        (
            "render.rs".to_string(),
            "std::env::var(\"GRIND_MAX_ATTEMPTS\")".to_string(),
        ),
    ];
    assert_eq!(files_naming(&damaged, "std::env").len(), 2);
}

#[test]
fn naming_a_module_in_prose_is_not_naming_it_in_code() {
    let prose = vec![(
        "lib.rs".to_string(),
        "//! `world` is the sole namer of `std::process` and `std::fs`.\npub mod world;"
            .to_string(),
    )];
    assert!(files_naming(&prose, "std::process").is_empty());
    assert!(files_naming(&prose, "std::fs").is_empty());
}

#[test]
fn a_main_that_grows_a_second_statement_is_caught() {
    assert!(main_is_only_a_delegation(
        "fn main() {\n    grind::cli::run();\n}\n"
    ));
    assert!(main_is_only_a_delegation(
        "fn main() {\n    grind::world::exit(grind::cli::run());\n}\n"
    ));
    assert!(!main_is_only_a_delegation(
        "fn main() {\n    let home = grind::world::home();\n    grind::cli::run();\n}\n"
    ));
    assert!(!main_is_only_a_delegation(
        "fn main() {\n    println!(\"hello\");\n}\n"
    ));
}

#[test]
fn a_module_nested_under_a_shared_parent_is_caught() {
    let nested = ["record/supervisor.rs", "record/view.rs"];
    assert!(nested.iter().all(|n| n.contains('/')));
}
