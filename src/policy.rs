//! When to re-enter, when to sleep, when to stop — **returned as a value**.
//!
//! A `thread::sleep` in here makes *"a rate limit asks for thirty minutes"* an assertion you
//! have to wait thirty minutes for. Returned, it is an equality check on a literal, and the
//! loop is the only thing that blocks.
//!
//! **Never a pre-flight quota check** (ADR-0004). Even a perfectly informed supervisor would be
//! wrong about what a stage costs, so Grind sleeps long and re-enters rather than predicting.

use crate::attempt::{self, Attempt, Mode};
use crate::decide::Verdict;
use crate::observe::{Observed, Reason};
use std::time::Duration;

/// The conditions a Run was dispatched under, **read from the record rather than the
/// environment** — so re-entering under a different environment cannot change a Run's budget
/// mid-pipeline, and *attempt N of M* stays true.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub attempts: usize,
    pub limit_sleep: Duration,
    /// How many times a blind observation may be retried before the Run stops and says so. A
    /// fault in Grind's eyes must never cost an attempt.
    pub reobservations: usize,
    /// How long to wait between re-observations, so a transient window — the one after a
    /// laptop wake is the case this exists for — has a real chance to clear before the next
    /// look. Sourced from a compiled constant beside `REOBSERVATIONS`, never per-Job.
    pub reobserve_pause: Duration,
    /// How many Waits in a row before *nothing is happening forever* becomes terminal. Waits
    /// never spend `attempts`, so this is the only thing bounding a Run against a permanent
    /// wall. Grind's own policy knob, from a compiled constant like `reobservations` — not a
    /// record field and not per-Job.
    pub consecutive_waits: usize,
}

/// What the loop does next.
#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    Reenter,
    SleepThenReenter(Duration),
    /// Look again, after the given pause. **Never a re-entry** — an unobservable signal is a
    /// fault in Grind's eyes, and paying for it with an attempt would let Grind's blindness
    /// mutate a branch. The pause is a value like every other wait in this module: given here,
    /// taken by the loop.
    Reobserve(Duration),
    /// The one bounded PR-babysitting invocation a decided-and-failing CI buys.
    SpendCiBudget,
    Stop(Stop),
}

/// Why the loop stopped. Each of these is a fact about what happened; **exhaustion is a
/// distinct fact, not a failure**.
#[derive(Debug, Clone, PartialEq)]
pub enum Stop {
    Completed,
    Uncorroborated(Vec<String>),
    Unobserved(Vec<String>),
    Exhausted,
    /// An obstacle only a human can clear, carrying **what must be cleared**.
    ///
    /// A stop and a supervisor state, and deliberately **never a `Verdict` variant**: ADR-0006
    /// prohibits `Verdict::{Rejected, Blocked, Failed}` by name because those words are quality
    /// judgements about the *work*. A Blocker is a fact about the *world*, in the same family
    /// as the rate limit the base has carried since day one.
    Blocked(String),
}

/// Did this working Attempt fail to advance the Run?
///
/// **Three-valued, mandatory.** `commits_ahead` read zero for all eight of Run 2's Attempts
/// against twelve real commits, so a terminal state keyed on it gets the same guard every other
/// observation gets: blind must not read as blocked. The first working Attempt of a process has
/// nothing to compare against and reads *could not observe* for that reason.
pub fn stalled(before: Option<&Observed<u64>>, now: &Observed<u64>) -> Observed<bool> {
    match (before, now) {
        (Some(Observed::Present(was)), Observed::Present(is)) => Observed::Present(is <= was),
        (None, _) => Observed::Unobservable(Reason::saying(
            "no earlier commit count to compare this Attempt against",
        )),
        _ => Observed::Unobservable(Reason::saying(
            "the commit count was not observed on both sides of this Attempt",
        )),
    }
}

/// **A Blocker: the same denied invocation on two consecutive working Attempts that both failed
/// to advance.** Supervisor-authoritative, read from the recorded denials.
///
/// Three clauses are load-bearing. **Two working Attempts, not one** — a Run may legitimately
/// probe a denied tool once and route around it, which is exactly what Run 2 did on the Attempt
/// that opened its PR. **An `Unobservable` progress reading never fires it** — see `stalled`.
/// And **the Run's own declaration never fires it alone**: nothing here reads the Run's prose,
/// which is *observed, never declared* holding here too.
///
/// `stalls` carries one reading per working Attempt, newest last.
pub fn blocker(attempts: &[Attempt], stalls: &[Observed<bool>]) -> Option<Stop> {
    let recent: Vec<&Observed<bool>> = stalls.iter().rev().take(2).collect();
    if recent.len() < 2 || !recent.iter().all(|s| s == &&Observed::Present(true)) {
        return None;
    }
    what_must_be_cleared(attempts).map(Stop::Blocked)
}

/// The denied invocation the **last two working Attempts** have in common, which is the whole
/// of *what must be cleared*. Separated from the stop so the Handback can name it on a Run it
/// only found blocked in the record.
pub fn what_must_be_cleared(attempts: &[Attempt]) -> Option<String> {
    let worked: Vec<&Attempt> = attempts.iter().filter(|a| !a.is_wait()).collect();
    let [.., before, last] = worked.as_slice() else {
        return None;
    };
    let earlier = denied_invocations(before);
    denied_invocations(last)
        .into_iter()
        .find(|denial| earlier.contains(denial))
}

/// A denial as an identity, so *the same invocation twice* is a comparison rather than a guess.
fn denied_invocations(attempt: &Attempt) -> Vec<String> {
    attempt
        .permission_denials
        .iter()
        .map(|denial| {
            let tool = denial
                .get("tool_name")
                .and_then(|t| t.as_str())
                .unwrap_or("?");
            let input = denial.get("tool_input");
            let said = input
                .and_then(|i| i.get("command"))
                .and_then(|c| c.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| input.map(ToString::to_string).unwrap_or_default());
            format!("{tool}({said})")
        })
        .collect()
}

/// Read the stated reset hour out of a rate-limit message shaped like Run 4's — `"resets 12am
/// (Europe/Paris)"`, `"resets 3pm"` — and the sleep needed to reach that wall-clock hour from
/// `now`, capped at 12h so a garbled parse can never park a Run for a week.
///
/// **The timezone name is not resolved.** Only the stated hour is read; a message naming a
/// timezone the host is not itself in reads against the host's own clock, which is a coarser
/// answer than resolving it properly but strictly better than the fixed sleep it replaces — that
/// one ignored the wall clock entirely. Unparseable or absent text returns `None`, and the caller
/// falls back to the recorded fixed sleep (`Budget::limit_sleep`) for that reading, exactly as
/// before this existed.
///
/// `now` is `(hour, minute)`, 24-hour, supplied by the caller — this module stays pure, so it
/// never reads a clock itself (ADR-0007).
pub fn reset_time_sleep(text: &str, now: (u32, u32)) -> Option<Duration> {
    let target_hour = parse_reset_hour(text)?;
    let (now_hour, now_minute) = (now.0 % 24, now.1.min(59));
    let hours_ahead = if target_hour >= now_hour {
        target_hour - now_hour
    } else {
        24 - now_hour + target_hour
    };
    let mut seconds = u64::from(hours_ahead) * 3600;
    seconds = seconds.saturating_sub(u64::from(now_minute) * 60);
    if seconds == 0 {
        seconds = 24 * 3600;
    }
    Some(Duration::from_secs(seconds.min(12 * 3600)))
}

/// `"resets 12am (Europe/Paris)"` -> `Some(0)`, `"resets 3pm"` -> `Some(15)`, garbage or a
/// missing `"resets "` marker -> `None`. Also accepts a bare 24-hour hour with no am/pm marker,
/// since a message need not use 12-hour time to be a stated reset time.
fn parse_reset_hour(text: &str) -> Option<u32> {
    let after = text.find("resets ").map(|i| &text[i + "resets ".len()..])?;
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || digits.len() > 2 {
        return None;
    }
    let mut hour: u32 = digits.parse().ok()?;
    let rest = &after[digits.len()..];
    let marker: String = rest.chars().take(2).collect::<String>().to_lowercase();
    if marker == "am" && hour == 12 {
        hour = 0;
    } else if marker == "pm" && hour != 12 {
        hour += 12;
    }
    (hour < 24).then_some(hour)
}

/// The Blocker a terminal reason earns when it names one of the three credential statuses,
/// carrying the reason verbatim; anything else reads as transient and gets `None`.
///
/// **Why these three:** HTTP 401, 402 and 403 are verdicts on *the credential* itself — a
/// missing or revoked key, an exhausted balance, a refused authorization — so they read the
/// same on every retry. Re-entering cannot change them; only a human acting outside Grind
/// can, which makes them facts about the world in exactly the sense `Stop::Blocked` exists
/// for, alongside the rate limit. Everything else a failed turn can name (timeouts, server
/// errors, malformed output) stays transient and keeps the ordinary re-entry. The literals
/// are matched as bare `HTTP <status>` substrings because that is the exact shape native.rs
/// records into `terminal_reason`, and the whole reason is quoted back so a later
/// `grind cleared` handback states what actually had to be cleared.
/// **Checked before the budget guards in [`next`]:** a credential verdict burns every
/// remaining attempt identically, so it must win over `Stop::Exhausted` even on the final
/// allowed attempt — exhaustion there dies unactionable exactly when the clearance path is
/// what a human needs.
fn credential_blocker(terminal_reason: &str) -> Option<String> {
    for status in ["HTTP 401", "HTTP 402", "HTTP 403"] {
        if terminal_reason.contains(status) {
            return Some(format!(
                "the provider rejects this credential — {status}: {terminal_reason}"
            ));
        }
    }
    None
}

/// The whole policy, as one pure function over the attempt list and the verdict.
pub fn next(
    attempts: &[Attempt],
    verdict: &Verdict,
    ci_red: &Observed<bool>,
    reobservations: usize,
    budget: &Budget,
) -> Next {
    match verdict {
        Verdict::Completed => {
            let spent = attempts.iter().any(|a| a.mode == Mode::CiBabysit);
            if matches!(ci_red, Observed::Present(true)) && !spent {
                Next::SpendCiBudget
            } else {
                Next::Stop(Stop::Completed)
            }
        }
        Verdict::Uncorroborated(unmet) => Next::Stop(Stop::Uncorroborated(unmet.clone())),
        Verdict::Unobserved(blind) => {
            if reobservations < budget.reobservations {
                Next::Reobserve(budget.reobserve_pause)
            } else {
                Next::Stop(Stop::Unobserved(blind.clone()))
            }
        }
        Verdict::Incomplete(_) => {
            let credential = attempts
                .last()
                .and_then(|last| last.terminal_reason.as_deref())
                .and_then(credential_blocker);
            if let Some(blocked_on) = credential {
                return Next::Stop(Stop::Blocked(blocked_on));
            }
            if attempt::working(attempts) >= budget.attempts {
                return Next::Stop(Stop::Exhausted);
            }
            if attempt::trailing_waits(attempts) >= budget.consecutive_waits {
                return Next::Stop(Stop::Exhausted);
            }
            match attempts.last() {
                Some(last) if last.rate_limited => Next::SleepThenReenter(budget.limit_sleep),
                _ => Next::Reenter,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::Reason;

    fn budget(attempts: usize, limit_sleep_secs: u64) -> Budget {
        budget_with_pause(attempts, limit_sleep_secs, 15)
    }

    fn budget_with_pause(
        attempts: usize,
        limit_sleep_secs: u64,
        reobserve_pause_secs: u64,
    ) -> Budget {
        Budget {
            attempts,
            limit_sleep: Duration::from_secs(limit_sleep_secs),
            reobservations: 3,
            reobserve_pause: Duration::from_secs(reobserve_pause_secs),
            consecutive_waits: 12,
        }
    }

    /// An Attempt that **did work** — real cost, many turns. The budget counts these.
    fn attempt(n: usize, mode: Mode, rate_limited: bool) -> Attempt {
        Attempt {
            n,
            mode,
            started_at: "s".to_string(),
            ended_at: "e".to_string(),
            exit_code: Some(1),
            is_error: true,
            parse_ok: true,
            subtype: Some("success".to_string()),
            stop_reason: None,
            api_error_status: rate_limited.then(|| "429".to_string()),
            terminal_reason: Some("api_error".to_string()),
            num_turns: Some(37),
            total_cost_usd: Some(2.35),
            usage: None,
            permission_denials: vec![],
            done_promise: false,
            rate_limited,
            result_tail: String::new(),
            fanout: crate::observe::Observed::Absent,
            transcript_name: None,
        }
    }

    /// An Attempt that did **no** work: it parsed, cost nothing and took one turn. Run 2's
    /// attempts 3 through 7, exactly.
    fn wait(n: usize, rate_limited: bool) -> Attempt {
        Attempt {
            num_turns: Some(1),
            total_cost_usd: Some(0.0),
            ..attempt(n, Mode::Resume, rate_limited)
        }
    }

    /// A child that died before emitting parseable JSON. Never a Wait, whatever is absent.
    fn crashed(n: usize) -> Attempt {
        Attempt {
            parse_ok: false,
            subtype: Some("unparseable-output".to_string()),
            num_turns: None,
            total_cost_usd: None,
            ..attempt(n, Mode::Resume, false)
        }
    }

    fn incomplete() -> Verdict {
        Verdict::Incomplete(vec!["PR open".to_string()])
    }

    fn clear() -> Observed<bool> {
        Observed::Present(false)
    }

    #[test]
    fn a_rate_limited_attempt_asks_for_the_recorded_sleep_and_not_a_constant() {
        let attempts = [attempt(1, Mode::Dispatch, true)];
        let incomplete = Verdict::Incomplete(vec!["PR open".to_string()]);
        assert_eq!(
            next(&attempts, &incomplete, &clear(), 0, &budget(8, 1800)),
            Next::SleepThenReenter(Duration::from_secs(1800))
        );
        assert_eq!(
            next(&attempts, &incomplete, &clear(), 0, &budget(8, 600)),
            Next::SleepThenReenter(Duration::from_secs(600))
        );
    }

    #[test]
    fn an_attempt_list_at_the_budget_stops_as_exhausted_and_that_is_not_a_failure() {
        let attempts: Vec<Attempt> = (1..=8).map(|n| attempt(n, Mode::Resume, false)).collect();
        let found = next(
            &attempts,
            &Verdict::Incomplete(vec!["PR open".to_string()]),
            &clear(),
            0,
            &budget(8, 1800),
        );
        assert_eq!(found, Next::Stop(Stop::Exhausted));
        let words = format!("{found:?}").to_lowercase();
        for banned in ["fail", "error", "reject", "abort"] {
            assert!(
                !words.contains(banned),
                "exhaustion is its own fact: {words}"
            );
        }
    }

    #[test]
    fn a_rate_limited_attempt_at_the_budget_still_stops_rather_than_sleeping_forever() {
        let mut attempts: Vec<Attempt> = (1..=7).map(|n| attempt(n, Mode::Resume, false)).collect();
        attempts.push(attempt(8, Mode::Resume, true));
        assert_eq!(
            next(
                &attempts,
                &Verdict::Incomplete(vec![]),
                &clear(),
                0,
                &budget(8, 1800)
            ),
            Next::Stop(Stop::Exhausted)
        );
    }

    /// The dogfooded case: a missing authentication header is recorded verbatim by native.rs
    /// and used to be retried identically until the attempt budget died.
    #[test]
    fn a_credential_refusal_stops_as_blocked_instead_of_reentering() {
        for status in ["HTTP 401", "HTTP 402", "HTTP 403"] {
            let mut last = attempt(1, Mode::Dispatch, false);
            last.terminal_reason = Some(format!(
                "turn 1 failed: {status}: Missing Authentication header"
            ));
            let attempts = [last];
            let Next::Stop(Stop::Blocked(blocked_on)) =
                next(&attempts, &incomplete(), &clear(), 0, &budget(8, 1800))
            else {
                panic!("a {status} credential refusal must stop as Blocked");
            };
            assert!(
                blocked_on.contains(status),
                "the Blocker must quote the matched status line verbatim, got: {blocked_on}"
            );
        }
    }

    #[test]
    fn a_credential_refusal_on_the_final_allowed_attempt_blocks_rather_than_exhausting() {
        for status in ["HTTP 401", "HTTP 402", "HTTP 403"] {
            let mut attempts: Vec<Attempt> =
                (1..=8).map(|n| attempt(n, Mode::Resume, false)).collect();
            let last = attempts.last_mut().expect("the seeded final attempt");
            last.terminal_reason = Some(format!(
                "turn 1 failed: {status}: Missing Authentication header"
            ));
            let Next::Stop(Stop::Blocked(blocked_on)) =
                next(&attempts, &incomplete(), &clear(), 0, &budget(8, 1800))
            else {
                panic!("the final allowed attempt's {status} must Block, not Exhaust");
            };
            assert!(blocked_on.contains(status), "{blocked_on}");
        }
    }

    #[test]
    fn any_other_terminal_reason_still_reenters_as_before() {
        let mut last = attempt(1, Mode::Dispatch, false);
        last.terminal_reason = Some("api_error".to_string());
        let attempts = [last];
        assert_eq!(
            next(&attempts, &incomplete(), &clear(), 0, &budget(8, 1800)),
            Next::Reenter
        );
    }

    #[test]
    fn a_blind_signal_asks_to_look_again_and_never_to_re_enter() {
        let attempts = [attempt(1, Mode::Dispatch, false)];
        let blind = Verdict::Unobserved(vec!["PR open: gh pr view: connection reset".to_string()]);
        for spent in 0..3 {
            let found = next(&attempts, &blind, &clear(), spent, &budget(8, 1800));
            assert_eq!(
                found,
                Next::Reobserve(Duration::from_secs(15)),
                "{spent} re-observations spent"
            );
            assert_ne!(found, Next::Reenter);
        }
    }

    #[test]
    fn a_blind_signal_asks_for_the_recorded_pause_and_not_a_constant() {
        let attempts = [attempt(1, Mode::Dispatch, false)];
        let blind = Verdict::Unobserved(vec!["PR open: gh pr view: connection reset".to_string()]);
        assert_eq!(
            next(
                &attempts,
                &blind,
                &clear(),
                0,
                &budget_with_pause(8, 1800, 15)
            ),
            Next::Reobserve(Duration::from_secs(15))
        );
        assert_eq!(
            next(
                &attempts,
                &blind,
                &clear(),
                0,
                &budget_with_pause(8, 1800, 5)
            ),
            Next::Reobserve(Duration::from_secs(5))
        );
    }

    #[test]
    fn re_observation_spent_stops_as_unobserved_rather_than_as_a_death() {
        let attempts = [attempt(1, Mode::Dispatch, false)];
        let blind = Verdict::Unobserved(vec!["PR open: connection reset".to_string()]);
        let found = next(&attempts, &blind, &clear(), 3, &budget(8, 1800));
        let Next::Stop(Stop::Unobserved(said)) = found else {
            panic!("*I could not look* is never reported as *the Run died*: {found:?}");
        };
        assert_eq!(said, vec!["PR open: connection reset".to_string()]);
    }

    #[test]
    fn a_decided_verdict_with_red_ci_buys_exactly_one_bounded_invocation() {
        let attempts = vec![attempt(1, Mode::Dispatch, false)];
        let red = Observed::Present(true);
        assert_eq!(
            next(&attempts, &Verdict::Completed, &red, 0, &budget(8, 1800)),
            Next::SpendCiBudget
        );

        let mut after = attempts.clone();
        after.push(attempt(2, Mode::CiBabysit, false));
        assert_eq!(
            next(&after, &Verdict::Completed, &red, 0, &budget(8, 1800)),
            Next::Stop(Stop::Completed)
        );
    }

    #[test]
    fn ci_that_could_not_be_observed_does_not_buy_the_invocation() {
        let attempts = [attempt(1, Mode::Dispatch, false)];
        let blind_ci = Observed::Unobservable(Reason::saying("gh: connection reset"));
        assert_eq!(
            next(
                &attempts,
                &Verdict::Completed,
                &blind_ci,
                0,
                &budget(8, 1800)
            ),
            Next::Stop(Stop::Completed)
        );
    }

    #[test]
    fn uncorroborated_stops_and_never_re_enters() {
        let attempts = [attempt(1, Mode::Dispatch, false)];
        let found = next(
            &attempts,
            &Verdict::Uncorroborated(vec!["PR open".to_string()]),
            &clear(),
            0,
            &budget(8, 1800),
        );
        assert_eq!(
            found,
            Next::Stop(Stop::Uncorroborated(vec!["PR open".to_string()]))
        );
        assert_ne!(found, Next::Reenter);
    }

    #[test]
    fn an_ordinary_death_with_budget_left_re_enters() {
        let attempts = [attempt(1, Mode::Dispatch, false)];
        assert_eq!(
            next(
                &attempts,
                &Verdict::Incomplete(vec!["PR open".to_string()]),
                &clear(),
                0,
                &budget(8, 1800)
            ),
            Next::Reenter
        );
    }

    #[test]
    fn a_wait_does_not_decrement_the_attempt_budget() {
        let mut attempts: Vec<Attempt> = (1..=8).map(|n| wait(n, true)).collect();
        attempts.push(attempt(9, Mode::Resume, false));
        assert_ne!(
            next(&attempts, &incomplete(), &clear(), 0, &budget(8, 1800)),
            Next::Stop(Stop::Exhausted)
        );
    }

    #[test]
    fn a_run_of_consecutive_waits_terminates_on_its_own_bound() {
        let eleven: Vec<Attempt> = (1..=11).map(|n| wait(n, true)).collect();
        assert_eq!(
            next(&eleven, &incomplete(), &clear(), 0, &budget(8, 1800)),
            Next::SleepThenReenter(Duration::from_secs(1800))
        );
        let twelve: Vec<Attempt> = (1..=12).map(|n| wait(n, true)).collect();
        assert_eq!(
            next(&twelve, &incomplete(), &clear(), 0, &budget(8, 1800)),
            Next::Stop(Stop::Exhausted),
            "*nothing is happening forever* is still terminal"
        );
    }

    #[test]
    fn a_working_attempt_ends_the_run_of_waits() {
        let mut attempts: Vec<Attempt> = (1..=11).map(|n| wait(n, true)).collect();
        attempts.push(attempt(12, Mode::Resume, false));
        attempts.extend((13..=15).map(|n| wait(n, true)));
        assert_eq!(
            crate::attempt::trailing_waits(&attempts),
            3,
            "the count is of the trailing run, not of the list"
        );
        assert_eq!(
            next(&attempts, &incomplete(), &clear(), 0, &budget(8, 1800)),
            Next::SleepThenReenter(Duration::from_secs(1800))
        );
    }

    #[test]
    fn the_consecutive_wait_bound_survives_a_re_entry() {
        let twelve: Vec<Attempt> = (1..=12).map(|n| wait(n, true)).collect();
        let after_a_restart = twelve.clone();
        assert_eq!(
            next(
                &after_a_restart,
                &incomplete(),
                &clear(),
                0,
                &budget(8, 1800)
            ),
            Next::Stop(Stop::Exhausted)
        );
    }

    #[test]
    fn an_unparseable_child_spends_the_budget_and_never_loops_forever() {
        let eight: Vec<Attempt> = (1..=8).map(crashed).collect();
        assert_eq!(crate::attempt::working(&eight), 8);
        assert_eq!(
            next(&eight, &incomplete(), &clear(), 0, &budget(8, 1800)),
            Next::Stop(Stop::Exhausted)
        );
    }

    #[test]
    fn replaying_run_2s_eight_attempt_shapes_leaves_five_waits_and_three_working_attempts() {
        let mut run2 = vec![
            Attempt {
                num_turns: Some(187),
                total_cost_usd: Some(37.04),
                ..attempt(1, Mode::Dispatch, false)
            },
            Attempt {
                num_turns: Some(52),
                total_cost_usd: Some(7.06),
                ..attempt(2, Mode::Resume, false)
            },
        ];
        run2.extend((3..=7).map(|n| wait(n, true)));
        run2.push(Attempt {
            num_turns: Some(96),
            total_cost_usd: Some(20.22),
            ..attempt(8, Mode::Resume, false)
        });

        assert_eq!(crate::attempt::working(&run2), 3);
        assert_eq!(run2.len() - crate::attempt::working(&run2), 5);
        assert_ne!(
            next(&run2, &incomplete(), &clear(), 0, &budget(8, 1800)),
            Next::Stop(Stop::Exhausted),
            "eight Attempts against a budget of eight, of which five did no work"
        );
    }

    #[test]
    fn wall_clock_is_not_a_bound_and_does_not_become_one() {
        let fields = format!("{:?}", budget(8, 1800));
        assert!(fields.contains("limit_sleep"), "{fields}");
        assert!(fields.contains("reobserve_pause"), "{fields}");
        for ceiling in ["deadline", "max_wall", "timeout", "elapsed"] {
            assert!(!fields.contains(ceiling), "{fields}");
        }
    }

    fn denying(n: usize, command: &str) -> Attempt {
        Attempt {
            permission_denials: vec![serde_json::json!({
                "tool_name": "Bash",
                "tool_input": {"command": command},
            })],
            ..attempt(n, Mode::Resume, false)
        }
    }

    fn stalled_twice() -> Vec<Observed<bool>> {
        vec![
            Observed::Unobservable(Reason::saying("nothing earlier")),
            Observed::Present(true),
            Observed::Present(true),
        ]
    }

    #[test]
    fn the_same_denial_on_two_consecutive_working_attempts_with_no_progress_fires_the_blocker() {
        let attempts = [
            attempt(1, Mode::Dispatch, false),
            denying(2, "git push --force-with-lease"),
            denying(3, "git push --force-with-lease"),
        ];
        let Some(Stop::Blocked(what)) = blocker(&attempts, &stalled_twice()) else {
            panic!("a repeated denial with no progress is a fact about the world");
        };
        assert!(what.contains("git push --force-with-lease"), "{what}");
    }

    #[test]
    fn a_single_denial_on_one_working_attempt_does_not_fire_it() {
        let attempts = [
            attempt(1, Mode::Dispatch, false),
            attempt(2, Mode::Resume, false),
            denying(3, "git push --force-with-lease"),
        ];
        assert_eq!(blocker(&attempts, &stalled_twice()), None);
    }

    #[test]
    fn two_denials_of_different_invocations_do_not_fire_it() {
        let attempts = [
            attempt(1, Mode::Dispatch, false),
            denying(2, "git push --force-with-lease"),
            denying(3, "gh pr merge 30"),
        ];
        assert_eq!(blocker(&attempts, &stalled_twice()), None);
    }

    #[test]
    fn a_denial_on_a_working_attempt_that_advanced_does_not_fire_it() {
        let attempts = [
            attempt(1, Mode::Dispatch, false),
            denying(2, "git push --force-with-lease"),
            denying(3, "git push --force-with-lease"),
        ];
        let advanced = vec![
            Observed::Unobservable(Reason::saying("nothing earlier")),
            Observed::Present(true),
            Observed::Present(false),
        ];
        assert_eq!(blocker(&attempts, &advanced), None);
    }

    #[test]
    fn an_unobservable_commit_count_on_either_attempt_never_fires_it() {
        let attempts = [
            attempt(1, Mode::Dispatch, false),
            denying(2, "git push --force-with-lease"),
            denying(3, "git push --force-with-lease"),
        ];
        let blind = Observed::Unobservable(Reason::saying("git rev-list --count: exit 128"));
        for stalls in [
            vec![Observed::Present(true), blind.clone()],
            vec![blind.clone(), Observed::Present(true)],
            vec![blind.clone(), blind.clone()],
        ] {
            assert_eq!(blocker(&attempts, &stalls), None, "{stalls:?}");
        }
    }

    #[test]
    fn a_declaration_with_no_recorded_denial_does_not_fire_it_on_its_own() {
        let declaring = |n: usize| Attempt {
            result_tail: "I am blocked: the signer is dead and I cannot sign a commit.".to_string(),
            ..attempt(n, Mode::Resume, false)
        };
        let attempts = [
            attempt(1, Mode::Dispatch, false),
            declaring(2),
            declaring(3),
        ];
        assert_eq!(blocker(&attempts, &stalled_twice()), None);
    }

    #[test]
    fn the_first_working_attempt_of_a_process_has_nothing_to_compare_against() {
        let now = Observed::Present(3);
        assert!(matches!(stalled(None, &now), Observed::Unobservable(_)));
        assert_eq!(
            stalled(Some(&Observed::Present(3)), &Observed::Present(3)),
            Observed::Present(true)
        );
        assert_eq!(
            stalled(Some(&Observed::Present(3)), &Observed::Present(4)),
            Observed::Present(false)
        );
        assert!(matches!(
            stalled(
                Some(&Observed::Unobservable(Reason::saying("x"))),
                &Observed::Present(4)
            ),
            Observed::Unobservable(_)
        ));
    }

    #[test]
    fn a_blocker_is_a_stop_and_never_a_verdict_variant() {
        let variants = [
            format!("{:?}", Verdict::Completed),
            format!("{:?}", Verdict::Uncorroborated(vec![])),
            format!("{:?}", Verdict::Unobserved(vec![])),
            format!("{:?}", Verdict::Incomplete(vec![])),
        ];
        for variant in variants {
            let said = variant.to_lowercase();
            for banned in ["blocked", "rejected", "failed"] {
                assert!(!said.contains(banned), "{variant}");
            }
        }
        assert!(matches!(
            Stop::Blocked("Bash(git push --force)".to_string()),
            Stop::Blocked(_)
        ));
    }

    #[test]
    fn a_stated_midnight_reset_is_read_from_a_run_4_shaped_message() {
        let text = "The 5-hour limit resets 12am (Europe/Paris).";
        assert_eq!(
            reset_time_sleep(text, (22, 0)),
            Some(Duration::from_secs(2 * 3600))
        );
    }

    #[test]
    fn a_stated_afternoon_reset_is_read_and_partial_minutes_are_subtracted() {
        let text = "resets 3pm";
        assert_eq!(
            reset_time_sleep(text, (13, 30)),
            Some(Duration::from_secs(3600 + 1800))
        );
    }

    #[test]
    fn garbage_text_falls_back_to_none() {
        assert_eq!(
            reset_time_sleep("nothing about a reset here", (10, 0)),
            None
        );
        assert_eq!(reset_time_sleep("resets whenever", (10, 0)), None);
        assert_eq!(reset_time_sleep("resets 37pm", (10, 0)), None);
    }

    #[test]
    fn the_reset_sleep_never_exceeds_twelve_hours() {
        let text = "resets 11pm";
        let found = reset_time_sleep(text, (0, 1)).unwrap();
        assert!(found <= Duration::from_secs(12 * 3600), "{found:?}");
    }

    #[test]
    fn a_reset_landing_now_asks_for_a_full_cycle_rather_than_spinning() {
        let text = "resets 12am";
        let found = reset_time_sleep(text, (0, 0)).unwrap();
        assert!(found > Duration::ZERO, "a zero sleep would spin");
    }

    #[test]
    fn nothing_here_blocks() {
        let attempts = [attempt(1, Mode::Dispatch, true)];
        let asked = next(
            &attempts,
            &Verdict::Incomplete(vec![]),
            &clear(),
            0,
            &budget(8, 1800),
        );
        assert_eq!(asked, Next::SleepThenReenter(Duration::from_secs(1800)));
    }
}
