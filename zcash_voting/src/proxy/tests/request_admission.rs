//! Every admission rule refuses with its own reason before anything is
//! planned.

use super::fixtures::{bundles, choice, request};
use crate::proxy::{plan_allocation, DcShareLayout};
use crate::wire::{ProxyDelegateChoice, ProxyDelegationError};

/// Plans `request` over bundles `0, 1, …` holding `weights`, returning the
/// error it is refused with.
fn refusal(request: &[ProxyDelegateChoice], weights: &[u64]) -> ProxyDelegationError {
    plan_allocation(request, &bundles(weights), DcShareLayout::Standard)
        .expect_err("the request must be refused")
}

/// An allocation needs one to eight delegates.
#[test]
fn an_allocation_names_one_to_eight_delegates() {
    assert_eq!(
        refusal(&request(&[]), &[100]),
        ProxyDelegationError::other("an allocation needs at least one delegate")
    );

    let eight: Vec<(u32, u16)> = (1..=8).map(|index| (index, 1_250)).collect();
    plan_allocation(&request(&eight), &bundles(&[100]), DcShareLayout::Standard)
        .expect("eight delegates are allowed");

    let nine: Vec<(u32, u16)> = (1..=9).map(|index| (index, 1_000)).collect();
    assert_eq!(
        refusal(&request(&nine), &[100]),
        ProxyDelegationError::other("an allocation names at most 8 delegates, got 9")
    );
}

/// A choice must name a nonzero, unrepeated delegate index with a nonzero
/// share.
#[test]
fn each_choice_names_a_distinct_real_delegate_with_a_share() {
    assert_eq!(
        refusal(&request(&[(0, 5_000)]), &[100]),
        ProxyDelegationError::other("delegate index 0 is not a registry entry")
    );
    assert_eq!(
        refusal(&request(&[(7, 2_000), (7, 3_000)]), &[100]),
        ProxyDelegationError::other("delegate 7 is chosen more than once")
    );
    assert_eq!(
        refusal(&request(&[(7, 2_000), (8, 0)]), &[100]),
        ProxyDelegationError::other("delegate 8 needs a share of at least 1 basis point")
    );
}

/// A choice carries a handle canonical for its provider and a canonical
/// `zvdk1` key, since no entry could hold anything else.
#[test]
fn a_choice_names_a_canonical_handle_and_key() {
    let good = choice(5, 5_000);
    let malformed = [
        ProxyDelegateChoice {
            handle: "Delegate5".to_owned(),
            ..good.clone()
        },
        ProxyDelegateChoice {
            dk: "qqqs-yqcy-q5rq-wzqf".to_owned(),
            ..good
        },
    ];
    for choice in malformed {
        let request = vec![choice.clone()];
        assert_eq!(
            refusal(&request, &[100]),
            ProxyDelegationError::other("delegate 5's handle or key is malformed"),
            "{choice:?}"
        );
    }
}

/// Shares never exceed the round. The delegator keeps what they leave
/// unnamed, which is nothing when they fill it.
#[test]
fn shares_fit_the_round_and_the_rest_is_kept() {
    assert_eq!(
        refusal(&request(&[(1, 6_000), (2, 4_001)]), &[100]),
        ProxyDelegationError::other("choices total 10001 basis points, more than 10000")
    );
    let quotas = |choices: &[(u32, u16)]| {
        let plan =
            plan_allocation(&request(choices), &bundles(&[100]), DcShareLayout::Standard).unwrap();
        let ballots: Vec<u64> = plan.quotas().iter().map(|quota| quota.ballots).collect();
        (ballots, plan.kept_ballots())
    };
    assert_eq!(quotas(&[(1, 6_000), (2, 4_000)]), (vec![60, 40], 0));
    assert_eq!(quotas(&[(1, 6_000), (2, 3_000)]), (vec![60, 30], 10));
}

/// Bundles must exist and each hold at least a ballot.
#[test]
fn bundles_exist_and_hold_weight() {
    let one_choice = request(&[(1, 5_000)]);
    assert_eq!(
        refusal(&one_choice, &[]),
        ProxyDelegationError::other("the round has no bundle to delegate from")
    );
    assert_eq!(
        refusal(&one_choice, &[10, 0]),
        ProxyDelegationError::other("bundle 1 holds no ballots")
    );
}
