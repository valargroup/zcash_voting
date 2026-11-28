//! Signing a proved batch's attempt: D1 under every DC's stored randomizer,
//! persisted with its expiry, and re-signed only once that expiry passed.

use zcash_vote_delegate::{proxy_batch_digest, proxy_batch_sighash};

use super::submission_fixtures::{
    proved_batch, FixtureAnchor, ProvedBatchFixture, CHAIN_ANCHOR_HEIGHT, CHAIN_ID, HOTKEY_SECRET,
    NETWORK, ROUND_ID, WALLET_ID,
};
use crate::{
    proxy::{
        batch::{
            batch_fields, load_signed_attempt, sign_batch_attempt, verify_dc_signature,
            AttemptSigningRequest, SignedBatchAttempt,
        },
        PROXY_BATCH_EXPIRY_BLOCKS,
    },
    storage::VotingDb,
    types::VotingError,
};

/// A request to sign bundle 0's attempt at `chain_height`.
fn request(
    registration_signature: Option<[u8; 64]>,
    chain_height: u64,
) -> AttemptSigningRequest<'static> {
    AttemptSigningRequest {
        round_id: ROUND_ID,
        bundle_index: 0,
        hotkey_seed: &HOTKEY_SECRET,
        network: NETWORK,
        chain_id: CHAIN_ID,
        registration_signature,
        chain_height,
        stored_attempt_expired: false,
    }
}

/// The stored attempt of bundle 0.
fn stored_attempt(db: &VotingDb) -> Option<SignedBatchAttempt> {
    load_signed_attempt(&db.conn(), ROUND_ID, WALLET_ID, 0).unwrap()
}

/// The registration VAN the fixture binds, read back from the bundle.
fn registration_van(db: &VotingDb) -> [u8; 32] {
    db.conn()
        .query_row(
            "SELECT gov_comm FROM bundles WHERE round_id = ?1 AND bundle_index = 0",
            [ROUND_ID],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .unwrap()
        .try_into()
        .unwrap()
}

fn assert_invalid_input(result: Result<impl std::fmt::Debug, VotingError>, needle: &str) {
    match result {
        Err(VotingError::InvalidInput { message }) => {
            assert!(message.contains(needle), "{message}")
        }
        other => panic!("expected an invalid-input refusal naming {needle:?}, got {other:?}"),
    }
}

#[test]
fn the_first_attempt_binds_the_registration_and_persists_every_signature() {
    let ProvedBatchFixture {
        db,
        batch,
        registration_signature,
    } = proved_batch(FixtureAnchor::Registration);

    let signed = sign_batch_attempt(&db, request(Some(registration_signature), 1_000)).unwrap();

    assert!(signed.signed_now);
    let attempt = signed.attempt;
    assert_eq!(attempt.expiry_height, 1_000 + PROXY_BATCH_EXPIRY_BLOCKS);
    assert_eq!(attempt.registration_signature, Some(registration_signature));
    assert_eq!(attempt.chain_id, CHAIN_ID);
    let round = <[u8; 32]>::try_from(hex::decode(ROUND_ID).unwrap()).unwrap();
    let fields = batch_fields(
        round,
        &batch,
        Some(registration_van(&db)),
        attempt.expiry_height,
    );
    assert_eq!(
        attempt.batch_digest,
        proxy_batch_digest(CHAIN_ID, &fields).unwrap()
    );
    let d1 = proxy_batch_sighash(CHAIN_ID, &fields).unwrap();
    assert_eq!(attempt.dc_signatures.len(), batch.dcs.len());
    for (dc, signature) in batch.dcs.iter().zip(&attempt.dc_signatures) {
        verify_dc_signature(&dc.r_vpk, &d1, signature).unwrap();
    }
    assert_eq!(stored_attempt(&db), Some(attempt));
}

#[test]
fn a_chain_anchored_attempt_binds_no_registration() {
    let ProvedBatchFixture { db, batch, .. } = proved_batch(FixtureAnchor::Chain);

    let attempt = sign_batch_attempt(&db, request(None, 2_000))
        .unwrap()
        .attempt;

    assert_eq!(attempt.registration_signature, None);
    let round = <[u8; 32]>::try_from(hex::decode(ROUND_ID).unwrap()).unwrap();
    let fields = batch_fields(round, &batch, None, attempt.expiry_height);
    assert_eq!(
        fields.vote_comm_tree_anchor_height,
        u64::from(CHAIN_ANCHOR_HEIGHT)
    );
    let d1 = proxy_batch_sighash(CHAIN_ID, &fields).unwrap();
    for (dc, signature) in batch.dcs.iter().zip(&attempt.dc_signatures) {
        verify_dc_signature(&dc.r_vpk, &d1, signature).unwrap();
    }
}

#[test]
fn a_live_attempt_is_kept_until_its_expiry_passes() {
    let fixture = proved_batch(FixtureAnchor::Registration);
    let db = &fixture.db;
    let first = sign_batch_attempt(db, request(Some(fixture.registration_signature), 1_000))
        .unwrap()
        .attempt;

    // Below the expiry the stored attempt is the live one, and no new
    // registration signature is needed or read.
    let kept = sign_batch_attempt(db, request(None, first.expiry_height - 1)).unwrap();
    assert!(!kept.signed_now);
    assert_eq!(kept.attempt, first);

    // At the expiry the chain refuses it, so the same batch is re-signed with
    // a later expiry and the device's registration signature is kept.
    let resigned = sign_batch_attempt(db, request(None, first.expiry_height)).unwrap();
    assert!(resigned.signed_now);
    let second = resigned.attempt;
    assert_eq!(
        second.expiry_height,
        first.expiry_height + PROXY_BATCH_EXPIRY_BLOCKS
    );
    assert_eq!(second.batch_digest, first.batch_digest);
    assert_eq!(second.registration_signature, first.registration_signature);
    assert_ne!(second.dc_signatures, first.dc_signatures);
    assert_eq!(stored_attempt(db), Some(second));
}

#[test]
fn an_expired_answer_re_signs_past_the_stored_expiry_even_from_a_lagging_height() {
    let fixture = proved_batch(FixtureAnchor::Registration);
    let db = &fixture.db;
    let first = sign_batch_attempt(db, request(Some(fixture.registration_signature), 1_000))
        .unwrap()
        .attempt;

    let resigned = sign_batch_attempt(
        db,
        AttemptSigningRequest {
            stored_attempt_expired: true,
            ..request(None, 1_200)
        },
    )
    .unwrap();

    assert!(resigned.signed_now);
    assert_eq!(
        resigned.attempt.expiry_height,
        first.expiry_height + PROXY_BATCH_EXPIRY_BLOCKS
    );
}

#[test]
fn a_registration_batch_needs_a_valid_registration_signature() {
    let fixture = proved_batch(FixtureAnchor::Registration);
    let db = &fixture.db;

    assert_invalid_input(
        sign_batch_attempt(db, request(None, 1_000)),
        "needs the registration's SpendAuth signature",
    );
    let mut forged = fixture.registration_signature;
    forged[0] ^= 1;
    assert!(sign_batch_attempt(db, request(Some(forged), 1_000)).is_err());
    assert_eq!(stored_attempt(db), None);
}

#[test]
fn an_attempt_is_not_signed_while_a_submission_may_be_on_the_wire() {
    let fixture = proved_batch(FixtureAnchor::Registration);
    let db = &fixture.db;
    let first = sign_batch_attempt(db, request(Some(fixture.registration_signature), 1_000))
        .unwrap()
        .attempt;
    insert_lifecycle_row(db, &first.batch_digest);

    assert_invalid_input(
        sign_batch_attempt(db, request(None, first.expiry_height + 10)),
        "may be on the wire",
    );
    assert_eq!(stored_attempt(db), Some(first));
}

#[test]
fn stored_attempts_keep_their_identity_and_only_move_their_expiry_forward() {
    let fixture = proved_batch(FixtureAnchor::Registration);
    let db = &fixture.db;
    sign_batch_attempt(db, request(Some(fixture.registration_signature), 1_000)).unwrap();
    let conn = db.conn();
    let refused = |sql: &str, message: &str| {
        let error = conn.execute(sql, []).unwrap_err().to_string();
        assert!(error.contains(message), "{sql}: {error}");
    };

    refused(
        "UPDATE proxy_batch_attempts SET chain_id = 'other'",
        "a proxy batch keeps its identity and registration signature",
    );
    refused(
        "UPDATE proxy_batch_attempts SET expiry_height = expiry_height",
        "a re-signed proxy batch expires after the attempt it replaces",
    );
}

/// Inserts a submitting 0x09 lifecycle row for bundle 0's batch.
fn insert_lifecycle_row(db: &VotingDb, batch_digest: &[u8; 32]) {
    db.conn()
        .execute(
            "INSERT INTO chain_submissions
                 (identity_key, round_id, wallet_id, network, bundle_index, kind,
                  ordered_batch_digest, generation_digest, state, created_at, updated_at)
             VALUES (randomblob(40), ?1, ?2, 'regtest', 0, 'proxy_delegation_batch',
                     ?3, zeroblob(32), 'submitting', 1, 1)",
            rusqlite::params![ROUND_ID, WALLET_ID, batch_digest.as_slice()],
        )
        .unwrap();
}
