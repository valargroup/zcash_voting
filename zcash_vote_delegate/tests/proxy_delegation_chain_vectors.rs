//! Checks the chain vector file, `test-vectors/proxy_delegation_chain_v1.json`,
//! copied byte for byte from vote-sdk's `testutil/testdata/` (contract §13).
//!
//! The file's `entry_value` section holds `ProxyDelegateEntry` values as the
//! chain writes them, which [`ProxyDelegateEntry::decode`] must read back to
//! the generator's inputs. Its `routing` section is the chain's own and has
//! no Rust counterpart.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use zcash_vote_delegate::ProxyDelegateEntry;

/// The committed chain vector file, embedded at compile time.
const COMMITTED_CHAIN_VECTORS: &str =
    include_str!("../test-vectors/proxy_delegation_chain_v1.json");

/// The file's SHA-256, recorded in contract §13.
const CHAIN_VECTORS_SHA256: &str =
    "fdfe5131df8bb8a37b9025567e3b6d8cd41e04a106f6d2f3102952efa5f3f244";

/// The parts of the chain vector file this crate checks.
#[derive(Deserialize)]
struct ChainVectorFile {
    format: String,
    entry_value: Vec<EntryValueCase>,
}

/// One `entry_value` case: an entry and the bytes the chain stores for it.
#[derive(Deserialize)]
struct EntryValueCase {
    name: String,
    inputs: EntryInputs,
    expected: EntryValueExpected,
}

/// An entry's fields, with the key as lowercase hex.
#[derive(Debug, PartialEq, Deserialize)]
struct EntryInputs {
    index: u32,
    dk: String,
    provider: u32,
    handle: String,
    proof_id: String,
    status: u32,
    status_reason: u32,
    registered_height: u64,
}

/// The stored value of one entry.
#[derive(Deserialize)]
struct EntryValueExpected {
    value: String,
}

/// The committed file is byte for byte the one contract §13 records.
#[test]
fn committed_chain_vectors_match_the_recorded_hash() {
    assert_eq!(
        hex::encode(Sha256::digest(COMMITTED_CHAIN_VECTORS)),
        CHAIN_VECTORS_SHA256,
        "the chain vector file changed; copy vote-sdk's file byte for byte"
    );
}

/// Every stored value decodes to its case's entry, and the entry encodes back
/// to exactly those bytes.
#[test]
fn entry_value_vectors_decode_and_encode() {
    let file: ChainVectorFile =
        serde_json::from_str(COMMITTED_CHAIN_VECTORS).expect("the chain vectors parse");
    assert_eq!(file.format, "proxy_delegation_chain_v1");
    assert!(!file.entry_value.is_empty(), "no entry_value cases");
    for case in &file.entry_value {
        let value = hex::decode(&case.expected.value).expect("vector hex");
        let entry = ProxyDelegateEntry::decode(&value)
            .unwrap_or_else(|error| panic!("decode entry_value/{}: {error}", case.name));
        let decoded = EntryInputs {
            index: entry.index,
            dk: hex::encode(entry.dk.to_bytes()),
            provider: entry.provider.code(),
            handle: entry.handle.clone(),
            proof_id: entry.proof_id.clone(),
            status: entry.status.code(),
            status_reason: entry.status_reason,
            registered_height: entry.registered_height,
        };
        assert_eq!(decoded, case.inputs, "entry_value/{}", case.name);
        assert_eq!(entry.encode(), value, "encode entry_value/{}", case.name);
    }
}
