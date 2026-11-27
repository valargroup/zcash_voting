//! Every admission rule refuses with its own error before anything is
//! planned.

use super::fixtures::{bundles, choice, request};
use crate::{
    proxy::{
        plan_allocation, AllocationPlanError, AllocationRequest, BundleBallots, DcShareLayout,
        DelegateChoice, MAX_PROXY_DELEGATES,
    },
    wire::ProxyDelegateProvider,
};

/// Plans `request` over bundles `0, 1, …` holding `weights`, returning the
/// error it is refused with.
fn refusal(request: &AllocationRequest, weights: &[u64]) -> AllocationPlanError {
    plan_allocation(request, &bundles(weights), DcShareLayout::Standard)
        .expect_err("the request must be refused")
}

/// An allocation needs one to eight delegates.
#[test]
fn an_allocation_names_one_to_eight_delegates() {
    assert_eq!(
        refusal(&request(&[]), &[100]),
        AllocationPlanError::NoDelegates
    );

    let eight: Vec<(u32, u16)> = (1..=8).map(|index| (index, 1_250)).collect();
    plan_allocation(&request(&eight), &bundles(&[100]), DcShareLayout::Standard)
        .expect("eight delegates are allowed");

    let nine: Vec<(u32, u16)> = (1..=9).map(|index| (index, 1_000)).collect();
    assert_eq!(
        refusal(&request(&nine), &[100]),
        AllocationPlanError::TooManyDelegates {
            count: MAX_PROXY_DELEGATES + 1
        }
    );
}

/// A choice must name a nonzero, unrepeated delegate index with a nonzero
/// share.
#[test]
fn each_choice_names_a_distinct_real_delegate_with_a_share() {
    assert_eq!(
        refusal(&request(&[(0, 5_000)]), &[100]),
        AllocationPlanError::ZeroDelegateIndex
    );
    assert_eq!(
        refusal(&request(&[(7, 2_000), (7, 3_000)]), &[100]),
        AllocationPlanError::DuplicateDelegate { delegate_index: 7 }
    );
    assert_eq!(
        refusal(&request(&[(7, 2_000), (8, 0)]), &[100]),
        AllocationPlanError::ZeroBasisPoints { delegate_index: 8 }
    );
}

/// A choice carries a handle canonical for its provider and a canonical
/// `zvdk1` key, since no entry could hold anything else.
#[test]
fn a_choice_names_a_canonical_handle_and_key() {
    let good = choice(5, 5_000);
    let malformed = [
        DelegateChoice {
            handle: "Delegate5".to_owned(),
            ..good.clone()
        },
        DelegateChoice {
            provider: ProxyDelegateProvider::GitHub,
            handle: "delegate_5".to_owned(),
            ..good.clone()
        },
        DelegateChoice {
            dk: good.dk.to_uppercase(),
            ..good.clone()
        },
        DelegateChoice {
            dk: "qqqs-yqcy-q5rq-wzqf".to_owned(),
            ..good
        },
    ];
    for choice in malformed {
        let request = AllocationRequest {
            choices: vec![choice.clone()],
        };
        assert_eq!(
            refusal(&request, &[100]),
            AllocationPlanError::InvalidDelegateIdentity { delegate_index: 5 },
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
        AllocationPlanError::BasisPointsExceedWhole { total_bps: 10_001 }
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

/// Bundles must exist, appear once, hold at least a ballot and total no more
/// than `u64::MAX`.
#[test]
fn bundles_are_distinct_hold_weight_and_fit_u64() {
    let one_choice = request(&[(1, 5_000)]);
    assert_eq!(refusal(&one_choice, &[]), AllocationPlanError::NoBundles);
    assert_eq!(
        plan_allocation(
            &one_choice,
            &[
                BundleBallots {
                    bundle_index: 3,
                    ballots: 10
                },
                BundleBallots {
                    bundle_index: 3,
                    ballots: 20
                },
            ],
            DcShareLayout::Standard,
        )
        .unwrap_err(),
        AllocationPlanError::DuplicateBundle { bundle_index: 3 }
    );
    assert_eq!(
        refusal(&one_choice, &[10, 0]),
        AllocationPlanError::EmptyBundle { bundle_index: 1 }
    );
    assert_eq!(
        refusal(&one_choice, &[u64::MAX, 1]),
        AllocationPlanError::WeightOverflow
    );
}

/// Bundle order does not change the plan.
#[test]
fn bundles_may_arrive_in_any_order() {
    let request = request(&[(1, 7_000)]);
    let in_order = plan_allocation(&request, &bundles(&[30, 50]), DcShareLayout::Standard).unwrap();
    let mut reversed = bundles(&[30, 50]);
    reversed.reverse();
    let shuffled = plan_allocation(&request, &reversed, DcShareLayout::Standard).unwrap();
    assert_eq!(in_order, shuffled);
}
