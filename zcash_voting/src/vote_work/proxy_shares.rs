//! The DC share step: deliver a landed DC's shares to the helpers, derived
//! again from the round hotkey.

use crate::{
    proxy::shares::{deliver_dc_shares, DcDeliveryProgress, DcDeliveryRequest},
    ChainTransport,
};

use super::{
    step_ledger::StepLedger, step_scope::StepScope, RoundExecutor, RoundStepDisposition,
    RoundStepFailure, RoundStepFailureKind, RoundStepOutcome,
};

impl<T: ChainTransport> RoundExecutor<T> {
    /// Delivers the shares of the DC in `bundle_index`'s slot `dc_slot` that
    /// are not handed off ([`deliver_dc_shares`]). Planning owes this step
    /// only before the vote end.
    ///
    /// Advances once every share is handed off. Stays pending while a later
    /// pass may still reconcile a short share's unknown outcome, or when the
    /// vote ended during the pass, and otherwise fails with
    /// `HelperDeliveryIncomplete`.
    #[allow(
        clippy::result_large_err,
        reason = "a step fails with the executor's RoundStepFailure, as every step does"
    )]
    pub(super) async fn run_deliver_dc_shares(
        &self,
        scope: &StepScope<'_>,
        bundle_index: u32,
        dc_slot: u8,
    ) -> Result<RoundStepOutcome, RoundStepFailure> {
        let ledger = StepLedger::default();
        let hotkey_secret = scope.hotkey_secret.clone().ok_or_else(|| {
            self.step_failure(
                RoundStepFailureKind::InvalidInput,
                Some(&scope.step),
                None,
                &ledger,
                "delivering a DC's shares requires the round voting hotkey",
            )
        })?;
        let helper_client = self.helper_client.observing(&scope.observations);
        let cancel = || scope.interrupted();
        let pass = deliver_dc_shares(
            &self.database,
            &helper_client,
            DcDeliveryRequest {
                round_id: &scope.round_id,
                bundle_index,
                dc_slot,
                hotkey_seed: hotkey_secret.as_slice(),
                configured_server_urls: &scope.host.configured_helper_urls,
                now_seconds: scope.host.now_seconds,
                wall_clock: (self.wall_clock)(),
                vote_end_time_seconds: scope.host.vote_end_time_seconds,
                last_moment_buffer_seconds: scope.host.last_moment_buffer_seconds(),
            },
            &cancel,
        )
        .await
        .map_err(|error| self.step_voting_failure(error, Some(&scope.step), &ledger))?;
        if pass.cancelled {
            return self.step_cancelled(scope, ledger);
        }
        let disposition = match pass.progress {
            DcDeliveryProgress::Complete => RoundStepDisposition::Advanced,
            DcDeliveryProgress::Unplanned => RoundStepDisposition::Pending,
            DcDeliveryProgress::Delivering if pass.outcome_unknown || pass.vote_ended => {
                RoundStepDisposition::Pending
            }
            DcDeliveryProgress::Delivering => {
                return Err(self.step_failure(
                    RoundStepFailureKind::HelperDeliveryIncomplete,
                    Some(&scope.step),
                    None,
                    &ledger,
                    "a DC share reached too few helpers",
                ))
            }
        };
        self.outcome(scope, disposition, ledger)
    }
}
