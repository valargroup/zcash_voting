//! Unproven vote chain reads (contract §8), which only point or display.

use thiserror::Error;

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
