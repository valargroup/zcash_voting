//! Voting service config resolution.
//!
//! Wallets choose the static config source and fetch bytes using their own
//! transport. This module authenticates and normalizes those bytes, then
//! returns a resolved config and explicit switch plan for wallet state.
//!
//! ```text
//!     ┌──────────────┐
//!     │Wallet chooses│
//!     │static source │
//!     └──────┬───────┘
//!            │ fetch bytes with wallet transport
//!            ▼
//!     ┌──────────────┐      hash mismatch
//!     │Resolve static├──────────────────────┐
//!     │    config    │                      │
//!     └──────┬───────┘                      ▼
//!            │ dynamic_config_url     .───────────────.
//!            ▼                       ( Remote auth    )
//!     ┌──────────────┐                ( failed        )
//!     │Wallet fetches│                 `───────────────'
//!     │dynamic bytes │
//!     └──────┬───────┘
//!            ▼
//!     ┌──────────────┐
//!     │Resolve voting│
//!     │    config    │
//!     └──────┬───────┘
//!            │
//!            ▼
//!     ┌──────────────┐      bad signature
//!     │Authenticate  ├──────────────────────┐
//!     │ round entries│                      │
//!     └──────┬───────┘                      ▼
//!            │                       .───────────────.
//!            │ unsupported version  ( Config error   )
//!            ├─────────────────────> `───────────────'
//!            ▼
//!     ┌──────────────┐
//!     │Plan config   │
//!     │   switch     │
//!     └──────┬───────┘
//!            ▼
//!     ┌──────────────┐
//!     │Wallet applies│
//!     │ invalidation │
//!     └──────────────┘
//! ```
//!
//! The intended integration boundary is:
//!
//! - platform code owns URL choice and network transport;
//! - [`resolve_static_voting_config`] authenticates the static trust anchor and
//!   exposes the `dynamic_config_url` to fetch next;
//! - [`resolve_dynamic_voting_config`] takes that resolved static config plus
//!   the dynamic config bytes and returns a [`ResolvedVotingConfig`];
//! - [`decide_config_switch`] classifies the config change so the wallet can
//!   choose the correct state transition.
//!
//! Source parsing, hash pins, fingerprints, the mirror walk and the
//! attestation plumbing come from `zakura-service-config`. Voting keeps its
//! document schemas, version negotiation and round authentication: dynamic
//! documents are authenticated by the round-signature method in
//! `attestation`, which is this profile's only allowed method and its default,
//! so existing static documents keep their authentication rule unchanged.
//!
//! Children, in resolution order: `schema` (wire documents), `resolve`
//! (static then dynamic entry points), `validation` (schema and version
//! checks), `attestation` (round signatures), `mirror` (ordered mirror
//! attempts) and `switch` (state-transition classification).

mod attestation;
mod mirror;
mod resolve;
mod schema;
mod switch;
mod validation;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use schema::{base64_bytes, TrustedKey};

pub use mirror::{
    resolve_dynamic_voting_config_from_attempts, resolve_dynamic_voting_config_over_mirrors,
    DynamicConfigAttempt, DynamicConfigMirrorFailure, DYNAMIC_MIRROR_FETCH_TIMEOUT,
};
pub use resolve::{resolve_dynamic_voting_config, resolve_static_voting_config};
pub use switch::{
    decide_config_switch, ConfigSwitchDecision, ConfigSwitchKind, ResolvedVotingConfigSummary,
};
pub(crate) use validation::validate_and_convert_pir_layout;

const VERSION_V0: &str = "v0";
const VOTE_PROTOCOL_VERSION_V1: &str = "v1";
const VOTE_SERVER_VERSION_V1: &str = "v1";
const ROUND_PARAM_BYTE_LEN: usize = 32;

/// Versions of each voting-protocol component implemented by this crate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletCapabilities {
    pub vote_server: Vec<String>,
    pub vote_protocol: Vec<String>,
    pub tally: Vec<String>,
    pub pir: Vec<String>,
}

impl Default for WalletCapabilities {
    fn default() -> Self {
        Self {
            vote_server: vec![VOTE_SERVER_VERSION_V1.to_string()],
            // Both protocol generations are accepted so one wallet build
            // spans the server rollout: every deployed config advertises v0
            // today, and a config that moves to v1 must not strand it.
            vote_protocol: vec![VERSION_V0.to_string(), VOTE_PROTOCOL_VERSION_V1.to_string()],
            tally: vec![VERSION_V0.to_string()],
            pir: vec![VERSION_V0.to_string()],
        }
    }
}

/// Options that control voting config resolution.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveVotingConfigOptions {
    pub capabilities: WalletCapabilities,
}

/// Parsed static-config source chosen by the embedding wallet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinnedConfigSource {
    pub raw: String,
    pub url: String,
    pub sha256: Option<String>,
}

/// Endpoint advertised by a voting service config.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceEndpoint {
    pub url: String,
    pub label: String,
}

/// PIR tree geometry selected by the dynamic voting config.
///
/// Fixed-width fields keep this DTO stable for generated wallet bindings.
/// [`PirLayout::UNKNOWN`] is reserved for summaries persisted before layout
/// identity was recorded and is never accepted from dynamic config.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PirLayout {
    pub pir_depth: u32,
    pub tier0_layers: u32,
    pub tier1_layers: u32,
    /// YPIR RLWE polynomial degree (2048 or 4096).
    pub poly_len: u32,
}

impl PirLayout {
    /// Sentinel used when deserializing a legacy resolved-config summary.
    pub const UNKNOWN: Self = Self {
        pir_depth: 0,
        tier0_layers: 0,
        tier1_layers: 0,
        poly_len: 0,
    };
}

impl Default for PirLayout {
    /// Same sentinel as [`PirLayout::UNKNOWN`]; not a live layout for connect
    /// or dynamic config — only for legacy summary deserialization defaults.
    fn default() -> Self {
        Self::UNKNOWN
    }
}

/// Protocol component versions advertised by the dynamic config.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupportedVersions {
    pub pir: Vec<String>,
    pub vote_protocol: String,
    pub tally: String,
    pub vote_server: String,
}

/// Resolved and authenticated static voting config.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedStaticVotingConfig {
    pub source: PinnedConfigSource,
    pub source_fingerprint: String,
    pub trusted_key_fingerprint: String,
    /// First entry of [`Self::dynamic_config_urls`].
    ///
    /// Retained so callers written against the v1 single-URL schema keep
    /// working unchanged. Prefer `dynamic_config_urls` to get mirror fallback.
    pub dynamic_config_url: String,
    /// Ordered dynamic config mirrors, most preferred first.
    ///
    /// Always non-empty. A v1 static config yields exactly one entry; a v2
    /// static config yields its `dynamic_config_urls` list verbatim. Every
    /// mirror is expected to serve the same document, so falling back to a
    /// later entry widens availability without widening trust: rounds are
    /// still authenticated against [`Self::trusted_key_fingerprint`]'s keys.
    pub dynamic_config_urls: Vec<String>,
    /// `static_config_version` of the document this was resolved from.
    pub static_config_version: u32,
    trusted_keys: Vec<TrustedKey>,
}

/// Resolved dynamic voting config, ready for wallet use.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedVotingConfig {
    pub source_fingerprint: String,
    pub trusted_key_fingerprint: String,
    pub dynamic_config_fingerprint: String,
    pub vote_servers: Vec<ServiceEndpoint>,
    pub pir_endpoints: Vec<ServiceEndpoint>,
    pub pir_layout: PirLayout,
    pub supported_versions: SupportedVersions,
    pub authenticated_rounds: Vec<AuthenticatedRound>,
    pub skipped_round_ids: Vec<String>,
    pub conditions: Vec<ConfigCondition>,
}

/// Dynamic round metadata authenticated by trusted static keys.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthenticatedRound {
    pub round_id: String,
    #[serde(with = "base64_bytes")]
    pub ea_pk: Vec<u8>,
}

impl ResolvedVotingConfig {
    /// Build round params from server metadata while binding trusted `ea_pk`.
    ///
    /// Dynamic server fields (`snapshot_height`, roots) are accepted as inputs,
    /// but `ea_pk` is always sourced from authenticated dynamic config material.
    pub fn trusted_voting_round_params(
        &self,
        round_id: String,
        snapshot_height: u64,
        nc_root: Vec<u8>,
        nullifier_imt_root: Vec<u8>,
    ) -> Result<crate::wire::VotingRoundParams, VotingConfigError> {
        if nc_root.len() != ROUND_PARAM_BYTE_LEN {
            return Err(VotingConfigError::InvalidInput {
                message: format!("nc_root must be exactly {ROUND_PARAM_BYTE_LEN} bytes"),
            });
        }
        if nullifier_imt_root.len() != ROUND_PARAM_BYTE_LEN {
            return Err(VotingConfigError::InvalidInput {
                message: format!("nullifier_imt_root must be exactly {ROUND_PARAM_BYTE_LEN} bytes"),
            });
        }

        let trusted_round = self
            .authenticated_rounds
            .iter()
            .find(|round| round.round_id == round_id)
            .ok_or_else(|| VotingConfigError::RemoteAuthenticationFailed {
                message: format!("round {round_id} is not authenticated in resolved voting config"),
            })?;
        if trusted_round.ea_pk.len() != ROUND_PARAM_BYTE_LEN {
            return Err(VotingConfigError::InvalidInput {
                message: format!(
                    "authenticated round {round_id} has invalid ea_pk length: expected {ROUND_PARAM_BYTE_LEN} bytes"
                ),
            });
        }

        Ok(crate::wire::VotingRoundParams {
            vote_round_id: round_id,
            snapshot_height,
            ea_pk: trusted_round.ea_pk.clone(),
            nc_root,
            nullifier_imt_root,
        })
    }
}

/// Structured status emitted while resolving a voting config.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigCondition {
    pub kind: ConfigConditionKind,
    pub status: bool,
    pub message: String,
}

/// Machine-readable condition categories for config resolution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigConditionKind {
    StaticHashPinVerified,
    StaticConfigDecoded,
    DynamicConfigDecoded,
    DynamicSignaturesVerified,
    VersionsSupported,
    /// A dynamic config mirror other than the first was used.
    ///
    /// Only emitted by [`resolve_dynamic_voting_config_from_attempts`], and
    /// only when at least one earlier mirror was passed over.
    DynamicMirrorFallbackUsed,
}

/// Errors from voting config parsing, authentication, and compatibility checks.
#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VotingConfigError {
    #[error("invalid input: {message}")]
    InvalidInput { message: String },
    #[error("decode failed: {message}")]
    DecodeFailed { message: String },
    #[error("unsupported version for {component}: {advertised}")]
    UnsupportedVersion {
        component: String,
        advertised: String,
    },
    #[error("remote authentication failed: {message}")]
    RemoteAuthenticationFailed { message: String },
    /// Every dynamic config mirror was tried and none resolved.
    ///
    /// `message` enumerates each mirror URL and its reason, and each reason is
    /// the rendering of that mirror's own [`VotingConfigError`]. This variant
    /// exists because no single mirror's error can stand in for the whole set:
    /// mirrors commonly fail for different reasons, and picking one would
    /// discard the rest. Only produced by
    /// [`resolve_dynamic_voting_config_from_attempts`], and only when more than
    /// one mirror was attempted.
    #[error("all dynamic config mirrors failed: {message}")]
    AllMirrorsFailed { message: String },
}
