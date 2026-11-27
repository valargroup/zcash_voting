//! Delegate-mode types of the client API (proxy delegation contracts-v1
//! §15.1), bridged to Dart unchanged. Times are unix seconds, and
//! `options[i]` is the choice on proposal `i + 1`.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::{DelegateBallotView, DelegateEntryStatus, DelegateKeyInfo};

/// The identity provider of a delegate entry's account.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProxyDelegateProvider {
    /// X, wire code 1.
    #[serde(rename = "x")]
    X,
    /// GitHub, wire code 2.
    #[serde(rename = "github")]
    GitHub,
}

/// One verifier signature over ATTEST (contract §12.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegateAttestationSignature {
    /// The verifier's id in the chain's params verifier set.
    pub verifier_id: u32,
    /// First second at which the chain accepts the attestation.
    pub not_before: u64,
    /// First second at which it no longer does, at most 72 hours later.
    pub expires_at: u64,
    /// The 64-byte Ed25519 signature, as padded standard base64.
    pub sig: String,
}

/// The attester's answer to `POST /v1/attest` (contract §12.4). The host
/// passes it to register while its attestations are valid.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegateAttestation {
    /// The attested key.
    pub key: DelegateKeyInfo,
    /// The account's provider.
    pub provider: ProxyDelegateProvider,
    /// The account's canonical handle, without `@`.
    pub handle: String,
    /// The canonical id of the post or gist that carries the marker.
    pub proof_id: String,
    /// One signature per verifier.
    pub attestations: Vec<DelegateAttestationSignature>,
}

/// An attester API error code (contract §12.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegateVerifierError {
    /// 400: a malformed request or unsupported provider.
    BadRequest,
    /// 404: no post or gist at the proof URL.
    PostNotFound,
    /// 409: the post's text or the gist's file is not the marker for this key.
    MarkerMismatch,
    /// 429: the per-address or daily limit is reached until `retry_after`.
    RateLimited,
    /// 503: the provider could not be read. Retry later.
    ProviderUnavailable,
    /// An unknown code, a code under a status it is not paired with, or an
    /// answer that is not the API's JSON.
    Unknown,
}

/// An attester answer other than success.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegateVerifierRefusal {
    /// What the attester said.
    pub error: DelegateVerifierError,
    /// The unix second after which a retry can succeed, when given.
    pub retry_after: Option<u64>,
    /// The answer's `message` or what was wrong with it. Diagnostic only.
    pub detail: Option<String>,
}

impl fmt::Display for DelegateVerifierRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "verifier refused: {:?}", self.error)?;
        if let Some(detail) = &self.detail {
            write!(f, ": {detail}")?;
        }
        Ok(())
    }
}

impl std::error::Error for DelegateVerifierRefusal {}

/// Outcome of a delegate registration (register op, contract §2.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegateRegisterResult {
    /// The device's key is the key of entry `delegate_index`.
    Registered {
        /// The entry's chain-assigned index.
        delegate_index: u32,
    },
    /// The chain refused the registration (contract §6).
    Rejected {
        /// ABCI codespace. Anything but `vote` is terminal.
        codespace: String,
        /// Registered error code.
        code: u32,
        /// The chain's bounded log line. Diagnostic only.
        reason: String,
    },
}

/// Outcome of Retire (retire op, contract §2.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegateRetireResult {
    /// The entry is retired, by this op or before it (code 55).
    Retired,
    /// The chain refused the retirement, such as a suspended entry's
    /// (code 54).
    Rejected {
        /// ABCI codespace.
        codespace: String,
        /// Registered error code.
        code: u32,
        /// The chain's bounded log line. Diagnostic only.
        reason: String,
    },
}

/// Whether a ballot is already stored for a round (contract §15.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BallotPreflight {
    /// Neither the chain nor this device has a ballot for the round.
    NoBallot,
    /// A ballot is on chain or signed here, the only one this device submits.
    AlreadySubmitted {
        /// The stored ballot.
        options: Vec<u32>,
        /// Whether it equals the ballot about to be submitted.
        identical: bool,
    },
}

/// Outcome of submitting a delegate ballot once (contract §15.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BallotSubmitResult {
    /// The node accepted the ballot into its mempool (code 0).
    Accepted,
    /// The chain already stores this exact ballot (code 57), a success.
    AlreadyAccepted,
    /// The chain refused the ballot. Clients key on (`codespace`, `code`).
    Rejected {
        /// ABCI codespace. Anything but `vote` is terminal.
        codespace: String,
        /// Registered error code (contract §6).
        code: u32,
        /// The chain's bounded log line. Diagnostic only.
        reason: String,
    },
}

/// The device's delegate entry, from its proven registry entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegateEntryView {
    /// Registry index `d`.
    pub delegate_index: u32,
    /// The provider of the entry's account.
    pub provider: ProxyDelegateProvider,
    /// The entry's handle, without `@`.
    pub handle: String,
    /// The id of the post or gist that carries the marker.
    pub proof_id: String,
    /// Whether the entry is active, suspended or retired.
    pub status: DelegateEntryStatus,
    /// The coordinators' reason code for the status, or 0.
    pub status_reason: u32,
}

/// The one delegate key this device holds on a chain, and its entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegateModeStatus {
    /// The device's key.
    pub key: DelegateKeyInfo,
    /// The key's entry. `None` until a registration lands.
    pub entry: Option<DelegateEntryView>,
    /// A signed register op is stored and its registration has not landed.
    pub registration_pending: bool,
}

/// Why the device cannot sign a round's ballot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegateBallotBlock {
    /// The device's key has no registry entry.
    NotRegistered,
    /// Coordinators suspended the entry.
    Suspended,
    /// The entry is retired.
    Retired,
}

/// A proxy round as the delegate sees it, without the title and questions
/// Vizor already holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegateRound {
    /// The round id, lowercase hex.
    pub round_id: String,
    /// The round's `vote_end_time`, the ballot deadline.
    pub vote_end_time: u64,
    /// R, the pool shares revealed to this delegate. The dashboard shows at
    /// least `ceil(R / 16)` delegations, never R itself.
    pub revealed_shares: u64,
    /// The delegate's ballot as published.
    pub ballot: DelegateBallotView,
    /// A signed ballot not on chain yet, which a retry resubmits unchanged.
    pub stored_ballot: Option<Vec<u32>>,
    /// Why the device cannot sign the round's ballot, if it cannot.
    pub ballot_block: Option<DelegateBallotBlock>,
}
