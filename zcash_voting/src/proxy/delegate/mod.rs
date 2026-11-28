//! Delegate mode (plan §4.4, §4.5): what a public delegate's own device
//! needs to register, ballot and retire.
//!
//! Transport-agnostic like [`super::registry`]. The host fetches and posts
//! bytes, runs the participation reader's proofs and keeps the clock. These
//! functions build bodies, read answers and keep the device's records,
//! scoped to the [`VotingDb`]'s wallet id and the [`DelegateChain`]: its
//! network and `chain_id`. A device holds one delegate key and one entry per
//! chain. No secret is stored. The host keeps the key's `zvdsk1…` export in
//! secure storage and passes its [`DelegateSigningKey`] to each call that
//! signs.
//!
//! 1. **Key.** [`create_delegate_key`] or [`DelegateSigningKey::import`],
//!    then [`record_delegate_key`].
//! 2. **Attest.** [`delegate_marker_text`], [`attest_request`],
//!    [`parse_attest_response`], then [`check_attestation`] before signing.
//! 3. **Register.** [`prepare_register_op`], [`classify_register_answer`].
//! 4. **Ballot.** [`ballot_preflight`], [`sign_ballot`],
//!    [`classify_ballot_answer`]; a retry posts [`stored_ballot`]'s op.
//! 5. **Status.** [`record_proven_entry`] settles a registration, then
//!    [`delegate_mode_status`], [`delegate_round`].
//! 6. **Retire.** [`build_retire_op`], [`classify_retire_answer`]. To change
//!    a key, retire, [`forget_delegate_key`] and register again.
//!
//! [`VotingDb`]: crate::storage::VotingDb

mod attestation;
mod ballot;
mod chain_reads;
mod delegate_op;
mod delegate_store;
mod device_key;
mod registration;
mod status;
mod verifier;

use thiserror::Error;

use crate::types::{validate_vote_chain_id, Network, VotingError};

pub use super::ballot_pages::{
    ChainDelegateBallot, DelegateBallotPage, DELEGATE_BALLOTS_PAGE_LIMIT,
};
pub use super::registry::{DelegateEntryError, ProvenDelegateEntry};
pub use attestation::{
    check_attestation, AttestationExpectation, AttestationMismatch, MAX_ATTESTATIONS,
    MAX_ATTESTATION_CLOCK_SKEW_SECONDS, MAX_ATTESTATION_WINDOW_SECONDS,
};
pub use ballot::{
    ballot_preflight, classify_ballot_answer, existing_ballot_result, sign_ballot, stored_ballot,
    BallotAnswer, BallotError, CompleteBallot,
};
pub use chain_reads::{
    delegate_ballots_path, delegate_by_key_path, parse_delegate_ballots_page,
    parse_delegate_by_key_response, parse_pool_share_count, parse_proxy_params_response,
    ChainReadError, ProxyVerifier, ProxyVerifierSet, PROXY_PARAMS_PATH,
};
pub use delegate_op::{
    parse_delegate_op_answer, DelegateOpAnswer, DelegateOpAnswerError, DelegateOpKind,
    SignedDelegateOp, DELEGATE_OP_PATH,
};
pub use delegate_store::{DeviceDelegateKey, StoredBallot};
pub use device_key::{
    create_delegate_key, delegate_key_info, delegate_marker_text, device_delegate_key,
    forget_delegate_key, record_delegate_key, record_proven_entry,
};
pub use registration::{
    build_retire_op, classify_register_answer, classify_retire_answer, pending_register_op,
    prepare_register_op, RegisterAnswer, RetireAnswer,
};
pub use status::{delegate_mode_status, delegate_round};
pub use verifier::{attest_request, parse_attest_response, VERIFIER_ATTEST_PATH};
pub use zcash_vote_delegate::{
    pool_share_count_store_key, DelegateKeyDecodeError, DelegateSigningKey,
};

/// The vote chain a delegate acts on: its network and its `chain_id`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegateChain {
    network: Network,
    chain_id: String,
}

impl DelegateChain {
    /// The chain `chain_id` on `network`.
    ///
    /// # Errors
    ///
    /// [`DelegateModeError::InvalidInput`] unless `chain_id` is 1 to 128
    /// printable ASCII bytes without whitespace.
    pub fn new(network: Network, chain_id: impl Into<String>) -> Result<Self, DelegateModeError> {
        let chain_id = chain_id.into();
        validate_vote_chain_id(&chain_id).map_err(|error| DelegateModeError::InvalidInput {
            message: error.to_string(),
        })?;
        Ok(Self { network, chain_id })
    }

    /// The chain's Zcash network.
    pub fn network(&self) -> Network {
        self.network
    }

    /// The chain id every REGISTER, RETIRE and BALLOT digest binds. The
    /// device keeps separate records for each chain of a network.
    pub fn chain_id(&self) -> &str {
        &self.chain_id
    }
}

/// A proxy round's ballot deadline, from the round as the host reads it. A
/// wrong `vote_end_time` only makes the chain refuse the ballot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DelegateRoundTiming {
    pub round_id: [u8; 32],
    /// The round's `vote_end_time`. Voting is open strictly before it.
    pub vote_end_time: u64,
}

impl DelegateRoundTiming {
    /// Whether voting is still open at `now_seconds`.
    fn is_running_at(&self, now_seconds: u64) -> bool {
        now_seconds < self.vote_end_time
    }
}

/// A delegate-mode call failed on its inputs or the device's records.
#[derive(Debug, Error)]
pub enum DelegateModeError {
    /// The host passed a malformed or inconsistent value.
    #[error("invalid input: {message}")]
    InvalidInput { message: String },
    /// The device already holds another delegate key on the chain.
    #[error("the device already holds delegate key {held}")]
    AnotherKeyHeld {
        /// The held key, `zvdk1…`.
        held: String,
    },
    /// The device's key already has its entry, so it needs no register op.
    #[error("the device's delegate key is registered in entry {delegate_index}")]
    KeyRegistered { delegate_index: u32 },
    /// A proven entry is not the device's: its key is not the device's key,
    /// or the device recorded its key in another entry.
    #[error("entry {delegate_index} is not the device's delegate entry")]
    NotDeviceEntry { delegate_index: u32 },
    /// A stored delegate-mode row cannot be read back as written.
    #[error("stored delegate-mode record is unreadable: {message}")]
    CorruptRecord { message: String },
    /// A chain answer the host fetched is malformed.
    #[error(transparent)]
    ChainRead(#[from] ChainReadError),
    /// The attestation does not bind what the delegate expects.
    #[error(transparent)]
    Attestation(#[from] AttestationMismatch),
    /// The database failed.
    #[error(transparent)]
    Storage(#[from] VotingError),
}

#[cfg(test)]
mod tests;
