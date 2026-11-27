//! A delegation's stage follows its batch, then its shares: `Preparing`,
//! `Submitting`, `Included` or `HandedOff`. Once the round closes anything
//! short of a hand-off is `NotCounted`, including a batch that never landed.

use crate::{
    proxy::{
        batch::ProxyBatchProgress,
        delegator::{dc_stage, delegation_stage},
        shares::DcDeliveryProgress,
        ProxyRoundPhase,
    },
    wire::ProxyDelegationStage,
};

use ProxyDelegationStage::{HandedOff, Included, NotCounted, Preparing, Submitting};

/// Every batch progress, each with the delivery a DC of it can have.
fn every_dc() -> Vec<(ProxyBatchProgress, Option<DcDeliveryProgress>)> {
    let mut dcs = vec![
        (ProxyBatchProgress::Planned, None),
        (ProxyBatchProgress::Proved, None),
        (ProxyBatchProgress::InFlight { refused: false }, None),
        (ProxyBatchProgress::InFlight { refused: true }, None),
        (ProxyBatchProgress::Landed, None),
    ];
    dcs.extend(
        [
            DcDeliveryProgress::Unplanned,
            DcDeliveryProgress::Delivering,
            DcDeliveryProgress::Complete,
        ]
        .map(|delivery| (ProxyBatchProgress::Landed, Some(delivery))),
    );
    dcs
}

#[test]
fn a_dc_s_stage_follows_its_batch_then_its_shares() {
    let active: Vec<_> = every_dc()
        .into_iter()
        .map(|(batch, delivery)| dc_stage(batch, delivery, ProxyRoundPhase::Active))
        .collect();
    assert_eq!(
        active,
        [Preparing, Preparing, Submitting, Submitting, Included, Included, Included, HandedOff,],
        "a DC is handed off once every share is, whatever its reveals"
    );
}

#[test]
fn once_the_round_closes_only_a_handed_off_dc_is_counted() {
    let closed: Vec<_> = every_dc()
        .into_iter()
        .map(|(batch, delivery)| dc_stage(batch, delivery, ProxyRoundPhase::Closed))
        .collect();
    assert_eq!(
        closed,
        [
            NotCounted, NotCounted, NotCounted, NotCounted, NotCounted, NotCounted, NotCounted,
            HandedOff,
        ],
        "a handed-off DC stays handed off, whatever its reveals"
    );
}

#[test]
fn a_delegation_is_as_far_as_its_least_advanced_dc() {
    for (stages, expected) in [
        (vec![HandedOff], HandedOff),
        (vec![HandedOff, Included], Included),
        (vec![Submitting, Preparing], Preparing),
        (vec![HandedOff, NotCounted], NotCounted),
    ] {
        assert_eq!(delegation_stage(stages.clone()), expected, "{stages:?}");
    }
}
