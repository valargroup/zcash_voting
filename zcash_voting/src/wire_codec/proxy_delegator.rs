//! Conversions between the delegator types of the client API and the proxy
//! module's own types.

use std::fmt;

use crate::{
    proxy::{AllocationPlan, AllocationPlanError, AllocationRequest, ProxyAllocationError},
    types::VotingError,
    wire::{
        ProxyAllocationPreview, ProxyAllocationRequest, ProxyDelegationError,
        ProxyDelegationErrorKind, ProxyPreviewDelegation,
    },
};

impl ProxyDelegationError {
    /// A failure of `kind`, described by `detail`.
    pub(crate) fn new(kind: ProxyDelegationErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: Some(detail.into()),
        }
    }

    /// A failure of kind [`ProxyDelegationErrorKind::Other`].
    pub(crate) fn other(detail: impl Into<String>) -> Self {
        Self::new(ProxyDelegationErrorKind::Other, detail)
    }
}

impl fmt::Display for ProxyDelegationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            ProxyDelegationErrorKind::CannotVerify => "cannot verify",
            ProxyDelegationErrorKind::VoteEnded => "vote ended",
            ProxyDelegationErrorKind::NotEnoughVotingPower => "not enough voting power",
            ProxyDelegationErrorKind::PlanChanged => "plan changed",
            ProxyDelegationErrorKind::AlreadyVoted => "already voted",
            ProxyDelegationErrorKind::RegistrationUnusable => "registration unusable",
            ProxyDelegationErrorKind::KeptShareUnanswered => "kept share unanswered",
            ProxyDelegationErrorKind::Other => "proxy delegation failed",
        };
        match &self.detail {
            Some(detail) => write!(f, "{kind}: {detail}"),
            None => f.write_str(kind),
        }
    }
}

impl std::error::Error for ProxyDelegationError {}

impl From<VotingError> for ProxyDelegationError {
    fn from(error: VotingError) -> Self {
        Self::other(error.to_string())
    }
}

impl From<ProxyAllocationError> for ProxyDelegationError {
    fn from(error: ProxyAllocationError) -> Self {
        let kind = match &error {
            ProxyAllocationError::Plan(AllocationPlanError::InsufficientWeight { .. }) => {
                ProxyDelegationErrorKind::NotEnoughVotingPower
            }
            ProxyAllocationError::PlanChanged => ProxyDelegationErrorKind::PlanChanged,
            ProxyAllocationError::AlreadyVoted { .. } => ProxyDelegationErrorKind::AlreadyVoted,
            ProxyAllocationError::RegistrationUnusable { .. } => {
                ProxyDelegationErrorKind::RegistrationUnusable
            }
            ProxyAllocationError::KeptShareUnanswered { .. } => {
                ProxyDelegationErrorKind::KeptShareUnanswered
            }
            _ => ProxyDelegationErrorKind::Other,
        };
        Self::new(kind, error.to_string())
    }
}

impl From<&ProxyAllocationRequest> for AllocationRequest {
    /// The planner's request. The round id is not part of it; the caller
    /// checks it.
    fn from(request: &ProxyAllocationRequest) -> Self {
        Self {
            choices: request.choices.clone(),
        }
    }
}

impl From<&AllocationPlan> for ProxyAllocationPreview {
    fn from(plan: &AllocationPlan) -> Self {
        // Both counts are bounded by the number of bundles, whose indexes are
        // `u32`, plus the 8-delegate cap.
        let saturating = |count: usize| u32::try_from(count).unwrap_or(u32::MAX);
        Self {
            delegations: plan
                .quotas()
                .iter()
                .map(|quota| ProxyPreviewDelegation {
                    delegate_index: quota.delegate_index,
                    ballots: quota.ballots,
                })
                .collect(),
            kept_ballots: plan.kept_ballots(),
            dc_count: saturating(plan.dc_count()),
            bundle_count: saturating(plan.bundle_count()),
            single_share: plan.single_share(),
        }
    }
}
