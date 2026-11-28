//! Rounding basis points to whole-ballot quotas by Hamilton's largest
//! remainder method, in `u128` so `T · bps` cannot overflow.
//!
//! The parties are the choices in request order, then the kept remainder
//! (the basis points the choices leave unnamed) if it has a share. Each gets `floor(T · bps / 10,000)` and the leftover goes
//! one each to the largest remainders (ties to the larger share, then the
//! earlier party). A delegate left at zero is raised to one, funded by the
//! kept remainder while it has a ballot, else by the delegate with the most
//! (earliest on a tie). Matches `largest_remainder` in
//! `prototypes/client/planner.py`.

use std::cmp::Reverse;

use crate::{
    proxy::WHOLE_ALLOCATION_BPS,
    wire::{ProxyDelegateChoice, ProxyDelegationError, ProxyDelegationErrorKind},
};

/// Whole-ballot quotas for a request.
pub(super) struct Apportionment {
    /// Each choice's quota, in request order, every one at least 1.
    pub(super) quotas: Vec<u64>,
    /// The ballots the delegator keeps. The quotas and this sum to `T`.
    pub(super) kept_ballots: u64,
}

/// Apportions `total_ballots` among an admitted request's choices and kept
/// remainder. Fails with [`ProxyDelegationErrorKind::NotEnoughVotingPower`]
/// when there are fewer ballots than delegates, so the one-ballot floor
/// fails.
pub(super) fn apportion(
    total_ballots: u64,
    choices: &[ProxyDelegateChoice],
) -> Result<Apportionment, ProxyDelegationError> {
    let delegates = choices.len();
    let whole = u128::from(WHOLE_ALLOCATION_BPS);
    let total = u128::from(total_ballots);

    let mut shares: Vec<u128> = choices
        .iter()
        .map(|choice| u128::from(choice.percent_bps))
        .collect();
    let kept_share = whole - shares.iter().sum::<u128>();
    let kept_party = if kept_share > 0 {
        shares.push(kept_share);
        Some(delegates)
    } else {
        None
    };

    // The leftover is below the party count, so only parties with a nonzero
    // remainder gain a ballot.
    let mut seats: Vec<u128> = shares.iter().map(|share| total * share / whole).collect();
    let leftover = total - seats.iter().sum::<u128>();
    let mut by_remainder: Vec<usize> = (0..shares.len()).collect();
    by_remainder.sort_by_key(|&party| {
        (
            Reverse(total * shares[party] % whole),
            Reverse(shares[party]),
            party,
        )
    });
    for &party in by_remainder.iter().take(leftover as usize) {
        seats[party] += 1;
    }

    // The one-ballot floor, delegates in request order.
    for delegate in 0..delegates {
        if seats[delegate] > 0 {
            continue;
        }
        let donor = match kept_party {
            Some(kept) if seats[kept] >= 1 => kept,
            _ => (0..delegates)
                .filter(|&candidate| seats[candidate] > 1)
                .max_by_key(|&candidate| (seats[candidate], Reverse(candidate)))
                .ok_or_else(|| {
                    ProxyDelegationError::new(
                        ProxyDelegationErrorKind::NotEnoughVotingPower,
                        format!(
                            "{delegates} delegates need at least {delegates} ballots, the \
                             bundles hold {total_ballots}"
                        ),
                    )
                })?,
        };
        seats[donor] -= 1;
        seats[delegate] = 1;
    }

    // Every seat is at most `total`, which came from a `u64`.
    let to_ballots = |seat: u128| seat as u64;
    Ok(Apportionment {
        quotas: seats[..delegates].iter().copied().map(to_ballots).collect(),
        kept_ballots: kept_party.map_or(0, |kept| to_ballots(seats[kept])),
    })
}
