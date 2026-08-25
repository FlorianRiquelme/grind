//! The other prose/code seam: `DENIED_TOOLS` in `src/attempt.rs` and the list in `CLAUDE.md`.
//!
//! CLAUDE.md says the two halves must change in the same commit and stay byte-identical, and
//! nothing checked it. The globs are documented as **the entire barrier** — no credential at any
//! tier can withhold merge from something allowed to open a PR — so the list a reader trusts and
//! the list a Run actually carries drifting apart is the expensive failure, and it is silent:
//! the prose is what a human reasons from, and the constant is what binds.
//!
//! Same shape as `tests/enqueue_template.rs`, for the same reason. It has to be an integration
//! test: reading a file from a unit test inside `src/` would name the filesystem outside
//! `world`, which `tests/topology.rs` forbids.
//!
//! What it catches is a **half changed alone**, never a list that is too narrow. Widening is
//! still a judgement, and no carrier defends against intent (ADR-0006).

use grind::attempt::{DENIED_TOOLS, denied_for, denied_for_reflect};
use grind::rung::Stage;
use std::fs;
use std::path::PathBuf;

const ALL_STAGES: [Stage; 10] = [
    Stage::Plan,
    Stage::Triage,
    Stage::PlanReview,
    Stage::Work,
    Stage::Simplify,
    Stage::DiffTriage,
    Stage::Review,
    Stage::Validate,
    Stage::Fixes,
    Stage::Ship,
];

fn read(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?} must be readable: {e}"))
}

/// The globs CLAUDE.md prints, in order — every line of the one fenced block whose contents are
/// all `Bash(…)`. Taken by shape rather than by heading, so the surrounding prose can be
/// rewritten freely.
fn documented() -> Vec<String> {
    let claude_md = read("CLAUDE.md");
    let mut blocks: Vec<Vec<String>> = Vec::new();
    let mut current: Option<Vec<String>> = None;
    for line in claude_md.lines() {
        let trimmed = line.trim();
        if trimmed == "```" {
            match current.take() {
                Some(block) => blocks.push(block),
                None => current = Some(Vec::new()),
            }
        } else if let Some(block) = current.as_mut() {
            block.push(trimmed.to_string());
        }
    }
    let mut all_bash: Vec<Vec<String>> = blocks
        .into_iter()
        .filter(|block| !block.is_empty() && block.iter().all(|line| line.starts_with("Bash(")))
        .collect();
    assert_eq!(
        all_bash.len(),
        1,
        "CLAUDE.md must carry exactly one fenced block of `Bash(…)` globs; found {}",
        all_bash.len()
    );
    all_bash.remove(0)
}

/// The globs `src/attempt.rs` binds, in order, read as text rather than by linking the crate —
/// a test that imports the constant proves the constant agrees with itself.
fn bound() -> Vec<String> {
    let source = read("src/attempt.rs");
    let at = source
        .find("pub const DENIED_TOOLS")
        .expect("src/attempt.rs must declare DENIED_TOOLS");
    let body = &source[at..];
    let end = body
        .find("\n];")
        .expect("the DENIED_TOOLS literal must close");
    body[..end]
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let inner = trimmed.strip_prefix('"')?;
            Some(inner.strip_suffix("\",")?.to_string())
        })
        .collect()
}

#[test]
fn the_documented_deny_list_and_the_bound_one_are_the_same_list() {
    let documented = documented();
    let bound = bound();
    assert!(
        !bound.is_empty(),
        "the DENIED_TOOLS literal parsed to nothing, which means this test stopped looking \
         rather than that the barrier is empty"
    );
    assert_eq!(
        documented, bound,
        "CLAUDE.md and `DENIED_TOOLS` have drifted. Both halves change in the same commit: \
         the prose is what a human reasons from and the constant is what binds, and the globs \
         are the entire barrier."
    );
}

#[test]
fn the_declared_length_matches_the_globs_actually_listed() {
    let source = read("src/attempt.rs");
    let at = source
        .find("pub const DENIED_TOOLS: [&str; ")
        .expect("DENIED_TOOLS must declare its length");
    let declared: usize = source[at..]
        .trim_start_matches("pub const DENIED_TOOLS: [&str; ")
        .split(']')
        .next()
        .expect("a length")
        .parse()
        .expect("the declared length is a number");
    assert_eq!(declared, bound().len());
}

/// Widening per stage never drops a base denial: every one of the ten stages, iterated against
/// every glob CLAUDE.md documents, must carry it. Read the documented list rather than the
/// linked constant, so a base glob added to one and not the other is caught here too.
#[test]
fn every_stage_carries_every_documented_base_denial() {
    let base = documented();
    for stage in ALL_STAGES {
        let denied = denied_for(stage);
        for glob in &base {
            assert!(
                denied.contains(glob),
                "{stage} must carry the base denial {glob}"
            );
        }
    }
    let reflect = denied_for_reflect();
    for glob in &base {
        assert!(reflect.contains(glob), "reflect must carry {glob}");
    }
}

#[test]
fn the_report_only_stages_deny_write_and_edit() {
    for stage in [Stage::PlanReview, Stage::Review, Stage::Validate] {
        let denied = denied_for(stage);
        assert!(denied.contains(&"Write".to_string()), "{stage}");
        assert!(denied.contains(&"Edit".to_string()), "{stage}");
    }
}

#[test]
fn only_review_and_validate_carry_the_write_capable_bash_forms() {
    let forms = [
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
    for stage in [Stage::Review, Stage::Validate] {
        let denied = denied_for(stage);
        for form in forms {
            assert!(
                denied.iter().any(|g| g == form),
                "{stage} must carry {form}"
            );
        }
    }
    for stage in [
        Stage::Plan,
        Stage::Triage,
        Stage::PlanReview,
        Stage::Work,
        Stage::Simplify,
        Stage::DiffTriage,
        Stage::Fixes,
        Stage::Ship,
    ] {
        let denied = denied_for(stage);
        for form in forms {
            assert!(
                !denied.iter().any(|g| g == form),
                "{stage} must not carry the panel-only {form}"
            );
        }
    }
}

#[test]
fn work_fixes_and_ship_do_not_deny_write_or_edit() {
    for stage in [Stage::Work, Stage::Fixes, Stage::Ship] {
        let denied = denied_for(stage);
        assert!(!denied.contains(&"Write".to_string()), "{stage}");
        assert!(!denied.contains(&"Edit".to_string()), "{stage}");
    }
}

#[test]
fn every_built_argv_carries_all_base_denials_regardless_of_stage() {
    for stage in ALL_STAGES {
        let denied = denied_for(stage);
        for glob in DENIED_TOOLS {
            assert!(denied.iter().any(|g| g == glob), "{stage}: missing {glob}");
        }
    }
}
