//! Checks every step that starts new vote work runs first: the vote end and
//! the bound voting hotkey.

use zeroize::Zeroizing;

use crate::{ChainTransport, VotingHotkey};

use super::{
    step_ledger::StepLedger, step_scope::StepScope, RoundExecutor, RoundStepFailure,
    RoundStepFailureKind,
};

impl<T: ChainTransport> RoundExecutor<T> {
    /// Refuses new work with `VoteEnded` once the host clock reaches the
    /// authenticated vote end. `refused` ends the message and says what
    /// cannot start. Advancing work already on the wire is not new work.
    #[allow(
        clippy::result_large_err,
        reason = "a step fails with the executor's RoundStepFailure, as every step does"
    )]
    pub(super) fn refuse_after_vote_end(
        &self,
        scope: &StepScope<'_>,
        ledger: &StepLedger,
        refused: &str,
    ) -> Result<(), RoundStepFailure> {
        let host = scope.host;
        if let Some(vote_end) = host.vote_end_time_seconds {
            if host.now_seconds >= vote_end {
                return Err(self.step_failure(
                    RoundStepFailureKind::VoteEnded,
                    Some(&scope.step),
                    None,
                    ledger,
                    format!(
                        "vote ended at {vote_end} and the host clock reads {}; {refused}",
                        host.now_seconds
                    ),
                ));
            }
        }
        Ok(())
    }

    /// The round's voting hotkey secret, once its delegation target is
    /// checked to be the one `bundle_index`'s delegation targets. Otherwise
    /// a proof would fail only after a tree sync and a witness. A missing
    /// secret is `InvalidInput` with the message `missing`.
    #[allow(
        clippy::result_large_err,
        reason = "a step fails with the executor's RoundStepFailure, as every step does"
    )]
    pub(super) fn bound_hotkey_secret(
        &self,
        scope: &StepScope<'_>,
        bundle_index: u32,
        ledger: &StepLedger,
        missing: &str,
    ) -> Result<Zeroizing<Vec<u8>>, RoundStepFailure> {
        let hotkey_secret = scope.hotkey_secret.clone().ok_or_else(|| {
            self.step_failure(
                RoundStepFailureKind::InvalidInput,
                Some(&scope.step),
                None,
                ledger,
                missing,
            )
        })?;
        let bound_target =
            VotingHotkey::from_stored_secret(hotkey_secret.as_slice(), scope.network)
                .map_err(|error| self.step_voting_failure(error, Some(&scope.step), ledger))?
                .delegation_target();
        self.database
            .validate_bundle_hotkey_target(&scope.round_id, bundle_index, &bound_target)
            .map_err(|error| self.step_voting_failure(error, Some(&scope.step), ledger))?;
        Ok(hotkey_secret)
    }
}
