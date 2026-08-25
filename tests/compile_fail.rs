//! The two things that must not compile, asserted by compiling them.
//!
//! **Why a scratch copy of the crate rather than a file compiled against the built rlib.** From
//! *outside* the crate every non-`pub` item is inaccessible, so `E0603` fires identically for a
//! private type, a `pub(crate)` type, and a reader nested as a child of the record's owner —
//! which is the exact arrangement ADR-0007 says compiles clean. Compiled as a sibling module
//! *inside* a copy of the crate, the error is attributable to the sibling wall, and the two
//! controls below are what keep that attribution honest rather than assumed.
//!
//! No `trybuild` and no dev-dependency: this shells out to `rustc` against the rlibs
//! `cargo test` has already built.
//!
//! Both cases sit inside `cargo test` rather than behind a sibling `just` recipe. Otherwise
//! `cargo test` — the idiom an agent reaches for unprompted — is a false green on the two most
//! load-bearing tests in the repo.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const READ_PATH_CASE: &str = include_str!("compile_fail/01_read_path_reaches_the_writable_type.rs");
const FIFTH_SIGNAL_CASE: &str = include_str!("compile_fail/02_fifth_signal_dropped_at_the_fold.rs");

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// This test binary lives in `<target>/<profile>/deps/`, which is exactly where the rlibs it
/// needs are — so the dependency path is derived rather than guessed.
fn deps_dir() -> PathBuf {
    std::env::current_exe()
        .expect("this test binary")
        .parent()
        .expect("the deps directory")
        .to_path_buf()
}

fn newest_rlib_per_crate(deps: &Path) -> Vec<(String, PathBuf)> {
    let mut newest: Vec<(String, std::time::SystemTime, PathBuf)> = fs::read_dir(deps)
        .expect("read the deps directory")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "rlib"))
        .filter_map(|p| {
            let name = p.file_name()?.to_str()?.to_string();
            let stem = name.strip_prefix("lib")?.strip_suffix(".rlib")?;
            let base = stem
                .split_once('-')
                .map_or(stem, |(base, _)| base)
                .to_string();
            if base == "grind" {
                return None;
            }
            fs::metadata(&p)
                .and_then(|m| m.modified())
                .ok()
                .map(|t| (base, t, p))
        })
        .collect();
    newest.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    newest.dedup_by(|a, b| a.0 == b.0);
    newest
        .into_iter()
        .map(|(base, _, path)| (base, path))
        .collect()
}

/// A copy of `src/` under the target directory, ready to be damaged.
fn scratch_crate(name: &str) -> PathBuf {
    let root = deps_dir().join("grind-compile-fail").join(name);
    let _ = fs::remove_dir_all(&root);
    copy_tree(&manifest_dir().join("src"), &root);
    let _ = fs::remove_file(root.join("main.rs"));
    root
}

/// Recursive copy, kept general rather than flat: `tests/topology.rs` enforces that `src/`
/// carries no subdirectories at all (the privacy guarantee ADR-0007 documents depends on
/// every module being a crate-root sibling), but nothing here should have to assume that
/// invariant holds forever to keep compiling — a future module tree gaining a subdirectory
/// should fail `tests/topology.rs` loudly, not silently stop being copied here too.
fn copy_tree(src: &Path, dest: &Path) {
    fs::create_dir_all(dest).expect("a scratch directory");
    for entry in fs::read_dir(src).expect("read src/").flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().expect("a file name");
            copy_tree(&path, &dest.join(name));
        } else if path.extension().is_some_and(|e| e == "rs") {
            let name = path.file_name().expect("a file name");
            fs::copy(&path, dest.join(name)).expect("copy a module");
        }
    }
}

fn read(root: &Path, module: &str) -> String {
    fs::read_to_string(root.join(format!("{module}.rs"))).expect("read a scratch module")
}

fn write(root: &Path, module: &str, contents: &str) {
    fs::write(root.join(format!("{module}.rs")), contents).expect("write a scratch module");
}

/// Add a module to the scratch crate root, as a **sibling** of everything else.
fn add_sibling_module(root: &Path, module: &str, source: &str) {
    write(root, module, source);
    let lib = read(root, "lib");
    write(root, "lib", &format!("{lib}\npub mod {module};\n"));
}

fn compile(root: &Path) -> Output {
    let deps = deps_dir();
    let mut command = Command::new("rustc");
    command
        .arg("--edition=2024")
        .arg("--crate-type=lib")
        .arg("--crate-name=grind")
        .arg("--emit=metadata")
        .arg("--out-dir")
        .arg(root.join("out"))
        .arg("-L")
        .arg(format!("dependency={}", deps.display()));
    for (name, path) in newest_rlib_per_crate(&deps) {
        command
            .arg("--extern")
            .arg(format!("{name}={}", path.display()));
    }
    command
        .arg(root.join("lib.rs"))
        .output()
        .expect("run rustc")
}

fn diagnostics(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_read_path_reaching_the_writable_record_type_does_not_compile() {
    let root = scratch_crate("read-path");
    add_sibling_module(&root, "status_like_view", READ_PATH_CASE);

    let output = compile(&root);
    let said = diagnostics(&output);
    assert!(
        !output.status.success(),
        "a sibling reaching the writable record type must not compile:\n{said}"
    );
    assert!(said.contains("E0603"), "expected E0603, got:\n{said}");
    assert!(
        !said.to_lowercase().contains("help: consider making"),
        "rustc must offer no fix in the diagnostic:\n{said}"
    );
}

#[test]
fn the_same_crate_with_the_record_type_made_crate_visible_compiles() {
    let root = scratch_crate("crate-visible");
    let supervisor = read(&root, "supervisor")
        .replace("\nstruct RunRecord {", "\npub(crate) struct RunRecord {");
    assert!(
        supervisor.contains("pub(crate) struct RunRecord"),
        "the control must actually widen it"
    );
    write(&root, "supervisor", &supervisor);
    add_sibling_module(&root, "status_like_view", READ_PATH_CASE);

    let output = compile(&root);
    assert!(
        output.status.success(),
        "the control must compile, or the case above proves nothing:\n{}",
        diagnostics(&output)
    );
}

#[test]
fn the_same_crate_with_the_read_path_nested_under_the_records_owner_compiles() {
    let root = scratch_crate("nested-child");
    let supervisor = read(&root, "supervisor");
    fs::create_dir_all(root.join("supervisor")).expect("a parent directory");
    fs::write(
        root.join("supervisor").join("mod.rs"),
        format!("{supervisor}\n\npub mod status_like_view;\n"),
    )
    .expect("write the parent module");
    fs::write(
        root.join("supervisor").join("status_like_view.rs"),
        READ_PATH_CASE,
    )
    .expect("write the child");
    fs::remove_file(root.join("supervisor.rs")).expect("the flat module gives way to the tree");

    let output = compile(&root);
    assert!(
        output.status.success(),
        "a child reaching its ancestor's private items compiles clean — that is the whole \
         reason the sibling arrangement is load-bearing:\n{}",
        diagnostics(&output)
    );
}

#[test]
fn a_fifth_signal_dropped_at_the_fold_does_not_compile() {
    let root = scratch_crate("fifth-signal");
    let decide = read(&root, "decide").replace(
        "    pub pr_base_matches_declared: Observed<bool>,\n}",
        "    pub pr_base_matches_declared: Observed<bool>,\n    pub fanout_healthy: Observed<bool>,\n}",
    );
    assert!(
        decide.contains("fanout_healthy"),
        "the fifth signal must actually be added"
    );
    write(&root, "decide", &decide);
    add_sibling_module(&root, "fifth_signal_note", FIFTH_SIGNAL_CASE);

    let output = compile(&root);
    let said = diagnostics(&output);
    assert!(
        !output.status.success(),
        "a dropped signal must not compile:\n{said}"
    );
    assert!(
        said.contains("E0027"),
        "the fold must be the site that fails, with E0027:\n{said}"
    );
}

#[test]
fn the_unmodified_crate_compiles_the_same_way() {
    let root = scratch_crate("baseline");
    let output = compile(&root);
    assert!(
        output.status.success(),
        "the unmodified scratch crate must compile, or every case above is vacuous:\n{}",
        diagnostics(&output)
    );
}
