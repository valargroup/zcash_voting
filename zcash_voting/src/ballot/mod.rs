//! Ballot content and its binding to the round attestation.
//!
//! A round's displayed content — its title, description and discussion link,
//! and each proposal's text and options — is served by vote servers alongside
//! the round. This module gives that content a canonical digest
//! ([`Ballot::digest`]) that config signers attest to with
//! [`RoundAuthPayloadV3`](crate::round_auth::RoundAuthPayloadV3), and checks a
//! served ballot against the attested digest
//! ([`ResolvedVotingConfig::authenticate_ballot`](crate::config::ResolvedVotingConfig::authenticate_ballot)).
//!
//! Phase order for a wallet:
//!
//! 1. resolve the voting config, which records each round's attested
//!    `ballot_digest` when its entry carries a valid ballot attestation;
//! 2. fetch the round from a vote server and map it into a [`Ballot`];
//! 3. call `authenticate_ballot` and display only the returned
//!    [`AuthenticatedBallot`]. On error, discard that server's response.
//!
//! The canonical encoding is specified in `docs/ballot_authentication.md`, with
//! cross-language vectors in `zcash_voting/test-vectors/ballot_digest_v1.json`.

mod authenticate;
mod digest;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use authenticate::AuthenticatedBallot;

/// One selectable answer to a proposal, as stored on chain.
///
/// Every field defaults when absent, matching proto3 zero values, so the
/// vote server's JSON (which omits zero values such as `index: 0`)
/// deserializes directly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BallotOption {
    /// Zero-based choice index submitted with a vote for this option.
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: String,
}

/// One question on the ballot, as stored on chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BallotProposal {
    /// One-based proposal id submitted with each vote.
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// Improvement-proposal reference such as `ZIP-227`, or empty.
    #[serde(default)]
    pub zip_number: String,
    #[serde(default)]
    pub forum_url: String,
    /// Options in display order.
    #[serde(default)]
    pub options: Vec<BallotOption>,
}

/// Everything a wallet displays about a round before the voter chooses.
///
/// The `round` object of the vote server's
/// `GET /shielded-vote/v1/round/{round_id}` response deserializes into this
/// type directly; fields outside the ballot are ignored.
///
/// Order is significant: proposals and options are digested in the order
/// given, which is the order the wallet displays them. Strings are digested as
/// their exact UTF-8 bytes, with no trimming or Unicode normalization.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ballot {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub discussion_url: String,
    #[serde(default)]
    pub proposals: Vec<BallotProposal>,
}

/// SHA-256 digest of a ballot's canonical encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BallotDigest([u8; 32]);

impl BallotDigest {
    /// Wraps raw digest bytes, such as those decoded from an attestation.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lowercase hex, the form carried in dynamic config.
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

/// Why a served ballot was not authenticated.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum BallotAuthenticationError {
    /// The resolved config has no authenticated entry for this round.
    #[error("round {round_id} is not authenticated in resolved voting config")]
    RoundNotAuthenticated { round_id: String },
    /// The round is authenticated, but its config entry carries no ballot
    /// attestation, so no ballot content can be checked against it.
    #[error("round {round_id} has no ballot attestation")]
    BallotNotAttested { round_id: String },
    /// The served ballot does not match the attested digest.
    #[error("ballot for round {round_id} does not match its attestation: attested {attested}, served {served}")]
    DigestMismatch {
        round_id: String,
        attested: String,
        served: String,
    },
}

#[cfg(test)]
mod tests;
