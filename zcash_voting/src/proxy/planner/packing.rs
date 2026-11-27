//! Packing quotas into DCs by sequential fill (plan decision 44).
//!
//! Bundles go largest first and delegates largest quota first. Each quota
//! fills the current bundle and spills into the next, so a plan has `D` to
//! `D + B − 1` DCs. No bundle carries two DCs to one delegate or more than
//! [`MAX_PROXY_DELEGATES`](crate::proxy::MAX_PROXY_DELEGATES). Ties go to the
//! lower bundle index and the earlier choice, matching `seq_fill` in
//! `prototypes/client/compare_packers.py`.

use std::cmp::Reverse;

use super::{BundleBallots, DelegateQuota, PlannedDc};

/// Lays `quotas` (request order) into `bundles` and returns the DCs in fill
/// order. The quotas must fit the bundles and every bundle must hold a
/// ballot, as admission and rounding guarantee.
pub(super) fn sequential_fill(
    bundles: &[BundleBallots],
    quotas: &[DelegateQuota],
) -> Vec<PlannedDc> {
    let mut bundle_order = bundles.to_vec();
    bundle_order.sort_by_key(|bundle| (Reverse(bundle.ballots), bundle.bundle_index));
    let mut delegate_order: Vec<&DelegateQuota> = quotas.iter().collect();
    // A stable sort keeps request order among equal quotas.
    delegate_order.sort_by_key(|quota| Reverse(quota.ballots));

    let mut dcs = Vec::new();
    let mut current = 0;
    let mut room = bundle_order.first().map_or(0, |bundle| bundle.ballots);
    for quota in delegate_order {
        let mut unplaced = quota.ballots;
        while unplaced > 0 {
            let bundle = bundle_order
                .get(current)
                .expect("quotas never exceed the bundles' total weight");
            let weight_ballots = unplaced.min(room);
            dcs.push(PlannedDc {
                bundle_index: bundle.bundle_index,
                delegate_index: quota.delegate_index,
                weight_ballots,
            });
            unplaced -= weight_ballots;
            room -= weight_ballots;
            if room == 0 {
                current += 1;
                room = bundle_order.get(current).map_or(0, |bundle| bundle.ballots);
            }
        }
    }
    dcs
}
