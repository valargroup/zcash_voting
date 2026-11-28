//! Batch planning: each slot's nullifier and spend-auth randomizer, written
//! once before the first proof. Storage refuses to change either after.
//!
//! The slot nullifier is `Poseidon(nk, TAG + dc_slot, round, van_comm_rand)`
//! (contract §10), from the circuit's helper, the hotkey's `nk` and the
//! bundle's stored `van_comm_rand`. `alpha_v` fixes the DC's `r_vpk`, so a
//! re-prove republishes the same key and a re-sign signs under it again.

use std::fmt;
use voting_crypto_deps::CircuitValue as _;

use rusqlite::{named_params, Connection};
use zcash_vote_delegate::DcShareLayout;

use crate::{
    backend::{
        orchard::keys::FullViewingKey,
        pasta_curves::{
            group::ff::{Field, PrimeField},
            pallas,
        },
    },
    hotkey::{spending_key_from_hotkey_seed, VOTING_HOTKEY_ACCOUNT_INDEX},
    storage::VotingDb,
    types::{Network, VotingError},
};

/// A committed `(d, w, slot, layout)` with the nullifier and randomizer every
/// proof and signature of the DC reuses. `Debug` never prints the randomizer.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PlannedSlot {
    /// Position in the batch.
    pub(crate) dc_slot: u8,
    pub(crate) delegate_index: u32,
    pub(crate) weight_ballots: u64,
    pub(crate) layout: DcShareLayout,
    /// The slot nullifier the DC's proof publishes.
    pub(crate) dc_slot_nullifier: [u8; 32],
    /// Canonical encoding of the spend-auth randomizer.
    pub(crate) alpha_v: [u8; 32],
}

impl fmt::Debug for PlannedSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlannedSlot")
            .field("dc_slot", &self.dc_slot)
            .field("delegate_index", &self.delegate_index)
            .field("weight_ballots", &self.weight_ballots)
            .field("layout", &self.layout)
            .field("dc_slot_nullifier", &hex::encode(self.dc_slot_nullifier))
            .finish_non_exhaustive()
    }
}

/// Writes the nullifier and randomizer of every slot of `bundle_index` that
/// lacks them, in one immediate transaction, and returns the slots in order.
///
/// Every nullifier is recomputed from the hotkey and stored setup, and a
/// stored one that does not reproduce is refused, not trusted. A randomizer
/// is drawn from the OS RNG only the first time, so planning again changes
/// nothing. Fails with [`VotingError::InvalidInput`] when the bundle has no
/// committed slots or no stored setup.
pub(crate) fn plan_batch_slots(
    db: &VotingDb,
    round_id: &str,
    bundle_index: u32,
    hotkey_seed: &[u8],
    network: Network,
) -> Result<Vec<PlannedSlot>, VotingError> {
    let spending_key =
        spending_key_from_hotkey_seed(hotkey_seed, network, VOTING_HOTKEY_ACCOUNT_INDEX)?;
    let nk =
        voting_crypto_deps::nullifier_key(&FullViewingKey::from(&spending_key)).circuit_value();
    let round = canonical_base("vote round id", &decode_round_id(round_id)?)?;
    let wallet_id = db.wallet_id();
    db.write_transaction("plan proxy batch", |tx| {
        let van_comm_rand = load_van_comm_rand(tx, round_id, &wallet_id, bundle_index)?;
        let rand = canonical_base("van_comm_rand", &van_comm_rand)?;
        let stored = load_slots(tx, round_id, &wallet_id, bundle_index)?;
        if stored.is_empty() {
            return Err(VotingError::InvalidInput {
                message: format!(
                    "bundle {bundle_index} of round {round_id} has no committed proxy slots"
                ),
            });
        }
        stored
            .into_iter()
            .map(|slot| {
                let nullifier = voting_circuits::proxy_delegation::dc_slot_nullifier_hash(
                    nk,
                    round,
                    rand,
                    slot.dc_slot,
                )
                .to_repr();
                let alpha_v = match (slot.dc_slot_nullifier, slot.alpha_v) {
                    (Some(stored_nullifier), Some(alpha_v)) => {
                        if stored_nullifier != nullifier {
                            return Err(VotingError::InvalidInput {
                                message: format!(
                                    "slot {} of bundle {bundle_index} stores a nullifier this \
                                     hotkey and setup do not reproduce",
                                    slot.dc_slot
                                ),
                            });
                        }
                        alpha_v
                    }
                    (None, None) => {
                        let alpha_v =
                            pallas::Scalar::random(&mut voting_crypto_deps::rand::rngs::OsRng)
                                .to_repr();
                        write_slot_plan(
                            tx,
                            round_id,
                            &wallet_id,
                            bundle_index,
                            slot.dc_slot,
                            &nullifier,
                            &alpha_v,
                        )?;
                        alpha_v
                    }
                    _ => {
                        return Err(VotingError::Internal {
                            message: format!(
                                "slot {} of bundle {bundle_index} stores a nullifier without its \
                                 randomizer",
                                slot.dc_slot
                            ),
                        })
                    }
                };
                Ok(PlannedSlot {
                    dc_slot: slot.dc_slot,
                    delegate_index: slot.delegate_index,
                    weight_ballots: slot.weight_ballots,
                    layout: slot.layout,
                    dc_slot_nullifier: nullifier,
                    alpha_v,
                })
            })
            .collect()
    })
}

/// Reads the bundle's planned slots in order, refusing any not planned yet.
pub(crate) fn load_planned_slots(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    bundle_index: u32,
) -> Result<Vec<PlannedSlot>, VotingError> {
    load_slots(conn, round_id, wallet_id, bundle_index)?
        .into_iter()
        .map(|slot| match (slot.dc_slot_nullifier, slot.alpha_v) {
            (Some(dc_slot_nullifier), Some(alpha_v)) => Ok(PlannedSlot {
                dc_slot: slot.dc_slot,
                delegate_index: slot.delegate_index,
                weight_ballots: slot.weight_ballots,
                layout: slot.layout,
                dc_slot_nullifier,
                alpha_v,
            }),
            _ => Err(VotingError::InvalidInput {
                message: format!(
                    "slot {} of bundle {bundle_index} is not planned yet; plan the batch first",
                    slot.dc_slot
                ),
            }),
        })
        .collect()
}

/// One stored slot, before or after batch planning.
struct StoredSlot {
    dc_slot: u8,
    delegate_index: u32,
    weight_ballots: u64,
    layout: DcShareLayout,
    dc_slot_nullifier: Option<[u8; 32]>,
    alpha_v: Option<[u8; 32]>,
}

/// Every stored slot of the bundle, in slot order.
fn load_slots(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    bundle_index: u32,
) -> Result<Vec<StoredSlot>, VotingError> {
    let mut statement = conn
        .prepare(
            "SELECT dc_slot, delegate_index, weight_ballots, layout, dc_slot_nf, alpha_v
               FROM proxy_slots
              WHERE round_id = :round_id AND wallet_id = :wallet_id
                AND bundle_index = :bundle_index
              ORDER BY dc_slot",
        )
        .map_err(|error| VotingError::from_sqlite("prepare proxy slot plan query", &error))?;
    let sqlite = |error| VotingError::from_sqlite("load proxy slot plan", &error);
    let corrupt = |what: &str| VotingError::Internal {
        message: format!("stored proxy slot of bundle {bundle_index} has {what}"),
    };
    let mut rows = statement
        .query(named_params! {
            ":round_id": round_id,
            ":wallet_id": wallet_id,
            ":bundle_index": i64::from(bundle_index),
        })
        .map_err(sqlite)?;
    let mut slots = Vec::new();
    while let Some(row) = rows.next().map_err(sqlite)? {
        let integer = |index| row.get::<_, i64>(index).map_err(sqlite);
        let blob = |index, name| {
            row.get::<_, Option<Vec<u8>>>(index)
                .map_err(sqlite)?
                .map(|bytes| crate::proxy::blob_array(name, bytes, corrupt))
                .transpose()
        };
        slots.push(StoredSlot {
            dc_slot: u8::try_from(integer(0)?).map_err(|_| corrupt("an invalid slot"))?,
            delegate_index: u32::try_from(integer(1)?)
                .map_err(|_| corrupt("an invalid delegate index"))?,
            weight_ballots: u64::try_from(integer(2)?).map_err(|_| corrupt("a negative weight"))?,
            layout: crate::proxy::layout_from_column(integer(3)?)
                .ok_or_else(|| corrupt("an unknown layout"))?,
            dc_slot_nullifier: blob(4, "nullifier")?,
            alpha_v: blob(5, "randomizer")?,
        });
    }
    Ok(slots)
}

/// Writes one unplanned slot's nullifier and randomizer. The caller read the
/// slot unplanned in its write transaction, and a trigger refuses rewriting
/// either.
fn write_slot_plan(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    bundle_index: u32,
    dc_slot: u8,
    dc_slot_nullifier: &[u8; 32],
    alpha_v: &[u8; 32],
) -> Result<(), VotingError> {
    conn.execute(
        "UPDATE proxy_slots SET dc_slot_nf = :nullifier, alpha_v = :alpha_v
          WHERE round_id = :round_id AND wallet_id = :wallet_id
            AND bundle_index = :bundle_index AND dc_slot = :dc_slot",
        named_params! {
            ":nullifier": dc_slot_nullifier.as_slice(),
            ":alpha_v": alpha_v.as_slice(),
            ":round_id": round_id,
            ":wallet_id": wallet_id,
            ":bundle_index": i64::from(bundle_index),
            ":dc_slot": i64::from(dc_slot),
        },
    )
    .map(|_| ())
    .map_err(|error| VotingError::from_sqlite("write proxy slot plan", &error))
}

/// The bundle's stored `van_comm_rand`, refusing a bundle without setup.
fn load_van_comm_rand(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    bundle_index: u32,
) -> Result<[u8; 32], VotingError> {
    let stored: Option<Vec<u8>> = conn
        .query_row(
            "SELECT van_comm_rand FROM bundles
              WHERE round_id = :round_id AND wallet_id = :wallet_id
                AND bundle_index = :bundle_index",
            named_params! {
                ":round_id": round_id,
                ":wallet_id": wallet_id,
                ":bundle_index": i64::from(bundle_index),
            },
            |row| row.get(0),
        )
        .map_err(|error| VotingError::from_sqlite("load bundle van_comm_rand", &error))?;
    stored
        .ok_or_else(|| VotingError::InvalidInput {
            message: format!("bundle {bundle_index} has no stored delegation setup"),
        })?
        .try_into()
        .map_err(|_| VotingError::Internal {
            message: format!("bundle {bundle_index} stores a van_comm_rand that is not 32 bytes"),
        })
}

/// The 32 bytes of a canonical lowercase-hex round id.
pub(crate) fn decode_round_id(round_id: &str) -> Result<[u8; 32], VotingError> {
    hex::decode(round_id)
        .ok()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .ok_or_else(|| VotingError::InvalidInput {
            message: format!("vote round id {round_id:?} is not 32 hex-encoded bytes"),
        })
}

/// Parses a canonical Pallas base-field encoding.
pub(crate) fn canonical_base(name: &str, bytes: &[u8; 32]) -> Result<pallas::Base, VotingError> {
    Option::from(pallas::Base::from_repr(*bytes)).ok_or_else(|| VotingError::InvalidInput {
        message: format!("{name} is not a canonical Pallas field element"),
    })
}
