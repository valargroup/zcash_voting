//! Checking a served ballot against the digest attested in resolved config.

use super::{Ballot, BallotAuthenticationError, BallotDigest};
use crate::config::ResolvedVotingConfig;

/// A ballot whose content matches its round's ballot attestation.
///
/// Only [`ResolvedVotingConfig::authenticate_ballot`] constructs one, so
/// holding it means the content was signed by a trusted config key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthenticatedBallot {
    round_id: String,
    digest: BallotDigest,
    ballot: Ballot,
}

impl AuthenticatedBallot {
    pub fn round_id(&self) -> &str {
        &self.round_id
    }

    pub fn digest(&self) -> BallotDigest {
        self.digest
    }

    pub fn ballot(&self) -> &Ballot {
        &self.ballot
    }

    /// Proposal ids in display order.
    pub fn proposal_ids(&self) -> Vec<u32> {
        self.ballot.proposals.iter().map(|p| p.id).collect()
    }

    pub fn into_ballot(self) -> Ballot {
        self.ballot
    }
}

impl ResolvedVotingConfig {
    /// Authenticate ballot content served for `round_id`.
    ///
    /// Recomputes the served ballot's digest and compares it with the digest
    /// attested in this config. Wallets should display ballot content only
    /// from the returned value and, on error, discard the serving vote server's
    /// response and try another.
    ///
    /// # Errors
    ///
    /// - [`BallotAuthenticationError::RoundNotAuthenticated`] if the round has
    ///   no authenticated config entry;
    /// - [`BallotAuthenticationError::BallotNotAttested`] if the entry carries
    ///   no ballot attestation;
    /// - [`BallotAuthenticationError::DigestMismatch`] if the content differs
    ///   from what was attested.
    pub fn authenticate_ballot(
        &self,
        round_id: &str,
        ballot: Ballot,
    ) -> Result<AuthenticatedBallot, BallotAuthenticationError> {
        let round = self
            .authenticated_rounds
            .iter()
            .find(|round| round.round_id == round_id)
            .ok_or_else(|| BallotAuthenticationError::RoundNotAuthenticated {
                round_id: round_id.to_string(),
            })?;
        let attested = round.ballot_digest.as_deref().ok_or_else(|| {
            BallotAuthenticationError::BallotNotAttested {
                round_id: round_id.to_string(),
            }
        })?;
        let digest = ballot.digest();
        let served = digest.to_hex();
        if served != attested {
            return Err(BallotAuthenticationError::DigestMismatch {
                round_id: round_id.to_string(),
                attested: attested.to_string(),
                served,
            });
        }
        Ok(AuthenticatedBallot {
            round_id: round_id.to_string(),
            digest,
            ballot,
        })
    }
}
