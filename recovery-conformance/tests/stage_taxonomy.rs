//! The stage taxonomy is the suite's index: a name that drifts, collides, or
//! goes missing silently reroutes a crash to the wrong durable boundary.

use std::collections::BTreeSet;
use std::str::FromStr;

use recovery_conformance::stages::{BroadcastPoint, CrashTrigger, SubmissionKind};
use recovery_conformance::CrashStage;

#[test]
fn every_stage_has_a_distinct_name_that_round_trips() {
    let names: BTreeSet<&str> = CrashStage::ALL.iter().map(|stage| stage.name()).collect();
    assert_eq!(
        names.len(),
        CrashStage::ALL.len(),
        "two stages share a name, so `--stage` cannot address them separately"
    );

    for stage in CrashStage::ALL {
        assert_eq!(CrashStage::from_str(stage.name()).unwrap(), *stage);
    }
}

#[test]
fn an_unknown_stage_name_is_rejected_rather_than_defaulted() {
    // Defaulting would silently crash somewhere other than where the test
    // asked, and the assertions would then describe the wrong boundary.
    assert!(CrashStage::from_str("after-everything").is_err());
    assert!(CrashStage::from_str("").is_err());
}

#[test]
fn all_lists_every_stage_in_round_order() {
    let mut ordered = CrashStage::ALL.to_vec();
    ordered.sort();
    ordered.dedup();
    assert_eq!(
        ordered.as_slice(),
        CrashStage::ALL,
        "ALL must be sorted and complete: pre-broadcast branching drives one \
         round toward a later stage and assumes earlier ones are reachable first"
    );
}

#[test]
fn only_the_five_in_post_stages_are_broadcast_triggered() {
    let broadcast: Vec<CrashStage> = CrashStage::ALL
        .iter()
        .copied()
        .filter(|stage| matches!(stage.trigger(), CrashTrigger::Broadcast { .. }))
        .collect();

    assert_eq!(
        broadcast,
        vec![
            CrashStage::BeforeBroadcast,
            CrashStage::BeforeVoteBroadcast,
            CrashStage::AfterBroadcastUnread,
            CrashStage::AfterVoteBroadcast,
            CrashStage::AfterBroadcastRead,
        ],
        "a stage that moved between the reporter and the transport would either \
         never fire or fire at the wrong instruction"
    );
}

#[test]
fn broadcast_stages_name_the_submission_they_belong_to() {
    // One transport carries both delegations and votes. A stage that did not
    // name its submission would fire on whichever the round POSTed first.
    let kind = |stage: CrashStage| match stage.trigger() {
        CrashTrigger::Broadcast { submission, .. } => submission,
        CrashTrigger::Event => panic!("{stage} is not a broadcast stage"),
    };

    assert_eq!(
        kind(CrashStage::BeforeBroadcast),
        SubmissionKind::DelegateAndVoteBatch
    );
    assert_eq!(
        kind(CrashStage::AfterBroadcastUnread),
        SubmissionKind::DelegateAndVoteBatch
    );
    assert_eq!(
        kind(CrashStage::BeforeVoteBroadcast),
        SubmissionKind::DelegateAndVoteBatch
    );
    assert_eq!(
        kind(CrashStage::AfterVoteBroadcast),
        SubmissionKind::DelegateAndVoteBatch
    );
}

#[test]
fn the_two_pre_dispatch_stages_die_before_the_marker() {
    // This is the definitely-unsent boundary. Dying after the marker instead
    // would put a transaction on staging that the stage claims never left.
    for stage in [CrashStage::BeforeBroadcast, CrashStage::BeforeVoteBroadcast] {
        assert!(matches!(
            stage.trigger(),
            CrashTrigger::Broadcast {
                point: BroadcastPoint::BeforeDispatch,
                ..
            }
        ));
    }
}

#[test]
fn only_stages_before_the_first_post_are_replayable_from_a_copied_sidecar() {
    let replayable: Vec<CrashStage> = CrashStage::ALL
        .iter()
        .copied()
        .filter(|stage| !stage.touches_chain())
        .collect();

    // Everything up to and including the definitely-unsent reservation has
    // left staging untouched, so one provisioned round can branch into all of
    // them. The moment a POST may have been delivered the chain has moved and
    // cannot be rewound.
    assert_eq!(
        replayable,
        vec![
            CrashStage::BeforeDelegation,
            CrashStage::AfterNoteSelection,
            CrashStage::AfterPczt,
            CrashStage::AfterProof,
            CrashStage::BeforeCast,
            CrashStage::AfterSigning,
            CrashStage::AfterVoteProof,
            CrashStage::AfterVoteCommit,
            CrashStage::AfterHelperPlans,
            CrashStage::BeforeBroadcast,
            CrashStage::BeforeVoteBroadcast,
        ]
    );
}

#[test]
fn the_sharp_stages_are_the_two_double_spend_adjacent_ones() {
    let sharp: Vec<CrashStage> = CrashStage::ALL
        .iter()
        .copied()
        .filter(|stage| stage.is_sharp())
        .collect();
    assert_eq!(
        sharp,
        vec![
            CrashStage::BeforeBroadcast,
            CrashStage::AfterBroadcastUnread
        ]
    );
}

/// Every variant the enum declares, whether or not `ALL` selects it.
///
/// The `match` below carries the guarantee: it has no wildcard arm, so adding a
/// variant to `CrashStage` fails to compile here until it is listed. That is
/// what makes the exclusion test beneath it meaningful — without it, a stage
/// could be declared, hooked, and left out of `ALL` without anything noticing,
/// which is exactly what happened to `after-tree-sync`.
fn declared_stages() -> Vec<CrashStage> {
    let declared = vec![
        CrashStage::BeforeDelegation,
        CrashStage::AfterNoteSelection,
        CrashStage::AfterPczt,
        CrashStage::AfterProof,
        CrashStage::BeforeCast,
        CrashStage::AfterTreeSync,
        CrashStage::AfterSigning,
        CrashStage::AfterVoteProof,
        CrashStage::AfterVoteCommit,
        CrashStage::AfterHelperPlans,
        CrashStage::BeforeBroadcast,
        CrashStage::BeforeVoteBroadcast,
        CrashStage::AfterBroadcastUnread,
        CrashStage::AfterVoteBroadcast,
        CrashStage::AfterBroadcastRead,
        CrashStage::AfterTracking,
        CrashStage::AfterVoteConfirmed,
        CrashStage::BeforeSharePost,
        CrashStage::AfterSharePost,
        CrashStage::AfterShareAccepted,
    ];
    for stage in &declared {
        match stage {
            CrashStage::BeforeDelegation
            | CrashStage::AfterNoteSelection
            | CrashStage::AfterPczt
            | CrashStage::AfterProof
            | CrashStage::BeforeCast
            | CrashStage::AfterTreeSync
            | CrashStage::AfterSigning
            | CrashStage::AfterVoteProof
            | CrashStage::AfterVoteCommit
            | CrashStage::AfterHelperPlans
            | CrashStage::BeforeBroadcast
            | CrashStage::BeforeVoteBroadcast
            | CrashStage::AfterBroadcastUnread
            | CrashStage::AfterVoteBroadcast
            | CrashStage::AfterBroadcastRead
            | CrashStage::AfterTracking
            | CrashStage::AfterVoteConfirmed
            | CrashStage::BeforeSharePost
            | CrashStage::AfterSharePost
            | CrashStage::AfterShareAccepted => {}
        }
    }
    declared
}

/// A declared stage outside `ALL` is unparseable and unselectable, so it is
/// dead weight with a live abort hook behind it.
///
/// `after-tree-sync` is the one that is deliberately excluded: its first
/// witness is synthetic, so it is not a fresh-combined crash stage, and the
/// tree-read case is covered by the stall matrix instead. Keeping it here as a
/// named exception rather than an untested absence is the point — a silently
/// growing exclusion set is how a taxonomy rots.
#[test]
fn after_tree_sync_is_the_only_declared_stage_excluded_from_all() {
    let declared: BTreeSet<CrashStage> = declared_stages().into_iter().collect();
    let selectable: BTreeSet<CrashStage> = CrashStage::ALL.iter().copied().collect();
    let excluded: BTreeSet<CrashStage> = declared.difference(&selectable).copied().collect();
    assert_eq!(
        excluded,
        BTreeSet::from([CrashStage::AfterTreeSync]),
        "the set of declared-but-unselectable stages changed. A stage added to \
         CrashStage but not to ALL can never be selected, run, or reported."
    );
    // And the exclusion is real: it cannot be named on the command line.
    assert!(CrashStage::from_str("after-tree-sync").is_err());
}

/// Two stages with the same trigger fire at the same seam, so the matrix pays
/// for two staging rounds and learns one thing.
///
/// The two pairs below are the historical aliases: `before-vote-broadcast` and
/// `after-vote-broadcast` date from when delegations and votes were separate
/// POSTs, and a combined batch made them identical to their delegation
/// counterparts. They are allowed by name, so a *new* collision fails here
/// rather than quietly doubling the matrix's cost.
#[test]
fn only_the_known_historical_aliases_share_a_trigger() {
    // Only broadcast triggers can collide meaningfully. Every event stage
    // reports `CrashTrigger::Event` and is told apart by name inside the
    // reporter, so grouping those would flag fourteen stages as one seam.
    let mut by_trigger: std::collections::BTreeMap<String, Vec<CrashStage>> =
        std::collections::BTreeMap::new();
    for stage in CrashStage::ALL {
        if !matches!(stage.trigger(), CrashTrigger::Broadcast { .. }) {
            continue;
        }
        by_trigger
            .entry(format!("{:?}", stage.trigger()))
            .or_default()
            .push(*stage);
    }
    let collisions: BTreeSet<BTreeSet<CrashStage>> = by_trigger
        .values()
        .filter(|stages| stages.len() > 1)
        .map(|stages| stages.iter().copied().collect())
        .collect();
    assert_eq!(
        collisions,
        BTreeSet::from([
            BTreeSet::from([CrashStage::BeforeBroadcast, CrashStage::BeforeVoteBroadcast]),
            BTreeSet::from([
                CrashStage::AfterBroadcastUnread,
                CrashStage::AfterVoteBroadcast
            ]),
        ]),
        "stages sharing a trigger fire at the same seam and cost a round each"
    );
}
