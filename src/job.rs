//! How a Job reference becomes everything a dispatch needs — the field table, the plugin pin,
//! the repo path, the `claude` binary, the worktree choice.
//!
//! All four resolutions are one act, and all four are pure over text some child printed. The
//! fix for *nothing that touches git is tested* is pure parse functions over output text, not
//! a library (ADR-0007): `world` spawns, this module reads what came back, and every test here
//! runs from string literals with no `git` invocation anywhere.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A refusal: **incoherent input, never a judgement.** The same register as the dirty-worktree
/// refusal and a refused lock. Checking is not gating (ADR-0003), so nothing here carries a
/// word about quality — a Job that cannot be read has not been assessed.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal(String);

impl Refusal {
    pub fn saying(what: impl Into<String>) -> Self {
        Refusal(what.into())
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Which issue, and in which repo. `None` means *the repo `gh` is already pointed at*, which
/// is what a bare number means.
#[derive(Debug, Clone, PartialEq)]
pub struct Reference {
    pub repo: Option<String>,
    pub number: u64,
}

/// The plugin pin. **No `Default`, no `Latest`, and no way to build one that is missing a
/// half** — once `Latest` is spelled, resolve-at-dispatch is one match arm away, and advancing
/// a pin is the act of Promotion. Refusal is the absence of a spelling rather than a rejected
/// case (ADR-0006), so nothing in this file tests for the string `latest`: it is refused
/// because it carries neither a marketplace nor a version, like any other prose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginPin {
    name: String,
    marketplace: String,
    version: String,
}

impl PluginPin {
    /// The only constructor. Both halves or nothing.
    pub fn parse(spec: &str) -> Result<Self, Refusal> {
        let cleaned = clean(spec);
        let version = find_version(&cleaned).ok_or_else(|| {
            Refusal::saying(format!(
                "the `pinned plugin version` row carries no literal x.y.z: {spec}"
            ))
        })?;
        let (name, marketplace) = find_name_at_marketplace(&cleaned).ok_or_else(|| {
            Refusal::saying(format!(
                "the `pinned plugin version` row carries no name@marketplace: {spec}"
            ))
        })?;
        Ok(PluginPin {
            name,
            marketplace,
            version,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn marketplace(&self) -> &str {
        &self.marketplace
    }

    pub fn version(&self) -> &str {
        &self.version
    }
}

/// One unit of queued work, as read off its issue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub issue: u64,
    pub url: String,
    pub title: String,
    pub labels: Vec<String>,
    pub target_repo: String,
    pub branch: String,
    pub handoff_sha: String,
    pub anchor: String,
    pub budget: Option<String>,
    pub model: Option<String>,
    pub plugin: PluginPin,
}

// --- reading the reference and the issue ------------------------------------------------

pub fn parse_reference(input: &str) -> Result<Reference, Refusal> {
    let trimmed = input.trim();
    if let Some(rest) = trimmed.strip_prefix("https://github.com/") {
        let mut parts = rest.split('/');
        let (Some(owner), Some(name), Some("issues"), Some(number)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(Refusal::saying(format!(
                "cannot read an issue out of: {input}"
            )));
        };
        let number = number
            .split(['#', '?'])
            .next()
            .unwrap_or_default()
            .parse::<u64>()
            .map_err(|_| Refusal::saying(format!("cannot read an issue number out of: {input}")))?;
        let repo = format!("{owner}/{name}");
        validate_repo(&repo)?;
        return Ok(Reference {
            repo: Some(repo),
            number,
        });
    }
    let number = trimmed
        .trim_start_matches('#')
        .parse::<u64>()
        .map_err(|_| Refusal::saying(format!("cannot read an issue number out of: {input}")))?;
    Ok(Reference { repo: None, number })
}

/// Reads a Job out of `gh issue view --json number,title,body,url,labels,state`.
///
/// A missing required row refuses and names the row, at dispatch rather than three hours into
/// a Run. `target repo` becomes a filesystem path and `branch` becomes a lock filename, so both
/// are validated as segments before they leave this module — a Job is a private-repo artifact
/// today, but a value that reaches a path deserves a shape either way.
pub fn from_issue_json(raw: &str) -> Result<Job, Refusal> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| Refusal::saying(format!("`gh issue view` returned unreadable JSON: {e}")))?;

    let url = value
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let issue = value
        .get("number")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| Refusal::saying("`gh issue view` returned no issue number".to_string()))?;
    let title = value
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let labels = value
        .get("labels")
        .and_then(|v| v.as_array())
        .map(|all| {
            all.iter()
                .filter_map(|l| l.get("name").and_then(|n| n.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let body = value
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    let fields = field_table(body);
    let required = |row: &str| -> Result<String, Refusal> {
        fields
            .iter()
            .find(|(key, _)| key == row)
            .map(|(_, value)| value.clone())
            .ok_or_else(|| {
                Refusal::saying(format!("Job {url} has no `{row}` row in its field table"))
            })
    };
    let optional = |row: &str| -> Option<String> {
        fields
            .iter()
            .find(|(key, _)| key == row)
            .map(|(_, value)| value.clone())
            .filter(|v| !is_blank_row(v))
    };

    let target_repo = required("target repo")?;
    validate_repo(&target_repo)?;
    let branch = required("branch")?;
    validate_branch(&branch)?;
    let handoff_sha = required("handoff sha")?;
    let anchor = required("anchor artifact")?;
    let plugin = PluginPin::parse(&required("pinned plugin version")?)?;
    let budget = optional("budget ceiling");
    let model = optional("model");

    Ok(Job {
        issue,
        url,
        title,
        labels,
        target_repo,
        branch,
        handoff_sha,
        anchor,
        budget,
        model,
        plugin,
    })
}

/// The `| key | value |` rows of a markdown table, lowercased keys, header and separator rows
/// dropped. Hand-rolled: five shapes do not justify a dependency, and no regex crate exists
/// here (ADR-0005).
fn field_table(body: &str) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if !line.starts_with('|') || !line.ends_with('|') || line.len() < 3 {
            continue;
        }
        let cells: Vec<&str> = line[1..line.len() - 1].split('|').collect();
        if cells.len() != 2 {
            continue;
        }
        let key = clean(cells[0]).to_lowercase();
        let value = clean(cells[1]);
        if key.is_empty() || key == "field" || key == "value" || is_separator(&key) {
            continue;
        }
        if is_separator(&value) {
            continue;
        }
        rows.push((key, value));
    }
    rows
}

fn is_separator(cell: &str) -> bool {
    !cell.is_empty() && cell.chars().all(|c| c == '-' || c == ':' || c == ' ')
}

fn is_blank_row(value: &str) -> bool {
    let lowered = value.trim().to_lowercase();
    lowered.is_empty() || lowered == "none" || lowered == "-" || lowered == "n/a"
}

fn clean(cell: &str) -> String {
    cell.chars()
        .filter(|c| *c != '`' && *c != '*')
        .collect::<String>()
        .trim()
        .to_string()
}

// --- validating what reaches a path -----------------------------------------------------

fn is_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

fn validate_repo(repo: &str) -> Result<(), Refusal> {
    let parts: Vec<&str> = repo.split('/').collect();
    if parts.len() != 2 || !parts.iter().all(|p| is_segment(p)) {
        return Err(Refusal::saying(format!(
            "the `target repo` row must be owner/name with no path in it: {repo}"
        )));
    }
    Ok(())
}

fn validate_branch(branch: &str) -> Result<(), Refusal> {
    if branch.starts_with('/') || branch.ends_with('/') || !branch.split('/').all(is_segment) {
        return Err(Refusal::saying(format!(
            "the `branch` row must be slash-separated segments with no path traversal: {branch}"
        )));
    }
    Ok(())
}

pub fn repo_owner_and_name(target_repo: &str) -> (&str, &str) {
    target_repo.split_once('/').unwrap_or(("", target_repo))
}

// --- resolving the host's four facts, purely --------------------------------------------

/// `~/.grind` — the host's Grind directory, whose **layout is the declaration**. No config
/// file, no format to parse, and no override: `$HOME` is the only variable (ADR-0008).
pub fn grind_dir(home: &Path) -> PathBuf {
    home.join(".grind")
}

/// `~/.grind/repos/<owner>/<name>` — one declared clone per target repo, never a search path.
pub fn repo_path(home: &Path, target_repo: &str) -> PathBuf {
    let (owner, name) = repo_owner_and_name(target_repo);
    grind_dir(home).join("repos").join(owner).join(name)
}

/// `~/.grind/bin/claude` — the binary Grind spawns, named by the layout because Grind had to
/// dodge a shim.
pub fn claude_bin(home: &Path) -> PathBuf {
    grind_dir(home).join("bin").join("claude")
}

pub fn runs_dir(home: &Path) -> PathBuf {
    grind_dir(home).join("runs")
}

pub fn locks_dir(home: &Path) -> PathBuf {
    grind_dir(home).join("locks")
}

/// Resolved **once**, at dispatch. The resolved path goes into the record and every attempt
/// and every `--resume` reads the record — an eight-attempt Run spanning hours of rate-limit
/// sleeps must not start on one version and finish on another.
pub fn plugin_dir(home: &Path, pin: &PluginPin) -> PathBuf {
    home.join(".claude")
        .join("plugins")
        .join("cache")
        .join(pin.marketplace())
        .join(pin.name())
        .join(pin.version())
}

// --- pure parses over porcelain ----------------------------------------------------------

/// `git status --porcelain`: any output at all means dirty.
pub fn is_dirty(porcelain: &str) -> bool {
    !porcelain.trim().is_empty()
}

/// Adopt the branch's existing worktree when the declared clone has one. The author runs ten
/// parallel worktrees and git allows a branch in only one of them, so a Run joins rather than
/// fights — which is also what makes the dispatch lock the only thing standing between two
/// `claude` processes and one directory.
pub fn adopt_worktree(porcelain: &str, branch: &str) -> Option<PathBuf> {
    let wanted = format!("refs/heads/{branch}");
    let mut current: Option<&str> = None;
    for line in porcelain.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current = Some(path.trim());
        } else if line
            .strip_prefix("branch ")
            .is_some_and(|f| f.trim() == wanted)
        {
            return current.map(PathBuf::from);
        }
    }
    None
}

/// Where a worktree goes when the clone has none for this branch.
pub fn worktree_to_create(repo_path: &Path, branch: &str) -> PathBuf {
    repo_path
        .join(".claude")
        .join("worktrees")
        .join(format!("grind-{}", branch.replace('/', "-")))
}

/// A HEAD that differs from the Handoff SHA is a **note on the dispatch plan, never a
/// refusal** — the information without a new gate.
pub fn head_note(head: &str, handoff_sha: &str) -> Option<String> {
    let head = head.trim();
    let handoff = handoff_sha.trim();
    (head != handoff).then(|| {
        format!(
            "worktree HEAD {} != Handoff SHA {}",
            short(head),
            short(handoff)
        )
    })
}

fn short(sha: &str) -> String {
    sha.chars().take(8).collect()
}

/// A `budget ceiling` row becomes the spend cap on every `claude` invocation.
pub fn spend_cap(budget: Option<&str>) -> Option<String> {
    let raw = budget?;
    if is_blank_row(raw) {
        return None;
    }
    let number: String = raw
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    (!number.is_empty()).then_some(number)
}

// --- the two scanners the plugin pin needs -----------------------------------------------

fn find_version(text: &str) -> Option<String> {
    for token in text.split(|c: char| !(c.is_ascii_digit() || c == '.')) {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() == 3
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        {
            return Some(token.to_string());
        }
    }
    None
}

fn find_name_at_marketplace(text: &str) -> Option<(String, String)> {
    for token in text.split_whitespace() {
        let Some((name, rest)) = token.split_once('@') else {
            continue;
        };
        let marketplace = rest.split('@').next().unwrap_or_default();
        if is_pin_word(name) && is_pin_word(marketplace) && find_version(marketplace).is_none() {
            return Some((name.to_string(), marketplace.to_string()));
        }
    }
    None
}

fn is_pin_word(word: &str) -> bool {
    !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(rows: &str) -> String {
        format!(
            "Some prose above the table.\n\n| Field | Value |\n|---|---|\n{rows}\n\nProse below."
        )
    }

    fn issue_json(rows: &str) -> String {
        serde_json::json!({
            "number": 28,
            "title": "Slice 1b: the agent surface",
            "url": "https://github.com/FlorianRiquelme/snapper/issues/28",
            "labels": [{"name": "grind:queued"}],
            "state": "OPEN",
            "body": body(rows),
        })
        .to_string()
    }

    const FULL_ROWS: &str = "| Target repo | FlorianRiquelme/snapper |\n\
         | Branch | feat/28-slice-1b-agent-surface-screensource-seam |\n\
         | Handoff SHA | `9d1f4c7a2b6e0538d4a17c9b3e5f8021ac6d4e77` |\n\
         | Anchor artifact | docs/plans/2026-08-05-002-plan.md |\n\
         | Pinned plugin version | `compound-engineering@compound-engineering-plugin` 3.21.3 |";

    #[test]
    fn a_number_and_a_url_resolve_to_the_same_issue() {
        let bare = parse_reference("123").expect("a bare number");
        let full = parse_reference("https://github.com/owner/name/issues/123").expect("a url");
        assert_eq!(bare.number, full.number);
        assert_eq!(full.repo.as_deref(), Some("owner/name"));
        assert_eq!(bare.repo, None);
    }

    #[test]
    fn a_target_repo_carrying_a_path_refuses_and_names_the_row() {
        for hostile in ["../../etc", "/etc/passwd", "owner/name/extra", "owner"] {
            let rows = FULL_ROWS.replace("FlorianRiquelme/snapper", hostile);
            let refusal = from_issue_json(&issue_json(&rows)).expect_err("must refuse");
            assert!(
                refusal.to_string().contains("target repo"),
                "the refusal must name the row: {refusal}"
            );
        }
    }

    #[test]
    fn a_branch_carrying_a_traversal_refuses_and_names_the_row() {
        for hostile in ["../escape", "/leading", "feat/../..", "trailing/"] {
            let rows =
                FULL_ROWS.replace("feat/28-slice-1b-agent-surface-screensource-seam", hostile);
            let refusal = from_issue_json(&issue_json(&rows)).expect_err("must refuse");
            assert!(refusal.to_string().contains("branch"), "{refusal}");
        }
    }

    #[test]
    fn a_body_missing_the_anchor_row_refuses_and_names_that_row() {
        let rows: String = FULL_ROWS
            .lines()
            .filter(|l| !l.to_lowercase().contains("anchor artifact"))
            .collect::<Vec<_>>()
            .join("\n");
        let refusal = from_issue_json(&issue_json(&rows)).expect_err("must refuse");
        assert!(
            refusal.to_string().contains("anchor artifact"),
            "the refusal must name the missing row: {refusal}"
        );
        assert!(!refusal.to_string().to_lowercase().contains("invalid"));
    }

    #[test]
    fn the_plugin_pin_needs_both_halves_and_has_no_latest_to_spell() {
        assert!(PluginPin::parse("latest").is_err());
        assert!(
            PluginPin::parse("compound-engineering@compound-engineering-plugin latest").is_err()
        );
        assert!(
            PluginPin::parse("3.21.3").is_err(),
            "a bare version has no marketplace"
        );
        let pinned = PluginPin::parse("`compound-engineering@compound-engineering-plugin` 3.21.3")
            .expect("both halves");
        assert_eq!(pinned.name(), "compound-engineering");
        assert_eq!(pinned.marketplace(), "compound-engineering-plugin");
        assert_eq!(pinned.version(), "3.21.3");
    }

    #[test]
    fn the_plugin_resolves_under_the_pin() {
        let pin =
            PluginPin::parse("compound-engineering@compound-engineering-plugin 3.21.3").unwrap();
        assert_eq!(
            plugin_dir(Path::new("/home/op"), &pin),
            Path::new(
                "/home/op/.claude/plugins/cache/compound-engineering-plugin/compound-engineering/3.21.3"
            )
        );
    }

    #[test]
    fn the_full_table_reads_every_row_including_the_optional_ones() {
        let rows = format!("{FULL_ROWS}\n| Budget ceiling | $12.50 |\n| Model | claude-opus-5 |");
        let job = from_issue_json(&issue_json(&rows)).expect("a complete Job");
        assert_eq!(job.issue, 28);
        assert_eq!(job.target_repo, "FlorianRiquelme/snapper");
        assert_eq!(job.handoff_sha, "9d1f4c7a2b6e0538d4a17c9b3e5f8021ac6d4e77");
        assert_eq!(job.anchor, "docs/plans/2026-08-05-002-plan.md");
        assert_eq!(job.budget.as_deref(), Some("$12.50"));
        assert_eq!(job.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(job.labels, vec!["grind:queued".to_string()]);
    }

    #[test]
    fn a_budget_ceiling_becomes_a_cap_and_an_absent_one_does_not() {
        assert_eq!(spend_cap(Some("$12.50")).as_deref(), Some("12.50"));
        assert_eq!(spend_cap(Some("USD 8")).as_deref(), Some("8"));
        assert_eq!(spend_cap(Some("none")), None);
        assert_eq!(spend_cap(Some("-")), None);
        assert_eq!(spend_cap(None), None);
    }

    #[test]
    fn an_optional_row_reading_none_is_the_same_as_no_row() {
        let rows = format!("{FULL_ROWS}\n| Budget ceiling | none |\n| Model | - |");
        let job = from_issue_json(&issue_json(&rows)).expect("a complete Job");
        assert_eq!(job.budget, None);
        assert_eq!(job.model, None);
    }

    #[test]
    fn the_dirty_check_reads_any_output_as_dirty() {
        assert!(!is_dirty(""));
        assert!(!is_dirty("\n  \n"));
        assert!(is_dirty(" M src/cli.rs\n"));
        assert!(is_dirty("?? scratch.txt\n"));
    }

    #[test]
    fn the_branchs_worktree_is_picked_out_of_a_multi_worktree_listing() {
        let porcelain = "worktree /Users/op/Repos/mine/snapper\n\
             HEAD 1111111111111111111111111111111111111111\n\
             branch refs/heads/main\n\
             \n\
             worktree /Users/op/Repos/mine/snapper/.claude/worktrees/other\n\
             HEAD 2222222222222222222222222222222222222222\n\
             branch refs/heads/feat/27-something-else\n\
             \n\
             worktree /Users/op/Repos/mine/snapper/.claude/worktrees/slice-1b\n\
             HEAD 3333333333333333333333333333333333333333\n\
             branch refs/heads/feat/28-slice-1b\n";
        assert_eq!(
            adopt_worktree(porcelain, "feat/28-slice-1b"),
            Some(PathBuf::from(
                "/Users/op/Repos/mine/snapper/.claude/worktrees/slice-1b"
            ))
        );
        assert_eq!(adopt_worktree(porcelain, "feat/99-absent"), None);
    }

    #[test]
    fn a_detached_worktree_is_not_adopted_for_a_branch_it_does_not_hold() {
        let porcelain = "worktree /Users/op/Repos/mine/snapper\n\
             HEAD 1111111111111111111111111111111111111111\n\
             detached\n";
        assert_eq!(adopt_worktree(porcelain, "feat/28-slice-1b"), None);
    }

    #[test]
    fn a_head_off_the_handoff_sha_is_a_note_and_not_a_refusal() {
        assert_eq!(head_note("abc123", "abc123"), None);
        let note = head_note("9d1f4c7a2b6e", "1111111111").expect("a note");
        assert!(note.contains("9d1f4c7a"));
        assert!(note.contains("11111111"));
    }

    #[test]
    fn the_layout_is_the_declaration() {
        let home = Path::new("/home/op");
        assert_eq!(
            repo_path(home, "owner/name"),
            Path::new("/home/op/.grind/repos/owner/name")
        );
        assert_eq!(claude_bin(home), Path::new("/home/op/.grind/bin/claude"));
        assert_eq!(runs_dir(home), Path::new("/home/op/.grind/runs"));
        assert_eq!(locks_dir(home), Path::new("/home/op/.grind/locks"));
    }
}
