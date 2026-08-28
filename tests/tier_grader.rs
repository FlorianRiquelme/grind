//! The Tier Grader seat's source-level carrier (ADR-0020, issue #166).
//!
//! Like `tests/topology.rs`, this is string matching over the base's own sources and
//! authored skills. **Do not harden it.** It guards convention — that the grader's
//! contract surface exists where the ADR names it, with the schema the supervisor parses
//! — and an agent renaming a carrier to dodge a test has crossed into intent, which
//! ADR-0006 establishes no carrier defends against.
//!
//! Some assertions name symbols a sibling slice is landing concurrently
//! (`GraderVerdict`, `triage_decision_with_grade`, `Decision::graded`); until those land
//! the file still parses and every assertion here is a literal grep, so a failure names
//! exactly the missing carrier.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn read_repo(rel: &str) -> String {
    fs::read_to_string(repo_dir().join(rel))
        .unwrap_or_else(|e| panic!("read {rel}: {e} — the carrier file moved, not the rule"))
}

/// The grader's return file and its strict schema are contract surface: the supervisor
/// reads `stages/triage/grade.json` and parses a `GraderVerdict` from it, and the skill
/// that writes it documents exactly those keys — `deny_unknown_fields` on the reader side
/// means a writer-side drift must be a failing test, not a silently dropped key (ADR-0020).
#[test]
fn grade_file_and_schema_are_named_on_both_sides() {
    let supervisor = read_repo("src/supervisor.rs");
    assert!(
        supervisor.contains("grade.json"),
        "src/supervisor.rs must name the grader's return file `grade.json`"
    );
    assert!(
        supervisor.contains("GraderVerdict"),
        "src/supervisor.rs must parse the grader's verdict as `decide::GraderVerdict`"
    );

    let skill = read_repo("skills/run/grade/SKILL.md");
    for needle in [
        "grade.json",
        "tier",
        "rationale",
        "signal",
        "value",
        "weight",
    ] {
        assert!(
            skill.contains(needle),
            "skills/run/grade/SKILL.md must document `{needle}` — the exact schema key the \
             strict parser accepts"
        );
    }
}

/// The grade decides, it never ships: the skill's only write is the one return file, and
/// the grader session must not carry git write verbs. Carrier of ADR-0003's no-gates rule
/// applied to the newest seat — a grader that could push would be a gate with a rationale.
#[test]
fn grade_skill_writes_only_its_return_file() {
    let skill = read_repo("skills/run/grade/SKILL.md");
    assert!(
        skill.contains("stages/triage/grade.json"),
        "the skill must name its one writable path `stages/triage/grade.json`"
    );
    for denied in ["git push", "gh pr merge", "git commit", "git amend"] {
        assert!(
            !skill.contains(denied),
            "skills/run/grade/SKILL.md must not carry the write verb `{denied}` — the grader \
             judges a tier, it never acts on the repo"
        );
    }
}

/// ADR-0015 as amended: the grader replaces the static tier at Triage only. The merge that
/// applies a grade lives in the supervisor's Triage wiring (`triage_decision_with_grade`),
/// while Diff-triage still maxes with the floor — so supervisor.rs carries the merge and
/// decide.rs carries the `graded` receipt field, and Diff-triage's path never touches either.
#[test]
fn grade_applies_at_triage_and_diff_triage_stays_static() {
    let supervisor = read_repo("src/supervisor.rs");
    assert!(
        supervisor.contains("triage_decision_with_grade"),
        "src/supervisor.rs must carry `triage_decision_with_grade` — the pure merge the \
         Triage pass calls before writing its decision"
    );
    let decide = read_repo("src/decide.rs");
    assert!(
        decide.contains("graded"),
        "src/decide.rs's `Decision` must carry a `graded` field — the receipt showing why, \
         not just that"
    );

    let supervisor = read_repo("src/supervisor.rs");
    let diff_triage = supervisor
        .split("Stage::DiffTriage")
        .nth(1)
        .expect("src/supervisor.rs must dispatch `Stage::DiffTriage`");
    let triage_cut = diff_triage.split("fn ").next().expect("non-empty");
    assert!(
        !triage_cut.contains("grade.json") && !triage_cut.contains("graded"),
        "Diff-triage must not read or apply a grade — ADR-0020 scopes the grader to the \
         Triage tier call; Diff-triage still maxes with the floor"
    );
}

/// The index-lines rule: adding `docs/adr/0020` means the counters naming `docs/adr/`
/// move in the same change. Carrier for the ledger pattern behind Job #138.
#[test]
fn adr_count_follows_the_directory() {
    let readme = read_repo("README.md");
    assert!(
        readme.contains("twenty accepted decisions"),
        "README.md's docs/adr index line must say twenty after ADR-0020"
    );
    let agents = read_repo("AGENTS.md");
    assert!(
        agents.contains("twenty accepted decisions"),
        "AGENTS.md's docs/adr index line must say twenty after ADR-0020"
    );
    assert!(
        repo_dir()
            .join("docs/adr/0020-the-triage-tier-call-is-judged-not-computed.md")
            .exists(),
        "docs/adr/0020-the-triage-tier-call-is-judged-not-computed.md must exist"
    );
}
