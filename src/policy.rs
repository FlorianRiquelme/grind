//! When to re-enter, when to sleep, when to stop — **returned as a value**.
//!
//! A `thread::sleep` in here makes *"a rate limit asks for thirty minutes"* an assertion you
//! have to wait thirty minutes for. Returned, it is an equality check on a literal, and the
//! loop is the only thing that blocks.
//!
//! **Never a pre-flight quota check** (ADR-0004). Even a perfectly informed supervisor would be
//! wrong about what a stage costs, so Grind sleeps long and re-enters rather than predicting.

use crate::attempt::{Attempt, Mode};
use crate::decide::Verdict;
use crate::observe::Observed;
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
}

/// What the loop does next.
#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    Reenter,
    SleepThenReenter(Duration),
    /// Look again. **Never a re-entry** — an unobservable signal is a fault in Grind's eyes, and
    /// paying for it with an attempt would let Grind's blindness mutate a branch.
    Reobserve,
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
            // Red CI does not hold the verdict open — it buys exactly one fresh bounded
            // invocation, once, recorded with its own mode so a spent CI budget is visible.
            let spent = attempts.iter().any(|a| a.mode == Mode::CiBabysit);
            if matches!(ci_red, Observed::Present(true)) && !spent {
                Next::SpendCiBudget
            } else {
                Next::Stop(Stop::Completed)
            }
        }
        // A session that believes it finished would re-emit the promise until the budget was
        // gone, so this stops rather than re-entering.
        Verdict::Uncorroborated(unmet) => Next::Stop(Stop::Uncorroborated(unmet.clone())),
        Verdict::Unobserved(blind) => {
            if reobservations < budget.reobservations {
                Next::Reobserve
            } else {
                Next::Stop(Stop::Unobserved(blind.clone()))
            }
        }
        Verdict::Incomplete(_) => {
            if attempts.len() >= budget.attempts {
                return Next::Stop(Stop::Exhausted);
            }
            match attempts.last() {
                // The recorded sleep, not a constant.
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
        Budget {
            attempts,
            limit_sleep: Duration::from_secs(limit_sleep_secs),
            reobservations: 3,
        }
    }

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
            num_turns: Some(1),
            total_cost_usd: Some(0.0),
            usage: None,
            permission_denials: vec![],
            done_promise: false,
            rate_limited,
            result_tail: String::new(),
        }
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
        // The same policy against a record carrying a different limit sleep.
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

    #[test]
    fn a_blind_signal_asks_to_look_again_and_never_to_re_enter() {
        let attempts = [attempt(1, Mode::Dispatch, false)];
        let blind = Verdict::Unobserved(vec!["PR open: gh pr view: connection reset".to_string()]);
        for spent in 0..3 {
            let found = next(&attempts, &blind, &clear(), spent, &budget(8, 1800));
            assert_eq!(found, Next::Reobserve, "{spent} re-observations spent");
            assert_ne!(found, Next::Reenter);
        }
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

        // Once spent, a second red-CI decision stops rather than buying another.
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
    fn nothing_here_blocks() {
        // The proof that effects are values: the whole suite runs in the time one assertion
        // takes, against a policy whose answer is a thirty-minute sleep.
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
