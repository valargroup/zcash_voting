//! The round's whole delegate ballot list (contract §8 `DelegateBallots`),
//! built from pages the host fetches. The status view reads chosen
//! delegates' ballots from it, never from a per-delegate query, so the server
//! does not learn whom the delegator follows (plan §4.4 Status view).

use std::collections::BTreeMap;

use hex::FromHex as _;

use crate::{
    proxy::ballot_pages::parse_ballot_page, types::validate_vote_round_id_hex,
    wire::ProxyDelegationError,
};

/// The ballot list of one round, complete once its last page is added.
/// Paging rules are on `parse_ballot_page`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegateBallotList {
    round_id: [u8; 32],
    /// Each delegate's options, one per proposal in proposal order.
    ballots: BTreeMap<u32, Vec<u32>>,
    /// `None` once the list is complete.
    next_start_after: Option<u32>,
}

impl DelegateBallotList {
    /// An empty list for `round_id`, whose first page is fetched with
    /// `start_after = 0`.
    pub fn new(round_id: &str) -> Result<Self, ProxyDelegationError> {
        validate_vote_round_id_hex(round_id)?;
        let round_id =
            <[u8; 32]>::from_hex(round_id).expect("a valid round id is 64 lowercase hex digits");
        Ok(Self {
            round_id,
            ballots: BTreeMap::new(),
            next_start_after: Some(0),
        })
    }

    /// The `start_after` for the next page, or `None` once complete.
    pub fn next_start_after(&self) -> Option<u32> {
        self.next_start_after
    }

    /// Whether the last page was added.
    pub fn is_complete(&self) -> bool {
        self.next_start_after.is_none()
    }

    /// Checks and adds the page fetched with
    /// [`next_start_after`](Self::next_start_after). On failure the list is
    /// unchanged.
    pub fn add_page(&mut self, page: &[u8]) -> Result<(), ProxyDelegationError> {
        let Some(start_after) = self.next_start_after else {
            return Err(ProxyDelegationError::other(
                "the delegate ballot list is already complete",
            ));
        };
        let page = parse_ballot_page(&self.round_id, start_after, page).map_err(|problem| {
            ProxyDelegationError::other(format!("a delegate ballot page is not valid: {problem}"))
        })?;
        self.ballots.extend(
            page.ballots
                .into_iter()
                .map(|ballot| (ballot.delegate_index, ballot.options)),
        );
        self.next_start_after = page.next_start_after;
        Ok(())
    }

    /// `delegate_index`'s options in proposal order, if added so far.
    pub fn options(&self, delegate_index: u32) -> Option<&[u32]> {
        self.ballots.get(&delegate_index).map(Vec::as_slice)
    }

    /// Whether the list is `round_id`'s.
    pub(super) fn is_for_round(&self, round_id: &str) -> bool {
        hex::encode(self.round_id) == round_id
    }
}
