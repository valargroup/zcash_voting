//! The dynamic config's `extensions.proxy_delegation_v1` (contract §12.1).
//! The URL lists are unsigned because everything fetched from them
//! authenticates itself or is display data.

use serde::{Deserialize, Serialize};

use super::RegistryError;

/// The URL lists of `extensions.proxy_delegation_v1`. An absent list reads
/// as empty.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxyServiceUrls {
    /// The verifier API the delegate app registers through.
    pub verifier_api_urls: Vec<String>,
    /// RPC hosts that serve the participation reader's proofs, tried in turn.
    pub proof_rpc_urls: Vec<String>,
}

/// Reads the URL lists from one dynamic config document, or `None` without
/// `extensions.proxy_delegation_v1`. Other members are ignored so older
/// configs stay readable. An absent list reads as empty, and config CI checks
/// list shapes. Fails with [`RegistryError::InvalidInput`] when the document
/// does not parse.
pub fn proxy_service_urls(
    dynamic_config_bytes: &[u8],
) -> Result<Option<ProxyServiceUrls>, RegistryError> {
    let document: DynamicConfigExtensions =
        serde_json::from_slice(dynamic_config_bytes).map_err(|error| {
            RegistryError::InvalidInput {
                message: format!("extensions.proxy_delegation_v1 does not parse: {error}"),
            }
        })?;
    Ok(document
        .extensions
        .and_then(|extensions| extensions.proxy_delegation_v1))
}

/// The parts of a dynamic config document this module reads.
#[derive(Deserialize)]
struct DynamicConfigExtensions {
    #[serde(default)]
    extensions: Option<ExtensionsWire>,
}

/// The dynamic config's `extensions` object.
#[derive(Deserialize)]
struct ExtensionsWire {
    #[serde(default)]
    proxy_delegation_v1: Option<ProxyServiceUrls>,
}
