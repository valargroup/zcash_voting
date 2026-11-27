//! The remaining-weight loader. It reads a bundle's current VAN from durable
//! state in one statement.
//!
//! The current VAN holds the registered ballots,
//! `floor(total_note_value / BALLOT_DIVISOR)`, less the ballots of every proxy
//! slot whose batch was dispatched or confirmed.
//! [`VanState::remaining_ballots`] is the only place that arithmetic lives.

use rusqlite::{named_params, Connection};

use super::{field_from_bytes, pasta_curves::pallas};
use crate::{governance::BALLOT_DIVISOR, types::VotingError};

/// The `chain_submissions` kind of a 0x09 proxy batch (contract §11.3).
pub(crate) const PROXY_BATCH_SUBMISSION_KIND: &str = "proxy_delegation_batch";

/// Initial authority bitmask. Bit 0 is the dead sentinel (proposal_id=0 is
/// rejected by the circuit); bits 1–50 are the usable slots.
const MAX_PROPOSAL_AUTHORITY: u64 = voting_circuits::MAX_PROPOSAL_AUTHORITY;

/// One bundle's current VAN, read from one consistent view of the round.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VanState {
    pub(crate) bundle_index: u32,
    /// The registration VAN's blinding. Every successor VAN keeps it.
    pub(crate) van_comm_rand: Option<Vec<u8>>,
    /// The registration's note total in zatoshi. Proofs spend
    /// [`Self::remaining_ballots`] instead.
    pub(crate) total_note_value: Option<u64>,
    /// The hotkey address index the registration VAN commits to.
    pub(crate) address_index: Option<u32>,
    /// The registration VAN commitment, `gov_comm`.
    pub(crate) registration_van: Option<Vec<u8>>,
    /// The confirmed tree position of the bundle's current VAN.
    pub(crate) van_position: Option<u64>,
    /// The round's election authority key.
    pub(crate) ea_pk: Vec<u8>,
    /// Bit `p` is clear once a vote of this bundle for proposal `p` reached
    /// the chain, by either its hash or its tree position.
    pub(crate) proposal_authority: u64,
    /// A vote, vote batch or proxy batch POST may have spent the
    /// registration VAN (see [`load_van_tree_entries`]).
    pub(crate) registration_van_spent: bool,
    /// Ballots of proxy slots whose batch has a lifecycle row other than a
    /// definite rejection.
    pub(crate) delegated_ballots: u64,
}

impl VanState {
    /// The ballots the registration minted. Fails with
    /// [`VotingError::InvalidInput`] before delegation setup is stored.
    pub(crate) fn registered_ballots(&self) -> Result<u64, VotingError> {
        self.total_note_value
            .map(|total| total / BALLOT_DIVISOR)
            .ok_or_else(|| VotingError::InvalidInput {
                message: format!(
                    "bundle {} has no stored delegation setup, so its VAN weight is unknown",
                    self.bundle_index
                ),
            })
    }

    /// The weight of the bundle's current VAN in ballots. Fails with
    /// [`VotingError::Internal`] when the slots claim more than the
    /// registration holds.
    pub(crate) fn remaining_ballots(&self) -> Result<u64, VotingError> {
        let registered = self.registered_ballots()?;
        registered
            .checked_sub(self.delegated_ballots)
            .ok_or_else(|| VotingError::Internal {
                message: format!(
                    "bundle {}'s dispatched proxy slots take {} ballots from a VAN of {registered}",
                    self.bundle_index, self.delegated_ballots
                ),
            })
    }

    /// Whether some slot counts and no ballot remains. No vote is cast from
    /// such a zero-weight VAN. A bundle without a counted slot is never fully
    /// delegated.
    pub(crate) fn fully_delegated(&self) -> Result<bool, VotingError> {
        Ok(self.delegated_ballots > 0 && self.remaining_ballots()? == 0)
    }
}

/// The VAN state of every bundle of `round_id`, in bundle order, read in one
/// statement so it is consistent on any connection.
pub(crate) fn load_van_states(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
) -> Result<Vec<VanState>, VotingError> {
    query_van_states(conn, round_id, wallet_id, None)
}

/// The VAN state of one bundle. Fails with [`VotingError::InvalidInput`] when
/// the round or bundle is missing.
pub(crate) fn load_van_state(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    bundle_index: u32,
) -> Result<VanState, VotingError> {
    query_van_states(conn, round_id, wallet_id, Some(bundle_index))?
        .pop()
        .ok_or_else(|| VotingError::InvalidInput {
            message: format!("no bundle {bundle_index} with a round row for round={round_id}"),
        })
}

/// Runs the loader's one statement, for the whole round or one bundle.
fn query_van_states(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    bundle_index: Option<u32>,
) -> Result<Vec<VanState>, VotingError> {
    let bundle_filter = match bundle_index {
        Some(_) => "AND b.bundle_index = :bundle_index",
        None => "",
    };
    // `registration_van_spent` follows the rule in `load_van_tree_entries`.
    let sql = format!(
        "SELECT b.bundle_index, b.van_comm_rand, b.total_note_value, b.address_index,
                b.gov_comm, b.van_leaf_position, r.ea_pk,
                (SELECT COALESCE(SUM(1 << v.proposal_id), 0) FROM votes v
                  WHERE v.round_id = b.round_id AND v.wallet_id = b.wallet_id
                    AND v.bundle_index = b.bundle_index
                    AND (v.tx_hash IS NOT NULL OR v.vc_tree_position IS NOT NULL)),
                (EXISTS (
                    SELECT 1 FROM votes v
                     WHERE v.round_id = b.round_id AND v.wallet_id = b.wallet_id
                       AND v.bundle_index = b.bundle_index AND v.tx_hash IS NOT NULL
                 )
                 OR EXISTS (
                    SELECT 1 FROM chain_submissions cs
                     WHERE cs.round_id = b.round_id AND cs.wallet_id = b.wallet_id
                       AND cs.bundle_index = b.bundle_index
                       AND cs.kind IN ('vote', 'vote_batch', 'delegate_and_cast_vote_batch',
                                       '{PROXY_BATCH_SUBMISSION_KIND}')
                       AND cs.state != 'rejected'
                 )),
                (SELECT COALESCE(SUM(slot.weight_ballots), 0) FROM proxy_slots slot
                  WHERE slot.round_id = b.round_id AND slot.wallet_id = b.wallet_id
                    AND slot.bundle_index = b.bundle_index
                    AND EXISTS (
                        SELECT 1 FROM chain_submissions cs
                         WHERE cs.round_id = slot.round_id AND cs.wallet_id = slot.wallet_id
                           AND cs.bundle_index = slot.bundle_index
                           AND cs.kind = '{PROXY_BATCH_SUBMISSION_KIND}'
                           AND cs.state != 'rejected'
                    ))
           FROM bundles b
           JOIN rounds r ON r.round_id = b.round_id AND r.wallet_id = b.wallet_id
          WHERE b.round_id = :round_id AND b.wallet_id = :wallet_id {bundle_filter}
          ORDER BY b.bundle_index"
    );
    let mut statement = conn.prepare(&sql).map_err(|e| VotingError::Internal {
        message: format!("failed to prepare VAN state query: {e}"),
    })?;
    let read = |row: &rusqlite::Row<'_>| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<Vec<u8>>>(1)?,
            row.get::<_, Option<i64>>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, Option<Vec<u8>>>(4)?,
            row.get::<_, Option<i64>>(5)?,
            row.get::<_, Vec<u8>>(6)?,
            row.get::<_, i64>(7)?,
            row.get::<_, i64>(8)? != 0,
            row.get::<_, i64>(9)?,
        ))
    };
    let rows = match bundle_index {
        Some(index) => statement
            .query_map(
                named_params! {
                    ":round_id": round_id,
                    ":wallet_id": wallet_id,
                    ":bundle_index": index,
                },
                read,
            )
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>()),
        None => statement
            .query_map(
                named_params! { ":round_id": round_id, ":wallet_id": wallet_id },
                read,
            )
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>()),
    }
    .map_err(|e| VotingError::from_sqlite("failed to load VAN state", &e))?;

    rows.into_iter()
        .map(
            |(
                bundle_index,
                van_comm_rand,
                total_note_value,
                address_index,
                registration_van,
                van_position,
                ea_pk,
                spent_proposal_bits,
                registration_van_spent,
                delegated_ballots,
            )| {
                let column = |name: &str, value: i64| {
                    u64::try_from(value).map_err(|_| VotingError::Internal {
                        message: format!(
                            "stored {name} for round={round_id}, bundle={bundle_index} must be non-negative, got {value}"
                        ),
                    })
                };
                Ok(VanState {
                    bundle_index: u32::try_from(bundle_index).map_err(|_| {
                        VotingError::Internal {
                            message: format!("stored bundle index {bundle_index} does not fit in u32"),
                        }
                    })?,
                    van_comm_rand,
                    total_note_value: total_note_value
                        .map(|value| column("total_note_value", value))
                        .transpose()?,
                    address_index: address_index
                        .map(|value| {
                            u32::try_from(value).map_err(|_| VotingError::Internal {
                                message: format!(
                                    "stored address_index {value} for bundle {bundle_index} does not fit in u32"
                                ),
                            })
                        })
                        .transpose()?,
                    registration_van,
                    van_position: van_position
                        .map(|value| column("van_leaf_position", value))
                        .transpose()?,
                    ea_pk,
                    proposal_authority: MAX_PROPOSAL_AUTHORITY
                        & !column("proposal bits", spent_proposal_bits)?,
                    registration_van_spent,
                    delegated_ballots: column("delegated slot ballots", delegated_ballots)?,
                })
            },
        )
        .collect()
}

// --- Projections of the loader ---

/// Data from delegation that ZKP #2 needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Zkp2DelegationData {
    pub gov_comm_rand: Vec<u8>,
    /// The registration's note total in zatoshi. A vote proof spends
    /// [`Self::van_ballots`] instead.
    pub total_note_value: u64,
    /// The weight of the bundle's current VAN in ballots
    /// (`VanState::remaining_ballots`).
    pub van_ballots: u64,
    pub address_index: u32,
    pub ea_pk: Vec<u8>,
    pub voting_round_id: String,
    /// Current proposal authority bitmask, decremented per submitted vote.
    /// Bit `i` is set iff the voter has not yet cast a vote for proposal `i`.
    /// Since proposal IDs are 1-indexed (matching on-chain IDs), bit 0 is never
    /// cleared and acts as a structural invariant — it corresponds to the circuit's
    /// sentinel value rejected by the non-zero gate.
    pub proposal_authority: u64,
}

/// Load all fields ZKP #2 needs for one bundle through the remaining-weight
/// loader.
///
/// A vote that reached the chain, by its hash or its tree position, clears its
/// proposal's bit, so the rebuilt VAN matches the VC tree. Testing the hash
/// alone left a tree-confirmed vote's bit set and the chain rejected the
/// nullifier as spent.
///
/// Fails with [`VotingError::InvalidInput`] for a fully delegated bundle
/// (`VanState::fully_delegated`).
pub fn load_zkp2_inputs(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    bundle_index: u32,
) -> Result<Zkp2DelegationData, VotingError> {
    let state = load_van_state(conn, round_id, wallet_id, bundle_index)?;
    if state.fully_delegated()? {
        return Err(VotingError::InvalidInput {
            message: format!(
                "bundle {bundle_index} of round={round_id} delegated its whole weight through proxy delegation; no vote is cast from its zero-weight VAN"
            ),
        });
    }
    let missing = |field: &str| VotingError::InvalidInput {
        message: format!(
            "failed to load ZKP2 inputs for round={round_id}, bundle={bundle_index} (no {field})"
        ),
    };
    Ok(Zkp2DelegationData {
        van_ballots: state.remaining_ballots()?,
        gov_comm_rand: state
            .van_comm_rand
            .ok_or_else(|| missing("van_comm_rand"))?,
        total_note_value: state
            .total_note_value
            .ok_or_else(|| missing("total_note_value"))?,
        address_index: state
            .address_index
            .ok_or_else(|| missing("address_index"))?,
        ea_pk: state.ea_pk,
        voting_round_id: round_id.to_string(),
        proposal_authority: state.proposal_authority,
    })
}

/// One confirmed VAN position that must be retained during vote-tree sync.
pub(crate) struct VanTreeEntry {
    pub bundle_index: u32,
    pub position: u32,
    /// Present only while the delegation VAN is still the bundle's current VAN.
    pub expected_delegation_van: Option<pallas::Base>,
}

/// Loads confirmed VAN positions and expected delegation VANs that remain
/// current, through the remaining-weight loader.
///
/// A bundle whose vote or proxy batch may be on chain no longer holds its
/// delegation VAN at that leaf. The reservation, not `tx_hash`, is the
/// authoritative fact. A POST dispatched but never classified has no hash yet
/// may have spent the VAN, and reading only `tx_hash` deadlocked recovery
/// (the stale expectation failed tree sync, which aborted the cast that would
/// classify the vote).
///
/// A terminally `rejected` row is excluded. It spent nothing and is terminal,
/// so it can never gain an outstanding POST, and since it is never deleted,
/// counting it would skip the leaf checks for the rest of the round.
///
/// A `recovering` row carrying `chain_rejected` is deliberately counted.
/// Exact-tree recovery may reserve and release a retry POST while leaving its
/// `state` and `diagnostic_kind` untouched, so excluding it could restore a
/// VAN the retry already spent and bring back the same deadlock. Telling the
/// two apart needs the retry dispatch journaled against the reservation
/// counter, which the row does not record.
pub(crate) fn load_van_tree_entries(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
) -> Result<Vec<VanTreeEntry>, VotingError> {
    load_van_states(conn, round_id, wallet_id)?
        .into_iter()
        .filter_map(|state| state.van_position.map(|position| (state, position)))
        .map(|(state, position)| {
            let bundle_index = state.bundle_index;
            let position = u32::try_from(position).map_err(|_| VotingError::Internal {
                message: format!(
                    "stored VAN position for bundle {bundle_index} does not fit in u32"
                ),
            })?;
            let expected_delegation_van = if state.registration_van_spent {
                None
            } else {
                let commitment = state
                    .registration_van
                    .ok_or_else(|| VotingError::Internal {
                        message: format!(
                        "confirmed delegation bundle {bundle_index} is missing its VAN commitment"
                    ),
                    })?;
                Some(field_from_bytes(
                    &commitment,
                    &format!("bundle {bundle_index} VAN commitment"),
                )?)
            };
            Ok(VanTreeEntry {
                bundle_index,
                position,
                expected_delegation_van,
            })
        })
        .collect()
}
