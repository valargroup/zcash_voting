//! The delegate registry digests: REGISTER, ATTEST, RETIRE and BALLOT
//! (contract §4.2).

use super::{
    DELEGATE_ATTEST_DOMAIN, DELEGATE_BALLOT_DOMAIN, DELEGATE_REGISTER_DOMAIN,
    DELEGATE_RETIRE_DOMAIN,
};
use crate::{DigestError, DigestPreimage};

/// The identity provider whose account a delegate registers with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum IdentityProvider {
    /// X, identified by its handle. Wire code 1.
    X = 1,
    /// GitHub, identified by the gist owner's login. Wire code 2.
    GitHub = 2,
}

impl IdentityProvider {
    /// The provider's wire code: 1 for X, 2 for GitHub.
    pub const fn code(self) -> u32 {
        self as u32
    }

    /// The provider for a wire code, or `None` for any code other than 1 or 2.
    pub const fn from_code(code: u32) -> Option<Self> {
        match code {
            1 => Some(Self::X),
            2 => Some(Self::GitHub),
            _ => None,
        }
    }
}

/// The registration fields that REGISTER and ATTEST both bind. `handle` and
/// `proof_id` are canonical (contracts-v1 §2.5), which the chain checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DelegateRegisterFields<'a> {
    /// The Ed25519 delegate key.
    pub dk: [u8; 32],
    /// The account's provider.
    pub provider: IdentityProvider,
    /// The account's handle.
    pub handle: &'a str,
    /// The id of the post or gist that carries the marker for `dk`.
    pub proof_id: &'a str,
}

/// An attestation's validity window in unix seconds. The chain accepts it
/// while `not_before <= blockTime < expires_at`, and caps it at 72 hours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttestationWindow {
    /// First second at which the attestation is valid.
    pub not_before: u64,
    /// First second at which the attestation is no longer valid.
    pub expires_at: u64,
}

/// The fields RETIRE binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DelegateRetireFields {
    /// The entry being retired.
    pub delegate_index: u32,
    /// The entry's key, which also signs the digest.
    pub dk: [u8; 32],
}

/// The fields BALLOT binds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegateBallotFields {
    /// The voting round.
    pub vote_round_id: [u8; 32],
    /// The delegate casting the ballot.
    pub delegate_index: u32,
    /// `options[i]` is the choice on proposal `i + 1`.
    pub options: Vec<u32>,
}

/// Computes REGISTER, which the delegate key signs as proof of possession.
pub fn register_digest(
    chain_id: &str,
    registration: &DelegateRegisterFields<'_>,
) -> Result<[u8; 32], DigestError> {
    let mut preimage = DigestPreimage::new(DELEGATE_REGISTER_DOMAIN, chain_id)?;
    write_registration(&mut preimage, registration)?;
    Ok(preimage.finish())
}

/// Computes ATTEST, which a verifier signs to vouch for a registration within
/// `window`.
pub fn attest_digest(
    chain_id: &str,
    registration: &DelegateRegisterFields<'_>,
    window: &AttestationWindow,
) -> Result<[u8; 32], DigestError> {
    let mut preimage = DigestPreimage::new(DELEGATE_ATTEST_DOMAIN, chain_id)?;
    write_registration(&mut preimage, registration)?;
    preimage.write_u64_as_32(window.not_before);
    preimage.write_u64_as_32(window.expires_at);
    Ok(preimage.finish())
}

/// Computes RETIRE, which the retiring entry's key signs.
pub fn retire_digest(
    chain_id: &str,
    retirement: &DelegateRetireFields,
) -> Result<[u8; 32], DigestError> {
    let mut preimage = DigestPreimage::new(DELEGATE_RETIRE_DOMAIN, chain_id)?;
    preimage.write_u32_as_32(retirement.delegate_index);
    preimage.write32(&retirement.dk);
    Ok(preimage.finish())
}

/// Computes BALLOT, which the delegate entry's key signs. The option
/// count precedes the options, so different proposal counts never share a preimage.
pub fn ballot_digest(
    chain_id: &str,
    ballot: &DelegateBallotFields,
) -> Result<[u8; 32], DigestError> {
    let mut preimage = DigestPreimage::new(DELEGATE_BALLOT_DOMAIN, chain_id)?;
    preimage.write32(&ballot.vote_round_id);
    preimage.write_u32_as_32(ballot.delegate_index);
    preimage.write_count(ballot.options.len())?;
    for option in &ballot.options {
        preimage.write_u32_as_32(*option);
    }
    Ok(preimage.finish())
}

/// Writes the registration fields REGISTER and ATTEST share, in order.
fn write_registration(
    preimage: &mut DigestPreimage,
    registration: &DelegateRegisterFields<'_>,
) -> Result<(), DigestError> {
    preimage.write32(&registration.dk);
    preimage.write_u32_as_32(registration.provider.code());
    preimage.write_lp8(registration.handle.as_bytes())?;
    preimage.write_lp8(registration.proof_id.as_bytes())
}
