//! When a committed allocation's job is complete and when it still owes
//! work (`ProxyDelegationProgress::is_complete` and `owes_work`).

use crate::wire::{
    DelegateBallotView, ProxyDelegateProvider, ProxyDelegationProgress, ProxyDelegationStage,
    ProxyDelegationView,
};

use ProxyDelegationStage::{HandedOff, Included, NotCounted, Preparing, Submitting};

/// Progress with one delegation per stage in `stages`, and the immediate
/// share planned and confirmed as given.
fn progress(
    stages: &[ProxyDelegationStage],
    planned_immediate: bool,
    immediate_reveal_confirmed: bool,
) -> ProxyDelegationProgress {
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
        planned_immediate,
        immediate_reveal_confirmed,
        refused_by_chain: false,
    }
}

#[test]
fn every_delegation_handed_off_completes_the_job() {
    for stages in [&[HandedOff][..], &[HandedOff, HandedOff]] {
        let progress = progress(stages, false, false);
        assert!(progress.is_complete(), "{stages:?}");
        assert!(!progress.owes_work(), "{stages:?}");
    }
}

#[test]
fn a_planned_immediate_share_completes_only_on_its_confirmed_reveal() {
    let unconfirmed = progress(&[HandedOff], true, false);
    assert!(!unconfirmed.is_complete());
    assert!(unconfirmed.owes_work());

    let confirmed = progress(&[HandedOff], true, true);
    assert!(confirmed.is_complete());
    assert!(!confirmed.owes_work());
}

#[test]
fn a_delegation_short_of_hand_off_owes_work() {
    for stage in [Preparing, Submitting, Included, NotCounted] {
        let progress = progress(&[HandedOff, stage], false, false);
        assert!(!progress.is_complete(), "{stage:?}");
        assert!(progress.owes_work(), "{stage:?}");
    }
}
