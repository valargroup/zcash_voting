//! Wire shapes of the static and dynamic voting config documents.
//!
//! Field names and serialization are part of the published config format and
//! of persisted [`super::ResolvedStaticVotingConfig`] values; do not rename.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{PirLayout, ServiceEndpoint, SupportedVersions};

pub(super) const STATIC_CONFIG_VERSION_V1: u32 = 1;
pub(super) const STATIC_CONFIG_VERSION_V2: u32 = 2;
pub(super) const DYNAMIC_CONFIG_VERSION: u32 = 1;
pub(super) const ALG_ED25519: &str = "ed25519";

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(super) struct WireStaticVotingConfig {
    pub(super) static_config_version: u32,
    /// Present on v1 documents only; forbidden on v2.
    #[serde(default)]
    pub(super) dynamic_config_url: Option<String>,
    /// Present on v2 documents only; forbidden on v1.
    #[serde(default)]
    pub(super) dynamic_config_urls: Option<Vec<String>>,
    pub(super) trusted_keys: Vec<TrustedKey>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(super) struct TrustedKey {
    pub(super) key_id: String,
    pub(super) alg: String,
    #[serde(with = "base64_bytes")]
    pub(super) pubkey: Vec<u8>,
    pub(super) notes: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(super) struct WireVotingServiceConfig {
    pub(super) config_version: u32,
    pub(super) vote_servers: Vec<ServiceEndpoint>,
    pub(super) pir_endpoints: Vec<ServiceEndpoint>,
    pub(super) pir_layout: PirLayout,
    pub(super) supported_versions: SupportedVersions,
    pub(super) rounds: BTreeMap<String, RoundEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(super) struct RoundEntry {
    pub(super) auth_version: u32,
    #[serde(with = "base64_bytes")]
    pub(super) ea_pk: Vec<u8>,
    pub(super) signatures: Vec<RoundSignature>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub(super) struct RoundSignature {
    pub(super) key_id: String,
    pub(super) alg: String,
    #[serde(with = "base64_bytes")]
    pub(super) sig: Vec<u8>,
}

pub(super) mod base64_bytes {
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
    use serde::Deserialize;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&BASE64.encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        BASE64.decode(value).map_err(serde::de::Error::custom)
    }
}
