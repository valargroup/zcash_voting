//! When a committed allocation's job is complete and when it still owes
//! work (`ProxyDelegationProgress::is_complete` and `owes_work`). The
//! immediate share's reveal gating the job is pinned by
//! `a_dc_planned_at_once_is_handed_off_before_its_reveal_which_completes_the_job`.

use crate::wire::{
    DelegateBallotView, ProxyDelegateProvider, ProxyDelegationProgress, ProxyDelegationStage,
    ProxyDelegationView,
};

use ProxyDelegationStage::{HandedOff, Included, NotCounted, Preparing, Submitting};

/// Progress with one delegation per stage in `stages`, and no immediate
/// share planned.
fn progress(stages: &[ProxyDelegationStage]) -> ProxyDelegationProgress {
    ProxyDelegationProgress {
        delegations: stages
            .iter()
            .zip(1..)
            .map(|(&stage, delegate_index)| ProxyDelegationView {
                delegate_index,
                provider: ProxyDelegateProvider::X,
                handle: "alice".to_owned(),
                ballots: 1,
                stage,
                ballot: DelegateBallotView::NoBallotYet,
            })
            .collect(),
        broadcast_started: false,
        planned_immediate: false,
        immediate_reveal_confirmed: false,
        refused_by_chain: false,
    }
}

#[test]
fn every_delegation_handed_off_completes_the_job() {
    let progress = progress(&[HandedOff, HandedOff]);
    assert!(progress.is_complete());
    assert!(!progress.owes_work());
}

#[test]
fn a_delegation_short_of_hand_off_owes_work() {
    for stage in [Preparing, Submitting, Included, NotCounted] {
        let progress = progress(&[HandedOff, stage]);
        assert!(!progress.is_complete(), "{stage:?}");
        assert!(progress.owes_work(), "{stage:?}");
    }
}
