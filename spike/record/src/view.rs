//! The read path. `RunView` is what `grind status` and `grind list` get.
//!
//! Deliberately derives `Deserialize` only — never `Serialize`. There is no
//! `save`, `to_json`, `write`, or anything else that turns a `RunView` back
//! into bytes. That is the whole mechanism: the bug in `bin/grind`'s
//! `cmd_status` is `load()` -> `observe()` -> `save()` on the *same* dict. If
//! the type you hold after `load()` has no operation that writes, that
//! sequence is not a call you can make — it's a compile error, not a
//! discipline you have to remember. See `wont-compile/` for the proof.

use serde::Deserialize;
use std::path::Path;

use crate::types::{Attempt, Job, Observation};

#[derive(Debug, Clone, Deserialize)]
pub struct RunView {
    pub run_id: String,
    pub created_at: String,
    pub state: String,
    pub job: Job,
    pub plugin_dir: String,
    pub worktree: String,
    pub session_id: String,
    #[serde(default)]
    pub claude_bin: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    // Added after this fixture was written (docs/CONTEXT.md: "it names the
    // host holding it"). Absent in the real fixture — proves the default.
    #[serde(default)]
    pub hostname: Option<String>,
    pub denied_tools: Vec<String>,
    pub attempts: Vec<Attempt>,
    #[serde(default)]
    pub observed: Option<Observation>,
}

impl RunView {
    /// Read-only load. Returns a value with no path back to disk.
    pub fn load(path: &Path) -> Result<Self, String> {
        let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        serde_json::from_str(&raw).map_err(|e| format!("{path:?}: {e}"))
    }
}
