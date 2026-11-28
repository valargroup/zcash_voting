//! The registry entry keys the host's participation reader proves (contract
//! §11.6), what it established for each, and the entry decoded from proven
//! bytes. See [`ProvenDelegateEntry`] for the invariants.

use thiserror::Error;
use zcash_vote_delegate::{delegate_entry_store_key, EntryDecodeError, ProxyDelegateEntry};

/// The chain store that holds the delegate registry.
pub const REGISTRY_PROOF_STORE: &str = "vote";

/// Most bytes a proven registry entry's value may hold.
pub const MAX_REGISTRY_ENTRY_BYTES: usize = 64 * 1024;

/// A registry entry for the participation reader to prove: the
/// [`REGISTRY_PROOF_STORE`] key `0x1A 0x01 || u32be(d)` with `d ≥ 1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RegistryProofRequest {
    /// The entry's index `d`.
    pub delegate_index: u32,
}

impl RegistryProofRequest {
    /// The store key to prove.
    pub fn store_key(&self) -> Vec<u8> {
        delegate_entry_store_key(self.delegate_index).to_vec()
    }

    /// The request whose [`store_key`](Self::store_key) is `key`, or `None`
    /// for any other key. That includes entry 0, which the chain never
    /// assigns.
    pub fn from_store_key(key: &[u8]) -> Option<Self> {
        let delegate_index = u32::from_be_bytes(key.get(2..)?.try_into().ok()?);
        let request = Self { delegate_index };
        (delegate_index >= 1 && request.store_key() == key).then_some(request)
    }

    /// Whether a membership proof of this key may claim `value`: 1 to
    /// [`MAX_REGISTRY_ENTRY_BYTES`]. An empty value is never admitted, since
    /// the chain stores none and a proof answer uses it to claim absence.
    pub fn admits(&self, value: &[u8]) -> bool {
        (1..=MAX_REGISTRY_ENTRY_BYTES).contains(&value.len())
    }
}

/// What the participation reader established for one store key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProvenValue {
    /// The value bytes, proven present under a verified header.
    Present(Vec<u8>),
    /// The key, proven absent.
    Absent,
    /// No proof could be fetched or verified.
    Unavailable,
}

/// One proven entry key, as one evidence answer returns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenRecord {
    /// The key that was proven.
    pub request: RegistryProofRequest,
    /// What the reader established for it.
    pub value: ProvenValue,
}

/// Why proven registry bytes are not a usable delegate entry. The host
/// retries later.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum DelegateEntryError {
    /// The participation reader could not prove the entry's key.
    #[error("the registry entry could not be proven")]
    Unavailable,
    /// The proven bytes are not the chain's encoding of an entry.
    #[error("the proven registry entry does not decode: {0}")]
    Undecodable(EntryDecodeError),
    /// The entry carries another index than the one its key names.
    #[error("the proof for entry {expected} holds entry {actual}")]
    WrongIndex {
        /// The index whose key was proven.
        expected: u32,
        /// The index the entry carries.
        actual: u32,
    },
}

/// A delegate registry entry, proven present and decoded strictly, whose
/// index is the one asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenDelegateEntry(ProxyDelegateEntry);

impl ProvenDelegateEntry {
    /// The entry `delegate_index` from the proof of its key
    /// `0x1A 0x01 u32be(d)`, or `None` when proven absent.
    ///
    /// # Errors
    ///
    /// A [`DelegateEntryError`] for no proof, bytes that do not decode, or
    /// another entry's bytes.
    pub fn from_proof(
        delegate_index: u32,
        proof: &ProvenValue,
    ) -> Result<Option<Self>, DelegateEntryError> {
        let bytes = match proof {
            ProvenValue::Present(bytes) => bytes,
            ProvenValue::Absent => return Ok(None),
            ProvenValue::Unavailable => return Err(DelegateEntryError::Unavailable),
        };
        let entry = ProxyDelegateEntry::decode(bytes).map_err(DelegateEntryError::Undecodable)?;
        if entry.index != delegate_index {
            return Err(DelegateEntryError::WrongIndex {
                expected: delegate_index,
                actual: entry.index,
            });
        }
        Ok(Some(Self(entry)))
    }

    /// The proven entry.
    pub fn entry(&self) -> &ProxyDelegateEntry {
        &self.0
    }
}
