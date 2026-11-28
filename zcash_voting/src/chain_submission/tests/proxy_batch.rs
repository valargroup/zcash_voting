//! The `proxy_delegation_batch` kind end to end, over the SQLite store and
//! real derivation: every classification row of a 0x09 answer and recovery
//! of an ambiguous submission.

use std::{sync::Arc, time::Duration};

use super::scripted_chain::*;
use crate::{
    chain_submission::{
        AdvanceProxyDelegationBatch, ChainAdvanceOutcome, ChainAdvancePolicy, ChainAdvanceRequest,
        ChainRecoveryMode, ChainSubmissionClient, ChainSubmissionClientConfig,
        ChainSubmissionConfirmationSource, ChainSubmissionControl, ChainSubmissionDiagnosticKind,
        ChainSubmissionFailure, ChainSubmissionPending, ChainTransportError,
    },
    proxy::tests::submission_fixtures::{
        proved_batch, FixtureAnchor, ProvedBatchFixture, CHAIN_ID, NETWORK, ROUND_ID,
    },
    storage::VotingDb,
};

/// A proved batch on bundle 0 with an attempt signed at [`SIGNED_AT`], and
/// its batch digest.
fn signed(anchor: FixtureAnchor) -> (ProvedBatchFixture, [u8; 32]) {
    let fixture = proved_batch(anchor);
    let digest = sign_attempt(&fixture, SIGNED_AT, false);
    (fixture, digest)
}

fn client(
    db: &Arc<VotingDb>,
    chain: &Arc<ScriptedChain>,
    attempts: usize,
) -> ChainSubmissionClient<Arc<ScriptedChain>> {
    ChainSubmissionClient::with_transport(
        Arc::clone(db),
        Arc::clone(chain),
        ChainSubmissionClientConfig {
            network: NETWORK,
            vote_chain_id: CHAIN_ID.to_string(),
            endpoints: vec!["https://vote.example".to_string()],
            tracking_window: Duration::from_secs(60),
            maximum_post_attempts: attempts,
            retry_backoffs: vec![Duration::from_millis(1); attempts - 1],
        },
    )
    .unwrap()
}

/// One episode for the batch, as the step runs it.
async fn advance(
    client: &ChainSubmissionClient<Arc<ScriptedChain>>,
    batch_digest: [u8; 32],
) -> Result<ChainAdvanceOutcome, ChainSubmissionFailure> {
    client
        .advance_until_terminal_in_epoch(
            ChainAdvanceRequest::ProxyDelegationBatch(AdvanceProxyDelegationBatch {
                vote_round_id: hex::decode(ROUND_ID).unwrap().try_into().unwrap(),
                bundle_index: 0,
                batch_digest,
            }),
            &ChainAdvancePolicy {
                initial_recovery_mode: ChainRecoveryMode::ExactTree,
                pending_repoll: Duration::from_millis(1),
                escalate_to_exact_tree: true,
                max_passes: 4,
            },
            &ChainSubmissionControl::new(0),
            0,
        )
        .await
}

fn diagnostic_kind(outcome: &ChainAdvanceOutcome) -> ChainSubmissionDiagnosticKind {
    match outcome {
        ChainAdvanceOutcome::Rejected(diagnostic)
        | ChainAdvanceOutcome::SubmittedWithoutHash(diagnostic)
        | ChainAdvanceOutcome::StillPending(ChainSubmissionPending::Recovering {
            diagnostic,
            ..
        }) => diagnostic.kind(),
        other => panic!("no diagnostic in {other:?}"),
    }
}

#[tokio::test]
async fn an_accepted_batch_confirms_by_hash_and_lands_its_dcs() {
    let (fixture, digest) = signed(FixtureAnchor::Registration);
    let chain = Arc::new(ScriptedChain::default());
    chain.expect("POST", "/proxy-delegation-batch", accepted(digest));
    chain.expect("GET", "/tx/", landed(&fixture, digest, 5));

    let outcome = advance(&client(&fixture.db, &chain, 1), digest)
        .await
        .unwrap();

    let ChainAdvanceOutcome::Confirmed(confirmation) = outcome else {
        panic!("expected a confirmation, got {outcome:?}")
    };
    assert_eq!(
        confirmation.source(),
        ChainSubmissionConfirmationSource::Hash
    );
    assert_eq!(confirmation.final_van_position(), 5);
    assert_eq!(confirmation.vote_commitment_positions(), &[6, 7]);
    chain.assert_drained();
    assert_eq!(row(&fixture.db).unwrap().0, "confirmed");
    let db = &fixture.db;
    assert_eq!(db.load_van_position_u64(ROUND_ID, 0).unwrap(), 5);
    assert_eq!(
        db.get_delegation_tx_hash(ROUND_ID, 0).unwrap().as_deref(),
        Some(HASH)
    );
    let landings: Vec<(i64, i64)> = db
        .conn()
        .prepare("SELECT dc_slot, tree_position FROM proxy_dc_landings ORDER BY tree_position")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let expected: Vec<(i64, i64)> = fixture
        .batch
        .dcs
        .iter()
        .zip(6..)
        .map(|(dc, position)| (i64::from(dc.dc_slot), position))
        .collect();
    assert_eq!(landings, expected);
}

#[tokio::test]
async fn an_expired_attempt_retires_the_generation_and_its_re_sign_is_admitted() {
    let (fixture, digest) = signed(FixtureAnchor::Chain);
    let chain = Arc::new(ScriptedChain::default());
    chain.expect(
        "POST",
        "/proxy-delegation-batch",
        rejected(digest, "vote", 52),
    );
    let client = client(&fixture.db, &chain, 1);

    let outcome = advance(&client, digest).await.unwrap();

    assert_eq!(
        diagnostic_kind(&outcome),
        ChainSubmissionDiagnosticKind::ProxyBatchExpired
    );
    assert!(matches!(outcome, ChainAdvanceOutcome::Rejected(_)));
    assert_eq!(row(&fixture.db), None);

    // The same submission, re-signed with a later expiry, is admitted afresh.
    assert_eq!(sign_attempt(&fixture, 1_200, true), digest);
    chain.expect("POST", "/proxy-delegation-batch", accepted(digest));
    chain.expect("GET", "/tx/", landed(&fixture, digest, 9));
    let outcome = advance(&client, digest).await.unwrap();
    assert!(matches!(outcome, ChainAdvanceOutcome::Confirmed(_)));
    chain.assert_drained();
}

#[tokio::test]
async fn a_paused_batch_stays_recoverable_and_retries_once_the_pause_lifts() {
    let (fixture, digest) = signed(FixtureAnchor::Registration);
    let chain = Arc::new(ScriptedChain::default());
    chain.expect(
        "POST",
        "/proxy-delegation-batch",
        rejected(digest, "vote", 51),
    );
    let client = client(&fixture.db, &chain, 1);

    let outcome = advance(&client, digest).await.unwrap();

    assert!(matches!(outcome, ChainAdvanceOutcome::StillPending(_)));
    assert_eq!(
        diagnostic_kind(&outcome),
        ChainSubmissionDiagnosticKind::ProxyBatchPaused
    );
    assert_eq!(
        row(&fixture.db),
        Some((
            "recovering".to_string(),
            Some("proxy_batch_paused".to_string())
        ))
    );

    expect_tree_pass(&chain, &[unrelated_leaf(1)]);
    chain.expect("POST", "/proxy-delegation-batch", accepted(digest));
    chain.expect("GET", "/tx/", landed(&fixture, digest, 1));
    let outcome = advance(&client, digest).await.unwrap();
    assert!(matches!(outcome, ChainAdvanceOutcome::Confirmed(_)));
    chain.assert_drained();
}

#[tokio::test]
async fn a_spent_nullifier_with_the_registration_alone_in_the_tree_splits_the_batch_off() {
    let (fixture, digest) = signed(FixtureAnchor::Registration);
    let chain = Arc::new(ScriptedChain::default());
    chain.expect(
        "POST",
        "/proxy-delegation-batch",
        rejected(digest, "vote", 2),
    );
    let registration = registration_van(&fixture.db);
    expect_code_two_pass(&chain, 1_100, &[unrelated_leaf(1), registration]);

    let outcome = advance(&client(&fixture.db, &chain, 1), digest)
        .await
        .unwrap();

    assert!(matches!(outcome, ChainAdvanceOutcome::Rejected(_)));
    assert_eq!(
        diagnostic_kind(&outcome),
        ChainSubmissionDiagnosticKind::ProxyRegistrationLanded
    );
    chain.assert_drained();
    let db = &fixture.db;
    assert_eq!(row(db), None);
    assert_eq!(db.load_van_position_u64(ROUND_ID, 0).unwrap(), 1);
    assert_eq!(count(db, "SELECT COUNT(*) FROM proxy_batches"), 0);
    assert_eq!(count(db, "SELECT COUNT(*) FROM proxy_batch_attempts"), 0);
    // The slots keep their nullifiers and randomizers, so the batch is proved
    // again byte for byte against the chain anchor.
    assert_eq!(
        count(
            db,
            "SELECT COUNT(*) FROM proxy_slots WHERE dc_slot_nf IS NOT NULL"
        ),
        fixture.batch.dcs.len() as i64
    );
}

#[tokio::test]
async fn a_spent_nullifier_with_the_batch_in_the_tree_confirms_from_the_tree() {
    let (fixture, digest) = signed(FixtureAnchor::Chain);
    let chain = Arc::new(ScriptedChain::default());
    chain.expect(
        "POST",
        "/proxy-delegation-batch",
        rejected(digest, "vote", 2),
    );
    let mut leaves = tree_through_registration(&fixture.db);
    leaves.extend(batch_leaves(&fixture));
    expect_code_two_pass(&chain, 1_100, &leaves);

    let outcome = advance(&client(&fixture.db, &chain, 1), digest)
        .await
        .unwrap();

    let ChainAdvanceOutcome::Confirmed(confirmation) = outcome else {
        panic!("expected a tree confirmation, got {outcome:?}")
    };
    assert_eq!(
        confirmation.source(),
        ChainSubmissionConfirmationSource::Tree
    );
    assert_eq!(confirmation.final_van_position(), 8);
    assert_eq!(fixture.db.load_van_position_u64(ROUND_ID, 0).unwrap(), 8);
    assert_eq!(
        count(&fixture.db, "SELECT COUNT(*) FROM proxy_dc_landings"),
        fixture.batch.dcs.len() as i64
    );
}

/// Any refusal but a pause or an expiry, including spent nullifiers the tree
/// shows nowhere, leaves the batch pending, and the next pass sends it
/// again. Which (codespace, code) pairs are "other" is the protocol's
/// classification, tested with it.
#[tokio::test]
async fn any_other_refusal_leaves_the_batch_pending_and_sends_it_again() {
    for (codespace, code) in [("vote", 5), ("vote", 2)] {
        let (fixture, digest) = signed(FixtureAnchor::Chain);
        let chain = Arc::new(ScriptedChain::default());
        chain.expect(
            "POST",
            "/proxy-delegation-batch",
            rejected(digest, codespace, code),
        );
        if (codespace, code) == ("vote", 2) {
            expect_code_two_pass(&chain, 1_100, &[unrelated_leaf(1)]);
        }
        let client = client(&fixture.db, &chain, 1);

        let outcome = advance(&client, digest).await.unwrap();

        assert!(
            matches!(outcome, ChainAdvanceOutcome::StillPending(_)),
            "{codespace} {code}: {outcome:?}"
        );
        assert_eq!(
            diagnostic_kind(&outcome),
            ChainSubmissionDiagnosticKind::ChainRejected
        );
        assert_eq!(
            row(&fixture.db),
            Some(("recovering".to_string(), Some("chain_rejected".to_string())))
        );

        expect_tree_pass(&chain, &[unrelated_leaf(1)]);
        chain.expect("POST", "/proxy-delegation-batch", accepted(digest));
        chain.expect("GET", "/tx/", landed(&fixture, digest, 9));
        let outcome = advance(&client, digest).await.unwrap();
        assert!(matches!(outcome, ChainAdvanceOutcome::Confirmed(_)));
        chain.assert_drained();
    }
}

#[tokio::test]
async fn another_rejection_after_an_ambiguous_attempt_keeps_the_batch_recoverable() {
    let (fixture, digest) = signed(FixtureAnchor::Chain);
    let chain = Arc::new(ScriptedChain::default());
    chain.expect(
        "POST",
        "/proxy-delegation-batch",
        Err(ChainTransportError::possibly_dispatched("response lost")),
    );
    chain.expect(
        "POST",
        "/proxy-delegation-batch",
        rejected(digest, "vote", 5),
    );

    let outcome = advance(&client(&fixture.db, &chain, 2), digest)
        .await
        .unwrap();

    assert!(
        matches!(outcome, ChainAdvanceOutcome::StillPending(_)),
        "{outcome:?}"
    );
    assert_eq!(
        diagnostic_kind(&outcome),
        ChainSubmissionDiagnosticKind::ChainRejected
    );
    assert_eq!(
        row(&fixture.db),
        Some((
            "recovering".to_string(),
            Some("ambiguous_dispatch".to_string())
        ))
    );
}

#[tokio::test]
async fn an_ambiguous_submission_is_recovered_from_the_tree_without_redispatch() {
    let (fixture, digest) = signed(FixtureAnchor::Chain);
    let chain = Arc::new(ScriptedChain::default());
    chain.expect(
        "POST",
        "/proxy-delegation-batch",
        Err(ChainTransportError::possibly_dispatched("response lost")),
    );
    let client = client(&fixture.db, &chain, 1);
    let outcome = advance(&client, digest).await.unwrap();
    assert_eq!(
        diagnostic_kind(&outcome),
        ChainSubmissionDiagnosticKind::AmbiguousDispatch
    );

    let mut leaves = tree_through_registration(&fixture.db);
    leaves.push(unrelated_leaf(100));
    leaves.extend(batch_leaves(&fixture));
    expect_tree_pass(&chain, &leaves);
    let outcome = advance(&client, digest).await.unwrap();

    let ChainAdvanceOutcome::Confirmed(confirmation) = outcome else {
        panic!("expected a tree confirmation, got {outcome:?}")
    };
    assert_eq!(confirmation.final_van_position(), 9);
    assert_eq!(fixture.db.load_van_position_u64(ROUND_ID, 0).unwrap(), 9);
    assert_eq!(chain.posts(), 1);
    chain.assert_drained();
}

#[tokio::test]
async fn a_committed_failure_is_classified_on_its_codespace() {
    // An expiry in a block retires the generation like one at CheckTx.
    let (fixture, digest) = signed(FixtureAnchor::Chain);
    let chain = Arc::new(ScriptedChain::default());
    chain.expect("POST", "/proxy-delegation-batch", accepted(digest));
    chain.expect("GET", "/tx/", committed_failure(Some("vote"), 52));
    let outcome = advance(&client(&fixture.db, &chain, 1), digest)
        .await
        .unwrap();
    assert_eq!(
        diagnostic_kind(&outcome),
        ChainSubmissionDiagnosticKind::ProxyBatchExpired
    );
    assert_eq!(row(&fixture.db), None);

    // Any other committed failure leaves the batch recoverable, to be sent
    // again.
    let (fixture, digest) = signed(FixtureAnchor::Chain);
    let chain = Arc::new(ScriptedChain::default());
    chain.expect("POST", "/proxy-delegation-batch", accepted(digest));
    chain.expect("GET", "/tx/", committed_failure(Some("vote"), 7));
    let outcome = advance(&client(&fixture.db, &chain, 1), digest)
        .await
        .unwrap();
    assert!(matches!(outcome, ChainAdvanceOutcome::StillPending(_)));
    assert_eq!(
        row(&fixture.db),
        Some(("recovering".to_string(), Some("chain_rejected".to_string())))
    );

    // Without a codespace the failure is not classified: the row recovers.
    let (fixture, digest) = signed(FixtureAnchor::Chain);
    let chain = Arc::new(ScriptedChain::default());
    chain.expect("POST", "/proxy-delegation-batch", accepted(digest));
    chain.expect("GET", "/tx/", committed_failure(None, 52));
    let outcome = advance(&client(&fixture.db, &chain, 1), digest)
        .await
        .unwrap();
    assert_eq!(
        diagnostic_kind(&outcome),
        ChainSubmissionDiagnosticKind::ChainRejected
    );
    assert_eq!(row(&fixture.db).unwrap().0, "recovering");
    chain.assert_drained();
}

/// While no replica reaches the highest reported height, code 2 proves
/// nothing about where the nullifiers went: the batch stays recoverable and
/// pending rather than ending.
#[tokio::test]
async fn a_spent_nullifier_pass_on_lagging_replicas_leaves_the_batch_pending() {
    let (fixture, digest) = signed(FixtureAnchor::Chain);
    let chain = Arc::new(ScriptedChain::default());
    chain.expect(
        "POST",
        "/proxy-delegation-batch",
        rejected(digest, "vote", 2),
    );
    chain.expect("GET", "v1/height", chain_height(1_200));
    expect_tree_pass_at(&chain, 1_100, &[unrelated_leaf(1)]);

    let outcome = advance(&client(&fixture.db, &chain, 1), digest)
        .await
        .unwrap();

    assert!(
        matches!(outcome, ChainAdvanceOutcome::StillPending(_)),
        "{outcome:?}"
    );
    assert_eq!(row(&fixture.db).unwrap().0, "recovering");
    chain.assert_drained();
}

/// Saves an undispatched vote of the fixture bundle for proposal 1: its
/// recovery is stored, and it has no hash, tree position or lifecycle row.
fn save_undispatched_vote(db: &VotingDb) {
    crate::vote::insert_recovery_fixture(
        db,
        &crate::vote::VoteRecoveryBundle {
            vote_round_id: ROUND_ID.to_string(),
            bundle_index: 0,
            proposal_id: 1,
            vote_decision: 0,
            anchor_height: 41,
            vc_tree_position: 0,
            single_share: false,
            num_options: 2,
            van_nullifier: [0x10; 32],
            vote_authority_note_new: [0x01; 32],
            vote_commitment: [0x21; 32],
            proof: vec![0x13; 96],
            shares_hash: [0x14; 32],
            r_vpk: [0x15; 32],
            alpha_v: [0x16; 32],
            vote_auth_sig: [0x17; 64],
            encrypted_shares: vec![crate::types::EncryptedShare {
                c1: vec![0x21; 32],
                c2: vec![0x22; 32],
                share_index: 0,
                plaintext_value: 5,
                randomness: vec![0x23; 32],
            }],
            share_blinds: vec![[0x41; 32]],
            share_comms: vec![[0x51; 32]],
            batch: None,
        },
    )
    .unwrap();
}

/// Whether the fixture bundle's vote for proposal 1 still holds its
/// recovery, so it could still be dispatched.
fn vote_recovery_stored(db: &VotingDb) -> bool {
    count(
        db,
        "SELECT COUNT(*) FROM votes
          WHERE bundle_index = 0 AND proposal_id = 1 AND commitment_bundle_json IS NOT NULL",
    ) == 1
}

/// A vote saved before the batch was committed, and held from dispatch
/// since, was proved against the VAN the batch spends. The landing that
/// moves the bundle to the batch's successor VAN, and the split-off that
/// moves it to the lifted registration's leaf, each retire it in the same
/// transaction, so it is cast again from the bundle's current VAN.
#[tokio::test]
async fn an_undispatched_vote_is_retired_when_the_batch_lands_or_splits_off() {
    let (fixture, digest) = signed(FixtureAnchor::Registration);
    save_undispatched_vote(&fixture.db);
    let chain = Arc::new(ScriptedChain::default());
    chain.expect("POST", "/proxy-delegation-batch", accepted(digest));
    chain.expect("GET", "/tx/", landed(&fixture, digest, 5));
    assert!(vote_recovery_stored(&fixture.db));

    let outcome = advance(&client(&fixture.db, &chain, 1), digest)
        .await
        .unwrap();

    assert!(matches!(outcome, ChainAdvanceOutcome::Confirmed(_)));
    assert!(!vote_recovery_stored(&fixture.db), "retired at landing");

    let (fixture, digest) = signed(FixtureAnchor::Registration);
    save_undispatched_vote(&fixture.db);
    let chain = Arc::new(ScriptedChain::default());
    chain.expect(
        "POST",
        "/proxy-delegation-batch",
        rejected(digest, "vote", 2),
    );
    let registration = registration_van(&fixture.db);
    expect_code_two_pass(&chain, 1_100, &[unrelated_leaf(1), registration]);

    let outcome = advance(&client(&fixture.db, &chain, 1), digest)
        .await
        .unwrap();

    assert_eq!(
        diagnostic_kind(&outcome),
        ChainSubmissionDiagnosticKind::ProxyRegistrationLanded
    );
    assert!(!vote_recovery_stored(&fixture.db), "retired at split-off");
}
