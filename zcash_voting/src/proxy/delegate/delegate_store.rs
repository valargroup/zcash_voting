//! Schema 26's delegate-mode rows: `proxy_delegate_keys` (the device's one
//! key per vote chain) and `proxy_delegate_ballots` (one signed ballot per
//! round). Neither holds a secret. Schema triggers make a key's index
//! write-once and a stored ballot immutable. Rows are keyed by vote chain
//! (network and `chain_id`), since a key registers separately on each. Every
//! function runs in the caller's transaction.

use rusqlite::{named_params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use zcash_vote_delegate::DelegateKey;

use super::{
    delegate_op::{ballot_op, SignedDelegateOp},
    DelegateChain, DelegateModeError,
};
use crate::{
    storage::{queries::network_to_storage, VotingDb},
    types::{VotingError, MAX_PROPOSAL_ID},
};

/// The delegate key this account holds on a chain, as `proxy_delegate_keys`
/// records it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceDelegateKey {
    /// The public key.
    pub dk: DelegateKey,
    /// The key's entry, once a proven entry showed it.
    pub delegate_index: Option<u32>,
    /// When the device recorded the key, in unix seconds.
    pub created_at: u64,
}

/// The delegate's signed ballot for one round, stored before the host first
/// posts it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredBallot {
    vote_round_id: [u8; 32],
    delegate_index: u32,
    dk: DelegateKey,
    options: Vec<u32>,
    dk_sig: [u8; 64],
    signed_at: u64,
}

impl StoredBallot {
    /// A ballot signed by `dk` as `dk_sig` over BALLOT.
    pub(super) fn new(
        vote_round_id: [u8; 32],
        delegate_index: u32,
        dk: DelegateKey,
        options: Vec<u32>,
        dk_sig: [u8; 64],
        signed_at: u64,
    ) -> Self {
        Self {
            vote_round_id,
            delegate_index,
            dk,
            options,
            dk_sig,
            signed_at,
        }
    }

    /// The round the ballot is for.
    pub fn vote_round_id(&self) -> &[u8; 32] {
        &self.vote_round_id
    }

    /// The delegate's entry.
    pub fn delegate_index(&self) -> u32 {
        self.delegate_index
    }

    /// The key that signed it: the entry's key.
    pub fn dk(&self) -> DelegateKey {
        self.dk
    }

    /// `options[i]` is the choice on proposal `i + 1`.
    pub fn options(&self) -> &[u32] {
        &self.options
    }

    /// When the device signed it, in unix seconds.
    pub fn signed_at(&self) -> u64 {
        self.signed_at
    }

    /// The ballot op, rebuilt from the stored signature to the same bytes.
    pub fn op(&self) -> SignedDelegateOp {
        ballot_op(
            &self.vote_round_id,
            self.delegate_index,
            &self.dk,
            &self.options,
            &self.dk_sig,
        )
    }
}

/// Runs `body` in one transaction, committing only when it succeeds.
pub(super) fn in_transaction<T>(
    db: &VotingDb,
    behavior: TransactionBehavior,
    context: &str,
    body: impl FnOnce(&Transaction<'_>) -> Result<T, DelegateModeError>,
) -> Result<T, DelegateModeError> {
    let mut conn = db.conn();
    let tx = conn
        .transaction_with_behavior(behavior)
        .map_err(|error| VotingError::from_sqlite(context, &error))?;
    let value = body(&tx)?;
    tx.commit()
        .map_err(|error| VotingError::from_sqlite(context, &error))?;
    Ok(value)
}

/// The `proxy_delegate_keys` row of a chain.
pub(super) struct KeyRow {
    pub(super) key: DeviceDelegateKey,
    /// The signed register op body, until the registration settles.
    pub(super) pending_register_op: Option<Vec<u8>>,
}

/// The key held on `chain`, if any.
pub(super) fn load_key(
    conn: &Connection,
    wallet_id: &str,
    chain: &DelegateChain,
) -> Result<Option<KeyRow>, DelegateModeError> {
    let row = conn
        .query_row(
            "SELECT delegate_key, delegate_index, created_at, pending_register_op
               FROM proxy_delegate_keys
              WHERE wallet_id = :wallet_id AND network = :network
                AND chain_id = :chain_id",
            named_params! {
                ":wallet_id": wallet_id,
                ":network": network_to_storage(chain.network()),
                ":chain_id": chain.chain_id(),
            },
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<Vec<u8>>>(3)?,
                ))
            },
        )
        .optional()
        .map_err(storage_error("load delegate key"))?;
    let Some((dk, delegate_index, created_at, pending_register_op)) = row else {
        return Ok(None);
    };
    let key = DeviceDelegateKey {
        dk: delegate_key_column(&dk)?,
        delegate_index: delegate_index
            .map(|index| index_column(index, "delegate_index"))
            .transpose()?,
        created_at: time_column(created_at, "created_at")?,
    };
    Ok(Some(KeyRow {
        key,
        pending_register_op,
    }))
}

/// Records `dk` as the key held on `chain` since `created_at`. The caller
/// checked the chain has none.
pub(super) fn insert_key(
    conn: &Connection,
    wallet_id: &str,
    chain: &DelegateChain,
    dk: &DelegateKey,
    created_at: u64,
) -> Result<(), DelegateModeError> {
    conn.execute(
        "INSERT INTO proxy_delegate_keys (wallet_id, network, chain_id, delegate_key, created_at)
         VALUES (:wallet_id, :network, :chain_id, :delegate_key, :created_at)",
        named_params! {
            ":wallet_id": wallet_id,
            ":network": network_to_storage(chain.network()),
            ":chain_id": chain.chain_id(),
            ":delegate_key": dk.to_bytes().as_slice(),
            ":created_at": sqlite_time(created_at, "created_at")?,
        },
    )
    .map_err(storage_error("record delegate key"))?;
    Ok(())
}

/// Deletes the record of the key held on `chain`.
pub(super) fn delete_key(
    conn: &Connection,
    wallet_id: &str,
    chain: &DelegateChain,
) -> Result<(), DelegateModeError> {
    conn.execute(
        "DELETE FROM proxy_delegate_keys
          WHERE wallet_id = :wallet_id AND network = :network
            AND chain_id = :chain_id",
        named_params! {
            ":wallet_id": wallet_id,
            ":network": network_to_storage(chain.network()),
            ":chain_id": chain.chain_id(),
        },
    )
    .map_err(storage_error("forget delegate key"))?;
    Ok(())
}

/// Stores `body` as the pending register op of the unregistered key held on
/// `chain`, replacing any earlier one.
pub(super) fn store_pending_register_op(
    conn: &Connection,
    wallet_id: &str,
    chain: &DelegateChain,
    body: &[u8],
) -> Result<(), DelegateModeError> {
    conn.execute(
        "UPDATE proxy_delegate_keys SET pending_register_op = :body
          WHERE wallet_id = :wallet_id AND network = :network
            AND chain_id = :chain_id AND delegate_index IS NULL",
        named_params! {
            ":wallet_id": wallet_id,
            ":network": network_to_storage(chain.network()),
            ":chain_id": chain.chain_id(),
            ":body": body,
        },
    )
    .map_err(storage_error("store pending register op"))?;
    Ok(())
}

/// Writes the held key's entry index once, clearing its pending register op
/// in the same write.
pub(super) fn set_key_index(
    conn: &Connection,
    wallet_id: &str,
    chain: &DelegateChain,
    delegate_index: u32,
) -> Result<(), DelegateModeError> {
    conn.execute(
        "UPDATE proxy_delegate_keys
            SET delegate_index = :delegate_index, pending_register_op = NULL
          WHERE wallet_id = :wallet_id AND network = :network
            AND chain_id = :chain_id AND delegate_index IS NULL",
        named_params! {
            ":wallet_id": wallet_id,
            ":network": network_to_storage(chain.network()),
            ":chain_id": chain.chain_id(),
            ":delegate_index": i64::from(delegate_index),
        },
    )
    .map_err(storage_error("record delegate key index"))?;
    Ok(())
}

/// The stored ballot of entry `delegate_index` for `vote_round_id`, if any.
pub(super) fn load_ballot(
    conn: &Connection,
    wallet_id: &str,
    chain: &DelegateChain,
    vote_round_id: &[u8; 32],
    delegate_index: u32,
) -> Result<Option<StoredBallot>, DelegateModeError> {
    let row = conn
        .query_row(
            "SELECT delegate_key, options, signature, signed_at
               FROM proxy_delegate_ballots
              WHERE wallet_id = :wallet_id AND network = :network
                AND chain_id = :chain_id
                AND round_id = :round_id AND delegate_index = :delegate_index",
            named_params! {
                ":wallet_id": wallet_id,
                ":network": network_to_storage(chain.network()),
                ":chain_id": chain.chain_id(),
                ":round_id": hex::encode(vote_round_id),
                ":delegate_index": i64::from(delegate_index),
            },
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(storage_error("load delegate ballot"))?;
    let Some((dk, options, signature, signed_at)) = row else {
        return Ok(None);
    };
    let dk_sig: [u8; 64] = signature
        .as_slice()
        .try_into()
        .map_err(|_| corrupt("a stored ballot signature is not 64 bytes"))?;
    Ok(Some(StoredBallot::new(
        *vote_round_id,
        delegate_index,
        delegate_key_column(&dk)?,
        options_column(&options)?,
        dk_sig,
        time_column(signed_at, "signed_at")?,
    )))
}

/// Stores `ballot`. The caller checked the round has none for the entry.
pub(super) fn insert_ballot(
    conn: &Connection,
    wallet_id: &str,
    chain: &DelegateChain,
    ballot: &StoredBallot,
) -> Result<(), DelegateModeError> {
    let options: Vec<u8> = ballot
        .options
        .iter()
        .flat_map(|option| option.to_le_bytes())
        .collect();
    conn.execute(
        "INSERT INTO proxy_delegate_ballots
             (wallet_id, network, chain_id, round_id, delegate_index, delegate_key, options,
              signature, signed_at)
         VALUES (:wallet_id, :network, :chain_id, :round_id, :delegate_index, :delegate_key,
                 :options, :signature, :signed_at)",
        named_params! {
            ":wallet_id": wallet_id,
            ":network": network_to_storage(chain.network()),
            ":chain_id": chain.chain_id(),
            ":round_id": hex::encode(ballot.vote_round_id),
            ":delegate_index": i64::from(ballot.delegate_index),
            ":delegate_key": ballot.dk.to_bytes().as_slice(),
            ":options": options,
            ":signature": ballot.dk_sig.as_slice(),
            ":signed_at": sqlite_time(ballot.signed_at, "signed_at")?,
        },
    )
    .map_err(storage_error("store delegate ballot"))?;
    Ok(())
}

/// A stored key column as a delegate key.
fn delegate_key_column(bytes: &[u8]) -> Result<DelegateKey, DelegateModeError> {
    let dk: [u8; 32] = bytes
        .try_into()
        .map_err(|_| corrupt("a stored delegate key is not 32 bytes"))?;
    DelegateKey::from_bytes(dk).map_err(|error| corrupt(&format!("a stored delegate key: {error}")))
}

/// A stored options column, 1 to [`MAX_PROPOSAL_ID`] little-endian `u32`s.
fn options_column(bytes: &[u8]) -> Result<Vec<u32>, DelegateModeError> {
    let (options, rest) = bytes.as_chunks::<4>();
    if !rest.is_empty() || options.is_empty() || options.len() > MAX_PROPOSAL_ID as usize {
        return Err(corrupt(
            "stored ballot options are not 1 to 50 little-endian u32s",
        ));
    }
    Ok(options
        .iter()
        .map(|option| u32::from_le_bytes(*option))
        .collect())
}

/// A stored entry index, which the schema bounds to `1..=u32::MAX`.
fn index_column(value: i64, column: &str) -> Result<u32, DelegateModeError> {
    u32::try_from(value)
        .ok()
        .filter(|index| *index >= 1)
        .ok_or_else(|| corrupt(&format!("stored {column} {value} is not a delegate index")))
}

/// A stored unix time, which the schema keeps non-negative.
fn time_column(value: i64, column: &str) -> Result<u64, DelegateModeError> {
    u64::try_from(value).map_err(|_| corrupt(&format!("stored {column} {value} is negative")))
}

/// A unix time as SQLite stores it.
fn sqlite_time(value: u64, column: &str) -> Result<i64, DelegateModeError> {
    i64::try_from(value).map_err(|_| DelegateModeError::InvalidInput {
        message: format!("{column} {value} does not fit a database integer"),
    })
}

/// A corrupt-row error.
fn corrupt(message: &str) -> DelegateModeError {
    DelegateModeError::CorruptRecord {
        message: message.to_owned(),
    }
}

/// Maps a SQLite failure in `context` to a storage error.
fn storage_error(context: &'static str) -> impl Fn(rusqlite::Error) -> DelegateModeError {
    move |error| DelegateModeError::Storage(VotingError::from_sqlite(context, &error))
}
