//! The 0x09 `MsgProxyDelegationBatch` JSON body that
//! `POST /shielded-vote/v1/proxy-delegation-batch` decodes (contract §2.1, §8).
//!
//! Keys are snake_case, bytes padded standard base64 and integers JSON
//! numbers (contract §0). The endpoint rejects unknown fields.

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::Serialize;

use super::ProvedProxyBatch;
use crate::{types::VotingError, wire::DelegationSubmissionWire};

/// One DC's `ProxyDelegationAction`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ProxyDelegationActionWire {
    pub(crate) van_nullifier: String,
    pub(crate) r_vpk: String,
    /// The successor VAN.
    pub(crate) vote_authority_note_new: String,
    pub(crate) proxy_delegation_commitment: String,
    pub(crate) dc_slot_nullifier: String,
    /// The ZKP4 proof.
    pub(crate) proof: String,
    /// RedPallas signature over D1 under `r_vpk`.
    pub(crate) vote_auth_sig: String,
}

/// A `MsgProxyDelegationBatch`, one signed attempt of a bundle's batch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ProxyDelegationBatchWire {
    pub(crate) vote_round_id: String,
    /// 0 exactly when `registration` is set.
    pub(crate) vote_comm_tree_anchor_height: u64,
    /// The bundle's ZKP1 registration, if carried.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) registration: Option<DelegationSubmissionWire>,
    /// In slot order.
    pub(crate) dcs: Vec<ProxyDelegationActionWire>,
    /// First height at which the chain rejects this attempt.
    pub(crate) expiry_height: u64,
}

impl ProxyDelegationBatchWire {
    /// The body of one signed attempt of `batch`, pairing each DC with its D1
    /// signature.
    pub(crate) fn from_attempt(
        vote_round_id: &[u8; 32],
        batch: &ProvedProxyBatch,
        registration: Option<DelegationSubmissionWire>,
        dc_signatures: &[[u8; 64]],
        expiry_height: u64,
    ) -> Self {
        Self {
            vote_round_id: BASE64_STANDARD.encode(vote_round_id),
            vote_comm_tree_anchor_height: u64::from(batch.anchor_height),
            registration,
            dcs: batch
                .dcs
                .iter()
                .zip(dc_signatures)
                .map(|(dc, signature)| ProxyDelegationActionWire {
                    van_nullifier: BASE64_STANDARD.encode(dc.van_nullifier),
                    r_vpk: BASE64_STANDARD.encode(dc.r_vpk),
                    vote_authority_note_new: BASE64_STANDARD.encode(dc.vote_authority_note_new),
                    proxy_delegation_commitment: BASE64_STANDARD.encode(dc.delegation_commitment),
                    dc_slot_nullifier: BASE64_STANDARD.encode(dc.dc_slot_nullifier),
                    proof: BASE64_STANDARD.encode(&dc.proof),
                    vote_auth_sig: BASE64_STANDARD.encode(signature),
                })
                .collect(),
            expiry_height,
        }
    }

    /// Serializes the body the endpoint decodes.
    pub(crate) fn to_json(&self) -> Result<String, VotingError> {
        serde_json::to_string(self).map_err(|error| VotingError::Internal {
            message: format!("failed to encode the proxy delegation batch: {error}"),
        })
    }
}
