//! Pure HTML rendering for the dashboard (plan U3). Every byte the server sends as HTML
//! is a `String` built here; mirrors `render`'s discipline — pure functions, tested
//! wording, escaping mandatory for record-derived strings (ADR-0014). The design contract
//! is the user-approved cockpit mockups: kanban roster, cockpit Run page with an attempt
//! waterfall (KTD15). Classes are `g-`-prefixed and bind to `style.rs`; DOM hooks
//! (`data-g-root`, `data-gid`, `data-epoch`, …) bind to `script.rs`.

use crate::attempt::Attempt;
use crate::observe::Observed;
use crate::view::{Facts, Live, RosterRow, RunView};

// ───────────────────────────── escaping ─────────────────────────────

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

// ───────────────────────────── time ─────────────────────────────

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

// ─────────────────────────── classification ───────────────────────────

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
        // died / unobserved / uncorroborated — and anything unrecognized: an unknown
        // recorded state must still be seen, so it lands where attention goes.
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

// ───────────────────────────── board ─────────────────────────────

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
        // Grouped by the **recorded state** itself — the column's display label is
        // applied when the column renders, never when rows are filed (a "complete" label
        // keyed where the record says "completed" loses every card to a silent miss).
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
        // Known columns first, in house order. The hold lane always carries its second
        // slot even when empty: it names the repair path (`grind cleared`, then resume).
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
        // Unrecognized recorded states appended after the known ones, labeled verbatim.
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

pub fn roster_page(rows: &[RosterRow]) -> String {
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
            "{}{}<div style=\"max-width:1010px;margin:0 auto;padding:0 16px\">{}</div>{}",
            command_bar(),
            telemetry(rows),
            roster_fragment(rows),
            status_bar()
        ),
    )
}

// ───────────────────────────── run page ─────────────────────────────

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
    // Sleep bands: a bounded re-entry sleep between two Attempts reads as hatched time,
    // because it is the wall the Run slept against — the thing ADR-0004 refuses to hide.
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
        out.push_str(&format!(
            "<div class=\"g-a\"><div class=\"g-aline\"><span class=\"g-idx\">#{}</span>\
<span class=\"g-verdict {vcls}\">{word}</span><span class=\"g-dur\">{}{}</span></div>\
{reason}\
<div class=\"g-ev\"><a class=\"g-link\" href=\"/raw/runs/{}/attempt-{}.prompt.txt\">prompt.txt</a>\
<a class=\"g-link\" href=\"/raw/runs/{}/attempt-{}.stdout.json\">stdout.json</a>\
<a class=\"g-link\" href=\"/raw/runs/{}/attempt-{}.stderr.log\">stderr.log</a></div></div>",
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
            esc(run_id),
            a.n,
            esc(run_id),
            a.n,
            esc(run_id),
            a.n
        ));
    }
    if out.is_empty() {
        out = "<div class=\"g-a g-dim\">nothing recorded</div>".to_string();
    }
    out
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
    format!(
        "<div data-g-root=\"run\" data-live=\"{}\">{}\
<div style=\"max-width:1010px;margin:0 auto;padding:12px 16px\">\
<div class=\"g-grid\" style=\"display:grid;grid-template-columns:1fr 396px;gap:12px;align-items:start\">\
<div style=\"min-width:0\">{}</div><div>{}</div></div></div>\
<div class=\"g-panel g-jump\" style=\"max-width:1010px;margin:0 auto\"><div class=\"g-phead\">supervisor.log<span class=\"g-r g-follow\">\u{25aa} following</span></div>\
<div data-g-root=\"log\"><div class=\"g-log\" data-offset=\"0\"></div><div class=\"g-pill\">\u{2193} following</div></div></div></div>{}",
        if any_live { "true" } else { "false" },
        run_head(facts, live, here),
        run_grid(run_id, facts),
        last_words_block(live),
        clearance_panel(facts)
    )
}

pub fn run_page(run_id: &str, facts: &Facts, live: &Live, here: &Observed<bool>) -> String {
    let body = format!(
        "{}{}<div style=\"max-width:1010px;margin:0 auto;padding:12px 16px\">\
<div class=\"g-grid\" style=\"display:grid;grid-template-columns:1fr 396px;gap:12px;align-items:start\">\
<div style=\"min-width:0\">{}</div>\
<div>{}{}</div></div></div>",
        crumb(run_id),
        run_head(facts, live, here),
        run_grid(run_id, facts),
        // The following pane lives on the full page too, so a deep link lands
        // with the log already attached.
        "<div class=\"g-panel g-jump\"><div class=\"g-phead\">supervisor.log\
<span class=\"g-r g-follow\">\u{25aa} following</span></div>\
<div data-g-root=\"log\"><div class=\"g-log\" data-offset=\"0\"></div>\
<div class=\"g-pill\">\u{2193} following</div></div></div>",
        last_words_block(live)
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
        let html = roster_page(&[]);
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
}
