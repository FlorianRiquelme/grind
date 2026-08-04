// Spike: does a three-valued `Observed<T>` stop Grind's completion verdict from collapsing
// "could not ask" into "asked and got no"? See ../FINDINGS.md for the measured answer.

/// What we know about one signal, after trying to observe it.
///
/// Exactly three states on purpose — no `Result<Option<T>, E>` stand-in. The point of a
/// dedicated type is that `match` on it must be exhaustive, so a caller cannot reach the
/// `Present`/`Absent` cases without the compiler also making them look at this one.
#[derive(Debug, Clone)]
enum Observed<T> {
    Present(T),
    Absent,
    Unobservable(String),
}

impl<T: std::fmt::Debug> Observed<T> {
    fn describe(&self) -> String {
        match self {
            Observed::Present(v) => format!("present({v:?})"),
            Observed::Absent => "absent".to_string(),
            Observed::Unobservable(reason) => format!("unobservable({reason})"),
        }
    }
}

/// The five ANDed completion signals from Grind's decision "completion is observed, never
/// declared" (CONTEXT.md). Each is independently observed — and independently observable.
struct Signals {
    pr_open: Observed<bool>,
    tree_clean: Observed<bool>,
    commits_ahead: Observed<bool>,
    ci_clear: Observed<bool>,       // true means "no CI check pending"
    verify_reported: Observed<bool>, // true means the target repo's `just verify` reported an outcome
}

/// The verdict a supervisor is allowed to reach. `Completed` is unreachable unless every
/// signal was actually observed — a missing observation can only ever produce
/// `Uncorroborated`, never silently read as a negative `Incomplete`.
#[derive(Debug)]
enum Verdict {
    Completed,
    Incomplete(Vec<&'static str>),
    Uncorroborated(Vec<String>),
}

fn verdict(signals: &Signals) -> Verdict {
    let named: [(&'static str, &Observed<bool>); 5] = [
        ("pr_open", &signals.pr_open),
        ("tree_clean", &signals.tree_clean),
        ("commits_ahead", &signals.commits_ahead),
        ("ci_clear", &signals.ci_clear),
        ("verify_reported", &signals.verify_reported),
    ];

    let mut unobservable = Vec::new();
    let mut failed = Vec::new();

    for (name, sig) in named {
        // Exhaustive: this is the whole point. Add a fourth Observed<T> variant, or forget
        // one of these arms, and this file stops compiling — see wont-compile/.
        match sig {
            Observed::Present(true) => {}
            Observed::Present(false) => failed.push(name),
            Observed::Absent => failed.push(name),
            Observed::Unobservable(reason) => unobservable.push(format!("{name}: {reason}")),
        }
    }

    // Unobservable wins over failed: one signal grind could not ask about is enough to
    // withhold `Completed`, even if every other signal it *could* ask looked satisfied.
    if !unobservable.is_empty() {
        return Verdict::Uncorroborated(unobservable);
    }
    if failed.is_empty() {
        Verdict::Completed
    } else {
        Verdict::Incomplete(failed)
    }
}

fn print_scenario(name: &str, signals: Signals) {
    println!("=== {name} ===");
    println!("  pr_open:       {}", signals.pr_open.describe());
    println!("  tree_clean:    {}", signals.tree_clean.describe());
    println!("  commits_ahead: {}", signals.commits_ahead.describe());
    println!("  ci_clear:      {}", signals.ci_clear.describe());
    println!("  verify_reported: {}", signals.verify_reported.describe());
    println!("  verdict:       {:?}", verdict(&signals));
    println!();
}

fn main() {
    print_scenario(
        "all five present and satisfied",
        Signals {
            pr_open: Observed::Present(true),
            tree_clean: Observed::Present(true),
            commits_ahead: Observed::Present(true),
            ci_clear: Observed::Present(true),
            verify_reported: Observed::Present(true),
        },
    );

    print_scenario(
        "one signal observed absent (tree is dirty)",
        Signals {
            pr_open: Observed::Present(true),
            tree_clean: Observed::Present(false),
            commits_ahead: Observed::Present(true),
            ci_clear: Observed::Present(true),
            verify_reported: Observed::Present(true),
        },
    );

    print_scenario(
        "one signal unobservable (gh pr view timed out)",
        Signals {
            pr_open: Observed::Unobservable("gh pr view: network timeout".into()),
            tree_clean: Observed::Present(true),
            commits_ahead: Observed::Present(true),
            ci_clear: Observed::Present(true),
            verify_reported: Observed::Present(true),
        },
    );

    print_scenario(
        "two signals unobservable (laptop woke mid-Run, gh flaky)",
        Signals {
            pr_open: Observed::Unobservable("gh pr view: connection reset".into()),
            tree_clean: Observed::Present(true),
            commits_ahead: Observed::Present(true),
            ci_clear: Observed::Unobservable("gh api /checks: connection reset".into()),
            verify_reported: Observed::Present(true),
        },
    );

    print_scenario(
        "verify entrypoint's outcome unreadable (just verify hung, no result parsed)",
        Signals {
            pr_open: Observed::Present(true),
            tree_clean: Observed::Present(true),
            commits_ahead: Observed::Present(true),
            ci_clear: Observed::Present(true),
            verify_reported: Observed::Unobservable("just verify: no output before timeout".into()),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // The proof: enumerate all 3^5 combinations of signal states and check the one
    // invariant that matters. Any Unobservable signal anywhere forces Uncorroborated,
    // and Uncorroborated is never reachable any other way.
    #[test]
    fn unobservable_always_wins_and_only_unobservable_produces_it() {
        let states = |name: &'static str| -> [Observed<bool>; 3] {
            [
                Observed::Present(true),
                Observed::Present(false),
                Observed::Unobservable(format!("{name} could not be asked")),
            ]
        };

        let mut checked = 0;
        for pr_open in states("pr_open") {
            for tree_clean in states("tree_clean") {
                for commits_ahead in states("commits_ahead") {
                    for ci_clear in states("ci_clear") {
                        for verify_reported in states("verify_reported") {
                            let any_unobservable = [
                                &pr_open,
                                &tree_clean,
                                &commits_ahead,
                                &ci_clear,
                                &verify_reported,
                            ]
                            .iter()
                            .any(|s| matches!(s, Observed::Unobservable(_)));

                            let v = verdict(&Signals {
                                pr_open: pr_open.clone(),
                                tree_clean: tree_clean.clone(),
                                commits_ahead: commits_ahead.clone(),
                                ci_clear: ci_clear.clone(),
                                verify_reported: verify_reported.clone(),
                            });

                            match v {
                                Verdict::Uncorroborated(_) => assert!(
                                    any_unobservable,
                                    "reached Uncorroborated with no Unobservable signal"
                                ),
                                Verdict::Completed | Verdict::Incomplete(_) => assert!(
                                    !any_unobservable,
                                    "an Unobservable signal produced {v:?} instead of Uncorroborated"
                                ),
                            }
                            checked += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(checked, 3 * 3 * 3 * 3 * 3);
    }
}
