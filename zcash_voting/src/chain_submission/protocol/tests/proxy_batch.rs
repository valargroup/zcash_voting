//! The 0x09 endpoint: its exact request, its codespace-bearing answer, and
//! the classification of a rejection on (codespace, code).

use super::*;
use crate::proxy::batch::{ProxyDelegationActionWire, ProxyDelegationBatchWire};

/// A one-DC body; `registration` decides whether it carries the ZKP1.
fn proxy_batch(registration: bool) -> ProxyDelegationBatchWire {
    ProxyDelegationBatchWire {
        vote_round_id: "cm91bmQ=".to_string(),
        vote_comm_tree_anchor_height: if registration { 0 } else { 41 },
        registration: registration.then(|| DelegationSubmissionWire {
            rk: "rk".to_string(),
            spend_auth_sig: "sig".to_string(),
            tx1_effects: "effects".to_string(),
            nf_signed: "nf".to_string(),
            cmx_new: "cmx".to_string(),
            gov_comm: "van".to_string(),
            gov_nullifiers: vec!["gov".to_string()],
            proof: "zkp1".to_string(),
            vote_round_id: "cm91bmQ=".to_string(),
        }),
        dcs: vec![ProxyDelegationActionWire {
            van_nullifier: "nf".to_string(),
            r_vpk: "rvpk".to_string(),
            vote_authority_note_new: "van-new".to_string(),
            proxy_delegation_commitment: "dc".to_string(),
            dc_slot_nullifier: "slot".to_string(),
            proof: "zkp4".to_string(),
            vote_auth_sig: "d1sig".to_string(),
        }],
        expiry_height: 540,
    }
}

const DIGEST: [u8; 32] = [0x5d; 32];

/// The endpoint's answer: a `BroadcastResult` with codespace and the digest.
fn answer(status: u16, code: u32, codespace: &str) -> ChainHttpResponse {
    json(
        status,
        format!(
            r#"{{"tx_hash":"{HASH}","code":{code},"codespace":"{codespace}","log":"chain says no","batch_digest":"{}"}}"#,
            hex::encode(DIGEST)
        ),
    )
}

async fn submit(response: ChainHttpResponse) -> (PostAttemptOutcome, Arc<ScriptedTransport>) {
    let transport = Arc::new(ScriptedTransport::default());
    transport.queue(Ok(response));
    let client = protocol_client(
        transport.clone(),
        Network::Testnet,
        &["https://vote.example"],
    );
    let outcome = client
        .submit_proxy_delegation_batch_with_dispatch(
            0,
            &proxy_batch(true),
            DIGEST,
            ChainPostDispatch::default(),
            &crate::ObservationScope::disabled(),
        )
        .await;
    (outcome, transport)
}

/// The body goes to `/proxy-delegation-batch` as exactly the message's
/// fields, `registration` omitted when the batch does not carry it.
#[tokio::test]
async fn constructs_exact_proxy_batch_url_and_json() {
    let (outcome, transport) = submit(answer(200, 0, "")).await;
    assert_eq!(
        outcome,
        PostAttemptOutcome::Accepted(CandidateTransactionHash::from_str(HASH).unwrap())
    );
    let calls = transport.calls.lock().unwrap();
    let (is_post, request, body) = &calls[0];
    assert!(*is_post);
    assert_eq!(
        request.url(),
        "https://vote.example/shielded-vote/v1/proxy-delegation-batch"
    );
    assert_eq!(body, proxy_batch(true).to_json().unwrap().as_bytes());

    let keys = |wire: &ProxyDelegationBatchWire| {
        let value: serde_json::Value = serde_json::from_str(&wire.to_json().unwrap()).unwrap();
        value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(
        keys(&proxy_batch(true)),
        [
            "dcs",
            "expiry_height",
            "registration",
            "vote_comm_tree_anchor_height",
            "vote_round_id"
        ]
    );
    assert_eq!(
        keys(&proxy_batch(false)),
        [
            "dcs",
            "expiry_height",
            "vote_comm_tree_anchor_height",
            "vote_round_id"
        ]
    );
    let value: serde_json::Value =
        serde_json::from_str(&proxy_batch(true).to_json().unwrap()).unwrap();
    let action = value["dcs"][0].as_object().unwrap();
    assert_eq!(
        action.keys().cloned().collect::<Vec<_>>(),
        [
            "dc_slot_nullifier",
            "proof",
            "proxy_delegation_commitment",
            "r_vpk",
            "van_nullifier",
            "vote_auth_sig",
            "vote_authority_note_new"
        ]
    );
    assert_eq!(value["registration"]["van_cmx"], "van");
    assert_eq!(value["registration"]["signed_note_nullifier"], "nf");
    assert_eq!(value["expiry_height"], 540);
}

/// Every rejection is classified on the pair: only codespace `vote` names a
/// registered error, so the same number elsewhere is a terminal rejection.
#[tokio::test]
async fn proxy_batch_rejections_classify_on_codespace_and_code() {
    let cases = [
        ("vote", 2, ChainRejectionKind::NullifierAlreadySpent),
        ("vote", 51, ChainRejectionKind::ProxyBatchPaused),
        ("vote", 52, ChainRejectionKind::ProxyBatchExpired),
        ("vote", 5, ChainRejectionKind::Other),
        ("sdk", 2, ChainRejectionKind::Other),
        ("sdk", 52, ChainRejectionKind::Other),
        ("", 52, ChainRejectionKind::Other),
    ];
    for (codespace, code, expected) in cases {
        let (outcome, _) = submit(answer(422, code, codespace)).await;
        let PostAttemptOutcome::Rejected {
            code: observed_code,
            codespace: observed_codespace,
            kind,
            diagnostic,
            candidate_transaction_hash,
        } = outcome
        else {
            panic!("{codespace} {code}: expected a rejection, got {outcome:?}");
        };
        assert_eq!(kind, expected, "{codespace} {code}");
        assert_eq!(observed_code, code);
        assert_eq!(observed_codespace.as_deref(), Some(codespace));
        assert_eq!(
            diagnostic.kind(),
            ChainSubmissionDiagnosticKind::ChainRejected
        );
        assert!(candidate_transaction_hash.is_some());
    }
}

/// An answer without codespace, without the matching canonical digest, or
/// with any other field is not evidence: the POST stays possibly dispatched.
#[tokio::test]
async fn a_proxy_batch_answer_needs_its_codespace_and_matching_digest() {
    let digest = hex::encode(DIGEST);
    for body in [
        format!(r#"{{"tx_hash":"{HASH}","code":0,"batch_digest":"{digest}"}}"#),
        format!(r#"{{"tx_hash":"{HASH}","code":0,"codespace":""}}"#),
        format!(
            r#"{{"tx_hash":"{HASH}","code":0,"codespace":"","batch_digest":"{}"}}"#,
            digest.to_ascii_uppercase()
        ),
        format!(
            r#"{{"tx_hash":"{HASH}","code":0,"codespace":"","batch_digest":"{}"}}"#,
            hex::encode([0x11; 32])
        ),
        format!(
            r#"{{"tx_hash":"{HASH}","code":0,"codespace":"","batch_digest":"{digest}","height":3}}"#
        ),
        format!(r#"{{"code":0,"codespace":"","batch_digest":"{digest}"}}"#),
    ] {
        let (outcome, _) = submit(json(200, body.clone())).await;
        assert!(
            matches!(outcome, PostAttemptOutcome::PossiblyDispatched(_)),
            "{body}: {outcome:?}"
        );
    }
}

/// The height route's answer at `value`.
fn chain_height(value: &str) -> ChainHttpResponse {
    json(200, format!(r#"{{"height":"{value}"}}"#))
}

/// The validator-set route's answer at `value`.
fn validator_set_height(value: &str) -> ChainHttpResponse {
    json(
        200,
        format!(r#"{{"block_height":"{value}","validators":[],"pagination":{{}}}}"#),
    )
}

/// The chain height is the highest any endpoint reports. Each endpoint is
/// read from the vote chain's height route, and from the validator-set route
/// when that route is absent; one unreachable endpoint does not fail the
/// lookup and is not asked again.
#[tokio::test]
async fn the_chain_height_is_the_highest_any_endpoint_reports() {
    let transport = Arc::new(ScriptedTransport::default());
    transport.queue(Ok(chain_height("1200")));
    transport.queue(Err(ChainTransportError::possibly_dispatched("down")));
    transport.queue(Ok(json(404, r#"{"error":"not found"}"#)));
    transport.queue(Ok(validator_set_height("1450")));
    let client = protocol_client(
        transport.clone(),
        Network::Testnet,
        &[
            "https://one.example",
            "https://two.example",
            "https://three.example",
        ],
    );

    let observed = client
        .latest_block_height(&crate::ObservationScope::disabled())
        .await
        .unwrap();

    assert_eq!(observed, 1450);
    let calls = transport.calls.lock().unwrap();
    let urls: Vec<&str> = calls.iter().map(|(_, request, _)| request.url()).collect();
    assert_eq!(
        urls,
        [
            "https://one.example/shielded-vote/v1/height",
            "https://two.example/shielded-vote/v1/height",
            "https://three.example/shielded-vote/v1/height",
            "https://three.example/cosmos/base/tendermint/v1beta1/validatorsets/latest?pagination.limit=1",
        ]
    );
    assert!(calls.iter().all(|(is_post, ..)| !is_post));
}

/// A node without the height route, or whose answer omits the height, is
/// read from the validator-set route, as before the route existed.
#[tokio::test]
async fn a_node_without_the_height_route_falls_back_to_the_validator_set() {
    for missing in [
        json(404, r#"{"error":"not found"}"#),
        ChainHttpResponse::new(
            404,
            b"404 page not found".to_vec(),
            Some("text/plain".into()),
            Vec::new(),
        ),
        json(200, "{}"),
        json(200, r#"{"height":"soon"}"#),
    ] {
        let transport = Arc::new(ScriptedTransport::default());
        transport.queue(Ok(missing));
        transport.queue(Ok(validator_set_height("1300")));
        let client = protocol_client(transport, Network::Testnet, &["https://vote.example"]);

        assert_eq!(
            client
                .latest_block_height(&crate::ObservationScope::disabled())
                .await
                .unwrap(),
            1300
        );
    }
}

/// No answer, or none that carries a decimal height on either route, fails
/// the lookup.
#[tokio::test]
async fn the_chain_height_lookup_fails_without_a_decimal_height() {
    for fallback in [
        Ok(json(200, r#"{"block_height":"twelve"}"#)),
        Ok(json(200, r#"{"validators":[]}"#)),
        Ok(json(404, r#"{"error":"not found"}"#)),
        Err(ChainTransportError::possibly_dispatched("down")),
    ] {
        let transport = Arc::new(ScriptedTransport::default());
        transport.queue(Ok(json(404, r#"{"error":"not found"}"#)));
        transport.queue(fallback);
        let client = protocol_client(transport, Network::Testnet, &["https://vote.example"]);

        assert!(client
            .latest_block_height(&crate::ObservationScope::disabled())
            .await
            .is_err());
    }
    let transport = Arc::new(ScriptedTransport::default());
    transport.queue(Err(ChainTransportError::possibly_dispatched("down")));
    let client = protocol_client(transport, Network::Testnet, &["https://vote.example"]);
    assert!(client
        .latest_block_height(&crate::ObservationScope::disabled())
        .await
        .is_err());
}
