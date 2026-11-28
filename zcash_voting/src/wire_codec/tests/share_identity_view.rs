//! A helper share names its commitment: a proposal's vote, or a delegation
//! commitment's slot. The views keep a vote share exactly as hosts read it
//! before DCs existed and add the slot only for a DC share.

use crate::session::NextStep;
use crate::share_tracking::{CommitmentRef, ShareKey};
use crate::wire::{NextStepKind, NextStepView, ShareKeyView};

#[test]
fn a_vote_share_step_serializes_as_it_did_before_commitments() {
    let step = NextStep::SubmitShares {
        bundle_index: 2,
        commitment: CommitmentRef::Vote { proposal_id: 3 },
        share_index: 4,
    };

    let json = serde_json::to_value(&step).unwrap();

    assert_eq!(
        json,
        serde_json::json!({
            "kind": "submit_shares", "bundle_index": 2, "proposal_id": 3, "share_index": 4,
        })
    );
    assert_eq!(serde_json::from_value::<NextStep>(json).unwrap(), step);
}

#[test]
fn a_dc_share_step_names_its_slot_and_no_proposal() {
    let step = NextStep::ConfirmShare {
        bundle_index: 2,
        commitment: CommitmentRef::Proxy { dc_slot: 5 },
        share_index: 0,
    };

    let json = serde_json::to_value(&step).unwrap();

    assert_eq!(
        json,
        serde_json::json!({
            "kind": "confirm_share", "bundle_index": 2, "dc_slot": 5, "share_index": 0,
        })
    );
    assert_eq!(serde_json::from_value::<NextStep>(json).unwrap(), step);
}

#[test]
fn share_step_views_round_trip_both_commitments() {
    let vote = NextStep::SubmitShares {
        bundle_index: 1,
        commitment: CommitmentRef::Vote { proposal_id: 9 },
        share_index: 15,
    };
    let dc = NextStep::SubmitShares {
        bundle_index: 1,
        commitment: CommitmentRef::Proxy { dc_slot: 7 },
        share_index: 15,
    };

    let vote_view = NextStepView::try_from(vote.clone()).unwrap();
    let dc_view = NextStepView::try_from(dc.clone()).unwrap();

    assert_eq!(vote_view.kind, NextStepKind::SubmitShares);
    assert_eq!((vote_view.proposal_id, vote_view.dc_slot), (9, None));
    assert_eq!(
        (dc_view.proposal_id, dc_view.dc_slot),
        (crate::proxy::POOL_PROPOSAL_ID, Some(7))
    );
    assert_eq!(NextStep::from(vote_view.clone()), vote);
    assert_eq!(NextStep::from(dc_view), dc);
    assert!(
        serde_json::to_value(&vote_view)
            .unwrap()
            .get("dc_slot")
            .is_none(),
        "a vote step's view carries no slot"
    );
}

#[test]
fn share_key_views_name_the_commitment() {
    let vote = ShareKeyView::from(ShareKey::vote(3, 4, 5));
    let dc = ShareKeyView::from(ShareKey::proxy(3, 6, 0));

    assert_eq!((vote.proposal_id, vote.dc_slot), (4, None));
    assert_eq!(
        (dc.proposal_id, dc.dc_slot),
        (crate::proxy::POOL_PROPOSAL_ID, Some(6))
    );
    assert_eq!(
        serde_json::to_value(vote).unwrap(),
        serde_json::json!({ "bundle_index": 3, "proposal_id": 4, "share_index": 5 })
    );
    assert_eq!(
        serde_json::from_value::<ShareKeyView>(serde_json::to_value(dc).unwrap()).unwrap(),
        dc
    );
}
