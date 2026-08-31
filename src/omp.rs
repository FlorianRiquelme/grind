//! Adapter #3: the omp harness CLI behind the StageRunner seam (#176).
//!
//! Spawns `omp -p --mode json --auto-approve` per attempt over a per-stage session
//! directory (`run_dir/sessions/<stage-session-id>/`), harvests the child's own session
//! transcript into the Run's evidence tree, and classifies the JSONL frame stream with
//! the same tolerance rules as [`crate::claude`]. One deliberate divergence from the
//! claude-code adapter: the invocation's argv is **ignored** — omp exposes no
//! denial-carrier flags worth preserving, so the argv is rebuilt from the [`RunSpec`]
//! alone and only the prompt text crosses over from the built
//! [`crate::attempt::Invocation`]. Everything doc-level about omp was proven
//! empirically on v18.0.6 before this adapter existed (contract P0 observations).

use crate::attempt::{Attempt, DONE_PROMISE, Mode, Transcript, mentions_limit, normalise, text_at};
use crate::observe::{Observed, Reason, native_freshness};
use crate::runner::{Backend, ModelClass, RunSpec, StageModel, StageRunner};
use crate::view::{Fanout, Live, one_line};
use crate::world;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

impl StageRunner for crate::runner::OmpAdapter {
    fn backend(&self) -> Backend {
        Backend::Omp
    }

    /// The claude-code adapter's sequence with omp shapes: the per-stage session
    /// directory created up front, the argv rebuilt from the spec (never read off the
    /// invocation), prompt file first, then the recorded spawn, then the transcript
    /// harvest, classification and the fan-out arithmetic over the lines appended to
    /// this stage's transcript since it began.
    fn run(&self, spec: &RunSpec) -> Attempt {
        let n = spec.attempt_n;
        let mode = spec.invocation.mode();
        let started_at = world::now_iso();
        let started = SystemTime::now();
        let stage_dir = spec.run_dir.join("sessions").join(spec.session_id);
        // The child creates the directory too, but a missing directory makes every
        // harvest below read the empty answer; one `mkdir -p` keeps the evidence tree
        // shaped even for a child that dies at startup.
        let _ = world::create_dir_all(&stage_dir);
        let already_written = stage_lines(&stage_dir);

        let mut argv = vec![
            self.bin.clone(),
            "-p".to_string(),
            "--mode".to_string(),
            "json".to_string(),
            "--auto-approve".to_string(),
        ];
        if let Some(id) = model_flag(
            spec.model,
            self.fast_model.as_deref(),
            self.strong_model.as_deref(),
            &self.routes,
        ) {
            argv.push("--model".to_string());
            argv.push(id);
        }
        argv.push("--session-dir".to_string());
        argv.push(format!("{}/", stage_dir.display()));
        // Resume rides the same dispatch argv plus one flag; with no prior session file
        // on disk there is nothing to resume and the plain dispatch shape is correct.
        if let Some(suffix) = match mode {
            Mode::Resume | Mode::CiBabysit => newest_stage_file(&stage_dir).and_then(|file| {
                let name = file.file_name()?.to_str()?.to_string();
                resume_suffix(&name).map(str::to_string)
            }),
            Mode::Dispatch => None,
        } {
            argv.push("--resume".to_string());
            argv.push(suffix.to_string());
        }

        let prefix = spec.file_label.as_str();
        let prompt_path = spec.run_dir.join(format!("{prefix}-{n}.prompt.txt"));
        let stdout_path = spec.run_dir.join(format!("{prefix}-{n}.stdout.json"));
        let stderr_path = spec.run_dir.join(format!("{prefix}-{n}.stderr.log"));
        // Every death is diagnosable from Run state alone: a prompt that never landed
        // on disk is a failed attempt, not a child spawned without its instruction.
        let outcome = match world::write(&prompt_path, spec.invocation.prompt()) {
            Ok(()) => world::spawn_recorded(
                &argv,
                spec.cwd,
                spec.invocation.prompt(),
                &stdout_path,
                &stderr_path,
            ),
            Err(e) => Err(format!("could not write the prompt: {e}")),
        };
        let ended_at = world::now_iso();

        let (stdout, stderr, code) = match outcome {
            Ok(code) => (
                world::read_to_string(&stdout_path).unwrap_or_default(),
                world::read_to_string(&stderr_path).unwrap_or_default(),
                code,
            ),
            Err(reason) => (reason, String::new(), None),
        };

        let mut attempt = classify(&stdout, &stderr, code, n, mode, &started_at, &ended_at);
        let (transcript, fanout) = match harvest(spec.run_dir, spec.session_id, &stage_dir, started)
        {
            Ok(copied) => (
                Transcript::Recorded(copied.recorded_name),
                fanout_since_text(&copied.text, already_written),
            ),
            Err(said) => (
                Transcript::WroteNone,
                Observed::Unobservable(Reason::saying(&format!(
                    "no stage transcript to read: {said}"
                ))),
            ),
        };
        attempt.transcript = transcript;
        attempt.with_fanout(fanout)
    }
}
/// The `--model` flag this stage-model resolves to, or `None` for no flag at all.
///
/// [`StageModel::native_id`]'s semantics with one omp twist: a job pin crosses verbatim
/// and a route naming omp resolves its class — a `None` id omitting the flag instead of
/// falling back to a provider id grind invented — but an **undeclared** class omits the
/// flag too; the harness's own default model is the honest answer, not
/// [`crate::runner::DEFAULT_MODEL`].
fn model_flag(
    model: &StageModel,
    fast: Option<&str>,
    strong: Option<&str>,
    routes: &crate::runner::ClassRoutes,
) -> Option<String> {
    let class = match model {
        StageModel::Pinned(id) => return Some(id.clone()),
        StageModel::Class(class) => *class,
    };
    match routes.for_class(class) {
        Some(crate::runner::Route {
            backend: crate::runner::Backend::Omp,
            id,
        }) => id.clone(),
        _ => match class {
            ModelClass::Fast => fast.map(str::to_string),
            ModelClass::Strong => strong.map(str::to_string),
        },
    }
}

/// What a harvest left in the Run's evidence tree.
struct Harvested {
    /// The transcript fact's bare name, relative to the Run directory
    /// (`sessions/<sid>/<filename>`) — the shape `Transcript::Recorded` records.
    recorded_name: String,
    /// The copied bytes as text, so the fan-out arithmetic reads the **same** lines the
    /// Attempt points at rather than re-discovering the file.
    text: String,
}

/// Copy the child's own session transcript into the Run's evidence tree, **after** exit.
///
/// Primary: the newest `.jsonl` directly under this stage's session directory — the P0
/// observation (`<ISO-ts>_<uuid>.jsonl`, flat inside `--session-dir`). Fallback,
/// empirical: the harness sometimes buckets the session under an encoded-cwd directory
/// despite `--session-dir`, so a `.jsonl` anywhere under `$HOME/.omp/agent/sessions`
/// **written after this attempt began** is taken instead — the time floor keeps another
/// Run's transcript from being mistaken for ours. Failure at every arm is loud in the
/// Attempt (`WroteNone`, an unobservable fan-out), never silent.
fn harvest(
    run_dir: &Path,
    sid: &str,
    stage_dir: &Path,
    started: SystemTime,
) -> Result<Harvested, String> {
    let source = newest_stage_file(stage_dir).or_else(|| strayed_after(started));
    let Some(source) = source else {
        return Err("the child allocated no session transcript".to_string());
    };
    let Some(filename) = source.file_name().and_then(|name| name.to_str()) else {
        return Err(format!("{}: unnameable transcript", source.display()));
    };
    let text = world::read_to_string(&source)?;
    let recorded_name = format!("sessions/{sid}/{filename}");
    world::write(&run_dir.join(&recorded_name), &text)?;
    Ok(Harvested {
        recorded_name,
        text,
    })
}

/// The newest-mtime `.jsonl` directly under `stage_dir` — the omp session file lands
/// flat there (P0). Nothing yet is `None`, which is the dispatch-shape answer for
/// `--resume` and the zero-line answer for the pre-run count.
fn newest_stage_file(stage_dir: &Path) -> Option<PathBuf> {
    let found: Vec<_> = world::list_with_extension(stage_dir, "jsonl")
        .into_iter()
        .map(|path| {
            let at = world::mtime(&path);
            (path, at)
        })
        .collect();
    newest_pair(found).map(|(path, _)| path)
}

/// Any `.jsonl` anywhere under `$HOME/.omp/agent/sessions` written after `started`.
fn strayed_after(started: SystemTime) -> Option<PathBuf> {
    let mut found = Vec::new();
    tree_jsonls(
        &world::home()?.join(".omp").join("agent").join("sessions"),
        &mut found,
    );
    found.retain(|(_, at)| at.is_some_and(|at| at > started));
    newest_pair(found).map(|(path, _)| path)
}

/// Newest by mtime, ties broken by path — the deterministic direction every mtime-max
/// read in this crate takes.
fn newest_pair(found: Vec<(PathBuf, Option<SystemTime>)>) -> Option<(PathBuf, Option<SystemTime>)> {
    found
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)))
        .filter(|(_, at)| at.is_some())
}

/// Newest `.jsonl` transcript under `run_dir`, recursively (the harness may nest an
/// encoded-cwd bucket inside an explicit `--session-dir`; observed empirically).
pub fn newest_session(run_dir: &Path) -> Option<PathBuf> {
    let mut found = Vec::new();
    tree_jsonls(run_dir, &mut found);
    newest_pair(found).map(|(path, _)| path)
}

/// Depth-first collection of every `.jsonl` file with a known mtime. An unreachable
/// subtree costs itself and nothing else — the same degrade-one-arm rule every reader
/// here follows.
fn tree_jsonls(dir: &Path, out: &mut Vec<(PathBuf, Option<SystemTime>)>) {
    for entry in world::list_dir(dir) {
        if world::is_dir(&entry) {
            tree_jsonls(&entry, out);
        } else if entry.extension().is_some_and(|e| e == "jsonl") {
            let at = world::mtime(&entry);
            out.push((entry, at));
        }
    }
}

/// How much of **this stage's** transcript existed right before the spawn, in lines —
/// the boundary the per-attempt fan-out slice cuts at. No file yet is zero.
fn stage_lines(stage_dir: &Path) -> usize {
    newest_stage_file(stage_dir)
        .and_then(|path| world::read_to_string(&path).ok())
        .map(|text| text.lines().count())
        .unwrap_or(0)
}

/// `<timestamp>_<uuid>.jsonl` resumes by the uuid — the part after the last `_`,
/// `.jsonl` stripped (the P0 probe: "id prefix, filename prefix, or id suffix after
/// timestamp" are all resume-matchable, and the id suffix is the one grind can derive
/// without storing anything). A name without an `_…​.jsonl` tail answers `None` rather
/// than handing a timestamp-shaped guess to `--resume`.
fn resume_suffix(filename: &str) -> Option<&str> {
    let after = filename.rsplit_once('_')?.1;
    let stem = after.strip_suffix(".jsonl")?;
    (!stem.is_empty()).then_some(stem)
}

/// The pure classifier over a raw triple, omp frames edition.
///
/// The tolerance rules mirror [`crate::claude::classify`] with the payload grammar
/// swapped: omp emits one JSONL event frame per line (`agent_start/end`, `turn_start/
/// end`, `message_*`, `tool_execution_*`), so a whole-string parse attempt would be the
/// wrong shape outright — junk lines cost themselves and nothing else, the same
/// line-by-line rule every transcript reader in this crate records for files whose real
/// format drifts between its own lines.
///
/// `pub` for the same reason [`crate::claude::classify`] is: it is a pure function over a raw
/// triple, and `tests/cost_conventions.rs` compares this backend's `total_cost_usd` convention
/// with the other two adapters' in one place (issue #194).
pub fn classify(
    stdout: &str,
    stderr: &str,
    code: Option<i32>,
    n: usize,
    mode: Mode,
    started_at: &str,
    ended_at: &str,
) -> Attempt {
    let mut frames = Frames::default();
    for line in stdout.lines() {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) {
            frames.absorb(&value);
        }
    }
    let parse_ok = frames.parse_ok();
    let is_error = !parse_ok || code.is_none_or(|c| c != 0);

    let final_text = frames.final_texts.join("\n");
    let stream_tail = tail(
        if final_text.is_empty() {
            stdout
        } else {
            &final_text
        },
        1500,
    );

    Attempt {
        n,
        mode,
        started_at: started_at.to_string(),
        ended_at: ended_at.to_string(),
        exit_code: code,
        is_error,
        parse_ok,
        subtype: frames.subtype,
        stop_reason: frames.stop_reason,
        total_cost_usd: frames.saw_usage.then_some(frames.cost),
        num_turns: frames.saw_frame.then_some(frames.turns),
        usage: frames.last_usage,
        api_error_status: frames.api_error_status,
        terminal_reason: frames.terminal_reason,
        permission_denials: Vec::new(),
        done_promise: frames.all_texts.iter().any(|t| t.contains(DONE_PROMISE)),
        // The claude.rs fallback logic with no payload to ask first: only a raw stream
        // that failed or died non-zero gets the needle folded over the normalised
        // stdout tail and stderr — where a limit that killed the child before any
        // frame leaves its verdict. False positives sleep instead of burning attempts.
        rate_limited: (!parse_ok || code.is_none_or(|c| c != 0))
            && mentions_limit(&normalise(&format!("{stream_tail} {stderr}"))),
        result_tail: stream_tail,
        fanout: Observed::Unobservable(Reason::saying("the transcript was not read")),
        transcript: Transcript::PredatesName,
    }
}

/// One stream's salient facts, folded tolerantly frame by frame.
#[derive(Default)]
struct Frames {
    saw_frame: bool,
    agent_ends_seen: usize,
    last_agent_end_had_assistant: bool,
    turns: u64,
    cost: f64,
    last_usage: Option<serde_json::Value>,
    /// At least one frame carried a usage object. Distinguishes "the stream
    /// exposed no spend channel" (`false` → `total_cost_usd: None`, the honest
    /// undeclared answer) from "spend was zero" — the manufactured non-null
    /// `$0.00` the old `saw_frame` gate produced is worse than either.
    saw_usage: bool,
    stop_reason: Option<String>,
    subtype: Option<String>,
    api_error_status: Option<String>,
    terminal_reason: Option<String>,
    /// The **last** assistant message's text blocks — replaced per message, because
    /// "final-assistant" means the words the stage ended on, not every word it said.
    final_texts: Vec<String>,
    /// Every assistant text block in order — what `done_promise` searches.
    all_texts: Vec<String>,
}

impl Frames {
    fn absorb(&mut self, value: &serde_json::Value) {
        self.saw_frame = true;
        match value.get("type").and_then(|t| t.as_str()) {
            Some("agent_end") => {
                self.agent_ends_seen += 1;
                self.last_agent_end_had_assistant = has_role(value, "assistant");
                if let Some(subtype) = text_at(value, "subtype") {
                    self.subtype = Some(subtype);
                }
            }
            Some("turn_start") => {
                self.turns += 1;
                self.final_texts.clear();
            }
            Some("message_end") => {
                // Usage accounting rides the assistant branch: spend belongs to
                // assistant turns, and a custom-role frame (subagent preludes,
                // reminders) that ever carried a usage object must not pollute
                // the ledger or the last-usage snapshot.
                if let Some((stop, texts)) = assistant_of(value) {
                    if let Some(stop) = stop {
                        self.stop_reason = Some(stop);
                    }
                    if !texts.is_empty() {
                        self.final_texts = texts.clone();
                        self.all_texts.extend(texts);
                    }
                    // Real omp v18 carries spend on assistant `message_end`
                    // frames (`message.usage.cost.total`), not on `turn_end` —
                    // run 178's transcripts have usage-free `turn_end`s, and
                    // the P0 spike's turn-borne shape is the doc claim this
                    // refutes.
                    if let Some(total) = value
                        .pointer("/message/usage/cost/total")
                        .and_then(|total| total.as_f64())
                    {
                        self.cost += total;
                    }
                    if let Some(usage) = value.pointer("/message/usage") {
                        self.saw_usage = true;
                        self.last_usage = Some(usage.clone());
                    }
                }
            }
            _ => {}
        }
        // Error-bearing frames speak when they can. The P0 frame list carries none, so
        // this costs one type check per frame and stays `None` until omp grows one.
        if let Some(kind) = value.get("type").and_then(|t| t.as_str())
            && kind.contains("error")
        {
            if let Some(status) = text_at(value, "api_error_status") {
                self.api_error_status = Some(status);
            }
            if let Some(reason) = text_at(value, "terminal_reason") {
                self.terminal_reason = Some(reason);
            }
        }
    }

    /// ≥1 `agent_end` frame **and** the last one carries assistant messages — a run
    /// whose closing frame arrived without any assistant work is not a parsed run.
    fn parse_ok(&self) -> bool {
        self.agent_ends_seen > 0 && self.last_agent_end_had_assistant
    }
}

/// `(stopReason, text blocks)` off one `message_end` frame's assistant message, both
/// optional-with-default the way the undocumented format demands.
fn assistant_of(value: &serde_json::Value) -> Option<(Option<String>, Vec<String>)> {
    let message = value.get("message")?;
    if message.get("role").and_then(|role| role.as_str()) != Some("assistant") {
        return None;
    }
    let stop = message
        .get("stopReason")
        .or_else(|| message.get("stop_reason"))
        .and_then(|stop| stop.as_str())
        .map(str::to_string);
    let texts = match message.get("content") {
        Some(serde_json::Value::String(text)) => {
            if text.is_empty() {
                Vec::new()
            } else {
                vec![text.clone()]
            }
        }
        Some(serde_json::Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(|text| text.as_str()))
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    };
    Some((stop, texts))
}

/// Whether any object at any depth carries `"role": role` — how an `agent_end` frame is
/// asked whether it closed over assistant messages, without betting on one nesting.
fn has_role(value: &serde_json::Value, role: &str) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            map.get("role").and_then(|r| r.as_str()) == Some(role)
                || map.values().any(|child| has_role(child, role))
        }
        serde_json::Value::Array(items) => items.iter().any(|item| has_role(item, role)),
        _ => false,
    }
}

fn tail(text: &str, characters: usize) -> String {
    let count = text.chars().count();
    text.chars()
        .skip(count.saturating_sub(characters))
        .collect()
}

/// The tools a fan-out spawn names. The omp CLI spells it lowercase (`task`); the
/// capitalised claude-code spelling rides along because the harness inherited its
/// vocabulary, and a rename in either direction is exactly the silence the extra name
/// costs nothing to prevent.
pub const FANOUT_TOOLS: [&str; 2] = ["task", "Agent"];

/// What one text holds about tool calls and their completions: every `toolCall` block
/// anywhere in the frames, the ids of the ones naming a fan-out tool, and every
/// `tool_execution_end` `callId`. Two passes over the finished text, deliberately —
/// completion frames read cleanly whether they arrived before or after their call's
/// content frame.
struct Scanned {
    total: usize,
    /// The fan-out spawns: `(call id, description)`. An idless spawn cannot pair, so it
    /// counts as spawned and never as returned — the safe low direction.
    spawns: Vec<(Option<String>, String)>,
    ended: Vec<String>,
}

fn scan_toolcalls(text: &str) -> Scanned {
    let mut scanned = Scanned {
        total: 0,
        spawns: Vec::new(),
        ended: Vec::new(),
    };
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            collect_toolcalls(&value, &mut scanned);
        }
    }
    scanned
}

fn collect_toolcalls(value: &serde_json::Value, out: &mut Scanned) {
    match value {
        serde_json::Value::Object(map) => {
            match map.get("type").and_then(|t| t.as_str()) {
                Some("toolCall") => {
                    out.total += 1;
                    let named = map.get("name").and_then(|n| n.as_str()).unwrap_or_default();
                    if FANOUT_TOOLS.contains(&named) {
                        let described = map
                            .get("arguments")
                            .and_then(|args| {
                                args.get("description").or_else(|| args.get("subject"))
                            })
                            .and_then(|d| d.as_str())
                            .unwrap_or_default()
                            .to_string();
                        out.spawns.push((
                            map.get("id").and_then(|i| i.as_str()).map(str::to_string),
                            described,
                        ));
                    }
                }
                Some("tool_execution_end") => {
                    if let Some(call) = map.get("callId").and_then(|c| c.as_str()) {
                        out.ended.push(call.to_string());
                    }
                }
                _ => {}
            }
            for child in map.values() {
                collect_toolcalls(child, out);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_toolcalls(item, out);
            }
        }
        _ => {}
    }
}

/// *Could not observe*, with the tool-call count in the reason — or `Absent` where
/// there was nothing in the transcript to recognise in the first place. This is what
/// separates *nothing recognised* from *nothing there*.
fn nothing_recognised<T>(total: usize) -> Observed<T> {
    if total == 0 {
        Observed::Absent
    } else {
        Observed::Unobservable(Reason::saying(&format!(
            "{total} tool call{} in the transcript and no recognised fan-out spawn",
            if total == 1 { "" } else { "s" }
        )))
    }
}

/// Per-attempt fan-out over the **lines appended since** `already_written` — this
/// stage's transcript is appended to by every resume, so counting the whole file on
/// attempt N would count attempts 1..N (`claude::fanout_since`'s rule).
pub(crate) fn fanout_since_text(text: &str, already_written: usize) -> Observed<(u64, u64)> {
    let sliced = text
        .lines()
        .skip(already_written)
        .collect::<Vec<&str>>()
        .join("\n");
    let scanned = scan_toolcalls(&sliced);
    if scanned.spawns.is_empty() {
        return nothing_recognised(scanned.total);
    }
    let returned = scanned
        .spawns
        .iter()
        .filter(|(id, _)| {
            id.as_deref()
                .is_some_and(|id| scanned.ended.iter().any(|end| end == id))
        })
        .count() as u64;
    Observed::Present((scanned.spawns.len() as u64, returned))
}

/// The subagents **still listed without a completion**, with descriptions — the live
/// answer to *is it blocked on agents right now*. The whole append-only file is read,
/// so a spawn paired to a `tool_execution_end` anywhere in it is finished work and is
/// not listed (`claude::fanout`'s rule): listing every spawn ever would read finished
/// fan-outs as running forever.
fn running_fanouts(text: &str) -> Observed<Vec<Fanout>> {
    let scanned = scan_toolcalls(text);
    if scanned.total == 0 {
        return Observed::Absent;
    }
    if scanned.spawns.is_empty() {
        return nothing_recognised(scanned.total);
    }
    let running: Vec<Fanout> = scanned
        .spawns
        .iter()
        .filter(|(id, _)| {
            !id.as_deref()
                .is_some_and(|id| scanned.ended.iter().any(|end| end == id))
        })
        .map(|(_, description)| Fanout {
            description: description.clone(),
        })
        .collect();
    if running.is_empty() {
        return Observed::Absent;
    }
    Observed::Present(running)
}

/// The last skill an attempt claimed, one line: read off whichever frame grew an
/// `attributionSkill`-shaped row, the key the other adapters recognise. omp v18.0.6
/// carried none, so the honest live answer is `Absent` today and *present* the day the
/// harness starts writing the row — no reader change needed.
pub fn now_skill(text: &str) -> Observed<String> {
    let mut last: Option<String> = None;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line)
            && let Some(skill) = value.get("attributionSkill").and_then(|s| s.as_str())
            && !skill.is_empty()
        {
            last = Some(skill.to_string());
        }
    }
    match last {
        Some(skill) => Observed::Present(skill),
        None => Observed::Absent,
    }
}

/// The last thing the assistant itself said, one line: *what is it doing right now*
/// (#82's question). Read off `message_end` frames only — the complete messages — so a
/// partial `message_update` never poses as the current state.
pub fn assistant_now(text: &str) -> Observed<String> {
    let mut last: Option<String> = None;
    let mut saw_frame = false;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            saw_frame = true;
            if value.get("type").and_then(|t| t.as_str()) == Some("message_end")
                && let Some((_, texts)) = assistant_of(&value)
                && !texts.is_empty()
            {
                last = Some(one_line(&texts.join("\n")));
            }
        }
    }
    match (saw_frame, last) {
        (false, _) => Observed::Unobservable(Reason::saying("no readable frame in the transcript")),
        (true, Some(said)) => Observed::Present(said),
        (true, None) => Observed::Absent,
    }
}

/// The last-words block, fixed at exactly `wanted` lines so `watch -n 30` never
/// jitters — every complete message, assistant or not, because what came back is half
/// of what a human reads.
pub fn last_words(text: &str, wanted: usize) -> Vec<String> {
    let mut said: Vec<String> = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line)
            && value.get("type").and_then(|t| t.as_str()) == Some("message_end")
            && let Some(message) = value.get("message")
        {
            let joined = match message.get("content") {
                Some(serde_json::Value::String(text)) => text.clone(),
                Some(serde_json::Value::Array(blocks)) => blocks
                    .iter()
                    .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<&str>>()
                    .join("\n"),
                _ => String::new(),
            };
            if !joined.is_empty() {
                said.push(one_line(&joined));
            }
        }
    }
    let start = said.len().saturating_sub(wanted);
    let mut block: Vec<String> = said[start..].to_vec();
    while block.len() < wanted {
        block.push(String::new());
    }
    block
}

fn unread<T>() -> Observed<T> {
    Observed::Unobservable(Reason::saying("the transcript could not be read"))
}

/// An omp Run's [`Live`], read off the transcripts under the Run's directory.
///
/// The newest-written `.jsonl` anywhere in the tree is the one read — attempts append
/// to one per-stage file and Reflect writes another, so *what is it doing now* is
/// whichever was touched last. Freshness spans every file, through the same
/// [`native_freshness`] the native panel fills with; the fields themselves come from
/// this module's frame readers, each degrading on its own.
pub fn live(run_dir: &Path, now_epoch: u64) -> Live {
    let mut candidates: Vec<(PathBuf, Option<SystemTime>)> = Vec::new();
    tree_jsonls(run_dir, &mut candidates);
    let (newest_path, newest_at) = match newest_pair(candidates) {
        Some((path, at)) => (Some(path), at),
        None => (None, None),
    };
    let text = newest_path
        .as_ref()
        .and_then(|path| world::read_to_string(path).ok());
    Live {
        transcript: newest_path.unwrap_or_else(|| run_dir.to_path_buf()),
        now_skill: match &text {
            Some(body) => now_skill(body),
            None => unread(),
        },
        assistant_now: match &text {
            Some(body) => assistant_now(body),
            None => unread(),
        },
        last_words: match &text {
            Some(body) => last_words(body, 3),
            None => vec![String::new(); 3],
        },
        fanout: match &text {
            Some(body) => running_fanouts(body),
            None => unread(),
        },
        freshness: native_freshness(&newest_at.into_iter().collect::<Vec<_>>(), now_epoch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A healthy two-turn session: junk line, per-message usage rows on the
    /// assistant `message_end` frames (the real omp v18 spend channel — run 178's
    /// transcripts carry usage-free `turn_end`s), a final assistant message
    /// carrying the promise, and an `agent_end` closing over assistant work.
    const PONG: &str = concat!(
        r#"{"type":"session","version":3,"id":"5f0c2e1a-0000-4000-8000-000000000001"}"#,
        "\n",
        r#"{"type":"agent_start","agent":"main"}"#,
        "\n",
        r#"{"type":"turn_start","n":1}"#,
        "\n",
        r#"{"type":"message_end","message":{"role":"user","content":"ping"}}"#,
        "\n",
        r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"checking"}],"stopReason":"toolUse","usage":{"input":1000,"output":200,"cacheRead":0,"cacheWrite":0,"cost":{"input":0.01,"output":0.02,"cacheRead":0.0,"cacheWrite":0.0,"total":0.03}}}}"#,
        "\n",
        r#"{"type":"turn_end"}"#,
        "\n",
        r#"not json at all <<<"#,
        "\n",
        r#"{"type":"turn_start","n":2}"#,
        "\n",
        r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"all green <promise>DONE</promise>"}],"stopReason":"endTurn","usage":{"input":2000,"output":400,"cacheRead":0,"cacheWrite":0,"cost":{"input":0.02,"output":0.02,"cacheRead":0.0,"cacheWrite":0.0,"total":0.04}}}}"#,
        "\n",
        r#"{"type":"turn_end"}"#,
        "\n",
        r#"{"type":"agent_end","messages":[{"role":"assistant","content":[{"type":"text","text":"all green"}]}]}"#,
    );

    /// A fan-out whose `task` spawn completed and whose `Agent` spawn did not, beside
    /// an unrelated `bash` call that must never be counted as a spawn.
    const PAIRED: &str = concat!(
        r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"toolCall","id":"t-1","name":"task","arguments":{"description":"scout"}},{"type":"toolCall","id":"t-2","name":"bash","arguments":{"command":"ls"}}]}}"#,
        "\n",
        r#"{"type":"tool_execution_end","callId":"t-1"}"#,
        "\n",
        r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"toolCall","id":"t-3","name":"Agent","arguments":{"subject":"deep"}}]}}"#,
    );

    fn classified(stdout: &str, stderr: &str, code: Option<i32>) -> Attempt {
        classify(stdout, stderr, code, 1, Mode::Dispatch, "start", "end")
    }

    #[test]
    fn a_healthy_session_classifies_clean() {
        let attempt = classified(PONG, "", Some(0));
        assert!(attempt.parse_ok);
        assert!(!attempt.is_error);
        assert!(!attempt.rate_limited);
        assert_eq!(attempt.num_turns, Some(2));
        assert!(
            (attempt.total_cost_usd.unwrap() - 0.07).abs() < 1e-9,
            "cost must sum the per-turn totals"
        );
        assert_eq!(attempt.stop_reason.as_deref(), Some("endTurn"));
        assert!(attempt.done_promise);
        assert!(attempt.result_tail.contains("<promise>DONE</promise>"));
        assert_eq!(attempt.subtype, None);
        assert_eq!(attempt.api_error_status, None);
        assert!(attempt.permission_denials.is_empty());
    }

    #[test]
    fn final_assistant_replaces_not_accumulates() {
        let attempt = classified(PONG, "", Some(0));
        assert!(
            attempt.result_tail.starts_with("all green"),
            "only the last turn's words are the tail: {:?}",
            attempt.result_tail
        );
    }

    #[test]
    fn an_agent_end_without_assistant_work_is_not_parsed() {
        let attempt = classified(
            concat!(
                r#"{"type":"turn_start","n":1}"#,
                "\n",
                r#"{"type":"agent_end","messages":[]}"#,
            ),
            "",
            Some(0),
        );
        assert!(!attempt.parse_ok);
        assert!(attempt.is_error);
    }

    #[test]
    fn a_truncated_stream_is_an_error_that_reads_the_stderr_for_limits() {
        let attempt = classified(
            r#"{"type":"session","version":3,"id":"cut-off her"#,
            "you have hit your rate limit",
            Some(1),
        );
        assert!(!attempt.parse_ok);
        assert!(attempt.is_error);
        assert!(attempt.rate_limited);
        assert!(!attempt.result_tail.is_empty(), "the raw tail is kept");
    }

    #[test]
    fn a_clean_zero_exit_with_no_limit_prose_is_not_a_wall() {
        let attempt = classified("", "", Some(0));
        assert!(!attempt.rate_limited);
        assert!(attempt.is_error, "!parse_ok is an error regardless of exit");
        assert_eq!(attempt.num_turns, None);
        assert_eq!(attempt.total_cost_usd, None);
    }

    #[test]
    fn a_stream_without_any_usage_frames_records_none_not_a_zero() {
        // Run 178's real shape: frames flow, no message ever carries usage —
        // the old `saw_frame` gate manufactured a non-null `$0.00` here.
        let attempt = classified(
            concat!(
                r#"{"type":"turn_start","n":1}"#,
                "\n",
                r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"done"}],"stopReason":"endTurn"}}"#,
                "\n",
                r#"{"type":"turn_end"}"#,
                "\n",
                r#"{"type":"agent_end","messages":[{"role":"assistant","content":[{"type":"text","text":"done"}]}]}"#,
            ),
            "",
            Some(0),
        );
        assert!(attempt.parse_ok);
        assert_eq!(attempt.total_cost_usd, None);
        assert_eq!(attempt.usage, None);
    }

    #[test]
    fn a_non_assistant_message_end_with_usage_is_not_accounted() {
        // A custom-role frame that ever carries a usage object must not
        // pollute the ledger or the last-usage snapshot — spend belongs to
        // assistant turns only.
        let attempt = classified(
            concat!(
                r#"{"type":"message_end","message":{"role":"custom","customType":"eager-task-prelude","content":"<system-reminder>spawn</system-reminder>","usage":{"input":99999,"output":99999,"cost":{"total":99.0}}}}"#,
                "\n",
                r#"{"type":"turn_start","n":1}"#,
                "\n",
                r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"done"}],"stopReason":"endTurn","usage":{"input":10,"output":20,"cost":{"total":0.02}}}}"#,
                "\n",
                r#"{"type":"turn_end"}"#,
                "\n",
                r#"{"type":"agent_end","messages":[{"role":"assistant","content":[{"type":"text","text":"done"}]}]}"#,
            ),
            "",
            Some(0),
        );
        assert!(attempt.parse_ok);
        assert_eq!(attempt.total_cost_usd, Some(0.02));
        assert_eq!(
            attempt
                .usage
                .and_then(|u| u.pointer("/cost/total").cloned()),
            Some(serde_json::json!(0.02))
        );
    }

    #[test]
    fn tool_call_pairing_counts_spawned_and_returned() {
        assert_eq!(
            fanout_since_text(PAIRED, 0),
            Observed::Present((2, 1)),
            "`bash` is never a spawn; `task` paired; `Agent` did not"
        );
    }

    #[test]
    fn the_per_attempt_slice_skips_pre_run_lines() {
        let appended = format!(
            "{}\n{}",
            PAIRED.lines().next().unwrap(),
            PAIRED.lines().nth(1).unwrap()
        );
        assert_eq!(
            fanout_since_text(&appended, 1),
            Observed::Absent,
            "a pre-run line containing a completion leaves the slice empty of spawns"
        );
    }

    #[test]
    fn unrecognised_tool_calls_are_unobservable_never_absent() {
        let bash_only = r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"toolCall","id":"b-1","name":"bash","arguments":{}}]}}"#;
        assert!(matches!(
            fanout_since_text(bash_only, 0),
            Observed::Unobservable(_)
        ));
    }

    #[test]
    fn running_fanouts_lists_only_the_unpaired_spawns() {
        let Observed::Present(running) = running_fanouts(PAIRED) else {
            panic!("the unpaired Agent spawn must be listed");
        };
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].description, "deep");
    }

    #[test]
    fn resume_suffix_takes_the_part_after_the_last_underscore() {
        assert_eq!(
            resume_suffix("2026-08-27T05-12-05-123Z_01a03755-3103-71b1-a5f2-fefc55addef1.jsonl"),
            Some("01a03755-3103-71b1-a5f2-fefc55addef1")
        );
        assert_eq!(resume_suffix("flat.jsonl"), None, "no underscore, no guess");
        assert_eq!(resume_suffix("2026-08-27T05Z_stem.txt"), None, "not jsonl");
        assert_eq!(resume_suffix("x_.jsonl"), None, "empty stem");
    }

    #[test]
    fn the_model_flag_matches_native_id_semantics_without_a_default_fallback() {
        let pinned = StageModel::Pinned("z-ai/glm-5.3-flash".to_string());
        let fast = StageModel::Class(ModelClass::Fast);
        let strong = StageModel::Class(ModelClass::Strong);
        let none = crate::runner::ClassRoutes::default();
        assert_eq!(
            model_flag(&pinned, None, None, &none).as_deref(),
            Some("z-ai/glm-5.3-flash")
        );
        assert_eq!(
            model_flag(&fast, Some("qwen/qwen3-max"), None, &none).as_deref(),
            Some("qwen/qwen3-max")
        );
        assert_eq!(
            model_flag(&fast, None, None, &none),
            None,
            "undeclared = no flag"
        );
        assert_eq!(model_flag(&strong, None, None, &none), None);
        assert_eq!(
            model_flag(&strong, None, Some("deepseek/deepseek-chat-v3.1"), &none).as_deref(),
            Some("deepseek/deepseek-chat-v3.1")
        );
    }

    /// A route naming omp resolves the class — a declared id becomes the flag, a `None`
    /// id stays no-flag (the harness's own default, never a grind-invented id) — and
    /// routes naming other backends fall back to the legacy fields, pinned verbatim.
    #[test]
    fn the_model_flag_resolves_routes_naming_omp() {
        let id_route = |backend: crate::runner::Backend, id: Option<&str>| crate::runner::Route {
            backend,
            id: id.map(str::to_string),
        };
        let routed = crate::runner::ClassRoutes {
            fast: Some(id_route(
                crate::runner::Backend::Omp,
                Some("z-ai/glm-5.3-flash"),
            )),
            strong: Some(id_route(crate::runner::Backend::Omp, None)),
        };
        assert_eq!(
            model_flag(&StageModel::Class(ModelClass::Fast), None, None, &routed).as_deref(),
            Some("z-ai/glm-5.3-flash")
        );
        assert_eq!(
            model_flag(
                &StageModel::Class(ModelClass::Strong),
                Some("legacy/id"),
                None,
                &routed
            ),
            None,
            "an omp route with no id is no flag, never the legacy field"
        );
        assert_eq!(
            model_flag(
                &StageModel::Pinned("pinned/id".to_string()),
                None,
                None,
                &routed
            )
            .as_deref(),
            Some("pinned/id"),
            "the pin crosses verbatim regardless of routes"
        );
        let foreign = crate::runner::ClassRoutes {
            fast: Some(id_route(
                crate::runner::Backend::ClaudeCode,
                Some("claude/alias"),
            )),
            strong: None,
        };
        assert_eq!(
            model_flag(
                &StageModel::Class(ModelClass::Fast),
                Some("legacy/fast"),
                None,
                &foreign
            )
            .as_deref(),
            Some("legacy/fast"),
            "routes naming other backends fall back to the legacy fields"
        );
    }

    #[test]
    fn newest_session_walks_the_tree_and_answers_newest_mtime() {
        let root = world::temp_dir("omp-newest");
        let early = root.join("early");
        let late = root.join("nested").join("deeper");
        world::create_dir_all(&late).unwrap();
        world::create_dir_all(&early).unwrap();
        let _ = world::write(&early.join("first.jsonl"), "{}\n");
        let _ = world::write(&late.join("second.jsonl"), "{}\n{}\n");
        assert_eq!(
            newest_session(&root),
            Some(late.join("second.jsonl")),
            "the nested newer file wins over the shallower older one"
        );
        world::remove_tree(&root);
    }

    #[test]
    fn newest_session_on_an_empty_directory_is_none() {
        let root = world::temp_dir("omp-newest-empty");
        assert_eq!(newest_session(&root), None);
        world::remove_tree(&root);
    }
}
