//! One page of `GET /shielded-vote/v1/delegate-ballots/{round_id}` (contract
//! §8 `DelegateBallots`), as [`DelegateBallotList`](super::DelegateBallotList)
//! reads it.

use serde::Deserialize;

use super::registry::encoding;
use crate::types::MAX_PROPOSAL_ID;

/// The page size hosts ask for and the most ballots a page may carry. The
/// chain refuses a larger `limit`.
pub const DELEGATE_BALLOTS_PAGE_LIMIT: u32 = 1_000;

/// One stored delegate ballot as the chain lists it (`DelegateBallotRecord`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainDelegateBallot {
    pub delegate_index: u32,
    /// `options[i]` is the choice on proposal `i + 1`.
    pub options: Vec<u32>,
    /// The height the ballot landed at.
    pub height: u64,
}

/// One page of a round's ballot list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegateBallotPage {
    /// The page's ballots, in ascending delegate index.
    pub ballots: Vec<ChainDelegateBallot>,
    /// The `start_after` of the next page, or `None` on the last page.
    pub next_start_after: Option<u32>,
}

/// Parses the page of `vote_round_id`'s list fetched with `start_after`, or
/// says what is wrong with it.
///
/// A page holds at most [`DELEGATE_BALLOTS_PAGE_LIMIT`] ballots for the round,
/// each with 1 to 50 options, indices strictly ascending above `start_after`.
/// `next_start_after` is 0 on the last page and otherwise the last ballot's
/// index, so a server cannot skip a ballot. Zero values may be omitted and
/// unknown fields are ignored.
pub(crate) fn parse_ballot_page(
    vote_round_id: &[u8; 32],
    start_after: u32,
    body: &[u8],
) -> Result<DelegateBallotPage, String> {
    let page: PageWire = serde_json::from_slice(body)
        .map_err(|error| format!("the ballot page does not parse: {error}"))?;
    if page.ballots.len() > DELEGATE_BALLOTS_PAGE_LIMIT as usize {
        return Err(format!(
            "the ballot page holds {} ballots, more than {DELEGATE_BALLOTS_PAGE_LIMIT}",
            page.ballots.len()
        ));
    }
    let mut last_index = start_after;
    let mut ballots = Vec::with_capacity(page.ballots.len());
    for ballot in page.ballots {
        if encoding::base64_array::<32>(&ballot.vote_round_id) != Some(*vote_round_id) {
            return Err(format!(
                "delegate {}'s ballot is for another round",
                ballot.delegate_index
            ));
        }
        if ballot.delegate_index <= last_index {
            return Err(format!(
                "delegate {}'s ballot is out of order after {last_index}",
                ballot.delegate_index
            ));
        }
        if ballot.options.is_empty() || ballot.options.len() > MAX_PROPOSAL_ID as usize {
            return Err(format!(
                "delegate {}'s ballot has {} options",
                ballot.delegate_index,
                ballot.options.len()
            ));
        }
        last_index = ballot.delegate_index;
        ballots.push(ChainDelegateBallot {
            delegate_index: ballot.delegate_index,
            options: ballot.options,
            height: ballot.height,
        });
    }
    let next_start_after = match page.next_start_after {
        0 => None,
        next if next == last_index && !ballots.is_empty() => Some(next),
        next => {
            return Err(format!(
                "the ballot page continues after {next}, not after its last delegate"
            ))
        }
    };
    Ok(DelegateBallotPage {
        ballots,
        next_start_after,
    })
}

/// `QueryDelegateBallotsResponse` as the REST API encodes it.
#[derive(Deserialize)]
struct PageWire {
    #[serde(default)]
    ballots: Vec<BallotWire>,
    #[serde(default)]
    next_start_after: u32,
}

/// One `DelegateBallotRecord`; the round id is standard Base64.
#[derive(Deserialize)]
struct BallotWire {
    #[serde(default)]
    vote_round_id: String,
    #[serde(default)]
    delegate_index: u32,
    #[serde(default)]
    options: Vec<u32>,
    #[serde(default)]
    height: u64,
}
