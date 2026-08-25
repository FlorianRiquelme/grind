//! The ten-rung stage ladder Grit walks, and the total pure function that climbs it.
//!
//! Advancement over `decide::Stage`'s five pre-cutover states was a scan the supervisor smeared
//! through its own loop. Here it is a value: `next` reads nothing but the durable returns a Run
//! already wrote to disk, so **any morning diagnosis replays the exact decision from run state
//! alone** (ADR-0007) — no clock, no `world`, nothing this module cannot already see.
//!
//! Nothing here wires into the supervisor yet. This is the pure vocabulary phase 1 of Grit
//! ships; `RunRecord` gaining `stages: Vec<StageEntry>` and `resume()` calling `next` instead of
//! scanning are later changes.

use serde::{Deserialize, Serialize};

/// The ten-rung ladder. Two of the ten ([R] `Triage` and `DiffTriage`) are pure-Rust passes with
/// no session — they still write a return and occupy a rung uniformly, so `next` never
/// special-cases them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stage {
    Plan,
    Triage,
    PlanReview,
    Work,
    Simplify,
    DiffTriage,
    Review,
    Validate,
    Fixes,
    Ship,
}

/// Every rung, in ladder order — the same order [`next`] walks. Lets a caller enumerate
/// `stages/<name>.return.json` on disk without hand-writing the ten names a second time.
pub const ALL: [Stage; 10] = [
    Stage::Plan,
    Stage::Triage,
    Stage::PlanReview,
    Stage::Work,
    Stage::Simplify,
    Stage::DiffTriage,
    Stage::Review,
    Stage::Validate,
    Stage::Fixes,
    Stage::Ship,
];

impl std::fmt::Display for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let word = match self {
            Stage::Plan => "plan",
            Stage::Triage => "triage",
            Stage::PlanReview => "plan-review",
            Stage::Work => "work",
            Stage::Simplify => "simplify",
            Stage::DiffTriage => "diff-triage",
            Stage::Review => "review",
            Stage::Validate => "validate",
            Stage::Fixes => "fixes",
            Stage::Ship => "ship",
        };
        write!(f, "{word}")
    }
}

/// What a stage's own return says about itself. **A fact, never a grade** — there is no
/// `Rejected` or `Failed` here, the same rule ADR-0006 states for `decide::Verdict`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReturnStatus {
    Complete,
    /// Simplify at T0 is absorbed by Work, and the skip is itself a return row — keeping `next`
    /// total rather than carving an exception into it for one stage.
    Skipped,
    Incomplete,
}

/// One stage's return file, as far as advancement needs it. Strict serde: a field renamed on
/// either side of the write/read seam is `serde(deny_unknown_fields)` catching a rename, never a
/// meaning that drifted.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StageReturn {
    pub status: ReturnStatus,
    /// PlanReview's conditional revision round lives inside its own return rather than as a
    /// second `Stage` variant, keeping the enum flat. Absent on every other stage's return.
    #[serde(default)]
    pub revised: bool,
}

/// The returns present on disk for a Run, one optional slot per stage. A struct with ten
/// `Option` fields is honest about which stages have and have not written back — a map would
/// let an omission hide as `None` from a lookup that never happened.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StageReturns {
    pub plan: Option<StageReturn>,
    pub triage: Option<StageReturn>,
    pub plan_review: Option<StageReturn>,
    pub work: Option<StageReturn>,
    pub simplify: Option<StageReturn>,
    pub diff_triage: Option<StageReturn>,
    pub review: Option<StageReturn>,
    pub validate: Option<StageReturn>,
    pub fixes: Option<StageReturn>,
    pub ship: Option<StageReturn>,
}

/// A stage is satisfied when its return exists and says `Complete` or `Skipped`. The design only
/// ever skips Simplify, but reading `Skipped` uniformly across every rung keeps this function
/// total without carving a per-stage exception into it.
fn satisfied(entry: &Option<StageReturn>) -> bool {
    matches!(
        entry,
        Some(StageReturn {
            status: ReturnStatus::Complete | ReturnStatus::Skipped,
            ..
        })
    )
}

/// The earliest stage whose return is absent or not satisfied is the next rung. Total: `None`
/// only when every stage through Ship is satisfied — the ladder is walked.
///
/// **A gap wins toward the earliest absent stage**, not the furthest complete one: a later
/// stage completing while an earlier one is still absent or incomplete re-enters the earlier
/// stage, which is exactly re-entry's contract — resume the stage that did not finish.
pub fn next(returns: &StageReturns) -> Option<Stage> {
    let ladder: [(Stage, &Option<StageReturn>); 10] = [
        (Stage::Plan, &returns.plan),
        (Stage::Triage, &returns.triage),
        (Stage::PlanReview, &returns.plan_review),
        (Stage::Work, &returns.work),
        (Stage::Simplify, &returns.simplify),
        (Stage::DiffTriage, &returns.diff_triage),
        (Stage::Review, &returns.review),
        (Stage::Validate, &returns.validate),
        (Stage::Fixes, &returns.fixes),
        (Stage::Ship, &returns.ship),
    ];
    for (stage, entry) in ladder {
        if !satisfied(entry) {
            return Some(stage);
        }
    }
    None
}

/// The serde row a Run's record carries per stage. Wired into `RunRecord.stages` (unit C).
/// Shapes follow `attempt::Attempt`'s conventions: cost and turn counts are `Option`, present
/// only once a session actually reports them.
///
/// **`name` is a `String`, not a `Stage`.** Reflect is a post-run pass, not a rung — it has no
/// `Stage` variant by design (the design's own words: *deliberately not an eleventh stage*), yet
/// it still needs a row so its session, cost and turns land in the record. A `Stage`-typed field
/// would have no honest variant to give it; `String` carries both a rung's `Display` output
/// (`"plan"`, `"work"`, …) and a post-run pass's own name (`"reflect"`) without inventing a
/// rung that does not exist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageEntry {
    pub name: String,
    pub session_id: String,
    pub status: ReturnStatus,
    pub artifact_paths: Vec<String>,
    pub model: Option<String>,
    pub cost_usd: Option<f64>,
    pub turns: Option<u64>,
}

/// Assemble [`StageReturns`] from ten optional slots of raw JSON text, one per stage in ladder
/// order — the shape a supervisor reading `stages/<name>.return.json` fresh off disk naturally
/// produces. **Tolerant per slot**: an absent file is `None`, and unparseable text also reads as
/// `None` rather than refusing the whole assembly — fail-closed toward re-entering that one
/// stage, never toward losing every other stage's progress over one bad file.
pub fn returns_from(slots: [Option<&str>; 10]) -> StageReturns {
    let parse = |raw: Option<&str>| raw.and_then(|text| serde_json::from_str(text).ok());
    StageReturns {
        plan: parse(slots[0]),
        triage: parse(slots[1]),
        plan_review: parse(slots[2]),
        work: parse(slots[3]),
        simplify: parse(slots[4]),
        diff_triage: parse(slots[5]),
        review: parse(slots[6]),
        validate: parse(slots[7]),
        fixes: parse(slots[8]),
        ship: parse(slots[9]),
    }
}

/// The explicit total mapping from the pre-cutover `decide::Stage` to the earliest new rung
/// each old variant implies, so a pre-cutover record resumes onto a real rung rather than
/// nowhere. No wildcard arm: a new pre-cutover variant fails to compile here rather than
/// silently mapping to the wrong rung.
pub fn from_furthest(stage: crate::decide::Stage) -> Stage {
    match stage {
        crate::decide::Stage::Dispatched => Stage::Plan,
        crate::decide::Stage::Planned => Stage::Work,
        crate::decide::Stage::Implemented => Stage::Review,
        crate::decide::Stage::Reviewed => Stage::Fixes,
        crate::decide::Stage::PrOpen => Stage::Ship,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete() -> StageReturn {
        StageReturn {
            status: ReturnStatus::Complete,
            revised: false,
        }
    }

    fn skipped() -> StageReturn {
        StageReturn {
            status: ReturnStatus::Skipped,
            revised: false,
        }
    }

    fn incomplete() -> StageReturn {
        StageReturn {
            status: ReturnStatus::Incomplete,
            revised: false,
        }
    }

    #[test]
    fn empty_returns_start_the_ladder_at_plan() {
        assert_eq!(next(&StageReturns::default()), Some(Stage::Plan));
    }

    #[test]
    fn a_full_walk_advances_one_rung_at_a_time_and_finishes_at_none() {
        let mut returns = StageReturns::default();
        let ladder = [
            Stage::Plan,
            Stage::Triage,
            Stage::PlanReview,
            Stage::Work,
            Stage::Simplify,
            Stage::DiffTriage,
            Stage::Review,
            Stage::Validate,
            Stage::Fixes,
            Stage::Ship,
        ];
        for stage in ladder {
            assert_eq!(next(&returns), Some(stage), "at {stage}");
            match stage {
                Stage::Plan => returns.plan = Some(complete()),
                Stage::Triage => returns.triage = Some(complete()),
                Stage::PlanReview => returns.plan_review = Some(complete()),
                Stage::Work => returns.work = Some(complete()),
                Stage::Simplify => returns.simplify = Some(complete()),
                Stage::DiffTriage => returns.diff_triage = Some(complete()),
                Stage::Review => returns.review = Some(complete()),
                Stage::Validate => returns.validate = Some(complete()),
                Stage::Fixes => returns.fixes = Some(complete()),
                Stage::Ship => returns.ship = Some(complete()),
            }
        }
        assert_eq!(next(&returns), None);
    }

    #[test]
    fn a_skipped_simplify_advances_past_it_the_same_as_complete() {
        let mut returns = StageReturns {
            plan: Some(complete()),
            triage: Some(complete()),
            plan_review: Some(complete()),
            work: Some(complete()),
            ..Default::default()
        };
        returns.simplify = Some(skipped());
        assert_eq!(next(&returns), Some(Stage::DiffTriage));
    }

    #[test]
    fn an_incomplete_return_re_enters_the_same_stage() {
        let returns = StageReturns {
            plan: Some(complete()),
            triage: Some(incomplete()),
            ..Default::default()
        };
        assert_eq!(next(&returns), Some(Stage::Triage));
    }

    #[test]
    fn a_gap_with_a_later_stage_complete_still_resumes_the_earliest_absent_one() {
        let returns = StageReturns {
            plan: Some(complete()),
            work: Some(complete()),
            ..Default::default()
        };
        assert_eq!(next(&returns), Some(Stage::Triage));
    }

    #[test]
    fn from_furthest_covers_all_five_pre_cutover_variants() {
        use crate::decide::Stage as Old;
        assert_eq!(from_furthest(Old::Dispatched), Stage::Plan);
        assert_eq!(from_furthest(Old::Planned), Stage::Work);
        assert_eq!(from_furthest(Old::Implemented), Stage::Review);
        assert_eq!(from_furthest(Old::Reviewed), Stage::Fixes);
        assert_eq!(from_furthest(Old::PrOpen), Stage::Ship);
    }

    #[test]
    fn a_return_with_no_revised_field_defaults_it_to_false() {
        let parsed: StageReturn = serde_json::from_str(r#"{"status":"complete"}"#).unwrap();
        assert_eq!(
            parsed,
            StageReturn {
                status: ReturnStatus::Complete,
                revised: false,
            }
        );
    }

    #[test]
    fn an_unknown_field_on_a_return_fails_to_parse() {
        let found: Result<StageReturn, _> =
            serde_json::from_str(r#"{"status":"complete","bogus":true}"#);
        assert!(found.is_err(), "deny_unknown_fields must catch a rename");
    }

    #[test]
    fn a_stage_entry_round_trips_through_serde() {
        let entry = StageEntry {
            name: Stage::Work.to_string(),
            session_id: "run-1-work".to_string(),
            status: ReturnStatus::Complete,
            artifact_paths: vec!["stages/work/evidence.json".to_string()],
            model: Some("claude-opus-5".to_string()),
            cost_usd: Some(1.23),
            turns: Some(7),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: StageEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, entry);
    }

    #[test]
    fn returns_from_assembles_ten_optional_slots_in_ladder_order() {
        let plan = r#"{"status":"complete"}"#;
        let triage = r#"{"status":"complete"}"#;
        let assembled = returns_from([
            Some(plan),
            Some(triage),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        ]);
        assert_eq!(assembled.plan, Some(complete()));
        assert_eq!(assembled.triage, Some(complete()));
        assert_eq!(assembled.plan_review, None);
        assert_eq!(next(&assembled), Some(Stage::PlanReview));
    }

    #[test]
    fn returns_from_reads_an_unparseable_slot_as_absent_rather_than_refusing() {
        let assembled = returns_from([
            Some("not json at all"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        ]);
        assert_eq!(assembled.plan, None);
        assert_eq!(next(&assembled), Some(Stage::Plan));
    }

    #[test]
    fn display_prints_kebab_case_for_all_ten_stages() {
        let words = [
            (Stage::Plan, "plan"),
            (Stage::Triage, "triage"),
            (Stage::PlanReview, "plan-review"),
            (Stage::Work, "work"),
            (Stage::Simplify, "simplify"),
            (Stage::DiffTriage, "diff-triage"),
            (Stage::Review, "review"),
            (Stage::Validate, "validate"),
            (Stage::Fixes, "fixes"),
            (Stage::Ship, "ship"),
        ];
        for (stage, word) in words {
            assert_eq!(stage.to_string(), word);
        }
    }
}
