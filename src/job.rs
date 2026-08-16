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
            .filter(|v| !is_blank_row(v))
            .ok_or_else(|| {
                Refusal::saying(format!(
                    "Job {url} has no usable `{row}` row in its field table"
                ))
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
    let handoff_sha = extract_handoff_sha(&required("handoff sha")?)?;
    let anchor = required("anchor artifact")?;
    validate_anchor(&anchor)?;
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

/// The `Handoff SHA` row is the one required row that still trusts the human's formatting —
/// Run 2's read `` `723ca91…` (`main` after #29) `` and every consumer took the whole cell.
/// Take the longest run of `[0-9a-f]` and accept it at a commit's length, 7 to 40.
///
/// A bare function beside the three validators, and not a newtype: `PluginPin` earns its type
/// because `Latest` must be unspellable (ADR-0006), and a SHA has no forbidden spelling, only
/// a required shape. No regex crate either — ADR-0005's single dependency holds.
fn extract_handoff_sha(cell: &str) -> Result<String, Refusal> {
    let mut longest = "";
    for run in cell.split(|c: char| !matches!(c, '0'..='9' | 'a'..='f')) {
        if run.len() > longest.len() {
            longest = run;
        }
    }
    if (7..=40).contains(&longest.len()) {
        return Ok(longest.to_string());
    }
    Err(Refusal::saying(format!(
        "the `handoff sha` row carries no run of 7 to 40 hex characters: {cell}"
    )))
}

fn validate_anchor(anchor: &str) -> Result<(), Refusal> {
    if anchor.starts_with('/') || anchor.ends_with('/') || !anchor.split('/').all(is_segment) {
        return Err(Refusal::saying(format!(
            "the `anchor artifact` row must be slash-separated segments with no path traversal: {anchor}"
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

// --- the host item list -------------------------------------------------------------------
//
// One list, checked at two depths: presence before every Dispatch, the full list by
// `grind doctor`. The list lives here because `job` already absorbs host resolution — repo
// path, worktree adoption, plugin directory, `claude` binary — and the dispatch-time subset is
// part of turning a Job reference into a dispatch.
//
// The tension is named rather than hidden: `grind doctor` takes no Job argument, so the list
// stretches this module's stated scope. The alternatives are worse — an eleventh module breaks
// the cut, and `world` holds no branching. Revisit if a second Job-independent concern lands
// here.

/// How an item is caught. `docs/provisioned-host.md` is the operative list and these three
/// marks are its depth model; the test below asserts each item carries the mark that document
/// gives it, because membership alone cannot catch a mis-marked item and that is the only
/// failure this list has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// Verified before every Dispatch — presence only, local, free, no network. Also verified,
    /// at full depth, by `grind doctor`.
    Dispatch,
    /// Verified by `grind doctor` alone, including the live checks.
    Doctor,
    /// Performed during provisioning, with no honest boolean behind it. Not checked, because
    /// every available check is a guess.
    Step,
}

/// What the driver has to do to answer for one item. `cli` walks the list, `world` runs what
/// each variant needs, and `observe` classifies the raw triples — so this stays data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    DeclaredClone,
    OneClonePerRepo,
    ClaudeBinary,
    OnPath(&'static str),
    GitVersionFloor,
    PluginInstalled,
    GhAuthStore,
    SshKeyPassphraseless,
    SshKeyBothTypes,
    SigningConfig,
    CommitterIdentity,
    OriginOverSsh,
    /// No honest boolean exists. Rendered as unchecked, with no boolean beside it.
    NoBoolean,
}

#[derive(Debug, Clone, Copy)]
pub struct HostItem {
    /// What the report calls it.
    pub name: &'static str,
    pub depth: Depth,
    pub check: Check,
    /// A distinctive fragment of this item's entry in `docs/provisioned-host.md`. It exists so
    /// the list and the document cannot drift apart silently.
    pub doc_anchor: &'static str,
}

/// The whole list, in the document's order.
pub fn host_items() -> &'static [HostItem] {
    &[
        HostItem {
            name: "declared clone",
            depth: Depth::Dispatch,
            check: Check::DeclaredClone,
            doc_anchor: "`repos/<owner>/<name>` exists and its `origin` matches the target repo.",
        },
        HostItem {
            name: "one clone per target repo",
            depth: Depth::Doctor,
            check: Check::OneClonePerRepo,
            doc_anchor: "One declared clone per target repo.",
        },
        HostItem {
            name: "claude binary",
            depth: Depth::Dispatch,
            check: Check::ClaudeBinary,
            doc_anchor: "`bin/claude` is executable and is not a shim.",
        },
        HostItem {
            name: "git on PATH",
            depth: Depth::Dispatch,
            check: Check::GitVersionFloor,
            doc_anchor: "`git` on `PATH`, ≥ 2.34.",
        },
        HostItem {
            name: "gh on PATH",
            depth: Depth::Dispatch,
            check: Check::OnPath("gh"),
            doc_anchor: "`gh` on `PATH`.",
        },
        HostItem {
            name: "just on PATH",
            depth: Depth::Doctor,
            check: Check::OnPath("just"),
            doc_anchor: "`just` on `PATH`.",
        },
        HostItem {
            name: "lfg plugin installed",
            depth: Depth::Dispatch,
            check: Check::PluginInstalled,
            doc_anchor: "The `lfg` plugin is installed.",
        },
        HostItem {
            name: "credential: gh auth store",
            depth: Depth::Doctor,
            check: Check::GhAuthStore,
            doc_anchor: "`gh auth login` — device-code flow",
        },
        HostItem {
            name: "credential: passphrase-less ssh key",
            depth: Depth::Doctor,
            check: Check::SshKeyPassphraseless,
            doc_anchor: "`ssh-keygen`, passphrase-less.",
        },
        HostItem {
            name: "credential: key uploaded for both types",
            depth: Depth::Doctor,
            check: Check::SshKeyBothTypes,
            doc_anchor: "`gh ssh-key add --type authentication`",
        },
        HostItem {
            name: "credential: ssh commit signing",
            depth: Depth::Doctor,
            check: Check::SigningConfig,
            doc_anchor: "`git config --global gpg.format ssh`",
        },
        HostItem {
            name: "credential: committer identity",
            depth: Depth::Doctor,
            check: Check::CommitterIdentity,
            doc_anchor: "`user.name` / `user.email` set to the machine identity",
        },
        HostItem {
            name: "credential: origin over ssh",
            depth: Depth::Doctor,
            check: Check::OriginOverSsh,
            doc_anchor: "`origin` on SSH, and the push",
        },
        HostItem {
            name: "the grind binary on PATH",
            depth: Depth::Step,
            check: Check::NoBoolean,
            doc_anchor: "The `grind` binary is on `PATH`.",
        },
        HostItem {
            name: "auto-update for claude and the plugin",
            depth: Depth::Step,
            check: Check::NoBoolean,
            doc_anchor: "Auto-update for `claude` and for the plugin.",
        },
        HostItem {
            name: "the dispatching user's $HOME",
            depth: Depth::Step,
            check: Check::NoBoolean,
            doc_anchor: "The dispatching user's `$HOME`.",
        },
    ]
}

/// What runs before every Dispatch: presence only, local, free, no network. A strict subset of
/// what doctor runs, from the same list — one definition of provisioned rather than two.
pub fn dispatch_subset() -> Vec<&'static HostItem> {
    host_items()
        .iter()
        .filter(|item| item.depth == Depth::Dispatch)
        .collect()
}

/// The floor `git` inherits from SSH commit signing. Not invented here — nothing else in Grind
/// needs a recent git.
pub const GIT_VERSION_FLOOR: (u64, u64) = (2, 34);

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

/// What Dispatch does about the worktree it adopted. One question — *does this worktree
/// contain the Handoff SHA* — and exactly one answer to it.
#[derive(Debug, Clone, PartialEq)]
pub enum Reachability {
    /// The worktree is at the Handoff SHA. Nothing to say.
    Proceed,
    /// Worth knowing, and never a gate.
    Note(String),
    /// Not the worktree the Job named. Incoherent input, in the dirty-worktree register.
    Refuse(Refusal),
}

/// Reachability from git's own exit statuses, replacing a string comparison that fired
/// identically when the worktree was harmlessly ahead and when it did not contain the Handoff
/// SHA at all. Run 2's worktree was behind and fast-forwardable at second zero, and the note
/// said the same thing it says when everything is fine.
///
/// `ancestor_exit` is `git merge-base --is-ancestor <handoff> HEAD`; `handoff_contains_head` is
/// the **reverse** call, which is the only thing that separates *behind* from *diverged* — exit
/// 1 from the forward call says only *not an ancestor*.
pub fn reachability(
    fetch_ok: bool,
    ancestor_exit: Option<i32>,
    handoff_contains_head: bool,
    head: &str,
    handoff_sha: &str,
) -> Reachability {
    let head = head.trim();
    let handoff = handoff_sha.trim();
    // The row to get right: a fetch that could not be made leaves the question unanswered, and
    // an unanswered question is neither a clean bill of health nor a refusal.
    if !fetch_ok {
        return Reachability::Note(format!(
            "could not fetch, so whether this worktree contains {} was not observed",
            short(handoff)
        ));
    }
    match ancestor_exit {
        Some(0) if same_commit(head, handoff) => Reachability::Proceed,
        Some(0) => Reachability::Note(format!(
            "worktree HEAD {} is ahead of Handoff SHA {}",
            short(head),
            short(handoff)
        )),
        // Grind never fast-forwards and never moves the worktree: the declared clone may be a
        // symlink to the human's own (ADR-0008), so a visible refusal is traded for an
        // invisible mutation deliberately.
        Some(1) if handoff_contains_head => Reachability::Refuse(Refusal::saying(format!(
            "worktree HEAD {} is behind Handoff SHA {} — fast-forward and re-dispatch",
            short(head),
            short(handoff)
        ))),
        Some(1) => Reachability::Refuse(Refusal::saying(format!(
            "Handoff SHA {} is not in the history of worktree HEAD {}",
            short(handoff),
            short(head)
        ))),
        Some(128) => Reachability::Refuse(Refusal::saying(format!(
            "Handoff SHA {} is not an object in the worktree at HEAD {}",
            short(handoff),
            short(head)
        ))),
        other => Reachability::Refuse(Refusal::saying(format!(
            "could not read whether the worktree contains {}: git merge-base exited {}",
            short(handoff),
            other.map_or_else(|| "on a signal".to_string(), |c| c.to_string())
        ))),
    }
}

/// Two spellings of one commit. The `Handoff SHA` row may legitimately carry an abbreviation —
/// the scan above accepts seven characters — so equality is a prefix over the shorter of the
/// two, asked only of a pair git has already called an ancestor.
fn same_commit(head: &str, handoff: &str) -> bool {
    let n = head.len().min(handoff.len());
    n >= 7 && head.as_bytes()[..n] == handoff.as_bytes()[..n]
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
    fn a_present_but_blank_handoff_sha_row_refuses_and_names_that_row() {
        for blank in ["", "none", "-", "n/a"] {
            let rows = FULL_ROWS.replace("`9d1f4c7a2b6e0538d4a17c9b3e5f8021ac6d4e77`", blank);
            let refusal = from_issue_json(&issue_json(&rows)).expect_err("must refuse");
            assert!(
                refusal.to_string().contains("handoff sha"),
                "the refusal must name the blank row: {refusal}"
            );
        }
    }

    #[test]
    fn a_handoff_sha_row_carrying_parenthetical_context_yields_the_bare_sha() {
        // The shape `docs/findings/0002` recorded, which every consumer took whole.
        let rows = FULL_ROWS.replace(
            "`9d1f4c7a2b6e0538d4a17c9b3e5f8021ac6d4e77`",
            "`723ca91` (`main` after #29)",
        );
        let job = from_issue_json(&issue_json(&rows)).expect("a readable Job");
        assert_eq!(job.handoff_sha, "723ca91");
    }

    #[test]
    fn a_bare_forty_character_sha_survives_the_scan_unchanged() {
        let job = from_issue_json(&issue_json(FULL_ROWS)).expect("a readable Job");
        assert_eq!(job.handoff_sha, "9d1f4c7a2b6e0538d4a17c9b3e5f8021ac6d4e77");
    }

    #[test]
    fn a_handoff_sha_row_with_no_hex_run_refuses_and_names_the_row() {
        let rows = FULL_ROWS.replace(
            "`9d1f4c7a2b6e0538d4a17c9b3e5f8021ac6d4e77`",
            "whichever tip the pull request sits on",
        );
        let refusal = from_issue_json(&issue_json(&rows)).expect_err("must refuse");
        assert!(
            refusal.to_string().contains("handoff sha"),
            "the refusal must name the row: {refusal}"
        );
        let lowered = refusal.to_string().to_lowercase();
        for quality in ["bad", "invalid", "wrong", "fail", "error", "reject"] {
            assert!(
                !lowered.contains(quality),
                "a refusal is incoherent input, never a judgement: {refusal}"
            );
        }
    }

    #[test]
    fn a_hex_run_shorter_than_a_short_sha_refuses_rather_than_truncating() {
        // Six characters is not a commit, and yielding it would hand `handoff..HEAD` a
        // revision that resolves to something else or to nothing.
        let rows = FULL_ROWS.replace("`9d1f4c7a2b6e0538d4a17c9b3e5f8021ac6d4e77`", "`abc123`");
        let refusal = from_issue_json(&issue_json(&rows)).expect_err("must refuse");
        assert!(refusal.to_string().contains("handoff sha"), "{refusal}");
    }

    #[test]
    fn an_anchor_carrying_a_traversal_refuses_and_names_the_row() {
        for hostile in ["../escape", "/leading", "docs/../..", "trailing/"] {
            let rows = FULL_ROWS.replace("docs/plans/2026-08-05-002-plan.md", hostile);
            let refusal = from_issue_json(&issue_json(&rows)).expect_err("must refuse");
            assert!(refusal.to_string().contains("anchor artifact"), "{refusal}");
        }
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

    // --- reachability, all six rows of the table ---------------------------------------------

    const HEAD_SHA: &str = "3333333333333333333333333333333333333333";
    const HANDOFF: &str = "9d1f4c7a2b6e0538d4a17c9b3e5f8021ac6d4e77";

    fn refusal_of(r: &Reachability) -> String {
        match r {
            Reachability::Refuse(refusal) => refusal.to_string(),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_worktree_at_the_handoff_sha_proceeds_silently() {
        assert_eq!(
            reachability(true, Some(0), true, HANDOFF, HANDOFF),
            Reachability::Proceed
        );
        // An abbreviated row is the same commit, not a different one.
        assert_eq!(
            reachability(true, Some(0), true, HANDOFF, "9d1f4c7"),
            Reachability::Proceed
        );
    }

    #[test]
    fn a_worktree_ahead_of_the_handoff_sha_proceeds_with_a_note() {
        let Reachability::Note(note) = reachability(true, Some(0), false, HEAD_SHA, HANDOFF) else {
            panic!("a worktree that contains the Handoff SHA and has moved on is a note");
        };
        assert!(note.contains("ahead"), "{note}");
    }

    #[test]
    fn a_worktree_behind_the_handoff_sha_refuses_saying_fast_forward() {
        // Run 2's opening condition: behind and fast-forwardable at second zero.
        let said = refusal_of(&reachability(true, Some(1), true, HEAD_SHA, HANDOFF));
        assert!(said.contains("fast-forward"), "{said}");
        assert!(
            said.contains("33333333") && said.contains("9d1f4c7a"),
            "{said}"
        );
    }

    #[test]
    fn a_handoff_sha_off_this_history_refuses_without_offering_a_fast_forward() {
        let said = refusal_of(&reachability(true, Some(1), false, HEAD_SHA, HANDOFF));
        assert!(!said.contains("fast-forward"), "{said}");
        assert!(said.contains("not in the history"), "{said}");
    }

    #[test]
    fn a_handoff_sha_that_is_not_an_object_here_refuses() {
        let said = refusal_of(&reachability(true, Some(128), false, HEAD_SHA, HANDOFF));
        assert!(said.contains("not an object"), "{said}");
    }

    #[test]
    fn a_failed_fetch_is_a_note_and_never_a_refusal_or_a_clean_bill_of_health() {
        // The row to get right. Every ancestor answer under a failed fetch is unobserved.
        for exit in [Some(0), Some(1), Some(128), None] {
            let observed = reachability(false, exit, false, HEAD_SHA, HANDOFF);
            let Reachability::Note(note) = observed else {
                panic!("a failed fetch is a note, got {observed:?}");
            };
            assert!(note.contains("not observed"), "{note}");
        }
    }

    #[test]
    fn a_merge_base_that_could_not_be_run_says_so_rather_than_passing() {
        let said = refusal_of(&reachability(true, None, false, HEAD_SHA, HANDOFF));
        assert!(said.contains("could not read"), "{said}");
    }

    #[test]
    fn no_reachability_answer_carries_a_quality_word() {
        let all = [
            reachability(true, Some(0), true, HANDOFF, HANDOFF),
            reachability(true, Some(0), false, HEAD_SHA, HANDOFF),
            reachability(true, Some(1), true, HEAD_SHA, HANDOFF),
            reachability(true, Some(1), false, HEAD_SHA, HANDOFF),
            reachability(true, Some(128), false, HEAD_SHA, HANDOFF),
            reachability(false, Some(0), false, HEAD_SHA, HANDOFF),
            reachability(true, None, false, HEAD_SHA, HANDOFF),
        ];
        for answer in &all {
            let said = match answer {
                Reachability::Proceed => String::new(),
                Reachability::Note(note) => note.clone(),
                Reachability::Refuse(refusal) => refusal.to_string(),
            }
            .to_lowercase();
            for quality in ["bad", "invalid", "wrong", "fail", "error", "reject"] {
                assert!(!said.contains(quality), "{said}");
            }
        }
    }

    // --- the host item list ---------------------------------------------------------------

    // The operative list arrives through `include_str!` rather than the filesystem: reading it
    // at run time would name `std::fs` inside `src/`, which `tests/topology.rs` forbids.
    const PROVISIONED_HOST: &str = include_str!("../docs/provisioned-host.md");

    /// Every entry in the document that carries a depth mark, reassembled from its wrapped
    /// lines, paired with the mark it carries.
    fn document_entries() -> Vec<(String, Depth)> {
        let mut entries = Vec::new();
        let mut current: Option<String> = None;
        let mut in_credentials = false;
        for line in PROVISIONED_HOST.lines() {
            if line.starts_with("## ") {
                in_credentials = line.contains("Credentials");
            }
            let starts_entry = line.starts_with("- **")
                || (in_credentials
                    && line.starts_with(|c: char| c.is_ascii_digit())
                    && line.contains(". "));
            if starts_entry {
                if let Some(entry) = current.take() {
                    push_entry(&mut entries, entry, in_credentials);
                }
                current = Some(line.to_string());
            } else if line.starts_with("  ") {
                if let Some(entry) = current.as_mut() {
                    entry.push(' ');
                    entry.push_str(line.trim());
                }
            } else if let Some(entry) = current.take() {
                push_entry(&mut entries, entry, in_credentials);
            }
        }
        if let Some(entry) = current.take() {
            push_entry(&mut entries, entry, in_credentials);
        }
        entries
    }

    fn push_entry(entries: &mut Vec<(String, Depth)>, entry: String, in_credentials: bool) {
        // The credential section marks all six of its steps at once, in prose above them:
        // "All *doctor*, never *dispatch*".
        if in_credentials && entry.starts_with(|c: char| c.is_ascii_digit()) {
            entries.push((entry, Depth::Doctor));
        } else if entry.contains("— *dispatch") {
            entries.push((entry, Depth::Dispatch));
        } else if entry.contains("— *doctor*") {
            entries.push((entry, Depth::Doctor));
        } else if entry.contains("— *step*") {
            entries.push((entry, Depth::Step));
        }
    }

    #[test]
    fn every_item_carries_the_mark_the_document_gives_it() {
        // Membership alone cannot catch a mis-marked item, and a mis-marked item is the only
        // failure this list has: an item quietly demoted from *dispatch* to *doctor* stops
        // running before a Dispatch and nothing says so.
        let entries = document_entries();
        assert_eq!(
            entries.len(),
            host_items().len(),
            "docs/provisioned-host.md carries {} marked entries and the list holds {}",
            entries.len(),
            host_items().len()
        );
        for item in host_items() {
            let matching: Vec<&(String, Depth)> = entries
                .iter()
                .filter(|(text, _)| text.contains(item.doc_anchor))
                .collect();
            assert_eq!(
                matching.len(),
                1,
                "`{}` must match exactly one entry in docs/provisioned-host.md; its anchor \
                 `{}` matched {}",
                item.name,
                item.doc_anchor,
                matching.len()
            );
            assert_eq!(
                matching[0].1, item.depth,
                "`{}` is marked {:?} here and {:?} in docs/provisioned-host.md",
                item.name, item.depth, matching[0].1
            );
        }
    }

    #[test]
    fn the_dispatch_subset_is_a_strict_subset_of_one_list() {
        let subset = dispatch_subset();
        assert!(!subset.is_empty());
        assert!(
            subset.len() < host_items().len(),
            "a subset that is the whole list is not one"
        );
        for item in &subset {
            assert!(
                host_items().iter().any(|whole| whole.name == item.name),
                "the dispatch subset must be drawn from the one list"
            );
            assert_eq!(item.depth, Depth::Dispatch);
        }
    }

    #[test]
    fn a_host_missing_just_fails_doctor_and_passes_the_dispatch_subset() {
        let just = host_items()
            .iter()
            .find(|i| i.name == "just on PATH")
            .expect("just is listed");
        assert_eq!(just.depth, Depth::Doctor);
        assert!(
            !dispatch_subset().iter().any(|i| i.name == "just on PATH"),
            "`just` is doctor's, not dispatch's — the failure is the Run's, not the Dispatch's"
        );
    }

    #[test]
    fn items_with_no_honest_boolean_carry_no_check() {
        for item in host_items().iter().filter(|i| i.depth == Depth::Step) {
            assert_eq!(
                item.check,
                Check::NoBoolean,
                "`{}` is marked *step*, so every available check is a guess",
                item.name
            );
        }
        for item in host_items().iter().filter(|i| i.depth != Depth::Step) {
            assert_ne!(
                item.check,
                Check::NoBoolean,
                "`{}` claims a check it has not got",
                item.name
            );
        }
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
