//! Admission checks on a request and its bundle weights, before rounding.

use std::collections::BTreeSet;

use super::BundleBallots;
use zcash_vote_delegate::{is_canonical_handle, DelegateKey};

use crate::{
    proxy::{registry::identity_provider, MAX_PROXY_DELEGATES, WHOLE_ALLOCATION_BPS},
    wire::{ProxyDelegateChoice, ProxyDelegationError},
};

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
    choices: &[ProxyDelegateChoice],
    bundles: &[BundleBallots],
) -> Result<AdmittedBundles, ProxyDelegationError> {
    admit_choices(choices)?;
    admit_bundles(bundles)
}

/// Checks the choice count, each choice, and that the basis points total at
/// most the whole round.
fn admit_choices(choices: &[ProxyDelegateChoice]) -> Result<(), ProxyDelegationError> {
    if choices.is_empty() {
        return Err(ProxyDelegationError::other(
            "an allocation needs at least one delegate",
        ));
    }
    if choices.len() > MAX_PROXY_DELEGATES {
        return Err(ProxyDelegationError::other(format!(
            "an allocation names at most {MAX_PROXY_DELEGATES} delegates, got {}",
            choices.len()
        )));
    }
    let mut chosen = BTreeSet::new();
    for choice in choices {
        let delegate_index = choice.delegate_index;
        if delegate_index == 0 {
            return Err(ProxyDelegationError::other(
                "delegate index 0 is not a registry entry",
            ));
        }
        if !chosen.insert(delegate_index) {
            return Err(ProxyDelegationError::other(format!(
                "delegate {delegate_index} is chosen more than once"
            )));
        }
        if choice.percent_bps == 0 {
            return Err(ProxyDelegationError::other(format!(
                "delegate {delegate_index} needs a share of at least 1 basis point"
            )));
        }
        // A handle not canonical for its provider, or a key that is not a
        // canonical `zvdk1…` delegate key, no entry could hold.
        let provider = identity_provider(choice.provider);
        if !is_canonical_handle(provider, &choice.handle)
            || DelegateKey::decode(&choice.dk).is_err()
        {
            return Err(ProxyDelegationError::other(format!(
                "delegate {delegate_index}'s handle or key is malformed"
            )));
        }
    }

    let total_bps: u32 = choices
        .iter()
        .map(|choice| u32::from(choice.percent_bps))
        .sum();
    if total_bps > u32::from(WHOLE_ALLOCATION_BPS) {
        return Err(ProxyDelegationError::other(format!(
            "choices total {total_bps} basis points, more than 10000"
        )));
    }
    Ok(())
}

/// Orders the bundles by index and checks each holds weight, appears once,
/// and adds to a total that fits in `u64`.
fn admit_bundles(bundles: &[BundleBallots]) -> Result<AdmittedBundles, ProxyDelegationError> {
    if bundles.is_empty() {
        return Err(ProxyDelegationError::other(
            "the round has no bundle to delegate from",
        ));
    }
    let mut ordered = bundles.to_vec();
    ordered.sort_by_key(|bundle| bundle.bundle_index);
    let mut total_ballots = 0u64;
    for (position, bundle) in ordered.iter().enumerate() {
        let bundle_index = bundle.bundle_index;
        if position > 0 && ordered[position - 1].bundle_index == bundle_index {
            return Err(ProxyDelegationError::other(format!(
                "bundle {bundle_index} is listed twice"
            )));
        }
        if bundle.ballots == 0 {
            return Err(ProxyDelegationError::other(format!(
                "bundle {bundle_index} holds no ballots"
            )));
        }
        total_ballots = total_ballots.checked_add(bundle.ballots).ok_or_else(|| {
            ProxyDelegationError::other("the bundles' total weight overflows u64 ballots")
        })?;
    }
    Ok(AdmittedBundles {
        bundles: ordered,
        total_ballots,
    })
}
