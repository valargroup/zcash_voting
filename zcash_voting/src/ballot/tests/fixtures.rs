//! Shared fixtures: a signed static/dynamic config pair and sample ballots.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signer, SigningKey};

use crate::ballot::{Ballot, BallotOption, BallotProposal};
use crate::config::{
    resolve_dynamic_voting_config, resolve_static_voting_config, PirLayout,
    ResolveVotingConfigOptions, ResolvedVotingConfig,
};
use crate::round_auth::{RoundAuthPayloadV2, RoundAuthPayloadV3};

pub(super) const ROUND_ID: &str =
    "0101010101010101010101010101010101010101010101010101010101010101";
pub(super) const EA_PK: [u8; 32] = [7u8; 32];

pub(super) fn trusted_key() -> SigningKey {
    SigningKey::from_bytes(&[11u8; 32])
}

pub(super) fn untrusted_key() -> SigningKey {
    SigningKey::from_bytes(&[12u8; 32])
}

pub(super) fn layout() -> PirLayout {
    PirLayout {
        pir_depth: 19,
        tier0_layers: 12,
        tier1_layers: 7,
        poly_len: 4096,
    }
}

fn option(index: u32, label: &str, description: &str) -> BallotOption {
    BallotOption {
        index,
        label: label.to_string(),
        description: description.to_string(),
    }
}

/// The `full_ballot` entry of the published test vectors.
pub(super) fn full_ballot() -> Ballot {
    Ballot {
        title: "Round 1".to_string(),
        description: "Two questions.".to_string(),
        discussion_url: "https://forum.example.org/t/1".to_string(),
        proposals: vec![
            BallotProposal {
                id: 1,
                title: "Adopt ZIP-1".to_string(),
                description: "First question.".to_string(),
                zip_number: "ZIP-1".to_string(),
                forum_url: "https://forum.example.org/t/2".to_string(),
                options: vec![
                    option(0, "Support", "In favor"),
                    option(1, "Oppose", ""),
                    option(2, "Abstain", ""),
                ],
            },
            BallotProposal {
                id: 2,
                title: "Second".to_string(),
                description: String::new(),
                zip_number: String::new(),
                forum_url: String::new(),
                options: vec![option(0, "A", ""), option(1, "B", "")],
            },
        ],
    }
}

/// How the dynamic config entry for [`ROUND_ID`] attests its ballot.
#[expect(
    clippy::large_enum_variant,
    reason = "test fixture built once per test"
)]
pub(super) enum Attestation {
    /// A v2 entry with no ballot attestation, as published before this change.
    Absent,
    /// A ballot attestation over `ballot`, signed by `signer`.
    Signed { ballot: Ballot, signer: SigningKey },
    /// A ballot attestation whose digest field is not 32 lowercase-hex bytes.
    MalformedDigest,
    /// A ballot attestation whose signature is not valid base64.
    UndecodableSignature,
    /// A ballot attestation missing its required `ballot_digest` field.
    MissingDigest,
}

fn round_signature(signer: &SigningKey, payload: &[u8]) -> serde_json::Value {
    serde_json::json!({
        "key_id": "k1",
        "alg": "ed25519",
        "sig": BASE64.encode(signer.sign(payload).to_bytes()),
    })
}

fn round_id_bytes(round_id: &str) -> [u8; 32] {
    hex::decode(round_id).unwrap().try_into().unwrap()
}

fn round_entry(round_id: &str, attestation: Attestation) -> serde_json::Value {
    let v2_payload = RoundAuthPayloadV2::new(round_id_bytes(round_id), EA_PK, layout()).to_bytes();
    let mut entry = serde_json::json!({
        "auth_version": 2,
        "ea_pk": BASE64.encode(EA_PK),
        "signatures": [round_signature(&trusted_key(), &v2_payload)],
    });
    match attestation {
        Attestation::Absent => {}
        Attestation::Signed { ballot, signer } => {
            let digest = ballot.digest();
            let v3_payload = RoundAuthPayloadV3::new(
                round_id_bytes(round_id),
                EA_PK,
                layout(),
                *digest.as_bytes(),
            )
            .to_bytes();
            entry["ballot_attestation"] = serde_json::json!({
                "ballot_digest": digest.to_hex(),
                "signatures": [round_signature(&signer, &v3_payload)],
            });
        }
        Attestation::MalformedDigest => {
            entry["ballot_attestation"] = serde_json::json!({
                "ballot_digest": "not-hex",
                "signatures": [],
            });
        }
        Attestation::UndecodableSignature => {
            entry["ballot_attestation"] = serde_json::json!({
                "ballot_digest": "00".repeat(32),
                "signatures": [{"key_id": "k1", "alg": "ed25519", "sig": "not base64!"}],
            });
        }
        Attestation::MissingDigest => {
            entry["ballot_attestation"] = serde_json::json!({ "signatures": [] });
        }
    }
    entry
}

fn dynamic_config_bytes(rounds: Vec<(&str, Attestation)>) -> Vec<u8> {
    let rounds: serde_json::Map<String, serde_json::Value> = rounds
        .into_iter()
        .map(|(round_id, attestation)| (round_id.to_string(), round_entry(round_id, attestation)))
        .collect();
    let layout = layout();
    serde_json::json!({
        "config_version": 1,
        "vote_servers": [{"url": "https://vote.example.com", "label": "vote"}],
        "pir_endpoints": [{"url": "https://pir.example.com", "label": "pir"}],
        "pir_layout": {
            "pir_depth": layout.pir_depth,
            "tier0_layers": layout.tier0_layers,
            "tier1_layers": layout.tier1_layers,
            "poly_len": layout.poly_len
        },
        "supported_versions": {
            "pir": ["v0"],
            "vote_protocol": "v0",
            "tally": "v0",
            "vote_server": "v1"
        },
        "rounds": rounds
    })
    .to_string()
    .into_bytes()
}

fn static_config_bytes(require_ballot_attestation: bool) -> Vec<u8> {
    let mut config = serde_json::json!({
        "static_config_version": 2,
        "dynamic_config_urls": ["https://a.example.com/dynamic.json"],
        "trusted_keys": [{
            "key_id": "k1",
            "alg": "ed25519",
            "pubkey": BASE64.encode(trusted_key().verifying_key().to_bytes()),
            "notes": null
        }]
    });
    if require_ballot_attestation {
        config["require_ballot_attestation"] = serde_json::json!(true);
    }
    config.to_string().into_bytes()
}

/// Resolves a config whose single round, [`ROUND_ID`], carries `attestation`.
pub(super) fn resolved_config(attestation: Attestation) -> ResolvedVotingConfig {
    resolved_config_with_rounds(vec![(ROUND_ID, attestation)])
}

/// Resolves a config with one entry per `(round_id, attestation)`.
pub(super) fn resolved_config_with_rounds(
    rounds: Vec<(&str, Attestation)>,
) -> ResolvedVotingConfig {
    resolve_with_static(false, rounds)
}

/// Resolves a config whose static trust anchor requires ballot attestations.
pub(super) fn resolved_config_requiring_attestation(
    rounds: Vec<(&str, Attestation)>,
) -> ResolvedVotingConfig {
    resolve_with_static(true, rounds)
}

fn resolve_with_static(
    require_ballot_attestation: bool,
    rounds: Vec<(&str, Attestation)>,
) -> ResolvedVotingConfig {
    let resolved_static = resolve_static_voting_config(
        "https://example.com/static.json",
        &static_config_bytes(require_ballot_attestation),
    )
    .unwrap();
    assert_eq!(
        resolved_static.require_ballot_attestation,
        require_ballot_attestation
    );
    resolve_dynamic_voting_config(
        resolved_static,
        &dynamic_config_bytes(rounds),
        ResolveVotingConfigOptions::default(),
    )
    .unwrap()
}
