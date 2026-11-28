//! The registered chain errors proxy delegation paths return (contract §6).
//! Clients key on (codespace, code). The same code in another codespace is a
//! different, terminal rejection.

/// The codespace of every error in [`ProxyChainError`].
pub const VOTE_CODESPACE: &str = "vote";

/// A registered `vote` error a proxy delegation path can return. The
/// contract §6 table says how a client handles each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum ProxyChainError {
    /// 2: a nullifier is already spent.
    DuplicateNullifier = 2,
    /// 3: the round does not exist.
    RoundNotFound = 3,
    /// 4: the round is not active, or its voting window has closed.
    RoundNotActive = 4,
    /// 5: a zero-knowledge proof does not verify.
    InvalidProof = 5,
    /// 6: a RedPallas or Ed25519 signature does not verify.
    InvalidSignature = 6,
    /// 7: the commitment-tree anchor height has no root.
    InvalidAnchorHeight = 7,
    /// 9: a message field fails `ValidateBasic`.
    InvalidField = 9,
    /// 11: the commitment tree cannot take the batch's leaves.
    CommitmentTreeFull = 11,
    /// 51: coordinators have paused new DC batches.
    ProxyDelegationPaused = 51,
    /// 52: the batch reached its `expiry_height` without landing.
    ProxyBatchExpired = 52,
    /// 53: no delegate entry has this index.
    DelegateNotFound = 53,
    /// 54: the delegate is suspended.
    DelegateSuspended = 54,
    /// 55: the delegate is retired.
    DelegateRetired = 55,
    /// 57: the delegate already has a ballot in this round.
    DelegateBallotExists = 57,
    /// 58: the ballot has the wrong option count or an option out of range.
    InvalidBallot = 58,
    /// 59: the params verifier set is empty, so registration is closed.
    RegistrationClosed = 59,
    /// 60: the registry has reached `max_delegates`.
    RegistryFull = 60,
    /// 61: the delegate key was registered before.
    DelegateKeyReused = 61,
    /// 62: the handle already has a live entry.
    HandleTaken = 62,
    /// 64: an attestation's verifier, window or signature is invalid.
    InvalidAttestation = 64,
    /// 65: too few distinct verifiers attested.
    InsufficientAttestations = 65,
    /// 66: a delegate or verifier key fails the Ed25519 key rule.
    InvalidDelegateKey = 66,
    /// 68: the params payload is invalid.
    InvalidProxyParams = 68,
    /// 69: the round is not a proxy round, or proxy delegation is off.
    ProxyDelegationDisabled = 69,
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
        self as u32
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
