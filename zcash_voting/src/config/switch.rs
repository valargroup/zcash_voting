//! Classifying the wallet state transition between two resolved configs.

use serde::{Deserialize, Serialize};
use zakura_service_config::fingerprint_json;

use super::{PirLayout, ResolvedVotingConfig, SupportedVersions};

fn deserialize_summary_pir_layout<'de, D>(deserializer: D) -> Result<PirLayout, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct PersistedPirLayout {
        pir_depth: u32,
        tier0_layers: u32,
        tier1_layers: u32,
        #[serde(default)]
        poly_len: u32,
    }

    let layout = PersistedPirLayout::deserialize(deserializer)?;
    Ok(PirLayout {
        pir_depth: layout.pir_depth,
        tier0_layers: layout.tier0_layers,
        tier1_layers: layout.tier1_layers,
        poly_len: layout.poly_len,
    })
}

/// Small, stable summary used to decide how much state a config switch
/// invalidates.
///
/// The static source fingerprint is intentionally absent. A new static URL or
/// hash pin can resolve to the same operational service. Switching wallet state
/// should be driven by the resolved service identity below, not by where the
/// wallet fetched the trust anchor from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedVotingConfigSummary {
    pub trusted_key_fingerprint: String,
    pub vote_server_fingerprint: String,
    pub pir_endpoint_fingerprint: String,
    /// Defaults to [`PirLayout::UNKNOWN`] for summaries persisted by older
    /// versions so the next known layout is treated as a service update.
    #[serde(default, deserialize_with = "deserialize_summary_pir_layout")]
    pub pir_layout: PirLayout,
    pub authenticated_round_set_fingerprint: String,
    pub protocol_versions: SupportedVersions,
}

impl From<&ResolvedVotingConfig> for ResolvedVotingConfigSummary {
    fn from(config: &ResolvedVotingConfig) -> Self {
        let authenticated_round_ids = config
            .authenticated_rounds
            .iter()
            .map(|round| round.round_id.clone())
            .collect::<Vec<_>>();
        Self {
            trusted_key_fingerprint: config.trusted_key_fingerprint.clone(),
            vote_server_fingerprint: fingerprint_json(&config.vote_servers),
            pir_endpoint_fingerprint: fingerprint_json(&config.pir_endpoints),
            pir_layout: config.pir_layout,
            authenticated_round_set_fingerprint: fingerprint_json(&authenticated_round_ids),
            protocol_versions: config.supported_versions.clone(),
        }
    }
}

/// Semantic decision for moving between resolved configs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigSwitchDecision {
    pub kind: ConfigSwitchKind,
}

/// High-level meaning of a resolved config switch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSwitchKind {
    /// The resolved config summary is unchanged.
    Unchanged,
    /// There was no prior resolved config summary.
    InitialLoad,
    /// Same authenticated round set and protocol tuple, but endpoint, PIR
    /// layout, or trusted-signing-key material changed.
    ///
    /// Wallets should restart endpoint caches, status polls, share tracking,
    /// and PIR/delegation precompute. They should keep round-id-indexed wallet
    /// artifacts.
    SameChainServiceUpdate,
    /// The authenticated round set changed.
    ///
    /// Wallets should treat this as moving to a new chain/round context for
    /// active UI and in-flight work. Durable state for old round ids can remain
    /// indexed by round id, but the current visible round context should be
    /// discarded or reselected from the reloaded authenticated round list.
    NewChainOrRound,
    /// The resolved protocol-version tuple changed.
    ///
    /// Wallets should stop using cached voting state until compatibility is
    /// re-established.
    ProtocolChanged,
}

/// Classify the wallet transition required to switch to `next`.
pub fn decide_config_switch(
    current: Option<ResolvedVotingConfigSummary>,
    next: ResolvedVotingConfigSummary,
) -> ConfigSwitchDecision {
    let Some(current) = current else {
        return ConfigSwitchDecision {
            kind: ConfigSwitchKind::InitialLoad,
        };
    };

    let kind = if current.protocol_versions != next.protocol_versions {
        ConfigSwitchKind::ProtocolChanged
    } else if current.authenticated_round_set_fingerprint
        != next.authenticated_round_set_fingerprint
    {
        ConfigSwitchKind::NewChainOrRound
    } else if current.trusted_key_fingerprint != next.trusted_key_fingerprint
        || current.vote_server_fingerprint != next.vote_server_fingerprint
        || current.pir_endpoint_fingerprint != next.pir_endpoint_fingerprint
        || current.pir_layout != next.pir_layout
    {
        ConfigSwitchKind::SameChainServiceUpdate
    } else {
        ConfigSwitchKind::Unchanged
    };

    ConfigSwitchDecision { kind }
}
