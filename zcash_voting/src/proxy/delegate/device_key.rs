//! The device's one delegate key per vote chain (contract §11.2): creating
//! it, the marker it registers with (§11.5), and the device's record of it.
//! The record holds the public key only and mirrors the proven entry.

use rand::{CryptoRng, RngCore};
use rusqlite::{Connection, TransactionBehavior};
use zcash_vote_delegate::{DelegateKey, DelegateMarker, DelegateSigningKey};
use zeroize::Zeroizing;

use super::{
    delegate_store::{self, DeviceDelegateKey},
    DelegateChain, DelegateModeError, ProvenDelegateEntry,
};
use crate::{proxy::in_transaction, storage::VotingDb, wire::DelegateKeyInfo};

/// Draws a delegate key from `rng`, which must be the OS CSPRNG. Nothing is
/// stored. The host keeps the key's `zvdsk1…` export in secure storage, then
/// calls [`record_delegate_key`].
pub fn create_delegate_key<R: RngCore + CryptoRng>(rng: &mut R) -> DelegateSigningKey {
    let mut dk_sk = Zeroizing::new([0u8; 32]);
    rng.fill_bytes(dk_sk.as_mut_slice());
    DelegateSigningKey::from_secret_seed(*dk_sk)
}

/// A delegate key as the client API shows it: `zvdk1…` and its fingerprint.
pub fn delegate_key_info(dk: &DelegateKey) -> DelegateKeyInfo {
    DelegateKeyInfo {
        dk: dk.encode(),
        fingerprint: dk.fingerprint().to_string(),
    }
}

/// The marker a delegate posts to register `dk`, `zcash-vote-delegate v1
/// <dk>`.
pub fn delegate_marker_text(dk: &DelegateKey) -> String {
    DelegateMarker { dk: *dk }.to_text()
}

/// Records `dk` as the device's key on the chain from `now_seconds` and
/// returns the record, keeping the first one on a repeat. Call once the
/// secret is in secure storage.
///
/// # Errors
///
/// [`DelegateModeError::AnotherKeyHeld`] while the device holds another key
/// on the chain. A device holds one, so [`forget_delegate_key`] comes first.
pub fn record_delegate_key(
    db: &VotingDb,
    chain: &DelegateChain,
    dk: &DelegateKey,
    now_seconds: u64,
) -> Result<DeviceDelegateKey, DelegateModeError> {
    let wallet_id = db.wallet_id();
    in_transaction(
        db,
        TransactionBehavior::Immediate,
        "record delegate key",
        |tx| hold_key(tx, &wallet_id, chain, dk, now_seconds),
    )
}

/// [`record_delegate_key`]'s write, inside the caller's transaction.
pub(super) fn hold_key(
    conn: &Connection,
    wallet_id: &str,
    chain: &DelegateChain,
    dk: &DelegateKey,
    now_seconds: u64,
) -> Result<DeviceDelegateKey, DelegateModeError> {
    match delegate_store::load_key(conn, wallet_id, chain)?.map(|row| row.key) {
        Some(held) if held.dk == *dk => Ok(held),
        Some(held) => Err(DelegateModeError::AnotherKeyHeld {
            held: held.dk.encode(),
        }),
        None => {
            delegate_store::insert_key(conn, wallet_id, chain, dk, now_seconds)?;
            Ok(DeviceDelegateKey {
                dk: *dk,
                delegate_index: None,
                created_at: now_seconds,
            })
        }
    }
}

/// The delegate key the device holds on the chain, if any.
pub fn device_delegate_key(
    db: &VotingDb,
    chain: &DelegateChain,
) -> Result<Option<DeviceDelegateKey>, DelegateModeError> {
    let wallet_id = db.wallet_id();
    Ok(delegate_store::load_key(&db.conn(), &wallet_id, chain)?.map(|row| row.key))
}

/// Forgets the device's key on the chain, with any pending register op, so
/// another can be created or imported: after the entry retired, or to
/// abandon a registration. No key held is a no-op. The secret is the host's
/// to delete, and signed ballots stay.
pub fn forget_delegate_key(db: &VotingDb, chain: &DelegateChain) -> Result<(), DelegateModeError> {
    let wallet_id = db.wallet_id();
    delegate_store::delete_key(&db.conn(), &wallet_id, chain)
}

/// Records that `entry`, proven, is the device's key's entry and returns the
/// record. This settles a registration: the index is written once and the
/// pending register op cleared. Call whenever the host proves the entry the
/// lookup by key names.
///
/// # Errors
///
/// [`DelegateModeError::NotDeviceEntry`] when the entry's key is not the
/// device's, or the device recorded its key in another entry.
pub fn record_proven_entry(
    db: &VotingDb,
    chain: &DelegateChain,
    entry: &ProvenDelegateEntry,
) -> Result<DeviceDelegateKey, DelegateModeError> {
    let entry = entry.entry();
    let wallet_id = db.wallet_id();
    in_transaction(
        db,
        TransactionBehavior::Immediate,
        "record proven delegate entry",
        |tx| {
            let held = delegate_store::load_key(tx, &wallet_id, chain)?.map(|row| row.key);
            match held {
                Some(held) if held.dk == entry.dk && held.delegate_index.is_none() => {
                    delegate_store::set_key_index(tx, &wallet_id, chain, entry.index)?;
                    Ok(DeviceDelegateKey {
                        delegate_index: Some(entry.index),
                        ..held
                    })
                }
                Some(held) if held.dk == entry.dk && held.delegate_index == Some(entry.index) => {
                    Ok(held)
                }
                _ => Err(DelegateModeError::NotDeviceEntry {
                    delegate_index: entry.index,
                }),
            }
        },
    )
}
