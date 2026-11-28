//! Unproven vote chain reads for delegate mode (contract §8), which only
//! point or display. The delegate's own entry is proven separately, see
//! [`ProvenDelegateEntry`](super::ProvenDelegateEntry). Unknown fields are
//! ignored and every known field is checked.

use std::collections::BTreeSet;

use serde::Deserialize;
use thiserror::Error;
use zcash_vote_delegate::{validate_ed25519_key, DelegateKey};

use super::delegate_op::ErrorJson;
use crate::proxy::{
    ballot_pages::{parse_ballot_page, DelegateBallotPage, DELEGATE_BALLOTS_PAGE_LIMIT},
    registry::base64_array,
};

/// The vote chain's REST prefix.
const REST_PREFIX: &str = "/shielded-vote/v1";

/// The chain's registry params: the verifier set and threshold.
pub const PROXY_PARAMS_PATH: &str = "/shielded-vote/v1/proxy-delegation-params";

/// An answer from the vote chain that cannot be used.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ChainReadError {
    /// The chain answered with an unexpected HTTP status.
    #[error("the vote chain answered HTTP {status}")]
    Unavailable {
        /// The HTTP status.
        status: u16,
    },
    /// The chain has no such round (HTTP 404 on the ballot list).
    #[error("the vote chain has no round {round_id}")]
    RoundNotFound {
        /// The round, as hex.
        round_id: String,
    },
    /// The answer is not in the shape the contract gives it.
    #[error("malformed vote chain answer: {message}")]
    Malformed {
        /// The first problem found.
        message: String,
    },
}

/// `GET /delegates/by-key/{hex}`, naming the entry `dk` registered. The
/// host proves that entry before trusting it.
pub fn delegate_by_key_path(dk: &DelegateKey) -> String {
    format!(
        "{REST_PREFIX}/delegates/by-key/{}",
        hex::encode(dk.to_bytes())
    )
}

/// Reads the lookup by key for `dk`: the entry's index, or `None` when the
/// key was never registered.
///
/// `None` needs a 404 whose JSON `error` names `dk` as never registered.
/// Any other 404 (dormant node, missing route) leaves the question open, so
/// a host never forgets a key on its word.
///
/// # Errors
///
/// [`ChainReadError::Unavailable`] for any other status or 404, and
/// [`ChainReadError::Malformed`] for a 200 that names no entry or entry 0.
pub fn parse_delegate_by_key_response(
    dk: &DelegateKey,
    status: u16,
    body: &[u8],
) -> Result<Option<u32>, ChainReadError> {
    match status {
        200 => {}
        404 if names_unregistered_key(dk, body) => return Ok(None),
        status => return Err(ChainReadError::Unavailable { status }),
    }
    let answer: ByKeyAnswer = serde_json::from_slice(body)
        .map_err(|error| malformed(format!("the lookup by key does not parse: {error}")))?;
    if answer.delegate.index == 0 {
        return Err(malformed("the lookup by key names entry 0".to_owned()));
    }
    Ok(Some(answer.delegate.index))
}

/// Whether `body` is vote-sdk's not-found answer for `dk`.
fn names_unregistered_key(dk: &DelegateKey, body: &[u8]) -> bool {
    let not_found = format!(
        "delegate key {} was never registered",
        hex::encode(dk.to_bytes())
    );
    serde_json::from_slice::<ErrorJson>(body).is_ok_and(|answer| answer.error.contains(&not_found))
}

/// One page of the whole-round ballot list,
/// `GET /delegate-ballots/{round_id}`, for indices above `start_after`.
pub fn delegate_ballots_path(vote_round_id: &[u8; 32], start_after: u32) -> String {
    format!(
        "{REST_PREFIX}/delegate-ballots/{}?start_after={start_after}&limit={DELEGATE_BALLOTS_PAGE_LIMIT}",
        hex::encode(vote_round_id)
    )
}

/// Reads one page of `vote_round_id`'s ballot list, fetched with
/// `start_after`, on the page rules the delegator's
/// [`DelegateBallotList`](crate::proxy::DelegateBallotList) shares.
///
/// # Errors
///
/// [`ChainReadError::RoundNotFound`] for HTTP 404, and [`ChainReadError`]
/// for another status or a malformed page.
pub fn parse_delegate_ballots_page(
    vote_round_id: &[u8; 32],
    start_after: u32,
    status: u16,
    body: &[u8],
) -> Result<DelegateBallotPage, ChainReadError> {
    match status {
        200 => {}
        404 => {
            return Err(ChainReadError::RoundNotFound {
                round_id: hex::encode(vote_round_id),
            })
        }
        status => return Err(ChainReadError::Unavailable { status }),
    }
    parse_ballot_page(vote_round_id, start_after, body).map_err(malformed)
}

/// One verifier in the chain's params set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProxyVerifier {
    /// The id attestations name.
    pub verifier_id: u32,
    /// Its Ed25519 key.
    pub pubkey: [u8; 32],
}

/// The chain's verifier set and threshold (`ProxyDelegationParams`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyVerifierSet {
    /// The verifiers, with unique ids. Empty while registration is closed.
    pub verifiers: Vec<ProxyVerifier>,
    /// How many distinct verifiers must attest a registration.
    pub threshold: u32,
}

/// Reads `GET` [`PROXY_PARAMS_PATH`]'s verifier set. Ids must be unique,
/// keys valid, and the threshold 1 to the set's size (0 for an empty set).
///
/// # Errors
///
/// [`ChainReadError`] for another status than 200 or a malformed answer.
pub fn parse_proxy_params_response(
    status: u16,
    body: &[u8],
) -> Result<ProxyVerifierSet, ChainReadError> {
    if status != 200 {
        return Err(ChainReadError::Unavailable { status });
    }
    let answer: ParamsAnswer = serde_json::from_slice(body)
        .map_err(|error| malformed(format!("the registry params do not parse: {error}")))?;
    let mut ids = BTreeSet::new();
    let mut verifiers = Vec::with_capacity(answer.params.verifiers.len());
    for verifier in answer.params.verifiers {
        let pubkey = base64_array::<32>(&verifier.pubkey)
            .filter(|pubkey| validate_ed25519_key(pubkey).is_ok())
            .ok_or_else(|| {
                malformed(format!(
                    "verifier {}'s key is invalid",
                    verifier.verifier_id
                ))
            })?;
        if !ids.insert(verifier.verifier_id) {
            return Err(malformed(format!(
                "verifier {} is listed twice",
                verifier.verifier_id
            )));
        }
        verifiers.push(ProxyVerifier {
            verifier_id: verifier.verifier_id,
            pubkey,
        });
    }
    let threshold = answer.params.verifier_threshold;
    let threshold_fits = if verifiers.is_empty() {
        threshold == 0
    } else {
        threshold >= 1 && threshold as usize <= verifiers.len()
    };
    if !threshold_fits {
        return Err(malformed(format!(
            "threshold {threshold} does not fit {} verifiers",
            verifiers.len()
        )));
    }
    Ok(ProxyVerifierSet {
        verifiers,
        threshold,
    })
}

/// Reads a pool's share count from the raw store value at
/// [`pool_share_count_store_key`](zcash_vote_delegate::pool_share_count_store_key).
/// An absent key is 0, otherwise a big-endian `u64`.
///
/// # Errors
///
/// [`ChainReadError::Malformed`] when the value is not 8 bytes.
pub fn parse_pool_share_count(value: Option<&[u8]>) -> Result<u64, ChainReadError> {
    let Some(value) = value else {
        return Ok(0);
    };
    let bytes: [u8; 8] = value.try_into().map_err(|_| {
        malformed(format!(
            "a share count is 8 bytes, the store holds {}",
            value.len()
        ))
    })?;
    Ok(u64::from_be_bytes(bytes))
}

/// A malformed-answer error.
fn malformed(message: String) -> ChainReadError {
    ChainReadError::Malformed { message }
}

/// The lookup by key's answer, `QueryProxyDelegateByKeyResponse`.
#[derive(Deserialize)]
struct ByKeyAnswer {
    delegate: ByKeyEntry,
}

/// The listed entry. Only its index is read, since the host proves the rest.
#[derive(Deserialize)]
struct ByKeyEntry {
    #[serde(default)]
    index: u32,
}

/// `QueryProxyDelegationParamsResponse`.
#[derive(Deserialize)]
struct ParamsAnswer {
    #[serde(default)]
    params: ParamsBody,
}

/// `ProxyDelegationParams`.
#[derive(Default, Deserialize)]
struct ParamsBody {
    #[serde(default)]
    verifiers: Vec<VerifierAnswer>,
    #[serde(default)]
    verifier_threshold: u32,
}

/// `ProxyVerifier`.
#[derive(Deserialize)]
struct VerifierAnswer {
    #[serde(default)]
    verifier_id: u32,
    pubkey: String,
}
