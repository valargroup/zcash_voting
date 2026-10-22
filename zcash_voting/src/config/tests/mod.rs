//! Compatibility tests for voting config resolution: static v1/v2 documents,
//! dynamic documents, round authentication, mirror fallback and switch
//! classification. They drive the public API, so they also pin the behavior
//! kept across the move onto `zakura-service-config`.

use super::*;
use crate::round_auth::RoundAuthPayloadV2;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signer, SigningKey};
use std::time::Duration;
use zakura_service_config::fingerprint_bytes;

const ROUND_ID: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const ROUND_ID_2: &str = "0000000000000000000000000000000000000000000000000000000000000002";

fn source() -> String {
    "https://example.com/static.json".to_string()
}

fn static_bytes(signing_key: &SigningKey) -> Vec<u8> {
    let pubkey = signing_key.verifying_key().to_bytes();
    serde_json::json!({
        "static_config_version": 1,
        "dynamic_config_url": "https://example.com/dynamic.json",
        "trusted_keys": [{
            "key_id": "k1",
            "alg": "ed25519",
            "pubkey": BASE64.encode(pubkey),
            "notes": null
        }]
    })
    .to_string()
    .into_bytes()
}

fn round_auth_v2_preimage(round_id: &str, ea_pk: &[u8], layout: PirLayout) -> Vec<u8> {
    let round_id = hex::decode(round_id).unwrap().try_into().unwrap();
    let ea_pk = ea_pk.try_into().unwrap();
    RoundAuthPayloadV2::new(round_id, ea_pk, layout).to_bytes()
}

fn test_pir_layout() -> PirLayout {
    PirLayout {
        pir_depth: 19,
        tier0_layers: 12,
        tier1_layers: 7,
        poly_len: 4096,
    }
}

fn static_bytes_v2(signing_key: &SigningKey, urls: &[&str]) -> Vec<u8> {
    let pubkey = signing_key.verifying_key().to_bytes();
    serde_json::json!({
        "static_config_version": 2,
        "dynamic_config_urls": urls,
        "trusted_keys": [{
            "key_id": "k1",
            "alg": "ed25519",
            "pubkey": BASE64.encode(pubkey),
            "notes": null
        }]
    })
    .to_string()
    .into_bytes()
}

const MIRROR_A: &str = "https://a.example.com/dynamic.json";
const MIRROR_B: &str = "https://b.example.com/dynamic.json";

fn dynamic_bytes_with_layout_and_round_signers(
    layout: PirLayout,
    round_signers: &[(&str, &SigningKey)],
) -> Vec<u8> {
    let mut rounds = serde_json::Map::new();
    for (round_id, signing_key) in round_signers {
        let ea_pk = [7u8; 32];
        let sig = signing_key
            .sign(&round_auth_v2_preimage(round_id, &ea_pk, layout))
            .to_bytes();
        rounds.insert(
            (*round_id).to_string(),
            serde_json::json!({
                "auth_version": 2,
                "ea_pk": BASE64.encode(ea_pk),
                "signatures": [{
                    "key_id": "k1",
                    "alg": "ed25519",
                    "sig": BASE64.encode(sig)
                }]
            }),
        );
    }
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

fn dynamic_bytes_with_round_signers(round_signers: &[(&str, &SigningKey)]) -> Vec<u8> {
    dynamic_bytes_with_layout_and_round_signers(test_pir_layout(), round_signers)
}

fn dynamic_bytes(signing_key: &SigningKey) -> Vec<u8> {
    dynamic_bytes_with_round_signers(&[(ROUND_ID, signing_key)])
}

fn resolve_test_dynamic(
    signing_key: &SigningKey,
    dynamic_bytes: &[u8],
) -> Result<ResolvedVotingConfig, VotingConfigError> {
    let resolved_static =
        resolve_static_voting_config(&source(), &static_bytes(signing_key)).unwrap();
    resolve_dynamic_voting_config(
        resolved_static,
        dynamic_bytes,
        ResolveVotingConfigOptions::default(),
    )
}

#[test]
fn static_resolution_verifies_hash_pin_and_exposes_dynamic_url() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let bytes = static_bytes(&signing_key);
    let hash = fingerprint_bytes(&bytes);
    let source = format!("{}?checksum=sha256:{}", source(), hash);

    let resolved = resolve_static_voting_config(&source, &bytes).unwrap();

    assert_eq!(
        resolved.dynamic_config_url,
        "https://example.com/dynamic.json"
    );
    assert_eq!(resolved.source.url, "https://example.com/static.json");
}

#[test]
fn static_resolution_reports_remote_authentication_for_hash_mismatch() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let bytes = static_bytes(&signing_key);
    let source = format!("{}?checksum=sha256:{}", source(), "00".repeat(32));

    let err = resolve_static_voting_config(&source, &bytes).unwrap_err();

    assert!(matches!(
        err,
        VotingConfigError::RemoteAuthenticationFailed { .. }
    ));
    assert!(err.to_string().contains("remote authentication failed"));
}

#[test]
fn dynamic_resolution_verifies_round_signatures() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let resolved_static =
        resolve_static_voting_config(&source(), &static_bytes(&signing_key)).unwrap();

    let resolved = resolve_dynamic_voting_config(
        resolved_static,
        &dynamic_bytes(&signing_key),
        ResolveVotingConfigOptions::default(),
    )
    .unwrap();

    assert_eq!(
        resolved.authenticated_rounds,
        vec![AuthenticatedRound {
            round_id: ROUND_ID.to_string(),
            ea_pk: vec![7u8; 32],
        }]
    );
    assert!(resolved.skipped_round_ids.is_empty());
}

#[test]
fn dynamic_resolution_exposes_fixed_width_pir_layout_on_wire_surface() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);

    let resolved = resolve_test_dynamic(&signing_key, &dynamic_bytes(&signing_key)).unwrap();
    let wire_layout: crate::wire::PirLayout = resolved.pir_layout;
    let json = serde_json::to_value(wire_layout).unwrap();

    assert_eq!(
        resolved.pir_layout,
        PirLayout {
            pir_depth: 19,
            tier0_layers: 12,
            tier1_layers: 7,
            poly_len: 4096,
        }
    );
    assert_eq!(
        json,
        serde_json::json!({
            "pir_depth": 19,
            "tier0_layers": 12,
            "tier1_layers": 7,
            "poly_len": 4096,
        })
    );
}

#[test]
fn dynamic_resolution_requires_pir_layout() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let mut dynamic: serde_json::Value =
        serde_json::from_slice(&dynamic_bytes(&signing_key)).unwrap();
    dynamic.as_object_mut().unwrap().remove("pir_layout");

    let err =
        resolve_test_dynamic(&signing_key, &serde_json::to_vec(&dynamic).unwrap()).unwrap_err();

    assert!(matches!(err, VotingConfigError::DecodeFailed { .. }));
    assert!(err.to_string().contains("missing field `pir_layout`"));
}

#[test]
fn dynamic_resolution_rejects_pir_layout_values_outside_u32() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let mut dynamic: serde_json::Value =
        serde_json::from_slice(&dynamic_bytes(&signing_key)).unwrap();
    dynamic["pir_layout"]["pir_depth"] = serde_json::json!(u64::from(u32::MAX) + 1);

    let err =
        resolve_test_dynamic(&signing_key, &serde_json::to_vec(&dynamic).unwrap()).unwrap_err();

    assert!(matches!(err, VotingConfigError::DecodeFailed { .. }));
    assert!(err.to_string().contains("dynamic config decode failed"));
}

#[test]
fn dynamic_resolution_rejects_inconsistent_pir_layout() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let mut dynamic: serde_json::Value =
        serde_json::from_slice(&dynamic_bytes(&signing_key)).unwrap();
    dynamic["pir_layout"]["tier1_layers"] = serde_json::json!(8);

    let err =
        resolve_test_dynamic(&signing_key, &serde_json::to_vec(&dynamic).unwrap()).unwrap_err();

    assert!(matches!(err, VotingConfigError::DecodeFailed { .. }));
    assert!(err
        .to_string()
        .contains("PIR layout is inconsistent: pir_depth 19 != tier0_layers 12 + tier1_layers 8"));
}

#[test]
fn dynamic_resolution_rejects_pir_layout_above_shared_tier_limit() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let mut dynamic: serde_json::Value =
        serde_json::from_slice(&dynamic_bytes(&signing_key)).unwrap();
    dynamic["pir_layout"] = serde_json::json!({
        "pir_depth": 29,
        "tier0_layers": 17,
        "tier1_layers": 12,
    "poly_len": 4096
    });

    let err =
        resolve_test_dynamic(&signing_key, &serde_json::to_vec(&dynamic).unwrap()).unwrap_err();

    assert!(matches!(err, VotingConfigError::DecodeFailed { .. }));
    assert!(err
        .to_string()
        .contains("PIR layout Tier 0 layers 17 exceeds maximum 16"));
}

#[test]
fn dynamic_resolution_rejects_pir_depth_outside_circuit_range() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let mut dynamic: serde_json::Value =
        serde_json::from_slice(&dynamic_bytes(&signing_key)).unwrap();
    dynamic["pir_layout"] = serde_json::json!({
        "pir_depth": 30,
        "tier0_layers": 15,
        "tier1_layers": 15,
    "poly_len": 4096
    });

    let err =
        resolve_test_dynamic(&signing_key, &serde_json::to_vec(&dynamic).unwrap()).unwrap_err();

    assert!(matches!(err, VotingConfigError::DecodeFailed { .. }));
    assert!(err
        .to_string()
        .contains("unsupported PIR layout depth 30; expected 1..=29"));
}

#[test]
fn dynamic_resolution_rejects_pir_layouts_unusable_by_ypir() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    for (layout, expected) in [
        (
            serde_json::json!({
                "pir_depth": 19,
                "tier0_layers": 10,
                "tier1_layers": 9,
            "poly_len": 4096
            }),
            "Tier 1 rows 1024 below YPIR minimum 2048",
        ),
        (
            serde_json::json!({
                "pir_depth": 19,
                "tier0_layers": 14,
                "tier1_layers": 5,
            "poly_len": 4096
            }),
            "Tier 1 item bits 24576 below YPIR minimum 28672",
        ),
        (
            serde_json::json!({
                "pir_depth": 19,
                "tier0_layers": 0,
                "tier1_layers": 19,
            "poly_len": 4096
            }),
            "PIR layout tiers must be non-zero",
        ),
    ] {
        let mut dynamic: serde_json::Value =
            serde_json::from_slice(&dynamic_bytes(&signing_key)).unwrap();
        dynamic["pir_layout"] = layout;

        let err =
            resolve_test_dynamic(&signing_key, &serde_json::to_vec(&dynamic).unwrap()).unwrap_err();

        assert!(matches!(err, VotingConfigError::DecodeFailed { .. }));
        assert!(err.to_string().contains(expected), "{err}");
    }
}

#[test]
fn dynamic_resolution_rejects_pir_layouts_exceeding_shared_tier_limits() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    for (layout, expected) in [
        (
            serde_json::json!({
                "pir_depth": 29,
                "tier0_layers": 23,
                "tier1_layers": 6,
            "poly_len": 4096
            }),
            "PIR layout Tier 0 layers 23 exceeds maximum 16",
        ),
        (
            serde_json::json!({
                "pir_depth": 29,
                "tier0_layers": 11,
                "tier1_layers": 18,
            "poly_len": 4096
            }),
            "PIR layout Tier 1 layers 18 exceeds maximum 15",
        ),
    ] {
        let mut dynamic: serde_json::Value =
            serde_json::from_slice(&dynamic_bytes(&signing_key)).unwrap();
        dynamic["pir_layout"] = layout;

        let err =
            resolve_test_dynamic(&signing_key, &serde_json::to_vec(&dynamic).unwrap()).unwrap_err();

        assert!(matches!(err, VotingConfigError::DecodeFailed { .. }));
        assert!(err.to_string().contains(expected), "{err}");
    }
}

#[test]
fn dynamic_resolution_accepts_layouts_supported_by_shared_predicate() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    for (tier0_layers, tier1_layers, poly_len) in
        [(11, 8, 2048), (13, 6, 4096), (16, 11, 2048), (11, 15, 4096)]
    {
        let layout = PirLayout {
            pir_depth: tier0_layers + tier1_layers,
            tier0_layers,
            tier1_layers,
            poly_len,
        };
        let dynamic =
            dynamic_bytes_with_layout_and_round_signers(layout, &[(ROUND_ID, &signing_key)]);

        let resolved = resolve_test_dynamic(&signing_key, &dynamic).unwrap();

        assert_eq!(resolved.pir_layout, layout);
        assert_eq!(resolved.authenticated_rounds.len(), 1);
        assert!(resolved.skipped_round_ids.is_empty());
    }
}

#[test]
fn dynamic_resolution_skips_rounds_with_bad_signature() {
    let trusted_key = SigningKey::from_bytes(&[3u8; 32]);
    let bad_key = SigningKey::from_bytes(&[4u8; 32]);
    let resolved_static =
        resolve_static_voting_config(&source(), &static_bytes(&trusted_key)).unwrap();

    let resolved = resolve_dynamic_voting_config(
        resolved_static,
        &dynamic_bytes(&bad_key),
        ResolveVotingConfigOptions::default(),
    )
    .unwrap();

    assert!(resolved.authenticated_rounds.is_empty());
    assert_eq!(resolved.skipped_round_ids, vec![ROUND_ID.to_string()]);
}

#[test]
fn static_document_cannot_downgrade_round_signatures_to_transport_trust() {
    // Voting has one attestation method, round signatures, and it is the
    // default. A static document naming another method is not a selection
    // voting honors: unsigned rounds are still skipped.
    let trusted_key = SigningKey::from_bytes(&[3u8; 32]);
    let bad_key = SigningKey::from_bytes(&[4u8; 32]);
    let mut static_json: serde_json::Value =
        serde_json::from_slice(&static_bytes(&trusted_key)).unwrap();
    static_json["attestation"] = serde_json::json!({ "method": "https-transport" });
    let resolved_static =
        resolve_static_voting_config(&source(), static_json.to_string().as_bytes()).unwrap();

    let resolved = resolve_dynamic_voting_config(
        resolved_static,
        &dynamic_bytes(&bad_key),
        ResolveVotingConfigOptions::default(),
    )
    .unwrap();

    assert!(resolved.authenticated_rounds.is_empty());
    assert_eq!(resolved.skipped_round_ids, vec![ROUND_ID.to_string()]);
}

#[test]
fn dynamic_resolution_accepts_vote_sdk_signed_round_entry() {
    // Golden vector for the layout-bound v2 preimage including poly_len
    // 4096 (tag || round_id || ea_pk || 19/12/7/4096), signed with Ed25519
    // seed [3u8;32] (the same trusted key as static_bytes).
    let trusted_key = SigningKey::from_bytes(&[3u8; 32]);
    let mut dynamic: serde_json::Value =
        serde_json::from_slice(&dynamic_bytes(&trusted_key)).unwrap();
    dynamic["rounds"][ROUND_ID] = serde_json::json!({
        "auth_version": 2,
        "ea_pk": "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
        "signatures": [{
            "key_id": "k1",
            "alg": "ed25519",
            "sig": "IJHHSQaV6x0PVgLL+NBZiOGjDtVdt+A298LVLwqiarhWbEIFUJUGH6qfozLwypuo+3+YFn6HfRG/LM4WXM1MBg=="
        }]
    });

    let resolved =
        resolve_test_dynamic(&trusted_key, &serde_json::to_vec(&dynamic).unwrap()).unwrap();

    assert_eq!(
        resolved.authenticated_rounds,
        vec![AuthenticatedRound {
            round_id: ROUND_ID.to_string(),
            ea_pk: vec![7u8; 32],
        }]
    );
    assert!(resolved.skipped_round_ids.is_empty());
}

#[test]
fn dynamic_resolution_accepts_vote_sdk_ui_signed_round_entry() {
    // Golden from vote-sdk admin UI / Keplr-derived key over the
    // poly_len-bound v2 preimage (tag || round_id || ea_pk || 19/12/7/4096).
    const UI_ROUND_ID: &str = "06aae723e42cf615d174f338e8f30a72d2bf3275eb9d9e835cc894f197904b20";
    const UI_KEY_ID: &str = "keplr:sv1mqts0klc9768rns9h2ykeaka5tve6ts39c2zu3";
    const UI_EA_PK: &str = "GpYa1sCGIMe2bp1O9UgrThrwkCdxu6oHDmhoBTw6EZ8=";
    const UI_SIG: &str =
        "RHbpnj2a1VA+wadIQT3JM/r6ADH11VeA8UgT5dhwhixMcS5Bw5ispndM/ZYH/d2vxNBxTRtZwnLyXZjxcVD+Dg==";
    const UI_PUBKEY: &str = "NDygCpG+Y4T4uu8M1Sb/YG+74lUVj9XgYypUoMQMXT8=";
    let static_config = serde_json::json!({
        "static_config_version": 1,
        "dynamic_config_url": "https://example.com/dynamic.json",
        "trusted_keys": [{
            "key_id": UI_KEY_ID,
            "alg": "ed25519",
            "pubkey": UI_PUBKEY,
            "notes": "derived key for sv1mqts0klc9768rns9h2ykeaka5tve6ts39c2zu3"
        }]
    })
    .to_string();
    let resolved_static =
        resolve_static_voting_config(&source(), static_config.as_bytes()).unwrap();

    let fixture_key = SigningKey::from_bytes(&[3u8; 32]);
    let mut dynamic: serde_json::Value =
        serde_json::from_slice(&dynamic_bytes(&fixture_key)).unwrap();
    dynamic["rounds"] = serde_json::json!({
        UI_ROUND_ID: {
            "auth_version": 2,
            "ea_pk": UI_EA_PK,
            "signatures": [{
                "key_id": UI_KEY_ID,
                "alg": "ed25519",
                "sig": UI_SIG
            }]
        }
    });

    let resolved = resolve_dynamic_voting_config(
        resolved_static,
        &serde_json::to_vec(&dynamic).unwrap(),
        ResolveVotingConfigOptions::default(),
    )
    .unwrap();

    assert_eq!(
        resolved.authenticated_rounds,
        vec![AuthenticatedRound {
            round_id: UI_ROUND_ID.to_string(),
            ea_pk: BASE64.decode(UI_EA_PK).unwrap(),
        }]
    );
    assert!(resolved.skipped_round_ids.is_empty());
}

#[test]
fn dynamic_resolution_skips_legacy_auth_version_1_rounds() {
    let trusted_key = SigningKey::from_bytes(&[3u8; 32]);
    let ea_pk = [7u8; 32];
    // A valid v1-style signature over the raw ea_pk bytes must no longer
    // authenticate, regardless of the advertised auth_version.
    let raw_sig = trusted_key.sign(&ea_pk).to_bytes();
    let mut dynamic: serde_json::Value =
        serde_json::from_slice(&dynamic_bytes(&trusted_key)).unwrap();
    dynamic["rounds"][ROUND_ID] = serde_json::json!({
        "auth_version": 1,
        "ea_pk": BASE64.encode(ea_pk),
        "signatures": [{
            "key_id": "k1",
            "alg": "ed25519",
            "sig": BASE64.encode(raw_sig)
        }]
    });

    let resolved =
        resolve_test_dynamic(&trusted_key, &serde_json::to_vec(&dynamic).unwrap()).unwrap();

    assert!(resolved.authenticated_rounds.is_empty());
    assert_eq!(resolved.skipped_round_ids, vec![ROUND_ID.to_string()]);
}

#[test]
fn dynamic_resolution_skips_round_entry_replayed_under_different_round_id() {
    let trusted_key = SigningKey::from_bytes(&[3u8; 32]);
    let mut dynamic: serde_json::Value =
        serde_json::from_slice(&dynamic_bytes(&trusted_key)).unwrap();
    // Copy the validly signed entry for ROUND_ID verbatim under ROUND_ID_2:
    // the signature binds the round id, so the replayed entry must fail.
    let entry = dynamic["rounds"][ROUND_ID].clone();
    dynamic["rounds"][ROUND_ID_2] = entry;

    let resolved =
        resolve_test_dynamic(&trusted_key, &serde_json::to_vec(&dynamic).unwrap()).unwrap();

    assert_eq!(
        resolved.authenticated_rounds,
        vec![AuthenticatedRound {
            round_id: ROUND_ID.to_string(),
            ea_pk: vec![7u8; 32],
        }]
    );
    assert_eq!(resolved.skipped_round_ids, vec![ROUND_ID_2.to_string()]);
}

#[test]
fn dynamic_resolution_skips_rounds_when_pir_layout_changed_after_signing() {
    let trusted_key = SigningKey::from_bytes(&[3u8; 32]);
    let mut dynamic: serde_json::Value =
        serde_json::from_slice(&dynamic_bytes(&trusted_key)).unwrap();
    // Entries were signed over layout 19/12/7. A config host swapping the
    // advertised layout must invalidate every round signature.
    dynamic["pir_layout"] = serde_json::json!({
        "pir_depth": 19,
        "tier0_layers": 11,
        "tier1_layers": 8,
    "poly_len": 4096
    });

    let resolved =
        resolve_test_dynamic(&trusted_key, &serde_json::to_vec(&dynamic).unwrap()).unwrap();

    assert!(resolved.authenticated_rounds.is_empty());
    assert_eq!(resolved.skipped_round_ids, vec![ROUND_ID.to_string()]);
}

#[test]
fn dynamic_resolution_requires_poly_len() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let mut dynamic: serde_json::Value =
        serde_json::from_slice(&dynamic_bytes(&signing_key)).unwrap();
    dynamic["pir_layout"]
        .as_object_mut()
        .unwrap()
        .remove("poly_len");

    let err =
        resolve_test_dynamic(&signing_key, &serde_json::to_vec(&dynamic).unwrap()).unwrap_err();

    assert!(matches!(err, VotingConfigError::DecodeFailed { .. }));
    assert!(err.to_string().contains("missing field `poly_len`"));
}

#[test]
fn dynamic_resolution_rejects_unsupported_poly_len() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let mut dynamic: serde_json::Value =
        serde_json::from_slice(&dynamic_bytes(&signing_key)).unwrap();
    dynamic["pir_layout"]["poly_len"] = serde_json::json!(1024);

    let err =
        resolve_test_dynamic(&signing_key, &serde_json::to_vec(&dynamic).unwrap()).unwrap_err();

    assert!(matches!(err, VotingConfigError::DecodeFailed { .. }));
    assert!(
        err.to_string()
            .contains("unsupported PIR layout poly_len 1024"),
        "{err}"
    );
}

#[test]
fn dynamic_resolution_skips_rounds_when_poly_len_changed_after_signing() {
    let trusted_key = SigningKey::from_bytes(&[3u8; 32]);
    let mut dynamic: serde_json::Value =
        serde_json::from_slice(&dynamic_bytes(&trusted_key)).unwrap();
    // Entries were signed over poly_len 4096.
    dynamic["pir_layout"]["poly_len"] = serde_json::json!(2048);

    let resolved =
        resolve_test_dynamic(&trusted_key, &serde_json::to_vec(&dynamic).unwrap()).unwrap();

    assert!(resolved.authenticated_rounds.is_empty());
    assert_eq!(resolved.skipped_round_ids, vec![ROUND_ID.to_string()]);
}

#[test]
fn dynamic_resolution_partitions_authenticated_and_skipped_rounds() {
    let trusted_key = SigningKey::from_bytes(&[3u8; 32]);
    let bad_key = SigningKey::from_bytes(&[4u8; 32]);
    let resolved_static =
        resolve_static_voting_config(&source(), &static_bytes(&trusted_key)).unwrap();

    let resolved = resolve_dynamic_voting_config(
        resolved_static,
        &dynamic_bytes_with_round_signers(&[(ROUND_ID, &trusted_key), (ROUND_ID_2, &bad_key)]),
        ResolveVotingConfigOptions::default(),
    )
    .unwrap();

    assert_eq!(
        resolved.authenticated_rounds,
        vec![AuthenticatedRound {
            round_id: ROUND_ID.to_string(),
            ea_pk: vec![7u8; 32],
        }]
    );
    assert_eq!(resolved.skipped_round_ids, vec![ROUND_ID_2.to_string()]);
}

#[test]
fn summary_fingerprint_ignores_skipped_round_ids() {
    let base = ResolvedVotingConfig {
        source_fingerprint: "src".to_string(),
        trusted_key_fingerprint: "keys".to_string(),
        dynamic_config_fingerprint: "dyn".to_string(),
        vote_servers: vec![ServiceEndpoint {
            url: "https://vote.example.com".to_string(),
            label: "vote".to_string(),
        }],
        pir_endpoints: vec![ServiceEndpoint {
            url: "https://pir.example.com".to_string(),
            label: "pir".to_string(),
        }],
        pir_layout: PirLayout {
            pir_depth: 19,
            tier0_layers: 12,
            tier1_layers: 7,
            poly_len: 4096,
        },
        supported_versions: SupportedVersions {
            pir: vec!["v0".to_string()],
            vote_protocol: "v0".to_string(),
            tally: "v0".to_string(),
            vote_server: "v1".to_string(),
        },
        authenticated_rounds: vec![AuthenticatedRound {
            round_id: ROUND_ID.to_string(),
            ea_pk: vec![7u8; 32],
        }],
        skipped_round_ids: vec![ROUND_ID_2.to_string()],
        conditions: vec![],
    };
    let mut with_different_skips = base.clone();
    with_different_skips.skipped_round_ids = vec!["f".repeat(64)];

    let summary_base = ResolvedVotingConfigSummary::from(&base);
    let summary_with_different_skips = ResolvedVotingConfigSummary::from(&with_different_skips);
    assert_eq!(
        summary_base.authenticated_round_set_fingerprint,
        summary_with_different_skips.authenticated_round_set_fingerprint
    );
}

fn resolved_config_with_authenticated_round(ea_pk: Vec<u8>) -> ResolvedVotingConfig {
    ResolvedVotingConfig {
        source_fingerprint: "src".to_string(),
        trusted_key_fingerprint: "keys".to_string(),
        dynamic_config_fingerprint: "dyn".to_string(),
        vote_servers: vec![],
        pir_endpoints: vec![],
        pir_layout: PirLayout {
            pir_depth: 19,
            tier0_layers: 12,
            tier1_layers: 7,
            poly_len: 4096,
        },
        supported_versions: SupportedVersions {
            pir: vec!["v0".to_string()],
            vote_protocol: "v0".to_string(),
            tally: "v0".to_string(),
            vote_server: "v1".to_string(),
        },
        authenticated_rounds: vec![AuthenticatedRound {
            round_id: ROUND_ID.to_string(),
            ea_pk,
        }],
        skipped_round_ids: vec![],
        conditions: vec![],
    }
}

#[test]
fn trusted_voting_round_params_uses_authenticated_ea_pk() {
    let config = resolved_config_with_authenticated_round(vec![7u8; 32]);

    let params = config
        .trusted_voting_round_params(ROUND_ID.to_string(), 123, vec![2u8; 32], vec![3u8; 32])
        .unwrap();

    assert_eq!(params.vote_round_id, ROUND_ID);
    assert_eq!(params.snapshot_height, 123);
    assert_eq!(params.ea_pk, vec![7u8; 32]);
    assert_eq!(params.nc_root, vec![2u8; 32]);
    assert_eq!(params.nullifier_imt_root, vec![3u8; 32]);
}

#[test]
fn trusted_voting_round_params_rejects_unauthenticated_round() {
    let config = resolved_config_with_authenticated_round(vec![7u8; 32]);

    let err = config
        .trusted_voting_round_params("f".repeat(64), 123, vec![2u8; 32], vec![3u8; 32])
        .unwrap_err();
    assert!(matches!(
        err,
        VotingConfigError::RemoteAuthenticationFailed { .. }
    ));
}

#[test]
fn trusted_voting_round_params_rejects_invalid_nc_root_length() {
    let config = resolved_config_with_authenticated_round(vec![7u8; 32]);

    let err = config
        .trusted_voting_round_params(ROUND_ID.to_string(), 123, vec![2u8; 31], vec![3u8; 32])
        .unwrap_err();

    assert!(matches!(err, VotingConfigError::InvalidInput { .. }));
    assert!(err.to_string().contains("nc_root must be exactly"));
}

#[test]
fn trusted_voting_round_params_rejects_invalid_nullifier_imt_root_length() {
    let config = resolved_config_with_authenticated_round(vec![7u8; 32]);

    let err = config
        .trusted_voting_round_params(ROUND_ID.to_string(), 123, vec![2u8; 32], vec![3u8; 31])
        .unwrap_err();

    assert!(matches!(err, VotingConfigError::InvalidInput { .. }));
    assert!(err
        .to_string()
        .contains("nullifier_imt_root must be exactly"));
}

#[test]
fn trusted_voting_round_params_rejects_invalid_authenticated_ea_pk_length() {
    let config = resolved_config_with_authenticated_round(vec![7u8; 31]);

    let err = config
        .trusted_voting_round_params(ROUND_ID.to_string(), 123, vec![2u8; 32], vec![3u8; 32])
        .unwrap_err();

    assert!(matches!(err, VotingConfigError::InvalidInput { .. }));
    assert!(err.to_string().contains("has invalid ea_pk length"));
}

#[test]
fn resolve_dynamic_voting_config_resolves_dynamic_bytes() {
    let trusted_key = SigningKey::from_bytes(&[3u8; 32]);
    let static_config = static_bytes(&trusted_key);
    let source_url = format!(
        "{}?checksum=sha256:{}",
        source(),
        fingerprint_bytes(&static_config)
    );
    let dynamic_config = dynamic_bytes(&trusted_key);
    let resolved_static = resolve_static_voting_config(&source_url, &static_config).unwrap();

    let resolved = resolve_dynamic_voting_config(
        resolved_static,
        &dynamic_config,
        ResolveVotingConfigOptions::default(),
    )
    .unwrap();

    assert_eq!(
        resolved.authenticated_rounds,
        vec![AuthenticatedRound {
            round_id: ROUND_ID.to_string(),
            ea_pk: vec![7u8; 32],
        }]
    );
}

#[test]
fn resolve_dynamic_voting_config_rejects_unsupported_vote_protocol() {
    let trusted_key = SigningKey::from_bytes(&[3u8; 32]);
    let mut dynamic: serde_json::Value =
        serde_json::from_slice(&dynamic_bytes(&trusted_key)).unwrap();
    dynamic["supported_versions"]["vote_protocol"] = serde_json::json!("v2");
    let resolved_static =
        resolve_static_voting_config(&source(), &static_bytes(&trusted_key)).unwrap();

    let error = resolve_dynamic_voting_config(
        resolved_static,
        &dynamic.to_string().into_bytes(),
        ResolveVotingConfigOptions::default(),
    )
    .unwrap_err();

    assert_eq!(
        error,
        VotingConfigError::UnsupportedVersion {
            component: "vote_protocol".to_string(),
            advertised: "v2".to_string(),
        }
    );
}

#[test]
fn resolve_dynamic_voting_config_accepts_both_vote_protocol_generations() {
    // One wallet build has to span the rollout: deployed configs
    // advertise v0 today and will advertise v1 later, and neither may
    // strand it.
    for advertised in ["v0", "v1"] {
        let trusted_key = SigningKey::from_bytes(&[3u8; 32]);
        let mut dynamic: serde_json::Value =
            serde_json::from_slice(&dynamic_bytes(&trusted_key)).unwrap();
        dynamic["supported_versions"]["vote_protocol"] = serde_json::json!(advertised);
        let resolved_static =
            resolve_static_voting_config(&source(), &static_bytes(&trusted_key)).unwrap();

        let resolved = resolve_dynamic_voting_config(
            resolved_static,
            &dynamic.to_string().into_bytes(),
            ResolveVotingConfigOptions::default(),
        );

        assert!(
            resolved.is_ok(),
            "vote_protocol {advertised} was rejected: {:?}",
            resolved.err()
        );
    }
}

#[test]
fn config_switch_for_pir_change_is_service_update() {
    let versions = SupportedVersions {
        pir: vec!["v0".to_string()],
        vote_protocol: "v0".to_string(),
        tally: "v0".to_string(),
        vote_server: "v1".to_string(),
    };
    let current = ResolvedVotingConfigSummary {
        trusted_key_fingerprint: "a".to_string(),
        vote_server_fingerprint: "b".to_string(),
        pir_endpoint_fingerprint: "c".to_string(),
        pir_layout: PirLayout::UNKNOWN,
        authenticated_round_set_fingerprint: "d".to_string(),
        protocol_versions: versions.clone(),
    };
    let mut next = current.clone();
    next.pir_endpoint_fingerprint = "changed".to_string();

    let decision = decide_config_switch(Some(current), next);

    assert_eq!(decision.kind, ConfigSwitchKind::SameChainServiceUpdate);
}

#[test]
fn legacy_summary_deserializes_and_known_layout_is_service_update() {
    let legacy_json = serde_json::json!({
        "trusted_key_fingerprint": "same-keys",
        "vote_server_fingerprint": "same-vote-servers",
        "pir_endpoint_fingerprint": "same-pir-endpoints",
        "authenticated_round_set_fingerprint": "same-rounds",
        "protocol_versions": {
            "pir": ["v0"],
            "vote_protocol": "v0",
            "tally": "v0",
            "vote_server": "v1",
        },
    });
    let current: ResolvedVotingConfigSummary =
        serde_json::from_value(legacy_json).expect("legacy summary remains readable");
    assert_eq!(current.pir_layout, PirLayout::UNKNOWN);

    let mut next = current.clone();
    next.pir_layout = PirLayout {
        pir_depth: 19,
        tier0_layers: 12,
        tier1_layers: 7,
        poly_len: 4096,
    };

    let decision = decide_config_switch(Some(current), next);

    assert_eq!(decision.kind, ConfigSwitchKind::SameChainServiceUpdate);
}

#[test]
fn pre_poly_len_summary_deserializes_and_known_layout_is_service_update() {
    let legacy_json = serde_json::json!({
        "trusted_key_fingerprint": "same-keys",
        "vote_server_fingerprint": "same-vote-servers",
        "pir_endpoint_fingerprint": "same-pir-endpoints",
        "pir_layout": {
            "pir_depth": 19,
            "tier0_layers": 12,
            "tier1_layers": 7,
        },
        "authenticated_round_set_fingerprint": "same-rounds",
        "protocol_versions": {
            "pir": ["v0"],
            "vote_protocol": "v0",
            "tally": "v0",
            "vote_server": "v1",
        },
    });
    let current: ResolvedVotingConfigSummary =
        serde_json::from_value(legacy_json).expect("pre-poly_len summary remains readable");
    assert_eq!(
        current.pir_layout,
        PirLayout {
            pir_depth: 19,
            tier0_layers: 12,
            tier1_layers: 7,
            poly_len: 0,
        }
    );

    let mut next = current.clone();
    next.pir_layout.poly_len = 4096;

    let decision = decide_config_switch(Some(current), next);

    assert_eq!(decision.kind, ConfigSwitchKind::SameChainServiceUpdate);
}

#[test]
fn config_switch_for_known_pir_layout_change_is_service_update() {
    let current = ResolvedVotingConfigSummary {
        trusted_key_fingerprint: "same-keys".to_string(),
        vote_server_fingerprint: "same-vote-servers".to_string(),
        pir_endpoint_fingerprint: "same-pir-endpoints".to_string(),
        pir_layout: PirLayout {
            pir_depth: 19,
            tier0_layers: 12,
            tier1_layers: 7,
            poly_len: 4096,
        },
        authenticated_round_set_fingerprint: "same-rounds".to_string(),
        protocol_versions: SupportedVersions {
            pir: vec!["v0".to_string()],
            vote_protocol: "v0".to_string(),
            tally: "v0".to_string(),
            vote_server: "v1".to_string(),
        },
    };
    let mut next = current.clone();
    next.pir_layout = PirLayout {
        pir_depth: 20,
        tier0_layers: 13,
        tier1_layers: 7,
        poly_len: 4096,
    };

    let decision = decide_config_switch(Some(current), next);

    assert_eq!(decision.kind, ConfigSwitchKind::SameChainServiceUpdate);
}

#[test]
fn config_switch_for_round_set_change_is_new_chain_or_round() {
    let versions = SupportedVersions {
        pir: vec!["v0".to_string()],
        vote_protocol: "v0".to_string(),
        tally: "v0".to_string(),
        vote_server: "v1".to_string(),
    };
    let current = ResolvedVotingConfigSummary {
        trusted_key_fingerprint: "same-keys".to_string(),
        vote_server_fingerprint: "same-chain".to_string(),
        pir_endpoint_fingerprint: "same-pir".to_string(),
        pir_layout: PirLayout::UNKNOWN,
        authenticated_round_set_fingerprint: "rounds-a".to_string(),
        protocol_versions: versions.clone(),
    };
    let mut next = current.clone();
    next.authenticated_round_set_fingerprint = "rounds-b".to_string();

    let decision = decide_config_switch(Some(current), next);

    assert_eq!(decision.kind, ConfigSwitchKind::NewChainOrRound);
}

#[test]
fn config_switch_for_vote_server_change_is_service_update() {
    let versions = SupportedVersions {
        pir: vec!["v0".to_string()],
        vote_protocol: "v0".to_string(),
        tally: "v0".to_string(),
        vote_server: "v1".to_string(),
    };
    let current = ResolvedVotingConfigSummary {
        trusted_key_fingerprint: "same-keys".to_string(),
        vote_server_fingerprint: "servers-a".to_string(),
        pir_endpoint_fingerprint: "same-pir".to_string(),
        pir_layout: PirLayout::UNKNOWN,
        authenticated_round_set_fingerprint: "same-rounds".to_string(),
        protocol_versions: versions.clone(),
    };
    let mut next = current.clone();
    next.vote_server_fingerprint = "servers-b".to_string();

    let decision = decide_config_switch(Some(current), next);

    assert_eq!(decision.kind, ConfigSwitchKind::SameChainServiceUpdate);
}

#[test]
fn config_switch_for_trusted_key_change_is_service_update() {
    let versions = SupportedVersions {
        pir: vec!["v0".to_string()],
        vote_protocol: "v0".to_string(),
        tally: "v0".to_string(),
        vote_server: "v1".to_string(),
    };
    let current = ResolvedVotingConfigSummary {
        trusted_key_fingerprint: "keys-a".to_string(),
        vote_server_fingerprint: "same-servers".to_string(),
        pir_endpoint_fingerprint: "same-pir".to_string(),
        pir_layout: PirLayout::UNKNOWN,
        authenticated_round_set_fingerprint: "same-rounds".to_string(),
        protocol_versions: versions.clone(),
    };
    let mut next = current.clone();
    next.trusted_key_fingerprint = "keys-b".to_string();

    let decision = decide_config_switch(Some(current), next);

    assert_eq!(decision.kind, ConfigSwitchKind::SameChainServiceUpdate);
}

#[test]
fn config_switch_for_protocol_change_wins_over_other_changes() {
    let current = ResolvedVotingConfigSummary {
        trusted_key_fingerprint: "keys-a".to_string(),
        vote_server_fingerprint: "servers-a".to_string(),
        pir_endpoint_fingerprint: "pir-a".to_string(),
        pir_layout: PirLayout::UNKNOWN,
        authenticated_round_set_fingerprint: "rounds-a".to_string(),
        protocol_versions: SupportedVersions {
            pir: vec!["v0".to_string()],
            vote_protocol: "v0".to_string(),
            tally: "v0".to_string(),
            vote_server: "v1".to_string(),
        },
    };
    let mut next = current.clone();
    next.authenticated_round_set_fingerprint = "rounds-b".to_string();
    next.protocol_versions.vote_protocol = "v1".to_string();

    let decision = decide_config_switch(Some(current), next);

    assert_eq!(decision.kind, ConfigSwitchKind::ProtocolChanged);
}

#[test]
fn config_switch_for_unchanged_summary_is_unchanged() {
    let current = ResolvedVotingConfigSummary {
        trusted_key_fingerprint: "keys-a".to_string(),
        vote_server_fingerprint: "servers-a".to_string(),
        pir_endpoint_fingerprint: "pir-a".to_string(),
        pir_layout: PirLayout::UNKNOWN,
        authenticated_round_set_fingerprint: "rounds-a".to_string(),
        protocol_versions: SupportedVersions {
            pir: vec!["v0".to_string()],
            vote_protocol: "v0".to_string(),
            tally: "v0".to_string(),
            vote_server: "v1".to_string(),
        },
    };

    let decision = decide_config_switch(Some(current.clone()), current);

    assert_eq!(decision.kind, ConfigSwitchKind::Unchanged);
}

// --- static config v1 / v2 schema ---

#[test]
fn static_v1_resolution_exposes_single_mirror_list() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let resolved = resolve_static_voting_config(&source(), &static_bytes(&signing_key)).unwrap();

    assert_eq!(resolved.static_config_version, 1);
    assert_eq!(
        resolved.dynamic_config_url,
        "https://example.com/dynamic.json"
    );
    assert_eq!(
        resolved.dynamic_config_urls,
        vec!["https://example.com/dynamic.json".to_string()]
    );
}

#[test]
fn static_v2_resolution_preserves_mirror_order() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let resolved = resolve_static_voting_config(
        &source(),
        &static_bytes_v2(&signing_key, &[MIRROR_A, MIRROR_B]),
    )
    .unwrap();

    assert_eq!(resolved.static_config_version, 2);
    assert_eq!(resolved.dynamic_config_url, MIRROR_A);
    assert_eq!(
        resolved.dynamic_config_urls,
        vec![MIRROR_A.to_string(), MIRROR_B.to_string()]
    );
}

#[test]
fn static_v2_resolution_rejects_empty_mirror_list() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let error =
        resolve_static_voting_config(&source(), &static_bytes_v2(&signing_key, &[])).unwrap_err();

    assert!(
        matches!(&error, VotingConfigError::DecodeFailed { message } if message.contains("at least one entry")),
        "unexpected error: {error:?}"
    );
}

#[test]
fn static_v2_resolution_rejects_singular_dynamic_config_url() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let pubkey = signing_key.verifying_key().to_bytes();
    let bytes = serde_json::json!({
        "static_config_version": 2,
        "dynamic_config_url": MIRROR_A,
        "dynamic_config_urls": [MIRROR_A],
        "trusted_keys": [{
            "key_id": "k1", "alg": "ed25519",
            "pubkey": BASE64.encode(pubkey), "notes": null
        }]
    })
    .to_string()
    .into_bytes();

    let error = resolve_static_voting_config(&source(), &bytes).unwrap_err();

    assert!(
        matches!(&error, VotingConfigError::DecodeFailed { message } if message.contains("must not set dynamic_config_url")),
        "unexpected error: {error:?}"
    );
}

#[test]
fn static_v1_resolution_rejects_mirror_list() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let pubkey = signing_key.verifying_key().to_bytes();
    let bytes = serde_json::json!({
        "static_config_version": 1,
        "dynamic_config_url": MIRROR_A,
        "dynamic_config_urls": [MIRROR_A, MIRROR_B],
        "trusted_keys": [{
            "key_id": "k1", "alg": "ed25519",
            "pubkey": BASE64.encode(pubkey), "notes": null
        }]
    })
    .to_string()
    .into_bytes();

    let error = resolve_static_voting_config(&source(), &bytes).unwrap_err();

    assert!(
        matches!(&error, VotingConfigError::DecodeFailed { message } if message.contains("must not set dynamic_config_urls")),
        "unexpected error: {error:?}"
    );
}

#[test]
fn static_v2_resolution_rejects_duplicate_mirrors() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let error = resolve_static_voting_config(
        &source(),
        &static_bytes_v2(&signing_key, &[MIRROR_A, MIRROR_A]),
    )
    .unwrap_err();

    assert!(
        matches!(&error, VotingConfigError::DecodeFailed { message } if message.contains("duplicate")),
        "unexpected error: {error:?}"
    );
}

#[test]
fn static_v2_resolution_rejects_non_https_mirror() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let error = resolve_static_voting_config(
        &source(),
        &static_bytes_v2(
            &signing_key,
            &[MIRROR_A, "http://b.example.com/dynamic.json"],
        ),
    )
    .unwrap_err();

    assert!(
        matches!(&error, VotingConfigError::InvalidInput { message } if message.contains("dynamic_config_urls[1]")),
        "unexpected error: {error:?}"
    );
}

#[test]
fn static_resolution_rejects_unknown_version() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let pubkey = signing_key.verifying_key().to_bytes();
    let bytes = serde_json::json!({
        "static_config_version": 3,
        "dynamic_config_urls": [MIRROR_A],
        "trusted_keys": [{
            "key_id": "k1", "alg": "ed25519",
            "pubkey": BASE64.encode(pubkey), "notes": null
        }]
    })
    .to_string()
    .into_bytes();

    let error = resolve_static_voting_config(&source(), &bytes).unwrap_err();

    assert_eq!(
        error,
        VotingConfigError::DecodeFailed {
            message: "unsupported static_config_version 3".to_string()
        }
    );
}

/// The real `prod/v2-static-voting-config.json` published by
/// token-holder-voting-config, so a publisher-side shape change that this
/// crate cannot parse fails here rather than in a wallet.
#[test]
fn static_v2_resolution_accepts_published_prod_document() {
    let bytes = br#"{
  "static_config_version": 2,
  "dynamic_config_urls": [
"https://voting.valargroup.dev/prod/dynamic-voting-config.json",
"https://raw.githubusercontent.com/valargroup/token-holder-voting-config/main/prod/dynamic-voting-config.json"
  ],
  "trusted_keys": [
{
  "key_id": "valargroup",
  "alg": "ed25519",
  "pubkey": "8oQiUWq6QDGnAgRw6U3YhnXb6JLFSXauYnWHIFmJRcw=",
  "notes": "Vote-manager Keplr-derived key for sv1wyf8tuys2ussdqwc6ugnvq0x273j8wq8fm3jrj"
},
{
  "key_id": "tachyon",
  "alg": "ed25519",
  "pubkey": "F8L2S+sjsQMwsZr03ebovvbVRd2B8BXx5e8KpTBmxHs=",
  "notes": "Vote-manager Keplr-derived key for sv1zd8mc9mx85zgarx692w38n8t2g2f6r92ajwhth"
},
{
  "key_id": "zodl",
  "alg": "ed25519",
  "pubkey": "b7aj3HFKqKAF/gwIcf01KXqvDN91ww759pjAxN8whBk=",
  "notes": "Vote-manager Keplr-derived key for ZODL prod vote manager; sv1jpxuakysz65rzg9kn90xg4m4vpyer6np9n7c0t"
},
{
  "key_id": "shielded-labs",
  "alg": "ed25519",
  "pubkey": "Lsn7flRsI5udUOwqK8ShWu1+jU08AkP0/ed8ihj46kE=",
  "notes": "derived key for sv169hpcstyc9qal5t2hu2y6xlqjjvc303cpdanam"
}
  ]
}"#;

    let resolved = resolve_static_voting_config(&source(), bytes).unwrap();

    assert_eq!(resolved.static_config_version, 2);
    assert_eq!(resolved.dynamic_config_urls.len(), 2);
    assert_eq!(
        resolved.dynamic_config_url,
        "https://voting.valargroup.dev/prod/dynamic-voting-config.json"
    );
}

#[test]
fn unpinned_source_reports_hash_pin_condition_as_unverified() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let resolved = resolve_test_dynamic(&signing_key, &dynamic_bytes(&signing_key)).unwrap();

    let condition = resolved
        .conditions
        .iter()
        .find(|c| c.kind == ConfigConditionKind::StaticHashPinVerified)
        .expect("hash pin condition");
    assert!(!condition.status);
}

// --- dynamic config mirror fallback ---

fn resolved_static_v2(signing_key: &SigningKey) -> ResolvedStaticVotingConfig {
    resolve_static_voting_config(
        &source(),
        &static_bytes_v2(signing_key, &[MIRROR_A, MIRROR_B]),
    )
    .unwrap()
}

#[test]
fn mirror_fallback_skips_unreachable_mirror() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);

    let (resolved, skipped) = resolve_dynamic_voting_config_from_attempts(
        resolved_static_v2(&signing_key),
        vec![
            DynamicConfigAttempt::failed(MIRROR_A, "dns lookup failed"),
            DynamicConfigAttempt::fetched(MIRROR_B, dynamic_bytes(&signing_key)),
        ],
        ResolveVotingConfigOptions::default(),
    )
    .unwrap();

    assert_eq!(resolved.authenticated_rounds.len(), 1);
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].url, MIRROR_A);
    assert!(skipped[0].reason.contains("dns lookup failed"));
    assert!(resolved
        .conditions
        .iter()
        .any(|c| c.kind == ConfigConditionKind::DynamicMirrorFallbackUsed));
}

#[test]
fn mirror_fallback_skips_undecodable_mirror() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);

    let (resolved, skipped) = resolve_dynamic_voting_config_from_attempts(
        resolved_static_v2(&signing_key),
        vec![
            DynamicConfigAttempt::fetched(MIRROR_A, b"<html>504</html>".to_vec()),
            DynamicConfigAttempt::fetched(MIRROR_B, dynamic_bytes(&signing_key)),
        ],
        ResolveVotingConfigOptions::default(),
    )
    .unwrap();

    assert_eq!(resolved.authenticated_rounds.len(), 1);
    assert_eq!(skipped.len(), 1);
    assert!(skipped[0].reason.contains("decode failed"));
}

/// A mirror that decodes but authenticates nothing is deprioritized, so a
/// later mirror carrying a verifiable round set wins.
#[test]
fn mirror_fallback_prefers_mirror_authenticating_rounds() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let untrusted_key = SigningKey::from_bytes(&[4u8; 32]);

    let (resolved, skipped) = resolve_dynamic_voting_config_from_attempts(
        resolved_static_v2(&signing_key),
        vec![
            DynamicConfigAttempt::fetched(MIRROR_A, dynamic_bytes(&untrusted_key)),
            DynamicConfigAttempt::fetched(MIRROR_B, dynamic_bytes(&signing_key)),
        ],
        ResolveVotingConfigOptions::default(),
    )
    .unwrap();

    assert_eq!(resolved.authenticated_rounds.len(), 1);
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].url, MIRROR_A);
    assert!(skipped[0].reason.contains("authenticated no rounds"));
}

/// A round-less resolution must never be downgraded into an error: it is a
/// valid config, and prod resolves this way today.
#[test]
fn mirror_fallback_returns_round_less_config_when_no_mirror_has_rounds() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let untrusted_key = SigningKey::from_bytes(&[4u8; 32]);

    let (resolved, skipped) = resolve_dynamic_voting_config_from_attempts(
        resolved_static_v2(&signing_key),
        vec![
            DynamicConfigAttempt::fetched(MIRROR_A, dynamic_bytes(&untrusted_key)),
            DynamicConfigAttempt::fetched(MIRROR_B, dynamic_bytes(&untrusted_key)),
        ],
        ResolveVotingConfigOptions::default(),
    )
    .unwrap();

    assert!(resolved.authenticated_rounds.is_empty());
    assert_eq!(resolved.skipped_round_ids, vec![ROUND_ID.to_string()]);
    // Mirror A won, so nothing was passed over ahead of it.
    assert!(skipped.is_empty());
    assert!(!resolved
        .conditions
        .iter()
        .any(|c| c.kind == ConfigConditionKind::DynamicMirrorFallbackUsed));
}

/// The v1 single-mirror path must behave exactly as it did before mirror
/// support existed, including for a config that authenticates no rounds.
#[test]
fn single_round_less_mirror_matches_direct_resolution() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let untrusted_key = SigningKey::from_bytes(&[4u8; 32]);
    let dynamic = dynamic_bytes(&untrusted_key);
    let resolved_static =
        resolve_static_voting_config(&source(), &static_bytes(&signing_key)).unwrap();

    let (via_attempts, skipped) = resolve_dynamic_voting_config_from_attempts(
        resolved_static.clone(),
        vec![DynamicConfigAttempt::fetched(
            &resolved_static.dynamic_config_url,
            dynamic.clone(),
        )],
        ResolveVotingConfigOptions::default(),
    )
    .unwrap();
    let direct = resolve_dynamic_voting_config(
        resolved_static,
        &dynamic,
        ResolveVotingConfigOptions::default(),
    )
    .unwrap();

    assert!(skipped.is_empty());
    assert_eq!(via_attempts, direct);
}

#[test]
fn mirror_fallback_reports_every_mirror_when_all_fail() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);

    let error = resolve_dynamic_voting_config_from_attempts(
        resolved_static_v2(&signing_key),
        vec![
            DynamicConfigAttempt::failed(MIRROR_A, "connection refused"),
            DynamicConfigAttempt::fetched(MIRROR_B, b"not json".to_vec()),
        ],
        ResolveVotingConfigOptions::default(),
    )
    .unwrap_err();

    let message = error.to_string();
    assert!(message.contains(MIRROR_A), "missing mirror a: {message}");
    assert!(message.contains(MIRROR_B), "missing mirror b: {message}");
    assert!(message.contains("connection refused"), "{message}");
}

#[test]
fn single_mirror_fallback_matches_direct_resolution() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let dynamic = dynamic_bytes(&signing_key);
    let resolved_static =
        resolve_static_voting_config(&source(), &static_bytes(&signing_key)).unwrap();

    let (via_attempts, skipped) = resolve_dynamic_voting_config_from_attempts(
        resolved_static.clone(),
        vec![DynamicConfigAttempt::fetched(
            &resolved_static.dynamic_config_url,
            dynamic.clone(),
        )],
        ResolveVotingConfigOptions::default(),
    )
    .unwrap();
    let direct = resolve_dynamic_voting_config(
        resolved_static,
        &dynamic,
        ResolveVotingConfigOptions::default(),
    )
    .unwrap();

    assert!(skipped.is_empty());
    assert_eq!(via_attempts, direct);
    assert!(!via_attempts
        .conditions
        .iter()
        .any(|c| c.kind == ConfigConditionKind::DynamicMirrorFallbackUsed));
}

/// An unsupported-version failure must not swallow the other mirrors: it
/// is the one error kind with no message field of its own, so the
/// enumeration is the only place its detail can survive.
#[test]
fn mirror_fallback_preserves_detail_when_last_mirror_is_unsupported_version() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let mut doc: serde_json::Value = serde_json::from_slice(&dynamic_bytes(&signing_key)).unwrap();
    doc["supported_versions"]["vote_server"] = serde_json::json!("v99");
    let unsupported = doc.to_string().into_bytes();

    let error = resolve_dynamic_voting_config_from_attempts(
        resolved_static_v2(&signing_key),
        vec![
            DynamicConfigAttempt::failed(MIRROR_A, "connection refused"),
            DynamicConfigAttempt::fetched(MIRROR_B, unsupported),
        ],
        ResolveVotingConfigOptions::default(),
    )
    .unwrap_err();

    let VotingConfigError::AllMirrorsFailed { message } = &error else {
        panic!("unexpected error: {error:?}");
    };
    // The unreachable mirror survives alongside the version failure.
    assert!(message.contains(MIRROR_A), "{message}");
    assert!(message.contains("connection refused"), "{message}");
    // And the version failure keeps its component and advertised value.
    assert!(message.contains(MIRROR_B), "{message}");
    assert!(
        message.contains("unsupported version for vote_server: v99"),
        "{message}"
    );
}

/// A single mirror has nothing to enumerate, so its own error kind must
/// survive rather than being flattened into `AllMirrorsFailed`.
#[test]
fn single_mirror_failure_reports_underlying_error_verbatim() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let mut doc: serde_json::Value = serde_json::from_slice(&dynamic_bytes(&signing_key)).unwrap();
    doc["supported_versions"]["vote_server"] = serde_json::json!("v99");
    let unsupported = doc.to_string().into_bytes();
    let resolved_static =
        resolve_static_voting_config(&source(), &static_bytes(&signing_key)).unwrap();

    let via_attempts = resolve_dynamic_voting_config_from_attempts(
        resolved_static.clone(),
        vec![DynamicConfigAttempt::fetched(
            &resolved_static.dynamic_config_url,
            unsupported.clone(),
        )],
        ResolveVotingConfigOptions::default(),
    )
    .unwrap_err();
    let direct = resolve_dynamic_voting_config(
        resolved_static,
        &unsupported,
        ResolveVotingConfigOptions::default(),
    )
    .unwrap_err();

    assert_eq!(via_attempts, direct);
    assert_eq!(
        via_attempts,
        VotingConfigError::UnsupportedVersion {
            component: "vote_server".to_string(),
            advertised: "v99".to_string(),
        }
    );
}

/// A v1 / one-URL fetch failure must surface the transport cause, not a
/// bare "dynamic config fetch failed" that drops DNS or HTTP detail.
#[test]
fn single_mirror_fetch_failure_preserves_transport_error() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let resolved_static =
        resolve_static_voting_config(&source(), &static_bytes(&signing_key)).unwrap();

    let error = resolve_dynamic_voting_config_from_attempts(
        resolved_static.clone(),
        vec![DynamicConfigAttempt::failed(
            &resolved_static.dynamic_config_url,
            "dns error: no such host",
        )],
        ResolveVotingConfigOptions::default(),
    )
    .unwrap_err();

    assert_eq!(
        error,
        VotingConfigError::RemoteAuthenticationFailed {
            message: "dynamic config fetch failed: dns error: no such host".to_string(),
        }
    );
}

#[test]
fn mirror_fallback_rejects_empty_attempts() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);

    let error = resolve_dynamic_voting_config_from_attempts(
        resolved_static_v2(&signing_key),
        Vec::new(),
        ResolveVotingConfigOptions::default(),
    )
    .unwrap_err();

    assert!(
        matches!(&error, VotingConfigError::InvalidInput { message } if message.contains("at least one entry")),
        "unexpected error: {error:?}"
    );
}

/// A primary that never answers must not block a healthy secondary: each
/// mirror attempt is bounded, and the timed-out URL is skipped like any
/// other transport failure.
#[tokio::test(start_paused = true)]
async fn stalled_primary_mirror_times_out_and_falls_back() {
    let signing_key = SigningKey::from_bytes(&[3u8; 32]);
    let healthy = dynamic_bytes(&signing_key);
    let timeout = Duration::from_secs(5);

    let (resolved, skipped) = resolve_dynamic_voting_config_over_mirrors(
        resolved_static_v2(&signing_key),
        timeout,
        ResolveVotingConfigOptions::default(),
        |url| {
            let healthy = healthy.clone();
            async move {
                if url == MIRROR_A {
                    std::future::pending::<Result<Vec<u8>, String>>().await
                } else {
                    Ok(healthy)
                }
            }
        },
    )
    .await
    .unwrap();

    assert_eq!(resolved.authenticated_rounds.len(), 1);
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].url, MIRROR_A);
    assert!(
        skipped[0].reason.contains("timed out after 5s"),
        "unexpected skip reason: {}",
        skipped[0].reason
    );
    assert!(resolved
        .conditions
        .iter()
        .any(|c| c.kind == ConfigConditionKind::DynamicMirrorFallbackUsed));
}
