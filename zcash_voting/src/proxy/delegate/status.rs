//! The delegate's status and round views (plan §4.5 DG-8). Every view is
//! built from the proven entry, never from the device's records alone.

use zcash_vote_delegate::DelegateStatus;

use super::{
    ballot::ballot_block,
    chain_reads::parse_pool_share_count,
    delegate_store::{self, KeyRow},
    device_key::delegate_key_info,
    ChainDelegateBallot, DelegateChain, DelegateModeError, DelegateRoundTiming,
    ProvenDelegateEntry,
};
use crate::{
    proxy::{delegator::ballot_view, registry::provider_view, ProxyRoundPhase},
    storage::VotingDb,
    wire::{
        DelegateBallotBlock, DelegateEntryStatus, DelegateEntryView, DelegateModeStatus,
        DelegateRound,
    },
};

/// The device's delegate key on the chain and `entry`, its proven entry
/// (`None` before a registration lands). `None` when the device holds no
/// key.
///
/// # Errors
///
/// [`DelegateModeError::NotDeviceEntry`] when `entry`'s key is not the
/// device's.
pub fn delegate_mode_status(
    db: &VotingDb,
    chain: &DelegateChain,
    entry: Option<&ProvenDelegateEntry>,
) -> Result<Option<DelegateModeStatus>, DelegateModeError> {
    let wallet_id = db.wallet_id();
    let held = delegate_store::load_key(&db.conn(), &wallet_id, chain)?;
    let Some(KeyRow {
        key,
        pending_register_op,
    }) = held
    else {
        return Ok(None);
    };
    let entry = entry.map(ProvenDelegateEntry::entry);
    if let Some(entry) = entry.filter(|entry| entry.dk != key.dk) {
        return Err(DelegateModeError::NotDeviceEntry {
            delegate_index: entry.index,
        });
    }
    Ok(Some(DelegateModeStatus {
        key: delegate_key_info(&key.dk),
        entry: entry.map(|entry| DelegateEntryView {
            delegate_index: entry.index,
            provider: provider_view(entry.provider),
            handle: entry.handle.clone(),
            proof_id: entry.proof_id.clone(),
            status: match entry.status {
                DelegateStatus::Active => DelegateEntryStatus::Active,
                DelegateStatus::Suspended => DelegateEntryStatus::Suspended,
                DelegateStatus::Retired => DelegateEntryStatus::Retired,
            },
            status_reason: entry.status_reason,
        }),
        registration_pending: entry.is_none() && pending_register_op.is_some(),
    }))
}

/// One round as the delegate sees it at device time `now_seconds`.
///
/// `chain_ballot` is the delegate's ballot from the whole-round list, and
/// `pool_share_count` the raw store value at the pool's share count key. A
/// ballot the device signed shows as stored until the list carries one.
///
/// # Errors
///
/// [`DelegateModeError::InvalidInput`] for another delegate's ballot or a
/// ballot without an entry, and [`DelegateModeError::ChainRead`] for a
/// share count that is not 8 bytes.
pub fn delegate_round(
    db: &VotingDb,
    chain: &DelegateChain,
    round: &DelegateRoundTiming,
    entry: Option<&ProvenDelegateEntry>,
    chain_ballot: Option<&ChainDelegateBallot>,
    pool_share_count: Option<&[u8]>,
    now_seconds: u64,
) -> Result<DelegateRound, DelegateModeError> {
    let entry = entry.map(ProvenDelegateEntry::entry);
    if let Some(ballot) = chain_ballot {
        if entry.map(|entry| entry.index) != Some(ballot.delegate_index) {
            return Err(DelegateModeError::InvalidInput {
                message: format!(
                    "the listed ballot of delegate {} is not the entry's",
                    ballot.delegate_index
                ),
            });
        }
    }
    let revealed_shares = parse_pool_share_count(pool_share_count)?;
    let local_ballot = match entry {
        Some(entry) => super::stored_ballot(db, chain, &round.round_id, entry.index)?,
        None => None,
    };
    let phase = if round.is_running_at(now_seconds) {
        ProxyRoundPhase::Active
    } else {
        ProxyRoundPhase::Closed
    };
    Ok(DelegateRound {
        round_id: hex::encode(round.round_id),
        vote_end_time: round.vote_end_time,
        revealed_shares,
        ballot: ballot_view(chain_ballot.map(|ballot| ballot.options.as_slice()), phase),
        stored_ballot: local_ballot
            .filter(|_| chain_ballot.is_none())
            .map(|stored| stored.options().to_vec()),
        ballot_block: match entry {
            None => Some(DelegateBallotBlock::NotRegistered),
            Some(entry) => ballot_block(entry.status),
        },
    })
}
