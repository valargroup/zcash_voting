//! Background tracking of a landed DC's handed-off shares, as a vote
//! share's are tracked once a helper accepted it (`share_tracking`), and
//! confirming one share's reveal through the configured helper quorum.

use super::{
    delivered_share_indexes,
    journal::{landed_dc_shares_on, load_dc_share, mark_dc_share_confirmed, DcShareJournal},
    payloads::{derive_dc_share_payloads, load_landed_dc},
    DcShareRecord,
};
use crate::{
    share::ShareOperationScope,
    share_policy::ShareSchedule,
    share_tracking::{
        ResendPayload, ShareAttemptJournal, ShareConfirmationParams, StoredShare, TrackedShare,
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

/// The journal of `params.share`, a share of the DC in `dc_slot` of its
/// bundle, bound to the nullifier it is stored with, for
/// [`confirm_pending_share`](crate::share_tracking::confirm_pending_share).
///
/// # Errors
///
/// [`VotingError::InvalidInput`] when the DC has no stored share at
/// `params.share.share_index`; storage errors unchanged.
pub(crate) fn stored_dc_share<'a>(
    db: &'a VotingDb,
    scope: &'a ShareOperationScope,
    params: &ShareConfirmationParams<'a>,
    dc_slot: u8,
) -> Result<DcShareJournal<'a>, VotingError> {
    let share = params.share;
    let record = db
        .read_transaction("load DC share", |tx| {
            load_dc_share(
                tx,
                params.round_id,
                scope.wallet_id(),
                share.bundle_index,
                dc_slot,
                share.share_index,
            )
        })?
        .ok_or_else(|| VotingError::InvalidInput {
            message: format!(
                "helper share not found: round={}, bundle={}, dc_slot={dc_slot}, share={}",
                params.round_id, share.bundle_index, share.share_index
            ),
        })?;
    Ok(DcShareJournal {
        db,
        scope,
        round_id: params.round_id,
        bundle_index: share.bundle_index,
        dc_slot,
        share_index: share.share_index,
        nullifier: record.nullifier,
    })
}

/// The handed-off, unconfirmed shares of `round_id`'s landed DCs, each with
/// its journal: the DC shares background tracking owns. A share not yet
/// handed off is owed to foreground delivery instead.
pub(crate) fn tracked_dc_shares<'a>(
    db: &'a VotingDb,
    scope: &'a ShareOperationScope,
    round_id: &'a str,
) -> Result<Vec<(DcShareJournal<'a>, DcShareRecord)>, VotingError> {
    let landed = db.read_transaction("load landed proxy DCs", |tx| {
        landed_dc_shares_on(tx, scope.wallet_id(), round_id)
    })?;
    Ok(landed
        .values()
        .flat_map(|dc| {
            dc.tracked_shares().map(|share| {
                let journal = DcShareJournal {
                    db,
                    scope,
                    round_id,
                    bundle_index: dc.bundle_index,
                    dc_slot: dc.dc_slot,
                    share_index: share.share_index,
                    nullifier: share.nullifier,
                };
                (journal, share.clone())
            })
        })
        .collect())
}

impl TrackedShare for DcShareJournal<'_> {
    type Record = DcShareRecord;

    fn reload(&self) -> Result<Option<DcShareRecord>, VotingError> {
        self.record()
    }

    fn mark_confirmed(&self) -> Result<bool, VotingError> {
        mark_dc_share_confirmed(
            self.db,
            self.scope,
            self.round_id,
            self.share(),
            self.dc_slot,
            &self.nullifier,
        )
    }

    /// Derives the share again from the round hotkey, as delivery does
    /// (`payloads`), refusing a hotkey that does not derive the landed DC.
    /// Nothing about a DC's secrets is stored, so without the hotkey the
    /// share is only polled.
    fn resend_payload(
        &self,
        submit_at: u64,
        round_hotkey_seed: Option<&[u8]>,
        _observations: &crate::ObservationScope,
    ) -> Result<ResendPayload, VotingError> {
        let Some(hotkey_seed) = round_hotkey_seed else {
            return Ok(ResendPayload::Unavailable);
        };
        let dc = match self.db.read_transaction("load landed proxy DC", |tx| {
            load_landed_dc(
                tx,
                self.round_id,
                self.scope.wallet_id(),
                self.bundle_index,
                self.dc_slot,
            )
        }) {
            Ok(dc) => dc,
            Err(error @ (VotingError::Storage { .. } | VotingError::DbBusy { .. })) => {
                return Err(error)
            }
            Err(_) => return Ok(ResendPayload::Unrecoverable),
        };
        let Ok(derived) = derive_dc_share_payloads(&dc, hotkey_seed) else {
            return Ok(ResendPayload::Unrecoverable);
        };
        let Some(position) = delivered_share_indexes(dc.layout)
            .iter()
            .position(|share_index| *share_index == self.share_index)
        else {
            return Ok(ResendPayload::Unrecoverable);
        };
        if derived.nullifiers[position] != self.nullifier {
            return Ok(ResendPayload::NullifierMismatch);
        }
        Ok(
            match derived.payloads[position].to_wire_json(Some(dc.tree_position), submit_at) {
                Ok(share_wire_json) => ResendPayload::Ready(share_wire_json),
                Err(_) => ResendPayload::Unrecoverable,
            },
        )
    }
}
