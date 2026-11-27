//! Admission checks on a request and its bundle weights, before rounding.

use std::collections::BTreeSet;

use super::{AllocationPlanError, AllocationRequest, BundleBallots};
use zcash_vote_delegate::{is_canonical_handle, DelegateKey};

use crate::proxy::{registry::identity_provider, MAX_PROXY_DELEGATES, WHOLE_ALLOCATION_BPS};

/// Bundle weights that passed admission.
pub(super) struct AdmittedBundles {
    /// Every bundle, ordered by index.
    pub(super) bundles: Vec<BundleBallots>,
    /// Their total weight, `T`.
    pub(super) total_ballots: u64,
}

/// Checks the choices and bundles, and returns the bundles in
/// index order with their total. Rounding checks the one-ballot floor.
pub(super) fn admit(
    request: &AllocationRequest,
    bundles: &[BundleBallots],
) -> Result<AdmittedBundles, AllocationPlanError> {
    admit_choices(request)?;
    admit_bundles(bundles)
}

/// Checks the choice count, each choice, and that the basis points total at
/// most the whole round.
fn admit_choices(request: &AllocationRequest) -> Result<(), AllocationPlanError> {
    let choices = &request.choices;
    if choices.is_empty() {
        return Err(AllocationPlanError::NoDelegates);
    }
    if choices.len() > MAX_PROXY_DELEGATES {
        return Err(AllocationPlanError::TooManyDelegates {
            count: choices.len(),
        });
    }
    let mut chosen = BTreeSet::new();
    for choice in choices {
        if choice.delegate_index == 0 {
            return Err(AllocationPlanError::ZeroDelegateIndex);
        }
        if !chosen.insert(choice.delegate_index) {
            return Err(AllocationPlanError::DuplicateDelegate {
                delegate_index: choice.delegate_index,
            });
        }
        if choice.percent_bps == 0 {
            return Err(AllocationPlanError::ZeroBasisPoints {
                delegate_index: choice.delegate_index,
            });
        }
        let provider = identity_provider(choice.provider);
        if !is_canonical_handle(provider, &choice.handle)
            || DelegateKey::decode(&choice.dk).is_err()
        {
            return Err(AllocationPlanError::InvalidDelegateIdentity {
                delegate_index: choice.delegate_index,
            });
        }
    }

    let total_bps: u32 = choices
        .iter()
        .map(|choice| u32::from(choice.percent_bps))
        .sum();
    if total_bps > u32::from(WHOLE_ALLOCATION_BPS) {
        return Err(AllocationPlanError::BasisPointsExceedWhole { total_bps });
    }
    Ok(())
}

/// Orders the bundles by index and checks each holds weight, appears once,
/// and adds to a total that fits in `u64`.
fn admit_bundles(bundles: &[BundleBallots]) -> Result<AdmittedBundles, AllocationPlanError> {
    if bundles.is_empty() {
        return Err(AllocationPlanError::NoBundles);
    }
    let mut ordered = bundles.to_vec();
    ordered.sort_by_key(|bundle| bundle.bundle_index);
    let mut total_ballots = 0u64;
    for (position, bundle) in ordered.iter().enumerate() {
        if position > 0 && ordered[position - 1].bundle_index == bundle.bundle_index {
            return Err(AllocationPlanError::DuplicateBundle {
                bundle_index: bundle.bundle_index,
            });
        }
        if bundle.ballots == 0 {
            return Err(AllocationPlanError::EmptyBundle {
                bundle_index: bundle.bundle_index,
            });
        }
        total_ballots = total_ballots
            .checked_add(bundle.ballots)
            .ok_or(AllocationPlanError::WeightOverflow)?;
    }
    Ok(AdmittedBundles {
        bundles: ordered,
        total_ballots,
    })
}
