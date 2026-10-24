//! Round authentication: the voting profile's attestation method.
//!
//! A dynamic voting document is not signed as a whole. Each round entry
//! carries ed25519 signatures over its round auth v2 payload (round id, EA
//! key and PIR layout) by keys in the static document. Entries that do not
//! verify are skipped, not fatal, so the verdict partitions rounds into
//! authenticated and skipped ones.

use std::collections::BTreeMap;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use zakura_service_config::{
    AttestationError, AttestationMethod, AttestationPolicy, AttestationSubject,
    AttestationVerifier, ConfigProfile, ResolveError,
};

use super::{
    schema::{RoundEntry, TrustedKey, WireVotingServiceConfig, ALG_ED25519},
    validation::validate_dynamic_config,
    AuthenticatedRound, PirLayout, ResolvedStaticVotingConfig, VotingConfigError,
    WalletCapabilities,
};
use crate::round_auth::{RoundAuthPayloadV2, ROUND_AUTH_VERSION_V2};

/// Registered name of the round-signature method.
const ROUND_SIGNATURES_METHOD: &str = "voting-round-auth-v2";

/// The voting application profile: dynamic documents are checked against the
/// wallet's capabilities and authenticated by round signatures.
///
/// Voting static documents have no attestation field, so the policy's default
/// (and only) method always applies.
pub(super) struct VotingProfile {
    capabilities: WalletCapabilities,
    attestation:
        AttestationPolicy<ResolvedStaticVotingConfig, WireVotingServiceConfig, AuthenticatedRounds>,
}

impl VotingProfile {
    pub(super) fn new(capabilities: WalletCapabilities) -> Self {
        let method = AttestationMethod::new(ROUND_SIGNATURES_METHOD)
            .expect("round-signature method name is valid");
        let attestation = AttestationPolicy::new()
            .allow(method.clone(), RoundSignatures)
            .and_then(|policy| policy.with_default(method))
            .expect("a single registered method is a valid default");
        Self {
            capabilities,
            attestation,
        }
    }
}

impl ConfigProfile for VotingProfile {
    type Anchor = ResolvedStaticVotingConfig;
    type Dynamic = WireVotingServiceConfig;
    type Verdict = AuthenticatedRounds;
    type Error = VotingConfigError;

    fn attestation(
        &self,
    ) -> &AttestationPolicy<ResolvedStaticVotingConfig, WireVotingServiceConfig, AuthenticatedRounds>
    {
        &self.attestation
    }

    fn validate_dynamic(
        &self,
        _anchor: &ResolvedStaticVotingConfig,
        document: &WireVotingServiceConfig,
    ) -> Result<(), VotingConfigError> {
        validate_dynamic_config(document, &self.capabilities)
    }
}

impl From<ResolveError> for VotingConfigError {
    /// Keeps the kinds and messages voting reported before the shared crate.
    fn from(error: ResolveError) -> Self {
        let message = error.to_string();
        match error {
            ResolveError::Source(_) => Self::InvalidInput { message },
            ResolveError::StaticDecode(_) | ResolveError::DynamicDecode(_) => {
                Self::DecodeFailed { message }
            }
            ResolveError::PinMismatch(_) | ResolveError::Attestation(_) => {
                Self::RemoteAuthenticationFailed { message }
            }
        }
    }
}

/// Rounds whose signatures verified, and the ids of those that did not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AuthenticatedRounds {
    pub(super) authenticated_rounds: Vec<AuthenticatedRound>,
    pub(super) skipped_round_ids: Vec<String>,
}

/// Verifies round entries against the static document's trusted keys.
struct RoundSignatures;

impl AttestationVerifier<ResolvedStaticVotingConfig, WireVotingServiceConfig> for RoundSignatures {
    type Verdict = AuthenticatedRounds;

    /// Never rejects the document: an unverifiable round is skipped.
    fn verify(
        &self,
        subject: AttestationSubject<'_, ResolvedStaticVotingConfig, WireVotingServiceConfig>,
    ) -> Result<AuthenticatedRounds, AttestationError> {
        Ok(authenticate_dynamic_rounds(
            &subject.document.rounds,
            &subject.anchor.trusted_keys,
            subject.document.pir_layout,
        ))
    }
}

fn authenticate_dynamic_rounds(
    rounds: &BTreeMap<String, RoundEntry>,
    trusted_keys: &[TrustedKey],
    pir_layout: PirLayout,
) -> AuthenticatedRounds {
    let mut authenticated_rounds = Vec::new();
    let mut skipped_round_ids = Vec::new();
    for (round_id, entry) in rounds {
        if !verify_round_entry(round_id, entry, trusted_keys, pir_layout) {
            skipped_round_ids.push(round_id.clone());
            continue;
        }
        authenticated_rounds.push(AuthenticatedRound {
            round_id: round_id.clone(),
            ea_pk: entry.ea_pk.clone(),
        });
    }
    AuthenticatedRounds {
        authenticated_rounds,
        skipped_round_ids,
    }
}

fn verify_round_entry(
    round_id: &str,
    entry: &RoundEntry,
    trusted_keys: &[TrustedKey],
    pir_layout: PirLayout,
) -> bool {
    if entry.auth_version != ROUND_AUTH_VERSION_V2
        || entry.ea_pk.len() != 32
        || entry.signatures.is_empty()
    {
        return false;
    }
    // Round keys are hex-validated during dynamic-config validation, but decode
    // defensively: an undecodable round id can never authenticate.
    let Ok(round_id_bytes) = hex::decode(round_id) else {
        return false;
    };
    let Ok(round_id_bytes) = <[u8; 32]>::try_from(round_id_bytes.as_slice()) else {
        return false;
    };
    let Ok(ea_pk) = <[u8; 32]>::try_from(entry.ea_pk.as_slice()) else {
        return false;
    };
    let payload = RoundAuthPayloadV2::new(round_id_bytes, ea_pk, pir_layout).to_bytes();

    for signature in &entry.signatures {
        let Some(key) = trusted_keys
            .iter()
            .find(|key| key.key_id == signature.key_id)
        else {
            continue;
        };
        if key.alg != ALG_ED25519 || signature.alg != key.alg || signature.sig.len() != 64 {
            continue;
        }

        let Ok(pubkey_bytes) = <[u8; 32]>::try_from(key.pubkey.as_slice()) else {
            continue;
        };
        let Ok(sig_bytes) = <[u8; 64]>::try_from(signature.sig.as_slice()) else {
            continue;
        };
        let Ok(verifying_key) = VerifyingKey::from_bytes(&pubkey_bytes) else {
            continue;
        };
        let sig = Signature::from_bytes(&sig_bytes);
        if verifying_key.verify(&payload, &sig).is_ok() {
            return true;
        }
    }
    false
}
