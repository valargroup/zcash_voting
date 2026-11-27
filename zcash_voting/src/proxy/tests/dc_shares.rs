//! A landed DC's shares: planned from the layout its slot committed, never
//! as the round's immediate share, and derived again from the round hotkey,
//! identically after a restart.

use super::submission_fixtures::{
    proved_batch_with, FixtureAnchor, ProvedBatchFixture, HOTKEY_SECRET, ROUND_ID, WALLET_ID,
};
use crate::{
    helper::client::HelperFleetPreflight,
    proxy::{
        batch::record_dc_landings,
        shares::{derive_dc_share_payloads, load_landed_dc, plan_dc_shares},
        DcShareLayout, POOL_PROPOSAL_ID,
    },
    share::compute_nullifier,
    share_policy::{
        last_moment_buffer_seconds, share_submission_target_count,
        SHARE_SUBMIT_AT_MAX_DELAY_SECONDS,
    },
    storage::VotingDb,
    wire::VoteShareWire,
};

const CEREMONY_START: u64 = 1_000;
const VOTE_END: u64 = 1_000_000;

/// A fleet of `count` helpers, all ready.
fn fleet(count: usize) -> HelperFleetPreflight {
    let urls: Vec<String> = (1..=count)
        .map(|index| format!("https://helper-{index}.example"))
        .collect();
    HelperFleetPreflight::from_readiness(&urls, &urls).unwrap()
}

/// The fixture batch with its two DCs landed at positions 6 and 7.
fn landed(layout: DcShareLayout) -> ProvedBatchFixture {
    let fixture = proved_batch_with(FixtureAnchor::Registration, layout);
    fixture
        .db
        .write_transaction("land the fixture DCs", |tx| {
            record_dc_landings(tx, ROUND_ID, WALLET_ID, 0, &[(0, 6), (1, 7)])
        })
        .unwrap();
    fixture
}

#[test]
fn a_single_share_dc_plans_share_zero_alone_at_once_like_a_vote_share() {
    // 15 helpers is where the vote-share cap of 5 differs from half the fleet.
    for helpers in [1, 2, 5, 9, 15] {
        let plans = plan_dc_shares(
            DcShareLayout::SingleShare,
            &fleet(helpers),
            CEREMONY_START + 10,
            VOTE_END,
            last_moment_buffer_seconds(CEREMONY_START, VOTE_END),
        )
        .unwrap();

        assert_eq!(plans.len(), 1, "share 0 alone");
        assert_eq!(plans[0].submit_at, 0, "at once, even before the window");
        assert!(!plans[0].immediate, "never the round's immediate share");
        let target = share_submission_target_count(helpers);
        assert_eq!(plans[0].target_count as usize, target);
        assert_eq!(plans[0].target_servers.len(), target);
    }
}

#[test]
fn a_standard_dc_plans_sixteen_shares_on_the_standard_schedule() {
    let now = CEREMONY_START + 10;
    let buffer = last_moment_buffer_seconds(CEREMONY_START, VOTE_END).unwrap();
    let plans = plan_dc_shares(
        DcShareLayout::Standard,
        &fleet(5),
        now,
        VOTE_END,
        Some(buffer),
    )
    .unwrap();

    assert_eq!(plans.len(), 16);
    let deadline = (VOTE_END - buffer).min(now + SHARE_SUBMIT_AT_MAX_DELAY_SECONDS);
    for plan in &plans {
        assert!(
            !plan.immediate,
            "no DC share is the round's immediate share"
        );
        assert!(
            (now..deadline).contains(&plan.submit_at),
            "a delayed submit_at before the last-moment window"
        );
        assert_eq!(plan.target_count, 3);
        assert_eq!(plan.target_servers.len(), 3);
    }
}

#[test]
fn a_standard_dc_planned_inside_the_window_submits_every_share_at_once() {
    let buffer = last_moment_buffer_seconds(CEREMONY_START, VOTE_END).unwrap();
    let plans = plan_dc_shares(
        DcShareLayout::Standard,
        &fleet(3),
        VOTE_END - buffer + 1,
        VOTE_END,
        Some(buffer),
    )
    .unwrap();

    assert_eq!(plans.len(), 16);
    assert!(plans
        .iter()
        .all(|plan| plan.submit_at == 0 && !plan.immediate));
}

#[test]
fn a_landed_dc_s_payloads_reveal_into_its_delegate_s_pool() {
    let fixture = landed(DcShareLayout::Standard);
    for (dc_slot, built) in fixture.batch.dcs.iter().enumerate() {
        let dc = fixture
            .db
            .read_transaction("load", |tx| {
                load_landed_dc(tx, ROUND_ID, WALLET_ID, 0, dc_slot as u8)
            })
            .unwrap();

        let derived = derive_dc_share_payloads(&dc, &HOTKEY_SECRET).unwrap();

        assert_eq!(derived.payloads.len(), 16);
        for (share_index, (payload, nullifier)) in
            derived.payloads.iter().zip(&derived.nullifiers).enumerate()
        {
            assert_eq!(payload.proposal_id, POOL_PROPOSAL_ID);
            assert_eq!(payload.vote_decision, built.delegate_index);
            assert_eq!(payload.tree_position, 6 + dc_slot as u64);
            assert_eq!(payload.enc_share.share_index, share_index as u32);
            let blind: [u8; 32] = payload.primary_blind.clone().try_into().unwrap();
            assert_eq!(
                *nullifier,
                compute_nullifier(&built.delegation_commitment, share_index as u32, &blind)
                    .unwrap()
            );
            let wire = VoteShareWire::from_json(
                &payload.to_wire_json(Some(6 + dc_slot as u64), 0).unwrap(),
            )
            .unwrap();
            assert_eq!(
                (wire.proposal_id, wire.vote_decision),
                (0, built.delegate_index)
            );
        }
    }
}

#[test]
fn a_pool_share_names_a_delegate_with_no_option_bound() {
    let fixture = landed(DcShareLayout::SingleShare);
    let dc = fixture
        .db
        .read_transaction("load", |tx| load_landed_dc(tx, ROUND_ID, WALLET_ID, 0, 0))
        .unwrap();
    let json = derive_dc_share_payloads(&dc, &HOTKEY_SECRET)
        .unwrap()
        .payloads[0]
        .to_wire_json(None, 0)
        .unwrap();
    let mut body: serde_json::Value = serde_json::from_str(&json).unwrap();

    body["vote_decision"] = 1_000_000.into();
    assert!(
        VoteShareWire::from_json(&body.to_string()).is_ok(),
        "a delegate index has no option bound"
    );
    body["vote_decision"] = 0.into();
    assert!(
        VoteShareWire::from_json(&body.to_string()).is_err(),
        "delegate 0 does not exist"
    );
    body["proposal_id"] = 1.into();
    body["vote_decision"] = 8.into();
    assert!(
        VoteShareWire::from_json(&body.to_string()).is_err(),
        "a vote share keeps its option bound"
    );
}

#[test]
fn a_single_share_dc_derives_share_zero_alone() {
    let fixture = landed(DcShareLayout::SingleShare);
    let dc = fixture
        .db
        .read_transaction("load", |tx| load_landed_dc(tx, ROUND_ID, WALLET_ID, 0, 0))
        .unwrap();

    let derived = derive_dc_share_payloads(&dc, &HOTKEY_SECRET).unwrap();

    assert_eq!(derived.payloads.len(), 1);
    assert_eq!(derived.payloads[0].enc_share.share_index, 0);
    assert_eq!(derived.nullifiers.len(), 1);
}

#[test]
fn shares_are_derived_again_identically_after_a_restart() {
    let path = std::env::temp_dir().join(format!(
        "zcash-voting-dc-shares-{}-{}.sqlite",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let path = path.to_str().unwrap().to_string();
    let fixture = landed(DcShareLayout::Standard);
    fixture
        .db
        .conn()
        .execute("VACUUM INTO ?1", [&path])
        .unwrap();
    let before = {
        let dc = fixture
            .db
            .read_transaction("load", |tx| load_landed_dc(tx, ROUND_ID, WALLET_ID, 0, 1))
            .unwrap();
        derive_dc_share_payloads(&dc, &HOTKEY_SECRET).unwrap()
    };
    drop(fixture);

    let reopened = VotingDb::open(&path).unwrap();
    reopened.set_wallet_id(WALLET_ID);
    let dc = reopened
        .read_transaction("load", |tx| load_landed_dc(tx, ROUND_ID, WALLET_ID, 0, 1))
        .unwrap();
    let after = derive_dc_share_payloads(&dc, &HOTKEY_SECRET).unwrap();
    std::fs::remove_file(&path).ok();

    assert_eq!(after.nullifiers, before.nullifiers);
    for (after, before) in after.payloads.iter().zip(&before.payloads) {
        assert_eq!(
            after.to_wire_json(None, 0).unwrap(),
            before.to_wire_json(None, 0).unwrap()
        );
    }
}

#[test]
fn another_hotkey_does_not_derive_the_landed_dc() {
    let fixture = landed(DcShareLayout::Standard);
    let dc = fixture
        .db
        .read_transaction("load", |tx| load_landed_dc(tx, ROUND_ID, WALLET_ID, 0, 0))
        .unwrap();

    let error = derive_dc_share_payloads(&dc, &[0x22; 64])
        .err()
        .expect("another hotkey derives other secrets");

    assert!(error.to_string().contains("does not derive the landed DC"));
}

#[test]
fn a_dc_whose_batch_has_not_landed_has_no_payloads() {
    let fixture = proved_batch_with(FixtureAnchor::Registration, DcShareLayout::Standard);

    let error = fixture
        .db
        .read_transaction("load", |tx| load_landed_dc(tx, ROUND_ID, WALLET_ID, 0, 0))
        .unwrap_err();

    assert!(error.to_string().contains("has not landed"));
}
