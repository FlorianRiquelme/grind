//! How a Run reads to a human. **Every function returns a `String`; `cli` prints.**
//!
//! That is what makes *status degrades, never fails* an assertion rather than an intention — a
//! rendered view is a value a test can compare in full, so line order and the fixed height of
//! the last-words block are checkable rather than hoped for.
//!
//! **Verdict language describes what happened, never quality** (ADR-0003). Check every string
//! this module emits against that rule; there is a test at the bottom that does.

use crate::decide::{Stage, Verdict, VerifyContract};
use crate::observe::{Observation, Observed, Outcome, UNOBSERVABLE_MARK};
use crate::view::{Facts, Live, RosterRow, RunView};
use std::path::Path;

/// One item of doctor's report, as `cli` hands it over: the name and the depth mark alongside
/// the classified result, so this module needs no edge to the module that owns the list.
pub struct DoctorLine<'a> {
    pub name: &'a str,
    pub mark: &'a str,
    pub outcome: Observed<Outcome>,
}

/// Everything the single-Run view is composed from. A named struct rather than eight
/// arguments, so a new line's input is `E0063` at the call site rather than a positional slot
/// somebody transposes.
pub struct SingleRun<'a> {
    pub found: &'a RunView,
    pub observation: &'a Observation,
    pub live: &'a Live,
    pub verdict: &'a Verdict,
    pub contract: &'a VerifyContract,
    pub furthest: Stage,
    pub supervisor_here: &'a Observed<bool>,
    pub run_state: &'a Path,
}

/// The single-Run view: **alive, where, stuck, and about to cost something**, top to bottom,
/// with no follow-up needed. Thirty seconds of looking is the whole budget.
///
/// The line order is fixed and the last-words block is exactly three lines, so
/// `watch -n 30 grind status <id>` never jitters and the operator's eye can park on one row.
pub fn run_view(view: &SingleRun) -> String {
    let SingleRun {
        found,
        observation,
        live,
        verdict,
        contract,
        furthest,
        supervisor_here,
        run_state,
    } = view;
    let furthest = *furthest;
    let (made, budget) = found.attempt_counter();
    let mut out = String::new();
    line(
        &mut out,
        &format!("Run     {}  [{}]", found.run_id, found.state),
    );
    line(
        &mut out,
        &format!(
            "Host    {}   supervisor {} {}",
            found.hostname,
            found.supervisor_pid,
            presence_word(supervisor_here)
        ),
    );
    line(&mut out, &format!("Job     {}", found.job.url));
    line(
        &mut out,
        &format!(
            "Branch  {}  (worktree {})",
            found.job.branch, found.worktree
        ),
    );
    line(&mut out, &format!("Session {}", found.session_id));
    line(&mut out, &format!("Model   {}", model_of(found)));
    line(&mut out, "");
    line(
        &mut out,
        &format!("  verdict           {}", verdict_line(verdict, observation)),
    );
    // Two separate stage lines. *How far it got* and *what it is doing* are never conflated.
    line(&mut out, &format!("  furthest stage    {furthest}"));
    line(&mut out, &format!("  now               {}", live.now_skill));
    line(
        &mut out,
        &format!("  progress          {}", freshness_line(&live.freshness)),
    );
    line(
        &mut out,
        &format!("  fan-out           {}", fanout_line(live)),
    );
    line(
        &mut out,
        &format!("  attempts          attempt {made} of {budget}"),
    );
    // The API-pricing counterfactual. Remaining quota prints not at all: the number nothing can
    // compute is not estimated.
    line(
        &mut out,
        &format!(
            "  spend             ${:.2} (API pricing)",
            found.total_spend()
        ),
    );
    line(
        &mut out,
        &format!("  commits ahead     {}", observation.commits_ahead),
    );
    line(&mut out, &format!("  PR                {}", observation.pr));
    line(
        &mut out,
        &format!("  tree clean        {}", observation.tree_clean),
    );
    line(
        &mut out,
        &format!("  checks pending    {}", observation.checks_pending),
    );
    line(
        &mut out,
        &format!("  verify contract   {}", contract_line(contract)),
    );
    line(&mut out, "");
    line(&mut out, "  last words");
    for said in live.last_words.iter().take(3) {
        line(&mut out, &format!("    {said}"));
    }
    line(&mut out, "");
    line(
        &mut out,
        &format!("  transcript        {}", live.transcript.display()),
    );
    line(
        &mut out,
        &format!("  run state         {}", run_state.display()),
    );
    out
}

/// The roster. It says which host it is speaking for, because Run state does not travel.
pub fn roster(hostname: &str, rows: &[RosterRow]) -> String {
    let mut out = String::new();
    line(&mut out, &format!("Runs on {hostname} — this host only."));
    line(&mut out, "");
    if rows.is_empty() {
        line(&mut out, "  no Runs here.");
        return out;
    }
    for row in rows {
        line(
            &mut out,
            &format!(
                "  {}  {:<14} supervisor {:<9} attempt {} of {}  {}",
                row.run_id,
                row.recorded_state,
                presence_word(&row.supervisor_here),
                row.attempts.0,
                row.attempts.1,
                row.branch
            ),
        );
        line(&mut out, &format!("      {}", row.job_url));
    }
    out
}

/// A run id this host has never held. Not an error, and not a typo — a pointer to where to
/// look instead.
pub fn not_here(run_id: &str, hostname: &str) -> String {
    format!(
        "Run `{run_id}` is not on {hostname}.\n\nRun state does not travel. The Job issue carries \
         the pointer to the host that holds it.\n"
    )
}

/// One line of the observation block, with the mark it came back with.
struct Row {
    label: &'static str,
    value: String,
    unobserved: bool,
}

/// What a finished Run leaves for the human to pick up. Its shape is what the morning costs.
///
/// **Five claims and nothing else at that weight** (#16). Everything that only points at
/// something moves to the trailing block, everything that is a permanent negative does not
/// print at all, and everything that could not be observed groups where the eye can see that it
/// is a different kind of row rather than a mark down a column it reads as uniform.
pub fn handback(view: &Facts) -> String {
    let Facts {
        found,
        observation,
        verdict,
        contract,
        coverage,
        furthest,
        blocker,
        run_state,
    } = view;
    let furthest = *furthest;
    let blocker = blocker.as_deref();
    let mut out = String::new();

    // The fresh verdict, in the top position, and the recorded state nowhere. Where the two
    // disagree the fresh one is right by construction, and printing both asks the human to
    // adjudicate between two things Grind produced.
    line(
        &mut out,
        &format!(
            "Verdict  {}",
            handback_verdict(verdict, observation, found, blocker)
        ),
    );
    line(&mut out, "");
    line(&mut out, &format!("Run      {}", found.run_id));
    line(&mut out, &format!("Job      {}", found.job.url));
    line(&mut out, &format!("Branch   {}", found.job.branch));
    line(&mut out, &format!("Model    {}", model_of(found)));
    let (made, budget) = found.attempt_counter();
    line(
        &mut out,
        &format!(
            "Attempts {made} of {budget}   spend ${:.2} (API pricing)   tool denials {}",
            found.total_spend(),
            found.denial_count()
        ),
    );
    line(&mut out, "");

    let mut rows = vec![
        Row {
            label: "furthest stage",
            value: furthest.to_string(),
            unobserved: false,
        },
        row("commits ahead", &observation.commits_ahead),
        row("PR", &observation.pr),
        row("tree clean", &observation.tree_clean),
        row("checks pending", &observation.checks_pending),
        Row {
            label: "verify contract",
            value: contract_line(contract),
            unobserved: false,
        },
    ];
    // The same rule five times: **surface the surprise, never the permanent negative.** A row
    // that could not be observed still prints — in the block below — because *I could not look*
    // is a surprise too.
    if found.denial_count() > 0 {
        rows.push(Row {
            label: "denied",
            value: denied_invocations(found).join("; "),
            unobserved: false,
        });
    }
    if let Observed::Present(open) = &observation.pr
        && open.is_draft
    {
        rows.push(Row {
            label: "draft",
            value: "yes".to_string(),
            unobserved: false,
        });
    }
    if let Some(drift) = surprising(&observation.base_drift, |d| d.commits > 0) {
        rows.push(Row {
            label: "base drift",
            value: drift.0,
            unobserved: drift.1,
        });
    }
    // Two integers, and **no summary, boolean or health word over them**. A count of processes
    // must never become an assertion about a review.
    match fanout_totals(found) {
        FanoutTotals {
            counted: Some(pair),
            unread: None,
        } if pair.0 > 0 => rows.push(Row {
            label: "fan-out",
            value: fanout_counted(pair),
            unobserved: false,
        }),
        // **Counted and incomplete.** The number stays, because it is real; the mark stays with
        // it, because an understated total printed bare is indistinguishable from a low one.
        FanoutTotals {
            counted: Some(pair),
            unread: Some(reason),
        } => rows.push(Row {
            label: "fan-out",
            value: format!(
                "{}  {UNOBSERVABLE_MARK} at least one attempt unread: {reason}",
                fanout_counted(pair)
            ),
            unobserved: true,
        }),
        FanoutTotals {
            counted: None,
            unread: Some(reason),
        } => rows.push(Row {
            label: "fan-out",
            value: format!("{UNOBSERVABLE_MARK}  {reason}"),
            unobserved: true,
        }),
        _ => {}
    }
    if let Some(estimate) = surprising(coverage, |c| !c.uncovered.is_empty()) {
        rows.push(Row {
            label: "verify coverage",
            value: estimate.0,
            unobserved: estimate.1,
        });
    }

    for seen in rows.iter().filter(|r| !r.unobserved) {
        line(&mut out, &format!("  {:<17} {}", seen.label, seen.value));
    }
    // Empty on a Run where nothing failed to observe, and the Handback is flat.
    let blind: Vec<&Row> = rows.iter().filter(|r| r.unobserved).collect();
    if !blind.is_empty() {
        line(&mut out, "");
        line(&mut out, "  could not observe");
        for row in blind {
            line(&mut out, &format!("    {:<15} {}", row.label, row.value));
        }
    }

    // Things you type at something, rather than claims about the world. The session handle is
    // worthless off its host, and the worktree path is a place rather than a fact.
    line(&mut out, "");
    line(
        &mut out,
        &format!("  session          {}", found.session_id),
    );
    line(&mut out, &format!("  worktree         {}", found.worktree));
    line(
        &mut out,
        &format!("  run state        {}", run_state.display()),
    );
    out
}

/// **The account that leaves the host**, over the same fact set the Handback renders.
///
/// A terminal wants fixed width; markdown wants a table. Two independently-chosen lists would
/// drift *invisibly*, because nobody ever sees both renderings of one Run — which is why this
/// takes [`Facts`] rather than composing its own.
///
/// **It carries each observation's three-valued mark and never its `Reason`.** `Reason::of`
/// composes `<call site>: exit N: <first stderr line>`, so a reason is raw child stderr, and
/// `observe` already forbids rendering that for host checks on the grounds that a misprovisioned
/// host is exactly where an HTTPS `origin` embeds a token. The Handback prints reasons on the
/// host, where the human already has them; a comment that is appended and never edited must not.
///
/// **No summary boolean.** A public surface is bound at least as hard as a private one.
///
/// Host and run-state path are published deliberately: the audience is the one already trusted
/// with the dispatch comment, which named both.
pub fn job_comment(view: &Facts) -> String {
    let Facts {
        found,
        observation,
        verdict,
        contract,
        coverage,
        furthest,
        blocker,
        run_state,
    } = view;
    let (made, budget) = found.attempt_counter();
    let mut out = String::new();
    line(
        &mut out,
        &format!("**Run `{}` on `{}`**", found.run_id, found.hostname),
    );
    line(&mut out, "");
    line(
        &mut out,
        &handback_verdict(&off_host(verdict), observation, found, blocker.as_deref()),
    );
    line(&mut out, "");
    line(&mut out, "| | |");
    line(&mut out, "|---|---|");
    let mut cell = |label: &str, value: &str| {
        line(&mut out, &format!("| {label} | {value} |"));
    };
    cell("furthest stage", &furthest.to_string());
    cell("attempts", &format!("{made} of {budget} (working)"));
    cell(
        "spend",
        &format!("${:.2} (API pricing)", found.total_spend()),
    );
    cell("tool denials", &found.denial_count().to_string());
    // The four completion observations, each with its mark and nothing else.
    cell("PR", &marked(&observation.pr));
    cell("tree clean", &marked(&observation.tree_clean));
    cell("commits ahead", &marked(&observation.commits_ahead));
    cell("checks pending", &marked(&observation.checks_pending));
    cell("base drift", &marked(&observation.base_drift));
    // The same two facts, with the mark and **never the `Reason`** — the rule the verdict line
    // above learned the hard way.
    cell(
        "fan-out",
        &match fanout_totals(found) {
            FanoutTotals {
                counted: Some(pair),
                unread: None,
            } => fanout_counted(pair),
            FanoutTotals {
                counted: Some(pair),
                unread: Some(_),
            } => format!(
                "{}  {UNOBSERVABLE_MARK} at least one attempt unread",
                fanout_counted(pair)
            ),
            FanoutTotals {
                counted: None,
                unread: Some(_),
            } => UNOBSERVABLE_MARK.to_string(),
            FanoutTotals {
                counted: None,
                unread: None,
            } => crate::observe::ABSENT_MARK.to_string(),
        },
    );
    // Presence **and** absence: naming only one of them is how a partial contract reads whole.
    cell(
        "verify contract present",
        &or_none(&contract.present.join(", ")),
    );
    cell(
        "verify contract missing",
        &or_none(&contract.missing.join(", ")),
    );
    cell("verify coverage", &marked(coverage));
    cell("run state", &format!("`{}`", run_state.display()));
    out
}

/// The verdict, reduced to what the surface that leaves the host is allowed to carry.
///
/// The table cells honour *the mark and never the `Reason`* through [`marked`]; the verdict line
/// above them is composed by `decide::verdict`, which spells an `Unobserved` entry
/// `format!("{name}: {reason}")` — and a `Reason::of` is `<call site>: exit N: <first stderr
/// line>`, so its tail is raw child stderr. Taking the text before the first `:` leaves the
/// signal name, which is the whole of what a reader off-host can act on. `Completed`,
/// `Uncorroborated` and `Incomplete` carry signal names only, and pass through untouched.
///
/// A transform on the verdict rather than a second renderer, so the comment and the Handback
/// cannot drift into two shapes for one Run — which is the same reason `job_comment` takes
/// [`Facts`] rather than composing its own.
fn off_host(verdict: &Verdict) -> Verdict {
    match verdict {
        Verdict::Unobserved(blind) => Verdict::Unobserved(
            blind
                .iter()
                .map(|said| said.split(':').next().unwrap_or(said).trim().to_string())
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The mark, and never the reason behind it.
fn marked<T: std::fmt::Display>(found: &Observed<T>) -> String {
    match found {
        Observed::Present(value) => value.to_string(),
        other => negative_mark(other).to_string(),
    }
}

fn or_none(said: &str) -> String {
    if said.is_empty() {
        crate::observe::ABSENT_MARK.to_string()
    } else {
        said.to_string()
    }
}

/// The fresh verdict, plus the parentheticals the line is allowed to carry: red CI names the
/// repair budget it spent, a Blocker names what must be cleared. Same line, same renderer, no
/// second surface.
fn handback_verdict(
    verdict: &Verdict,
    observation: &Observation,
    found: &RunView,
    blocker: Option<&str>,
) -> String {
    let mut said = verdict_line(verdict, observation);
    if matches!(observation.checks_red, Observed::Present(true)) {
        said = format!("{said} — ${:.2} of repair spent", repair_spend(found));
    }
    if let Some(what) = blocker {
        said = format!("{said}  (a Blocker: {what} must be cleared)");
    }
    said
}

/// What the one bounded CI-babysit invocation cost, which is the whole of the repair budget.
fn repair_spend(found: &RunView) -> f64 {
    found
        .attempts
        .iter()
        .filter(|a| a.mode == crate::attempt::Mode::CiBabysit)
        .filter_map(|a| a.total_cost_usd)
        .sum()
}

/// The Run's fan-out arithmetic across its Attempts: **two integers and no summary**, and —
/// separately — whether some Attempt's pair could not be read at all.
///
/// Two fields rather than an `Observed<(u64, u64)>`, because the two facts are independent and
/// the type has no variant for *counted, and incomplete*. Folding them cost the second one:
/// `(Some(counted), _) => Present(counted)` discarded a recorded blind reason whenever at least
/// one attempt was readable, and both render sites print `Present` as a bare "N spawned,
/// M returned". A Run whose attempt 3 transcript could not be read therefore reported an
/// understated total **as a definite fact** — the exact ambiguity between *observed a low
/// number* and *could not observe some of it* that `Observed` exists to prevent, on the surface
/// that leaves the host (R94).
struct FanoutTotals {
    counted: Option<(u64, u64)>,
    unread: Option<crate::observe::Reason>,
}

fn fanout_totals(found: &RunView) -> FanoutTotals {
    let mut counted: Option<(u64, u64)> = None;
    let mut unread: Option<crate::observe::Reason> = None;
    for attempt in &found.attempts {
        match &attempt.fanout {
            Observed::Present((spawned, returned)) => {
                let (s, r) = counted.unwrap_or((0, 0));
                counted = Some((s + spawned, r + returned));
            }
            Observed::Unobservable(reason) => unread = Some(reason.clone()),
            Observed::Absent => {}
        }
    }
    FanoutTotals { counted, unread }
}

/// The two integers, written once so the Handback and the comment cannot spell them differently.
fn fanout_counted((spawned, returned): (u64, u64)) -> String {
    format!("{spawned} spawned, {returned} returned")
}

/// A value worth printing, and whether it is worth printing because nobody could look.
///
/// `Absent`, and `Present` that the predicate calls unsurprising, print nothing at all.
fn surprising<T: std::fmt::Display>(
    found: &Observed<T>,
    worth_saying: impl Fn(&T) -> bool,
) -> Option<(String, bool)> {
    match found {
        Observed::Present(value) if worth_saying(value) => Some((value.to_string(), false)),
        Observed::Present(_) | Observed::Absent => None,
        Observed::Unobservable(reason) => Some((format!("{UNOBSERVABLE_MARK}  {reason}"), true)),
    }
}

fn row<T: std::fmt::Display>(label: &'static str, found: &Observed<T>) -> Row {
    Row {
        label,
        value: match found {
            Observed::Unobservable(reason) => format!("{UNOBSERVABLE_MARK}  {reason}"),
            other => other.to_string(),
        },
        unobserved: matches!(found, Observed::Unobservable(_)),
    }
}

/// The denied invocations, as the record holds them. Listed only when the count is non-zero.
fn denied_invocations(found: &RunView) -> Vec<String> {
    found
        .attempts
        .iter()
        .flat_map(|a| a.permission_denials.iter())
        .map(|denial| {
            denial
                .get("tool_input")
                .and_then(|i| i.get("command"))
                .and_then(|c| c.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    denial
                        .get("tool_name")
                        .and_then(|t| t.as_str())
                        .unwrap_or("an unnamed tool")
                        .to_string()
                })
        })
        .collect()
}

/// Doctor's report. Items marked *step* appear as unchecked, with **no boolean beside them** —
/// every available check for them is a guess.
pub fn doctor(hostname: &str, lines: &[DoctorLine]) -> String {
    let mut out = String::new();
    line(&mut out, &format!("Host {hostname}"));
    line(&mut out, "");
    for item in lines {
        line(
            &mut out,
            &format!(
                "  {:<9} {:<40} {}",
                item.mark,
                item.name,
                item_outcome(&item.outcome)
            ),
        );
    }
    line(&mut out, "");
    line(
        &mut out,
        "  A failed item is incoherent input, not a judgement. Checking is not gating.",
    );
    out
}

/// A refusal, in the register a refused Dispatch and a failed host check share.
pub fn refusal(said: &str) -> String {
    format!("grind: {said}\n")
}

// --- the pieces ------------------------------------------------------------------------------

/// The two negatives, for the values whose `T` is a collection and so has no `Display`. Same
/// marks as the type's own, because a reader must never have to learn two vocabularies.
fn negative_mark<T>(found: &Observed<T>) -> &'static str {
    match found {
        Observed::Present(_) => "",
        Observed::Absent => crate::observe::ABSENT_MARK,
        Observed::Unobservable(_) => UNOBSERVABLE_MARK,
    }
}

fn line(out: &mut String, text: &str) {
    out.push_str(text);
    out.push('\n');
}

fn model_of(found: &RunView) -> String {
    found
        .model
        .clone()
        .unwrap_or_else(|| "(session default — unpinned)".to_string())
}

fn presence_word(here: &Observed<bool>) -> &'static str {
    match here {
        Observed::Present(true) => "present",
        Observed::Present(false) => "gone",
        Observed::Absent => "gone",
        Observed::Unobservable(_) => UNOBSERVABLE_MARK,
    }
}

/// Red CI lands **on the verdict line** rather than holding the verdict open.
fn verdict_line(verdict: &Verdict, observation: &Observation) -> String {
    let said = match verdict {
        Verdict::Completed => "completed".to_string(),
        Verdict::Uncorroborated(unmet) => {
            format!(
                "uncorroborated — DONE promised, {} disagrees",
                unmet.join(", ")
            )
        }
        Verdict::Unobserved(blind) => format!("unobserved — {}", blind.join("; ")),
        Verdict::Incomplete(unmet) => format!("incomplete — {}", unmet.join(", ")),
    };
    match observation.checks_red {
        Observed::Present(true) => format!("{said}  (a check came back red)"),
        _ => said,
    }
}

fn freshness_line(freshness: &Observed<u64>) -> String {
    match freshness {
        Observed::Present(seconds) => format!("newest write {seconds}s ago"),
        other => other.to_string(),
    }
}

fn fanout_line(live: &Live) -> String {
    match &live.fanout {
        Observed::Present(agents) if agents.is_empty() => "none".to_string(),
        Observed::Present(agents) => {
            let described: Vec<&str> = agents.iter().map(|a| a.description.as_str()).collect();
            format!(
                "{} agent{}: {}  ({})",
                agents.len(),
                if agents.len() == 1 { "" } else { "s" },
                described.join("; "),
                freshness_line(&live.freshness)
            )
        }
        other => negative_mark(other).to_string(),
    }
}

/// Presence and absence, and **never a verdict on quality**. This is the one place a gate would
/// be one line away, which is why the contract carries no summary boolean to test.
fn contract_line(contract: &VerifyContract) -> String {
    match (contract.present.is_empty(), contract.missing.is_empty()) {
        (_, true) => format!("all {} contracted steps present", contract.present.len()),
        (true, false) => format!("none present; missing: {}", contract.missing.join(", ")),
        (false, false) => format!(
            "present: {}; missing: {}",
            contract.present.join(", "),
            contract.missing.join(", ")
        ),
    }
}

fn item_outcome(outcome: &Observed<Outcome>) -> String {
    match outcome {
        Observed::Present(Outcome::Satisfied(said)) => format!("ok        {said}"),
        Observed::Present(Outcome::Unsatisfied(said)) => format!("not met   {said}"),
        Observed::Present(Outcome::Unchecked(said)) => format!("unchecked {said}"),
        Observed::Absent => format!("{}         absent", crate::observe::ABSENT_MARK),
        Observed::Unobservable(reason) => format!("{UNOBSERVABLE_MARK}         {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decide::VerifyCoverage;
    use crate::observe::{Pr, Reason};
    use crate::view::Fanout;
    use std::path::PathBuf;

    const DAY_ONE: &str = include_str!("../tests/fixtures/record/day-one.json");

    fn found() -> RunView {
        serde_json::from_str(DAY_ONE).expect("the day-one record")
    }

    fn observation() -> Observation {
        Observation {
            observed_at: "2026-08-06T17:41:00+00:00".to_string(),
            commits_ahead: Observed::Present(12),
            tree_clean: Observed::Present(true),
            pr: Observed::Present(Pr {
                number: 30,
                url: "https://github.com/FlorianRiquelme/snapper/pull/30".to_string(),
                state: "OPEN".to_string(),
                is_draft: false,
            }),
            checks_pending: Observed::Present(false),
            checks_red: Observed::Present(false),
            plan_files: Observed::Present(vec!["docs/plans/a.md".to_string()]),
            residual_findings: Observed::Present(vec![
                "docs/residual-review-findings/a.md".to_string(),
            ]),
            ledger_entries: Observed::Absent,
            changed_files: Observed::Present(vec!["docs/plans/a.md".to_string()]),
            base_drift: Observed::Present(crate::observe::BaseDrift {
                default_branch: "origin/main".to_string(),
                commits: 0,
                overlapping: vec![],
            }),
        }
    }

    fn live(words: usize) -> Live {
        Live {
            transcript: PathBuf::from("/home/op/.claude/projects/x/session.jsonl"),
            now_skill: Observed::Present("compound-engineering:ce-work".to_string()),
            last_words: (0..3)
                .map(|n| {
                    if n < words {
                        format!("line {n}")
                    } else {
                        String::new()
                    }
                })
                .collect(),
            fanout: Observed::Present(vec![Fanout {
                description: "review the diff for regressions".to_string(),
            }]),
            freshness: Observed::Present(40),
        }
    }

    fn contract() -> VerifyContract {
        VerifyContract {
            present: vec!["rust-fmt".into(), "rust-clippy".into(), "rust-test".into()],
            missing: vec!["ts-lint".into(), "ts-test".into()],
        }
    }

    fn rendered(observation: &Observation, live: &Live, verdict: &Verdict) -> String {
        run_view(&SingleRun {
            found: &found(),
            observation,
            live,
            verdict,
            contract: &contract(),
            furthest: Stage::PrOpen,
            supervisor_here: &Observed::Present(true),
            run_state: Path::new("/home/op/.grind/runs/20260806-122620-snapper-28/run.json"),
        })
    }

    fn label_order(text: &str) -> Vec<String> {
        text.lines()
            .filter(|l| l.starts_with("  ") && !l.starts_with("    "))
            .map(|l| l.trim().split("  ").next().unwrap_or_default().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    }

    #[test]
    fn the_single_run_view_prints_its_lines_in_a_fixed_order_across_two_records() {
        let first = rendered(&observation(), &live(3), &Verdict::Completed);
        let mut other = observation();
        other.pr = Observed::Absent;
        other.commits_ahead = Observed::Present(0);
        let second = rendered(
            &other,
            &live(1),
            &Verdict::Incomplete(vec!["PR open".into()]),
        );
        assert_eq!(
            label_order(&first),
            label_order(&second),
            "`watch -n 30` must never jitter"
        );
    }

    #[test]
    fn the_last_words_block_is_exactly_three_lines_whatever_the_transcript_said() {
        for words in [0, 1, 3] {
            let text = rendered(&observation(), &live(words), &Verdict::Completed);
            let at = text
                .lines()
                .position(|l| l.trim() == "last words")
                .expect("the block");
            let block: Vec<&str> = text.lines().skip(at + 1).take(3).collect();
            assert_eq!(block.len(), 3, "{words} words");
            assert!(
                text.lines()
                    .nth(at + 4)
                    .is_some_and(|l| l.trim().is_empty()),
                "the block ends after exactly three lines"
            );
        }
    }

    #[test]
    fn observed_absent_renders_differently_from_could_not_observe_in_the_same_column() {
        let mut absent = observation();
        absent.pr = Observed::Absent;
        let mut blind = observation();
        blind.pr = Observed::Unobservable(Reason::saying("gh pr view: connection reset"));

        let absent_line = pr_line(&rendered(&absent, &live(3), &Verdict::Completed));
        let blind_line = pr_line(&rendered(&blind, &live(3), &Verdict::Completed));
        assert_ne!(absent_line, blind_line);
        assert!(
            absent_line.contains(crate::observe::ABSENT_MARK),
            "{absent_line}"
        );
        assert!(blind_line.contains(UNOBSERVABLE_MARK), "{blind_line}");
    }

    fn pr_line(text: &str) -> String {
        text.lines()
            .find(|l| l.trim_start().starts_with("PR "))
            .expect("a PR line")
            .to_string()
    }

    #[test]
    fn a_pr_that_could_not_be_observed_does_not_render_as_no_pr() {
        let mut blind = observation();
        blind.pr = Observed::Unobservable(Reason::saying("gh pr view: connection reset"));
        let line = pr_line(&rendered(&blind, &live(3), &Verdict::Completed));
        assert!(
            !line.contains(crate::observe::ABSENT_MARK),
            "a blind supervisor's silence must not read as a fact: {line}"
        );
    }

    #[test]
    fn the_view_prints_no_remaining_quota_figure() {
        let text = rendered(&observation(), &live(3), &Verdict::Completed);
        for banned in ["remaining", "quota", "left in", "budget left"] {
            assert!(
                !text.to_lowercase().contains(banned),
                "the number nothing can compute is not estimated: {banned}"
            );
        }
        assert!(text.contains("(API pricing)"));
    }

    #[test]
    fn the_two_stage_lines_are_separate_and_never_conflated() {
        let text = rendered(&observation(), &live(3), &Verdict::Completed);
        assert!(text.contains("furthest stage    pr-open"));
        assert!(text.contains("now               compound-engineering:ce-work"));
    }

    #[test]
    fn the_verify_contract_line_names_both_missing_steps_and_carries_no_verdict_word() {
        let text = rendered(&observation(), &live(3), &Verdict::Completed);
        let line = text
            .lines()
            .find(|l| l.contains("verify contract"))
            .expect("a contract line");
        assert!(line.contains("ts-lint"), "{line}");
        assert!(line.contains("ts-test"), "{line}");
        for banned in ["incomplete", "gutted", "bad", "fail", "should", "must"] {
            assert!(!line.to_lowercase().contains(banned), "{line}");
        }
    }

    #[test]
    fn a_full_contract_and_an_empty_one_both_read_as_presence_and_absence() {
        let all = VerifyContract {
            present: (0..7).map(|n| format!("step-{n}")).collect(),
            missing: vec![],
        };
        assert_eq!(contract_line(&all), "all 7 contracted steps present");
        let none = VerifyContract {
            present: vec![],
            missing: vec!["rust-fmt".into()],
        };
        assert!(contract_line(&none).starts_with("none present; missing:"));
    }

    fn coverage() -> Observed<VerifyCoverage> {
        Observed::Present(VerifyCoverage {
            uncovered: vec![],
            changed: 4,
        })
    }

    /// One fact set, built the way `view::gather` builds it.
    fn facts_of(
        found: RunView,
        observation: Observation,
        verdict: Verdict,
        coverage: Observed<VerifyCoverage>,
        blocker: Option<&str>,
    ) -> Facts {
        Facts {
            found,
            observation,
            verdict,
            contract: contract(),
            coverage,
            furthest: Stage::PrOpen,
            blocker: blocker.map(str::to_string),
            run_state: PathBuf::from("/home/op/.grind/runs/20260806-122620-snapper-28/run.json"),
        }
    }

    /// The Handback over one fact set, with the two things a caller varies most.
    fn handed_back(observation: &Observation, verdict: &Verdict) -> String {
        handed_back_with(observation, verdict, &coverage(), None)
    }

    fn handed_back_with(
        observation: &Observation,
        verdict: &Verdict,
        coverage: &Observed<VerifyCoverage>,
        blocker: Option<&str>,
    ) -> String {
        handback(&facts_of(
            found(),
            observation.clone(),
            verdict.clone(),
            coverage.clone(),
            blocker,
        ))
    }

    #[test]
    fn the_handback_renders_the_fresh_verdict_in_the_top_position_and_never_the_recorded_state() {
        // Run 2's Handback said `[exhausted]` with `PR —` over an open, green, twelve-commit PR.
        let mut record = found();
        record.state = "exhausted".to_string();
        let text = handback(&facts_of(
            record,
            observation(),
            Verdict::Completed,
            coverage(),
            None,
        ));
        assert!(text.starts_with("Verdict  completed"), "{text}");
        assert!(
            !text.contains("exhausted"),
            "printing both asks the human to adjudicate between two things Grind produced:\n{text}"
        );
    }

    #[test]
    fn the_handback_names_where_run_state_lives_and_keeps_the_model_a_fact() {
        let text = handed_back(&observation(), &Verdict::Completed);
        assert!(
            text.contains(
                "run state        /home/op/.grind/runs/20260806-122620-snapper-28/run.json"
            )
        );
        for named in ["Job", "Branch", "Model", "Attempts", "tool denials"] {
            assert!(text.contains(named), "the Handback must name {named}");
        }
        assert!(text.contains("furthest stage"));
        assert!(text.contains("commits ahead"));
    }

    #[test]
    fn the_handback_carries_no_plan_residual_or_ledger_count() {
        // Three whole-directory listings that counted other people's files, every one of which
        // is already in the PR's own diff.
        let text = handed_back(&observation(), &Verdict::Completed);
        for dropped in ["plan  ", "review residuals", "ledger entries"] {
            assert!(!text.contains(dropped), "`{dropped}` still prints:\n{text}");
        }
        // And the observations behind them still feed the stage ladder.
        assert!(matches!(
            observation().plan_files,
            Observed::Present(_) | Observed::Absent
        ));
    }

    #[test]
    fn the_session_handle_and_the_worktree_move_into_the_trailing_pointer_block() {
        // Things you type at something, not claims about the world — and a session handle is
        // worthless off its host.
        let text = handed_back(&observation(), &Verdict::Completed);
        let at = |needle: &str| {
            text.find(needle)
                .unwrap_or_else(|| panic!("{needle}\n{text}"))
        };
        assert!(at("\n  session ") > at("furthest stage"), "{text}");
        assert!(at("\n  worktree ") > at("furthest stage"), "{text}");
        assert!(at("\n  session ") < at("\n  run state"), "{text}");
        assert!(
            !text.contains("\nSession "),
            "not a top-level fact:\n{text}"
        );
    }

    #[test]
    fn attempt_n_of_m_counts_working_attempts_only_on_every_surface_that_prints_it() {
        // The day-one record holds four Attempts, of which attempt 3 cost $0 and ran one turn.
        let text = handed_back(&observation(), &Verdict::Completed);
        assert!(text.contains("Attempts 3 of 8"), "{text}");
        let single = rendered(&observation(), &live(3), &Verdict::Completed);
        assert!(single.contains("attempt 3 of 8"), "{single}");
    }

    #[test]
    fn a_denial_count_prints_unconditionally_and_the_invocations_only_when_non_zero() {
        let text = handed_back(&observation(), &Verdict::Completed);
        assert!(text.contains("tool denials 1"), "{text}");
        assert!(
            text.contains("denied "),
            "the one denial is listed:\n{text}"
        );
        assert!(text.contains("git push --force-with-lease"), "{text}");

        let mut clean = found();
        clean.attempts.iter_mut().for_each(|a| {
            a.permission_denials.clear();
        });
        let text = handback(&facts_of(
            clean,
            observation(),
            Verdict::Completed,
            coverage(),
            None,
        ));
        assert!(text.contains("tool denials 0"), "{text}");
        assert!(
            !text.contains("denied "),
            "a zero-length list is a permanent negative:\n{text}"
        );
    }

    #[test]
    fn a_draft_pr_surfaces_the_flag_and_a_non_draft_one_prints_no_row() {
        assert!(!handed_back(&observation(), &Verdict::Completed).contains("draft"));
        let mut draft = observation();
        draft.pr = Observed::Present(Pr {
            number: 30,
            url: "https://github.com/o/n/pull/30".to_string(),
            state: "OPEN".to_string(),
            is_draft: true,
        });
        assert!(handed_back(&draft, &Verdict::Completed).contains("draft"));
    }

    #[test]
    fn base_drift_surfaces_only_when_non_zero() {
        assert!(!handed_back(&observation(), &Verdict::Completed).contains("base drift"));
        let mut drifted = observation();
        drifted.base_drift = Observed::Present(crate::observe::BaseDrift {
            default_branch: "origin/main".to_string(),
            commits: 4,
            overlapping: vec!["docs/adr/0013.md".to_string()],
        });
        let text = handed_back(&drifted, &Verdict::Completed);
        assert!(text.contains("base drift"), "{text}");
        assert!(text.contains("docs/adr/0013.md"), "{text}");
    }

    #[test]
    fn a_total_over_an_attempt_that_could_not_be_read_carries_the_mark_beside_the_number() {
        // The day-one record's attempt 2 could not be read and its attempts 1 and 4 could, so
        // 4 is a **floor**, not the total. `(Some(counted), _) => Present(counted)` dropped the
        // blind reason whenever anything else was readable, and both surfaces print a bare
        // "N spawned, M returned" — so an understated number left the host as a definite fact.
        let on_host = handed_back(&observation(), &Verdict::Completed);
        assert!(on_host.contains("4 spawned, 4 returned"), "{on_host}");
        assert!(on_host.contains("at least one attempt unread"), "{on_host}");
        assert!(
            on_host.contains("the transcript could not be read"),
            "on the host the reason still shows:\n{on_host}"
        );
        assert!(
            on_host.contains("could not observe"),
            "and the row joins the blind block:\n{on_host}"
        );

        // The comment carries the same two facts with the mark and **never the `Reason`**.
        let markdown = commented(&observation(), &Verdict::Completed);
        assert!(markdown.contains("4 spawned, 4 returned"), "{markdown}");
        assert!(
            markdown.contains("at least one attempt unread"),
            "{markdown}"
        );
        assert!(
            !markdown.contains("the transcript could not be read"),
            "{markdown}"
        );

        // A Run whose every attempt was read says the number and nothing else.
        let clean = handback(&facts_of(
            every_attempt_read(),
            observation(),
            Verdict::Completed,
            coverage(),
            None,
        ));
        assert!(clean.contains("4 spawned, 4 returned"), "{clean}");
        assert!(!clean.contains("at least one attempt unread"), "{clean}");
    }

    #[test]
    fn the_fan_out_arithmetic_surfaces_as_two_integers_with_no_summary_word() {
        // The day-one record holds (3, 3) and (1, 1) across its Attempts.
        let text = handed_back(&observation(), &Verdict::Completed);
        assert!(text.contains("fan-out"), "{text}");
        assert!(text.contains("4 spawned, 4 returned"), "{text}");
        for banned in ["healthy", "degraded", "complete fan-out", "all returned"] {
            assert!(!text.contains(banned), "{text}");
        }

        let mut quiet = found();
        quiet
            .attempts
            .iter_mut()
            .for_each(|a| a.fanout = Observed::Absent);
        let text = handback(&facts_of(
            quiet,
            observation(),
            Verdict::Completed,
            coverage(),
            None,
        ));
        assert!(
            !text.contains("fan-out"),
            "a Run that spawned nothing:\n{text}"
        );
    }

    #[test]
    fn the_verify_coverage_estimate_surfaces_the_paths_and_is_labelled_an_estimate() {
        assert!(!handed_back(&observation(), &Verdict::Completed).contains("verify coverage"));
        let uncovered = Observed::Present(VerifyCoverage {
            uncovered: vec!["docs/adr/0013.md".to_string()],
            changed: 4,
        });
        let text = handed_back_with(&observation(), &Verdict::Completed, &uncovered, None);
        assert!(text.contains("verify coverage"), "{text}");
        assert!(text.contains("estimate"), "{text}");
        assert!(text.contains("docs/adr/0013.md"), "{text}");
    }

    /// The day-one record with **every** Attempt's fan-out readable. The fixture's attempt 2 is
    /// genuinely `Unobservable`, so the record as shipped is not a fully-observed Run — which
    /// only stopped being visible because `fanout_totals` used to swallow it.
    fn every_attempt_read() -> RunView {
        let mut record = found();
        for attempt in &mut record.attempts {
            if matches!(attempt.fanout, Observed::Unobservable(_)) {
                attempt.fanout = Observed::Absent;
            }
        }
        record
    }

    #[test]
    fn the_could_not_observe_block_is_empty_on_a_fully_observed_run() {
        let flat = handback(&facts_of(
            every_attempt_read(),
            observation(),
            Verdict::Completed,
            coverage(),
            None,
        ));
        assert!(!flat.contains("could not observe"), "{flat}");

        let mut blind = observation();
        blind.tree_clean = Observed::Unobservable(Reason::saying("git status: exit 128"));
        let text = handed_back(&blind, &Verdict::Unobserved(vec!["tree clean: x".into()]));
        assert!(text.contains("could not observe"), "{text}");
        let block = text.split("could not observe").nth(1).expect("the block");
        assert!(block.contains("tree clean"), "{block}");
        // And as a row it appears there and nowhere else. The verdict line names the blind
        // signal too, which is the verdict speaking rather than a row.
        let before = text.split("could not observe").next().expect("the rows");
        assert!(!before.contains("\n  tree clean"), "{before}");
    }

    #[test]
    fn red_ci_puts_the_spent_repair_budget_on_the_verdict_line() {
        let mut red = observation();
        red.checks_red = Observed::Present(true);
        let text = handed_back(&red, &Verdict::Completed);
        let said = text.lines().next().expect("the verdict line");
        assert!(said.contains("completed"), "{said}");
        assert!(said.contains("a check came back red"), "{said}");
        // The day-one record's CI-babysit attempt cost $3.18.
        assert!(said.contains("$3.18 of repair spent"), "{said}");
    }

    #[test]
    fn a_blocker_puts_what_must_be_cleared_on_the_verdict_line() {
        let text = handed_back_with(
            &observation(),
            &Verdict::Incomplete(vec!["PR open".into()]),
            &coverage(),
            Some("git push --force-with-lease"),
        );
        let said = text.lines().next().expect("the verdict line");
        assert!(said.contains("a Blocker"), "{said}");
        assert!(said.contains("git push --force-with-lease"), "{said}");
        assert!(said.contains("must be cleared"), "{said}");
    }

    #[test]
    fn no_did_not_declare_done_line_prints_on_the_completed_path() {
        let text = handed_back(&observation(), &Verdict::Completed);
        assert!(!text.to_lowercase().contains("did not declare"), "{text}");
        assert!(!text.contains("DONE"), "{text}");
        // Where the promise was made and the artifacts disagree, it still says so.
        let mut absent = observation();
        absent.pr = Observed::Absent;
        let uncorroborated = handed_back(&absent, &Verdict::Uncorroborated(vec!["PR open".into()]));
        assert!(uncorroborated.contains("DONE promised"), "{uncorroborated}");
    }

    // --- the account that leaves the host ------------------------------------------------------

    fn commented(observation: &Observation, verdict: &Verdict) -> String {
        job_comment(&facts_of(
            found(),
            observation.clone(),
            verdict.clone(),
            coverage(),
            None,
        ))
    }

    #[test]
    fn both_renderers_given_one_fact_set_make_the_same_five_claims() {
        let facts = facts_of(found(), observation(), Verdict::Completed, coverage(), None);
        let terminal = handback(&facts);
        let markdown = job_comment(&facts);
        for claim in [
            "completed",
            "3 of 8",
            "26.69",
            "https://github.com/FlorianRiquelme/snapper/pull/30",
            "/home/op/.grind/runs/20260806-122620-snapper-28/run.json",
        ] {
            assert!(terminal.contains(claim), "the Handback drops `{claim}`");
            assert!(markdown.contains(claim), "the comment drops `{claim}`");
        }
        // And the comment says which host is holding the Run state it points at.
        assert!(markdown.contains("snapper.local"), "{markdown}");
    }

    /// An observation that could not be made, carrying a `Reason` built the way the real one is:
    /// `Reason::of` over a failed child, whose stderr is an HTTPS `origin` with a token in it.
    fn could_not_look() -> Observation {
        let mut blind = observation();
        blind.pr = Observed::Unobservable(Reason::of(
            "gh pr view",
            &crate::world::Completed {
                stdout: String::new(),
                stderr: "fatal: Authentication failed for 'https://ghp_secret@github.com/o/n'\n"
                    .to_string(),
                code: Some(128),
            },
        ));
        blind
    }

    /// The comment with its verdict **composed the way `view::gather` composes it** — through
    /// `decide`, from the same observation. An authored verdict literal is a sanitised input,
    /// and a guard that authors its own input cannot see what the composition path puts on the
    /// line.
    fn commented_from(observation: &Observation) -> String {
        commented(
            observation,
            &crate::decide::verdict(&crate::decide::signals_of(observation), false),
        )
    }

    #[test]
    fn the_comment_renders_at_every_one_of_the_five_terminal_states() {
        // completed, uncorroborated, unobserved, exhausted and blocked. Exhaustion reads as an
        // incomplete verdict over a Run whose budget ran out, and a Blocker rides the same line.
        //
        // **Each case asserts what only that state says.** The Run name and the table header are
        // state-invariant, so a guard built from those two passes on all five even when the one
        // line that tells them apart has gone — and `exhausted` and `blocked` are the same
        // verdict, differing only by the parenthetical.
        let mut absent = observation();
        absent.pr = Observed::Absent;
        let each: [(&str, String, &[&str], &[&str]); 5] = [
            (
                "completed",
                commented(&observation(), &Verdict::Completed),
                &["completed"],
                &["uncorroborated", "unobserved", "incomplete", "a Blocker"],
            ),
            (
                "uncorroborated",
                commented(&absent, &Verdict::Uncorroborated(vec!["PR open".into()])),
                &["uncorroborated — DONE promised, PR open disagrees"],
                &["a Blocker"],
            ),
            (
                "unobserved",
                commented_from(&could_not_look()),
                &["unobserved — PR open"],
                &["ghp_secret", "exit 128", "a Blocker"],
            ),
            (
                "exhausted",
                commented(&absent, &Verdict::Incomplete(vec!["PR open".into()])),
                &["incomplete — PR open"],
                &["a Blocker"],
            ),
            (
                "blocked",
                job_comment(&facts_of(
                    found(),
                    absent.clone(),
                    Verdict::Incomplete(vec!["PR open".into()]),
                    coverage(),
                    Some("git push --force-with-lease"),
                )),
                &[
                    "incomplete — PR open",
                    "a Blocker: git push --force-with-lease must be cleared",
                ],
                &[],
            ),
        ];
        for (state, markdown, said, unsaid) in each {
            assert!(
                markdown.contains("**Run `20260806-122620-snapper-28`"),
                "the {state} comment names the Run:\n{markdown}"
            );
            assert!(
                markdown.contains("| run state |"),
                "the {state} comment carries the table:\n{markdown}"
            );
            for claim in said {
                assert!(
                    markdown.contains(claim),
                    "the {state} comment drops `{claim}`:\n{markdown}"
                );
            }
            for banned in unsaid {
                assert!(
                    !markdown.contains(banned),
                    "the {state} comment carries `{banned}`:\n{markdown}"
                );
            }
        }
    }

    #[test]
    fn no_rendered_comment_contains_a_reason_built_by_reason_of() {
        // `Reason::of` composes `<call site>: exit N: <first stderr line>`, so a reason is raw
        // child stderr — and a misprovisioned host is exactly where an HTTPS `origin` embeds a
        // token. An observation that could not be made shows its mark and nothing else.
        //
        // **The verdict is composed, never authored.** The earlier shape of this test hand-wrote
        // its blind vector as `vec!["PR open: x"]`, which is a sanitised input: the leak was on
        // the verdict line, built by `decide::verdict` out of the very `Reason` this test
        // constructs, and a guard holding a literal cannot reach it.
        let mut blind = could_not_look();
        blind.base_drift = Observed::Unobservable(Reason::saying("git symbolic-ref: exit 1"));
        let markdown = commented_from(&blind);
        assert!(!markdown.contains("ghp_secret"), "{markdown}");
        assert!(!markdown.contains("exit 128"), "{markdown}");
        assert!(!markdown.contains("symbolic-ref"), "{markdown}");
        assert!(
            markdown.contains(UNOBSERVABLE_MARK),
            "the mark still shows:\n{markdown}"
        );
        // The signal name survives — the reader off-host is told *which* observation is missing.
        assert!(markdown.contains("unobserved — PR open"), "{markdown}");
        // And the Handback, on the host where the human already has them, still prints reasons.
        let on_host = handed_back(
            &blind,
            &crate::decide::verdict(&crate::decide::signals_of(&blind), false),
        );
        assert!(on_host.contains("ghp_secret"), "{on_host}");
    }

    #[test]
    fn the_comment_names_verify_contract_presence_and_absence_and_not_one_of_them() {
        let markdown = commented(&observation(), &Verdict::Completed);
        assert!(markdown.contains("verify contract present"), "{markdown}");
        assert!(markdown.contains("verify contract missing"), "{markdown}");
        assert!(markdown.contains("ts-lint"), "{markdown}");
        assert!(markdown.contains("rust-fmt"), "{markdown}");
    }

    #[test]
    fn the_comment_carries_the_four_completion_observations_and_the_fan_out_arithmetic() {
        let markdown = commented(&observation(), &Verdict::Completed);
        for named in [
            "PR",
            "tree clean",
            "commits ahead",
            "checks pending",
            "fan-out",
            "tool denials",
            "run state",
        ] {
            assert!(markdown.contains(named), "the comment drops `{named}`");
        }
        assert!(markdown.contains("4 spawned, 4 returned"), "{markdown}");
    }

    #[test]
    fn nothing_in_the_comment_is_a_summary_boolean_or_a_quality_word() {
        // A public surface is bound at least as hard as a private one.
        let markdown = commented(&observation(), &Verdict::Completed).to_lowercase();
        for banned in [
            "rejected",
            "approved",
            "healthy",
            "passing",
            "all good",
            "looks good",
            "everything",
        ] {
            assert!(!markdown.contains(banned), "`{banned}`:\n{markdown}");
        }
    }

    #[test]
    fn nothing_the_handback_renders_is_a_summary_boolean() {
        // `tree clean  true` is a three-valued observation of one named fact, which is the
        // opposite of a fold. What must not exist is a word standing over the rest of them.
        let text = handed_back(&observation(), &Verdict::Completed).to_lowercase();
        for banned in [" ok ", "healthy", "passing", "all good", "everything"] {
            assert!(!text.contains(banned), "`{banned}`:\n{text}");
        }
        let verdict = text.lines().next().expect("the verdict line");
        for banned in ["true", "false", "ok"] {
            assert!(!verdict.contains(banned), "`{banned}` in `{verdict}`");
        }
    }

    #[test]
    fn red_ci_lands_on_the_verdict_line_without_holding_it_open() {
        let mut red = observation();
        red.checks_red = Observed::Present(true);
        let text = rendered(&red, &live(3), &Verdict::Completed);
        let line = text
            .lines()
            .find(|l| l.contains("verdict"))
            .expect("a verdict line");
        assert!(line.contains("completed"), "{line}");
        assert!(line.contains("a check came back red"), "{line}");
    }

    #[test]
    fn a_step_item_shows_as_unchecked_with_no_boolean() {
        let report = doctor(
            "snapper.local",
            &[
                DoctorLine {
                    name: "the grind binary on PATH",
                    mark: "step",
                    outcome: crate::observe::unchecked("every available check is a guess"),
                },
                DoctorLine {
                    name: "declared clone",
                    mark: "dispatch",
                    outcome: Observed::Present(Outcome::Unsatisfied(
                        "no declared clone at ~/.grind/repos/<owner>/<name>".into(),
                    )),
                },
            ],
        );
        assert!(report.contains("unchecked"), "{report}");
        assert!(report.contains("not met"), "{report}");
        assert!(report.contains("Checking is not gating"), "{report}");
    }

    #[test]
    fn no_rendered_string_carries_a_quality_word_for_a_verdict() {
        // ADR-0003 is enforceable as a variant set and as the strings that name those variants.
        let surfaces = [
            rendered(&observation(), &live(3), &Verdict::Completed),
            rendered(
                &observation(),
                &live(3),
                &Verdict::Uncorroborated(vec!["PR open".into()]),
            ),
            rendered(
                &observation(),
                &live(3),
                &Verdict::Unobserved(vec!["PR open: connection reset".into()]),
            ),
            handed_back(&observation(), &Verdict::Completed),
            handed_back(&observation(), &Verdict::Incomplete(vec!["PR open".into()])),
            roster("snapper.local", &[]),
            not_here("20260806-122620-snapper-28", "snapper.local"),
        ];
        for text in surfaces {
            let said = text.to_lowercase();
            for banned in [
                "rejected",
                "blocked",
                "failed",
                "approved",
                "good",
                "bad quality",
            ] {
                assert!(!said.contains(banned), "`{banned}` in:\n{text}");
            }
        }
    }

    #[test]
    fn the_roster_says_which_host_it_speaks_for() {
        let text = roster("snapper.local", &[]);
        assert!(text.contains("snapper.local"));
        assert!(text.contains("this host only"));
        assert!(text.contains("no Runs here"));
    }
}
