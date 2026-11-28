//! Planner properties over random wallets and requests (plan §9.1):
//! conservation, `D ≤ #DC ≤ D + B − 1`, quotas within one ballot of their
//! exact shares, at most eight delegates, and never two DCs to one delegate
//! in a bundle.

use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;

use super::fixtures::{bundles, planning_case};
use crate::proxy::{
    plan_allocation, AllocationPlan, AllocationPlanError, AllocationRequest, MAX_PROXY_DELEGATES,
    WHOLE_ALLOCATION_BPS,
};

/// `T · bps` in ten-thousandths of a ballot: a delegate's exact share scaled
/// so it is an integer.
fn exact_share_scaled(total_ballots: u64, bps: u16) -> u128 {
    u128::from(total_ballots) * u128::from(bps)
}

/// Checks every planner property for one planned case.
fn check_plan(request: &AllocationRequest, weights: &[u64], plan: &AllocationPlan) {
    let total: u64 = weights.iter().sum();
    let delegates = request.choices.len();
    let whole = u128::from(WHOLE_ALLOCATION_BPS);

    // Conservation: quotas and the kept remainder make up the whole weight,
    // the DCs make up each quota, and no bundle gives more than it holds.
    let quota_sum: u64 = plan.quotas().iter().map(|quota| quota.ballots).sum();
    assert_eq!(quota_sum + plan.kept_ballots(), total);
    assert_eq!(plan.total_ballots(), total);
    let mut per_delegate: BTreeMap<u32, u64> = BTreeMap::new();
    let mut per_bundle: BTreeMap<u32, u64> = BTreeMap::new();
    for dc in plan.dcs() {
        assert!(dc.weight_ballots >= 1);
        *per_delegate.entry(dc.delegate_index).or_default() += dc.weight_ballots;
        *per_bundle.entry(dc.bundle_index).or_default() += dc.weight_ballots;
    }
    for quota in plan.quotas() {
        assert_eq!(
            per_delegate.get(&quota.delegate_index),
            Some(&quota.ballots)
        );
    }
    let mut unused = 0;
    for (bundle_index, &weight) in weights.iter().enumerate() {
        let used = per_bundle.get(&(bundle_index as u32)).copied().unwrap_or(0);
        assert!(used <= weight);
        unused += weight - used;
    }
    assert_eq!(unused, plan.kept_ballots());
    let chosen_bps: u16 = request.choices.iter().map(|c| c.percent_bps).sum();
    if chosen_bps == WHOLE_ALLOCATION_BPS {
        assert_eq!(plan.kept_ballots(), 0);
    }

    // D ≤ #DC ≤ D + B − 1.
    assert!(plan.dc_count() >= delegates);
    assert!(plan.dc_count() < delegates + weights.len());

    // Quotas against exact shares. Every quota is at least the floor of one
    // ballot and less than one ballot above its exact share. When every exact
    // share is at least one ballot the floor never applies, and every quota
    // and the kept remainder are within one ballot of exact. Otherwise only
    // floor funding can push a quota further below its exact share, by at
    // most the number of delegates whose exact share is under one ballot.
    let exact: Vec<u128> = request
        .choices
        .iter()
        .map(|choice| exact_share_scaled(total, choice.percent_bps))
        .collect();
    let scaled_quotas: Vec<u128> = plan
        .quotas()
        .iter()
        .map(|quota| u128::from(quota.ballots) * whole)
        .collect();
    for (quota, exact) in scaled_quotas.iter().zip(&exact) {
        assert!(*quota >= whole, "every delegate gets at least one ballot");
        assert!(*quota < exact + whole, "no quota is a ballot above exact");
    }
    let below_one_ballot = exact.iter().filter(|&&exact| exact < whole).count() as u128;
    if below_one_ballot == 0 {
        for (quota, exact) in scaled_quotas.iter().zip(&exact) {
            assert!(quota.abs_diff(*exact) < whole, "quota within one ballot");
        }
        let kept_exact = exact_share_scaled(total, WHOLE_ALLOCATION_BPS - chosen_bps);
        let kept = u128::from(plan.kept_ballots()) * whole;
        assert!(kept.abs_diff(kept_exact) < whole, "kept within one ballot");
    } else {
        let floor_shortfall: u128 = scaled_quotas
            .iter()
            .zip(&exact)
            .map(|(quota, exact)| exact.saturating_sub(*quota) / whole)
            .sum();
        assert!(floor_shortfall <= below_one_ballot);
    }

    // At most eight delegates, and so at most eight DCs per bundle.
    assert!(plan.quotas().len() <= MAX_PROXY_DELEGATES);

    // No bundle carries two DCs to one delegate.
    let pairs: BTreeSet<(u32, u32)> = plan
        .dcs()
        .iter()
        .map(|dc| (dc.bundle_index, dc.delegate_index))
        .collect();
    assert_eq!(pairs.len(), plan.dc_count());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Every random request either plans with all planner invariants intact or
    /// is refused only for having fewer ballots than delegates.
    #[test]
    fn every_plan_keeps_the_planner_invariants(
        (request, weights, layout) in planning_case()
    ) {
        let total: u64 = weights.iter().sum();
        match plan_allocation(&request, &bundles(&weights), layout) {
            Ok(plan) => {
                prop_assert!(total >= request.choices.len() as u64);
                prop_assert_eq!(plan.layout(), layout);
                check_plan(&request, &weights, &plan);
            }
            Err(error) => prop_assert_eq!(
                error,
                AllocationPlanError::InsufficientWeight {
                    total_ballots: total,
                    delegates: request.choices.len(),
                }
            ),
        }
    }
}
