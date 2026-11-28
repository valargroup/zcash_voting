//! A vote chain scripted for proxy batch tests, and the answers and tree
//! passes those tests script it with.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use vote_commitment_tree::incrementalmerkletree::frontier::Frontier;
use vote_commitment_tree::{MerkleHashVote, TREE_DEPTH};

use crate::{
    chain_submission::{
        ChainHttpRequest, ChainHttpResponse, ChainTransport, ChainTransportError,
        ChainTransportFuture,
    },
    proxy::{
        batch::{sign_batch_attempt, AttemptSigningRequest},
        tests::submission_fixtures::{
            ProvedBatchFixture, CHAIN_ID, HOTKEY_SECRET, NETWORK, REGISTRATION_POSITION, ROUND_ID,
        },
    },
    storage::VotingDb,
};

/// The transaction hash every scripted acceptance returns.
pub(crate) const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
/// The height the fixture attempt is signed at.
pub(crate) const SIGNED_AT: u64 = 1_000;

/// Signs bundle 0's attempt of the fixture batch at `chain_height`, and
/// returns its batch digest.
pub(crate) fn sign_attempt(
    fixture: &ProvedBatchFixture,
    chain_height: u64,
    expired: bool,
) -> [u8; 32] {
    sign_batch_attempt(
        &fixture.db,
        AttemptSigningRequest {
            round_id: ROUND_ID,
            bundle_index: 0,
            hotkey_seed: &HOTKEY_SECRET,
            network: NETWORK,
            chain_id: CHAIN_ID,
            registration_signature: Some(fixture.registration_signature),
            chain_height,
            stored_attempt_expired: expired,
        },
    )
    .unwrap()
    .attempt
    .batch_digest
}

/// One scripted answer.
pub(crate) type Reply = Result<ChainHttpResponse, ChainTransportError>;

/// A check run inside every POST, before it is answered.
type PostHook = Box<dyn Fn() + Send + Sync>;

/// A vote chain that answers each request from a script and checks it was
/// the request the script expected.
#[derive(Default)]
pub(crate) struct ScriptedChain {
    replies: Mutex<VecDeque<(&'static str, &'static str, Reply)>>,
    /// The method of every request made, in order.
    methods: Mutex<Vec<&'static str>>,
    post_bodies: Mutex<Vec<serde_json::Value>>,
    before_post: Mutex<Option<PostHook>>,
}

impl ScriptedChain {
    /// Answers the next request, which must be `method` to a URL containing
    /// `fragment`.
    pub(crate) fn expect(&self, method: &'static str, fragment: &'static str, reply: Reply) {
        self.replies
            .lock()
            .unwrap()
            .push_back((method, fragment, reply));
    }

    /// Records `request` and returns the next scripted answer, checking it
    /// was the request the script expected.
    fn answer(&self, method: &'static str, request: &ChainHttpRequest) -> Reply {
        self.methods.lock().unwrap().push(method);
        let (expected_method, fragment, reply) = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| panic!("unscripted {method} {}", request.url()));
        assert_eq!(method, expected_method, "{}", request.url());
        assert!(
            request.url().contains(fragment),
            "{} does not contain {fragment}",
            request.url()
        );
        reply
    }

    /// Runs `hook` inside every later POST, before it is answered.
    pub(crate) fn before_post(&self, hook: impl Fn() + Send + Sync + 'static) {
        *self.before_post.lock().unwrap() = Some(Box::new(hook));
    }

    /// Every POST body, parsed, in order.
    pub(crate) fn post_bodies(&self) -> Vec<serde_json::Value> {
        self.post_bodies.lock().unwrap().clone()
    }

    /// How many POSTs were made.
    pub(crate) fn posts(&self) -> usize {
        self.methods
            .lock()
            .unwrap()
            .iter()
            .filter(|method| **method == "POST")
            .count()
    }

    /// Asserts every scripted answer was used.
    pub(crate) fn assert_drained(&self) {
        assert!(
            self.replies.lock().unwrap().is_empty(),
            "scripted replies were left unused"
        );
    }
}

impl ChainTransport for Arc<ScriptedChain> {
    fn chain_get<'a>(&'a self, request: ChainHttpRequest) -> ChainTransportFuture<'a> {
        Box::pin(async move { self.answer("GET", &request) })
    }

    fn chain_post_json<'a>(
        &'a self,
        request: ChainHttpRequest,
        json: Vec<u8>,
    ) -> ChainTransportFuture<'a> {
        Box::pin(async move {
            if let Some(hook) = self.before_post.lock().unwrap().as_ref() {
                hook();
            }
            self.post_bodies
                .lock()
                .unwrap()
                .push(serde_json::from_slice(&json).unwrap());
            self.answer("POST", &request)
        })
    }
}

/// A JSON answer with `status`.
pub(crate) fn json(status: u16, value: serde_json::Value) -> Reply {
    Ok(ChainHttpResponse::json(
        status,
        serde_json::to_vec(&value).unwrap(),
    ))
}

/// The proxy endpoint accepting the batch `batch_digest` names.
pub(crate) fn accepted(batch_digest: [u8; 32]) -> Reply {
    json(
        200,
        serde_json::json!({
            "tx_hash": HASH, "code": 0, "codespace": "", "log": "",
            "batch_digest": hex::encode(batch_digest),
        }),
    )
}

/// The proxy endpoint rejecting the batch with `codespace` `code`.
pub(crate) fn rejected(batch_digest: [u8; 32], codespace: &str, code: u32) -> Reply {
    json(
        422,
        serde_json::json!({
            "tx_hash": "", "code": code, "codespace": codespace, "log": "chain says no",
            "batch_digest": hex::encode(batch_digest),
        }),
    )
}

/// A `/tx` answer for a candidate that committed with `code`, carrying
/// `codespace` when the lookup asked for it.
pub(crate) fn committed_failure(codespace: Option<&str>, code: u32) -> Reply {
    let mut body = serde_json::json!({ "height": "42", "code": code, "log": "", "events": [] });
    if let Some(codespace) = codespace {
        body["codespace"] = codespace.into();
    }
    json(422, body)
}

/// The vote chain height route's answer at `height` (contracts-v1 §8).
pub(crate) fn chain_height(height: u64) -> Reply {
    json(200, serde_json::json!({ "height": height.to_string() }))
}

/// The validator-set answer of a replica at `height`.
pub(crate) fn block_height(height: u64) -> Reply {
    json(
        200,
        serde_json::json!({
            "block_height": height.to_string(), "validators": [],
            "pagination": { "next_key": null, "total": "0" },
        }),
    )
}

/// The `proxy_delegation_batch` event of the fixture batch landing with its
/// final VAN at `final_van` and its DCs right after it.
pub(crate) fn landed(
    fixture: &ProvedBatchFixture,
    batch_digest: [u8; 32],
    final_van: u64,
) -> Reply {
    let registration = fixture.batch.anchor_height == 0;
    let dcs = fixture.batch.dcs.len() as u64;
    let attribute = |key: &str, value: String| serde_json::json!({ "key": key, "value": value });
    json(
        200,
        serde_json::json!({
            "height": "42", "code": 0, "codespace": "", "log": "",
            "events": [{
                "type": "proxy_delegation_batch",
                "attributes": [
                    attribute("vote_round_id", ROUND_ID.to_string()),
                    attribute("batch_digest", hex::encode(batch_digest)),
                    attribute("has_registration", registration.to_string()),
                    attribute("nullifier_count", if registration { "5" } else { "0" }.to_string()),
                    attribute("final_van_leaf_index", final_van.to_string()),
                    attribute(
                        "dc_leaf_indices",
                        (1..=dcs)
                            .map(|offset| (final_van + offset).to_string())
                            .collect::<Vec<_>>()
                            .join(","),
                    ),
                    attribute(
                        "van_nullifiers",
                        fixture
                            .batch
                            .dcs
                            .iter()
                            .map(|dc| hex::encode(dc.van_nullifier))
                            .collect::<Vec<_>>()
                            .join(","),
                    ),
                    attribute("dc_count", dcs.to_string()),
                ],
            }],
        }),
    )
}

/// Scripts one exact-tree pass of a proxy batch over a tree holding
/// `leaves`: the replica's snapshot, then its one leaf page.
pub(crate) fn expect_tree_pass(chain: &ScriptedChain, leaves: &[[u8; 32]]) {
    let snapshot = |tree: serde_json::Value| json(200, serde_json::json!({ "tree": tree }));
    if leaves.is_empty() {
        chain.expect(
            "GET",
            "/latest",
            snapshot(serde_json::json!({ "next_index": 0, "height": 0 })),
        );
        return;
    }
    let mut frontier: Frontier<MerkleHashVote, { TREE_DEPTH as u8 }> = Frontier::empty();
    for leaf in leaves {
        assert!(frontier.append(MerkleHashVote::from_bytes(leaf).unwrap()));
    }
    let root = BASE64_STANDARD.encode(frontier.root().to_bytes());
    let encoded = leaves
        .iter()
        .map(|leaf| BASE64_STANDARD.encode(leaf))
        .collect::<Vec<_>>();
    chain.expect(
        "GET",
        "/latest",
        snapshot(serde_json::json!({ "next_index": leaves.len(), "root": root, "height": 1 })),
    );
    chain.expect(
        "GET",
        "/leaves",
        json(
            200,
            serde_json::json!({
                "blocks": [{ "height": 1, "start_index": 0, "leaves": encoded, "root": root }],
                "next_from_height": 0,
            }),
        ),
    );
}

/// [`expect_tree_pass`] on a replica at `height`, as a pass with a required
/// height reads it: the replica's height first. The snapshot carries no
/// `block_height`, so that height judges it.
pub(crate) fn expect_tree_pass_at(chain: &ScriptedChain, height: u64, leaves: &[[u8; 32]]) {
    chain.expect("GET", "validatorsets/latest", block_height(height));
    expect_tree_pass(chain, leaves);
}

/// Scripts code 2's tree pass for a one-endpoint chain: the highest height
/// the endpoint reports, which the pass's snapshot must reach, then
/// [`expect_tree_pass_at`] that height.
pub(crate) fn expect_code_two_pass(chain: &ScriptedChain, height: u64, leaves: &[[u8; 32]]) {
    chain.expect("GET", "v1/height", chain_height(height));
    expect_tree_pass_at(chain, height, leaves);
}

/// A canonical leaf that is no part of the fixture batch.
pub(crate) fn unrelated_leaf(byte: u64) -> [u8; 32] {
    use crate::backend::pasta_curves::{group::ff::PrimeField, pallas};
    pallas::Base::from(1_000_000 + byte).to_repr()
}

/// A tree whose leaf 7 is the chain-anchored fixture's registration VAN, as
/// [`REGISTRATION_POSITION`] says, so the batch lands after it.
pub(crate) fn tree_through_registration(db: &VotingDb) -> Vec<[u8; 32]> {
    let mut leaves: Vec<[u8; 32]> = (0..REGISTRATION_POSITION).map(unrelated_leaf).collect();
    leaves.push(registration_van(db));
    leaves
}

/// The batch's leaves in the order the chain appends them.
pub(crate) fn batch_leaves(fixture: &ProvedBatchFixture) -> Vec<[u8; 32]> {
    std::iter::once(fixture.batch.dcs.last().unwrap().vote_authority_note_new)
        .chain(fixture.batch.dcs.iter().map(|dc| dc.delegation_commitment))
        .collect()
}

/// The fixture bundle's registration VAN, `gov_comm`.
pub(crate) fn registration_van(db: &VotingDb) -> [u8; 32] {
    db.conn()
        .query_row("SELECT gov_comm FROM bundles", [], |row| {
            row.get::<_, Vec<u8>>(0)
        })
        .unwrap()
        .try_into()
        .unwrap()
}

/// The 0x09 row's state and diagnostic kind, if one exists.
pub(crate) fn row(db: &VotingDb) -> Option<(String, Option<String>)> {
    use rusqlite::OptionalExtension;
    db.conn()
        .query_row(
            "SELECT state, diagnostic_kind FROM chain_submissions
              WHERE kind = 'proxy_delegation_batch'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .unwrap()
}

/// The single integer `sql` selects.
pub(crate) fn count(db: &VotingDb, sql: &str) -> i64 {
    db.conn().query_row(sql, [], |row| row.get(0)).unwrap()
}
