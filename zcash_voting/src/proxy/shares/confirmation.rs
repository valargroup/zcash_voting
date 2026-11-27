//! Confirming one DC share's reveal through the configured helper quorum.

use super::{
    journal::{load_dc_share, mark_dc_share_confirmed},
    DcShareRecord,
};
use crate::{
    helper::client::HelperClient,
    share::ShareOperationScope,
    share_policy::ShareSchedule,
    share_tracking::{
        confirm_stored_share, ShareConfirmationParams, ShareConfirmationReport, StoredShare,
    },
    storage::VotingDb,
    types::VotingError,
};

impl ShareSchedule for DcShareRecord {
    fn submit_at(&self) -> u64 {
        self.submit_at
    }

    fn created_at(&self) -> u64 {
        self.created_at
    }

    fn is_confirmed(&self) -> bool {
        self.confirmed
    }
}

impl StoredShare for DcShareRecord {
    fn reveal_nullifier(&self) -> &[u8] {
        &self.nullifier
    }

    fn target_count(&self) -> u32 {
        self.target_count
    }

    fn sent_to_urls(&self) -> &[String] {
        &self.sent_to_urls
    }

    fn ambiguous_urls(&self) -> &[String] {
        &self.ambiguous_urls
    }

    fn attempting_urls(&self) -> &[String] {
        &self.attempting_urls
    }
}

/// [`confirm_pending_share`](crate::share_tracking::confirm_pending_share)
/// for a share of the DC in `dc_slot` of `params.share`'s bundle.
///
/// Confirms the share as a vote share is confirmed (see
/// `confirm_stored_share`). On the quorum it marks the share confirmed only
/// if it still has the nullifier it was loaded with. It never POSTs.
///
/// # Errors
///
/// [`VotingError::InvalidInput`] when the DC has no stored share at
/// `params.share.share_index`; storage errors unchanged.
pub(crate) async fn confirm_dc_share(
    db: &VotingDb,
    scope: &ShareOperationScope,
    params: &ShareConfirmationParams<'_>,
    dc_slot: u8,
    configured_urls: &[String],
    client: &HelperClient,
    cancel: &(dyn Fn() -> bool + Send + Sync),
) -> Result<ShareConfirmationReport, VotingError> {
    let share = params.share;
    confirm_stored_share(
        scope,
        params,
        configured_urls,
        client,
        cancel,
        || {
            db.read_transaction("load DC share", |tx| {
                load_dc_share(
                    tx,
                    params.round_id,
                    scope.wallet_id(),
                    share.bundle_index,
                    dc_slot,
                    share.share_index,
                )
            })
        },
        || VotingError::InvalidInput {
            message: format!(
                "helper share not found: round={}, bundle={}, dc_slot={dc_slot}, share={}",
                params.round_id, share.bundle_index, share.share_index
            ),
        },
        |current| {
            mark_dc_share_confirmed(
                db,
                scope,
                params.round_id,
                share,
                dc_slot,
                &current.nullifier,
            )
        },
    )
    .await
}
