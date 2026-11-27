//! The delegate's one ballot per round (plan §4.2, §4.4, §4.5 DG-9, DG-10).
//!
//! A ballot answers every proposal. It is signed once under the entry's key
//! and stored before its first post, so retries resend the same bytes and
//! the device never submits a second, different ballot.

use rusqlite::TransactionBehavior;
use zcash_vote_delegate::{
    ballot_digest, DelegateBallotFields, DelegateSigningKey, DelegateStatus, ProxyChainError,
    VOTE_CODESPACE,
};

use super::{
    delegate_op::DelegateOpAnswer,
    delegate_store::{self, StoredBallot},
    ChainDelegateBallot, DelegateChain, DelegateModeError, DelegateRoundTiming,
    ProvenDelegateEntry,
};
use crate::{
    storage::VotingDb,
    types::{validate_vote_options, MAX_PROPOSAL_ID},
    wire::{BallotPreflight, BallotSubmitResult, DelegateBallotBlock},
    ProposalRosterEntry,
};

/// A ballot that cannot be built or signed.
#[derive(Debug, thiserror::Error)]
pub enum BallotError {
    /// The round lists no proposal.
    #[error("the round lists no proposal")]
    EmptyRoster,
    /// The round lists more than [`MAX_PROPOSAL_ID`] proposals.
    #[error("the round lists {count} proposals, more than {MAX_PROPOSAL_ID}")]
    TooManyProposals {
        /// The number of proposals.
        count: usize,
    },
    /// The round's proposal ids are not `1..=n` in order.
    #[error("proposal {position} of the round has id {proposal_id}; ids run 1..n in order")]
    RosterOutOfOrder {
        /// The 1-based position in the roster.
        position: usize,
        /// The id found there.
        proposal_id: u32,
    },
    /// A proposal declares an option count outside 2 to 8.
    #[error("proposal {proposal_id} declares {num_options} options")]
    InvalidOptionCount {
        /// The proposal.
        proposal_id: u32,
        num_options: u32,
    },
    /// The ballot does not answer exactly every proposal.
    #[error("the ballot answers {answers} of the round's {proposals} proposals")]
    WrongAnswerCount {
        /// The ballot's answers.
        answers: usize,
        /// The round's proposals.
        proposals: usize,
    },
    /// An answer is not one of its proposal's options.
    #[error("option {option} is not one of proposal {proposal_id}'s {num_options} options")]
    OptionOutOfRange {
        proposal_id: u32,
        option: u32,
        num_options: u32,
    },
    /// The ballot is for another round than the one being signed.
    #[error("the ballot is for round {ballot}, not {round}")]
    RoundMismatch {
        /// The ballot's round, as hex.
        ballot: String,
        /// The signing round, as hex.
        round: String,
    },
    /// The device already signed a different ballot for the round.
    #[error("a different ballot is already signed for this round")]
    DifferentBallotStored {
        /// The stored ballot's options.
        stored: Vec<u32>,
    },
    /// Voting has ended on the device's clock.
    #[error("voting ended at {vote_end_time}")]
    VotingEnded {
        /// The round's `vote_end_time`.
        vote_end_time: u64,
    },
    /// The entry is suspended or retired, so it may not ballot.
    #[error("the round's ballot is blocked: {block:?}")]
    Blocked {
        /// Why.
        block: DelegateBallotBlock,
    },
    /// The signing key is not the entry's key.
    #[error("the ballot must be signed by the entry's key {fingerprint}")]
    NotEntryKey {
        /// Fingerprint of the entry's key.
        fingerprint: String,
    },
    /// The device's records failed.
    #[error(transparent)]
    Records(#[from] DelegateModeError),
}

/// A ballot answering every proposal of one round. `options[i]` is the
/// choice on proposal `i + 1`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompleteBallot {
    vote_round_id: [u8; 32],
    options: Vec<u32>,
}

impl CompleteBallot {
    /// The ballot `options` for round `vote_round_id`, checked against
    /// `proposals` from the authenticated round configuration.
    ///
    /// # Errors
    ///
    /// The first [`BallotError`] rule broken, roster rules first.
    pub fn new(
        vote_round_id: [u8; 32],
        proposals: &[ProposalRosterEntry],
        options: Vec<u32>,
    ) -> Result<Self, BallotError> {
        if proposals.is_empty() {
            return Err(BallotError::EmptyRoster);
        }
        if proposals.len() > MAX_PROPOSAL_ID as usize {
            return Err(BallotError::TooManyProposals {
                count: proposals.len(),
            });
        }
        for (position, proposal) in (1..).zip(proposals) {
            if proposal.proposal_id as usize != position {
                return Err(BallotError::RosterOutOfOrder {
                    position,
                    proposal_id: proposal.proposal_id,
                });
            }
            validate_vote_options(proposal.num_options).map_err(|_| {
                BallotError::InvalidOptionCount {
                    proposal_id: proposal.proposal_id,
                    num_options: proposal.num_options,
                }
            })?;
        }
        if options.len() != proposals.len() {
            return Err(BallotError::WrongAnswerCount {
                answers: options.len(),
                proposals: proposals.len(),
            });
        }
        for (proposal, &option) in proposals.iter().zip(&options) {
            if option >= proposal.num_options {
                return Err(BallotError::OptionOutOfRange {
                    proposal_id: proposal.proposal_id,
                    option,
                    num_options: proposal.num_options,
                });
            }
        }
        Ok(Self {
            vote_round_id,
            options,
        })
    }

    /// The round the ballot is for.
    pub fn vote_round_id(&self) -> &[u8; 32] {
        &self.vote_round_id
    }

    /// `options[i]` is the choice on proposal `i + 1`.
    pub fn options(&self) -> &[u32] {
        &self.options
    }
}

/// Whether a ballot is already stored for entry `delegate_index` in round
/// `vote_round_id`, and whether it matches `options`. `chain_ballot`, the
/// delegate's ballot from the whole-round list, wins over the device's own.
///
/// # Errors
///
/// [`DelegateModeError::InvalidInput`] when `chain_ballot` is another
/// delegate's.
pub fn ballot_preflight(
    db: &VotingDb,
    chain: &DelegateChain,
    vote_round_id: &[u8; 32],
    delegate_index: u32,
    chain_ballot: Option<&ChainDelegateBallot>,
    options: &[u32],
) -> Result<BallotPreflight, DelegateModeError> {
    if let Some(ballot) = chain_ballot {
        if ballot.delegate_index != delegate_index {
            return Err(DelegateModeError::InvalidInput {
                message: format!(
                    "the listed ballot is delegate {}'s, not {delegate_index}'s",
                    ballot.delegate_index
                ),
            });
        }
        return Ok(BallotPreflight::AlreadySubmitted {
            options: ballot.options.clone(),
            identical: ballot.options == options,
        });
    }
    Ok(
        match stored_ballot(db, chain, vote_round_id, delegate_index)? {
            Some(stored) => BallotPreflight::AlreadySubmitted {
                identical: stored.options() == options,
                options: stored.options().to_vec(),
            },
            None => BallotPreflight::NoBallot,
        },
    )
}

/// Signs `ballot` for `entry` under its key and stores the op before the
/// host posts it. An identical stored ballot is returned without a new
/// signature, for the host to resend.
///
/// In the chain's order: a stored ballot settles the call, then voting must
/// be open at `now_seconds`, the entry active, and `signing_key` its key.
///
/// # Errors
///
/// A [`BallotError`]. [`BallotError::DifferentBallotStored`] refuses a
/// second, different ballot for the round.
pub fn sign_ballot(
    db: &VotingDb,
    chain: &DelegateChain,
    round: &DelegateRoundTiming,
    entry: &ProvenDelegateEntry,
    ballot: &CompleteBallot,
    signing_key: &DelegateSigningKey,
    now_seconds: u64,
) -> Result<StoredBallot, BallotError> {
    let round_id = round.round_id;
    if ballot.vote_round_id != round_id {
        return Err(BallotError::RoundMismatch {
            ballot: hex::encode(ballot.vote_round_id),
            round: hex::encode(round_id),
        });
    }
    let entry = entry.entry();
    let wallet_id = db.wallet_id();
    let outcome = delegate_store::in_transaction(
        db,
        TransactionBehavior::Immediate,
        "sign delegate ballot",
        |tx| {
            if let Some(stored) =
                delegate_store::load_ballot(tx, &wallet_id, chain, &round_id, entry.index)?
            {
                return Ok(if stored.options() == ballot.options() {
                    Ok(stored)
                } else {
                    Err(BallotError::DifferentBallotStored {
                        stored: stored.options().to_vec(),
                    })
                });
            }
            if !round.is_running_at(now_seconds) {
                return Ok(Err(BallotError::VotingEnded {
                    vote_end_time: round.vote_end_time,
                }));
            }
            if let Some(block) = ballot_block(entry.status) {
                return Ok(Err(BallotError::Blocked { block }));
            }
            if signing_key.delegate_key() != entry.dk {
                return Ok(Err(BallotError::NotEntryKey {
                    fingerprint: entry.dk.fingerprint().to_string(),
                }));
            }
            let digest = ballot_digest(
                chain.chain_id(),
                &DelegateBallotFields {
                    vote_round_id: round_id,
                    delegate_index: entry.index,
                    options: ballot.options.clone(),
                },
            )
            .expect("a checked chain id and at most 50 options always encode");
            let signed = StoredBallot::new(
                round_id,
                entry.index,
                entry.dk,
                ballot.options.clone(),
                signing_key.sign_digest(&digest),
                now_seconds,
            );
            delegate_store::insert_ballot(tx, &wallet_id, chain, &signed)?;
            Ok(Ok(signed))
        },
    )?;
    outcome
}

/// Why an entry with `status` may not ballot, if it may not.
pub(super) fn ballot_block(status: DelegateStatus) -> Option<DelegateBallotBlock> {
    match status {
        DelegateStatus::Active => None,
        DelegateStatus::Suspended => Some(DelegateBallotBlock::Suspended),
        DelegateStatus::Retired => Some(DelegateBallotBlock::Retired),
    }
}

/// The ballot the device signed for entry `delegate_index` in round
/// `vote_round_id`, if any. A retry posts its [`StoredBallot::op`].
pub fn stored_ballot(
    db: &VotingDb,
    chain: &DelegateChain,
    vote_round_id: &[u8; 32],
    delegate_index: u32,
) -> Result<Option<StoredBallot>, DelegateModeError> {
    let wallet_id = db.wallet_id();
    delegate_store::in_transaction(
        db,
        TransactionBehavior::Deferred,
        "load delegate ballot",
        |tx| delegate_store::load_ballot(tx, &wallet_id, chain, vote_round_id, delegate_index),
    )
}

/// What the node's answer to a ballot op means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BallotAnswer {
    /// The answer settles the submission.
    Settled(BallotSubmitResult),
    /// Code 57: the round already has a ballot from this delegate. Settle it
    /// with [`existing_ballot_result`].
    BallotExists,
}

/// Classifies the node's answer to a ballot op on (codespace, code).
pub fn classify_ballot_answer(answer: &DelegateOpAnswer) -> BallotAnswer {
    match answer {
        DelegateOpAnswer::Accepted { .. } => BallotAnswer::Settled(BallotSubmitResult::Accepted),
        DelegateOpAnswer::Rejected {
            codespace,
            code,
            log,
        } => match ProxyChainError::from_codespace_and_code(codespace, *code) {
            Some(ProxyChainError::DelegateBallotExists) => BallotAnswer::BallotExists,
            _ => BallotAnswer::Settled(BallotSubmitResult::Rejected {
                codespace: codespace.clone(),
                code: *code,
                reason: log.clone(),
            }),
        },
    }
}

/// Settles a code 57 answer to `stored` from the whole-round list: success
/// when the listed ballot matches, a rejection when it differs, and `None`
/// while the list does not show it yet.
pub fn existing_ballot_result(
    stored: &StoredBallot,
    chain_ballot: Option<&ChainDelegateBallot>,
) -> Option<BallotSubmitResult> {
    let listed = chain_ballot.filter(|ballot| ballot.delegate_index == stored.delegate_index())?;
    Some(if listed.options == stored.options() {
        BallotSubmitResult::AlreadyAccepted
    } else {
        BallotSubmitResult::Rejected {
            codespace: VOTE_CODESPACE.to_owned(),
            code: ProxyChainError::DelegateBallotExists.code(),
            reason: "the chain stores a different ballot for this round".to_owned(),
        }
    })
}
