//! The registry entry keys the host's participation reader proves (contract
//! §11.6), and what it established for each.

use zcash_vote_delegate::delegate_entry_store_key;

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

/// What `proven` holds for entry `delegate_index`, or
/// [`ProvenValue::Unavailable`].
pub(super) fn proven_value(proven: &[ProvenRecord], delegate_index: u32) -> &ProvenValue {
    proven
        .iter()
        .find(|record| record.request.delegate_index == delegate_index)
        .map_or(&ProvenValue::Unavailable, |record| &record.value)
}
