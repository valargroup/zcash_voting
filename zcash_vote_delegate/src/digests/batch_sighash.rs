//! D1, the 0x09 `MsgProxyDelegationBatch` sighash, and the batch identity
//! derived from it (contract §4.1).

use super::PROXY_DELEGATION_BATCH_SIGHASH_DOMAIN;
use crate::{DigestError, DigestPreimage};

/// The public fields of one `ProxyDelegationAction` that D1 binds, in D1 order.
/// The ZKP4 proof binds them as public inputs and `vote_auth_sig` signs D1, so
/// neither is in D1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProxyDelegationActionFields {
    /// Randomized vote-authority key that signs D1 for this DC.
    pub r_vpk: [u8; 32],
    /// Nullifier of the VAN this DC spends.
    pub van_nullifier: [u8; 32],
    /// Successor VAN carrying the weight this DC leaves behind.
    pub vote_authority_note_new: [u8; 32],
    /// The delegation commitment (DC) leaf.
    pub proxy_delegation_commitment: [u8; 32],
    /// Nullifier of the DC slot this action uses.
    pub dc_slot_nullifier: [u8; 32],
}

/// The fields of a `MsgProxyDelegationBatch` that D1 binds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyDelegationBatchFields {
    /// The voting round.
    pub vote_round_id: [u8; 32],
    /// Commitment-tree anchor height. 0 exactly when the batch carries a registration.
    pub vote_comm_tree_anchor_height: u64,
    /// First height at which the chain rejects this batch.
    pub expiry_height: u64,
    /// The registration's `van_cmx` when the batch carries a registration (ZKP1).
    pub registration_van_cmx: Option<[u8; 32]>,
    /// The batch's DCs in message order.
    pub dcs: Vec<ProxyDelegationActionFields>,
}

/// Computes D1, the digest every DC's `vote_auth_sig` signs (contract §4.1).
///
/// Preimage: domain, `lp8(chain_id)`, round, anchor and expiry heights, a
/// registration flag byte then `van_cmx` if present, the DC count, and each
/// DC's index and fields. `ValidateBasic` checks message limits such as 1..8
/// DCs. Fails only for a chain id over 255 bytes or over `u32::MAX` DCs.
pub fn proxy_batch_sighash(
    chain_id: &str,
    batch: &ProxyDelegationBatchFields,
) -> Result<[u8; 32], DigestError> {
    sighash_with_expiry(chain_id, batch, batch.expiry_height)
}

/// Computes `batch_digest`, D1 with `expiry_height = 0`. Re-signed attempts of
/// one batch differ only in expiry, so they share it. The chain echoes it and
/// zcash_voting keys the submission on it.
pub fn proxy_batch_digest(
    chain_id: &str,
    batch: &ProxyDelegationBatchFields,
) -> Result<[u8; 32], DigestError> {
    sighash_with_expiry(chain_id, batch, 0)
}

/// D1 over `batch` with `expiry_height` in place of the batch's own expiry.
fn sighash_with_expiry(
    chain_id: &str,
    batch: &ProxyDelegationBatchFields,
    expiry_height: u64,
) -> Result<[u8; 32], DigestError> {
    let mut preimage = DigestPreimage::new(PROXY_DELEGATION_BATCH_SIGHASH_DOMAIN, chain_id)?;
    preimage.write32(&batch.vote_round_id);
    preimage.write_u64_as_32(batch.vote_comm_tree_anchor_height);
    preimage.write_u64_as_32(expiry_height);
    match &batch.registration_van_cmx {
        Some(van_cmx) => {
            preimage.write_u8(1);
            preimage.write32(van_cmx);
        }
        None => preimage.write_u8(0),
    }
    preimage.write_count(batch.dcs.len())?;
    for (index, dc) in batch.dcs.iter().enumerate() {
        // `write_count` above already proved every index fits in a u32.
        preimage.write_u32_as_32(index as u32);
        preimage.write32(&dc.r_vpk);
        preimage.write32(&dc.van_nullifier);
        preimage.write32(&dc.vote_authority_note_new);
        preimage.write32(&dc.proxy_delegation_commitment);
        preimage.write32(&dc.dc_slot_nullifier);
    }
    Ok(preimage.finish())
}
