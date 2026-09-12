//! Resume through the real SQLite store and episode loop after reopening.

use std::{collections::VecDeque, sync::Mutex};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use incrementalmerkletree::frontier::Frontier;
use vote_commitment_tree::{MerkleHashVote, TREE_DEPTH};

use super::{super::*, fixtures::*};
use crate::{
    AdvanceVote, AdvanceVoteBatch, ChainAdvanceOutcome, ChainAdvancePolicy, ChainAdvanceRequest,
    ChainHttpRequest, ChainHttpResponse, ChainSubmissionClient, ChainSubmissionClientConfig,
    ChainSubmissionControl, ChainTransport, ChainTransportFuture,
};

#[derive(Default)]
struct LandedVoteChain {
    responses: Mutex<VecDeque<ChainHttpResponse>>,
    gets: Mutex<usize>,
}

impl LandedVoteChain {
    fn with_leaves(leaves: &[[u8; 32]]) -> Self {
        let mut frontier: Frontier<MerkleHashVote, { TREE_DEPTH as u8 }> = Frontier::empty();
        for leaf in leaves {
            assert!(frontier.append(MerkleHashVote::from_bytes(leaf).unwrap()));
        }
        let root = STANDARD.encode(frontier.root().to_bytes());
        let leaves = leaves
            .iter()
            .map(|leaf| STANDARD.encode(leaf))
            .collect::<Vec<_>>();
        let responses = [
            serde_json::json!({"tree": {"next_index": leaves.len(), "root": root, "height": 1}}),
            serde_json::json!({
                "blocks": [{"height": 1, "start_index": 0, "leaves": leaves, "root": root}],
                "next_from_height": 0
            }),
        ]
        .into_iter()
        .map(|body| ChainHttpResponse::json(200, serde_json::to_vec(&body).unwrap()))
        .collect();
        Self {
            responses: Mutex::new(responses),
            gets: Mutex::new(0),
        }
    }
}

impl ChainTransport for Arc<LandedVoteChain> {
    fn chain_get<'a>(&'a self, _request: ChainHttpRequest) -> ChainTransportFuture<'a> {
        Box::pin(async move {
            *self.gets.lock().unwrap() += 1;
            Ok(self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("one complete tree scan"))
        })
    }

    fn chain_post_json<'a>(
        &'a self,
        _request: ChainHttpRequest,
        _json: Vec<u8>,
    ) -> ChainTransportFuture<'a> {
        Box::pin(async { panic!("a vote already in the tree must never be retransmitted") })
    }
}

#[tokio::test]
async fn the_first_resumed_episode_confirms_an_abandoned_vote_after_reopening() {
    for batch in [false, true] {
        let path = temporary_path(if batch {
            "restart-batch"
        } else {
            "restart-vote"
        });
        let (identity, request, leaves, positions) = {
            let db = open_prepared(&path);
            let (identity, admission, request, leaves, positions) = if batch {
                let digest = store_two_vote_batch(&db);
                let identity = batch_identity(digest);
                (
                    identity.clone(),
                    StoreAdvancementRequest::vote_batch(identity, vec![1, 2]).unwrap(),
                    ChainAdvanceRequest::VoteBatch(AdvanceVoteBatch {
                        vote_round_id: [0x11; 32],
                        bundle_index: 0,
                        ordered_batch_digest: digest,
                        ordered_proposal_ids: vec![1, 2],
                    }),
                    vec![
                        [8; 32],
                        [0x22; 32],
                        recovery_for(0, 1).vote_commitment,
                        [0x32; 32],
                    ],
                    vec![2, 3],
                )
            } else {
                (
                    identity(),
                    StoreAdvancementRequest::vote(identity()),
                    ChainAdvanceRequest::Vote(AdvanceVote {
                        vote_round_id: [0x11; 32],
                        bundle_index: 0,
                        proposal_id: 1,
                    }),
                    vec![
                        [8; 32],
                        recovery().vote_authority_note_new,
                        recovery().vote_commitment,
                    ],
                    vec![2],
                )
            };
            let store = SqliteChainSubmissionStore::new(db);
            assert!(matches!(
                store.admit(&admission, true, 1, 10).unwrap(),
                StoreAdmission::Ready {
                    fresh_reservation: true,
                    ..
                }
            ));
            // Drop the connection with its POST reservation still unclassified.
            (identity, request, leaves, positions)
        };

        {
            let db = open_prepared(&path);
            let chain = Arc::new(LandedVoteChain::with_leaves(&leaves));
            let client = ChainSubmissionClient::with_transport(
                Arc::clone(&db),
                Arc::clone(&chain),
                ChainSubmissionClientConfig::for_network(
                    crate::Network::Testnet,
                    vec!["https://chain.invalid".to_string()],
                ),
            )
            .unwrap();
            let control = ChainSubmissionControl::new(1);
            let outcome = client
                .advance_until_terminal_in_epoch(
                    request,
                    &ChainAdvancePolicy::for_persisted_work(),
                    &control,
                    1,
                )
                .await
                .unwrap();

            let ChainAdvanceOutcome::Confirmed(confirmation) = outcome else {
                panic!("the first resumed episode must check the tree: {outcome:?}");
            };
            assert_eq!(confirmation.vote_commitment_positions(), positions);
            assert_eq!(*chain.gets.lock().unwrap(), 2);
            let store = SqliteChainSubmissionStore::new(db);
            let record = store
                .transact(|tx| load_submission(tx, &identity))
                .unwrap()
                .unwrap();
            assert_eq!(record.durable_state(), ChainSubmissionState::Confirmed);
            assert_eq!(record.committed_post_reservations(), 1);
        }
        std::fs::remove_file(path).unwrap();
    }
}
