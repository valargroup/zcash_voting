//! Real staging helper selection, separate from synthetic fan-out experiments.

use anyhow::{ensure, Result};
use recovery_conformance::{
    helper_fleet::{HelperFleetPlan, SYNTHETIC_HELPER_URLS},
    round_run::endpoints_from,
    run_config::Endpoints,
    stage_config::StageDeployment,
};
use zcash_voting::HelperFleetPreflight;

/// Helper selection shared by CLI parsing and hermetic configuration tests.
#[derive(Clone, Debug, clap::Args)]
pub struct HelperFleetArgs {
    /// Real helpers in published order: 1 = primary, 2 = primary and secondary.
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u32).range(1..=2))]
    helpers: u32,
    /// Synthetic identities all routed to primary; not independent servers.
    #[arg(long, conflicts_with = "helpers", value_parser = clap::value_parser!(u32).range(2..=10))]
    synthetic_helpers: Option<u32>,
}

/// Validated endpoints and optional synthetic routing for a benchmark run.
#[derive(Debug)]
pub struct SelectedHelpers {
    /// Chain, PIR, and selected helper endpoints written into the run config.
    pub endpoints: Endpoints,
    /// Empty for real helpers; explicit primary-backed routing for experiments.
    pub fleet: HelperFleetPlan,
}

impl HelperFleetArgs {
    /// Resolves helpers before provisioning, without network or storage effects.
    /// Rejects missing real endpoints and invalid or duplicate helper identities;
    /// never silently shrinks a requested two-helper run to the primary alone.
    /// Chain and PIR endpoints retain the published configuration in both modes.
    pub fn resolve(&self, deployment: &StageDeployment) -> Result<SelectedHelpers> {
        let mut endpoints = endpoints_from(deployment);
        let fleet = if let Some(count) = self.synthetic_helpers {
            ensure!(
                (2..=SYNTHETIC_HELPER_URLS.len()).contains(&(count as usize)),
                "unsupported synthetic helper count"
            );
            let primary = endpoints
                .vote_servers
                .first()
                .ok_or_else(|| anyhow::anyhow!("staging configuration has no primary helper"))?;
            let fleet = HelperFleetPlan::all_answering(primary.clone(), count as usize);
            endpoints.helper_urls = fleet.configured_urls();
            fleet
        } else {
            ensure!(
                endpoints.vote_servers.len() >= self.helpers as usize,
                "requested {} real helpers, but staging publishes only {} vote servers",
                self.helpers,
                endpoints.vote_servers.len()
            );
            endpoints.helper_urls = endpoints
                .vote_servers
                .iter()
                .take(self.helpers as usize)
                .cloned()
                .collect();
            HelperFleetPlan::none()
        };
        let validated = HelperFleetPreflight::from_readiness(&endpoints.helper_urls, &[])?;
        endpoints.helper_urls = validated.configured_server_urls().to_vec();
        Ok(SelectedHelpers { endpoints, fleet })
    }
}
