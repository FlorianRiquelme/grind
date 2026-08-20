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

use std::fs;
use std::path::PathBuf;

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
    // `[&str; N]` is the omission-shaped half of this: a glob added without bumping `N` does not
    // compile. Asserting it here as well means the parser above cannot silently miss a line.
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
