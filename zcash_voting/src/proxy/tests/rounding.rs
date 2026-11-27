//! Hamilton rounding: floors, leftover ballots by largest remainder, and the
//! one-ballot floor with its funding order.

use super::fixtures::{bundles, request};
use crate::proxy::{plan_allocation, AllocationPlanError, DcShareLayout};

/// The quotas, in request order, and kept ballots of `choices` over one
/// bundle of `total_ballots`.
fn apportion(total_ballots: u64, choices: &[(u32, u16)]) -> (Vec<u64>, u64) {
    let plan = plan_allocation(
        &request(choices),
        &bundles(&[total_ballots]),
        DcShareLayout::Standard,
    )
    .unwrap();
    (
        plan.quotas().iter().map(|quota| quota.ballots).collect(),
        plan.kept_ballots(),
    )
}

/// Ballots left after the floors go to the largest remainders.
#[test]
fn leftover_ballots_go_to_the_largest_remainders() {
    // Exact shares 3.333, 3.333 and 3.334: floors of 3, and the one leftover
    // ballot goes to the largest fractional part.
    assert_eq!(
        apportion(10, &[(1, 3_333), (2, 3_333), (3, 3_334)]),
        (vec![3, 3, 4], 0)
    );
}

/// Equal remainders go to the larger share, then to the earlier choice.
#[test]
fn remainder_ties_go_to_the_larger_share_then_the_earlier_choice() {
    // 1.5 and 4.5: equal remainders, so the larger share wins.
    assert_eq!(apportion(6, &[(1, 2_500), (2, 7_500)]), (vec![1, 5], 0));
    // 2.5 and 2.5: equal remainders and shares, so the earlier choice wins.
    assert_eq!(apportion(5, &[(1, 5_000), (2, 5_000)]), (vec![3, 2], 0));
}

/// The kept remainder competes as the last party in rounding.
#[test]
fn the_kept_remainder_is_rounded_as_the_last_party() {
    // 2.5 against a kept 7.5: equal remainders, and the kept share is larger.
    assert_eq!(apportion(10, &[(1, 2_500)]), (vec![2], 8));
    // 2.5 against a kept 2.5: everything ties, and the kept party is last.
    assert_eq!(apportion(5, &[(1, 5_000)]), (vec![3], 2));
}

/// A floor ballot comes from the kept remainder before any delegate.
#[test]
fn the_kept_remainder_funds_the_one_ballot_floor_first() {
    // Exact shares 10, 0.002 and a kept 9.998 rounded to 10: the second
    // delegate's floor ballot comes out of what is kept, not the first
    // delegate's quota.
    assert_eq!(apportion(20, &[(1, 5_000), (2, 1)]), (vec![10, 1], 9));
}

/// With no kept ballot, the delegate with the most ballots funds the floor.
#[test]
fn without_a_kept_ballot_the_largest_delegate_funds_the_floor() {
    // A kept remainder that rounds to zero cannot fund the floor.
    assert_eq!(apportion(3, &[(1, 9_998), (2, 1)]), (vec![2, 1], 0));
    // Seven floor ballots, all from the one large delegate.
    let mut choices = vec![(1, 9_993)];
    choices.extend((2..=8).map(|index| (index, 1)));
    assert_eq!(apportion(8, &choices), (vec![1; 8], 0));
}

/// Of two equally large delegates, the earlier funds the floor.
#[test]
fn equal_largest_delegates_fund_the_floor_from_the_earliest() {
    // 2.9994, 2.9994 and 0.0012 round to 3, 3 and 0; the earlier of the two
    // equal delegates gives up the floor ballot.
    assert_eq!(
        apportion(6, &[(1, 4_999), (2, 4_999), (3, 2)]),
        (vec![2, 3, 1], 0)
    );
}

/// Fewer ballots than delegates cannot give each delegate one.
#[test]
fn fewer_ballots_than_delegates_cannot_meet_the_floor() {
    let refusal = plan_allocation(
        &request(&[(1, 3_000), (2, 3_000), (3, 3_000)]),
        &bundles(&[1, 1]),
        DcShareLayout::Standard,
    )
    .unwrap_err();
    assert_eq!(
        refusal,
        AllocationPlanError::InsufficientWeight {
            total_ballots: 2,
            delegates: 3
        }
    );
}
