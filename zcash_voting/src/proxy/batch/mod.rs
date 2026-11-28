//! One bundle's 0x09 proxy batch, planned once and then proved in phases.
//!
//! `slot_plan` persists each slot's nullifier and spend-auth randomizer
//! before the first proof. `dc_chain` builds the DCs from those slots and
//! `proving` proves them. `batch_store` re-verifies the inputs and persists
//! the batch. `attempt` signs D1 under the stored randomizers and persists
//! each attempt before its POST.
//!
//! Every DC input is persisted or derived from the round hotkey, so a
//! re-prove from the same anchor rebuilds byte-identical DCs, successor VANs,
//! nullifiers and `r_vpk`s (plan §4.4 Invariant). The bundle stays held from
//! casts until the batch lands.

mod attempt;
mod batch_message;
mod batch_store;
mod dc_chain;
mod proving;
mod slot_plan;

use std::fmt;

use zcash_vote_delegate::DcShareLayout;

use crate::{session::ProxyBatchStage, vote::VanWitness};

#[cfg(test)]
pub(crate) use attempt::SignedBatchAttempt;
pub(crate) use attempt::{
    batch_fields, load_signed_attempt, load_submission_state, registration_van, sign_batch_attempt,
    verify_dc_signature, AttemptSigningRequest, SubmissionState,
};
#[cfg(test)]
pub(crate) use batch_message::ProxyDelegationActionWire;
pub(crate) use batch_message::ProxyDelegationBatchWire;
#[cfg(test)]
pub(crate) use batch_store::insert_proved_batch;
pub(crate) use batch_store::{
    batch_holds_casts, delete_proved_batch, load_proved_batch, proxy_batch_statuses_on,
    record_dc_landings,
};
#[cfg(test)]
pub(crate) use dc_chain::{build_dc_chain, BuiltDc};
pub(crate) use dc_chain::{election_key, share_layout};
#[cfg(test)]
pub(crate) use proving::load_chain_inputs;
pub(crate) use proving::{prove_batch, ProveBatchRequest};
pub(crate) use slot_plan::{canonical_base, decode_round_id, plan_batch_slots, PlannedSlot};

/// One bundle's proxy batch as round planning reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProxyBatchStatus {
    /// How far the batch has come. `Refused` while the chain refused the
    /// last attempt, which is sent again until it lands or voting ends.
    pub(crate) progress: ProxyBatchStage,
    /// The first attempt needs the voter's SpendAuth signature for the
    /// carried registration.
    pub(crate) needs_registration_signature: bool,
}

impl ProxyBatchStatus {
    /// A status at `progress` with every flag false.
    #[cfg(test)]
    pub(crate) fn at(progress: ProxyBatchStage) -> Self {
        Self {
            progress,
            needs_registration_signature: false,
        }
    }
}

/// The VAN the first DC spends. Later DCs spend their predecessor's
/// successor VAN through a single-leaf root.
#[derive(Clone, Debug)]
pub(crate) enum BatchAnchor {
    /// The batch carries the ZKP1 registration. The first DC spends its VAN
    /// through a single-leaf root and every DC binds height 0 (contract §5.1).
    Registration,
    /// The registration landed. The first DC spends the confirmed VAN
    /// through this witness and every DC binds its anchor height.
    Chain(VanWitness),
}

impl BatchAnchor {
    /// The anchor height every DC of the batch binds.
    pub(crate) fn anchor_height(&self) -> u32 {
        match self {
            Self::Registration => 0,
            Self::Chain(witness) => witness.anchor_height,
        }
    }
}

/// One proved DC as stored, in its slot.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ProvedProxyDc {
    /// Position in the batch.
    pub(crate) dc_slot: u8,
    /// The delegate whose pool the DC funds.
    pub(crate) delegate_index: u32,
    pub(crate) weight_ballots: u64,
    /// Nullifier of the VAN the DC spends.
    pub(crate) van_nullifier: [u8; 32],
    /// Randomized spend key the batch signature verifies under.
    pub(crate) r_vpk: [u8; 32],
    /// Successor VAN, holding the spent weight less the DC's.
    pub(crate) vote_authority_note_new: [u8; 32],
    /// The DC's tree leaf.
    pub(crate) delegation_commitment: [u8; 32],
    /// The slot nullifier the proof publishes.
    pub(crate) dc_slot_nullifier: [u8; 32],
    /// The slot's spend-auth randomizer, which `Debug` never prints.
    pub(crate) alpha_v: [u8; 32],
    pub(crate) proof: Vec<u8>,
}

impl fmt::Debug for ProvedProxyDc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProvedProxyDc")
            .field("dc_slot", &self.dc_slot)
            .field("delegate_index", &self.delegate_index)
            .field("weight_ballots", &self.weight_ballots)
            .field(
                "delegation_commitment",
                &hex::encode(self.delegation_commitment),
            )
            .field("proof_len", &self.proof.len())
            .finish_non_exhaustive()
    }
}

/// A bundle's proved, unsigned proxy batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProvedProxyBatch {
    pub(crate) bundle_index: u32,
    /// See [`BatchAnchor::anchor_height`].
    pub(crate) anchor_height: u32,
    pub(crate) layout: DcShareLayout,
    /// Every DC, in slot order.
    pub(crate) dcs: Vec<ProvedProxyDc>,
}
