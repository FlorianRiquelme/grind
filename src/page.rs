//! Pure HTML rendering for the dashboard (plan U3). Every byte the server sends as HTML
//! is a `String` built here; mirrors `render`'s discipline — pure functions, tested
//! wording, escaping mandatory for record-derived strings (ADR-0014). The design contract
//! is the user-approved cockpit mockups: kanban roster, cockpit Run page with an attempt
//! waterfall (KTD15). Classes are `g-`-prefixed and bind to `style.rs`; DOM hooks
//! (`data-g-root`, `data-gid`, `data-epoch`, …) bind to `script.rs`.

use crate::attempt::Attempt;
use crate::observe::Observed;
use crate::view::{Facts, Live, ProposalEntry, RosterRow, RunView};

/// Escape a record-derived string for interpolation into HTML. Every string that
/// originated in a Run passes through this — model output is arbitrary bytes until
/// proven otherwise.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Days since 1970-01-01 from a civil date (Hinnant's algorithm). Proleptic Gregorian.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Unix epoch seconds for a recorded timestamp. Records carry one UTC spelling
/// (`2026-08-06T12:26:20+00:00`, written by [`crate::world`]); the date-time prefix is
/// parsed and any offset suffix ignored — the record never writes a wall-clock zone.
fn iso_epoch(ts: &str) -> Option<u64> {
    let b = ts.as_bytes();
    if b.len() < 19
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |a: usize, n: usize| ts.get(a..a + n)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let (h, mi, s) = (num(11, 2)?, num(14, 2)?, num(17, 2)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    Some((days_from_civil(y, mo, d) * 86_400 + h * 3_600 + mi * 60 + s) as u64)
}

/// `HH:MM` for an epoch, UTC.
fn hhmm(epoch: u64) -> String {
    let secs_of_day = epoch % 86_400;
    format!(
        "{:02}:{:02}",
        secs_of_day / 3_600,
        (secs_of_day % 3_600) / 60
    )
}

fn pct(t: u64, t0: u64, t1: u64) -> f64 {
    if t1 <= t0 {
        return 0.0;
    }
    ((t.saturating_sub(t0)) as f64 / (t1 - t0) as f64 * 100.0).clamp(0.0, 100.0)
}

/// A moment rendered as a ticking element when its epoch is known, plain text otherwise.
fn moment(label: &str, iso: &str) -> String {
    match iso_epoch(iso) {
        Some(e) => format!("<span class=\"g-quiet\" data-epoch=\"{e}\">{label}</span>"),
        None => format!("<span class=\"g-quiet\">{label}</span>"),
    }
}

/// Card accent family by recorded state: live-ish green, held amber, stopped red,
/// finished gray. Unknown states read as stopped-family — they must stay visible, and
/// red asks for eyes.
fn accent(state: &str) -> &'static str {
    match state {
        "dispatched" => "g",
        "blocked" => "a",
        "died" | "unobserved" | "uncorroborated" | "rate_limited" => "r",
        _ => "k",
    }
}

fn badge_class(state: &str) -> &'static str {
    match state {
        "dispatched" => "g-b-run",
        "blocked" => "g-b-hold",
        "rate_limited" | "died" | "unobserved" | "uncorroborated" => "g-b-dead",
        _ => "g-b-done",
    }
}

/// Liveness as (word, dot-class). *Could not answer* is **unobservable**, never
/// *stopped* (KTD12) — flattening the tri-state would be exactly the false all-clear
/// that sends an operator back to sleep.
fn liveness(here: &Observed<bool>) -> (&'static str, &'static str) {
    match here {
        Observed::Present(true) => ("live", "g g-pulse"),
        _ => ("unobservable", "k g-dim"),
    }
}

/// The swimlane each state lives in: `(rail class, rail title)`. Lane membership is
/// attention; columns carry the recorded state itself.
fn lane(state: &str) -> (&'static str, &'static str) {
    match state {
        "blocked" => ("hold", "Needs you"),
        "dispatched" | "rate_limited" => ("go", "In flight"),
        "completed" | "exhausted" => ("done", "Finished today"),
        _ => ("stop", "Stopped"),
    }
}

/// Column label for a state inside its lane. Known states get their display name;
/// unrecognized states render verbatim — data, not invention.
const SHELL_INNER: &str = "display:flex;width:100%;max-width:1010px;margin:0 auto;padding:0 16px";

pub fn layout(title: &str, body: &str) -> String {
    format!(
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<meta name=\"color-scheme\" content=\"dark\">\
<title>{}</title>\
<link rel=\"stylesheet\" href=\"/style.css\"><script src=\"/script.js\" defer></script>\
</head><body>{}</body></html>",
        esc(title),
        body
    )
}

fn command_bar() -> String {
    format!(
        "<div class=\"g-bar\"><div style=\"{SHELL_INNER}\">\
<span class=\"g-seg\"><span class=\"g-brand\">GRIND</span><span class=\"g-faint\">▪</span><span class=\"g-dim\">this-host</span></span>\
<span class=\"g-seg\"><span class=\"g-led g-blink\"></span><span class=\"g-dim\">poll 2s</span></span>\
<span class=\"g-seg g-dim\" id=\"clk\">--:--:-- UTC</span>\
</div></div>"
    )
}

fn tseg(label: &str, value: &str, color: &str, sub: &str) -> String {
    let sub_html = if sub.is_empty() {
        String::new()
    } else {
        format!("<div class=\"g-tsub\">{sub}</div>")
    };
    format!(
        "<div class=\"g-tseg\"><div class=\"g-tlab\">{label}</div>\
<div class=\"g-tval {color}\">{value}</div>{sub_html}</div>"
    )
}

fn telemetry(rows: &[RosterRow]) -> String {
    let inflight = rows
        .iter()
        .filter(|r| matches!(r.recorded_state.as_str(), "dispatched" | "rate_limited"))
        .count();
    let needs = rows
        .iter()
        .filter(|r| r.recorded_state == "blocked")
        .count();
    let stopped = rows
        .iter()
        .filter(|r| {
            matches!(
                r.recorded_state.as_str(),
                "died" | "unobserved" | "uncorroborated"
            )
        })
        .count();
    let finished = rows
        .iter()
        .filter(|r| matches!(r.recorded_state.as_str(), "completed" | "exhausted"))
        .count();
    let spend: f64 = rows.iter().map(|r| r.spend).sum();
    let (done, total): (usize, usize) = rows
        .iter()
        .fold((0, 0), |(a, b), r| (a + r.attempts.0, b + r.attempts.1));
    let meter_w = if total == 0 {
        0.0
    } else {
        done as f64 / total as f64 * 100.0
    };
    let attempts_value = format!(
        "{done}<span class=\"g-dim\">/{total}</span>\
<span class=\"g-meter\"><i style=\"width:{meter_w:.0}%\"></i></span>"
    );
    format!(
        "<div class=\"g-tele\"><div style=\"{SHELL_INNER}\">\
{}{}{}{}{}{}\
</div></div>",
        tseg(
            "in flight",
            &inflight.to_string(),
            if inflight > 0 { "g" } else { "n" },
            ""
        ),
        tseg(
            "needs you",
            &needs.to_string(),
            if needs > 0 { "a" } else { "n" },
            ""
        ),
        tseg("stopped", &stopped.to_string(), "n", ""),
        tseg("finished", &finished.to_string(), "n", "on this host"),
        tseg("spend", &format!("${spend:.2}"), "n", "all recorded runs"),
        tseg("attempts", &attempts_value, "n", "")
    )
}

fn status_bar() -> String {
    let k = |t: &str| format!("<span class=\"g-k\">{t}</span>");
    format!(
        "<div class=\"g-status\"><div style=\"{SHELL_INNER}\">\
<span class=\"g-seg2\"><span class=\"g-ok\">●</span> attached · pull-only</span>\
<span class=\"g-seg2 g-dim\">refresh 2s live · 10s idle · paused when hidden</span>\
<span class=\"g-spacer\"></span>\
<span class=\"g-seg2\"><span data-g-stamp class=\"g-dim\">updated —</span></span>\
<span class=\"g-seg2\"><button data-g-pause class=\"g-link\" style=\"background:none;border:0;color:inherit;font:inherit;cursor:pointer\">pause</button></span>\
<span class=\"g-seg2\">{}{} move</span>\
<span class=\"g-seg2\">{} open</span>\
<span class=\"g-seg2\">{} back</span>\
<span class=\"g-seg2\">{} sort {} filter</span>\
</div></div>",
        k("j"),
        k("k"),
        k("\u{21b5}"),
        k("esc"),
        k("s"),
        k("/")
    )
}

fn budget_dots(n: usize, m: usize) -> String {
    if m == 0 {
        return String::new();
    }
    let mut out = String::from("<span class=\"g-budget\">");
    for i in 0..m {
        out.push_str(if i < n {
            "<i class=\"g-f\"></i>"
        } else {
            "<i class=\"g-e\"></i>"
        });
    }
    out.push_str(&format!("&nbsp;{n}/{m}</span>"));
    out
}

fn card(r: &RosterRow) -> String {
    let (_, ldot) = liveness(&r.supervisor_here);
    let quiet = if matches!(r.supervisor_here, Observed::Unobservable(_)) {
        "<span class=\"g-quiet\">unobservable</span>".to_string()
    } else {
        String::new()
    };
    let age = moment("", &r.last_activity);
    let job = if r.job_url.is_empty() {
        String::new()
    } else {
        format!("<a class=\"g-link\" href=\"{}\">job</a>", esc(&r.job_url))
    };
    format!(
        "<div class=\"g-card {}\" data-gid=\"{}\">\
<div class=\"g-l1\"><span class=\"g-rid\">{}</span>{age}</div>\
<div class=\"g-l2\"><span class=\"g-title\">{}</span>\
<span class=\"g-st\"><span class=\"g-dot {ldot}\"></span><span class=\"g-badge {}\">{}</span>{quiet}</span></div>\
<div class=\"g-l3\"><span class=\"g-link\">{}</span>{}<span>${:.2}</span>{job}</div>\
</div>",
        accent(&r.recorded_state),
        esc(&r.run_id),
        esc(&r.run_id),
        esc(&r.job_title),
        badge_class(&r.recorded_state),
        esc(&r.recorded_state),
        esc(&r.branch),
        budget_dots(r.attempts.0, r.attempts.1),
        r.spend,
    )
}

/// One swimlane band: rail on the left, state columns to the right. An empty column is a
/// dashed slot so the shape of the board stays legible when runs are few.
fn board(rows: &[RosterRow]) -> String {
    let any_live = rows
        .iter()
        .any(|r| matches!(r.recorded_state.as_str(), "dispatched" | "rate_limited"));
    let mut out = format!(
        "<div data-g-root=\"board\" data-live=\"{}\">",
        if any_live { "true" } else { "false" }
    );

    #[derive(PartialEq, Eq, PartialOrd, Ord)]
    struct Key(String, String);
    let mut groups: std::collections::BTreeMap<Key, Vec<&RosterRow>> = Default::default();
    for r in rows {
        let (lane_name, _) = lane(&r.recorded_state);
        groups
            .entry(Key(lane_name.to_string(), r.recorded_state.clone()))
            .or_default()
            .push(r);
    }

    const LANES: &[(&str, &str)] = &[
        ("hold", "Needs you"),
        ("go", "In flight"),
        ("stop", "Stopped"),
        ("done", "Finished today"),
    ];
    for (rail, title) in LANES {
        let mut cols: Vec<(String, Vec<&RosterRow>)> = Vec::new();
        let known: &[&str] = match *rail {
            "go" => &["working", "waiting"],
            "stop" => &["died", "unobserved", "uncorroborated"],
            "done" => &["complete", "exhausted"],
            _ => &["blocked", "cleared · resumable"],
        };
        for k in known {
            let key_state = match *k {
                "working" => "dispatched",
                "waiting" => "rate_limited",
                "complete" => "completed",
                other => other,
            };
            let rows_here: Vec<&RosterRow> = groups
                .get(&Key((*rail).to_string(), key_state.to_string()))
                .cloned()
                .unwrap_or_default();
            cols.push(((*k).to_string(), rows_here));
        }
        let mut unknown: Vec<String> = groups
            .keys()
            .filter(|Key(l, c)| {
                l.as_str() == *rail
                    && !matches!(
                        c.as_str(),
                        "dispatched"
                            | "rate_limited"
                            | "died"
                            | "unobserved"
                            | "uncorroborated"
                            | "completed"
                            | "exhausted"
                            | "blocked"
                    )
            })
            .map(|Key(_, c)| c.clone())
            .collect();
        unknown.sort();
        for c in unknown {
            let rows_here: Vec<&RosterRow> = groups
                .get(&Key((*rail).to_string(), c.clone()))
                .cloned()
                .unwrap_or_default();
            cols.push((c.clone(), rows_here));
        }

        let wip: usize = cols.iter().map(|(_, v)| v.len()).sum();
        out.push_str(&format!(
            "<div class=\"g-lane\"><div class=\"g-rail {rail}\"><div class=\"g-nm\">{title}</div><div class=\"g-wip\">wip {wip}</div></div><div class=\"g-board\">"
        ));
        for (label, col_rows) in &cols {
            out.push_str(&format!(
                "<div class=\"g-col\" style=\"flex:1;min-width:150px\"><div class=\"g-colh\">{} <span class=\"g-c\">{}</span></div><div class=\"g-slots\">",
                esc(label),
                col_rows.len()
            ));
            if col_rows.is_empty() {
                out.push_str("<div class=\"g-empty\"></div>");
            } else {
                for r in col_rows {
                    out.push_str(&card(r));
                }
            }
            out.push_str("</div></div>");
        }
        out.push_str("</div></div>");
    }
    out.push_str("</div>");
    out
}

pub fn roster_fragment(rows: &[RosterRow]) -> String {
    board(rows)
}

/// One drafted follow-up Job or proposed skill diff, as Reflect left it — a GET-only
/// projection over `stages/reflect/` (design line 160): nothing stores this list, so
/// [`crate::view::proposal_queue`] rebuilds it fresh on every request and this only renders
/// what came back. Empty when nothing has been proposed.
fn proposal_queue_section(proposals: &[(String, ProposalEntry)]) -> String {
    if proposals.is_empty() {
        return String::new();
    }
    let mut rows = String::new();
    for (run_id, entry) in proposals {
        rows.push_str(&format!(
            "<div class=\"g-a\"><div class=\"g-aline\"><span class=\"g-idx\">{}</span>\
<span class=\"g-verdict\">{}</span></div>\
<div class=\"g-asub\">{} — {}</div></div>",
            esc(run_id),
            esc(entry.kind),
            esc(&entry.summary),
            esc(&entry.path.display().to_string()),
        ));
    }
    format!(
        "<div style=\"max-width:1010px;margin:0 auto;padding:0 16px\">\
<div class=\"g-panel\"><div class=\"g-phead\">proposal queue<span class=\"g-r\">drafted by reflect · read-only</span></div>{rows}</div></div>"
    )
}

pub fn roster_page(rows: &[RosterRow], proposals: &[(String, ProposalEntry)]) -> String {
    let needs = rows.iter().any(|r| r.recorded_state == "blocked");
    let inflight = rows
        .iter()
        .any(|r| matches!(r.recorded_state.as_str(), "dispatched" | "rate_limited"));
    let title = format!(
        "({}{}) grind",
        if needs { "1 needs you · " } else { "" },
        if inflight { "in flight" } else { "idle" }
    );
    layout(
        &title,
        &format!(
            "{}{}<div style=\"max-width:1010px;margin:0 auto;padding:0 16px\">{}</div>{}{}",
            command_bar(),
            telemetry(rows),
            roster_fragment(rows),
            proposal_queue_section(proposals),
            status_bar()
        ),
    )
}

/// Outcome label for a recorded Attempt, from what the record carries — never a quality
/// word beyond the record's own vocabulary.
fn outcome(a: &Attempt) -> (&'static str, &'static str) {
    if a.rate_limited {
        ("rate_limited", "g-v-amber")
    } else if !a.parse_ok {
        ("unparseable", "g-v-dead")
    } else if a.is_error {
        ("died", "g-v-dead")
    } else {
        ("ended", "")
    }
}

fn waterfall(found: &RunView) -> String {
    let atts = &found.attempts;
    let in_flight = matches!(found.state.as_str(), "dispatched" | "rate_limited");
    let t0 = atts.iter().filter_map(|a| iso_epoch(&a.started_at)).min();
    let t1 = atts.iter().filter_map(|a| iso_epoch(&a.ended_at)).max();
    if atts.is_empty() && !in_flight {
        return "<div class=\"g-wf g-dim\">no attempts recorded yet</div>".to_string();
    }
    let (axis0, axis1): (u64, u64) = match (t0, t1) {
        (Some(a), Some(b)) => (a, b.max(a)),
        (Some(a), None) => (a, a),
        _ => (0, 0),
    };

    let mut bars = String::new();
    for w in atts.windows(2) {
        if let (Some(e), Some(s)) = (iso_epoch(&w[0].ended_at), iso_epoch(&w[1].started_at))
            && s > e
            && s - e <= 3600
        {
            let l = pct(e, axis0, axis1);
            let r = pct(s, axis0, axis1);
            bars.push_str(&format!(
                "<div class=\"g-sleep\" style=\"left:{l:.1}%;width:{:.1}%\">asleep {}s</div>",
                (r - l).max(2.0),
                s - e
            ));
        }
    }
    for a in atts {
        let (Some(s), Some(e)) = (iso_epoch(&a.started_at), iso_epoch(&a.ended_at)) else {
            continue;
        };
        let l = pct(s, axis0, axis1);
        let wd = (pct(e, axis0, axis1) - l).max(3.0);
        let cls = if a.rate_limited {
            "amber"
        } else if a.is_error || !a.parse_ok {
            "dead"
        } else {
            ""
        };
        let (word, _) = outcome(a);
        bars.push_str(&format!(
            "<div class=\"g-wbar {}\" style=\"left:{l:.1}%;width:{wd:.1}%\">#{} {}</div>",
            cls, a.n, word
        ));
    }
    if in_flight {
        let n = crate::attempt::working(atts) + 1;
        let left = match t1 {
            Some(e) if axis1 > e => pct(e, axis0, axis1),
            _ => 0.0,
        };
        let width = (99.0 - left).max(6.0);
        bars.push_str(&format!(
            "<div class=\"g-wbar g-livebar\" style=\"left:{left:.1}%;width:{width:.1}%\">#{n} in flight</div>"
        ));
    }
    bars.push_str("<div class=\"g-now\" style=\"left:99%\"></div>");

    let mut axis = String::from("<div class=\"g-axis\">");
    if axis1 > axis0 {
        for f in [0.0, 25.0, 50.0, 75.0, 100.0] {
            let t = axis0 + ((axis1 - axis0) as f64 * f / 100.0) as u64;
            axis.push_str(&format!("<span>{}</span>", hhmm(t)));
        }
    }
    axis.push_str("</div>");
    let legend = "<div class=\"g-wl\"><span><i class=\"g-f\"></i>working</span>\
<span><i style=\"background:rgba(210,153,34,.75)\"></i>rate_limited</span>\
<span><i style=\"background:rgba(248,81,73,.75)\"></i>died</span>\
<span><i style=\"background:repeating-linear-gradient(135deg,rgba(125,133,144,.4) 0 3px,transparent 3px 6px)\"></i>sleep</span></div>";
    format!(
        "<div class=\"g-wf\"><div style=\"position:relative\"><div class=\"g-track\">\
<div class=\"g-gridline\" style=\"left:20%\"></div><div class=\"g-gridline\" style=\"left:40%\"></div>\
<div class=\"g-gridline\" style=\"left:60%\"></div><div class=\"g-gridline\" style=\"left:80%\"></div>\
{bars}</div><div class=\"g-nowlbl\" style=\"position:absolute;top:2px;right:4px\">NOW</div></div>{axis}{legend}</div>"
    )
}

fn attempt_list(run_id: &str, found: &RunView) -> String {
    let mut out = String::new();
    let in_flight = matches!(found.state.as_str(), "dispatched" | "rate_limited");
    if in_flight {
        let n = crate::attempt::working(&found.attempts) + 1;
        out.push_str(&format!(
            "<div class=\"g-a\"><div class=\"g-aline\"><span class=\"g-idx\">#{n}</span>\
<span class=\"g-verdict g-v-live\">\u{25b6} in flight</span><span class=\"g-dur\">not yet recorded</span></div>\
<div class=\"g-asub g-stage\">the supervisor writes the Attempt when it ends — raw output lands first (ADR-0004)</div></div>"
        ));
    }
    for a in found.attempts.iter().rev() {
        let (word, vcls) = outcome(a);
        let dur = match (iso_epoch(&a.started_at), iso_epoch(&a.ended_at)) {
            (Some(s), Some(e)) => {
                let d = e.saturating_sub(s);
                if d >= 60 {
                    format!("{}m", d / 60)
                } else {
                    format!("{d}s")
                }
            }
            _ => String::new(),
        };
        let mut meta = Vec::new();
        meta.push(a.mode.to_string());
        if let Some(c) = a.total_cost_usd {
            meta.push(format!("${c:.2}"));
        }
        if let Some(t) = a.num_turns {
            meta.push(format!("{t} turns"));
        }
        if !a.permission_denials.is_empty() {
            meta.push(format!("denials {}", a.permission_denials.len()));
        }
        if a.is_wait() {
            meta.push("wait — did no work".to_string());
        }
        let reason = a
            .terminal_reason
            .clone()
            .or_else(|| a.stop_reason.clone())
            .or_else(|| a.subtype.clone())
            .filter(|r| !r.is_empty())
            .map(|r| format!("<div class=\"g-asub\">{}</div>", esc(&r)))
            .unwrap_or_default();
        let evidence = evidence_links(run_id, a.n, found.backend, &a.transcript);
        out.push_str(&format!(
            "<div class=\"g-a\"><div class=\"g-aline\"><span class=\"g-idx\">#{}</span>\
<span class=\"g-verdict {vcls}\">{word}</span><span class=\"g-dur\">{}{}</span></div>\
{reason}\
{evidence}</div>",
            a.n,
            if dur.is_empty() {
                String::new()
            } else {
                format!("{dur} · ")
            },
            if a.done_promise {
                "made its DONE promise".to_string()
            } else {
                String::new()
            },
        ));
    }
    if out.is_empty() {
        out = "<div class=\"g-a g-dim\">nothing recorded</div>".to_string();
    }
    out
}

/// What one attempt's row links to, which depends on what that Run's backend actually wrote
/// to disk (issue found in review: `RunView.backend` was added by #135 and read nowhere).
/// `ClaudeCodeAdapter::run` writes the three `attempt-N.*` files; `NativeAdapter::run` writes
/// only `messages-N.jsonl` in the run dir — rendering the claude-code trio for a native attempt
/// links three files that were never written. This is a pure renderer with no filesystem
/// access, so it names what each backend writes rather than checking what exists.
///
/// `transcript` is the three-valued fact the Attempt itself carries, and each value renders
/// its own row (issue #161): a recorded name wins over the computed one — a native attempt that
/// re-entered after a crash allocated the first free slot, `messages-2-2.jsonl`, while
/// `messages-2.jsonl` still holds the dead attempt's record, so the computed name would put
/// another attempt's transcript under this row's heading (issue #156); every record written
/// before the name existed keeps the constructed fallback, which names that attempt's own file
/// exactly; and an attempt whose lifecycle ended before allocating anything renders no link at
/// all — the URL today's fallback served was backed by nothing. The claude-code trio ignores
/// the fact entirely: those three names are determined by `n` alone.
fn evidence_links(
    run_id: &str,
    n: usize,
    backend: crate::runner::Backend,
    transcript: &crate::attempt::Transcript,
) -> String {
    let run_id = esc(run_id);
    match backend {
        crate::runner::Backend::ClaudeCode => format!(
            "<div class=\"g-ev\"><a class=\"g-link\" href=\"/raw/runs/{run_id}/attempt-{n}.prompt.txt\">prompt.txt</a>\
<a class=\"g-link\" href=\"/raw/runs/{run_id}/attempt-{n}.stdout.json\">stdout.json</a>\
<a class=\"g-link\" href=\"/raw/runs/{run_id}/attempt-{n}.stderr.log\">stderr.log</a></div>"
        ),
        crate::runner::Backend::Native => match transcript {
            crate::attempt::Transcript::Recorded(name) => format!(
                "<div class=\"g-ev\"><a class=\"g-link\" href=\"/raw/runs/{run_id}/{}\">messages.jsonl</a></div>",
                esc(name)
            ),
            crate::attempt::Transcript::PredatesName => {
                let name = esc(&format!("messages-{n}.jsonl"));
                format!(
                    "<div class=\"g-ev\"><a class=\"g-link\" href=\"/raw/runs/{run_id}/{name}\">messages.jsonl</a></div>"
                )
            }
            // Nothing was ever written under this attempt's name, so there is nothing to
            // link — and no placeholder stands in for one (issue #161).
            crate::attempt::Transcript::WroteNone => String::new(),
        },
    }
}

fn last_words_block(live: &Live) -> String {
    let body: Vec<String> = live.last_words.iter().map(|l| esc(l)).collect();
    format!(
        "<div class=\"g-panel\"><div class=\"g-phead\">last words<span class=\"g-r\">transcript · fixed three lines</span></div>\
<pre class=\"g-log\" style=\"height:auto;padding:10px 12px;margin:0\">{}</pre></div>",
        body.join("\n")
    )
}

fn clearance_panel(facts: &Facts) -> String {
    let Some(c) = &facts.cleared else {
        return String::new();
    };
    format!(
        "<div class=\"g-panel\" style=\"border-color:rgba(210,153,34,.45)\"><div class=\"g-phead\">cleared</div>\
<div style=\"padding:10px 12px\">{} — {}</div></div>",
        esc(&c.cleared_at),
        esc(&c.note)
    )
}

/// The ten-rung stage table, one row per [`crate::rung::StageEntry`] the record carries. `[R]`
/// rows (Triage, Diff-triage) render as the zero-cost passes they are — the record already
/// carries their true cost and turns, so nothing here special-cases them. Absent entirely on a
/// pre-cutover or early Run, whose `stages` list is empty rather than partially populated.
fn stage_panel(found: &RunView) -> String {
    if found.stages.is_empty() {
        return String::new();
    }
    let mut rows = String::new();
    for s in &found.stages {
        let cost_turns = match (s.cost_usd, s.turns) {
            (Some(c), Some(t)) => format!("${c:.2} · {t} turns"),
            (Some(c), None) => format!("${c:.2}"),
            (None, Some(t)) => format!("{t} turns"),
            (None, None) => String::new(),
        };
        rows.push_str(&format!(
            "<div class=\"g-a\"><div class=\"g-aline\"><span class=\"g-idx\">{}</span>\
<span class=\"g-verdict\">{}</span><span class=\"g-dur\">{cost_turns}</span></div>\
<div class=\"g-asub\">session {} · model {}</div></div>",
            esc(&s.name),
            esc(stage_status_label(s.status)),
            esc(&s.session_id),
            esc(s.model.as_deref().unwrap_or("(session default)")),
        ));
    }
    format!(
        "<div class=\"g-panel\"><div class=\"g-phead\">stages<span class=\"g-r\">rung ladder</span></div>{rows}</div>"
    )
}

fn stage_status_label(status: crate::rung::ReturnStatus) -> &'static str {
    match status {
        crate::rung::ReturnStatus::Complete => "complete",
        crate::rung::ReturnStatus::Skipped => "skipped",
        crate::rung::ReturnStatus::Incomplete => "incomplete",
    }
}

/// One tier-selection pass's receipt: tier, floor, personas, and the rationale rows that
/// produced them — a fact, never a grade (ADR-0003 applies to rendering too: no ✓/✗ over a
/// tier). Absent entirely when the stage never ran, which is `decision.json` never existing
/// rather than a zero value to show.
fn decision_panel(label: &str, decision: &Option<crate::decide::Decision>) -> String {
    let Some(decision) = decision else {
        return String::new();
    };
    let personas = if decision.personas.is_empty() {
        "none".to_string()
    } else {
        decision
            .personas
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut rows = String::new();
    for r in &decision.rationale {
        rows.push_str(&format!(
            "<div class=\"g-asub\">{} \u{2192} {} ({})</div>",
            esc(&r.signal),
            esc(&r.value),
            esc(&r.weight)
        ));
    }
    format!(
        "<div class=\"g-panel\"><div class=\"g-phead\">{} decision<span class=\"g-r\">tier {} · floor {}</span></div>\
<div style=\"padding:10px 12px\">personas: {}</div>{rows}</div>",
        esc(label),
        esc(&decision.tier.to_string()),
        esc(&decision.floor_from_plan.to_string()),
        esc(&personas)
    )
}

/// Reflect's calibration row, walked as plain key/value facts. Its shape is Reflect's own — the
/// pass's own words call it "statistics feeding the monthly audit," not a schema this base
/// forces — so this reads whatever keys the row carries. Absent entirely when Reflect wrote
/// none.
fn calibration_panel(calibration: &Option<serde_json::Value>) -> String {
    let Some(serde_json::Value::Object(map)) = calibration else {
        return String::new();
    };
    let mut rows = String::new();
    for (key, value) in map {
        rows.push_str(&format!(
            "<div class=\"g-asub\">{}: {}</div>",
            esc(key),
            esc(&json_text(value))
        ));
    }
    format!("<div class=\"g-panel\"><div class=\"g-phead\">calibration</div>{rows}</div>")
}

fn json_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `outcome.json`'s facts, once `grind outcomes` has collected them: merged/closed state and
/// any revert — never a grade on the Run. Absent entirely until the human runs `grind
/// outcomes`.
fn outcome_panel(outcome: &Option<crate::observe::RunOutcome>) -> String {
    let Some(o) = outcome else {
        return String::new();
    };
    let merged_at = o
        .pr_merged_at
        .as_ref()
        .map(|a| format!(" · merged at {}", esc(a)))
        .unwrap_or_default();
    let closed_at = o
        .pr_closed_at
        .as_ref()
        .map(|a| format!(" · closed at {}", esc(a)))
        .unwrap_or_default();
    let reverted = if o.reverted_by.is_empty() {
        String::new()
    } else {
        format!(" · reverted by {}", esc(&o.reverted_by.join(", ")))
    };
    format!(
        "<div class=\"g-panel\"><div class=\"g-phead\">outcome<span class=\"g-r\">{}</span></div>\
<div style=\"padding:10px 12px\">merged {}{merged_at}{closed_at}{reverted}</div></div>",
        esc(&o.pr_state),
        if o.pr_merged { "yes" } else { "no" },
    )
}

/// The stage table, decision receipts, calibration and outcome — Grit's own facts, folded into
/// one block so a Run with none of them (pre-cutover, or one that has not reached the ladder
/// yet) adds nothing to the page. Each panel is independently absent-safe.
fn grit_panels(facts: &Facts) -> String {
    format!(
        "{}{}{}{}{}",
        stage_panel(&facts.found),
        decision_panel("triage", &facts.triage_decision),
        decision_panel("diff-triage", &facts.diff_triage_decision),
        calibration_panel(&facts.calibration),
        outcome_panel(&facts.outcome),
    )
}

fn budget(found: &RunView) -> String {
    let (n, m) = found.attempt_counter();
    budget_dots(n, m)
}

fn run_head(facts: &Facts, live: &Live, here: &Observed<bool>) -> String {
    let found = &facts.found;
    let (lword, ldot) = liveness(here);
    let fresh = match live.freshness {
        Observed::Present(n) => format!("live · transcript active <b>{n}s ago</b>"),
        Observed::Absent => "transcript quiet".to_string(),
        Observed::Unobservable(_) => "transcript unobservable".to_string(),
    };
    let fanout = found
        .attempts
        .last()
        .and_then(|a| match a.fanout {
            Observed::Present((s, r)) => Some(format!(
                "<span>fan-out <b>{s}</b> spawned · <b>{r}</b> returned</span>"
            )),
            _ => None,
        })
        .unwrap_or_default();
    let blocker = facts
        .blocker
        .as_ref()
        .map(|b| {
            format!(
                "<span style=\"color:var(--hold)\">blocker: {}</span>",
                esc(b)
            )
        })
        .unwrap_or_default();
    format!(
        "<div class=\"g-head\"><div style=\"max-width:1010px;margin:0 auto;padding:0 16px\">\
<div class=\"g-h1\"><span class=\"g-t\">{}</span>\
<span class=\"g-st\"><span class=\"g-dot {ldot}\"></span><span class=\"g-badge {}\">{}</span></span>\
<span class=\"g-st g-dim\">{lword}</span><span class=\"g-fresh\">{fresh}</span></div>\
<div class=\"g-spec\">{}<span>spent <b>${:.2}</b></span><span class=\"g-link\">{}</span>\
<a class=\"g-link\" href=\"{}\">job</a>{fanout}<span>worktree <b>{}</b></span>{blocker}</div>\
</div></div>",
        esc(&found.job.title),
        badge_class(&found.state),
        esc(&found.state),
        budget(found),
        found.total_spend(),
        esc(&found.job.branch),
        esc(&found.job.url),
        esc(&found.worktree),
    )
}

fn crumb(run_id: &str) -> String {
    format!(
        "<div class=\"g-bar\"><div style=\"{SHELL_INNER}\">\
<span class=\"g-seg\"><span class=\"g-brand\">GRIND</span></span>\
<span class=\"g-seg\"><a class=\"g-back\" href=\"/\">\u{2190} roster</a><span class=\"g-faint\">/</span><span class=\"g-dim\">runs</span><span>{}</span></span>\
<span class=\"g-seg g-dim\" id=\"clk\">--:--:-- UTC</span>\
</div></div>",
        esc(run_id)
    )
}

fn run_grid(run_id: &str, facts: &Facts) -> String {
    format!(
        "<div class=\"g-panel\"><div class=\"g-phead\">attempt timeline<span class=\"g-r\">wall clock</span></div>{}</div>\
<div class=\"g-panel\" style=\"margin-top:12px\"><div class=\"g-phead\">attempts<span class=\"g-r\">newest first · evidence verbatim</span></div>{}</div>",
        waterfall(&facts.found),
        attempt_list(run_id, &facts.found)
    )
}

pub fn run_fragment(run_id: &str, facts: &Facts, live: &Live, here: &Observed<bool>) -> String {
    let any_live = matches!(facts.found.state.as_str(), "dispatched" | "rate_limited");
    let side = format!("{}{}", last_words_block(live), grit_panels(facts));
    format!(
        "<div data-g-root=\"run\" data-live=\"{}\">{}\
<div style=\"max-width:1010px;margin:0 auto;padding:12px 16px\">\
<div class=\"g-grid\" style=\"display:grid;grid-template-columns:1fr 396px;gap:12px;align-items:start\">\
<div style=\"min-width:0\">{}</div><div>{side}</div></div></div>\
<div class=\"g-panel g-jump\" style=\"max-width:1010px;margin:0 auto\"><div class=\"g-phead\">supervisor.log<span class=\"g-r g-follow\">\u{25aa} following</span></div>\
<div data-g-root=\"log\"><div class=\"g-log\" data-offset=\"0\"></div><div class=\"g-pill\">\u{2193} following</div></div></div></div>{}",
        if any_live { "true" } else { "false" },
        run_head(facts, live, here),
        run_grid(run_id, facts),
        clearance_panel(facts)
    )
}

pub fn run_page(run_id: &str, facts: &Facts, live: &Live, here: &Observed<bool>) -> String {
    let side = format!("{}{}", last_words_block(live), grit_panels(facts));
    let body = format!(
        "{}{}<div style=\"max-width:1010px;margin:0 auto;padding:12px 16px\">\
<div class=\"g-grid\" style=\"display:grid;grid-template-columns:1fr 396px;gap:12px;align-items:start\">\
<div style=\"min-width:0\">{}</div>\
<div>{}{side}</div></div></div>",
        crumb(run_id),
        run_head(facts, live, here),
        run_grid(run_id, facts),
        "<div class=\"g-panel g-jump\"><div class=\"g-phead\">supervisor.log\
<span class=\"g-r g-follow\">\u{25aa} following</span></div>\
<div data-g-root=\"log\"><div class=\"g-log\" data-offset=\"0\"></div>\
<div class=\"g-pill\">\u{2193} following</div></div></div>",
    );
    layout(
        &format!("grind \u{b7} {}", esc(run_id)),
        &format!("{body}{}", status_bar()),
    ) + &clearance_panel(facts)
}

/// The 404 body. Names nothing about what exists.
pub fn not_found() -> String {
    "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>grind</title></head>\
<body><pre>no such page</pre></body></html>"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decide::{Stage, Verdict, VerifyContract};
    use crate::view::Facts;

    fn row(state: &str, id: &str) -> RosterRow {
        RosterRow {
            run_id: id.to_string(),
            recorded_state: state.to_string(),
            branch: "feat/x".to_string(),
            job_url: "https://example.test/issues/1".to_string(),
            supervisor_here: Observed::Present(true),
            attempts: (2, 8),
            job_title: "t".to_string(),
            spend: 1.5,
            last_activity: "2026-08-06T12:26:20+00:00".to_string(),
        }
    }

    #[test]
    fn hostile_payload_renders_inert() {
        let mut r = row("dispatched", "20260821-100201-x-52");
        r.job_title = "<script>alert(1)</script>".to_string();
        let html = roster_fragment(&[r]);
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn badge_carries_the_recorded_state_verbatim() {
        let html = roster_fragment(&[row("weird_state", "id1")]);
        assert!(html.contains(">weird_state</span>"));
        assert!(html.contains("g-b-done"));
    }

    #[test]
    fn unobservable_liveness_is_never_stopped() {
        let mut r = row("dispatched", "id2");
        r.supervisor_here = Observed::Unobservable(crate::observe::Reason::saying("no ps"));
        let html = roster_fragment(&[r]);
        assert!(html.contains("unobservable"));
        assert!(!html.contains(">stopped<"));
    }

    #[test]
    fn fragments_are_not_pages() {
        let html = roster_fragment(&[row("died", "id3")]);
        assert!(!html.contains("<html"));
        assert!(html.contains("data-g-root=\"board\""));
    }

    #[test]
    fn board_cadence_follows_liveness() {
        let live_html = roster_fragment(&[row("dispatched", "a"), row("completed", "b")]);
        assert!(live_html.contains("data-live=\"true\""));
        let idle_html = roster_fragment(&[row("completed", "a"), row("exhausted", "b")]);
        assert!(idle_html.contains("data-live=\"false\""));
    }

    #[test]
    fn waterfall_positions_are_monotonic_in_time() {
        assert!(pct(10, 0, 100) < pct(50, 0, 100));
        assert_eq!(pct(50, 0, 100), 50.0);
        assert_eq!(pct(99, 0, 100), 99.0);
    }

    #[test]
    fn iso_epochs_are_correct() {
        assert_eq!(iso_epoch("1970-01-01T00:00:00+00:00"), Some(0));
        assert_eq!(iso_epoch("2024-02-29T00:00:00+00:00"), Some(1_709_164_800));
        assert_eq!(iso_epoch("2024-03-01T00:00:00+00:00"), Some(1_709_251_200));
        assert!(iso_epoch("2026-08-06T12:26:20+00:00").is_some());
        assert_eq!(iso_epoch("garbage"), None);
    }

    fn day_one() -> RunView {
        serde_json::from_str::<RunView>(include_str!("../tests/fixtures/record/day-one.json"))
            .expect("fixture parses")
    }

    fn facts_of(found: RunView) -> Facts {
        Facts {
            found,
            observation: crate::observe::Observation {
                observed_at: "2026-08-06T12:00:00+00:00".to_string(),
                commits_ahead: Observed::Absent,
                tree_clean: Observed::Absent,
                pr: Observed::Absent,
                checks_pending: Observed::Absent,
                checks_red: Observed::Absent,
                plan_files: Observed::Absent,
                residual_findings: Observed::Absent,
                ledger_entries: Observed::Absent,
                changed_files: Observed::Absent,
                base_drift: Observed::Absent,
                pr_head_matches_job_branch: Observed::Absent,
                pr_base_matches_declared: Observed::Absent,
            },
            verdict: Verdict::Incomplete(Vec::new()),
            contract: VerifyContract {
                present: Vec::new(),
                missing: Vec::new(),
            },
            coverage: Observed::Absent,
            furthest: Stage::Dispatched,
            blocker: None,
            cleared: None,
            run_state: std::path::PathBuf::new(),
            triage_decision: None,
            diff_triage_decision: None,
            outcome: None,
            calibration: None,
        }
    }

    fn live_of() -> Live {
        Live {
            transcript: std::path::PathBuf::new(),
            now_skill: Observed::Absent,
            assistant_now: Observed::Absent,
            last_words: vec!["one".to_string(), String::new(), String::new()],
            fanout: Observed::Absent,
            freshness: Observed::Present(8),
        }
    }

    #[test]
    fn day_one_renders_the_cockpit() {
        let found = day_one();
        let facts = facts_of(found.clone());
        let here = Observed::<bool>::Present(false);
        let html = run_fragment("20260806-120000-x-1", &facts, &live_of(), &here);
        assert!(html.contains("data-g-root=\"run\""));
        assert!(html.contains("attempt timeline"));
        assert!(html.contains("last words"));
        assert!(html.contains("transcript active"));
        assert!(!html.contains(">cleared<"));
    }

    #[test]
    fn roster_renders_day_one_card() {
        let found = day_one();
        let r = RosterRow {
            run_id: found.run_id.clone(),
            recorded_state: found.state.clone(),
            branch: found.job.branch.clone(),
            job_url: found.job.url.clone(),
            supervisor_here: Observed::Present(false),
            attempts: found.attempt_counter(),
            job_title: found.job.title.clone(),
            spend: found.total_spend(),
            last_activity: "2026-08-06T17:41:55+00:00".to_string(),
        };
        let html = roster_fragment(&[r]);
        assert!(html.contains(&format!("data-gid=\"{}\"", esc(&found.run_id))));
        assert!(html.contains(&esc(&found.job.title)));
    }

    #[test]
    fn pages_carry_the_shell_and_hooks() {
        let html = roster_page(&[], &[]);
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("/style.css"));
        assert!(html.contains("/script.js"));
        assert!(html.contains("data-g-stamp"));
        assert!(html.contains("data-g-pause"));
        assert!(html.contains("id=\"clk\""));
        let found = day_one();
        let here = Observed::<bool>::Unobservable(crate::observe::Reason::saying("x"));
        let run = run_page("r", &facts_of(found), &live_of(), &here);
        assert!(run.contains("unobservable"));
        assert!(run.contains("data-g-root=\"log\""));
    }

    fn stages_two() -> Vec<crate::rung::StageEntry> {
        vec![
            crate::rung::StageEntry {
                name: "plan".to_string(),
                session_id: "run-1-plan".to_string(),
                status: crate::rung::ReturnStatus::Complete,
                artifact_paths: vec![],
                model: Some("claude-sonnet-5".to_string()),
                cost_usd: Some(1.23),
                turns: Some(4),
            },
            crate::rung::StageEntry {
                name: "triage".to_string(),
                session_id: "[R]".to_string(),
                status: crate::rung::ReturnStatus::Complete,
                artifact_paths: vec![],
                model: None,
                cost_usd: Some(0.0),
                turns: Some(0),
            },
        ]
    }

    #[test]
    fn a_run_page_with_two_stage_entries_renders_both_rows() {
        let mut found = day_one();
        found.stages = stages_two();
        let html = run_fragment(
            "id",
            &facts_of(found),
            &live_of(),
            &Observed::Present(false),
        );
        assert!(html.contains("run-1-plan"), "{html}");
        assert!(html.contains("[R]"), "{html}");
        assert!(html.contains("$1.23"), "{html}");
    }

    #[test]
    fn a_run_page_with_no_stages_carries_no_stage_panel() {
        let html = run_fragment(
            "id",
            &facts_of(day_one()),
            &live_of(),
            &Observed::Present(false),
        );
        assert!(!html.contains("rung ladder"), "{html}");
    }

    fn decision_literal() -> crate::decide::Decision {
        crate::decide::Decision {
            tier: crate::decide::Tier::T1,
            personas: vec![crate::decide::Persona::Correctness],
            depth: crate::decide::PlanReviewDepth { reviewers: 1 },
            model_per_stage: std::collections::BTreeMap::new(),
            floor_from_plan: crate::decide::Tier::T0,
            rationale: vec![crate::decide::RationaleRow {
                signal: "loc_changed".to_string(),
                value: "180".to_string(),
                weight: "t1".to_string(),
            }],
        }
    }

    #[test]
    fn a_decision_literal_renders_inert_in_its_own_panel() {
        let html = decision_panel("triage", &Some(decision_literal()));
        assert!(html.contains("triage decision"), "{html}");
        assert!(html.contains("tier t1"), "{html}");
        assert!(html.contains("loc_changed"), "{html}");
        assert_eq!(decision_panel("triage", &None), "");
    }

    #[test]
    fn hostile_rationale_text_renders_inert_in_the_decision_panel() {
        let mut decision = decision_literal();
        decision.rationale[0].value = "<script>alert(1)</script>".to_string();
        let html = decision_panel("triage", &Some(decision));
        assert!(!html.contains("<script>"), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
    }

    #[test]
    fn an_empty_run_dir_adds_no_grit_panels() {
        let html = run_fragment(
            "id",
            &facts_of(day_one()),
            &live_of(),
            &Observed::Present(false),
        );
        for absent in ["decision", "calibration", ">outcome<"] {
            assert!(
                !html.contains(absent),
                "`{absent}` on an empty run dir:\n{html}"
            );
        }
    }

    #[test]
    fn the_calibration_panel_renders_key_value_facts() {
        let row = serde_json::json!({"tier": "t1", "confirmed": "P1"});
        let html = calibration_panel(&Some(row));
        assert!(html.contains("calibration"), "{html}");
        assert!(html.contains("tier: t1"), "{html}");
        assert_eq!(calibration_panel(&None), "");
    }

    #[test]
    fn the_outcome_panel_renders_the_merged_and_reverted_facts() {
        let outcome = crate::observe::RunOutcome {
            collected_at: "2026-09-01T00:00:00+00:00".to_string(),
            pr_state: "MERGED".to_string(),
            pr_merged: true,
            pr_merged_at: Some("2026-08-20T00:00:00+00:00".to_string()),
            pr_closed_at: None,
            reverted_by: vec!["abc123".to_string()],
            followup_issues: vec![],
        };
        let html = outcome_panel(&Some(outcome));
        assert!(html.contains("MERGED"), "{html}");
        assert!(html.contains("merged yes"), "{html}");
        assert!(html.contains("abc123"), "{html}");
        assert_eq!(outcome_panel(&None), "");
    }

    #[test]
    fn the_proposal_queue_section_renders_each_entrys_kind_and_summary() {
        let proposals = vec![(
            "20260901-000000-snapper-40".to_string(),
            ProposalEntry {
                kind: "job",
                path: std::path::PathBuf::from("/home/op/.grind/runs/x/stages/reflect/jobs/a.md"),
                summary: "Fix the residual finding in observe.rs".to_string(),
            },
        )];
        let html = proposal_queue_section(&proposals);
        assert!(html.contains("proposal queue"), "{html}");
        assert!(html.contains("20260901-000000-snapper-40"), "{html}");
        assert!(html.contains("Fix the residual finding"), "{html}");
        assert_eq!(proposal_queue_section(&[]), "");
    }

    #[test]
    fn the_roster_page_carries_the_proposal_queue_section() {
        let proposals = vec![(
            "r1".to_string(),
            ProposalEntry {
                kind: "diff",
                path: std::path::PathBuf::from("/x/diffs/a.diff"),
                summary: "wording tweak".to_string(),
            },
        )];
        let html = roster_page(&[], &proposals);
        assert!(html.contains("proposal queue"), "{html}");
        assert!(html.contains("wording tweak"), "{html}");
    }

    #[test]
    fn a_claude_code_run_links_the_three_files_that_adapter_writes() {
        let found = day_one();
        assert_eq!(found.backend, crate::runner::Backend::ClaudeCode);
        let html = attempt_list("id", &found);
        assert!(html.contains("attempt-1.prompt.txt"), "{html}");
        assert!(html.contains("attempt-1.stdout.json"), "{html}");
        assert!(html.contains("attempt-1.stderr.log"), "{html}");
        assert!(!html.contains("messages-1.jsonl"), "{html}");
    }

    #[test]
    fn a_native_run_links_only_the_messages_transcript_it_actually_writes() {
        let mut found = day_one();
        found.backend = crate::runner::Backend::Native;
        let html = attempt_list("id", &found);
        assert!(html.contains("messages-1.jsonl"), "{html}");
        assert!(
            !html.contains("attempt-1.prompt.txt"),
            "the native adapter never writes this file: {html}"
        );
        assert!(
            !html.contains("attempt-1.stdout.json"),
            "the native adapter never writes this file: {html}"
        );
        assert!(
            !html.contains("attempt-1.stderr.log"),
            "the native adapter never writes this file: {html}"
        );
    }
    /// The row must present the file this attempt actually wrote. A re-entered attempt 2 that
    /// found slot 1 taken writes `messages-2-2.jsonl`, and the computed `messages-2.jsonl` is
    /// the *dead* attempt's transcript — a link under attempt 2's own heading to somebody
    /// else's record (issue #156).
    #[test]
    fn a_native_attempt_links_the_transcript_name_it_recorded() {
        let mut found = day_one();
        found.backend = crate::runner::Backend::Native;
        let dead = found
            .attempts
            .iter()
            .position(|a| a.n == 2)
            .expect("an attempt 2");
        let mut retry = found.attempts[dead].clone();
        retry.transcript = crate::attempt::Transcript::Recorded("messages-2-2.jsonl".to_string());
        found.attempts = vec![found.attempts[dead].clone(), retry];

        let html = attempt_list("id", &found);
        assert_eq!(
            html.matches("/raw/runs/id/messages-2-2.jsonl").count(),
            1,
            "the retry links the file it wrote: {html}"
        );
        assert_eq!(
            html.matches("/raw/runs/id/messages-2.jsonl\"").count(),
            1,
            "the slot-1 file belongs to the attempt that died, and to it alone: {html}"
        );
    }

    /// Every record written before the name was recorded, and every attempt that died before
    /// allocating one, carries `None` — the computed name is what those rows have always
    /// linked and it stays exactly right for them.
    #[test]
    fn a_native_attempt_with_no_recorded_name_falls_back_to_the_computed_one() {
        let mut found = day_one();
        found.backend = crate::runner::Backend::Native;
        assert_eq!(
            found.attempts[0].transcript,
            crate::attempt::Transcript::PredatesName,
            "fixture is old-shaped"
        );
        let html = attempt_list("id", &found);
        assert!(html.contains("messages-1.jsonl"), "{html}");
    }

    /// The endpoint-resolution failure synthesizes its attempt before `allocate_transcript`
    /// is reached, so no file was ever written under any name (issue #161). Today's fallback
    /// serves `/raw/runs/<id>/messages-N.jsonl` for exactly these rows — a URL backed by
    /// nothing, on the row where the reader most wants to know why no transcript exists.
    ///
    /// Pinned differentially: the wrote-none row must be byte-identical to the predates-name
    /// row except for the evidence div, so the link alone disappears and everything else the
    /// row ever carried stays.
    #[test]
    fn a_native_attempt_that_wrote_no_transcript_renders_no_link() {
        let mut found = day_one();
        found.backend = crate::runner::Backend::Native;
        let dead = found
            .attempts
            .iter()
            .position(|a| a.n == 2)
            .expect("an attempt 2");
        let base = found.attempts[dead].clone();
        let mut wrote_none = base.clone();
        wrote_none.transcript = crate::attempt::Transcript::WroteNone;
        let mut predates = base.clone();
        predates.transcript = crate::attempt::Transcript::PredatesName;

        found.attempts = vec![wrote_none];
        let without = attempt_list("id", &found);
        found.attempts = vec![predates];
        let fallback = attempt_list("id", &found);

        assert!(
            !without.contains("/raw/runs/id/messages-"),
            "a transcript-less attempt must not render a link to a file that was never written: {without}"
        );
        assert!(
            !without.contains("messages.jsonl"),
            "no placeholder stands in for the missing link either: {without}"
        );

        // Everything but the link div survives verbatim.
        let linked_segment_start = fallback
            .find("<div class=\"g-ev\">")
            .expect("the fallback row links");
        let linked_segment_end = linked_segment_start
            + fallback[linked_segment_start..]
                .find("</div>")
                .expect("the div closes")
            + "</div>".len();
        let fallback_minus_link = format!(
            "{}{}",
            &fallback[..linked_segment_start],
            &fallback[linked_segment_end..]
        );
        assert_eq!(
            without, fallback_minus_link,
            "removing only the transcript link leaves the row byte-identical"
        );
        // And the row really does carry the things a reader needs: index, verdict,
        // duration, and the terminal reason (`endpoint-resolution failure` sets
        // `terminal_reason`; the fixture's attempt 2 carries none, so the row shows
        // its `subtype`).
        assert!(without.contains("#2"), "{without}");
        assert!(without.contains("unparseable"), "{without}");
        assert!(without.contains("4m"), "{without}");
        let mut with_reason = base.clone();
        with_reason.transcript = crate::attempt::Transcript::WroteNone;
        with_reason.terminal_reason = Some("endpoint resolution failed: no key".to_string());
        found.attempts = vec![with_reason];
        let reasoned = attempt_list("id", &found);
        assert!(
            reasoned.contains("endpoint resolution failed"),
            "the terminal reason survives on a wrote-none row: {reasoned}"
        );
        assert!(
            !reasoned.contains("/raw/runs/id/messages-"),
            "and still no transcript link: {reasoned}"
        );
    }

    /// The claude-code trio is determined by `n` alone, so a recorded name — which that
    /// adapter never sets — must not reach it even if one somehow appeared.
    #[test]
    fn a_claude_code_attempts_trio_ignores_any_recorded_transcript_name() {
        let mut found = day_one();
        assert_eq!(found.backend, crate::runner::Backend::ClaudeCode);
        // The trio is determined by `n` alone, however the transcript fact stands: a
        // recorded name that adapter never sets, and the wrote-none fact it never
        // reaches either.
        for wrote in [
            crate::attempt::Transcript::Recorded("messages-1-2.jsonl".to_string()),
            crate::attempt::Transcript::WroteNone,
        ] {
            let mut found = found.clone();
            found.attempts[0].transcript = wrote;
            let html = attempt_list("id", &found);
            assert!(html.contains("attempt-1.prompt.txt"), "{html}");
            assert!(html.contains("attempt-1.stdout.json"), "{html}");
            assert!(html.contains("attempt-1.stderr.log"), "{html}");
            assert!(!html.contains("messages-1-2.jsonl"), "{html}");
            assert!(
                !html.contains("/raw/runs/id/messages-"),
                "the claude-code row never links any messages file at all: {html}"
            );
        }
    }
}
