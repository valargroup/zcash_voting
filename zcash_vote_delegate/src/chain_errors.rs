//! The registered chain errors proxy delegation paths return (contract §6).
//! Clients key on (codespace, code). The same code in another codespace is a
//! different, terminal rejection.

/// The codespace of every error in [`ProxyChainError`].
pub const VOTE_CODESPACE: &str = "vote";

/// A registered `vote` error a proxy delegation path can return. The
/// contract §6 table says how a client handles each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProxyChainError {
    /// 2: a nullifier is already spent.
    DuplicateNullifier,
    /// 3: the round does not exist.
    RoundNotFound,
    /// 4: the round is not active, or its voting window has closed.
    RoundNotActive,
    /// 5: a zero-knowledge proof does not verify.
    InvalidProof,
    /// 6: a RedPallas or Ed25519 signature does not verify.
    InvalidSignature,
    /// 7: the commitment-tree anchor height has no root.
    InvalidAnchorHeight,
    /// 9: a message field fails `ValidateBasic`.
    InvalidField,
    /// 11: the commitment tree cannot take the batch's leaves.
    CommitmentTreeFull,
    /// 51: coordinators have paused new DC batches.
    ProxyDelegationPaused,
    /// 52: the batch reached its `expiry_height` without landing.
    ProxyBatchExpired,
    /// 53: no delegate entry has this index.
    DelegateNotFound,
    /// 54: the delegate is suspended.
    DelegateSuspended,
    /// 55: the delegate is retired.
    DelegateRetired,
    /// 57: the delegate already has a ballot in this round.
    DelegateBallotExists,
    /// 58: the ballot has the wrong option count or an option out of range.
    InvalidBallot,
    /// 59: the params verifier set is empty, so registration is closed.
    RegistrationClosed,
    /// 60: the registry has reached `max_delegates`.
    RegistryFull,
    /// 61: the delegate key was registered before.
    DelegateKeyReused,
    /// 62: the handle already has a live entry.
    HandleTaken,
    /// 64: an attestation's verifier, window or signature is invalid.
    InvalidAttestation,
    /// 65: too few distinct verifiers attested.
    InsufficientAttestations,
    /// 66: a delegate or verifier key fails the Ed25519 key rule.
    InvalidDelegateKey,
    /// 68: the params payload is invalid.
    InvalidProxyParams,
    /// 69: the round is not a proxy round, or proxy delegation is off.
    ProxyDelegationDisabled,
}

impl ProxyChainError {
    /// Every error, in code order.
    pub const ALL: [Self; 24] = [
        Self::DuplicateNullifier,
        Self::RoundNotFound,
        Self::RoundNotActive,
        Self::InvalidProof,
        Self::InvalidSignature,
        Self::InvalidAnchorHeight,
        Self::InvalidField,
        Self::CommitmentTreeFull,
        Self::ProxyDelegationPaused,
        Self::ProxyBatchExpired,
        Self::DelegateNotFound,
        Self::DelegateSuspended,
        Self::DelegateRetired,
        Self::DelegateBallotExists,
        Self::InvalidBallot,
        Self::RegistrationClosed,
        Self::RegistryFull,
        Self::DelegateKeyReused,
        Self::HandleTaken,
        Self::InvalidAttestation,
        Self::InsufficientAttestations,
        Self::InvalidDelegateKey,
        Self::InvalidProxyParams,
        Self::ProxyDelegationDisabled,
    ];

    /// The registered ABCI code in [`VOTE_CODESPACE`].
    pub const fn code(self) -> u32 {
        match self {
            Self::DuplicateNullifier => 2,
            Self::RoundNotFound => 3,
            Self::RoundNotActive => 4,
            Self::InvalidProof => 5,
            Self::InvalidSignature => 6,
            Self::InvalidAnchorHeight => 7,
            Self::InvalidField => 9,
            Self::CommitmentTreeFull => 11,
            Self::ProxyDelegationPaused => 51,
            Self::ProxyBatchExpired => 52,
            Self::DelegateNotFound => 53,
            Self::DelegateSuspended => 54,
            Self::DelegateRetired => 55,
            Self::DelegateBallotExists => 57,
            Self::InvalidBallot => 58,
            Self::RegistrationClosed => 59,
            Self::RegistryFull => 60,
            Self::DelegateKeyReused => 61,
            Self::HandleTaken => 62,
            Self::InvalidAttestation => 64,
            Self::InsufficientAttestations => 65,
            Self::InvalidDelegateKey => 66,
            Self::InvalidProxyParams => 68,
            Self::ProxyDelegationDisabled => 69,
        }
    }

    /// The Go identifier vote-sdk registers it under.
    pub const fn name(self) -> &'static str {
        match self {
            Self::DuplicateNullifier => "ErrDuplicateNullifier",
            Self::RoundNotFound => "ErrRoundNotFound",
            Self::RoundNotActive => "ErrRoundNotActive",
            Self::InvalidProof => "ErrInvalidProof",
            Self::InvalidSignature => "ErrInvalidSignature",
            Self::InvalidAnchorHeight => "ErrInvalidAnchorHeight",
            Self::InvalidField => "ErrInvalidField",
            Self::CommitmentTreeFull => "ErrCommitmentTreeFull",
            Self::ProxyDelegationPaused => "ErrProxyDelegationPaused",
            Self::ProxyBatchExpired => "ErrProxyBatchExpired",
            Self::DelegateNotFound => "ErrDelegateNotFound",
            Self::DelegateSuspended => "ErrDelegateSuspended",
            Self::DelegateRetired => "ErrDelegateRetired",
            Self::DelegateBallotExists => "ErrDelegateBallotExists",
            Self::InvalidBallot => "ErrInvalidBallot",
            Self::RegistrationClosed => "ErrRegistrationClosed",
            Self::RegistryFull => "ErrRegistryFull",
            Self::DelegateKeyReused => "ErrDelegateKeyReused",
            Self::HandleTaken => "ErrHandleTaken",
            Self::InvalidAttestation => "ErrInvalidAttestation",
            Self::InsufficientAttestations => "ErrInsufficientAttestations",
            Self::InvalidDelegateKey => "ErrInvalidDelegateKey",
            Self::InvalidProxyParams => "ErrInvalidProxyParams",
            Self::ProxyDelegationDisabled => "ErrProxyDelegationDisabled",
        }
    }

    /// The error a chain answer names, or `None` when the codespace is not
    /// `vote` or the code is not in the table.
    pub fn from_codespace_and_code(codespace: &str, code: u32) -> Option<Self> {
        if codespace != VOTE_CODESPACE {
            return None;
        }
        Self::ALL.into_iter().find(|error| error.code() == code)
    }
}
