//! Checks, before REGISTER is signed, that the attester's answer binds the
//! delegate's key and provider to a canonical handle and proof id, within
//! windows the chain accepts (contract §2.5, §4.2, §5.3).

use std::collections::BTreeSet;

use thiserror::Error;
use zcash_vote_delegate::{
    attest_digest, is_canonical_handle, is_canonical_proof_id, verify_zip215, AttestationWindow,
    DelegateKey, DelegateRegisterFields, IdentityProvider,
};

use super::{chain_reads::ProxyVerifierSet, DelegateChain};
use crate::{
    proxy::registry::{base64_array, identity_provider},
    wire::DelegateAttestation,
};

/// Most attestations one register op carries (contract §2.5).
pub const MAX_ATTESTATIONS: usize = 16;
/// Longest attestation window `expires_at - not_before`, 72 hours (§2.5).
pub const MAX_ATTESTATION_WINDOW_SECONDS: u64 = 259_200;
/// How far `not_before` may lead the device's clock, in seconds.
pub const MAX_ATTESTATION_CLOCK_SKEW_SECONDS: u64 = 300;

/// What a delegate expects its attestation to bind.
#[derive(Clone, Copy, Debug)]
pub struct AttestationExpectation<'a> {
    /// The provider the marker was posted on.
    pub provider: IdentityProvider,
    /// The chain's verifier set, if fetched. Enables the signature and
    /// threshold checks.
    pub verifiers: Option<&'a ProxyVerifierSet>,
}

/// An attestation that does not bind what the delegate expects. Nothing is
/// signed.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum AttestationMismatch {
    /// A field does not decode, or is not canonical: the key, the handle,
    /// the proof id or a signature.
    #[error("the attestation's {field} is malformed")]
    Malformed { field: &'static str },
    /// The attestation is for another key than the one being registered.
    #[error("the attestation is for key {attested}, not {expected}")]
    WrongKey {
        /// The key being registered.
        expected: String,
        /// The attested key.
        attested: String,
    },
    /// The attestation is for another provider than the post's.
    #[error("the attestation is for provider {attested:?}, the marker was posted on {expected:?}")]
    WrongProvider {
        /// The post's provider.
        expected: IdentityProvider,
        /// The attested provider.
        attested: IdentityProvider,
    },
    /// Not 1 to [`MAX_ATTESTATIONS`] attestations.
    #[error("a register op carries 1 to {MAX_ATTESTATIONS} attestations, got {count}")]
    AttestationCount { count: usize },
    /// A window is empty or longer than [`MAX_ATTESTATION_WINDOW_SECONDS`].
    #[error(
        "verifier {verifier_id}'s window from {not_before} to {expires_at} is not 1 s to 72 h"
    )]
    InvalidWindow {
        verifier_id: u32,
        not_before: u64,
        expires_at: u64,
    },
    /// An attestation has expired on the device's clock.
    #[error("verifier {verifier_id}'s attestation expired at {expires_at}")]
    Expired { verifier_id: u32, expires_at: u64 },
    /// An attestation starts further ahead of the device's clock than
    /// [`MAX_ATTESTATION_CLOCK_SKEW_SECONDS`].
    #[error("verifier {verifier_id}'s attestation is valid only from {not_before}")]
    NotYetValid { verifier_id: u32, not_before: u64 },
    /// The chain's verifier set is empty, so registration is closed (code 59).
    #[error("registration is closed: the chain's verifier set is empty")]
    RegistrationClosed,
    /// An attestation names a verifier outside the chain's set.
    #[error("verifier {verifier_id} is not in the chain's verifier set")]
    UnknownVerifier { verifier_id: u32 },
    /// A signature does not verify over ATTEST under its verifier's key.
    #[error("verifier {verifier_id}'s signature does not verify over ATTEST")]
    BadSignature { verifier_id: u32 },
    /// Fewer distinct verifiers attested than the chain's threshold.
    #[error("{distinct} distinct verifiers attested; the chain needs {threshold}")]
    BelowThreshold {
        /// Distinct verifiers that attested.
        distinct: usize,
        threshold: u32,
    },
}

/// Checks that `attestation` binds what the delegate registering `dk`
/// expects at device time `now_seconds`.
///
/// In order: key, provider, a handle and proof id canonical for the
/// provider, the attestation count and windows (with clock skew), then
/// signatures and threshold when the verifier set is given.
///
/// # Errors
///
/// The first [`AttestationMismatch`] found.
pub fn check_attestation(
    chain: &DelegateChain,
    attestation: &DelegateAttestation,
    dk: &DelegateKey,
    expected: &AttestationExpectation<'_>,
    now_seconds: u64,
) -> Result<(), AttestationMismatch> {
    let attested_key = DelegateKey::decode(&attestation.key.dk)
        .map_err(|_| AttestationMismatch::Malformed { field: "key.dk" })?;
    if attested_key != *dk || attestation.key.fingerprint != dk.fingerprint().to_string() {
        return Err(AttestationMismatch::WrongKey {
            expected: dk.encode(),
            attested: attestation.key.dk.clone(),
        });
    }
    let fields = register_fields(attestation, dk);
    if fields.provider != expected.provider {
        return Err(AttestationMismatch::WrongProvider {
            expected: expected.provider,
            attested: fields.provider,
        });
    }
    if !is_canonical_handle(fields.provider, fields.handle) {
        return Err(AttestationMismatch::Malformed { field: "handle" });
    }
    if !is_canonical_proof_id(fields.provider, fields.proof_id) {
        return Err(AttestationMismatch::Malformed { field: "proof_id" });
    }
    let attestations = decoded_attestations(attestation, now_seconds)?;
    if let Some(verifiers) = expected.verifiers {
        check_signatures(chain, &fields, &attestations, verifiers)?;
    }
    Ok(())
}

/// The fields REGISTER and ATTEST bind for `attestation` of `dk`.
pub(super) fn register_fields<'a>(
    attestation: &'a DelegateAttestation,
    dk: &DelegateKey,
) -> DelegateRegisterFields<'a> {
    DelegateRegisterFields {
        dk: dk.to_bytes(),
        provider: identity_provider(attestation.provider),
        handle: &attestation.handle,
        proof_id: &attestation.proof_id,
    }
}

/// Checks the attestations' count and windows, and decodes each one's
/// verifier, window and signature.
fn decoded_attestations(
    attestation: &DelegateAttestation,
    now_seconds: u64,
) -> Result<Vec<(u32, AttestationWindow, [u8; 64])>, AttestationMismatch> {
    let count = attestation.attestations.len();
    if !(1..=MAX_ATTESTATIONS).contains(&count) {
        return Err(AttestationMismatch::AttestationCount { count });
    }
    attestation
        .attestations
        .iter()
        .map(|signature| {
            let verifier_id = signature.verifier_id;
            let window = AttestationWindow {
                not_before: signature.not_before,
                expires_at: signature.expires_at,
            };
            if window.not_before >= window.expires_at
                || window.expires_at - window.not_before > MAX_ATTESTATION_WINDOW_SECONDS
            {
                return Err(AttestationMismatch::InvalidWindow {
                    verifier_id,
                    not_before: window.not_before,
                    expires_at: window.expires_at,
                });
            }
            if now_seconds >= window.expires_at {
                return Err(AttestationMismatch::Expired {
                    verifier_id,
                    expires_at: window.expires_at,
                });
            }
            if window.not_before > now_seconds.saturating_add(MAX_ATTESTATION_CLOCK_SKEW_SECONDS) {
                return Err(AttestationMismatch::NotYetValid {
                    verifier_id,
                    not_before: window.not_before,
                });
            }
            let sig = base64_array::<64>(&signature.sig)
                .ok_or(AttestationMismatch::Malformed { field: "sig" })?;
            Ok((verifier_id, window, sig))
        })
        .collect()
}

/// Verifies each attestation under its verifier's key and that distinct
/// verifiers reach the threshold, counted as the chain counts them.
fn check_signatures(
    chain: &DelegateChain,
    fields: &DelegateRegisterFields<'_>,
    attestations: &[(u32, AttestationWindow, [u8; 64])],
    verifiers: &ProxyVerifierSet,
) -> Result<(), AttestationMismatch> {
    if verifiers.verifiers.is_empty() {
        return Err(AttestationMismatch::RegistrationClosed);
    }
    let mut distinct = BTreeSet::new();
    for &(verifier_id, window, sig) in attestations {
        let verifier = verifiers
            .verifiers
            .iter()
            .find(|verifier| verifier.verifier_id == verifier_id)
            .ok_or(AttestationMismatch::UnknownVerifier { verifier_id })?;
        let digest = attest_digest(chain.chain_id(), fields, &window)
            .expect("a checked chain id and canonical fields always encode");
        verify_zip215(&verifier.pubkey, &digest, &sig)
            .map_err(|_| AttestationMismatch::BadSignature { verifier_id })?;
        distinct.insert((verifier_id, verifier.pubkey));
    }
    if (distinct.len() as u64) < u64::from(verifiers.threshold) {
        return Err(AttestationMismatch::BelowThreshold {
            distinct: distinct.len(),
            threshold: verifiers.threshold,
        });
    }
    Ok(())
}
