//! How a Job reference becomes everything a dispatch needs — the field table, the repo path,
//! the `claude` binary, the worktree choice.
//!
//! All four resolutions are one act, and all four are pure over text some child printed. The
//! fix for *nothing that touches git is tested* is pure parse functions over output text, not
//! a library (ADR-0007): `world` spawns, this module reads what came back, and every test here
//! runs from string literals with no `git` invocation anywhere.

use crate::runner;
use crate::world;
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
    /// One line on the work's **nature**, and never a requirement. A second place stating what
    /// the work *is* drifts from the Anchor, which is the same argument that keeps a declared
    /// branch contract out. No validator: it is prose.
    pub intent: Option<String>,
    pub model: Option<String>,
    /// The Job's own agent declaration: a profile name from `~/.grind/agents/`, or one
    /// full agent line (ADR-0017, composite amendment). Wins over the repo binding and
    /// the host default; `#[serde(default)]` for the same pre-cutover reason as
    /// `done_predicate` — absence genuinely means a record from before this row existed.
    #[serde(default)]
    pub agent: Option<String>,
    /// How the Run will know this is done, stated so a machine could grade it. Consumed by the
    /// stage machinery (ADR-0015) — the Plan stage inherits it, plan review grades it, the PR
    /// body renders its verdict; parsed and recorded here. `#[serde(default)]` because a record
    /// on disk from before this row existed carries none, the same reasoning `clearances`
    /// documents: absence genuinely means a pre-cutover record, never a blank answer.
    #[serde(default)]
    pub done_predicate: String,
    /// The merge target the PR opens against. Consumed by the stage machinery (ADR-0015),
    /// observed afterward as `pr_base_matches_declared`; parsed and recorded here.
    /// `#[serde(default)]` for the same pre-cutover reason as `done_predicate`.
    #[serde(default)]
    pub base_branch: String,
    /// The repo's own generic answer to "how do I check this", handed verbatim to the stage
    /// machinery (ADR-0015) — Work, Fixes, CI-babysit; parsed and recorded here.
    /// `#[serde(default)]` for the same pre-cutover reason as `done_predicate`.
    #[serde(default)]
    pub verify_entrypoint: String,
    /// Human-declared, never Grind-classified (ADR-0012) performance-sensitive paths. Optional:
    /// absent or blank is an empty list, never a refusal. Consumed by the stage machinery
    /// (ADR-0015) as an observed fact; parsed and recorded here. `#[serde(default)]` for the
    /// same pre-cutover reason as `done_predicate`.
    #[serde(default)]
    pub declared_hot_paths: Vec<String>,
}

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
/// a Run. The same holds for the JSON's own `number` and `url`: both render into every later
/// message, so an absent one is refused here rather than printed blank. `target repo` becomes
/// a filesystem path and `branch` becomes a lock filename, so both are validated as segments
/// before they leave this module — a Job is a private-repo artifact today, but a value that
/// reaches a path deserves a shape either way.
pub fn from_issue_json(raw: &str) -> Result<Job, Refusal> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| Refusal::saying(format!("`gh issue view` returned unreadable JSON: {e}")))?;

    let url = value
        .get("url")
        .and_then(|v| v.as_str())
        .filter(|u| !u.trim().is_empty())
        .ok_or_else(|| Refusal::saying("`gh issue view` returned no issue url".to_string()))?
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
    let intent = optional("intent");
    let model = optional("model");
    let agent = optional("agent");
    let done_predicate = required("done predicate")?;
    let base_branch = required("base branch")?;
    let verify_entrypoint = required("verify entrypoint")?;
    let declared_hot_paths = optional("declared hot paths")
        .map(|v| split_hot_paths(&v))
        .unwrap_or_default();

    Ok(Job {
        issue,
        url,
        title,
        labels,
        target_repo,
        branch,
        handoff_sha,
        anchor,
        intent,
        agent,
        model,
        done_predicate,
        base_branch,
        verify_entrypoint,
        declared_hot_paths,
    })
}

/// `Declared hot paths` splits on whitespace or commas, whichever the human wrote — a row
/// naming `src/hot.rs, src/other.rs` and one naming `src/hot.rs src/other.rs` mean the same
/// list, and nothing here requires the human to pick.
fn split_hot_paths(cell: &str) -> Vec<String> {
    cell.split([',', ' ', '\t'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
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
        let Some((key_cell, value_cell)) = line[1..line.len() - 1].split_once('|') else {
            continue;
        };
        let key = clean(key_cell).to_lowercase();
        let value = clean(value_cell);
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
/// A bare function beside the three validators, and not a newtype: the retired `PluginPin`
/// earned its type because `Latest` had to be unspellable (ADR-0006), and a SHA has no
/// forbidden spelling, only a required shape. No regex crate either — ADR-0005's single
/// dependency holds.
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

/// Where the omp harness CLI lives, mirroring [`claude_bin`] but layout-defaulting rather
/// than layout-naming: omp installs under bun's own bin directory, so that is the fallback
/// path, and `GRIND_OMP_BIN` is the one escape hatch — a host fact like any other, read
/// through `world` alone (ADR-0008).
pub fn omp_bin(home: &Path) -> String {
    world::var("GRIND_OMP_BIN").unwrap_or_else(|_| {
        home.join(".bun")
            .join("bin")
            .join("omp")
            .to_string_lossy()
            .into_owned()
    })
}

pub fn runs_dir(home: &Path) -> PathBuf {
    grind_dir(home).join("runs")
}

pub fn locks_dir(home: &Path) -> PathBuf {
    grind_dir(home).join("locks")
}

/// `~/.grind/agent` — the one-line backend declaration (ADR-0017), beside `bin/claude`:
/// the same directory whose **layout is the declaration**, now carrying which adapter a
/// Run executes. Absent means claude-code, the only backend there has ever been.
pub fn agent_file(home: &Path) -> PathBuf {
    grind_dir(home).join("agent")
}

/// The selection, read once at dispatch and snapshotted into the RunRecord (ADR-0017) —
/// resume loads the record and proceeds on the snapshot without ever re-reading this file.
///
/// An absent file is the default selection, not an error: a host that never heard of
/// backends has always run claude-code. Everything else that cannot be read is loud —
/// an unreadable file, or a line that does not parse, refuses by naming the path and the
/// problem, on the same register as an unreadable Job issue. A file with more than one
/// non-blank line is refused rather than silently first-read, for the same reason.
pub fn read_selection(home: &Path) -> Result<runner::Selection, String> {
    let path = agent_file(home);
    if !world::exists(&path) {
        return Ok(runner::Selection::default());
    }
    let text = world::read_to_string(&path)
        .map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let parsed = match lines.as_slice() {
        [] => runner::Selection::parse_line(""),
        [only] => runner::Selection::parse_line(only),
        _ => Err(format!("expected one line, found {}", lines.len())),
    };
    parsed.map_err(|e| format!("{}: {e}", path.display()))
}

/// `~/.grind/agents/` — the profile library (ADR-0017, composite amendment): one file per
/// profile, the same one-line grammar as `agent`. Doctor seeds the defaults once when the
/// directory is absent; user-editable, never rewritten.
pub fn agents_dir(home: &Path) -> PathBuf {
    grind_dir(home).join("agents")
}

/// `<repo>/agent` — the repo binding: one line, a profile name or a full agent line.
pub fn repo_agent_file(repo_path: &Path) -> PathBuf {
    repo_path.join("agent")
}

/// The profile file for `name`, whose shape is `[a-z0-9][a-z0-9-]*` — a directory
/// segment, so a name that would escape the library refuses here rather than at the fs.
pub fn profile_file(home: &Path, name: &str) -> Result<PathBuf, String> {
    let valid = !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid {
        return Err(format!(
            "invalid profile name {name:?} (expected [a-z0-9][a-z0-9-]*)"
        ));
    }
    Ok(agents_dir(home).join(name))
}

/// Exactly what doctor seeds when `~/.grind/agents` is absent: the workhorse default the
/// host already runs, and the opus-plan composite the epic exists to enable. Never
/// rewritten, never seeded over an existing directory.
pub const SEED_PROFILES: [(&str, &str); 2] = [
    (
        "glm",
        "omp fast=openrouter/z-ai/glm-5.3-flash strong=openrouter/z-ai/glm-5.3-flash",
    ),
    (
        "opus-plan",
        "omp fast=openrouter/z-ai/glm-5.3-flash strong=claude-code/claude-opus-5",
    ),
];

/// `true` = seeded now; `false` = the directory already existed. Only real fs errors are
/// errors; an existing library is never touched.
pub fn seed_agent_profiles(home: &Path) -> Result<bool, String> {
    let dir = agents_dir(home);
    if world::exists(&dir) {
        return Ok(false);
    }
    world::create_dir_all(&dir)?;
    for (name, line) in SEED_PROFILES {
        let path = profile_file(home, name)?;
        world::write(&path, &format!("{line}\n"))
            .map_err(|e| format!("could not seed profile {name:?}: {e}"))?;
    }
    Ok(true)
}

/// One line that is either a profile name (dereferenced through `~/.grind/agents/<name>`)
/// or a full agent line (its first token parses as a `Backend`). Loud on an invalid name,
/// a missing profile, an unreadable file, an unparseable line. An empty or blank line is
/// the default selection — the file names a deviation, nothing else.
pub fn deref_agent_line(home: &Path, line: &str) -> Result<runner::Selection, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return runner::Selection::parse_line("");
    }
    let first = trimmed.split_whitespace().next().unwrap_or_default();
    if runner::Backend::parse(first).is_ok() {
        return runner::Selection::parse_line(trimmed);
    }
    let tokens = trimmed.split_whitespace().count();
    if tokens > 1 {
        return Err(format!(
            "a profile name is one token, found {tokens} in {trimmed:?} \
             (write the profile name alone, or a full agent line)"
        ));
    }
    let path = profile_file(home, first)?;
    if !world::exists(&path) {
        return Err(format!(
            "agent profile {first:?} not found (no file at {})",
            path.display()
        ));
    }
    let text = world::read_to_string(&path)?;
    let declared = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .ok_or_else(|| format!("profile file {} carries no line", path.display()))?;
    runner::Selection::parse_line(declared).map_err(|e| format!("{}: {e}", path.display()))
}

/// Where the winning agent declaration came from. Banner and doctor observability only —
/// never serialized into any record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentSource {
    JobPin,
    Repo,
    Host,
}

/// The precedence chain resolved fresh at dispatch (never on resume): the Job's `Agent`
/// pin, else the repo's `agent` file, else the host default. The host file is read first
/// and loudly regardless of which tier wins — a malformed `~/.grind/agent` refuses even
/// when a Job pin would have won, because a host fact stays loud (ADR-0017).
#[derive(Debug, Clone)]
pub struct ResolvedAgent {
    pub selection: runner::Selection,
    pub source: AgentSource,
    /// The profile name when the winning declaration dereferenced one, else `None`.
    pub profile: Option<String>,
}

pub fn resolve_agent(
    home: &Path,
    repo_path: Option<&Path>,
    job_agent: Option<&str>,
) -> Result<ResolvedAgent, String> {
    let host = read_selection(home)?;
    if let Some(pin) = job_agent.filter(|pin| !pin.trim().is_empty()) {
        let selection = deref_agent_line(home, pin)?;
        return Ok(ResolvedAgent {
            selection,
            source: AgentSource::JobPin,
            profile: profile_name_of(home, pin),
        });
    }
    if let Some(repo) = repo_path {
        let file = repo_agent_file(repo);
        if world::exists(&file) {
            let line = world::read_to_string(&file)
                .map_err(|e| format!("could not read {}: {e}", file.display()))?;
            let derefed =
                deref_agent_line(home, &line).map_err(|e| format!("{}: {e}", file.display()))?;
            // A blank binding names no deviation — absence means the host default, so it
            // falls through rather than reading as a Repo declaration of "the default".
            if !line.trim().is_empty() {
                return Ok(ResolvedAgent {
                    selection: derefed,
                    source: AgentSource::Repo,
                    profile: profile_name_of(home, &line),
                });
            }
        }
    }
    Ok(ResolvedAgent {
        selection: host,
        source: AgentSource::Host,
        profile: None,
    })
}

/// The profile name a declaration dereferenced, when it did: a line whose first token is
/// neither blank nor a `Backend` names a profile. A full agent line names nothing.
fn profile_name_of(home: &Path, line: &str) -> Option<String> {
    let first = line.split_whitespace().next()?;
    if runner::Backend::parse(first).is_ok() {
        return None;
    }
    profile_file(home, first).ok().map(|_| first.to_string())
}

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
    /// The omp harness CLI resolves — `GRIND_OMP_BIN`, else `~/.bun/bin/omp` — and is
    /// executable. `Backend::Omp`'s requirement, the exact shape [`Check::ClaudeBinary`] is
    /// for `Backend::ClaudeCode`: demanded at dispatch depth only when that backend is
    /// declared, so an omp-less host can still dispatch either other backend.
    OmpBinary,
    OnPath(&'static str),
    GitVersionFloor,
    /// The ten stage skill directories under `~/.grind/skills/run` (ADR-0015), replacing the
    /// retired `PluginInstalled` check the moment nothing invoked the plugin anymore.
    SkillsPresent,
    /// The **first platform-branching check** in a list where every other one is a single
    /// command everywhere: `launchctl print` on darwin, `systemctl --user is-enabled` on linux.
    BootOneShot,
    GhAuthStore,
    SshKeyPassphraseless,
    SshKeyBothTypes,
    SigningConfig,
    CommitterIdentity,
    OriginOverSsh,
    /// A provider API key (`OPENROUTER_API_KEY` / `OPENAI_API_KEY`) is in the environment —
    /// presence only; both backends' readiness is reported regardless of which is selected (R9).
    AgentKeyPresent,
    /// The OpenAI-compatible endpoint answers a connection-level probe (`net::probe_endpoint`,
    /// R9). No key resolves the endpoint, so without one this check cannot even be tried.
    EndpointReachable,
    /// Free space, read with `df -Pk`, on the volume holding `~/.grind` and on the volume
    /// holding each declared clone — measured against [`DISK_HEADROOM_FLOOR_GIB`].
    DiskHeadroom,
    /// The `~/.grind/agents/` profile library (ADR-0017, composite amendment): seeded when
    /// absent, then every file in it must be a valid profile that parses. Doctor only.
    AgentProfiles,
    /// Every declared clone's `agent` file, when present, derefs to a readable agent
    /// selection. Absent files pass; a malformed one fails loudly naming the path.
    RepoAgentDeclarations,
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
            name: "omp binary",
            depth: Depth::Dispatch,
            check: Check::OmpBinary,
            doc_anchor: "`~/.bun/bin/omp` is executable, or `GRIND_OMP_BIN` names it.",
        },
        HostItem {
            name: "disk headroom",
            depth: Depth::Dispatch,
            check: Check::DiskHeadroom,
            doc_anchor: "The volumes holding `~/.grind` and each declared clone have room left.",
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
            name: "ps on PATH",
            depth: Depth::Dispatch,
            check: Check::OnPath("ps"),
            doc_anchor: "`ps` on `PATH`.",
        },
        HostItem {
            name: "stage skills present",
            depth: Depth::Dispatch,
            check: Check::SkillsPresent,
            doc_anchor: "The ten stage skill directories are present under `~/.grind/skills/run`.",
        },
        HostItem {
            name: "restart one-shot loaded",
            depth: Depth::Doctor,
            check: Check::BootOneShot,
            doc_anchor: "A restart one-shot calling `grind resume --all` is loaded.",
        },
        HostItem {
            name: "agent api key",
            depth: Depth::Doctor,
            check: Check::AgentKeyPresent,
            doc_anchor: "An agent API key is present in the environment.",
        },
        HostItem {
            name: "agent endpoint reachable",
            depth: Depth::Doctor,
            check: Check::EndpointReachable,
            doc_anchor: "The agent endpoint answers.",
        },
        HostItem {
            name: "agent profiles",
            depth: Depth::Doctor,
            check: Check::AgentProfiles,
            doc_anchor: "`~/.grind/agents/` holds one profile per file; doctor seeds `glm` and `opus-plan` when absent.",
        },
        HostItem {
            name: "repo agent files",
            depth: Depth::Doctor,
            check: Check::RepoAgentDeclarations,
            doc_anchor: "Each declared clone's `agent` file, when present, holds a profile name or one agent line.",
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
            name: "auto-update for claude",
            depth: Depth::Step,
            check: Check::NoBoolean,
            doc_anchor: "Auto-update for `claude`.",
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

/// The floor a disk-headroom reading must clear. Not invented here — inherited from #165's own
/// sizing note ("the batch's five concurrent walks suggest ≥5 GiB per concurrent Run as a
/// starting floor constant") and `docs/findings/0006-five-run-native-batch.md:183` ("host disk
/// hit 98% full mid-batch (~3 GiB free at peak); four of the five verifies survived only after
/// regenerable build caches were cleared by hand").
pub const DISK_HEADROOM_FLOOR_GIB: u64 = 5;

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
         | Done predicate | `just verify` is green and the screensource seam has a test |\n\
         | Base branch | main |\n\
         | Verify entrypoint | `just verify` |";

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
    fn the_full_table_reads_every_row_including_the_optional_ones() {
        let rows = format!(
            "{FULL_ROWS}\n| Model | claude-opus-5 |\n| Declared hot paths | src/decide.rs |"
        );
        let job = from_issue_json(&issue_json(&rows)).expect("a complete Job");
        assert_eq!(job.issue, 28);
        assert_eq!(job.target_repo, "FlorianRiquelme/snapper");
        assert_eq!(job.handoff_sha, "9d1f4c7a2b6e0538d4a17c9b3e5f8021ac6d4e77");
        assert_eq!(job.anchor, "docs/plans/2026-08-05-002-plan.md");
        assert_eq!(job.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(job.labels, vec!["grind:queued".to_string()]);
        assert_eq!(
            job.done_predicate,
            "just verify is green and the screensource seam has a test"
        );
        assert_eq!(job.base_branch, "main");
        assert_eq!(job.verify_entrypoint, "just verify");
        assert_eq!(job.declared_hot_paths, vec!["src/decide.rs".to_string()]);
    }

    #[test]
    fn a_job_still_carrying_a_budget_ceiling_row_parses_and_the_row_is_ignored() {
        let rows = format!("{FULL_ROWS}\n| Budget ceiling | $12.50 |");
        let job = from_issue_json(&issue_json(&rows)).expect("a readable Job");
        assert_eq!(job.anchor, "docs/plans/2026-08-05-002-plan.md");
        assert!(!format!("{job:?}").contains("12.50"));
    }

    #[test]
    fn an_optional_row_reading_none_is_the_same_as_no_row() {
        for blank in ["none", "-", "n/a", ""] {
            let rows = format!("{FULL_ROWS}\n| Model | {blank} |\n| Intent | {blank} |");
            let job = from_issue_json(&issue_json(&rows)).expect("a complete Job");
            assert_eq!(job.model, None, "model `{blank}`");
            assert_eq!(job.intent, None, "intent `{blank}`");
        }
    }

    #[test]
    fn an_intent_row_is_read_as_prose_with_no_validator_over_it() {
        let rows = format!("{FULL_ROWS}\n| Intent | A settled plan transcribed into one module. |");
        let job = from_issue_json(&issue_json(&rows)).expect("a complete Job");
        assert_eq!(
            job.intent.as_deref(),
            Some("A settled plan transcribed into one module.")
        );
        let bare = from_issue_json(&issue_json(FULL_ROWS)).expect("a complete Job");
        assert_eq!(bare.intent, None);
    }

    #[test]
    fn a_body_missing_a_new_required_row_refuses_and_names_that_row() {
        for row_name in ["done predicate", "base branch", "verify entrypoint"] {
            let rows: String = FULL_ROWS
                .lines()
                .filter(|l| !l.to_lowercase().contains(row_name))
                .collect::<Vec<_>>()
                .join("\n");
            let refusal = from_issue_json(&issue_json(&rows)).expect_err("must refuse");
            assert!(
                refusal.to_string().contains(row_name),
                "the refusal must name the missing row `{row_name}`: {refusal}"
            );
        }
    }

    #[test]
    fn a_present_but_blank_new_required_row_refuses_and_names_that_row() {
        for (needle, row_name) in [
            (
                "| Done predicate | `just verify` is green and the screensource seam has a test |",
                "done predicate",
            ),
            ("| Base branch | main |", "base branch"),
            ("| Verify entrypoint | `just verify` |", "verify entrypoint"),
        ] {
            for blank in ["", "none", "-", "n/a"] {
                let replacement = format!("| {} | {blank} |", capitalize_row(row_name));
                let rows = FULL_ROWS.replace(needle, &replacement);
                let refusal = from_issue_json(&issue_json(&rows)).expect_err("must refuse");
                assert!(
                    refusal.to_string().contains(row_name),
                    "the refusal must name the blank row `{row_name}` for `{blank}`: {refusal}"
                );
            }
        }
    }

    fn capitalize_row(row: &str) -> String {
        let mut chars = row.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => String::new(),
        }
    }

    #[test]
    fn declared_hot_paths_absent_is_an_empty_list() {
        let job = from_issue_json(&issue_json(FULL_ROWS)).expect("a readable Job");
        assert!(job.declared_hot_paths.is_empty());
    }

    #[test]
    fn declared_hot_paths_present_splits_on_whitespace_or_commas() {
        let rows = format!(
            "{FULL_ROWS}\n| Declared hot paths | src/decide.rs, src/policy.rs src/attempt.rs |"
        );
        let job = from_issue_json(&issue_json(&rows)).expect("a readable Job");
        assert_eq!(
            job.declared_hot_paths,
            vec!["src/decide.rs", "src/policy.rs", "src/attempt.rs"]
        );
    }

    #[test]
    fn declared_hot_paths_reading_none_is_the_same_as_no_row() {
        for blank in ["none", "-", "n/a", ""] {
            let rows = format!("{FULL_ROWS}\n| Declared hot paths | {blank} |");
            let job = from_issue_json(&issue_json(&rows)).expect("a readable Job");
            assert!(job.declared_hot_paths.is_empty(), "hot paths `{blank}`");
        }
    }

    #[test]
    fn a_target_repo_row_whose_value_carries_a_pipe_parses_as_one_row() {
        let table = field_table(&body(
            "| Target repo | FlorianRiquelme/snapper \\| mirrored at #12 |",
        ));
        assert_eq!(
            table,
            vec![(
                "target repo".to_string(),
                "FlorianRiquelme/snapper \\| mirrored at #12".to_string()
            )]
        );
        let bare = field_table(&body(
            "| Target repo | FlorianRiquelme/snapper | see docs |",
        ));
        assert_eq!(
            bare,
            vec![(
                "target repo".to_string(),
                "FlorianRiquelme/snapper | see docs".to_string()
            )]
        );
    }

    #[test]
    fn an_issue_json_without_a_usable_url_refuses_and_names_the_field() {
        for url in [None, Some("")] {
            let mut raw = serde_json::json!({
                "number": 28,
                "title": "Slice 1b",
                "body": body(FULL_ROWS),
            });
            if let Some(u) = url {
                raw["url"] = serde_json::json!(u);
            }
            let refusal = from_issue_json(&raw.to_string()).expect_err("must refuse");
            assert!(refusal.to_string().contains("url"), "{refusal}");
        }
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
    fn the_boot_one_shot_is_doctors_and_never_a_dispatch_precondition() {
        let item = host_items()
            .iter()
            .find(|i| i.check == Check::BootOneShot)
            .expect("the boot one-shot is listed");
        assert_eq!(item.depth, Depth::Doctor);
        assert!(
            !dispatch_subset().iter().any(|i| i.name == item.name),
            "no dispatch path may consult the boot one-shot"
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
        assert_eq!(agent_file(home), Path::new("/home/op/.grind/agent"));
        assert_eq!(runs_dir(home), Path::new("/home/op/.grind/runs"));
        assert_eq!(locks_dir(home), Path::new("/home/op/.grind/locks"));
    }

    #[test]
    fn the_omp_binary_is_a_dispatch_depth_item_of_its_own_backend() {
        let item = host_items()
            .iter()
            .find(|i| i.check == Check::OmpBinary)
            .expect("the omp binary is listed");
        assert_eq!(item.depth, Depth::Dispatch);
        assert!(
            dispatch_subset()
                .iter()
                .any(|i| i.check == Check::OmpBinary)
        );
    }

    #[test]
    fn disk_headroom_is_dispatch_depth_and_in_the_dispatch_subset() {
        let matches: Vec<&HostItem> = host_items()
            .iter()
            .filter(|i| i.check == Check::DiskHeadroom)
            .collect();
        assert_eq!(matches.len(), 1, "exactly one disk-headroom item");
        let item = matches[0];
        assert_eq!(item.depth, Depth::Dispatch);
        assert_eq!(
            dispatch_subset()
                .iter()
                .filter(|i| i.name == item.name)
                .count(),
            1
        );
        assert_eq!(DISK_HEADROOM_FLOOR_GIB, 5);
    }

    #[test]
    fn omp_bin_falls_back_to_buns_bin_directory_without_an_override() {
        let _guard = world::env_test_guard();
        world::remove_var_for_test("GRIND_OMP_BIN");
        assert_eq!(omp_bin(Path::new("/home/op")), "/home/op/.bun/bin/omp");
    }

    #[test]
    fn omp_bin_prefers_grind_omp_bin_over_the_layout_default() {
        let _guard = world::env_test_guard();
        world::set_var_for_test("GRIND_OMP_BIN", "/opt/omp/bin/omp");
        assert_eq!(omp_bin(Path::new("/home/op")), "/opt/omp/bin/omp");
        world::remove_var_for_test("GRIND_OMP_BIN");
    }

    /// A throwaway home with `~/.grind/agent` laid out, removed when the test ends.
    fn home_with_agent(line: &str) -> PathBuf {
        let home = world::temp_dir("read-selection");
        world::create_dir_all(&grind_dir(&home)).expect("a scratch .grind dir");
        world::write_atomic(&agent_file(&home), line).expect("a scratch agent file");
        home
    }

    #[test]
    fn an_absent_agent_file_is_the_default_selection() {
        let home = world::temp_dir("read-selection");
        let selection = read_selection(&home).expect("the default selection");
        assert_eq!(selection.backend, runner::Backend::default());
        assert_eq!(selection.endpoint_override, None);
        world::remove_tree(&home);
    }

    #[test]
    fn an_empty_agent_file_is_the_default_selection() {
        let home = home_with_agent("\n");
        let selection = read_selection(&home).expect("the default selection");
        assert_eq!(selection.backend, runner::Backend::default());
        world::remove_tree(&home);
    }

    #[test]
    fn a_native_line_parses_to_the_backend_and_its_override() {
        let home = home_with_agent("native https://example.invalid/v1\n");
        let selection = read_selection(&home).expect("a parsed selection");
        assert_eq!(selection.backend, runner::Backend::parse("native").unwrap());
        assert_eq!(
            selection.endpoint_override.as_deref(),
            Some("https://example.invalid/v1")
        );
        world::remove_tree(&home);
    }

    #[test]
    fn an_unparseable_line_refuses_and_names_the_path() {
        let home = home_with_agent("claude-code https://surprise.example\n");
        let refusal = read_selection(&home).expect_err("must refuse");
        assert!(
            refusal.contains("claude-code takes no arguments"),
            "{refusal}"
        );
        assert!(refusal.contains(".grind/agent"), "{refusal}");
        world::remove_tree(&home);
    }

    #[test]
    fn an_unknown_backend_token_refuses_and_names_the_path() {
        let home = home_with_agent("codex\n");
        let refusal = read_selection(&home).expect_err("must refuse");
        assert!(refusal.contains(".grind/agent"), "{refusal}");
        world::remove_tree(&home);
    }

    #[test]
    fn more_than_one_line_refuses_rather_than_reading_the_first() {
        let home = home_with_agent("native\ncodex\n");
        let refusal = read_selection(&home).expect_err("must refuse");
        assert!(refusal.contains("expected one line"), "{refusal}");
        world::remove_tree(&home);
    }

    /// A throwaway home with a profile library laid out, removed when the test ends.
    fn home_with_profiles(entries: &[(&str, &str)]) -> PathBuf {
        let home = world::temp_dir("agent-chain");
        world::create_dir_all(&agents_dir(&home)).expect("a scratch agents dir");
        for (name, line) in entries {
            world::write(&profile_file(&home, name).expect("a valid name"), line)
                .expect("a scratch profile");
        }
        home
    }

    #[test]
    fn profile_file_rejects_names_that_are_not_directory_segments() {
        for bad in ["", "-glm", "Glm", "glm_5", "../glm", "glm/opus", "glm "] {
            assert!(
                profile_file(Path::new("/h"), bad).is_err(),
                "{bad:?} must refuse"
            );
        }
        for good in ["glm", "opus-plan", "7b", "glm-5-3"] {
            assert!(
                profile_file(Path::new("/h"), good).is_ok(),
                "{good:?} must be a valid profile name"
            );
        }
    }

    #[test]
    fn seeding_writes_the_defaults_once_and_never_rewrites() {
        let home = world::temp_dir("agent-chain");
        assert_eq!(seed_agent_profiles(&home), Ok(true));
        assert_eq!(seed_agent_profiles(&home), Ok(false));
        for (name, line) in SEED_PROFILES {
            let text = world::read_to_string(&profile_file(&home, name).expect("a valid name"))
                .expect("a seeded profile");
            assert_eq!(text.trim(), line, "profile {name} seeds verbatim");
        }
        // An existing library is never touched: a profile the operator added survives, and
        // seeding a second time does not restore anything over it.
        world::write(
            &profile_file(&home, "custom").expect("a valid name"),
            "native\n",
        )
        .expect("a custom profile");
        assert_eq!(seed_agent_profiles(&home), Ok(false));
        assert!(world::exists(
            &profile_file(&home, "custom").expect("a valid name")
        ));
        world::remove_tree(&home);
    }

    #[test]
    fn a_profile_name_derefs_through_the_library_and_a_line_does_not() {
        let home = home_with_profiles(&[("glm", SEED_PROFILES[0].1)]);
        let derefed = deref_agent_line(&home, "glm").expect("a derefable profile");
        assert_eq!(derefed.backend, runner::Backend::parse("omp").unwrap());
        assert_eq!(
            derefed.own_ids().1,
            Some("openrouter/z-ai/glm-5.3-flash".to_string())
        );
        let direct = deref_agent_line(&home, "native https://example.invalid/v1")
            .expect("a full agent line");
        assert_eq!(direct.backend, runner::Backend::parse("native").unwrap());
        world::remove_tree(&home);
    }

    #[test]
    fn deref_refuses_loudly_on_an_invalid_name_a_missing_profile_and_an_unparsable_line() {
        let home = home_with_profiles(&[("bad", "codex\n")]);
        assert!(
            deref_agent_line(&home, "../escape").is_err(),
            "invalid name"
        );
        let missing = deref_agent_line(&home, "no-such-profile").expect_err("must refuse");
        assert!(missing.contains("no-such-profile"), "{missing}");
        let unparsable = deref_agent_line(&home, "bad").expect_err("must refuse");
        assert!(unparsable.contains("agents/bad"), "{unparsable}");
        world::remove_tree(&home);
    }

    #[test]
    fn a_blank_deref_line_is_the_default_selection() {
        let home = world::temp_dir("agent-chain");
        let selection = deref_agent_line(&home, "   ").expect("the default");
        assert_eq!(selection.backend, runner::Backend::default());
        assert_eq!(selection.routes.is_empty(), true);
        world::remove_tree(&home);
    }

    #[test]
    fn a_profile_row_with_trailing_prose_refuses_rather_than_reading_the_first_token() {
        let home = home_with_profiles(&[("glm", SEED_PROFILES[0].1)]);
        let refusal = deref_agent_line(&home, "glm (the cheap one)").expect_err("must refuse");
        assert!(refusal.contains("one token"), "{refusal}");
        world::remove_tree(&home);
    }

    #[test]
    fn the_chain_prefers_the_job_pin_then_the_repo_file_then_the_host() {
        let home = home_with_profiles(&[
            ("glm", SEED_PROFILES[0].1),
            ("opus-plan", SEED_PROFILES[1].1),
        ]);
        let repo = grind_dir(&home).join("repos").join("o").join("n");
        world::create_dir_all(&repo).expect("a scratch repo dir");

        // Host only.
        let resolved = resolve_agent(&home, Some(&repo), None).expect("the host tier");
        assert_eq!(resolved.source, AgentSource::Host);
        assert_eq!(resolved.profile, None);
        assert_eq!(resolved.selection.backend, runner::Backend::default());
        assert!(resolved.selection.routes.is_empty());

        // Repo file overrides the host; a profile name derefs.
        world::write(&repo_agent_file(&repo), "glm\n").expect("a repo binding");
        let resolved = resolve_agent(&home, Some(&repo), None).expect("the repo tier");
        assert_eq!(resolved.source, AgentSource::Repo);
        assert_eq!(resolved.profile.as_deref(), Some("glm"));
        assert_eq!(
            resolved.selection.backend,
            runner::Backend::parse("omp").unwrap()
        );

        // Job pin overrides both — and wins even with a full agent line.
        let resolved =
            resolve_agent(&home, Some(&repo), Some("native https://x.example/v1")).expect("pin");
        assert_eq!(resolved.source, AgentSource::JobPin);
        assert_eq!(resolved.profile, None);
        assert_eq!(
            resolved.selection.endpoint_override.as_deref(),
            Some("https://x.example/v1")
        );
        let resolved = resolve_agent(&home, Some(&repo), Some("opus-plan")).expect("pinned name");
        assert_eq!(resolved.source, AgentSource::JobPin);
        assert_eq!(resolved.profile.as_deref(), Some("opus-plan"));
        world::remove_tree(&home);
    }

    #[test]
    fn a_malformed_host_file_refuses_even_when_a_job_pin_would_win() {
        let home = world::temp_dir("agent-chain");
        world::create_dir_all(&grind_dir(&home)).expect("a scratch .grind dir");
        world::write(&agent_file(&home), "codex\n").expect("a malformed host line");
        let refusal = resolve_agent(&home, None, Some("native"))
            .expect_err("the host tier stays loud under a winning pin");
        assert!(refusal.contains(".grind/agent"), "{refusal}");
        world::remove_tree(&home);
    }

    #[test]
    fn a_blank_repo_agent_file_falls_through_to_the_host() {
        let home = world::temp_dir("agent-chain");
        let repo = grind_dir(&home).join("repos").join("o").join("n");
        world::create_dir_all(&repo).expect("a scratch repo dir");
        world::write(&repo_agent_file(&repo), "\n").expect("a blank binding");
        let resolved = resolve_agent(&home, Some(&repo), None).expect("the host tier");
        assert_eq!(resolved.source, AgentSource::Host);
        world::remove_tree(&home);
    }

    #[test]
    fn a_malformed_repo_agent_file_refuses_and_names_the_path() {
        let home = world::temp_dir("agent-chain");
        let repo = grind_dir(&home).join("repos").join("o").join("n");
        world::create_dir_all(&repo).expect("a scratch repo dir");
        world::write(&repo_agent_file(&repo), "codex\n").expect("a malformed binding");
        let refusal = resolve_agent(&home, Some(&repo), None).expect_err("must refuse");
        assert!(refusal.contains("repos/o/n/agent"), "{refusal}");
        world::remove_tree(&home);
    }
}
