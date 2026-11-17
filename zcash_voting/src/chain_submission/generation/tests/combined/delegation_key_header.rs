//! The fingerprint saved beside a delegation proof reaches the vote chain as
//! a request header on both delegation routes, through the real client.

use super::*;
use crate::delegation_key_fingerprint::{
    DelegationKeyFingerprint, DELEGATION_KEY_FINGERPRINT_HEADER,
};
use crate::storage::queries;

/// Records every POST's route, headers and body, accepts it, and reports the
/// transaction as not yet committed when polled.
#[derive(Default)]
struct RecordingChain {
    posts: Mutex<Vec<RecordedPost>>,
}

struct RecordedPost {
    url: String,
    headers: Vec<(String, String)>,
    body: serde_json::Value,
}

impl RecordedPost {
    fn key_headers(&self) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(name, _)| name == DELEGATION_KEY_FINGERPRINT_HEADER)
            .map(|(_, value)| value.as_str())
            .collect()
    }
}

impl ChainTransport for RecordingChain {
    fn chain_get<'a>(&'a self, _request: ChainHttpRequest) -> ChainTransportFuture<'a> {
        Box::pin(async {
            Ok(ChainHttpResponse::json(
                404,
                br#"{"error":"tx not found"}"#.to_vec(),
            ))
        })
    }

    fn chain_post_json<'a>(
        &'a self,
        request: ChainHttpRequest,
        json: Vec<u8>,
    ) -> ChainTransportFuture<'a> {
        let body: serde_json::Value = serde_json::from_slice(&json).unwrap();
        let mut answer = serde_json::json!({"tx_hash": HASH, "code": 0});
        if request.url().ends_with("/delegate-and-cast-vote-batch") {
            let wire: crate::wire::DelegateAndVoteBatchWire =
                serde_json::from_slice(&json).unwrap();
            answer["batch_digest"] = hex::encode(wire.authorization_digest().unwrap()).into();
        }
        self.posts.lock().unwrap().push(RecordedPost {
            url: request.url().to_owned(),
            headers: request.headers().to_vec(),
            body,
        });
        Box::pin(async move {
            Ok(ChainHttpResponse::json(
                200,
                serde_json::to_vec(&answer).unwrap(),
            ))
        })
    }
}

fn client(
    db: Arc<VotingDb>,
    chain: Arc<RecordingChain>,
) -> ChainSubmissionClient<Arc<RecordingChain>> {
    ChainSubmissionClient::with_transport(
        db,
        chain,
        ChainSubmissionClientConfig::for_network(
            crate::Network::Testnet,
            vec!["https://vote.example".to_owned()],
        ),
    )
    .unwrap()
}

/// Saves the fixture's proof again as a proof made by `fingerprint`'s key.
fn save_proof_made_by(db: &VotingDb, fingerprint: &DelegationKeyFingerprint) {
    queries::store_proof(
        &db.conn(),
        ROUND,
        "wallet-1",
        0,
        &[0x1b; 96],
        Some(fingerprint),
    )
    .unwrap();
}

#[tokio::test]
async fn a_standalone_delegation_sends_the_fingerprint_saved_with_its_proof() {
    let (db, _, signature) = persisted_delegation();
    let db = Arc::new(db);
    let fingerprint = DelegationKeyFingerprint::from_bytes([0xE1; 32]);
    save_proof_made_by(&db, &fingerprint);
    let chain = Arc::new(RecordingChain::default());

    client(db, chain.clone())
        .advance_delegation(
            AdvanceDelegation::from_signature_bytes([0x11; 32], 0, &signature).unwrap(),
            &ChainSubmissionControl::new(1),
        )
        .await
        .unwrap();

    let posts = chain.posts.lock().unwrap();
    assert_eq!(posts.len(), 1);
    assert!(posts[0].url.ends_with("/shielded-vote/v1/delegate-vote"));
    assert_eq!(posts[0].key_headers(), vec![fingerprint.to_hex().as_str()]);
    // The fingerprint is request metadata: the signed body keeps its nine fields.
    assert_eq!(posts[0].body.as_object().unwrap().len(), 9);
}

#[tokio::test]
async fn a_combined_batch_sends_the_fingerprint_saved_with_its_delegation_proof() {
    let (db, request) = fixture(2);
    let fingerprint = DelegationKeyFingerprint::from_bytes([0x0A; 32]);
    save_proof_made_by(&db, &fingerprint);
    let chain = Arc::new(RecordingChain::default());

    client(db, chain.clone())
        .advance_delegate_and_vote_batch(
            request,
            ChainRecoveryMode::StatusOnly,
            &ChainSubmissionControl::new(1),
        )
        .await
        .unwrap();

    let posts = chain.posts.lock().unwrap();
    assert_eq!(posts.len(), 1);
    assert!(posts[0]
        .url
        .ends_with("/shielded-vote/v1/delegate-and-cast-vote-batch"));
    assert_eq!(posts[0].key_headers(), vec![fingerprint.to_hex().as_str()]);
    assert_eq!(
        posts[0]
            .body
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        vec!["batch", "delegation"]
    );
    assert_eq!(posts[0].body["delegation"].as_object().unwrap().len(), 9);
}

/// A proof saved by a build that did not record its key is sent without a
/// fingerprint, never with the fingerprint of whichever build re-sends it.
#[tokio::test]
async fn a_proof_saved_without_a_fingerprint_is_sent_without_one_on_both_routes() {
    let (standalone_db, _, signature) = persisted_delegation();
    let standalone_chain = Arc::new(RecordingChain::default());
    client(Arc::new(standalone_db), standalone_chain.clone())
        .advance_delegation(
            AdvanceDelegation::from_signature_bytes([0x11; 32], 0, &signature).unwrap(),
            &ChainSubmissionControl::new(1),
        )
        .await
        .unwrap();

    let (combined_db, request) = fixture(1);
    let combined_chain = Arc::new(RecordingChain::default());
    client(combined_db, combined_chain.clone())
        .advance_delegate_and_vote_batch(
            request,
            ChainRecoveryMode::StatusOnly,
            &ChainSubmissionControl::new(1),
        )
        .await
        .unwrap();

    for chain in [standalone_chain, combined_chain] {
        let posts = chain.posts.lock().unwrap();
        assert_eq!(posts.len(), 1);
        assert!(posts[0].key_headers().is_empty(), "{}", posts[0].url);
    }
}

/// Proving again replaces the proof and its fingerprint together.
#[tokio::test]
async fn a_replaced_proof_never_keeps_the_fingerprint_of_the_proof_it_replaced() {
    let (db, _, signature) = persisted_delegation();
    let db = Arc::new(db);
    save_proof_made_by(&db, &DelegationKeyFingerprint::from_bytes([0xFB; 32]));
    queries::store_proof(&db.conn(), ROUND, "wallet-1", 0, &[0x1b; 96], None).unwrap();
    let chain = Arc::new(RecordingChain::default());

    client(db, chain.clone())
        .advance_delegation(
            AdvanceDelegation::from_signature_bytes([0x11; 32], 0, &signature).unwrap(),
            &ChainSubmissionControl::new(1),
        )
        .await
        .unwrap();

    assert!(chain.posts.lock().unwrap()[0].key_headers().is_empty());
}
