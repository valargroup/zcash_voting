//! Sequential fill: bundles largest first, quotas largest first, ties to the
//! lower bundle index and the earlier choice. Each quota fills the current
//! bundle and spills into the next.

use super::fixtures::{bundles, choice};
use crate::proxy::{plan_allocation, AllocationRequest, DcShareLayout};

/// The DCs of `percent_bps`, in request order, over `bundle_ballots`, as
/// `(bundle index, choice position, weight)` in fill order. Delegate indices
/// run opposite to request order, so a tie broken by index would show.
fn packed(bundle_ballots: &[u64], percent_bps: &[u16]) -> Vec<(u32, u32, u64)> {
    let request = AllocationRequest {
        choices: (0..)
            .zip(percent_bps)
            .map(|(position, &bps)| choice(100 - position, bps))
            .collect(),
    };
    plan_allocation(&request, &bundles(bundle_ballots), DcShareLayout::Standard)
        .unwrap()
        .dcs()
        .iter()
        .map(|dc| (dc.bundle_index, 100 - dc.delegate_index, dc.weight_ballots))
        .collect()
}

#[test]
fn a_quota_fills_its_bundle_and_spills_into_the_next() {
    assert_eq!(
        packed(&[5_000, 3_000], &[5_000, 3_000]),
        [(0, 0, 4_000), (0, 1, 1_000), (1, 1, 1_400)]
    );
}

#[test]
fn bundles_fill_largest_first_and_equal_ones_by_index() {
    assert_eq!(
        packed(&[3, 9, 4], &[10_000]),
        [(1, 0, 9), (2, 0, 4), (0, 0, 3)]
    );
    assert_eq!(
        packed(&[5, 5, 5], &[4_000, 3_000, 3_000]),
        [(0, 0, 5), (1, 0, 1), (1, 1, 4), (2, 1, 1), (2, 2, 4)]
    );
}

#[test]
fn quotas_fill_largest_first_and_equal_ones_in_request_order() {
    assert_eq!(
        packed(&[7], &[2_500, 2_500, 5_000]),
        [(0, 2, 3), (0, 0, 2), (0, 1, 2)]
    );
}

/// Eight delegates over six bundles, two of them too small to be reached.
#[test]
fn many_quotas_spill_across_the_largest_bundles_only() {
    assert_eq!(
        packed(
            &[60, 7, 300, 41, 2, 90],
            &[1_500, 1_500, 1_200, 1_100, 1_000, 900, 800, 700]
        ),
        [
            (2, 0, 75),
            (2, 1, 75),
            (2, 2, 60),
            (2, 3, 55),
            (2, 4, 35),
            (5, 4, 15),
            (5, 5, 45),
            (5, 6, 30),
            (0, 6, 10),
            (0, 7, 35),
        ]
    );
}
