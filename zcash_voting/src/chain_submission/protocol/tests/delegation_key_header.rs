//! The delegation routes name the circuit key that made the proof, and the
//! vote chain's "wrong key" answer is an ordinary definite rejection.

use super::*;
use crate::delegation_key_fingerprint::{
    DelegationKeyFingerprint, DELEGATION_KEY_FINGERPRINT_HEADER,
};

/// Module code the vote chain answers with when the fingerprint a wallet sent
/// differs from the key its verifier uses.
const DELEGATION_KEY_MISMATCH_CODE: u32 = 50;

fn key_headers(request: &ChainHttpRequest) -> Vec<&str> {
    request
        .headers()
        .iter()
        .filter(|(name, _)| name == DELEGATION_KEY_FINGERPRINT_HEADER)
        .map(|(_, value)| value.as_str())
        .collect()
}

/// A combined body whose fields pass the client's own digest check.
fn combined_request() -> (crate::wire::DelegateAndVoteBatchWire, [u8; 32]) {
    use base64::Engine as _;
    let encode = |byte: u8| base64::prelude::BASE64_STANDARD.encode([byte; 32]);
    let mut delegation = delegation();
    delegation.vote_round_id = encode(0x11);
    delegation.gov_comm = encode(0x01);
    let mut member = vote();
    member.vote_round_id = encode(0x11);
    member.anchor_height = 0;
    member.r_vpk = encode(0x02);
    member.van_nullifier = encode(0x03);
    member.vote_authority_note_new = encode(0x04);
    member.vote_commitment = encode(0x05);
    let wire = crate::wire::DelegateAndVoteBatchWire {
        delegation,
        batch: crate::wire::VoteCommitmentBatchWire {
            votes: vec![member],
        },
    };
    let digest = wire.authorization_digest().unwrap();
    (wire, digest)
}

#[tokio::test]
async fn a_delegation_post_names_its_proof_key_in_a_header_and_leaves_the_body_unchanged() {
    let transport = Arc::new(ScriptedTransport::default());
    transport.queue(Ok(json(200, format!(r#"{{"tx_hash":"{HASH}","code":0}}"#))));
    let client = protocol_client(
        transport.clone(),
        Network::Testnet,
        &["https://vote.example"],
    );
    let wire = delegation();
    let fingerprint = DelegationKeyFingerprint::from_bytes([0xE1; 32]);

    let outcome = client
        .submit_delegation_with_dispatch(
            0,
            &wire,
            Some(fingerprint),
            ChainPostDispatch::default(),
            &crate::ObservationScope::disabled(),
        )
        .await;

    assert!(matches!(outcome, PostAttemptOutcome::Accepted(_)));
    let calls = transport.calls.lock().unwrap();
    let (_, request, body) = &calls[0];
    assert_eq!(key_headers(request), vec!["e1".repeat(32).as_str()]);
    assert_eq!(body, wire.to_json().unwrap().as_bytes());
}

#[tokio::test]
async fn a_combined_post_names_the_delegation_proof_key_in_the_same_header() {
    let (wire, digest) = combined_request();
    let transport = Arc::new(ScriptedTransport::default());
    transport.queue(Ok(json(
        200,
        format!(
            r#"{{"tx_hash":"{HASH}","code":0,"batch_digest":"{}"}}"#,
            hex::encode(digest)
        ),
    )));
    let client = protocol_client(
        transport.clone(),
        Network::Testnet,
        &["https://vote.example"],
    );
    let fingerprint = DelegationKeyFingerprint::from_bytes([0x0A; 32]);

    let outcome = client
        .submit_delegate_and_vote_batch_with_dispatch(
            0,
            &wire,
            digest,
            Some(fingerprint),
            ChainPostDispatch::default(),
            &crate::ObservationScope::disabled(),
        )
        .await;

    assert!(matches!(outcome, PostAttemptOutcome::Accepted(_)));
    let calls = transport.calls.lock().unwrap();
    let (_, request, body) = &calls[0];
    assert_eq!(
        request.url(),
        "https://vote.example/shielded-vote/v1/delegate-and-cast-vote-batch"
    );
    assert_eq!(key_headers(request), vec!["0a".repeat(32).as_str()]);
    assert_eq!(body, wire.to_json().unwrap().as_bytes());
}

#[tokio::test]
async fn a_proof_whose_key_was_not_recorded_sends_no_key_header() {
    let (combined, digest) = combined_request();
    let transport = Arc::new(ScriptedTransport::default());
    transport.queue(Ok(json(200, format!(r#"{{"tx_hash":"{HASH}","code":0}}"#))));
    transport.queue(Ok(json(
        200,
        format!(
            r#"{{"tx_hash":"{HASH}","code":0,"batch_digest":"{}"}}"#,
            hex::encode(digest)
        ),
    )));
    let client = protocol_client(
        transport.clone(),
        Network::Testnet,
        &["https://vote.example"],
    );

    client
        .submit_delegation_with_dispatch(
            0,
            &delegation(),
            None,
            ChainPostDispatch::default(),
            &crate::ObservationScope::disabled(),
        )
        .await;
    client
        .submit_delegate_and_vote_batch_with_dispatch(
            0,
            &combined,
            digest,
            None,
            ChainPostDispatch::default(),
            &crate::ObservationScope::disabled(),
        )
        .await;

    let calls = transport.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    for (_, request, _) in calls.iter() {
        assert!(key_headers(request).is_empty(), "{}", request.url());
    }
}

#[tokio::test]
async fn vote_routes_never_carry_a_delegation_key_header() {
    let transport = Arc::new(ScriptedTransport::default());
    let batch_digest = [9; 32];
    transport.queue(Ok(json(200, format!(r#"{{"tx_hash":"{HASH}","code":0}}"#))));
    transport.queue(Ok(json(
        200,
        format!(
            r#"{{"tx_hash":"{HASH}","code":0,"batch_digest":"{}"}}"#,
            hex::encode(batch_digest)
        ),
    )));
    let client = protocol_client(
        transport.clone(),
        Network::Testnet,
        &["https://vote.example"],
    );

    client
        .submit_vote_with_dispatch(
            0,
            &vote(),
            ChainPostDispatch::default(),
            &crate::ObservationScope::disabled(),
        )
        .await;
    client
        .submit_vote_batch_with_dispatch(
            0,
            &crate::wire::VoteCommitmentBatchWire {
                votes: vec![vote(), vote()],
            },
            batch_digest,
            ChainPostDispatch::default(),
            &crate::ObservationScope::disabled(),
        )
        .await;

    let calls = transport.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    for (_, request, _) in calls.iter() {
        assert!(key_headers(request).is_empty(), "{}", request.url());
    }
}

#[tokio::test]
async fn a_key_mismatch_answer_is_a_definite_rejection_read_from_its_numeric_code() {
    let transport = Arc::new(ScriptedTransport::default());
    // Nothing was broadcast, so the vote chain reports no transaction hash.
    transport.queue(Ok(json(
        422,
        format!(
            r#"{{"tx_hash":"","code":{DELEGATION_KEY_MISMATCH_CODE},"log":"delegation circuit key mismatch: update the app"}}"#
        ),
    )));
    let client = protocol_client(transport, Network::Testnet, &["https://vote.example"]);

    let outcome = client
        .submit_delegation_with_dispatch(
            0,
            &delegation(),
            Some(DelegationKeyFingerprint::from_bytes([0xFB; 32])),
            ChainPostDispatch::default(),
            &crate::ObservationScope::disabled(),
        )
        .await;

    let PostAttemptOutcome::Rejected {
        code,
        kind,
        diagnostic,
        candidate_transaction_hash,
    } = outcome
    else {
        panic!("a key mismatch must be a definite rejection, got {outcome:?}");
    };
    assert_eq!(code, DELEGATION_KEY_MISMATCH_CODE);
    assert_eq!(kind, ChainRejectionKind::Other);
    assert_eq!(candidate_transaction_hash, None);
    assert_eq!(
        diagnostic.kind(),
        ChainSubmissionDiagnosticKind::ChainRejected
    );
    assert!(diagnostic.message().contains("update the app"));
}

#[tokio::test]
async fn a_combined_key_mismatch_answer_is_definite_only_with_the_batch_digest() {
    let (wire, digest) = combined_request();
    let transport = Arc::new(ScriptedTransport::default());
    transport.queue(Ok(json(
        422,
        format!(
            r#"{{"tx_hash":"","code":{DELEGATION_KEY_MISMATCH_CODE},"log":"update the app","batch_digest":"{}"}}"#,
            hex::encode(digest)
        ),
    )));
    transport.queue(Ok(json(
        422,
        format!(r#"{{"tx_hash":"","code":{DELEGATION_KEY_MISMATCH_CODE},"log":"update the app"}}"#),
    )));
    let client = protocol_client(transport, Network::Testnet, &["https://vote.example"]);
    let fingerprint = Some(DelegationKeyFingerprint::from_bytes([0xFB; 32]));

    let with_digest = client
        .submit_delegate_and_vote_batch_with_dispatch(
            0,
            &wire,
            digest,
            fingerprint,
            ChainPostDispatch::default(),
            &crate::ObservationScope::disabled(),
        )
        .await;
    let without_digest = client
        .submit_delegate_and_vote_batch_with_dispatch(
            0,
            &wire,
            digest,
            fingerprint,
            ChainPostDispatch::default(),
            &crate::ObservationScope::disabled(),
        )
        .await;

    assert!(matches!(
        with_digest,
        PostAttemptOutcome::Rejected {
            code: DELEGATION_KEY_MISMATCH_CODE,
            kind: ChainRejectionKind::Other,
            candidate_transaction_hash: None,
            ..
        }
    ));
    assert!(matches!(
        without_digest,
        PostAttemptOutcome::PossiblyDispatched(_)
    ));
}
