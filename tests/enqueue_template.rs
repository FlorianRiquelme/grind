//! The one seam nothing tested: the Enqueue skill writes a Job table, and `src/job.rs` reads it
//! back, and no test spanned the two.
//!
//! *One repo means one diff* was the whole of the mitigation. This is the rest of it — the
//! template's **own** example table, parsed through the **real** parser, so a required row
//! renamed on either side turns `just verify` red instead of turning up three hours into a Run.
//!
//! It has to be an integration test. Reading a file from a unit test inside `src/` would name
//! the filesystem outside `world`, which `tests/topology.rs` forbids; an integration test is a
//! separate crate, so the conflict dissolves with **no exemption list**.
//!
//! What it catches is a **rename**, never a meaning that drifted. A change to either half still
//! belongs in the same diff.

use std::fs;
use std::path::PathBuf;

fn template() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("skills/enqueue/JOB-TEMPLATE.md");
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("the Enqueue template must be readable at {path:?}: {e}"))
}

/// The template's own example table, lifted out of the fenced block that holds it.
fn example_table(template: &str) -> String {
    let mut rows: Vec<&str> = Vec::new();
    for line in template.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            rows.push(line);
        } else if !rows.is_empty() {
            break;
        }
    }
    assert!(
        rows.len() > 2,
        "the template must carry an example table; found {} row(s)",
        rows.len()
    );
    rows.join("\n")
}

/// The table as `gh issue view --json …` hands it over.
fn as_issue(body: &str) -> String {
    serde_json::json!({
        "number": 28,
        "title": "Slice 1b: the agent surface",
        "url": "https://github.com/owner/name/issues/28",
        "state": "OPEN",
        "labels": [],
        "body": body,
    })
    .to_string()
}

#[test]
fn the_templates_own_example_table_parses_and_every_required_row_resolves() {
    let table = example_table(&template());
    let job = grind::job::from_issue_json(&as_issue(&table))
        .expect("the template the skill writes must be a Job the binary can read");

    assert_eq!(job.target_repo, "owner/name");
    assert_eq!(job.branch, "feat/28-slice-1b-agent-surface");
    assert_eq!(job.anchor, "docs/plans/2026-08-05-002-slice-1b-plan.md");
    assert!(!job.done_predicate.is_empty());
    assert_eq!(job.base_branch, "main");
    assert!(!job.verify_entrypoint.is_empty());
}

#[test]
fn the_templates_handoff_sha_row_yields_a_bare_sha_parenthetical_and_all() {
    let job = grind::job::from_issue_json(&as_issue(&example_table(&template())))
        .expect("a readable Job");
    assert_eq!(job.handoff_sha, "723ca913536d279e45549018f022e9d1092bbbec");
}

#[test]
fn the_templates_table_carries_an_intent_row_and_the_parser_reads_it() {
    let job = grind::job::from_issue_json(&as_issue(&example_table(&template())))
        .expect("a readable Job");
    let intent = job.intent.expect("the template offers an `Intent` row");
    assert!(!intent.is_empty());
}

#[test]
fn the_templates_agent_row_reads_none_and_round_trips_when_named() {
    let template = template();

    // The example table writes `none`, so the parser must read no pin at all.
    let job =
        grind::job::from_issue_json(&as_issue(&example_table(&template))).expect("a readable Job");
    assert_eq!(job.agent, None, "`none` must read as no Agent pin");

    // A named profile round-trips into Job.agent.
    let pinned =
        example_table(&template).replace("| **Agent** | `none` |", "| **Agent** | `opus-plan` |");
    assert_ne!(
        pinned,
        example_table(&template),
        "the Agent row must exist to rename"
    );
    let job = grind::job::from_issue_json(&as_issue(&pinned)).expect("a readable Job");
    assert_eq!(job.agent.as_deref(), Some("opus-plan"));
}

#[test]
fn the_templates_table_carries_no_budget_ceiling_row() {
    let table = example_table(&template()).to_lowercase();
    assert!(!table.contains("budget ceiling"), "{table}");
}

#[test]
fn renaming_a_required_row_on_either_side_fails_and_names_the_row() {
    let table = example_table(&template());
    for (was, now, named) in [
        ("**Anchor artifact**", "**Anchor**", "anchor artifact"),
        ("**Handoff SHA**", "**Handoff commit**", "handoff sha"),
        ("**Target repo**", "**Repo**", "target repo"),
        ("**Branch**", "**Branch name**", "branch"),
        ("**Done predicate**", "**Done check**", "done predicate"),
        ("**Base branch**", "**Merge target**", "base branch"),
        (
            "**Verify entrypoint**",
            "**Verify command**",
            "verify entrypoint",
        ),
    ] {
        let renamed = table.replace(was, now);
        assert_ne!(renamed, table, "`{was}` must be in the template to rename");
        let refusal = grind::job::from_issue_json(&as_issue(&renamed))
            .expect_err("a renamed required row must refuse");
        assert!(
            refusal.to_string().contains(named),
            "the refusal must name `{named}`: {refusal}"
        );
    }
}

#[test]
fn no_file_in_the_repo_still_claims_this_seam_is_untested() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for named in ["AGENTS.md", "skills/enqueue/SKILL.md"] {
        let text = fs::read_to_string(root.join(named)).expect("a readable file");
        for claim in ["nothing tests that seam", "nothing tests the seam"] {
            assert!(
                !text.contains(claim),
                "{named} still says `{claim}`, and this file is why that is false"
            );
        }
    }
}
