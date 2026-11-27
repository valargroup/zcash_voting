//! Registering the device's key and retiring its entry (contract §2.2,
//! §5.3).
//!
//! The device stores the register op it prepares so retries resend the same
//! bytes, see [`prepare_register_op`]. A registration settles only from a
//! proven entry, see [`registration_result`]. Retire is one op signed by the
//! entry's key, settled as [`retire_result`] says.

use rusqlite::TransactionBehavior;
use zcash_vote_delegate::{
    register_digest, retire_digest, DelegateRetireFields, DelegateSigningKey, DelegateStatus,
    ProxyChainError,
};

use super::{
    attestation::{checked_attestations, register_fields, AttestationExpectation},
    delegate_op::{register_op, retire_op, stored_register_op, DelegateOpAnswer, SignedDelegateOp},
    delegate_store::{self, KeyRow},
    device_key::{hold_key, record_proven_entry},
    DelegateChain, DelegateModeError, ProvenDelegateEntry,
};
use crate::{
    storage::VotingDb,
    wire::{DelegateAttestation, DelegateRegisterResult, DelegateRetireResult},
};

/// Builds the register op for `attestation`, signed by `signing_key`, the
/// key being registered, and in one transaction records the key as the
/// device's and stores the op as pending. Nothing is signed unless the
/// attestation passes [`check_attestation`](super::check_attestation).
/// Returns the op to post.
///
/// Preparing again with a fresh attestation replaces the pending op. That is
/// safe: both register the same key, which the chain accepts once.
///
/// # Errors
///
/// [`DelegateModeError::Attestation`] when the check fails,
/// [`DelegateModeError::AnotherKeyHeld`] when the device holds another key,
/// and [`DelegateModeError::KeyRegistered`] once the key has its entry.
pub fn prepare_register_op(
    db: &VotingDb,
    chain: &DelegateChain,
    attestation: &DelegateAttestation,
    expected: &AttestationExpectation<'_>,
    signing_key: &DelegateSigningKey,
    now_seconds: u64,
) -> Result<SignedDelegateOp, DelegateModeError> {
    let dk = signing_key.delegate_key();
    let attestations = checked_attestations(chain, attestation, &dk, expected, now_seconds)?;
    let fields = register_fields(attestation, &dk);
    let digest = register_digest(chain.chain_id(), &fields)
        .expect("a checked chain id and canonical fields always encode");
    let op = register_op(
        &fields,
        &dk,
        &attestations,
        &signing_key.sign_digest(&digest),
    );
    let wallet_id = db.wallet_id();
    delegate_store::in_transaction(
        db,
        TransactionBehavior::Immediate,
        "prepare register op",
        |tx| {
            let held = hold_key(tx, &wallet_id, chain, &dk, now_seconds)?;
            if let Some(delegate_index) = held.delegate_index {
                return Err(DelegateModeError::KeyRegistered { delegate_index });
            }
            delegate_store::store_pending_register_op(tx, &wallet_id, chain, op.body())
        },
    )?;
    Ok(op)
}

/// The stored register op of the device's key, byte for byte, until a proven
/// entry settles the registration. `None` when none is pending.
///
/// # Errors
///
/// [`DelegateModeError::CorruptRecord`] when the stored op is not a register
/// op of the device's key.
pub fn pending_register_op(
    db: &VotingDb,
    chain: &DelegateChain,
) -> Result<Option<SignedDelegateOp>, DelegateModeError> {
    let wallet_id = db.wallet_id();
    let held = delegate_store::in_transaction(
        db,
        TransactionBehavior::Deferred,
        "load pending register op",
        |tx| delegate_store::load_key(tx, &wallet_id, chain),
    )?;
    let Some(KeyRow {
        key,
        pending_register_op: Some(body),
    }) = held
    else {
        return Ok(None);
    };
    stored_register_op(&key.dk, body)
        .map(Some)
        .map_err(|message| DelegateModeError::CorruptRecord {
            message: message.to_owned(),
        })
}

/// What the node's answer to a register op means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegisterAnswer {
    /// CheckTx accepted the op. See [`registration_result`].
    Submitted {
        /// The transaction hash, uppercase hex.
        tx_hash: String,
    },
    /// Code 61: the key was registered before, so an earlier post landed.
    /// See [`registration_result`].
    AlreadyRegistered,
    /// Any other rejection. Code 62, the handle already has a live entry, is
    /// permanent until that entry retires.
    Rejected(DelegateRegisterResult),
}

/// Classifies the node's answer to a register op on (codespace, code).
pub fn classify_register_answer(answer: &DelegateOpAnswer) -> RegisterAnswer {
    match answer {
        DelegateOpAnswer::Accepted { tx_hash } => RegisterAnswer::Submitted {
            tx_hash: tx_hash.clone(),
        },
        DelegateOpAnswer::Rejected {
            codespace,
            code,
            log,
        } => match ProxyChainError::from_codespace_and_code(codespace, *code) {
            Some(ProxyChainError::DelegateKeyReused) => RegisterAnswer::AlreadyRegistered,
            _ => RegisterAnswer::Rejected(DelegateRegisterResult::Rejected {
                codespace: codespace.clone(),
                code: *code,
                reason: log.clone(),
            }),
        },
    }
}

/// Settles the device's registration from `entry`, the proven entry the
/// lookup by key named: the entry's key is the device's, so the registration
/// landed. Records the entry as [`record_proven_entry`] does.
///
/// While the lookup by key names no entry the registration has not landed,
/// and the host looks again later.
///
/// # Errors
///
/// [`DelegateModeError::NotDeviceEntry`] when the entry's key is not the
/// device's.
pub fn registration_result(
    db: &VotingDb,
    chain: &DelegateChain,
    entry: &ProvenDelegateEntry,
) -> Result<DelegateRegisterResult, DelegateModeError> {
    record_proven_entry(db, chain, entry)?;
    Ok(DelegateRegisterResult::Registered {
        delegate_index: entry.entry().index,
    })
}

/// Builds Retire: the retire op of `entry`, signed by `signing_key`, its
/// key. The chain refuses a suspended entry's with code 54.
///
/// # Errors
///
/// [`DelegateModeError::NotDeviceEntry`] when `signing_key` is not the
/// entry's key.
pub fn build_retire_op(
    chain: &DelegateChain,
    entry: &ProvenDelegateEntry,
    signing_key: &DelegateSigningKey,
) -> Result<SignedDelegateOp, DelegateModeError> {
    let entry = entry.entry();
    let dk = signing_key.delegate_key();
    if dk != entry.dk {
        return Err(DelegateModeError::NotDeviceEntry {
            delegate_index: entry.index,
        });
    }
    let digest = retire_digest(
        chain.chain_id(),
        &DelegateRetireFields {
            delegate_index: entry.index,
            dk: dk.to_bytes(),
        },
    )
    .expect("a checked chain id and a 32-byte key always encode");
    Ok(retire_op(
        entry.index,
        &dk,
        &signing_key.sign_digest(&digest),
    ))
}

/// What the node's answer to a retire op means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetireAnswer {
    /// CheckTx accepted the op. See [`retire_result`].
    Submitted {
        /// The transaction hash, uppercase hex.
        tx_hash: String,
    },
    /// The answer settles Retire: code 55, already retired, is a success.
    Settled(DelegateRetireResult),
}

/// Classifies the node's answer to a retire op on (codespace, code).
pub fn classify_retire_answer(answer: &DelegateOpAnswer) -> RetireAnswer {
    match answer {
        DelegateOpAnswer::Accepted { tx_hash } => RetireAnswer::Submitted {
            tx_hash: tx_hash.clone(),
        },
        DelegateOpAnswer::Rejected {
            codespace,
            code,
            log,
        } => RetireAnswer::Settled(
            match ProxyChainError::from_codespace_and_code(codespace, *code) {
                Some(ProxyChainError::DelegateRetired) => DelegateRetireResult::Retired,
                _ => DelegateRetireResult::Rejected {
                    codespace: codespace.clone(),
                    code: *code,
                    reason: log.clone(),
                },
            },
        ),
    }
}

/// Settles a submitted Retire from `entry`, proven again after the op:
/// retired once the entry shows it, else `None` and the host proves again
/// later.
pub fn retire_result(entry: &ProvenDelegateEntry) -> Option<DelegateRetireResult> {
    (entry.entry().status == DelegateStatus::Retired).then_some(DelegateRetireResult::Retired)
}
