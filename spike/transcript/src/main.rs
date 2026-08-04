//! Spike: does a compiled, statically-typed Rust base survive Claude Code's session
//! transcript format — undocumented, heterogeneous, and observed here to change field
//! names (`session_id` vs `sessionId`) and field types across lines of the SAME file?
//!
//! Grind's status view needs two things out of a transcript:
//!   - `attributionSkill`: the name of the skill running "now" (the newest line that has it).
//!   - progress mtime: the newest mtime across the parent transcript AND its fanned-out
//!     subagent transcripts, because a fan-out makes the PARENT go quiet while subagents
//!     work — parent-only mtime misreads a healthy fan-out as a stall (see FINDINGS.md).
//!
//! Same extraction, three ways, over the same real files:
//!   (a) `strict`   — naive derive, no `Option`, no `default`. Breaks on real data.
//!   (b) `defaulted`— derive with `Option<T>` + `#[serde(default)]` everywhere.
//!   (c) `tolerant` — hand-written lookups over `serde_json::Value`. Never errors.
//!
//! See FINDINGS.md for the measured verdict. Read real transcripts read-only; only
//! copies under fixtures/ are ever damaged.

use std::fs;
use std::path::Path;
use std::time::SystemTime;

// ---------------------------------------------------------------------------
// (a) STRICT — the naive approach. A dev who has only seen a handful of lines
// writes down what looked like the stable shape. No `Option`, no `default`.
// ---------------------------------------------------------------------------
mod strict {
    use serde::Deserialize;

    #[derive(Deserialize)]
    pub struct Line {
        #[serde(rename = "type")]
        pub line_type: String,
        #[serde(rename = "sessionId")]
        pub session_id: String,
        #[serde(rename = "attributionSkill")]
        pub attribution_skill: String,
    }

    /// Aborts the WHOLE file on the first line that doesn't match. Returns the
    /// verbatim serde error text — this is what "strict derive on real data" produces.
    pub fn now_skill(text: &str) -> Result<Option<String>, String> {
        let mut last = None;
        for (i, raw) in text.lines().enumerate() {
            if raw.trim().is_empty() {
                continue;
            }
            let line: Line = serde_json::from_str(raw)
                .map_err(|e| format!("line {}: {}", i + 1, e))?;
            if !line.attribution_skill.is_empty() {
                last = Some(line.attribution_skill);
            }
        }
        Ok(last)
    }
}

// ---------------------------------------------------------------------------
// (b) DEFAULTED — same shape, but every field is `Option<T>` with
// `#[serde(default)]`. Missing/renamed fields degrade to `None` for free.
// A genuinely malformed line (not JSON, or a field whose TYPE changed) still
// makes `from_str` return `Err` for that line — `default` only fills in
// absent keys, it does not coerce a present-but-wrong-shaped value.
// ---------------------------------------------------------------------------
mod defaulted {
    use serde::Deserialize;

    #[derive(Deserialize, Default)]
    pub struct Line {
        #[serde(rename = "type", default)]
        pub line_type: Option<String>,
        #[serde(rename = "sessionId", default)]
        pub session_id: Option<String>,
        #[serde(rename = "attributionSkill", default)]
        pub attribution_skill: Option<String>,
    }

    /// Per-line `match`, not a bare `?` — this loop IS the extra cost. Skip a line
    /// that fails to parse instead of aborting the file; still loses every OTHER
    /// field on that one line, because a single bad field poisons the whole struct.
    pub fn now_skill(text: &str) -> (Option<String>, Vec<String>) {
        let mut last = None;
        let mut skipped = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            if raw.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Line>(raw) {
                Ok(line) => {
                    if let Some(skill) = line.attribution_skill {
                        if !skill.is_empty() {
                            last = Some(skill);
                        }
                    }
                }
                Err(e) => skipped.push(format!("line {}: {}", i + 1, e)),
            }
        }
        (last, skipped)
    }
}

// ---------------------------------------------------------------------------
// (c) TOLERANT — hand-written lookups over `serde_json::Value`. Every accessor
// returns an `Option`/three-valued read; nothing here can `panic!` or bubble a
// parse error out of a single line. A bad line loses only the fields on that
// line that were actually bad, not its siblings.
// ---------------------------------------------------------------------------
mod tolerant {
    use serde_json::Value;

    /// One line's contribution to "now". Distinguishes "no such field" from
    /// "field present but not the shape we expected" — useful for the report,
    /// pointless for the status line itself (both mean "no skill here").
    pub enum SkillRead {
        Present(String),
        Absent,
        WrongType,
    }

    pub fn skill_of(line: &str) -> (SkillRead, Option<String>) {
        let value: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => return (SkillRead::Absent, Some(format!("not JSON: {e}"))),
        };
        match value.get("attributionSkill") {
            None => (SkillRead::Absent, None),
            Some(Value::String(s)) if !s.is_empty() => {
                (SkillRead::Present(s.clone()), None)
            }
            Some(Value::String(_)) => (SkillRead::Absent, None),
            Some(_) => (
                SkillRead::WrongType,
                Some("attributionSkill present but not a string".to_string()),
            ),
        }
    }

    /// Never returns `Err`. A damaged file just yields fewer usable lines.
    pub fn now_skill(text: &str) -> (Option<String>, Vec<String>) {
        let mut last = None;
        let mut notes = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            if raw.trim().is_empty() {
                continue;
            }
            let (read, note) = skill_of(raw);
            if let Some(n) = note {
                notes.push(format!("line {}: {}", i + 1, n));
            }
            if let SkillRead::Present(s) = read {
                last = Some(s);
            }
        }
        (last, notes)
    }
}

// ---------------------------------------------------------------------------
// Progress mtime: newest mtime across the parent transcript AND its
// subagents/*.jsonl fan-out. Filesystem-only — independent of which of the
// three JSON approaches above is in use. Degrades to `Unknown` on any missing
// path; never panics, never propagates an error out of `main`.
// ---------------------------------------------------------------------------
enum Progress {
    Known(SystemTime),
    Unknown(String),
}

fn mtime_of(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

/// `parent_jsonl` is `<dir>/<uuid>.jsonl`. Its fan-out, if any, lives at
/// `<dir>/<uuid>/subagents/*.jsonl` — confirmed empirically (see FINDINGS.md);
/// `isSidechain: true` never appears in a PARENT transcript's own lines, only
/// inside the separate subagent files.
fn progress_mtime(parent_jsonl: &Path) -> Progress {
    let mut newest = match mtime_of(parent_jsonl) {
        Some(t) => t,
        None => return Progress::Unknown(format!("no such file: {}", parent_jsonl.display())),
    };
    let stem = match parent_jsonl.file_stem() {
        Some(s) => s,
        None => return Progress::Known(newest),
    };
    let subagents_dir = parent_jsonl
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(stem)
        .join("subagents");
    if let Ok(entries) = fs::read_dir(&subagents_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().map(|e| e == "jsonl").unwrap_or(false) {
                if let Some(t) = mtime_of(&p) {
                    if t > newest {
                        newest = t;
                    }
                }
            }
        }
    }
    // No subagents dir at all is not an error: most sessions never fan out.
    Progress::Known(newest)
}

// ---------------------------------------------------------------------------
// main: surface state for real transcripts and for every damaged fixture.
// ---------------------------------------------------------------------------
fn main() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");

    println!("=== real transcripts: extracted \"now\" skill, all three approaches ===");
    let real_files = [
        "real-small-1.jsonl",
        "real-small-2.jsonl",
        "real-parent-heterogeneous.jsonl",
    ];
    for name in real_files {
        let path = fixtures.join(name);
        report_real(&path);
    }

    println!();
    println!("=== fan-out progress mtime (parent alone vs parent+subagents) ===");
    let fanout_parent = fixtures
        .join("real-fanout-session")
        .join("00366bef-326e-441a-8130-de0b9843c067.jsonl");
    match mtime_of(&fanout_parent) {
        Some(t) => println!("  parent-only mtime:        {t:?}"),
        None => println!("  parent-only mtime:        could not observe (no such file)"),
    }
    match progress_mtime(&fanout_parent) {
        Progress::Known(t) => println!("  parent+subagents mtime:   {t:?}"),
        Progress::Unknown(reason) => println!("  parent+subagents mtime:   could not observe ({reason})"),
    }

    println!();
    println!("=== damaged fixtures: which approach degrades, which aborts ===");
    let damaged = [
        "empty.jsonl",
        "not-json.jsonl",
        "truncated.jsonl",
        "renamed-field.jsonl",
        "type-changed.jsonl",
        "does-not-exist.jsonl",
    ];
    for name in damaged {
        report_damaged(&fixtures, name);
    }
}

fn report_real(path: &Path) {
    let display = path.file_name().unwrap().to_string_lossy();
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            println!("{display}: could not read ({e})");
            return;
        }
    };
    let a = strict::now_skill(&text);
    let (b_skill, b_skipped) = defaulted::now_skill(&text);
    let (c_skill, c_notes) = tolerant::now_skill(&text);

    println!("{display}:");
    match a {
        Ok(skill) => println!("  (a) strict:    Ok({skill:?})"),
        Err(e) => println!("  (a) strict:    Err({e:?})"),
    }
    println!(
        "  (b) defaulted: Some({b_skill:?}), {} line(s) skipped",
        b_skipped.len()
    );
    println!(
        "  (c) tolerant:  Some({c_skill:?}), {} note(s)",
        c_notes.len()
    );
}

fn report_damaged(fixtures_dir: &Path, name: &str) {
    let path = fixtures_dir.join(name);
    println!("--- {name} ---");

    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            // Missing/unreadable file: every approach degrades identically, at the
            // file level, before any of the three parsers ever runs.
            println!("  (a) strict:    Err(could not read: {e})");
            println!("  (b) defaulted: Err(could not read: {e})");
            println!("  (c) tolerant:  Err(could not read: {e}) -- degraded, not panicked");
            return;
        }
    };

    match strict::now_skill(&text) {
        Ok(skill) => println!("  (a) strict:    Ok({skill:?})  [degraded]"),
        Err(e) => println!("  (a) strict:    Err({e:?})  [ABORTED]"),
    }

    let (b_skill, b_skipped) = defaulted::now_skill(&text);
    println!(
        "  (b) defaulted: Ok({b_skill:?}), {} line(s) skipped  [degraded]",
        b_skipped.len()
    );

    let (c_skill, c_notes) = tolerant::now_skill(&text);
    println!(
        "  (c) tolerant:  Ok({c_skill:?}), {} note(s)  [degraded]",
        c_notes.len()
    );
}
