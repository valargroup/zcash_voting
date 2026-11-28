//! `proxy::availability` returns the first result of contract §11.4 that
//! applies, in its fixed order, from the chain's answers as fetched, the
//! host's proofs and timing, and the wallet's own casts.

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde_json::json;

use super::fixtures::{
    answered_db_with_bundle_weights, db_with_bundle_weights, host_at, request, seeded_rng,
    FIXTURE_ROSTER, ROUND_ID, WALLET_ID,
};
use crate::{
    config::WalletCapabilities,
    proxy::{
        availability, commit_allocation, delegator::evaluate_availability, preview_allocation,
        DcShareLayout, GovNullifierProof, ProxyAllocationError, ProxyAvailabilityInputs,
        CIRCUIT_FINGERPRINTS, PROXY_DELEGATION_ENABLED, PROXY_DELEGATION_WALLET_VERSION,
    },
    storage::{queries, VotingDb},
    vote_work::RoundHostContext,
    wire::{ProxyAllocationPreview, ProxyAvailability, ProxyDelegationErrorKind},
};

/// The vote end of every timed host here.
const VOTE_END: u64 = 100_000;

/// Every input of one availability check, owned.
struct Scenario {
    build_enabled: bool,
    wallet: WalletCapabilities,
    capabilities: serde_json::Value,
    round: serde_json::Value,
    params: Option<serde_json::Value>,
    gov_nullifiers: GovNullifierProof,
    host: RoundHostContext,
}

/// One change to a scenario, as a table of cases names it.
type Change = Box<dyn Fn(&mut Scenario)>;

/// The fixture round's id as the REST API encodes it.
fn round_b64(round_id: &str) -> String {
    BASE64_STANDARD.encode(hex::decode(round_id).unwrap())
}

/// Capability field 5 as the chain publishes it, from hex fingerprints.
fn fingerprints(published: &[(&str, &str)]) -> serde_json::Value {
    published
        .iter()
        .map(|(name, fingerprint)| {
            json!({
                "name": name,
                "vk_blake2b": BASE64_STANDARD.encode(hex::decode(fingerprint).unwrap()),
            })
        })
        .collect()
}

/// A scenario in which delegation is available.
fn available() -> Scenario {
    Scenario {
        build_enabled: true,
        wallet: WalletCapabilities {
            proxy_delegation: vec![PROXY_DELEGATION_WALLET_VERSION.to_owned()],
            ..WalletCapabilities::default()
        },
        capabilities: json!({
            "atomic_cast_vote_batch": true,
            "proxy_delegation_version": 1,
            "circuit_fingerprints": fingerprints(&CIRCUIT_FINGERPRINTS),
        }),
        round: json!({ "round": {
            "vote_round_id": round_b64(ROUND_ID),
            "status": 1,
            "proxy_delegation": { "enabled": true, "next_delegate_index_at_creation": 4 },
        } }),
        params: Some(json!({ "params": { "max_delegates": 100 }, "next_delegate_index": 4 })),
        gov_nullifiers: GovNullifierProof::Unspent,
        host: host_at(10, Some(0), Some(VOTE_END)),
    }
}

impl Scenario {
    /// The availability `db`'s wallet gets in this scenario.
    fn check(&self, db: &VotingDb) -> Result<ProxyAvailability, crate::wire::ProxyDelegationError> {
        let capabilities = self.capabilities.to_string();
        let round = self.round.to_string();
        let params = self.params.as_ref().map(serde_json::Value::to_string);
        evaluate_availability(
            self.build_enabled,
            db,
            ROUND_ID,
            &ProxyAvailabilityInputs {
                wallet_capabilities: &self.wallet,
                protocol_capabilities: capabilities.as_bytes(),
                vote_round: round.as_bytes(),
                proxy_delegation_params: params.as_deref().map(str::as_bytes),
                gov_nullifiers: self.gov_nullifiers,
            },
            &self.host,
        )
    }

    /// [`Self::check`], which must succeed.
    fn result(&self, db: &VotingDb) -> ProxyAvailability {
        self.check(db).unwrap()
    }
}

/// A wallet whose bundle 0 has a vote for proposal 1, submitted when
/// `submitted`.
fn wallet_with_vote(submitted: bool) -> VotingDb {
    let db = db_with_bundle_weights(&[10]);
    queries::store_vote(&db.conn(), ROUND_ID, WALLET_ID, 0, 1, 0, &[0x0C; 32]).unwrap();
    if submitted {
        db.mark_vote_submitted(ROUND_ID, 0, 1, &"ab".repeat(32))
            .unwrap();
    }
    db
}

#[test]
fn every_result_applies_in_the_contract_s_order() {
    let db = wallet_with_vote(true);
    let mut scenario = Scenario {
        build_enabled: true,
        capabilities: json!({ "proxy_delegation_version": 0 }),
        params: Some(json!({ "params": { "proxy_dc_paused": true } })),
        gov_nullifiers: GovNullifierProof::Spent,
        host: host_at(VOTE_END, None, None),
        ..available()
    };
    let mut seen = Vec::new();
    seen.push(scenario.result(&db));
    scenario.capabilities = available().capabilities;
    seen.push(scenario.result(&db));
    scenario.params = available().params;
    seen.push(scenario.result(&db));
    let db = db_with_bundle_weights(&[10]);
    seen.push(scenario.result(&db));
    scenario.gov_nullifiers = GovNullifierProof::Unspent;
    seen.push(scenario.result(&db));
    scenario.host = host_at(VOTE_END, Some(0), Some(VOTE_END));
    seen.push(scenario.result(&db));
    scenario.host = available().host;
    seen.push(scenario.result(&db));

    assert_eq!(
        seen,
        [
            ProxyAvailability::Unsupported,
            ProxyAvailability::Paused,
            ProxyAvailability::AlreadyVoted {
                used_elsewhere: false
            },
            ProxyAvailability::AlreadyVoted {
                used_elsewhere: true
            },
            ProxyAvailability::TimingUnavailable,
            ProxyAvailability::VoteEnded,
            ProxyAvailability::Available,
        ]
    );
}

#[test]
fn any_missing_capability_or_proxy_flag_is_unsupported() {
    let db = db_with_bundle_weights(&[10]);
    let mut other = CIRCUIT_FINGERPRINTS;
    other[3].1 = "00000000000000000000000000000000000000000000000000000000000000ff";
    let cases: Vec<(&str, Change)> = vec![
        (
            "a build without the feature",
            Box::new(|s| s.build_enabled = false),
        ),
        (
            "wallet capabilities without v1",
            Box::new(|s| s.wallet.proxy_delegation = vec!["v2".to_owned()]),
        ),
        (
            "a dormant chain",
            Box::new(|s| s.capabilities["proxy_delegation_version"] = json!(0)),
        ),
        (
            "another zkp4 fingerprint",
            Box::new(move |s| s.capabilities["circuit_fingerprints"] = fingerprints(&other)),
        ),
        (
            "a round whose proxy flag is off",
            Box::new(|s| s.round["round"]["proxy_delegation"]["enabled"] = json!(false)),
        ),
    ];
    for (case, change) in cases {
        let mut scenario = available();
        change(&mut scenario);
        // The params are read only once the round offers the feature.
        scenario.params = None;
        assert_eq!(
            scenario.result(&db),
            ProxyAvailability::Unsupported,
            "{case}"
        );
    }
    assert_eq!(available().result(&db), ProxyAvailability::Available);
}

#[test]
fn an_undispatched_vote_without_a_share_plan_is_not_already_voted() {
    let db = wallet_with_vote(false);
    assert_eq!(available().result(&db), ProxyAvailability::Available);
    let db = wallet_with_vote(true);
    assert_eq!(
        available().result(&db),
        ProxyAvailability::AlreadyVoted {
            used_elsewhere: false
        }
    );
}

/// Bundle 0's vote saved its share plan while bundle 1 still voted, so the
/// plan has no immediate share. Delegating all of bundle 1 would move the
/// immediate share onto that vote, against its plan, so preview, commit and
/// availability refuse. The commit rereads the plan after the preview.
#[test]
fn a_saved_vote_share_plan_refuses_delegating() {
    let db = answered_db_with_bundle_weights(&[60, 40]);
    let request = request(&[(17, 10_000)]);
    let preview = preview_allocation(&db, ROUND_ID, &request, DcShareLayout::Standard).unwrap();
    for sql in [
        "INSERT INTO votes (round_id, wallet_id, bundle_index, proposal_id, choice, commitment,
                            created_at, commitment_bundle_json)
         VALUES (?1, ?2, 0, 1, 0, zeroblob(32), 1, '{}')",
        "INSERT INTO helper_share_plans
             (round_id, wallet_id, bundle_index, proposal_id, commitment_bundle_json,
              configured_server_urls_json, share_plans_json, format_version,
              placement_guarantee, created_at)
         VALUES (?1, ?2, 0, 1, '{}', '[]', '[]', 1, 'strict', 1)",
    ] {
        db.conn()
            .execute(sql, rusqlite::params![ROUND_ID, WALLET_ID])
            .unwrap();
    }

    assert!(matches!(
        preview_allocation(&db, ROUND_ID, &request, DcShareLayout::Standard),
        Err(ProxyAllocationError::AlreadyVoted { bundle_index: 0 })
    ));
    assert!(matches!(
        commit_allocation(
            &db,
            ROUND_ID,
            &request,
            DcShareLayout::Standard,
            &ProxyAllocationPreview::from(&preview),
            &FIXTURE_ROSTER,
            &mut seeded_rng(1),
        ),
        Err(ProxyAllocationError::AlreadyVoted { bundle_index: 0 })
    ));
    assert_eq!(
        available().result(&db),
        ProxyAvailability::AlreadyVoted {
            used_elsewhere: false
        }
    );
}

#[test]
fn spent_governance_nullifiers_are_used_elsewhere_only_without_local_evidence() {
    let spent = Scenario {
        gov_nullifiers: GovNullifierProof::Spent,
        ..available()
    };
    let db = db_with_bundle_weights(&[10]);
    assert_eq!(
        spent.result(&db),
        ProxyAvailability::AlreadyVoted {
            used_elsewhere: true
        }
    );

    // This wallet's own registration explains the spend.
    queries::store_delegation_tx_hash(&db.conn(), ROUND_ID, WALLET_ID, 0, &"cd".repeat(32))
        .unwrap();
    assert_eq!(spent.result(&db), ProxyAvailability::Available);
}

#[test]
fn an_unproven_nullifier_state_cannot_be_verified_unless_a_cast_settles_it() {
    let unproven = Scenario {
        gov_nullifiers: GovNullifierProof::Unavailable,
        ..available()
    };
    let db = db_with_bundle_weights(&[10]);
    assert_eq!(
        unproven.check(&db).unwrap_err().kind,
        ProxyDelegationErrorKind::CannotVerify
    );
    let db = wallet_with_vote(true);
    assert_eq!(
        unproven.result(&db),
        ProxyAvailability::AlreadyVoted {
            used_elsewhere: false
        }
    );
}

#[test]
fn malformed_answers_and_answers_about_another_round_are_refused() {
    let db = db_with_bundle_weights(&[10]);
    let other_round = "02".repeat(32);
    let cases: Vec<(&str, Change)> = vec![
        (
            "another round's response",
            Box::new(move |s| s.round["round"]["vote_round_id"] = json!(round_b64(&other_round))),
        ),
        ("no params", Box::new(|s| s.params = None)),
    ];
    for (case, change) in cases {
        let mut scenario = available();
        change(&mut scenario);
        assert_eq!(
            scenario.check(&db).unwrap_err().kind,
            ProxyDelegationErrorKind::Other,
            "{case}"
        );
    }
}

/// The default build, which every shipped build is, keeps the gate closed:
/// the unstable feature is opt-in.
#[cfg(not(feature = "unstable-proxy-delegation"))]
#[test]
fn the_default_build_keeps_proxy_delegation_off() {
    const { assert!(!PROXY_DELEGATION_ENABLED) };
    assert!(WalletCapabilities::default().proxy_delegation.is_empty());
}

#[test]
fn this_build_reports_the_feature_only_once_it_is_enabled() {
    let db = db_with_bundle_weights(&[10]);
    let scenario = available();
    let capabilities = scenario.capabilities.to_string();
    let round = scenario.round.to_string();
    let params = scenario.params.as_ref().unwrap().to_string();
    let result = availability(
        &db,
        ROUND_ID,
        &ProxyAvailabilityInputs {
            wallet_capabilities: &scenario.wallet,
            protocol_capabilities: capabilities.as_bytes(),
            vote_round: round.as_bytes(),
            proxy_delegation_params: Some(params.as_bytes()),
            gov_nullifiers: scenario.gov_nullifiers,
        },
        &scenario.host,
    )
    .unwrap();
    let expected = if PROXY_DELEGATION_ENABLED {
        ProxyAvailability::Available
    } else {
        ProxyAvailability::Unsupported
    };
    assert_eq!(result, expected);
    assert_eq!(
        WalletCapabilities::default().proxy_delegation.is_empty(),
        !PROXY_DELEGATION_ENABLED
    );
    let legacy: WalletCapabilities = serde_json::from_value(json!({
        "vote_server": ["v1"], "vote_protocol": ["v0"], "tally": ["v0"], "pir": ["v0"],
    }))
    .unwrap();
    assert!(
        legacy.proxy_delegation.is_empty(),
        "capabilities stored before the field offer nothing"
    );
}

/// BLAKE2b-256 of a circuit's pinned verifying key, as capability field 5
/// publishes it.
fn fingerprint_of(pinned: String) -> String {
    hex::encode(
        blake2b_simd::Params::new()
            .hash_length(32)
            .hash(pinned.as_bytes())
            .as_bytes(),
    )
}

#[test]
#[ignore = "generates all four circuits' keys; run with `make proofs`"]
fn the_published_fingerprints_are_those_of_the_circuits_this_build_proves_with() {
    use voting_circuits::{delegation, proxy_delegation, share_reveal, vote_proof};
    let compiled = [
        (
            "zkp1",
            fingerprint_of(format!(
                "{:?}",
                delegation::delegation_cached_keys().unwrap().2.pinned()
            )),
        ),
        (
            "zkp2",
            fingerprint_of(format!(
                "{:?}",
                vote_proof::vote_proof_cached_keys().unwrap().2.pinned()
            )),
        ),
        (
            "zkp3",
            fingerprint_of(format!(
                "{:?}",
                share_reveal::share_reveal_cached_keys().unwrap().2.pinned()
            )),
        ),
        (
            "zkp4",
            fingerprint_of(format!(
                "{:?}",
                proxy_delegation::proxy_delegation_cached_keys()
                    .unwrap()
                    .2
                    .pinned()
            )),
        ),
    ];
    assert_eq!(
        compiled,
        CIRCUIT_FINGERPRINTS.map(|(name, fingerprint)| (name, fingerprint.to_owned()))
    );
}
