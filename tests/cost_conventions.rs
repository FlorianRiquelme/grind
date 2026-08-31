//! The one place all three adapters' `Attempt::total_cost_usd` conventions sit side by side
//! (issue #194).
//!
//! `total_cost_usd` is produced three unrelated ways and its `None` means something different
//! in each — an ambiguous absent JSON key on claude-code, an impossibility on native, *the
//! harness has no spend channel* on omp. Each convention is individually reasoned and each is
//! separately tested where it is produced. What was missing is the comparison: the field is
//! read by `Attempt::is_wait`, which decides whether an Attempt spends the attempt budget and
//! what `trailing_waits` counts against `CONSECUTIVE_WAITS`, so a fourth adapter picking a
//! convention by accident is a rate-limit loop that never terminates or a crash loop that
//! costs nothing.
//!
//! **No defect is pinned here.** Every assertion below records today's deliberate behavior, and
//! all three land on the safe side of `is_wait`. What fails this test is a *change* to one
//! adapter's convention made without the reader noticing the other two.

use grind::attempt::{Attempt, Mode};

fn claude(payload: &str) -> Attempt {
    grind::claude::classify(payload, "", Some(0), 1, Mode::Dispatch, "start", "end")
}

fn omp(frames: &str) -> Attempt {
    grind::omp::classify(frames, "", Some(0), 1, Mode::Dispatch, "start", "end")
}

fn usage(json: &str) -> serde_json::Value {
    serde_json::from_str(json).expect("a literal usage object")
}

/// **claude-code takes the child's own field, and its `None` is ambiguous.** A payload that
/// parsed but never carried `total_cost_usd` looks exactly like a true zero from here, which
/// is why `is_wait` keys on presence: absence spends the budget, the safe direction.
#[test]
fn the_claude_code_convention_is_the_childs_own_field_and_absence_stays_ambiguous() {
    let missing = claude(
        r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","num_turns":1}"#,
    );
    assert_eq!(missing.total_cost_usd, None);
    assert!(
        !missing.is_wait(),
        "a renamed or absent cost key must spend the budget rather than read as no work"
    );

    let zero = claude(
        r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","num_turns":1,"total_cost_usd":0.0}"#,
    );
    assert_eq!(zero.total_cost_usd, Some(0.0));
    assert!(
        zero.is_wait(),
        "an explicit zero over one turn is Run 2's Wait shape"
    );

    let spent = claude(
        r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","num_turns":1,"total_cost_usd":0.42}"#,
    );
    assert_eq!(spent.total_cost_usd, Some(0.42));
    assert!(!spent.is_wait());
}

/// **native floors, and never reports absence.** The loop authoritatively knows what it
/// recorded, so `None` would be a lie — and an expensive one: it would make every native
/// Attempt non-Wait regardless of turns, letting a first-turn rate limit spend the budget and
/// keeping `trailing_waits` at 0 forever.
#[test]
fn the_native_convention_is_a_floor_and_never_none() {
    assert_eq!(
        grind::native::cost_floor(None),
        Some(0.0),
        "no usage at all is a recorded zero, not an unknown"
    );
    assert_eq!(
        grind::native::cost_floor(Some(&usage(r#"{"input":10,"output":20}"#))),
        Some(0.0),
        "usage without a cost key is still a recorded zero"
    );
    assert_eq!(
        grind::native::cost_floor(Some(&usage(r#"{"cost":0.25}"#))),
        Some(0.25)
    );
    assert_eq!(
        grind::native::cost_floor(Some(&usage(r#"{"cost":"not-a-number"}"#))),
        Some(0.0),
        "a cost of the wrong type degrades to the floor rather than to absence"
    );
}

/// **omp reports absence, and means something a zero cannot say.** Run 178's real shape:
/// frames flow and no message carries usage, because the harness exposed no spend channel.
/// `Some(0.0)` there would manufacture a `$0.00` the harness never reported *and* make the
/// attempt a Wait, which is the unsafe direction — so `None` it is, and `None` spends the
/// budget.
#[test]
fn the_omp_convention_is_absence_meaning_no_spend_channel_at_all() {
    let silent = omp(concat!(
        r#"{"type":"turn_start","n":1}"#,
        "\n",
        r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"done"}],"stopReason":"endTurn"}}"#,
        "\n",
        r#"{"type":"turn_end"}"#,
        "\n",
        r#"{"type":"agent_end","messages":[{"role":"assistant","content":[{"type":"text","text":"done"}]}]}"#,
    ));
    assert!(silent.parse_ok);
    assert_eq!(silent.num_turns, Some(1));
    assert_eq!(silent.total_cost_usd, None);
    assert!(
        !silent.is_wait(),
        "an unreported spend channel spends the budget; the alternative is an endless Run"
    );

    let reported = omp(concat!(
        r#"{"type":"turn_start","n":1}"#,
        "\n",
        r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"done"}],"stopReason":"endTurn","usage":{"cost":{"total":0.0}}}}"#,
        "\n",
        r#"{"type":"turn_end"}"#,
        "\n",
        r#"{"type":"agent_end","messages":[{"role":"assistant","content":[{"type":"text","text":"done"}]}]}"#,
    ));
    assert_eq!(
        reported.total_cost_usd,
        Some(0.0),
        "a channel that reported zero is a different fact from no channel"
    );
    assert!(reported.is_wait());
}

/// The comparison itself: **the same absent cost means three different things**, and the two
/// backends that can report it disagree about what a silent turn is. Nothing here asks them to
/// agree — it asks that changing one of them be impossible to do quietly.
#[test]
fn the_three_conventions_disagree_and_that_is_the_documented_state() {
    let claude_silent = claude(
        r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","num_turns":1}"#,
    );
    let omp_silent = omp(
        r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"done"}],"stopReason":"endTurn"}}"#,
    );
    let native_silent = grind::native::cost_floor(None);

    assert_eq!(claude_silent.total_cost_usd, None);
    assert_eq!(omp_silent.total_cost_usd, None);
    assert_eq!(native_silent, Some(0.0));
    assert_ne!(
        claude_silent.total_cost_usd, native_silent,
        "a turn that reported no cost is `None` on two adapters and `Some(0.0)` on the third — \
         `Attempt::total_cost_usd`'s doc is where a reader is told which producer they have"
    );
}
