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
                // The recorded pause, not a constant fired back-to-back.
                Next::Reobserve(budget.reobserve_pause)
            } else {
                Next::Stop(Stop::Unobserved(blind.clone()))
            }
        }
        Verdict::Incomplete(_) => {
            // Working Attempts only, read from the Attempt list and never from an observation.
            // A progress-based cap would have killed Run 2 *faster*: `commits_ahead` read zero
            // for all eight of its Attempts while twelve real commits existed.
            if attempt::working(attempts) >= budget.attempts {
                return Next::Stop(Stop::Exhausted);
            }
            // Wall-clock never bounds a Run; a run of Waits does. Any working Attempt ends the
            // run by construction, and the count comes off the persisted list so a restart
            // cannot reset it.
            if attempt::trailing_waits(attempts) >= budget.consecutive_waits {
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
        // Exactly the shape of `a_rate_limited_attempt_asks_for_the_recorded_sleep_and_not_a_constant`:
        // three retries fired back-to-back cannot span the transient this exists for, so the
        // spacing has to be a value read from the budget rather than a literal baked into the
        // loop.
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
        // The same policy against a budget carrying a different pause.
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

    // --- a Wait is an Attempt that did no work -----------------------------------------------

    #[test]
    fn a_wait_does_not_decrement_the_attempt_budget() {
        // Eight Waits and one working Attempt against a budget of eight: one spent, not nine.
        let mut attempts: Vec<Attempt> = (1..=8).map(|n| wait(n, true)).collect();
        attempts.push(attempt(9, Mode::Resume, false));
        assert_ne!(
            next(&attempts, &incomplete(), &clear(), 0, &budget(8, 1800)),
            Next::Stop(Stop::Exhausted)
        );
    }

    #[test]
    fn a_run_of_consecutive_waits_terminates_on_its_own_bound() {
        // Waits spend nothing, so this counter is the only thing standing between a permanent
        // wall and a Run that never stops.
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
        // The count is derived from the persisted list, so a fresh process reads the same
        // number the one that died would have. A loop-local counter would hand a
        // permanently-walled Run a fresh allowance at every reboot and never terminate — and
        // `resume --all` re-enters rate-limited Runs at boot by design.
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
        // The load-bearing clause of the predicate. A crash leaves both cost and turns absent,
        // and reading absence as *did no work* would make every crash loop free.
        let eight: Vec<Attempt> = (1..=8).map(crashed).collect();
        assert_eq!(crate::attempt::working(&eight), 8);
        assert_eq!(
            next(&eight, &incomplete(), &clear(), 0, &budget(8, 1800)),
            Next::Stop(Stop::Exhausted)
        );
    }

    #[test]
    fn replaying_run_2s_eight_attempt_shapes_leaves_five_waits_and_three_working_attempts() {
        // `docs/findings/0002`: attempt 1 at $37.04 and 187 turns, attempt 2 at $7.06,
        // attempts 3–7 at $0 and one turn each, attempt 8 at $20.22 — the Attempt that opened
        // the PR. Under the recorded budget of eight, three working Attempts is not exhaustion.
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
        // Nothing in the budget names a duration a Run may take. The two `Duration`s here are
        // both waits Grind performs, never ceilings on the Run.
        let fields = format!("{:?}", budget(8, 1800));
        assert!(fields.contains("limit_sleep"), "{fields}");
        assert!(fields.contains("reobserve_pause"), "{fields}");
        for ceiling in ["deadline", "max_wall", "timeout", "elapsed"] {
            assert!(!fields.contains(ceiling), "{fields}");
        }
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
