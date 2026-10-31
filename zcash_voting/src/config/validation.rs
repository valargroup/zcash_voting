//! Voting schema and version checks applied after JSON decoding.

use zakura_service_config::validate_https_url as validate_https_url_shape;

use super::{
    schema::{
        WireStaticVotingConfig, WireVotingServiceConfig, ALG_ED25519, DYNAMIC_CONFIG_VERSION,
        STATIC_CONFIG_VERSION_V1, STATIC_CONFIG_VERSION_V2,
    },
    PirLayout, ServiceEndpoint, VotingConfigError, WalletCapabilities,
};
use crate::types::validate_vote_round_id_hex;

/// Validate a static config document and normalize it to an ordered mirror list.
///
/// Each schema version owns exactly one URL field, so a document carrying the
/// other version's field is rejected rather than silently reinterpreted. The
/// returned list is non-empty and ordered most-preferred first.
pub(super) fn validate_static_config(
    config: &WireStaticVotingConfig,
) -> Result<Vec<String>, VotingConfigError> {
    let dynamic_config_urls = match config.static_config_version {
        STATIC_CONFIG_VERSION_V1 => {
            if config.dynamic_config_urls.is_some() {
                return Err(VotingConfigError::DecodeFailed {
                    message: "static_config_version 1 must not set dynamic_config_urls".to_string(),
                });
            }
            let url = config.dynamic_config_url.clone().ok_or_else(|| {
                VotingConfigError::DecodeFailed {
                    message: "static_config_version 1 requires dynamic_config_url".to_string(),
                }
            })?;
            validate_https_url(&url, "dynamic_config_url")?;
            vec![url]
        }
        STATIC_CONFIG_VERSION_V2 => {
            if config.dynamic_config_url.is_some() {
                return Err(VotingConfigError::DecodeFailed {
                    message: "static_config_version 2 must not set dynamic_config_url".to_string(),
                });
            }
            let urls = config.dynamic_config_urls.clone().ok_or_else(|| {
                VotingConfigError::DecodeFailed {
                    message: "static_config_version 2 requires dynamic_config_urls".to_string(),
                }
            })?;
            if urls.is_empty() {
                return Err(VotingConfigError::DecodeFailed {
                    message: "dynamic_config_urls must contain at least one entry".to_string(),
                });
            }
            for (index, url) in urls.iter().enumerate() {
                validate_https_url(url, &format!("dynamic_config_urls[{index}]"))?;
                // Duplicates would retry the same origin and mask a real
                // outage as extra attempts, so the publisher forbids them.
                if urls[..index].contains(url) {
                    return Err(VotingConfigError::DecodeFailed {
                        message: format!("dynamic_config_urls[{index}] is a duplicate: {url}"),
                    });
                }
            }
            urls
        }
        version => {
            return Err(VotingConfigError::DecodeFailed {
                message: format!("unsupported static_config_version {version}"),
            });
        }
    };

    if config.trusted_keys.is_empty() {
        return Err(VotingConfigError::DecodeFailed {
            message: "trusted_keys must contain at least one entry".to_string(),
        });
    }
    for key in &config.trusted_keys {
        if key.alg != ALG_ED25519 {
            return Err(VotingConfigError::DecodeFailed {
                message: format!("trusted_keys[{}].alg unsupported: {}", key.key_id, key.alg),
            });
        }
        if key.pubkey.len() != 32 {
            return Err(VotingConfigError::DecodeFailed {
                message: format!(
                    "trusted_keys[{}].pubkey must decode to 32 bytes",
                    key.key_id
                ),
            });
        }
    }
    Ok(dynamic_config_urls)
}

pub(super) fn validate_dynamic_config(
    config: &WireVotingServiceConfig,
    capabilities: &WalletCapabilities,
) -> Result<(), VotingConfigError> {
    if config.config_version != DYNAMIC_CONFIG_VERSION {
        return Err(VotingConfigError::DecodeFailed {
            message: format!("unsupported config_version {}", config.config_version),
        });
    }
    validate_endpoints(&config.vote_servers, "vote_servers")?;
    validate_endpoints(&config.pir_endpoints, "pir_endpoints")?;
    validate_pir_layout(config.pir_layout)?;
    for round_id in config.rounds.keys() {
        validate_vote_round_id_hex(round_id).map_err(|e| VotingConfigError::DecodeFailed {
            message: format!("invalid rounds key: {e}"),
        })?;
    }
    require_supported(
        "vote_server",
        &config.supported_versions.vote_server,
        &capabilities.vote_server,
    )?;
    require_supported(
        "vote_protocol",
        &config.supported_versions.vote_protocol,
        &capabilities.vote_protocol,
    )?;
    require_supported(
        "tally",
        &config.supported_versions.tally,
        &capabilities.tally,
    )?;
    if config
        .supported_versions
        .pir
        .iter()
        .all(|v| !capabilities.pir.contains(v))
    {
        return Err(VotingConfigError::UnsupportedVersion {
            component: "pir".to_string(),
            advertised: config.supported_versions.pir.join(","),
        });
    }
    Ok(())
}

fn validate_pir_layout(layout: PirLayout) -> Result<(), VotingConfigError> {
    validate_and_convert_pir_layout(layout)
        .map(|_| ())
        .map_err(|message| VotingConfigError::DecodeFailed { message })
}

pub(crate) fn validate_and_convert_pir_layout(
    layout: PirLayout,
) -> Result<pir_types::PirLayout, String> {
    let negotiated = pir_types::PirLayout {
        pir_depth: usize::try_from(layout.pir_depth).map_err(|_| {
            format!(
                "pir_layout.pir_depth {} does not fit usize",
                layout.pir_depth
            )
        })?,
        tier0_layers: usize::try_from(layout.tier0_layers).map_err(|_| {
            format!(
                "pir_layout.tier0_layers {} does not fit usize",
                layout.tier0_layers
            )
        })?,
        tier1_layers: usize::try_from(layout.tier1_layers).map_err(|_| {
            format!(
                "pir_layout.tier1_layers {} does not fit usize",
                layout.tier1_layers
            )
        })?,
        poly_len: usize::try_from(layout.poly_len)
            .map_err(|_| format!("pir_layout.poly_len {} does not fit usize", layout.poly_len))?,
    };
    negotiated.validate_supported()?;
    Ok(negotiated)
}

fn require_supported(
    component: &str,
    advertised: &str,
    supported: &[String],
) -> Result<(), VotingConfigError> {
    if supported.iter().any(|v| v == advertised) {
        Ok(())
    } else {
        Err(VotingConfigError::UnsupportedVersion {
            component: component.to_string(),
            advertised: advertised.to_string(),
        })
    }
}

fn validate_endpoints(endpoints: &[ServiceEndpoint], field: &str) -> Result<(), VotingConfigError> {
    if endpoints.is_empty() {
        return Err(VotingConfigError::DecodeFailed {
            message: format!("{field} must contain at least one entry"),
        });
    }
    for (index, endpoint) in endpoints.iter().enumerate() {
        validate_https_url(&endpoint.url, &format!("{field}[{index}].url"))?;
    }
    Ok(())
}

/// Checks an `https://` URL with a host, reporting the failure as
/// [`VotingConfigError::InvalidInput`] naming `field`.
pub(super) fn validate_https_url(value: &str, field: &str) -> Result<(), VotingConfigError> {
    validate_https_url_shape(value, field).map_err(|e| VotingConfigError::InvalidInput {
        message: e.to_string(),
    })
}
