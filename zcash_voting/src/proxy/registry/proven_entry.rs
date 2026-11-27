//! A registry entry decoded from bytes the participation reader proved
//! (contract §11.6). See [`ProvenDelegateEntry`] for the invariants.

use thiserror::Error;
use zcash_vote_delegate::{EntryDecodeError, ProxyDelegateEntry};

use super::ProvenValue;

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
