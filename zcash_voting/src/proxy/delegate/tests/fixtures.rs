//! Delegate-mode fixtures: the shared golden vectors, an in-memory device
//! database, proven entries built as the chain encodes them, signed
//! attestations and chain answers. The keys are the vectors' keys: alice's
//! signs the X REGISTER and the RETIRE vectors, and bob's the BALLOT vectors.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::{json, Value};
use zcash_vote_delegate::{
    attest_digest, AttestationWindow, DelegateKey, DelegateRegisterFields, DelegateSigningKey,
    DelegateStatus, IdentityProvider, ProxyDelegateEntry,
};

use crate::{
    proxy::{
        delegate::{
            delegate_key_info, DelegateChain, DelegateRoundTiming, ProvenDelegateEntry,
            ProxyVerifier, ProxyVerifierSet,
        },
        registry::ProvenValue,
    },
    storage::VotingDb,
    types::Network,
    wire::{DelegateAttestation, DelegateAttestationSignature, ProxyDelegateProvider},
    ProposalRosterEntry,
};

/// The shared crate's client vector file, which every implementation tests
/// against.
pub(super) const PROXY_DELEGATION_VECTORS: &str =
    include_str!("../../../../../zcash_vote_delegate/test-vectors/proxy_delegation_v1.json");

/// The vote chain every vector signs for.
pub(super) const CHAIN_ID: &str = "zvote-1";
/// The wallet scope of the device's records.
pub(super) const WALLET_ID: &str = "delegate-mode";
/// The entry the RETIRE and BALLOT vectors name.
pub(super) const DELEGATE_INDEX: u32 = 17;
/// The secret seed of alice's key.
pub(super) const ALICE_SECRET: &str =
    "f535b15fe6b190b3f129317684a65cda55b5a91be34e9750d85b701822018c95";
/// The secret seed of bob's key.
pub(super) const BOB_SECRET: &str =
    "d7f1825dbab904ac7aab64c7ee9357be616184a3bd206f08eaa610d69880d8e2";
/// The vectors' verifier's secret seed.
pub(super) const VERIFIER_SECRET: &str =
    "a6b6929440b5794a711ce02178488fd78a7c62e3a9482124e7483556f31cabbb";
/// The vectors' second verifier's secret seed.
pub(super) const SECOND_VERIFIER_SECRET: &str =
    "6329fc9045ae3d79b900aa26a2c33585e2e1ca9d46c0d6b6b927edb71c1ab72f";
/// The post that carries alice's marker.
pub(super) const ALICE_PROOF_ID: &str = "1790000000000000000";
/// The ballot vectors' round.
pub(super) const VECTOR_ROUND_ID: &str =
    "c142ce6987b7d2b1133e86ebf059fc97f80c65d36825adc7febbef6694e17128";
/// A device time inside [`vector_window`].
pub(super) const ATTEST_NOW: u64 = 1_790_000_000;

/// The parsed vector file.
pub(super) fn vectors() -> Value {
    serde_json::from_str(PROXY_DELEGATION_VECTORS).expect("the vector file parses")
}

/// The case `name` of the vector file's `section`.
pub(super) fn vector_case(section: &str, name: &str) -> Value {
    vectors()[section]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == name)
        .unwrap_or_else(|| panic!("no vector {section}/{name}"))
        .clone()
}

/// Decodes a lowercase hex string of exactly `N` bytes.
pub(super) fn hex_bytes<const N: usize>(text: &str) -> [u8; N] {
    hex::decode(text)
        .expect("valid hex")
        .try_into()
        .expect("the expected byte length")
}

/// Decodes a hex vector field of exactly `N` bytes.
pub(super) fn vector_bytes<const N: usize>(value: &Value) -> [u8; N] {
    hex_bytes(value.as_str().expect("a hex string"))
}

/// The vectors' chain.
pub(super) fn chain() -> DelegateChain {
    DelegateChain::new(Network::Mainnet, CHAIN_ID).unwrap()
}

/// A fresh device database scoped to [`WALLET_ID`].
pub(super) fn db() -> VotingDb {
    let db = VotingDb::open_in_memory().unwrap();
    db.set_wallet_id(WALLET_ID);
    db
}

/// The signing key with secret seed `secret_hex`.
pub(super) fn signing_key(secret_hex: &str) -> DelegateSigningKey {
    DelegateSigningKey::from_secret_seed(hex_bytes(secret_hex))
}

/// The public key with secret seed `secret_hex`.
pub(super) fn key(secret_hex: &str) -> DelegateKey {
    signing_key(secret_hex).delegate_key()
}

/// Alice's key.
pub(super) fn alice_key() -> DelegateKey {
    key(ALICE_SECRET)
}

/// Bob's key.
pub(super) fn bob_key() -> DelegateKey {
    key(BOB_SECRET)
}

/// Entry `index` of `alice` on X with key `dk` and `status`, as the chain
/// stores it.
pub(super) fn entry(index: u32, dk: DelegateKey, status: DelegateStatus) -> ProxyDelegateEntry {
    ProxyDelegateEntry {
        index,
        dk,
        provider: IdentityProvider::X,
        handle: "alice".to_owned(),
        proof_id: ALICE_PROOF_ID.to_owned(),
        status,
        status_reason: if status == DelegateStatus::Suspended {
            4
        } else {
            0
        },
        registered_height: 1_000,
    }
}

/// `entry` proven present at its own index.
pub(super) fn proven(entry: &ProxyDelegateEntry) -> ProvenDelegateEntry {
    ProvenDelegateEntry::from_proof(entry.index, &ProvenValue::Present(entry.encode()))
        .unwrap()
        .unwrap()
}

/// Entry [`DELEGATE_INDEX`] with key `dk` and `status`, proven.
pub(super) fn proven_entry(dk: DelegateKey, status: DelegateStatus) -> ProvenDelegateEntry {
    proven(&entry(DELEGATE_INDEX, dk, status))
}

/// Alice's active entry [`DELEGATE_INDEX`], proven.
pub(super) fn alice_entry() -> ProvenDelegateEntry {
    proven_entry(alice_key(), DelegateStatus::Active)
}

/// The timing of round `round_id`, closing at `vote_end_time`.
pub(super) fn round_timing(round_id: [u8; 32], vote_end_time: u64) -> DelegateRoundTiming {
    DelegateRoundTiming {
        round_id,
        vote_end_time,
    }
}

/// A roster of `count` proposals with 8 options each.
pub(super) fn roster(count: u32) -> Vec<ProposalRosterEntry> {
    (1..=count)
        .map(|proposal_id| ProposalRosterEntry {
            proposal_id,
            num_options: 8,
        })
        .collect()
}

/// The vectors' attestation window: backdated 300 s, one hour long.
pub(super) fn vector_window() -> AttestationWindow {
    AttestationWindow {
        not_before: 1_789_999_700,
        expires_at: 1_790_003_600,
    }
}

/// The attester's answer for `dk` registering `alice` on X, with one
/// attestation per `(verifier_id, verifier secret)`, each over `window`.
pub(super) fn attestation(
    dk: &DelegateKey,
    signers: &[(u32, &str)],
    window: AttestationWindow,
) -> DelegateAttestation {
    let fields = DelegateRegisterFields {
        dk: dk.to_bytes(),
        provider: IdentityProvider::X,
        handle: "alice",
        proof_id: ALICE_PROOF_ID,
    };
    DelegateAttestation {
        key: delegate_key_info(dk),
        provider: ProxyDelegateProvider::X,
        handle: fields.handle.to_owned(),
        proof_id: fields.proof_id.to_owned(),
        attestations: signers
            .iter()
            .map(|(verifier_id, secret)| {
                let digest = attest_digest(CHAIN_ID, &fields, &window).unwrap();
                DelegateAttestationSignature {
                    verifier_id: *verifier_id,
                    not_before: window.not_before,
                    expires_at: window.expires_at,
                    sig: BASE64.encode(signing_key(secret).sign_digest(&digest)),
                }
            })
            .collect(),
    }
}

/// Alice's attestation by the vectors' first verifier over [`vector_window`],
/// which is the `attest/x_entry` vector.
pub(super) fn alice_attestation() -> DelegateAttestation {
    attestation(&alice_key(), &[(1, VERIFIER_SECRET)], vector_window())
}

/// The chain's verifier set: the vectors' two verifiers as ids 1 and 2.
pub(super) fn verifiers(threshold: u32) -> ProxyVerifierSet {
    ProxyVerifierSet {
        verifiers: vec![
            ProxyVerifier {
                verifier_id: 1,
                pubkey: key(VERIFIER_SECRET).to_bytes(),
            },
            ProxyVerifier {
                verifier_id: 2,
                pubkey: key(SECOND_VERIFIER_SECRET).to_bytes(),
            },
        ],
        threshold,
    }
}

/// The node's `BroadcastResult` answer body.
pub(super) fn broadcast_result(code: u32, codespace: &str, log: &str) -> Vec<u8> {
    json!({
        "tx_hash": if code == 0 { "AB".repeat(32) } else { String::new() },
        "code": code,
        "codespace": codespace,
        "log": log,
    })
    .to_string()
    .into_bytes()
}

/// One listed ballot as the whole-round list publishes it.
pub(super) fn listed_ballot(round_id: &[u8; 32], delegate_index: u32, options: &[u32]) -> Value {
    json!({
        "vote_round_id": BASE64.encode(round_id),
        "delegate_index": delegate_index,
        "options": options,
        "height": 1_234,
    })
}
