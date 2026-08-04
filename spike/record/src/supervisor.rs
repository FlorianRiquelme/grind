//! The write path. `RunRecord` is the only type in this crate with a `save`.
//!
//! Two extra layers on top of "wrong type has no save method":
//!
//! 1. `attempts` is a private field. The only mutator is `push_attempt`,
//!    which appends. There is no `set_attempts` and no `&mut Vec<Attempt>`
//!    getter, so "load a stale copy, then overwrite the whole vec" is not
//!    expressible even from *inside* the writable type — only "load, append
//!    what's new, save" is.
//! 2. `save` writes to a temp file in the same directory, then renames over
//!    the target. A crash between `write` and `rename` leaves the OLD
//!    `run.json` intact (the temp file is the only thing that can be half
//!    written); `json.dumps(state); write_text(...)` in the Python has no
//!    such guard — a crash mid-`write_text` truncates the real file. See
//!    `main.rs` for a demonstration and FINDINGS.md for how it was checked.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::types::{Attempt, Job, Observation};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RunRecord {
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
    #[serde(default)]
    pub hostname: Option<String>,
    pub denied_tools: Vec<String>,
    attempts: Vec<Attempt>,
    #[serde(default)]
    observed: Option<Observation>,
}

impl RunRecord {
    /// The supervisor's own load — same bytes on disk as `RunView::load`,
    /// but a different Rust type, one with `save` and append-only mutators.
    pub fn load(path: &Path) -> Result<Self, String> {
        let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        serde_json::from_str(&raw).map_err(|e| format!("{path:?}: {e}"))
    }

    pub fn attempts(&self) -> &[Attempt] {
        &self.attempts
    }

    /// The only way `attempts` changes. No `set_attempts`, no `&mut Vec<_>`
    /// getter — a caller cannot replace the history, only add to it.
    pub fn push_attempt(&mut self, attempt: Attempt) {
        self.attempts.push(attempt);
    }

    pub fn observed(&self) -> Option<&Observation> {
        self.observed.as_ref()
    }

    pub fn set_observed(&mut self, obs: Observation) {
        self.observed = Some(obs);
    }

    pub fn set_state(&mut self, state: impl Into<String>) {
        self.state = state.into();
    }

    /// Write-temp-then-rename: `rename` is atomic on the same filesystem, so
    /// a crash mid-write leaves either the old file or the new one, never a
    /// truncated hybrid of both.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let dir = path.parent().ok_or("run.json path has no parent dir")?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;

        let tmp: PathBuf = path.with_extension("json.tmp");
        let body = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        {
            let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
            f.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
            f.write_all(b"\n").map_err(|e| e.to_string())?;
            f.sync_all().map_err(|e| e.to_string())?; // flush to disk before the rename is visible
        }
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
        Ok(())
    }
}
