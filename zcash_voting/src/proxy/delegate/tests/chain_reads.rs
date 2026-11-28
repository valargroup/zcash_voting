//! What the host fetches from the vote chain for delegate mode, and how each
//! answer is read (contract §3.2, §8).

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::json;

use super::fixtures::{alice_key as first_key, key, listed_ballot, VERIFIER_SECRET};
use crate::proxy::delegate::{
    delegate_ballots_path, delegate_by_key_path, parse_delegate_ballots_page,
    parse_delegate_by_key_response, parse_pool_share_count, parse_proxy_params_response,
    ChainDelegateBallot, ChainReadError, ProxyVerifier, PROXY_PARAMS_PATH,
};

/// A round id.
const ROUND: [u8; 32] = [0xab; 32];

/// The REST paths the host fetches, under a vote server's base URL.
#[test]
fn paths_name_the_chain_s_routes() {
    let dk = first_key();
    assert_eq!(
        delegate_by_key_path(&dk),
        format!(
            "/shielded-vote/v1/delegates/by-key/{}",
            hex::encode(dk.to_bytes())
        )
    );
    assert_eq!(
        delegate_ballots_path(&ROUND, 0),
        format!(
            "/shielded-vote/v1/delegate-ballots/{}?start_after=0&limit=1000",
            "ab".repeat(32)
        )
    );
    assert!(delegate_ballots_path(&ROUND, 17).contains("start_after=17&"));
    assert_eq!(
        PROXY_PARAMS_PATH,
        "/shielded-vote/v1/proxy-delegation-params"
    );
}

/// The lookup by key names an entry, or nothing when the enabled node says
/// the key was never registered.
#[test]
fn lookups_by_key_name_an_entry() {
    let dk = first_key();
    let body = json!({ "delegate": { "index": 17, "keys": [{ "dk": "AAAA" }], "later": 1 } });
    assert_eq!(
        parse_delegate_by_key_response(&dk, 200, body.to_string().as_bytes()),
        Ok(Some(17))
    );
    assert_eq!(
        parse_delegate_by_key_response(&dk, 500, b"oops"),
        Err(ChainReadError::Unavailable { status: 500 })
    );
    assert!(matches!(
        parse_delegate_by_key_response(&dk, 200, br#"{"delegate":{}}"#),
        Err(ChainReadError::Malformed { .. })
    ));
}

/// Only an enabled node's not-found answer for this very key means it was
/// never registered; a dormant node's 404, a plain-text 404 and a not-found
/// answer for another key leave the question open.
#[test]
fn only_the_node_s_not_found_answer_means_unregistered() {
    let dk = first_key();
    let not_found = |key_hex: &str| {
        // vote-sdk's REST error for the query's NotFound status, as BaseApp
        // wraps it.
        format!(
            "{{\"error\":\"rpc error: code = NotFound desc = delegate key {key_hex} was never registered: key not found\"}}\n"
        )
    };
    assert_eq!(
        parse_delegate_by_key_response(&dk, 404, not_found(&hex::encode(dk.to_bytes())).as_bytes()),
        Ok(None)
    );
    let other_key = hex::encode(key(VERIFIER_SECRET).to_bytes());
    for body in [
        not_found(&other_key),
        "{\"error\":\"proxy delegation is not enabled\"}\n".to_owned(),
        "404 page not found\n".to_owned(),
    ] {
        assert_eq!(
            parse_delegate_by_key_response(&dk, 404, body.as_bytes()),
            Err(ChainReadError::Unavailable { status: 404 }),
            "{body}"
        );
    }
}

/// A ballot page lists the round's ballots, and its next page starts after
/// them. The page rules are on the shared parser, in
/// `proxy/tests/delegate_ballots.rs`.
#[test]
fn ballot_pages_list_ballots_in_order() {
    let body = json!({
        "ballots": [listed_ballot(&ROUND, 3, &[1, 0]), listed_ballot(&ROUND, 17, &[0, 2])],
        "next_start_after": 17,
    });
    let page = parse_delegate_ballots_page(&ROUND, 0, 200, body.to_string().as_bytes()).unwrap();
    assert_eq!(page.next_start_after, Some(17));
    assert_eq!(
        page.ballot_of(17),
        Some(&ChainDelegateBallot {
            delegate_index: 17,
            options: vec![0, 2],
            height: 1_234,
        })
    );
    assert_eq!(page.ballot_of(4), None);

    assert_eq!(
        parse_delegate_ballots_page(&ROUND, 0, 404, b"{}"),
        Err(ChainReadError::RoundNotFound {
            round_id: "ab".repeat(32)
        })
    );
    assert_eq!(
        parse_delegate_ballots_page(&ROUND, 0, 503, b""),
        Err(ChainReadError::Unavailable { status: 503 })
    );
    let too_many_options = json!({ "ballots": [listed_ballot(&ROUND, 3, &[0; 51])] });
    assert!(matches!(
        parse_delegate_ballots_page(&ROUND, 0, 200, too_many_options.to_string().as_bytes()),
        Err(ChainReadError::Malformed { .. })
    ));
}

/// The params answer gives the verifier set with unique ids, valid keys and
/// a threshold that fits; an absent set is empty and closed.
#[test]
fn params_answers_give_the_verifier_set() {
    let pubkey = key(VERIFIER_SECRET).to_bytes();
    let body = json!({
        "params": {
            "verifiers": [{ "verifier_id": 1, "pubkey": BASE64.encode(pubkey) }],
            "verifier_threshold": 1,
            "max_delegates": 20_000,
            "proxy_dc_paused": true,
        },
        "next_delegate_index": 18,
    });
    let set = parse_proxy_params_response(200, body.to_string().as_bytes()).unwrap();
    assert_eq!(
        set.verifiers,
        vec![ProxyVerifier {
            verifier_id: 1,
            pubkey
        }]
    );
    assert_eq!(set.threshold, 1);

    let closed = parse_proxy_params_response(200, br#"{"next_delegate_index":1}"#).unwrap();
    assert!(closed.verifiers.is_empty());
    assert_eq!(closed.threshold, 0);

    let verifier =
        |id: u32, key: [u8; 32]| json!({ "verifier_id": id, "pubkey": BASE64.encode(key) });
    for params in [
        json!({ "verifiers": [verifier(1, pubkey), verifier(1, first_key().to_bytes())], "verifier_threshold": 1 }),
        json!({ "verifiers": [verifier(1, [0; 32])], "verifier_threshold": 1 }),
        json!({ "verifiers": [verifier(1, pubkey)], "verifier_threshold": 2 }),
    ] {
        let body = json!({ "params": params }).to_string();
        assert!(
            matches!(
                parse_proxy_params_response(200, body.as_bytes()),
                Err(ChainReadError::Malformed { .. })
            ),
            "{body}"
        );
    }
    assert_eq!(
        parse_proxy_params_response(404, b""),
        Err(ChainReadError::Unavailable { status: 404 })
    );
}

/// A pool's share count is an 8-byte big-endian value, 0 when absent.
#[test]
fn pool_share_counts_are_big_endian() {
    assert_eq!(parse_pool_share_count(None), Ok(0));
    assert_eq!(parse_pool_share_count(Some(&37u64.to_be_bytes())), Ok(37));
    assert!(matches!(
        parse_pool_share_count(Some(&[0, 37])),
        Err(ChainReadError::Malformed { .. })
    ));
}
