//! The crash-during-recovery taxonomy, pinned.
//!
//! A live case costs a provisioned round and two or three killed children, so
//! these run first and hermetically: a name that stopped round-tripping, or a
//! pair whose second seam is no longer guaranteed to fire, is far cheaper to
//! catch here than forty minutes into a staging run.

use recovery_conformance::recrash::{RecrashCase, ALL};
use recovery_conformance::CrashStage;

#[test]
fn every_case_round_trips_through_its_name() {
    for case in ALL {
        assert_eq!(case.name().parse::<RecrashCase>().unwrap(), *case);
        assert_eq!(case.to_string(), case.name());
    }
}

#[test]
fn case_names_are_unique() {
    let mut names: Vec<&str> = ALL.iter().map(|case| case.name()).collect();
    names.sort_unstable();
    let count = names.len();
    names.dedup();
    assert_eq!(names.len(), count, "two recrash cases share a name");
}

#[test]
fn an_unknown_name_is_rejected_rather_than_selecting_nothing() {
    assert!("before-broadcast".parse::<RecrashCase>().is_err());
    assert!("".parse::<RecrashCase>().is_err());
}

/// A case with nothing in `again` is an ordinary crash stage wearing a
/// sequence's name, and would pay for a round while asking nothing new.
#[test]
fn every_case_crashes_at_least_twice() {
    for case in ALL {
        assert!(
            !case.again().is_empty(),
            "{case} arms no second crash, so it is an ordinary stage"
        );
        assert_eq!(case.crashes(), 1 + case.again().len());
        assert!(case.crashes() >= 2);
    }
}

/// Every armed stage must be one the matrix can actually select. A sequence
/// naming a stage outside `ALL` would fail at spawn time, live, after
/// provisioning a round.
#[test]
fn every_armed_stage_is_a_selectable_crash_stage() {
    for case in ALL {
        for stage in case.stages() {
            assert!(
                CrashStage::ALL.contains(&stage),
                "{case} arms {stage}, which is not in CrashStage::ALL"
            );
        }
    }
}

/// The rule that keeps this dimension honest: `run_until_crash` fails a round
/// that finished without the seam firing, so a second stage the resume might
/// legitimately skip would make its case flaky rather than strict.
///
/// `after-broadcast-unread` is the clearest example. Recovery may resolve a
/// hashless dispatch by scanning the tree instead of re-POSTing, and which
/// route wins is the SDK's choice — so a resume can reach quiescence without
/// ever passing that seam again.
#[test]
fn no_case_arms_a_second_stage_the_resume_may_legitimately_skip() {
    for case in ALL {
        for stage in case.again() {
            assert_ne!(
                *stage,
                CrashStage::AfterBroadcastUnread,
                "{case} arms {stage} during recovery, but a hashless dispatch may be \
                 resolved by tree scan without passing that seam again"
            );
        }
    }
}

/// A share seam can only be armed on a resume that still owes *initial*
/// delivery, which means the first crash must land before the helper plans are
/// executed.
///
/// Learned from two live runs rather than from reading the code: a case whose
/// first crash was itself `after-share-accepted` reached
/// `BackgroundShareWorkOnly` on the resume — terminal success — without the
/// seam firing, because what a resume owes after delivery has begun is recovery
/// of those shares rather than a fresh initial delivery. The rule is stated
/// conservatively, as the observation supports, rather than as a claim about
/// which events the driver emits on which path.
#[test]
fn a_share_seam_is_only_armed_after_a_crash_that_precedes_delivery() {
    for case in ALL {
        if !case.again().contains(&CrashStage::AfterShareAccepted) {
            continue;
        }
        assert!(
            !matches!(
                case.first(),
                CrashStage::BeforeSharePost
                    | CrashStage::AfterSharePost
                    | CrashStage::AfterShareAccepted
            ),
            "{case} arms a share seam during recovery, but its first crash at {} is \
             itself inside share delivery, so the resume owes only background \
             recovery of those shares and the seam cannot fire",
            case.first()
        );
    }
}

#[test]
fn every_case_states_what_only_it_can_ask() {
    for case in ALL {
        assert!(!case.asks().is_empty(), "{case} documents no question");
    }
}

/// The crash loop is the case that most directly answers the question the suite
/// exists for, so its shape is pinned rather than left to drift.
#[test]
fn a_repeat_crash_case_arms_one_boundary_at_least_three_times() {
    let loops: Vec<&RecrashCase> = ALL
        .iter()
        .filter(|case| {
            case.crashes() >= 3
                && case
                    .stages()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    == 1
        })
        .collect();
    assert_eq!(
        loops.len(),
        1,
        "expected exactly one same-boundary crash loop, found {loops:?}"
    );
    assert_eq!(loops[0].first(), CrashStage::BeforeBroadcast);
}

/// Selection is by name, and the selector is what a targeted live run uses, so
/// a rename that broke it would only show up on staging.
#[test]
fn selection_accepts_every_case_name() {
    for case in ALL {
        let parsed: RecrashCase = case.name().parse().unwrap();
        assert_eq!(parsed.first(), case.first());
        assert_eq!(parsed.again(), case.again());
    }
}
