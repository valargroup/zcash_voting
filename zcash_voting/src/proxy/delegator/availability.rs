//! Whether a round offers proxy delegation to this wallet (contract §11.4).
//! Hosts pass the chain's answers as fetched bytes.
//!
//! This only decides what the wallet offers. The chain rejects a batch for a
//! round that is not a proxy round without spending the VAN, so a lying RPC
//! server can at most show or hide the option.

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::Deserialize;

use crate::{
    config::WalletCapabilities,
    proxy::PROXY_DELEGATION_ENABLED,
    round_planning::vote_phase_holds_proxy_batch,
    share_tracking::lowest_bundle_with_vote_share_plan,
    storage::VotingDb,
    types::validate_vote_round_id_hex,
    vote_work::RoundHostContext,
    wire::{ProxyAvailability, ProxyDelegationError, ProxyDelegationErrorKind},
};

/// The [`WalletCapabilities::proxy_delegation`] version this build
/// implements.
pub const PROXY_DELEGATION_WALLET_VERSION: &str = "v1";

/// The chain's `proxy_delegation_version` (capability field 4) this build
/// implements, tags 0x09 and 0x0B with 8 DC slots per registration.
pub const PROXY_DELEGATION_CHAIN_VERSION: u32 = 1;

/// Capability field 5 fingerprints of this build's circuits, BLAKE2b-256 of
/// each pinned verifying key in lowercase hex (contract §10).
pub const CIRCUIT_FINGERPRINTS: [(&str, &str); 4] = [
    (
        "zkp1",
        "e1773ecd2eb8531cc54c1df5b935a978788ef4eea4949cb45a00edf550ecee81",
    ),
    (
        "zkp2",
        "95d9d97b0818ffe3f355bf0656aed154dbbffd9bfde4158766aac0bc4977595d",
    ),
    (
        "zkp3",
        "e7165da431d5c6f8396d082aba6abfd0213c567071ca4435b38d092242cc4837",
    ),
    (
        "zkp4",
        "4ec7e24b9bbf275807a613d3d3ea94cfae215083b7aa91374b79de52bf7d35aa",
    ),
];

/// What the host's participation reader proved about the account's
/// governance nullifiers in the round (plan §4.4 Gov nullifiers, LIV-8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GovNullifierProof {
    /// Non-membership of every one is proven.
    Unspent,
    /// At least one is proven spent.
    Spent,
    /// No proof could be fetched or verified.
    Unavailable,
}

/// The chain's answers and the host's proofs [`availability`] reads.
#[derive(Clone, Copy, Debug)]
pub struct ProxyAvailabilityInputs<'a> {
    /// What this wallet build advertises.
    pub wallet_capabilities: &'a WalletCapabilities,
    /// The body of `GET /shielded-vote/v1/protocol-capabilities`.
    pub protocol_capabilities: &'a [u8],
    /// The body of `GET /shielded-vote/v1/round/{round_id}`.
    pub vote_round: &'a [u8],
    /// The body of `GET /shielded-vote/v1/proxy-delegation-params` (404 on a
    /// chain without the feature). Read only once the round offers it.
    pub proxy_delegation_params: Option<&'a [u8]>,
    /// The participation reader's governance nullifier verdict.
    pub gov_nullifiers: GovNullifierProof,
}

/// Whether the current wallet may preview and commit a new allocation in
/// `round_id`: the first contract §11.4 result that applies, in the order
/// `Unsupported` (build, chain fields 4 and 5, or round flag), `Paused`,
/// `AlreadyVoted`, `TimingUnavailable`, `VoteEnded`, `Available`.
///
/// `AlreadyVoted` has `used_elsewhere = false` for a cast on the wire,
/// hashless or confirmed, or a saved vote share plan (not an undispatched
/// vote without one, since a commit holds its bundle), and `true` for spent
/// governance nullifiers with no local broadcast evidence. Writes nothing.
/// Fails with [`ProxyDelegationErrorKind::CannotVerify`] when that check
/// needs an unavailable nullifier proof, and otherwise for malformed inputs
/// or an answer about another round.
pub fn availability(
    db: &VotingDb,
    round_id: &str,
    inputs: &ProxyAvailabilityInputs<'_>,
    host: &RoundHostContext,
) -> Result<ProxyAvailability, ProxyDelegationError> {
    evaluate_availability(PROXY_DELEGATION_ENABLED, db, round_id, inputs, host)
}

/// [`availability`] for a build where the feature is `build_enabled`.
pub(crate) fn evaluate_availability(
    build_enabled: bool,
    db: &VotingDb,
    round_id: &str,
    inputs: &ProxyAvailabilityInputs<'_>,
    host: &RoundHostContext,
) -> Result<ProxyAvailability, ProxyDelegationError> {
    validate_vote_round_id_hex(round_id)?;
    let wallet_supports = build_enabled
        && inputs
            .wallet_capabilities
            .proxy_delegation
            .iter()
            .any(|version| version == PROXY_DELEGATION_WALLET_VERSION);
    if !wallet_supports
        || !chain_supports(inputs.protocol_capabilities)?
        || !round_offers(inputs.vote_round, round_id)?
    {
        return Ok(ProxyAvailability::Unsupported);
    }
    let params = inputs.proxy_delegation_params.ok_or_else(|| {
        ProxyDelegationError::other("the chain's proxy delegation params are missing")
    })?;
    if chain_paused(params)? {
        return Ok(ProxyAvailability::Paused);
    }
    if let Some(already_voted) = already_voted(db, round_id, inputs.gov_nullifiers)? {
        return Ok(already_voted);
    }
    let (Some(_), Some(vote_end)) = (host.ceremony_start_seconds, host.vote_end_time_seconds)
    else {
        return Ok(ProxyAvailability::TimingUnavailable);
    };
    if host.now_seconds >= vote_end {
        return Ok(ProxyAvailability::VoteEnded);
    }
    Ok(ProxyAvailability::Available)
}

/// Whether the chain's capabilities offer the proxy delegation version and
/// circuits this build implements.
fn chain_supports(protocol_capabilities: &[u8]) -> Result<bool, ProxyDelegationError> {
    let capabilities: ProtocolCapabilitiesWire = serde_json::from_slice(protocol_capabilities)
        .map_err(|error| {
            ProxyDelegationError::other(format!("the protocol capabilities are not valid: {error}"))
        })?;
    if capabilities.proxy_delegation_version != PROXY_DELEGATION_CHAIN_VERSION {
        return Ok(false);
    }
    Ok(CIRCUIT_FINGERPRINTS.iter().all(|(name, compiled)| {
        let mut published = capabilities
            .circuit_fingerprints
            .iter()
            .filter(|fingerprint| fingerprint.name == *name)
            .peekable();
        published.peek().is_some()
            && published.all(|fingerprint| {
                BASE64_STANDARD
                    .decode(&fingerprint.vk_blake2b)
                    .is_ok_and(|bytes| hex::encode(bytes) == *compiled)
            })
    }))
}

/// Whether `GET /round/{round_id}` says the round has the proxy flag.
fn round_offers(vote_round: &[u8], round_id: &str) -> Result<bool, ProxyDelegationError> {
    let response: VoteRoundResponseWire = serde_json::from_slice(vote_round).map_err(|error| {
        ProxyDelegationError::other(format!("the round response is not valid: {error}"))
    })?;
    let round = response
        .round
        .ok_or_else(|| ProxyDelegationError::other("the round response holds no round"))?;
    let answered = BASE64_STANDARD
        .decode(&round.vote_round_id)
        .map(hex::encode)
        .unwrap_or_default();
    if answered != round_id {
        return Err(ProxyDelegationError::other(format!(
            "the round response is not round {round_id}'s"
        )));
    }
    Ok(round
        .proxy_delegation
        .is_some_and(|proxy_delegation| proxy_delegation.enabled))
}

/// Whether the chain's params pause new DCs.
fn chain_paused(params: &[u8]) -> Result<bool, ProxyDelegationError> {
    let response: ProxyParamsResponseWire = serde_json::from_slice(params).map_err(|error| {
        ProxyDelegationError::other(format!(
            "the proxy delegation params are not valid: {error}"
        ))
    })?;
    Ok(response.params.is_some_and(|params| params.proxy_dc_paused))
}

/// The `AlreadyVoted` result that applies, if any; see [`availability`].
fn already_voted(
    db: &VotingDb,
    round_id: &str,
    gov_nullifiers: GovNullifierProof,
) -> Result<Option<ProxyAvailability>, ProxyDelegationError> {
    let cast = db
        .vote_phases(round_id)?
        .into_iter()
        .any(|(_, _, phase)| vote_phase_holds_proxy_batch(phase));
    if cast || lowest_bundle_with_vote_share_plan(&db.conn(), round_id, &db.wallet_id())?.is_some()
    {
        return Ok(Some(ProxyAvailability::AlreadyVoted {
            used_elsewhere: false,
        }));
    }
    match gov_nullifiers {
        GovNullifierProof::Unspent => Ok(None),
        GovNullifierProof::Unavailable => Err(ProxyDelegationError::new(
            ProxyDelegationErrorKind::CannotVerify,
            "the account's governance nullifiers could not be proven unspent",
        )),
        GovNullifierProof::Spent => Ok(db
            .delegation_broadcast_evidence(round_id, None)?
            .is_none()
            .then_some(ProxyAvailability::AlreadyVoted {
                used_elsewhere: true,
            })),
    }
}

/// `QueryProtocolCapabilitiesResponse`, zero values omitted. Only fields 4
/// and 5 are read.
#[derive(Deserialize)]
struct ProtocolCapabilitiesWire {
    #[serde(default)]
    proxy_delegation_version: u32,
    #[serde(default)]
    circuit_fingerprints: Vec<CircuitFingerprintWire>,
}

/// One `CircuitFingerprint`, its `vk_blake2b` in standard Base64.
#[derive(Deserialize)]
struct CircuitFingerprintWire {
    #[serde(default)]
    name: String,
    #[serde(default)]
    vk_blake2b: String,
}

/// `QueryVoteRoundResponse`; only the round's id and proxy state are read.
#[derive(Deserialize)]
struct VoteRoundResponseWire {
    round: Option<VoteRoundWire>,
}

/// The fields of `VoteRound` availability reads.
#[derive(Deserialize)]
struct VoteRoundWire {
    /// Standard Base64.
    #[serde(default)]
    vote_round_id: String,
    /// Field 31, written only for rounds created with the proxy flag.
    #[serde(default)]
    proxy_delegation: Option<ProxyRoundStateWire>,
}

/// `ProxyDelegationRoundState`; only `enabled` is read.
#[derive(Deserialize)]
struct ProxyRoundStateWire {
    #[serde(default)]
    enabled: bool,
}

/// `QueryProxyDelegationParamsResponse`; only the pause is read.
#[derive(Deserialize)]
struct ProxyParamsResponseWire {
    params: Option<ProxyParamsWire>,
}

/// The fields of `ProxyDelegationParams` availability reads.
#[derive(Deserialize)]
struct ProxyParamsWire {
    #[serde(default)]
    proxy_dc_paused: bool,
}
