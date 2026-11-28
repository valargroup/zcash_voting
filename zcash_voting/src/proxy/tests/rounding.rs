//! Hamilton rounding: floors, leftover ballots by largest remainder, and the
//! one-ballot floor with its funding order.

use super::fixtures::{bundles, not_enough_voting_power, request};
use crate::proxy::{plan_allocation, DcShareLayout};

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

/// Fewer ballots than delegates cannot give each delegate one.
#[test]
fn fewer_ballots_than_delegates_cannot_meet_the_floor() {
    let refusal = plan_allocation(
        &request(&[(1, 3_000), (2, 3_000), (3, 3_000)]),
        &bundles(&[1, 1]),
        DcShareLayout::Standard,
    )
    .unwrap_err();
    assert_eq!(refusal, not_enough_voting_power(2, 3));
}
