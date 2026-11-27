//! The `proxy_allocations` and `proxy_slots` rows (schema version 26).
//! Triggers lock both once written. A slot nullifier is written at most once,
//! and rows leave only with their round.

use rusqlite::{named_params, Connection, OptionalExtension};
use zcash_vote_delegate::DcShareLayout;

use super::{
    AllocationRequest, CommittedAllocation, DelegateChoice, ProxyAllocationError, ProxySlot,
};
use crate::types::VotingError;

/// Whether the round has a committed allocation.
pub(super) fn is_committed(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
) -> Result<bool, ProxyAllocationError> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM proxy_allocations
                         WHERE round_id = :round_id AND wallet_id = :wallet_id)",
        named_params! { ":round_id": round_id, ":wallet_id": wallet_id },
        |row| row.get(0),
    )
    .map_err(storage_error("load proxy allocation"))
}

/// Writes the allocation row and every slot. The caller holds the write
/// transaction, so both land together or not at all.
pub(super) fn insert_allocation(
    conn: &Connection,
    allocation: &CommittedAllocation,
    wallet_id: &str,
) -> Result<(), ProxyAllocationError> {
    let choices_json = serde_json::to_string(&allocation.request.choices).map_err(|error| {
        ProxyAllocationError::Storage(VotingError::Internal {
            message: format!("failed to encode proxy allocation choices: {error}"),
        })
    })?;
    conn.execute(
        "INSERT INTO proxy_allocations
             (round_id, wallet_id, choices_json, committed_at)
         VALUES (:round_id, :wallet_id, :choices_json, :committed_at)",
        named_params! {
            ":round_id": allocation.round_id,
            ":wallet_id": wallet_id,
            ":choices_json": choices_json,
            ":committed_at": allocation.committed_at as i64,
        },
    )
    .map_err(storage_error("insert proxy allocation"))?;

    for slot in &allocation.slots {
        conn.execute(
            "INSERT INTO proxy_slots
                 (round_id, wallet_id, bundle_index, dc_slot, delegate_index,
                  weight_ballots, layout, dc_slot_nf)
             VALUES (:round_id, :wallet_id, :bundle_index, :dc_slot, :delegate_index,
                     :weight_ballots, :layout, :dc_slot_nf)",
            named_params! {
                ":round_id": allocation.round_id,
                ":wallet_id": wallet_id,
                ":bundle_index": i64::from(slot.bundle_index),
                ":dc_slot": i64::from(slot.dc_slot),
                ":delegate_index": i64::from(slot.delegate_index),
                ":weight_ballots": slot.weight_ballots as i64,
                ":layout": slot.layout.layout_u8(),
                ":dc_slot_nf": slot.dc_slot_nullifier.as_ref().map(|nf| nf.as_slice()),
            },
        )
        .map_err(storage_error("insert proxy slot"))?;
    }
    Ok(())
}

/// Reads the round's committed allocation and slots, checking they are
/// consistent.
pub(crate) fn load_allocation(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
) -> Result<Option<CommittedAllocation>, ProxyAllocationError> {
    let row = conn
        .query_row(
            "SELECT choices_json, committed_at FROM proxy_allocations
              WHERE round_id = :round_id AND wallet_id = :wallet_id",
            named_params! { ":round_id": round_id, ":wallet_id": wallet_id },
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(storage_error("load proxy allocation"))?;
    let Some((choices_json, committed_at)) = row else {
        return Ok(None);
    };
    let request = AllocationRequest {
        choices: decode_choices(&choices_json)?,
    };
    let slots = load_slots(conn, round_id, wallet_id)?;
    let layout = batch_layout_of(&request, &slots)?;
    Ok(Some(CommittedAllocation {
        round_id: round_id.to_owned(),
        request,
        layout,
        slots,
        committed_at: column_u64("proxy_allocations.committed_at", committed_at)?,
    }))
}

/// Every slot of the round's allocation, ordered by bundle and slot.
fn load_slots(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
) -> Result<Vec<ProxySlot>, ProxyAllocationError> {
    let mut statement = conn
        .prepare(
            "SELECT bundle_index, dc_slot, delegate_index, weight_ballots, layout, dc_slot_nf
               FROM proxy_slots
              WHERE round_id = :round_id AND wallet_id = :wallet_id
              ORDER BY bundle_index, dc_slot",
        )
        .map_err(storage_error("prepare proxy slot query"))?;
    let rows = statement
        .query_map(
            named_params! { ":round_id": round_id, ":wallet_id": wallet_id },
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<Vec<u8>>>(5)?,
                ))
            },
        )
        .map_err(storage_error("load proxy slots"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_error("load proxy slots"))?;
    rows.into_iter()
        .map(
            |(bundle_index, dc_slot, delegate_index, weight_ballots, layout, dc_slot_nf)| {
                Ok(ProxySlot {
                    bundle_index: column_u32("proxy_slots.bundle_index", bundle_index)?,
                    dc_slot: u8::try_from(dc_slot)
                        .map_err(|_| corrupt(format!("dc_slot {dc_slot} is out of range")))?,
                    delegate_index: column_u32("proxy_slots.delegate_index", delegate_index)?,
                    weight_ballots: column_u64("proxy_slots.weight_ballots", weight_ballots)?,
                    layout: layout_from_column(layout)?,
                    dc_slot_nullifier: dc_slot_nf
                        .map(|bytes| {
                            <[u8; 32]>::try_from(bytes.as_slice())
                                .map_err(|_| corrupt("dc_slot_nf is not 32 bytes".to_owned()))
                        })
                        .transpose()?,
                })
            },
        )
        .collect()
}

/// The one layout every slot shares, after checking that the slots and the
/// choices describe the same delegates.
fn batch_layout_of(
    request: &AllocationRequest,
    slots: &[ProxySlot],
) -> Result<DcShareLayout, ProxyAllocationError> {
    let first = slots
        .first()
        .ok_or_else(|| corrupt("the allocation has no slots".to_owned()))?;
    if slots.iter().any(|slot| slot.layout != first.layout) {
        return Err(corrupt("slots disagree on the batch layout".to_owned()));
    }
    let chosen = |delegate_index: u32| {
        request
            .choices
            .iter()
            .any(|choice| choice.delegate_index == delegate_index)
    };
    if let Some(slot) = slots.iter().find(|slot| !chosen(slot.delegate_index)) {
        return Err(corrupt(format!(
            "a slot names delegate {}, which no choice names",
            slot.delegate_index
        )));
    }
    if let Some(choice) = request.choices.iter().find(|choice| {
        !slots
            .iter()
            .any(|slot| slot.delegate_index == choice.delegate_index)
    }) {
        return Err(corrupt(format!(
            "delegate {} has no slot",
            choice.delegate_index
        )));
    }
    Ok(first.layout)
}

/// Decodes `choices_json` back into choices.
fn decode_choices(choices_json: &str) -> Result<Vec<DelegateChoice>, ProxyAllocationError> {
    serde_json::from_str(choices_json).map_err(|error| corrupt(format!("choices_json: {error}")))
}

/// Parses a `proxy_slots.layout` value.
fn layout_from_column(value: i64) -> Result<DcShareLayout, ProxyAllocationError> {
    crate::proxy::layout_from_column(value)
        .ok_or_else(|| corrupt(format!("unknown layout {value}")))
}

/// Reads a non-negative `u32` column.
fn column_u32(column: &str, value: i64) -> Result<u32, ProxyAllocationError> {
    u32::try_from(value).map_err(|_| corrupt(format!("{column} {value} is out of range")))
}

/// Reads a non-negative `u64` column.
fn column_u64(column: &str, value: i64) -> Result<u64, ProxyAllocationError> {
    u64::try_from(value).map_err(|_| corrupt(format!("{column} {value} is negative")))
}

/// A stored-row inconsistency.
fn corrupt(message: String) -> ProxyAllocationError {
    ProxyAllocationError::CorruptStoredAllocation { message }
}

/// Maps a SQLite failure during `context` to a storage error, keeping busy
/// distinguishable.
fn storage_error(context: &'static str) -> impl Fn(rusqlite::Error) -> ProxyAllocationError {
    move |error| ProxyAllocationError::Storage(VotingError::from_sqlite(context, &error))
}
