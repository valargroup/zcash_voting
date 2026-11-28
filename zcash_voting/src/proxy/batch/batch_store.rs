//! The `proxy_batches` and `proxy_batch_dcs` rows (schema version 26) and
//! the per-bundle progress round planning reads. Triggers refuse updates, so
//! a proved batch is replaced whole or not at all.

use std::collections::BTreeMap;

use rusqlite::{named_params, Connection, OptionalExtension};

use super::{ProvedProxyBatch, ProvedProxyDc, ProxyBatchStatus};
use crate::{
    session::ProxyBatchStage, storage::queries::PROXY_BATCH_SUBMISSION_KIND, types::VotingError,
};

/// Each bundle of `round_id` with committed slots and its batch status. Reads
/// only on `conn`, so round planning can call it inside its snapshot.
pub(crate) fn proxy_batch_statuses_on(
    conn: &Connection,
    wallet_id: &str,
    round_id: &str,
) -> Result<BTreeMap<u32, ProxyBatchStatus>, VotingError> {
    let mut statement = conn
        .prepare(&format!(
            "SELECT slot.bundle_index,
                    batch.anchor_height,
                    attempt.bundle_index IS NOT NULL,
                    (SELECT cs.state FROM chain_submissions cs
                      WHERE cs.round_id = slot.round_id AND cs.wallet_id = slot.wallet_id
                        AND cs.bundle_index = slot.bundle_index
                        AND cs.kind = '{PROXY_BATCH_SUBMISSION_KIND}'),
                    (SELECT cs.diagnostic_kind FROM chain_submissions cs
                      WHERE cs.round_id = slot.round_id AND cs.wallet_id = slot.wallet_id
                        AND cs.bundle_index = slot.bundle_index
                        AND cs.kind = '{PROXY_BATCH_SUBMISSION_KIND}')
               FROM proxy_slots slot
               LEFT JOIN proxy_batches batch
                 ON batch.round_id = slot.round_id AND batch.wallet_id = slot.wallet_id
                AND batch.bundle_index = slot.bundle_index
               LEFT JOIN proxy_batch_attempts attempt
                 ON attempt.round_id = slot.round_id AND attempt.wallet_id = slot.wallet_id
                AND attempt.bundle_index = slot.bundle_index
              WHERE slot.round_id = :round_id AND slot.wallet_id = :wallet_id
              GROUP BY slot.bundle_index
              ORDER BY slot.bundle_index"
        ))
        .map_err(|error| VotingError::from_sqlite("prepare proxy batch progress", &error))?;
    let sqlite = |error| VotingError::from_sqlite("load proxy batch progress", &error);
    let mut rows = statement
        .query(named_params! { ":round_id": round_id, ":wallet_id": wallet_id })
        .map_err(sqlite)?;
    let mut statuses = BTreeMap::new();
    while let Some(row) = rows.next().map_err(sqlite)? {
        let bundle_index: i64 = row.get(0).map_err(sqlite)?;
        let anchor_height: Option<i64> = row.get(1).map_err(sqlite)?;
        let signed: bool = row.get(2).map_err(sqlite)?;
        let state: Option<String> = row.get(3).map_err(sqlite)?;
        let diagnostic_kind: Option<String> = row.get(4).map_err(sqlite)?;
        let bundle_index = u32::try_from(bundle_index).map_err(|_| VotingError::Internal {
            message: format!("stored proxy slot bundle {bundle_index} does not fit in u32"),
        })?;
        let progress = match (state.as_deref(), anchor_height) {
            (Some("confirmed"), _) => ProxyBatchStage::Landed,
            // The chain's answer to the last POST: a pause, another
            // rejection, or spent nullifiers with no landing found.
            (Some(_), _)
                if matches!(
                    diagnostic_kind.as_deref(),
                    Some("proxy_batch_paused" | "chain_rejected" | "nullifier_already_spent")
                ) =>
            {
                ProxyBatchStage::Refused
            }
            (Some(_), _) => ProxyBatchStage::Dispatched,
            (None, Some(_)) => ProxyBatchStage::Proved,
            (None, None) => ProxyBatchStage::Planned,
        };
        let status = ProxyBatchStatus {
            progress,
            needs_registration_signature: anchor_height == Some(0) && !signed,
        };
        statuses.insert(bundle_index, status);
    }
    Ok(statuses)
}

/// Whether the bundle already has a proved batch.
pub(super) fn has_proved_batch(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    bundle_index: u32,
) -> Result<bool, VotingError> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM proxy_batches
                         WHERE round_id = :round_id AND wallet_id = :wallet_id
                           AND bundle_index = :bundle_index)",
        named_params! {
            ":round_id": round_id,
            ":wallet_id": wallet_id,
            ":bundle_index": i64::from(bundle_index),
        },
        |row| row.get(0),
    )
    .map_err(|error| VotingError::from_sqlite("check for a proved proxy batch", &error))
}

/// Writes `batch` and its DCs inside the caller's write transaction.
pub(crate) fn insert_proved_batch(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    batch: &ProvedProxyBatch,
    proved_at: u64,
) -> Result<(), VotingError> {
    conn.execute(
        "INSERT INTO proxy_batches (round_id, wallet_id, bundle_index, anchor_height, proved_at)
         VALUES (:round_id, :wallet_id, :bundle_index, :anchor_height, :proved_at)",
        named_params! {
            ":round_id": round_id,
            ":wallet_id": wallet_id,
            ":bundle_index": i64::from(batch.bundle_index),
            ":anchor_height": i64::from(batch.anchor_height),
            ":proved_at": i64::try_from(proved_at).unwrap_or(i64::MAX),
        },
    )
    .map_err(|error| VotingError::from_sqlite("insert proved proxy batch", &error))?;
    for dc in &batch.dcs {
        conn.execute(
            "INSERT INTO proxy_batch_dcs
                 (round_id, wallet_id, bundle_index, dc_slot, van_nullifier, r_vpk,
                  vote_authority_note_new, delegation_commitment, proof)
             VALUES (:round_id, :wallet_id, :bundle_index, :dc_slot, :van_nullifier, :r_vpk,
                     :van_new, :dc, :proof)",
            named_params! {
                ":round_id": round_id,
                ":wallet_id": wallet_id,
                ":bundle_index": i64::from(batch.bundle_index),
                ":dc_slot": i64::from(dc.dc_slot),
                ":van_nullifier": dc.van_nullifier.as_slice(),
                ":r_vpk": dc.r_vpk.as_slice(),
                ":van_new": dc.vote_authority_note_new.as_slice(),
                ":dc": dc.delegation_commitment.as_slice(),
                ":proof": dc.proof.as_slice(),
            },
        )
        .map_err(|error| VotingError::from_sqlite("insert proved proxy DC", &error))?;
    }
    Ok(())
}

/// The bundle's proved batch, DCs in slot order, if stored.
pub(crate) fn load_proved_batch(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    bundle_index: u32,
) -> Result<Option<ProvedProxyBatch>, VotingError> {
    let params = named_params! {
        ":round_id": round_id,
        ":wallet_id": wallet_id,
        ":bundle_index": i64::from(bundle_index),
    };
    let anchor_height: Option<i64> = conn
        .query_row(
            "SELECT anchor_height FROM proxy_batches
              WHERE round_id = :round_id AND wallet_id = :wallet_id
                AND bundle_index = :bundle_index",
            params,
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| VotingError::from_sqlite("load proved proxy batch", &error))?;
    let Some(anchor_height) = anchor_height else {
        return Ok(None);
    };
    let mut statement = conn
        .prepare(
            "SELECT slot.dc_slot, slot.delegate_index, slot.weight_ballots, slot.layout,
                    slot.dc_slot_nf, slot.alpha_v, dc.van_nullifier, dc.r_vpk,
                    dc.vote_authority_note_new, dc.delegation_commitment, dc.proof
               FROM proxy_batch_dcs dc
               JOIN proxy_slots slot
                 ON slot.round_id = dc.round_id AND slot.wallet_id = dc.wallet_id
                AND slot.bundle_index = dc.bundle_index AND slot.dc_slot = dc.dc_slot
              WHERE dc.round_id = :round_id AND dc.wallet_id = :wallet_id
                AND dc.bundle_index = :bundle_index
              ORDER BY dc.dc_slot",
        )
        .map_err(|error| VotingError::from_sqlite("prepare proved proxy DC query", &error))?;
    let sqlite = |error| VotingError::from_sqlite("load proved proxy DCs", &error);
    let corrupt = |what: String| VotingError::Internal {
        message: format!("stored proved proxy batch of bundle {bundle_index}: {what}"),
    };
    let bytes32 = |name: &str, bytes: Vec<u8>| {
        <[u8; 32]>::try_from(bytes.as_slice())
            .map_err(|_| corrupt(format!("{name} is not 32 bytes")))
    };
    let mut rows = statement.query(params).map_err(sqlite)?;
    // `proxy_slots_one_layout_per_batch` gives every slot the batch's layout.
    let mut layout = None;
    let mut dcs = Vec::new();
    while let Some(row) = rows.next().map_err(sqlite)? {
        let integer = |index| row.get::<_, i64>(index).map_err(sqlite);
        let blob = |index| row.get::<_, Vec<u8>>(index).map_err(sqlite);
        let (dc_slot, delegate_index, weight_ballots) = (integer(0)?, integer(1)?, integer(2)?);
        let slot_layout = integer(3)?;
        // Batch planning writes both, together, before the first proof.
        let (Some(dc_slot_nullifier), Some(alpha_v)) =
            (row.get(4).map_err(sqlite)?, row.get(5).map_err(sqlite)?)
        else {
            return Err(corrupt("a DC without its nullifier".into()));
        };
        let slot_layout = crate::proxy::layout_from_column(slot_layout)
            .ok_or_else(|| corrupt(format!("unknown layout {slot_layout}")))?;
        layout.get_or_insert(slot_layout);
        dcs.push(ProvedProxyDc {
            dc_slot: u8::try_from(dc_slot).map_err(|_| corrupt(format!("slot {dc_slot}")))?,
            delegate_index: u32::try_from(delegate_index)
                .map_err(|_| corrupt(format!("delegate index {delegate_index}")))?,
            weight_ballots: u64::try_from(weight_ballots)
                .map_err(|_| corrupt(format!("weight {weight_ballots}")))?,
            van_nullifier: bytes32("van_nullifier", blob(6)?)?,
            r_vpk: bytes32("r_vpk", blob(7)?)?,
            vote_authority_note_new: bytes32("vote_authority_note_new", blob(8)?)?,
            delegation_commitment: bytes32("delegation_commitment", blob(9)?)?,
            dc_slot_nullifier: bytes32("dc_slot_nf", dc_slot_nullifier)?,
            alpha_v: bytes32("alpha_v", alpha_v)?,
            proof: blob(10)?,
        });
    }
    Ok(Some(ProvedProxyBatch {
        bundle_index,
        anchor_height: u32::try_from(anchor_height)
            .map_err(|_| corrupt(format!("anchor height {anchor_height}")))?,
        layout: layout.ok_or_else(|| corrupt("a batch without DCs".to_string()))?,
        dcs,
    }))
}

/// The bundle's registration state on durable storage.
pub(super) struct RegistrationEvidence {
    /// A delegation tx hash, VAN position or lifecycle row exists, so the
    /// registration may be on chain.
    pub(super) may_be_on_chain: bool,
    /// A successful ZKP1 proof is stored.
    pub(super) proved: bool,
}

/// Reads the bundle's registration evidence.
pub(super) fn registration_evidence(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    bundle_index: u32,
) -> Result<RegistrationEvidence, VotingError> {
    conn.query_row(
        "SELECT b.delegation_tx_hash IS NOT NULL
                OR b.van_leaf_position IS NOT NULL
                OR EXISTS (SELECT 1 FROM chain_submissions cs
                            WHERE cs.round_id = b.round_id AND cs.wallet_id = b.wallet_id
                              AND cs.bundle_index = b.bundle_index
                              AND cs.kind IN ('delegation', 'delegate_and_cast_vote_batch')),
                EXISTS (SELECT 1 FROM proofs p
                         WHERE p.round_id = b.round_id AND p.wallet_id = b.wallet_id
                           AND p.bundle_index = b.bundle_index AND p.success = 1)
           FROM bundles b
          WHERE b.round_id = :round_id AND b.wallet_id = :wallet_id
            AND b.bundle_index = :bundle_index",
        named_params! {
            ":round_id": round_id,
            ":wallet_id": wallet_id,
            ":bundle_index": i64::from(bundle_index),
        },
        |row| {
            Ok(RegistrationEvidence {
                may_be_on_chain: row.get::<_, i64>(0)? != 0,
                proved: row.get::<_, i64>(1)? != 0,
            })
        },
    )
    .map_err(|error| VotingError::from_sqlite("load proxy batch registration evidence", &error))
}

/// Whether the bundle's batch holds it from casts: it has a slot and no 0x09
/// batch of it is confirmed.
///
/// Act-time form of round planning's `proxy_batch_holds_bundle`. A pending
/// batch spends the VAN at full authority, so a vote's save and first
/// broadcast reservation re-read this in their own transactions and refuse
/// while it holds.
pub(crate) fn batch_holds_casts(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    bundle_index: u32,
) -> Result<bool, VotingError> {
    conn.query_row(
        &format!(
            "SELECT EXISTS (SELECT 1 FROM proxy_slots
                             WHERE round_id = :round_id AND wallet_id = :wallet_id
                               AND bundle_index = :bundle_index)
                AND NOT EXISTS (SELECT 1 FROM chain_submissions
                                 WHERE round_id = :round_id AND wallet_id = :wallet_id
                                   AND bundle_index = :bundle_index
                                   AND kind = '{PROXY_BATCH_SUBMISSION_KIND}'
                                   AND state = 'confirmed')"
        ),
        named_params! {
            ":round_id": round_id,
            ":wallet_id": wallet_id,
            ":bundle_index": i64::from(bundle_index),
        },
        |row| row.get(0),
    )
    .map_err(|error| VotingError::from_sqlite("check the proxy batch's hold on casts", &error))
}

/// Whether the bundle has a 0x09 lifecycle row. There is at most one, since a
/// split-off deletes it before the digest changes.
pub(super) fn has_lifecycle_row(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    bundle_index: u32,
) -> Result<bool, VotingError> {
    let rows: i64 = conn
        .prepare(&format!(
            "SELECT COUNT(*) FROM chain_submissions
              WHERE round_id = :round_id AND wallet_id = :wallet_id
                AND bundle_index = :bundle_index
                AND kind = '{PROXY_BATCH_SUBMISSION_KIND}'"
        ))
        .map_err(|error| VotingError::from_sqlite("prepare proxy batch lifecycle query", &error))?
        .query_row(
            named_params! {
                ":round_id": round_id,
                ":wallet_id": wallet_id,
                ":bundle_index": i64::from(bundle_index),
            },
            |row| row.get(0),
        )
        .map_err(|error| VotingError::from_sqlite("load proxy batch lifecycle state", &error))?;
    if rows > 1 {
        return Err(VotingError::Internal {
            message: format!("bundle {bundle_index} has {rows} proxy batch lifecycle rows"),
        });
    }
    Ok(rows == 1)
}

/// Deletes the bundle's proved batch with its DCs, attempt and landings, so
/// it is proved again from its persisted slots.
pub(crate) fn delete_proved_batch(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    bundle_index: u32,
) -> Result<(), VotingError> {
    conn.execute(
        "DELETE FROM proxy_batches
          WHERE round_id = :round_id AND wallet_id = :wallet_id
            AND bundle_index = :bundle_index",
        named_params! {
            ":round_id": round_id,
            ":wallet_id": wallet_id,
            ":bundle_index": i64::from(bundle_index),
        },
    )
    .map(|_| ())
    .map_err(|error| VotingError::from_sqlite("delete proved proxy batch", &error))
}

/// Records each landed DC's `(dc_slot, tree_position)`. Replaying the same
/// position is accepted and a different one is refused.
pub(crate) fn record_dc_landings(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    bundle_index: u32,
    landings: &[(u8, u64)],
) -> Result<(), VotingError> {
    for &(dc_slot, tree_position) in landings {
        let position = i64::try_from(tree_position).map_err(|_| VotingError::InvalidInput {
            message: format!("DC tree position {tree_position} does not fit storage"),
        })?;
        conn.execute(
            "INSERT INTO proxy_dc_landings
                 (round_id, wallet_id, bundle_index, dc_slot, tree_position)
             VALUES (:round_id, :wallet_id, :bundle_index, :dc_slot, :position)
             ON CONFLICT (round_id, wallet_id, bundle_index, dc_slot) DO NOTHING",
            named_params! {
                ":round_id": round_id,
                ":wallet_id": wallet_id,
                ":bundle_index": i64::from(bundle_index),
                ":dc_slot": i64::from(dc_slot),
                ":position": position,
            },
        )
        .map_err(|error| VotingError::from_sqlite("record proxy DC landing", &error))?;
        let stored: i64 = conn
            .query_row(
                "SELECT tree_position FROM proxy_dc_landings
                  WHERE round_id = :round_id AND wallet_id = :wallet_id
                    AND bundle_index = :bundle_index AND dc_slot = :dc_slot",
                named_params! {
                    ":round_id": round_id,
                    ":wallet_id": wallet_id,
                    ":bundle_index": i64::from(bundle_index),
                    ":dc_slot": i64::from(dc_slot),
                },
                |row| row.get(0),
            )
            .map_err(|error| VotingError::from_sqlite("read proxy DC landing", &error))?;
        if stored != position {
            return Err(VotingError::InvalidInput {
                message: format!(
                    "DC slot {dc_slot} of bundle {bundle_index} landed at {stored}, not {tree_position}"
                ),
            });
        }
    }
    Ok(())
}
