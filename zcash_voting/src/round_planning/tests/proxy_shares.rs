//! DC shares: only a landed batch's DCs owe shares, each owes every share it
//! has not handed off until the vote ends, and a handed-off share awaits its
//! reveal as background work, as an accepted vote share does. No DC share is
//! the round's immediate share.

use super::fixtures::*;
use crate::phases::DelegationPhase;
use crate::proxy::batch::ProxyBatchProgress;
use crate::proxy::shares::DcShareRecord;
use crate::proxy::DcShareLayout;
use crate::round_planning::{classify_round, plan_from_snapshot, resolve_step, Obligation};
use crate::session::{NextStep, RoundPlanAction};
use crate::share_tracking::CommitmentRef;

/// A planned DC share that `accepted` configured helpers definitely hold,
/// out of a target of 2, revealed `at` (0 for at once); once both hold it, a
/// delivery pass recorded it handed off, as passes do.
fn share(share_index: u32, accepted: usize, at: u64) -> DcShareRecord {
    DcShareRecord {
        share_index,
        nullifier: [share_index as u8; 32],
        target_count: 2,
        submit_at: at,
        sent_to_urls: (0..accepted)
            .map(|helper| format!("https://helper-{helper}.example"))
            .collect(),
        ambiguous_urls: Vec::new(),
        attempting_urls: Vec::new(),
        confirmed: false,
        handed_off_to_reachable_target: accepted >= 2,
        created_at: 0,
    }
}

/// Every share step of the plan's DCs, as `(kind, bundle, slot, share)`.
fn dc_steps(steps: &[NextStep]) -> Vec<(&'static str, u32, u8, u32)> {
    steps
        .iter()
        .filter_map(|step| match *step {
            NextStep::SubmitShares {
                bundle_index,
                commitment: CommitmentRef::Proxy { dc_slot },
                share_index,
            } => Some(("submit", bundle_index, dc_slot, share_index)),
            NextStep::ConfirmShare {
                bundle_index,
                commitment: CommitmentRef::Proxy { dc_slot },
                share_index,
            } => Some(("confirm", bundle_index, dc_slot, share_index)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_landed_dc_owes_every_share_its_layout_delivers() {
    let snapshot = snapshot()
        .bundle(0, DelegationPhase::Confirmed)
        .proxy_batch(0, ProxyBatchProgress::Landed)
        .landed_dc(0, 3, DcShareLayout::Standard, None)
        .landed_dc(0, 5, DcShareLayout::SingleShare, None)
        .build();

    let plan = plan_from_snapshot(&snapshot, &[1]).unwrap();

    let mut expected: Vec<_> = (0..16).map(|share| ("submit", 0, 3, share)).collect();
    expected.push(("submit", 0, 5, 0));
    assert_eq!(dc_steps(&plan.next_steps), expected);
    let obligations = classify_round(&snapshot, &[1]).unwrap().obligations;
    for step in &plan.next_steps {
        assert!(
            matches!(
                resolve_step(&obligations, step),
                Some(Obligation::Deliver {
                    commitment: CommitmentRef::Proxy { .. },
                    ..
                })
            ),
            "{step:?} resolves to its DC's delivery"
        );
    }
    let Some(Obligation::Deliver {
        vc_tree_position, ..
    }) = resolve_step(&obligations, &plan.next_steps[0])
    else {
        unreachable!("checked above");
    };
    assert_eq!(*vc_tree_position, 103, "the DC's own leaf");
}

#[test]
fn dc_shares_are_owed_only_once_their_batch_landed() {
    for progress in [
        ProxyBatchProgress::Proved,
        ProxyBatchProgress::InFlight { refused: false },
    ] {
        let snapshot = snapshot()
            .bundle(0, DelegationPhase::Confirmed)
            .proxy_batch(0, progress)
            .landed_dc(0, 0, DcShareLayout::Standard, None)
            .build();

        let plan = plan_from_snapshot(&snapshot, &[1]).unwrap();

        assert!(dc_steps(&plan.next_steps).is_empty(), "{progress:?}");
    }
}

#[test]
fn only_shares_not_handed_off_are_owed() {
    let mut shares: Vec<_> = (0..16).map(|index| share(index, 2, 500)).collect();
    shares[4] = share(4, 1, 500);
    shares[9] = DcShareRecord {
        confirmed: true,
        ..share(9, 0, 500)
    };
    shares[11] = DcShareRecord {
        ambiguous_urls: vec!["https://helper-7.example".to_string()],
        ..share(11, 0, 500)
    };
    // Two acceptances no pass recorded as a hand-off: those helpers may have
    // left the fleet since, so they hand nothing off.
    shares[13] = DcShareRecord {
        handed_off_to_reachable_target: false,
        ..share(13, 2, 500)
    };
    let snapshot = snapshot()
        .bundle(0, DelegationPhase::Confirmed)
        .proxy_batch(0, ProxyBatchProgress::Landed)
        .landed_dc(0, 2, DcShareLayout::Standard, Some(shares))
        .build();

    let plan = plan_from_snapshot(&snapshot, &[1]).unwrap();

    let mut expected = vec![
        ("submit", 0, 2, 4),
        ("submit", 0, 2, 11),
        ("submit", 0, 2, 13),
    ];
    expected.extend([0, 1, 2, 3, 5, 6, 7, 8, 10, 12, 14, 15].map(|share| ("confirm", 0, 2, share)));
    assert_eq!(
        dc_steps(&plan.next_steps),
        expected,
        "a confirmed share is handed off whatever its helpers say, and tracked no more"
    );
    assert!(plan.blocking_recovery);
}

#[test]
fn no_dc_owes_delivery_once_the_vote_ended() {
    let mut short: Vec<_> = (0..16).map(|index| share(index, 2, 0)).collect();
    short[3] = share(3, 0, 0);
    let handed_off: Vec<_> = (0..16).map(|index| share(index, 2, 0)).collect();
    let snapshot = snapshot()
        .bundle(0, DelegationPhase::Confirmed)
        .proxy_batch(0, ProxyBatchProgress::Landed)
        .landed_dc(0, 0, DcShareLayout::SingleShare, None)
        .landed_dc(0, 1, DcShareLayout::Standard, Some(short))
        .landed_dc(0, 2, DcShareLayout::Standard, Some(handed_off))
        .vote_ended()
        .build();

    let plan = plan_from_snapshot(&snapshot, &[1]).unwrap();

    let steps = dc_steps(&plan.next_steps);
    assert!(
        steps.iter().all(|(kind, ..)| *kind == "confirm"),
        "an unplanned or short DC owes no delivery"
    );
    assert_eq!(
        steps.len(),
        15 + 16,
        "every handed-off share is still tracked"
    );
}

/// A handed-off share, planned at once or on the delayed schedule, awaits
/// its reveal as an accepted vote share does: background tracking owns it,
/// and the host learns from the plan to start tracking.
#[test]
fn a_handed_off_dc_s_unconfirmed_shares_are_left_to_background_tracking() {
    for at in [0, 700] {
        let mut shares: Vec<_> = (0..16).map(|index| share(index, 2, at)).collect();
        shares[0].confirmed = true;
        let tracked = snapshot()
            .bundle(0, DelegationPhase::Confirmed)
            .proxy_batch(0, ProxyBatchProgress::Landed)
            .landed_dc(0, 1, DcShareLayout::Standard, Some(shares.clone()))
            .build();

        let plan = plan_from_snapshot(&tracked, &[1]).unwrap();

        assert_eq!(
            dc_steps(&plan.next_steps),
            (1..16)
                .map(|share| ("confirm", 0, 1, share))
                .collect::<Vec<_>>(),
            "{at}"
        );
        assert!(!plan.blocking_recovery, "{at}");
        assert_eq!(plan.primary_action, RoundPlanAction::Idle, "{at}");
        assert!(plan.has_unconfirmed_shares, "{at}");
        let obligations = classify_round(&tracked, &[1]).unwrap().obligations;
        assert!(matches!(
            resolve_step(&obligations, &plan.next_steps[0]),
            Some(Obligation::Confirm {
                accepted: true,
                outcome_unknown: false,
                ..
            })
        ));

        for share in &mut shares {
            share.confirmed = true;
        }
        let revealed = snapshot()
            .bundle(0, DelegationPhase::Confirmed)
            .proxy_batch(0, ProxyBatchProgress::Landed)
            .landed_dc(0, 1, DcShareLayout::Standard, Some(shares))
            .build();
        let plan = plan_from_snapshot(&revealed, &[1]).unwrap();
        assert!(dc_steps(&plan.next_steps).is_empty(), "{at}");
        assert!(!plan.has_unconfirmed_shares, "{at}");
    }
}

#[test]
fn no_dc_share_is_the_round_s_immediate_share() {
    let snapshot = snapshot()
        .bundle(0, DelegationPhase::Confirmed)
        .proxy_batch(0, ProxyBatchProgress::Landed)
        .landed_dc(0, 0, DcShareLayout::SingleShare, Some(vec![share(0, 2, 0)]))
        .build();

    let plan = plan_from_snapshot(&snapshot, &[1]).unwrap();

    assert_eq!(plan.immediate_share_key, None);
    assert!(!plan.immediate_share_confirmed);
}

#[test]
fn dc_share_steps_rank_ahead_of_every_proposal_s_shares_slot_by_slot() {
    let snapshot = snapshot()
        .bundle(0, DelegationPhase::Confirmed)
        .proxy_batch(0, ProxyBatchProgress::Landed)
        .landed_dc(0, 4, DcShareLayout::SingleShare, None)
        .landed_dc(0, 2, DcShareLayout::SingleShare, None)
        .build();

    let plan = plan_from_snapshot(&snapshot, &[1]).unwrap();

    assert_eq!(
        dc_steps(&plan.next_steps),
        vec![("submit", 0, 2, 0), ("submit", 0, 4, 0)]
    );
    assert!(
        plan.recovered_vote_work.is_empty(),
        "a DC's shares are not a vote to recover"
    );
}
