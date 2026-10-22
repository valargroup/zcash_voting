//! Static and dynamic resolution entry points.

use zakura_service_config::{fingerprint_json, resolve_dynamic, resolve_static, ConfigSource};

use super::{
    attestation::VotingProfile, schema::WireStaticVotingConfig, validation::validate_static_config,
    ConfigCondition, ConfigConditionKind, PinnedConfigSource, ResolveVotingConfigOptions,
    ResolvedStaticVotingConfig, ResolvedVotingConfig, VotingConfigError,
};

/// Parse and authenticate the static config bytes selected by the wallet.
pub fn resolve_static_voting_config(
    source: &str,
    static_config_bytes: &[u8],
) -> Result<ResolvedStaticVotingConfig, VotingConfigError> {
    let resolved = resolve_static::<WireStaticVotingConfig>(source, static_config_bytes)?;
    let static_config = resolved.document;
    let dynamic_config_urls = validate_static_config(&static_config)?;
    let source = PinnedConfigSource::from(resolved.source);

    Ok(ResolvedStaticVotingConfig {
        source_fingerprint: source.fingerprint(),
        trusted_key_fingerprint: fingerprint_json(&static_config.trusted_keys),
        source,
        // Validation guarantees at least one mirror, so index 0 always exists.
        dynamic_config_url: dynamic_config_urls[0].clone(),
        dynamic_config_urls,
        static_config_version: static_config.static_config_version,
        trusted_keys: static_config.trusted_keys,
    })
}

/// Resolve and authenticate the dynamic voting config from supplied bytes.
///
/// The static trust anchor must already be resolved through a separate
/// [`resolve_static_voting_config`] call. Given that authenticated static config
/// and the dynamic config bytes the wallet fetched from
/// `resolved_static.dynamic_config_url`, this validates advertised versions and
/// authenticates round entries against the static trusted keys.
pub fn resolve_dynamic_voting_config(
    resolved_static: ResolvedStaticVotingConfig,
    dynamic_bytes: &[u8],
    options: ResolveVotingConfigOptions,
) -> Result<ResolvedVotingConfig, VotingConfigError> {
    let profile = VotingProfile::new(options.capabilities);
    let resolved = resolve_dynamic(&profile, &resolved_static, dynamic_bytes)?;
    let dynamic_config = resolved.document;
    let rounds = resolved.attestation.verdict;

    let authenticated_count = rounds.authenticated_rounds.len();
    let skipped_count = rounds.skipped_round_ids.len();
    // The hash pin is optional: a source without `?checksum=sha256:` is
    // resolvable but unpinned, and the condition must say so rather than
    // claim a verification that never ran.
    let hash_pin_verified = resolved_static.source.sha256.is_some();

    Ok(ResolvedVotingConfig {
        source_fingerprint: resolved_static.source_fingerprint,
        trusted_key_fingerprint: resolved_static.trusted_key_fingerprint,
        dynamic_config_fingerprint: resolved.fingerprint,
        vote_servers: dynamic_config.vote_servers,
        pir_endpoints: dynamic_config.pir_endpoints,
        pir_layout: dynamic_config.pir_layout,
        supported_versions: dynamic_config.supported_versions,
        authenticated_rounds: rounds.authenticated_rounds,
        skipped_round_ids: rounds.skipped_round_ids,
        conditions: vec![
            ConfigCondition {
                kind: ConfigConditionKind::StaticHashPinVerified,
                status: hash_pin_verified,
                message: if hash_pin_verified {
                    "static hash pin verified".to_string()
                } else {
                    "static config source carried no hash pin".to_string()
                },
            },
            ConfigCondition {
                kind: ConfigConditionKind::DynamicConfigDecoded,
                status: true,
                message: "dynamic config decoded".to_string(),
            },
            ConfigCondition {
                kind: ConfigConditionKind::DynamicSignaturesVerified,
                status: true,
                message: format!(
                    "dynamic round signatures verified: authenticated={}, skipped={}",
                    authenticated_count, skipped_count
                ),
            },
            ConfigCondition {
                kind: ConfigConditionKind::VersionsSupported,
                status: true,
                message: "advertised versions are supported".to_string(),
            },
        ],
    })
}

impl PinnedConfigSource {
    /// Parse an HTTPS static-config source with an optional
    /// `checksum=sha256:{hex}` query item.
    pub fn parse(raw: &str) -> Result<Self, VotingConfigError> {
        ConfigSource::parse(raw)
            .map(Self::from)
            .map_err(|e| VotingConfigError::InvalidInput {
                message: e.to_string(),
            })
    }

    /// Stable fingerprint for comparing config sources across runs.
    pub fn fingerprint(&self) -> String {
        fingerprint_json(self)
    }
}

impl From<ConfigSource> for PinnedConfigSource {
    fn from(source: ConfigSource) -> Self {
        Self {
            raw: source.raw,
            url: source.url,
            sha256: source.sha256,
        }
    }
}
