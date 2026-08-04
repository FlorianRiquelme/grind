//! Shapes shared by the read-only view and the writable record.
//!
//! `run.json` is Grind's OWN format (not a third-party transcript), so strict
//! all-or-nothing serde is a legitimate default here: a field that is genuinely
//! required (run_id, state, job...) SHOULD fail loudly if missing, because a
//! record missing it is not a record grind can reason about.
//!
//! The one real risk strict serde creates: a record written by an OLDER grind,
//! read by a NEWER one, after a field was added. `claude_bin` and `hostname`
//! are exactly that case in the real fixture — see fixtures/run.json, which has
//! neither key. They are modelled as `Option<T>` with `#[serde(default)]` so an
//! old record still parses; anything that was required from day one stays
//! required and un-defaulted, so a genuinely corrupt/foreign record still hard
//! errors instead of silently loading with nonsense zeroed fields.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
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
    pub plugin_spec: String,
    pub plugin_version: String,
    pub plugin_name: String,
    pub plugin_marketplace: String,
    // present on the model field too, but always was — kept optional to match
    // the Python's `.get("model")`.
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Attempt {
    pub n: u32,
    pub mode: String,
    pub started_at: String,
    pub ended_at: String,
    pub exit_code: i32,
    pub is_error: bool,
    pub subtype: Option<String>,
    pub stop_reason: Option<String>,
    pub api_error_status: Option<String>,
    pub terminal_reason: Option<String>,
    pub num_turns: Option<u32>,
    pub total_cost_usd: Option<f64>,
    // `usage` is a large, ad-hoc nested blob the supervisor never inspects
    // field-by-field — round-trip it opaquely rather than modelling every key.
    #[serde(default)]
    pub usage: Option<serde_json::Value>,
    #[serde(default)]
    pub permission_denials: Vec<serde_json::Value>,
    pub done_promise: bool,
    pub rate_limited: bool,
    pub result_tail: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Pr {
    pub number: u64,
    pub url: String,
    pub state: String,
    #[serde(rename = "isDraft")]
    pub is_draft: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VerifyContract {
    pub justfile: bool,
    pub present: Vec<String>,
    pub missing: Vec<String>,
}

/// What `observe()` produces: a fresh reading of the world, not yet attached
/// to any record. The status path and the supervisor path both compute one of
/// these from the *same* function — what they're each allowed to DO with it
/// afterwards is where the two designs diverge.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Observation {
    pub observed_at: String,
    pub commits_ahead: i64,
    pub plan_files: Vec<String>,
    pub residual_findings: Vec<String>,
    pub ledger_entries: Vec<String>,
    pub pr: Option<Pr>,
    pub verify_contract: VerifyContract,
    pub furthest_stage: String,
}
