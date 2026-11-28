//! The delegate's status and round views from fixture entries: the device's
//! key and its proven entry, whether it can sign a round's ballot, the
//! published ballot and the pool count.

use zcash_vote_delegate::{DelegateStatus, IdentityProvider};

use super::fixtures::{
    alice_attestation, alice_entry, alice_key, chain, db, proven_entry, roster, round_timing,
    signing_key, verifiers, ALICE_PROOF_ID, ALICE_SECRET, ATTEST_NOW, DELEGATE_INDEX,
};
use crate::{
    proxy::delegate::{
        delegate_key_info, delegate_mode_status, delegate_round, prepare_register_op,
        record_delegate_key, record_proven_entry, sign_ballot, AttestationExpectation,
        ChainDelegateBallot, CompleteBallot,
    },
    wire::{
        DelegateBallotBlock, DelegateBallotView, DelegateEntryStatus, DelegateEntryView,
        DelegateModeStatus, ProxyDelegateProvider,
    },
};

/// The round's ballot deadline.
const VOTE_END_TIME: u64 = 1_791_000_000;
/// A device time while the round runs.
const NOW: u64 = VOTE_END_TIME - 3_600;

/// The status follows the device's key through its life: none, held,
/// registration pending, then its proven entry with the chain's handle,
/// proof id and status.
#[test]
fn status_shows_the_device_key_and_its_proven_entry() {
    let db = db();
    let chain = chain();
    assert_eq!(delegate_mode_status(&db, &chain, None).unwrap(), None);

    record_delegate_key(&db, &chain, &alice_key(), 10).unwrap();
    let held = DelegateModeStatus {
        key: delegate_key_info(&alice_key()),
        entry: None,
        registration_pending: false,
    };
    assert_eq!(
        delegate_mode_status(&db, &chain, None).unwrap(),
        Some(held.clone())
    );

    let set = verifiers(1);
    prepare_register_op(
        &db,
        &chain,
        &alice_attestation(),
        &AttestationExpectation {
            provider: IdentityProvider::X,
            verifiers: Some(&set),
        },
        &signing_key(ALICE_SECRET),
        ATTEST_NOW,
    )
    .unwrap();
    assert_eq!(
        delegate_mode_status(&db, &chain, None).unwrap(),
        Some(DelegateModeStatus {
            registration_pending: true,
            ..held.clone()
        })
    );

    record_proven_entry(&db, &chain, &alice_entry()).unwrap();
    let suspended = proven_entry(alice_key(), DelegateStatus::Suspended);
    assert_eq!(
        delegate_mode_status(&db, &chain, Some(&suspended)).unwrap(),
        Some(DelegateModeStatus {
            entry: Some(DelegateEntryView {
                delegate_index: DELEGATE_INDEX,
                provider: ProxyDelegateProvider::X,
                handle: "alice".to_owned(),
                proof_id: ALICE_PROOF_ID.to_owned(),
                status: DelegateEntryStatus::Suspended,
                status_reason: 4,
            }),
            ..held
        })
    );
}

/// A round says the device cannot sign its ballot without an entry. An
/// entry's own blocks are `sign_ballot`'s, in `ballot.rs`.
#[test]
fn round_views_show_what_blocks_the_ballot() {
    let db = db();
    let chain = chain();
    let round = round_timing([2; 32], VOTE_END_TIME);
    let block = |status: Option<DelegateStatus>| {
        let entry = status.map(|status| proven_entry(alice_key(), status));
        delegate_round(&db, &chain, &round, entry.as_ref(), None, None, NOW)
            .unwrap()
            .ballot_block
    };
    assert_eq!(block(None), Some(DelegateBallotBlock::NotRegistered));
    assert_eq!(block(Some(DelegateStatus::Active)), None);
}

/// A round shows the ballot as published, submitted while it runs and
/// counted once it closes, the device's signed ballot until the list shows
/// one, and R from the pool's share count.
#[test]
fn round_views_show_the_ballot_and_the_pool_count() {
    let db = db();
    let chain = chain();
    let round = round_timing([2; 32], VOTE_END_TIME);
    let entry = alice_entry();
    let listed = ChainDelegateBallot {
        delegate_index: DELEGATE_INDEX,
        options: vec![1, 0],
        height: 9,
    };
    let view = |chain_ballot: Option<&ChainDelegateBallot>, count: Option<&[u8]>, now: u64| {
        delegate_round(&db, &chain, &round, Some(&entry), chain_ballot, count, now)
    };

    let open = view(None, None, NOW).unwrap();
    assert_eq!(open.round_id, "02".repeat(32));
    assert_eq!(open.vote_end_time, VOTE_END_TIME);
    assert_eq!(open.revealed_shares, 0);
    assert_eq!(open.ballot, DelegateBallotView::NoBallotYet);
    assert_eq!(open.stored_ballot, None);

    let ballot = CompleteBallot::new([2; 32], &roster(2), vec![1, 0]).unwrap();
    sign_ballot(
        &db,
        &chain,
        &round,
        &entry,
        &ballot,
        &signing_key(ALICE_SECRET),
        NOW,
    )
    .unwrap();
    let signed = view(None, Some(&37u64.to_be_bytes()), NOW).unwrap();
    assert_eq!(signed.stored_ballot, Some(vec![1, 0]));
    assert_eq!(signed.revealed_shares, 37);

    let landed = view(Some(&listed), None, NOW).unwrap();
    assert_eq!(
        landed.ballot,
        DelegateBallotView::Submitted {
            options: vec![1, 0]
        }
    );
    assert_eq!(landed.stored_ballot, None);
    assert_eq!(
        view(Some(&listed), None, VOTE_END_TIME).unwrap().ballot,
        DelegateBallotView::Counted {
            options: vec![1, 0]
        }
    );
    assert_eq!(
        view(None, None, VOTE_END_TIME).unwrap().ballot,
        DelegateBallotView::NotCounted
    );
}
