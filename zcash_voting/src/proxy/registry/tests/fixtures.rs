//! Registry fixtures built the way the chain and the publishers build them:
//! entries encoded as the chain stores them and a wallet list signed with a
//! real Ed25519 key.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};
use zcash_vote_delegate::{
    DelegateKey, DelegateSigningKey, DelegateStatus, IdentityProvider, PinnedListKey,
    ProxyDelegateEntry, VerifiedWalletList, WalletList, WalletListRow,
};

use crate::proxy::registry::{
    ProvenRecord, ProvenValue, RegistryCache, RegistryPage, RegistryProofRequest, WalletListPin,
};

/// The vote chain the fixtures describe.
pub(super) const CHAIN_ID: &str = "zvote-1";
/// The round under test, a canonical round id.
pub(super) const ROUND_ID: &str =
    "0101010101010101010101010101010101010101010101010101010101010101";
/// When the fixture list was signed.
pub(super) const ISSUED_AT: u64 = 1_790_000_000;
/// The wallet the fixture list belongs to.
pub(super) const WALLET_ID: &str = "vizor";
/// The `key_id` the wallet pins.
pub(super) const PINNED_KEY_ID: &str = "valargroup";

/// Delegate key number `seed`.
pub(super) fn dk(seed: u8) -> DelegateKey {
    DelegateSigningKey::from_secret_seed([seed; 32]).delegate_key()
}

/// The active X entry `index` of `handle`, with key number `index`.
pub(super) fn entry(index: u32, handle: &str) -> ProxyDelegateEntry {
    ProxyDelegateEntry {
        index,
        dk: dk(index as u8),
        provider: IdentityProvider::X,
        handle: handle.to_owned(),
        proof_id: format!("17900000000000000{index:02}"),
        status: DelegateStatus::Active,
        status_reason: 0,
        registered_height: 10,
    }
}

/// `entry` with `status`.
pub(super) fn with_status(entry: ProxyDelegateEntry, status: DelegateStatus) -> ProxyDelegateEntry {
    ProxyDelegateEntry { status, ..entry }
}

/// `entry` proven present at its own index.
pub(super) fn proven(entry: &ProxyDelegateEntry) -> ProvenRecord {
    entry_proof(entry.index, ProvenValue::Present(entry.encode()))
}

/// The proof of entry `index`.
pub(super) fn entry_proof(index: u32, value: ProvenValue) -> ProvenRecord {
    ProvenRecord {
        request: RegistryProofRequest {
            delegate_index: index,
        },
        value,
    }
}

/// A cache holding `entries`.
pub(super) fn cache(entries: Vec<ProxyDelegateEntry>) -> RegistryCache {
    RegistryCache::from_pages(
        CHAIN_ID,
        [RegistryPage {
            entries,
            next_start_after: None,
        }],
    )
}

/// A WebP container of 20 bytes; only its structure is valid.
pub(super) fn picture() -> Vec<u8> {
    let mut picture = b"RIFF".to_vec();
    picture.extend_from_slice(&12u32.to_le_bytes());
    picture.extend_from_slice(b"WEBPVP8L");
    picture.extend_from_slice(&[0; 4]);
    picture
}

/// The key that signs the wallet's list.
pub(super) fn list_signer() -> SigningKey {
    SigningKey::from_bytes(&[9; 32])
}

/// What the fixture wallet pins on [`CHAIN_ID`].
pub(super) fn pin() -> WalletListPin {
    WalletListPin {
        wallet_id: WALLET_ID.to_owned(),
        chain_id: CHAIN_ID.to_owned(),
        keys: vec![PinnedListKey {
            key_id: PINNED_KEY_ID.to_owned(),
            public_key: list_signer().verifying_key().to_bytes(),
        }],
    }
}

/// The wallet list row that describes `entry`.
pub(super) fn row(entry: &ProxyDelegateEntry) -> WalletListRow {
    WalletListRow {
        index: entry.index,
        provider: entry.provider.code(),
        handle: entry.handle.clone(),
        dk: entry.dk.encode(),
        name: format!("Delegate {}", entry.index),
        statement: Some(format!("Statement of {}.", entry.index)),
        picture: Some(picture()),
    }
}

/// Version `version` of the wallet's list.
pub(super) fn wallet_list(
    version: u64,
    rows: Vec<WalletListRow>,
    reserved_names: &[&str],
) -> WalletList {
    WalletList {
        wallet_id: WALLET_ID.to_owned(),
        chain_id: CHAIN_ID.to_owned(),
        version,
        issued_at: ISSUED_AT,
        reserved_names: reserved_names
            .iter()
            .map(|name| (*name).to_owned())
            .collect(),
        delegates: rows,
    }
}

/// `list` as its signer publishes it, signed by `signer` under `key_id`.
pub(super) fn list_document(list: &WalletList, signer: &SigningKey, key_id: &str) -> Vec<u8> {
    let payload = list.payload().unwrap();
    let rows: Vec<Value> = list
        .delegates
        .iter()
        .map(|row| {
            let mut value = json!({
                "index": row.index,
                "provider": row.provider,
                "handle": row.handle,
                "dk": row.dk,
                "name": row.name,
            });
            if let Some(statement) = &row.statement {
                value["statement"] = json!(statement);
            }
            if let Some(picture) = &row.picture {
                value["picture"] = json!(BASE64.encode(picture));
            }
            value
        })
        .collect();
    json!({
        "format": "zcash-vote-wallet-list/v1",
        "wallet_id": list.wallet_id,
        "chain_id": list.chain_id,
        "version": list.version,
        "issued_at": list.issued_at,
        "reserved_names": list.reserved_names,
        "delegates": rows,
        "signatures": [{
            "key_id": key_id,
            "alg": "ed25519",
            "sig": BASE64.encode(signer.sign(&payload).to_bytes()),
        }],
    })
    .to_string()
    .into_bytes()
}

/// `list` verified under [`pin`].
pub(super) fn verified(list: &WalletList) -> VerifiedWalletList {
    pin()
        .verify(&list_document(list, &list_signer(), PINNED_KEY_ID))
        .unwrap()
}
