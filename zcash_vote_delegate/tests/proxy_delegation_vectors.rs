//! Checks the committed golden vectors, `test-vectors/proxy_delegation_v1.json`
//! (contracts-v1 §13).
//!
//! Every case's expected outputs are recomputed from its inputs through the
//! crate's public API, so the file cannot drift from the code, and its hash is
//! pinned. `rewrite_expected_outputs` rewrites the file after a contract
//! change; run it only through `make delegate-vectors`.

mod vectors;

use std::sync::LazyLock;

use serde_json::Value;
use sha2::{Digest, Sha256};
use vectors::{
    cases, decode_hex, hex32, hex_of, model::*, render, COMMITTED_VECTORS, VECTOR_FILE_PATH,
};
use zcash_vote_delegate::{
    verify_wallet_list, verify_zip215, PinnedListKey, ProxyDelegateEntry, WALLET_LIST_DOMAIN,
};

/// Environment variable that allows the writer test to rewrite the file.
const WRITE_VECTORS_ENV: &str = "ZCASH_VOTE_DELEGATE_WRITE_VECTORS";

/// The file's SHA-256, recorded in contracts-v1 §13.
const VECTORS_SHA256: &str = "4371f5db2a2b830cc3bcc198473cd3b2a4d18a2b71d67167e0fd84bf4c73490f";

/// The Pallas base field modulus p, big-endian.
const PALLAS_BASE_MODULUS: &str =
    "40000000000000000000000000000000224698fc094cf91b992d30ed00000001";

/// The committed file, parsed once.
static COMMITTED: LazyLock<VectorFile> =
    LazyLock::new(|| serde_json::from_str(COMMITTED_VECTORS).expect("the committed vectors parse"));

/// Asserts that `committed` equals `recomputed`, naming the first case or
/// field that differs. Every list of cases must be non-empty.
fn assert_same(path: &str, committed: &Value, recomputed: &Value) {
    match (committed, recomputed) {
        (Value::Array(committed), Value::Array(recomputed)) => {
            assert!(!committed.is_empty(), "{path} has no cases");
            assert_eq!(committed.len(), recomputed.len(), "{path}");
            for (committed, recomputed) in committed.iter().zip(recomputed) {
                let name = committed.get("name").and_then(Value::as_str).unwrap_or("-");
                assert_same(&format!("{path}/{name}"), committed, recomputed);
            }
        }
        (Value::Object(committed), Value::Object(recomputed))
            if !committed.contains_key("inputs") =>
        {
            assert!(committed.keys().eq(recomputed.keys()), "{path}");
            for (key, committed) in committed {
                assert_same(&format!("{path}/{key}"), committed, &recomputed[key]);
            }
        }
        _ => assert_eq!(committed, recomputed, "vector {path}"),
    }
}

/// Asserts that an Ed25519 signature from the file verifies under ZIP-215.
fn assert_signature(context: &str, public_key: &str, message: &[u8], signature: &str) {
    let signature: [u8; 64] = vectors::bytes(signature);
    verify_zip215(&hex32(public_key), message, &signature)
        .unwrap_or_else(|error| panic!("{context}: {error}"));
}

/// Every expected output recomputes from its inputs, and the domain and
/// error tables are the crate's.
#[test]
fn every_case_recomputes_from_its_inputs() {
    assert_same(
        "",
        &serde_json::to_value(&*COMMITTED).unwrap(),
        &serde_json::to_value(cases::recompute(&COMMITTED)).unwrap(),
    );
}

/// REGISTER, ATTEST, RETIRE and BALLOT signatures verify.
#[test]
fn registry_digest_signatures_verify() {
    let delegate_signed = (COMMITTED.register.iter())
        .map(|case| (&case.name, &case.expected))
        .chain((COMMITTED.retire.iter()).map(|case| (&case.name, &case.expected)))
        .chain((COMMITTED.ballot.iter()).map(|case| (&case.name, &case.expected)));
    for (name, expected) in delegate_signed {
        assert_signature(
            name,
            &expected.dk,
            &decode_hex(&expected.digest),
            &expected.dk_sig,
        );
    }
    for case in &COMMITTED.attest {
        let expected = &case.expected;
        assert_signature(
            &case.name,
            &expected.verifier_key,
            &decode_hex(&expected.digest),
            &expected.sig,
        );
    }
}

/// Both domain families are prefix-free, and the wallet list domain is longer
/// than round-auth v1's 32-byte payload.
#[test]
fn domains_are_prefix_free() {
    assert_eq!(COMMITTED.domains.existing.len(), 3);
    assert!(COMMITTED.domains.prefix_free);
    let config = &COMMITTED.config_domains;
    assert!(config.prefix_free);
    assert!(config
        .domains
        .iter()
        .any(|domain| domain.as_bytes() == WALLET_LIST_DOMAIN));
    assert!(WALLET_LIST_DOMAIN.len() > 32);
}

/// Every accepted entry value re-encodes byte for byte, and the section holds
/// each status.
#[test]
fn accepted_entry_values_reencode() {
    let mut statuses = Vec::new();
    for case in &COMMITTED.entry {
        let value = decode_hex(&case.inputs.value);
        if let Ok(entry) = ProxyDelegateEntry::decode(&value) {
            assert_eq!(entry.encode(), value, "{}", case.name);
            statuses.push(entry.status.code());
        }
    }
    statuses.sort_unstable();
    statuses.dedup();
    assert_eq!(statuses, [0, 1, 2]);
}

/// The DC-secret inputs that are Pallas base field elements, `round_id` and
/// `van_nullifier`, are canonical little-endian encodings, below p.
#[test]
fn dc_secret_field_inputs_are_canonical() {
    let modulus = hex32(PALLAS_BASE_MODULUS);
    for case in &COMMITTED.dc_secrets {
        let inputs = &case.inputs;
        for (field, value) in [
            ("round_id", &inputs.round_id),
            ("van_nullifier", &inputs.van_nullifier),
        ] {
            let mut big_endian = hex32(value);
            big_endian.reverse();
            assert!(
                big_endian < modulus,
                "vector dc_secrets/{}: {field} is not below p",
                case.name
            );
        }
    }
}

/// Wallet list signatures verify, each published document verifies to the
/// same payload and list, and the document section starts from the first
/// payload case's document.
#[test]
fn wallet_list_payloads_sign_and_verify() {
    for case in &COMMITTED.wallet_list_payload {
        let expected = &case.expected;
        let (Some(payload), Some(signature), Some(document)) =
            (&expected.payload, &expected.signature, &expected.document)
        else {
            continue;
        };
        let payload = decode_hex(payload);
        assert!(payload.starts_with(&WALLET_LIST_DOMAIN));
        assert_signature(&case.name, &expected.signer_key, &payload, signature);
        let pinned = [PinnedListKey {
            key_id: case.inputs.signer_key_id.clone(),
            public_key: hex32(&expected.signer_key),
        }];
        let verified = verify_wallet_list(
            document.as_bytes(),
            &case.inputs.wallet_id,
            &case.inputs.chain_id,
            &pinned,
        )
        .unwrap_or_else(|error| panic!("{}: {error}", case.name));
        assert_eq!(verified.payload(), payload, "{}", case.name);
        assert_eq!(
            verified.list(),
            &cases::wallet_list(&case.inputs),
            "{}",
            case.name
        );
    }
    assert_eq!(
        Some(&COMMITTED.wallet_list_document[0].inputs.document),
        COMMITTED.wallet_list_payload[0].expected.document.as_ref()
    );
}

/// Every error the file records is a bare name (contracts-v1 §13). The
/// recompute test cannot see Rust `Debug` text, which both sides would share.
#[test]
fn vector_error_names_are_bare() {
    /// Collects every string under an `error` or `…_error` key.
    fn collect_error_names<'a>(value: &'a Value, names: &mut Vec<&'a str>) {
        match value {
            Value::Object(fields) => {
                for (key, field) in fields {
                    if key.ends_with("error") {
                        names.extend(field.as_str());
                    }
                    collect_error_names(field, names);
                }
            }
            Value::Array(items) => {
                for item in items {
                    collect_error_names(item, names);
                }
            }
            _ => {}
        }
    }
    let committed: Value = serde_json::from_str(COMMITTED_VECTORS).unwrap();
    let mut names = Vec::new();
    collect_error_names(&committed, &mut names);
    assert!(!names.is_empty(), "the file records no errors");
    for name in names {
        assert!(
            !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_alphanumeric()),
            "error {name:?} is not a bare name"
        );
    }
}

/// The model reads and writes back every byte of the file, so no field
/// escapes the recompute test unseen.
#[test]
fn committed_vectors_round_trip_through_the_model() {
    assert_eq!(render(&*COMMITTED), COMMITTED_VECTORS);
}

/// The committed file is byte for byte the one contracts-v1 §13 records.
#[test]
fn committed_vectors_match_the_recorded_hash() {
    assert_eq!(
        hex_of(&Sha256::digest(COMMITTED_VECTORS)),
        VECTORS_SHA256,
        "the vector file changed; any change is a contract change"
    );
}

/// Rewrites every expected output in the vector file from its inputs. Run
/// through `make delegate-vectors` after editing inputs, then update
/// [`VECTORS_SHA256`] and review the diff as a contract change.
#[test]
#[ignore = "rewrites test-vectors/proxy_delegation_v1.json; run `make delegate-vectors`"]
fn rewrite_expected_outputs() {
    assert_eq!(
        std::env::var(WRITE_VECTORS_ENV).as_deref(),
        Ok("1"),
        "set {WRITE_VECTORS_ENV}=1 (or run `make delegate-vectors`) to rewrite the vectors"
    );
    std::fs::write(VECTOR_FILE_PATH, render(&cases::recompute(&COMMITTED)))
        .expect("write the vector file");
}
