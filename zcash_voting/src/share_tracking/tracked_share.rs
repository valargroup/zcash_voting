//! What a tracking pass needs of one stored share, vote or DC, and a vote
//! share's answers.

use crate::recovery as vote_recovery;
use crate::{
    share,
    share_policy::ShareSchedule,
    types::{ShareDelegationRecord, VotingError},
};

use super::{initial_delivery::VoteShareJournal, ShareAttemptJournal, StoredShare};

/// One stored share delivery or a tracking pass walks, bound to the
/// nullifier it was loaded with: the journal its attempts go to, and how to
/// reload, confirm and rebuild it for a resend.
pub(crate) trait TrackedShare: ShareAttemptJournal {
    /// The stored row.
    type Record: StoredShare;

    /// The share as stored now, or `None` once it was replaced or deleted.
    fn reload(&self) -> Result<Option<Self::Record>, VotingError>;

    /// The share's current helper state, keeping only helpers in
    /// `candidates`, or `None` once it was replaced or deleted.
    fn delivery_state(
        &self,
        candidates: &[String],
    ) -> Result<Option<share::ShareDeliveryState>, VotingError> {
        let Some(record) = self.reload()? else {
            return Ok(None);
        };
        let keep = |urls: &[String]| {
            urls.iter()
                .filter(|url| candidates.contains(url))
                .cloned()
                .collect::<Vec<_>>()
        };
        share::ShareDeliveryState::from_url_lists(
            &keep(record.sent_to_urls()),
            &keep(record.ambiguous_urls()),
            &keep(record.attempting_urls()),
        )
        .map(Some)
    }

    /// Whether the share is durably confirmed, or `None` once it was
    /// replaced or deleted.
    fn stored_confirmation(&self) -> Result<Option<bool>, VotingError> {
        Ok(self.reload()?.map(|record| record.is_confirmed()))
    }

    /// Durably confirms the share; `false` once it was replaced or deleted.
    fn mark_confirmed(&self) -> Result<bool, VotingError>;

    /// The share's helper request rebuilt for a resend with `submit_at`.
    fn resend_payload(
        &self,
        submit_at: u64,
        observations: &crate::ObservationScope,
    ) -> Result<ResendPayload, VotingError>;
}

/// A share's helper request rebuilt for a resend, or why it can't be.
pub(crate) enum ResendPayload {
    /// The request body.
    Ready(String),
    /// The vote is submitted, but its real VC tree position is not recorded.
    AwaitingVcPosition,
    /// The stored material is missing or cannot rebuild the share.
    Unrecoverable,
    /// The stored material rebuilds a share with another nullifier.
    NullifierMismatch,
}

impl<'a> VoteShareJournal<'a> {
    /// The journal of `share`, a vote share of `round_id` loaded under
    /// `scope`, for a tracking pass.
    pub(super) fn tracking(
        db: &'a crate::round::VotingDb,
        scope: &'a share::ShareOperationScope,
        round_id: &'a str,
        share: ShareDelegationRecord,
    ) -> Self {
        Self {
            db,
            scope,
            round_id,
            bundle_index: share.bundle_index,
            proposal_id: share.proposal_id,
            share_index: share.share_index,
            nullifier: share.nullifier,
        }
    }
}

impl TrackedShare for VoteShareJournal<'_> {
    type Record = ShareDelegationRecord;

    fn reload(&self) -> Result<Option<ShareDelegationRecord>, VotingError> {
        Ok(share::get_delegation_for_scope(
            self.db,
            self.scope,
            self.round_id,
            self.bundle_index,
            self.proposal_id,
            self.share_index,
        )?
        .filter(|share| share.nullifier == self.nullifier))
    }

    fn stored_confirmation(&self) -> Result<Option<bool>, VotingError> {
        self.db.share_is_confirmed_for_generation(
            self.scope.wallet_id(),
            self.round_id,
            self.bundle_index,
            self.proposal_id,
            self.share_index,
            Some(&self.nullifier),
        )
    }

    fn mark_confirmed(&self) -> Result<bool, VotingError> {
        share::confirm_for_generation(
            self.db,
            self.round_id,
            self.bundle_index,
            self.proposal_id,
            self.share_index,
            self.generation(),
        )
    }

    /// Rebuilds the request from the vote's persisted commitment bundle at
    /// its real VC tree position (see the invariants' "Recovery material").
    fn resend_payload(
        &self,
        submit_at: u64,
        observations: &crate::ObservationScope,
    ) -> Result<ResendPayload, VotingError> {
        let bundle = match vote_recovery::helper_recovery_material_for_wallet(
            self.db,
            self.scope.wallet_id(),
            self.round_id,
            self.bundle_index,
            self.proposal_id,
        )? {
            vote_recovery::HelperRecoveryMaterial::Ready(bundle) => bundle,
            vote_recovery::HelperRecoveryMaterial::AwaitingVcPosition => {
                return Ok(ResendPayload::AwaitingVcPosition);
            }
            vote_recovery::HelperRecoveryMaterial::Missing => {
                return Ok(ResendPayload::Unrecoverable);
            }
        };
        // Corrupt recovery material cannot be fixed by trying another helper.
        let Ok(wire_json) = share::observe_recover_wire_json(
            &bundle.commitment_bundle_json,
            self.proposal_id,
            self.share_index,
            bundle.vc_tree_position,
            submit_at,
            observations,
        ) else {
            return Ok(ResendPayload::Unrecoverable);
        };
        let Ok(nullifier) = share::nullifier_from_recovery_json(
            &bundle.commitment_bundle_json,
            self.proposal_id,
            self.share_index,
        ) else {
            return Ok(ResendPayload::Unrecoverable);
        };
        Ok(if nullifier.as_slice() == self.nullifier {
            ResendPayload::Ready(wire_json)
        } else {
            ResendPayload::NullifierMismatch
        })
    }
}
