//! Which delegation circuit key made a saved delegation proof.
//!
//! A delegation proof only verifies under the verifying key of the circuit
//! build that produced it. A wallet built against an older circuit therefore
//! produces proofs the vote chain can never accept, and without more
//! information the chain can only answer "invalid proof". The fingerprint
//! names the key, so the chain can tell an outdated wallet from a wallet that
//! proved the wrong statement.
//!
//! The fingerprint is computed once when a proof is generated, saved beside
//! that proof, and sent with every request that carries the proof. It always
//! describes the key that made the saved proof, never the key of whichever
//! build later re-sends it.

#[cfg(test)]
mod tests;

use std::fmt;
use std::sync::OnceLock;

use crate::backend::halo2_proofs::plonk;
use crate::backend::pasta_curves::vesta;

/// HTTP request header that carries [`DelegationKeyFingerprint::to_hex`] on
/// the two vote-chain routes that submit a delegation proof
/// (`delegate-vote` and `delegate-and-cast-vote-batch`).
///
/// The SDK adds it to its own requests. An app that broadcasts a delegation
/// itself must send it with the same value, taken from
/// [`crate::delegate::DelegationSubmission::proof_key_fingerprint`], and must
/// omit it when that value is `None`.
pub const DELEGATION_KEY_FINGERPRINT_HEADER: &str = "x-vote-delegation-key-v1";

/// Fingerprint of the delegation circuit verifying key that made a proof.
///
/// It is the 32-byte BLAKE2b hash (no key, no personalization) of the debug
/// rendering of the key's pinned form, the same definition the circuit crate
/// uses to detect an unintended key change. Two builds agree on it exactly
/// when their delegation proofs verify under the same key. It is public data:
/// it identifies a circuit build, not a wallet, a round, or a proof.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DelegationKeyFingerprint([u8; 32]);

impl DelegationKeyFingerprint {
    /// Wraps fingerprint bytes that were computed elsewhere, such as a value
    /// read back from storage or received from the device that proved.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw 32 fingerprint bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The 64-character lowercase hexadecimal form sent in
    /// [`DELEGATION_KEY_FINGERPRINT_HEADER`].
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Fingerprint of the delegation key this process proves with.
    ///
    /// The circuit crate builds that key once per process, so the fingerprint
    /// is computed on first use and reused afterwards; rendering the pinned
    /// key is too large to repeat for every proof.
    pub(crate) fn of_process_delegation_key(
        verifying_key: &plonk::VerifyingKey<vesta::Affine>,
    ) -> Self {
        static PROCESS_DELEGATION_KEY: OnceLock<DelegationKeyFingerprint> = OnceLock::new();
        *PROCESS_DELEGATION_KEY.get_or_init(|| Self::of_pinned_key(&verifying_key.pinned()))
    }

    /// Hashes the debug rendering of a pinned verifying key.
    fn of_pinned_key(pinned_key: &impl fmt::Debug) -> Self {
        let rendered = format!("{pinned_key:?}");
        let digest = blake2b_simd::Params::new()
            .hash_length(32)
            .hash(rendered.as_bytes());
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(digest.as_bytes());
        Self(bytes)
    }
}

impl fmt::Debug for DelegationKeyFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "DelegationKeyFingerprint({})", self.to_hex())
    }
}
