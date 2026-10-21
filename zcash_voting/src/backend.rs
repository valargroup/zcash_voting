//! Selected wallet and proving crates.
//!
//! Exactly one crate feature selects the concrete dependency family. This
//! module re-exports that family's crates under stable internal names so the
//! rest of the implementation remains backend-agnostic.

pub use voting_crypto_deps::{halo2_gadgets, halo2_proofs, pasta_curves};
pub use zakura_wallet_lib::{
    client_backend as zcash_client_backend, client_sqlite as zcash_client_sqlite,
    keys as zcash_keys, orchard, pczt, primitives as zcash_primitives,
};

#[cfg(feature = "lrz")]
pub use ::lrz_zcash_protocol as zcash_protocol;
#[cfg(feature = "zakura")]
pub use ::zcash_protocol;

/// Builds a non-expiring unified address containing only the supplied Orchard receiver.
pub(crate) fn orchard_unified_address(
    receiver: orchard::Address,
) -> Option<zcash_keys::address::UnifiedAddress> {
    #[cfg(feature = "zakura")]
    {
        zcash_keys::address::UnifiedAddress::from_receivers(Some(receiver), None, None)
    }
    #[cfg(feature = "lrz")]
    {
        zcash_keys::address::UnifiedAddress::from_receivers(Some(receiver), None, None, None, None)
    }
}

/// Returns the canonical 32-byte scalar encoding of a PCZT spend-authorization randomizer.
pub(crate) fn spend_auth_randomizer_bytes(
    randomizer: &voting_crypto_deps::SpendAuthRandomizer,
) -> [u8; 32] {
    #[cfg(feature = "zakura")]
    {
        randomizer.to_bytes()
    }
    #[cfg(feature = "lrz")]
    {
        use pasta_curves::group::ff::PrimeField;
        randomizer.to_repr()
    }
}

/// Encodes a viewing key for the selected network, returning unsupported encodings to callers.
pub(crate) fn encode_full_viewing_key<P: zcash_protocol::consensus::Parameters>(
    key: &zcash_keys::keys::UnifiedFullViewingKey,
    network: &P,
) -> Result<String, crate::types::VotingError> {
    #[cfg(feature = "zakura")]
    {
        Ok(key.encode(network))
    }
    #[cfg(feature = "lrz")]
    {
        key.encode(network)
            .map_err(|error| crate::types::VotingError::InvalidInput {
                message: format!("Cannot encode the unified full viewing key: {error:?}"),
            })
    }
}
