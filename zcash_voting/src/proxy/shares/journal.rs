//! A landed DC's durable share plan and delivery state, `proxy_share_plans`
//! and `proxy_dc_shares` (schema version 26).
//!
//! The plan and its share rows are written once, together. After that only
//! a share's helper lists (under `share::ShareDeliveryState`'s precedence),
//! confirmation and hand-off change, and every write
//! requires the nullifier the caller loaded, so it never lands on another
//! share.

use std::collections::BTreeMap;

use rusqlite::{named_params, Connection, OptionalExtension};

use super::{DcShareRecord, LandedDcShares};
use crate::{
    helper::url::canonicalize_helper_base_url,
    share::{
        ShareAttemptCapacityPolicy, ShareDeliveryAttemptOutcome, ShareDeliveryState,
        ShareOperationScope,
    },
    share_policy::ShareSubmissionPlan,
    share_tracking::{ShareAttemptJournal, ShareKey},
    storage::{queries::ShareAttemptReservation, VotingDb},
    types::VotingError,
};

/// Every landed DC of `round_id`, with its planned shares, by `(bundle_index, dc_slot)`. Reads only on `conn`, so
/// round planning can call it inside its snapshot transaction.
pub(crate) fn landed_dc_shares_on(
    conn: &Connection,
    wallet_id: &str,
    round_id: &str,
) -> Result<BTreeMap<(u32, u8), LandedDcShares>, VotingError> {
    let mut statement = conn
        .prepare(
            "SELECT landing.bundle_index, landing.dc_slot, slot.layout, landing.tree_position
               FROM proxy_dc_landings landing
               JOIN proxy_slots slot
                 ON slot.round_id = landing.round_id AND slot.wallet_id = landing.wallet_id
                AND slot.bundle_index = landing.bundle_index AND slot.dc_slot = landing.dc_slot
              WHERE landing.round_id = :round_id AND landing.wallet_id = :wallet_id
              ORDER BY landing.bundle_index, landing.dc_slot",
        )
        .map_err(|error| VotingError::from_sqlite("prepare landed proxy DC query", &error))?;
    let rows = statement
        .query_map(
            named_params! { ":round_id": round_id, ":wallet_id": wallet_id },
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
        .map_err(|error| VotingError::from_sqlite("load landed proxy DCs", &error))?;
    let mut landed = BTreeMap::new();
    for (bundle_index, dc_slot, layout, tree_position) in rows {
        let corrupt = |what: &str| VotingError::Internal {
            message: format!("a stored proxy DC landing has {what}"),
        };
        let bundle_index =
            u32::try_from(bundle_index).map_err(|_| corrupt("an invalid bundle index"))?;
        let dc_slot = u8::try_from(dc_slot).map_err(|_| corrupt("an invalid slot"))?;
        let shares = load_dc_shares(conn, round_id, wallet_id, bundle_index, dc_slot)?;
        landed.insert(
            (bundle_index, dc_slot),
            LandedDcShares {
                bundle_index,
                dc_slot,
                layout: crate::proxy::layout_from_column(layout)
                    .ok_or_else(|| corrupt("an unknown layout"))?,
                tree_position: u64::try_from(tree_position)
                    .map_err(|_| corrupt("a negative tree position"))?,
                shares,
            },
        );
    }
    Ok(landed)
}

/// The DC's planned shares in share order, or `None` before it is planned.
pub(super) fn load_dc_shares(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    bundle_index: u32,
    dc_slot: u8,
) -> Result<Option<Vec<DcShareRecord>>, VotingError> {
    // A plan and its share rows are written together and leave together, so
    // a DC has share rows exactly when it is planned.
    let mut statement = conn
        .prepare(&format!(
            "SELECT {SHARE_COLUMNS}
               FROM proxy_dc_shares
              WHERE round_id = :round_id AND wallet_id = :wallet_id
                AND bundle_index = :bundle_index AND dc_slot = :dc_slot
              ORDER BY share_index"
        ))
        .map_err(|error| VotingError::from_sqlite("prepare DC share query", &error))?;
    let records = statement
        .query_map(
            named_params! {
                ":round_id": round_id,
                ":wallet_id": wallet_id,
                ":bundle_index": i64::from(bundle_index),
                ":dc_slot": i64::from(dc_slot),
            },
            |row| Ok(share_record(row, "load DC shares", bundle_index, dc_slot)),
        )
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
        .map_err(|error| VotingError::from_sqlite("load DC shares", &error))?
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    Ok((!records.is_empty()).then_some(records))
}

/// One stored share of the DC in `bundle_index`'s slot `dc_slot`.
pub(super) fn load_dc_share(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    bundle_index: u32,
    dc_slot: u8,
    share_index: u32,
) -> Result<Option<DcShareRecord>, VotingError> {
    conn.query_row(
        &format!(
            "SELECT {SHARE_COLUMNS}
               FROM proxy_dc_shares
              WHERE round_id = :round_id AND wallet_id = :wallet_id
                AND bundle_index = :bundle_index AND dc_slot = :dc_slot
                AND share_index = :share_index"
        ),
        named_params! {
            ":round_id": round_id,
            ":wallet_id": wallet_id,
            ":bundle_index": i64::from(bundle_index),
            ":dc_slot": i64::from(dc_slot),
            ":share_index": i64::from(share_index),
        },
        |row| Ok(share_record(row, "load DC share", bundle_index, dc_slot)),
    )
    .optional()
    .map_err(|error| VotingError::from_sqlite("load DC share", &error))?
    .transpose()
}

/// The DC's stored share plans, one per planned share in share order, or
/// `None` before it is planned. The fleet stored beside them is never read
/// back.
pub(super) fn load_dc_share_plans(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    bundle_index: u32,
    dc_slot: u8,
) -> Result<Option<Vec<ShareSubmissionPlan>>, VotingError> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT share_plans_json FROM proxy_share_plans
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
        .optional()
        .map_err(|error| VotingError::from_sqlite("load DC share plan", &error))?;
    stored
        .map(|plans| {
            serde_json::from_str(&plans).map_err(|error| VotingError::Internal {
                message: format!(
                    "the stored share plan of the DC in slot {dc_slot} of bundle {bundle_index} \
                     is unreadable: {error}"
                ),
            })
        })
        .transpose()
}

/// Writes a DC's plan as first made, made against the canonical fleet
/// `configured_server_urls` at `now`, and its share rows, each delivered
/// share's index and reveal nullifier in `plans` order, inside the caller's
/// write transaction.
#[allow(clippy::too_many_arguments)]
pub(super) fn insert_dc_share_plan(
    conn: &Connection,
    round_id: &str,
    wallet_id: &str,
    bundle_index: u32,
    dc_slot: u8,
    configured_server_urls: &[String],
    plans: &[ShareSubmissionPlan],
    shares: &[(u32, [u8; 32])],
    now: u64,
) -> Result<(), VotingError> {
    if plans.len() != shares.len() {
        return Err(VotingError::Internal {
            message: format!(
                "{} share plans for {} shares of the DC in slot {dc_slot}",
                plans.len(),
                shares.len()
            ),
        });
    }
    let encoding_error = |error: serde_json::Error| VotingError::Internal {
        message: format!("failed to encode a DC share plan: {error}"),
    };
    let fleet = serde_json::to_string(configured_server_urls).map_err(encoding_error)?;
    let plans_json = serde_json::to_string(plans).map_err(encoding_error)?;
    let now = i64::try_from(now).unwrap_or(i64::MAX);
    conn.execute(
        "INSERT INTO proxy_share_plans
             (round_id, wallet_id, bundle_index, dc_slot, configured_server_urls_json,
              share_plans_json, created_at)
         VALUES (:round_id, :wallet_id, :bundle_index, :dc_slot, :fleet, :plans, :now)",
        named_params! {
            ":round_id": round_id,
            ":wallet_id": wallet_id,
            ":bundle_index": i64::from(bundle_index),
            ":dc_slot": i64::from(dc_slot),
            ":fleet": fleet,
            ":plans": plans_json,
            ":now": now,
        },
    )
    .map_err(|error| VotingError::from_sqlite("insert DC share plan", &error))?;
    for (share_plan, (share_index, nullifier)) in plans.iter().zip(shares) {
        conn.execute(
            "INSERT INTO proxy_dc_shares
                 (round_id, wallet_id, bundle_index, dc_slot, share_index, nullifier,
                  target_count, submit_at, created_at)
             VALUES (:round_id, :wallet_id, :bundle_index, :dc_slot, :share_index, :nullifier,
                     :target_count, :submit_at, :now)",
            named_params! {
                ":round_id": round_id,
                ":wallet_id": wallet_id,
                ":bundle_index": i64::from(bundle_index),
                ":dc_slot": i64::from(dc_slot),
                ":share_index": i64::from(*share_index),
                ":nullifier": nullifier.as_slice(),
                ":target_count": i64::from(share_plan.target_count),
                ":submit_at": i64::try_from(share_plan.submit_at).unwrap_or(i64::MAX),
                ":now": now,
            },
        )
        .map_err(|error| VotingError::from_sqlite("insert DC share", &error))?;
    }
    Ok(())
}

/// Marks the share confirmed when it still has `nullifier`. Returns `false`
/// when it was replaced or deleted.
pub(super) fn mark_dc_share_confirmed(
    db: &VotingDb,
    scope: &ShareOperationScope,
    round_id: &str,
    share: ShareKey,
    dc_slot: u8,
    nullifier: &[u8; 32],
) -> Result<bool, VotingError> {
    let updated = db
        .conn()
        .execute(
            "UPDATE proxy_dc_shares SET confirmed = 1
              WHERE round_id = :round_id AND wallet_id = :wallet_id
                AND bundle_index = :bundle_index AND dc_slot = :dc_slot
                AND share_index = :share_index AND nullifier = :nullifier",
            named_params! {
                ":round_id": round_id,
                ":wallet_id": scope.wallet_id(),
                ":bundle_index": i64::from(share.bundle_index),
                ":dc_slot": i64::from(dc_slot),
                ":share_index": i64::from(share.share_index),
                ":nullifier": nullifier.as_slice(),
            },
        )
        .map_err(|error| VotingError::from_sqlite("confirm DC share", &error))?;
    Ok(updated == 1)
}

/// Records each `(share_index, nullifier)` of the DC in `bundle_index`'s
/// slot `dc_slot` as handed off to its reachable target at `now`, in one
/// transaction (see `delivery::record_hand_offs`). A share keeps its first
/// hand-off.
pub(super) fn record_dc_share_hand_offs(
    db: &VotingDb,
    scope: &ShareOperationScope,
    round_id: &str,
    bundle_index: u32,
    dc_slot: u8,
    hand_offs: &[(u32, [u8; 32])],
    now: u64,
) -> Result<(), VotingError> {
    db.write_transaction("record DC share hand-offs", |tx| {
        for (share_index, nullifier) in hand_offs {
            tx.execute(
                "UPDATE proxy_dc_shares SET handed_off_at = :now
                  WHERE round_id = :round_id AND wallet_id = :wallet_id
                    AND bundle_index = :bundle_index AND dc_slot = :dc_slot
                    AND share_index = :share_index AND nullifier = :nullifier
                    AND handed_off_at IS NULL",
                named_params! {
                    ":now": i64::try_from(now).unwrap_or(i64::MAX),
                    ":round_id": round_id,
                    ":wallet_id": scope.wallet_id(),
                    ":bundle_index": i64::from(bundle_index),
                    ":dc_slot": i64::from(dc_slot),
                    ":share_index": i64::from(*share_index),
                    ":nullifier": nullifier.as_slice(),
                },
            )
            .map_err(|error| VotingError::from_sqlite("record DC share hand-off", &error))?;
        }
        Ok(())
    })
}

/// One DC share's helper attempts in `proxy_dc_shares`, bound to the
/// nullifier its caller loaded.
pub(crate) struct DcShareJournal<'a> {
    pub(crate) db: &'a VotingDb,
    /// The wallet the delivery runs for, captured before any await.
    pub(crate) scope: &'a ShareOperationScope,
    pub(crate) round_id: &'a str,
    pub(crate) bundle_index: u32,
    pub(crate) dc_slot: u8,
    pub(crate) share_index: u32,
    /// The share's reveal nullifier, as loaded.
    pub(crate) nullifier: [u8; 32],
}

impl DcShareJournal<'_> {
    /// The share as stored now, or `None` once it was replaced or deleted.
    pub(super) fn record(&self) -> Result<Option<DcShareRecord>, VotingError> {
        self.db
            .read_transaction("load DC share", |tx| self.current(tx))
    }

    /// The share's row, or `None` when it no longer has this nullifier.
    fn current(&self, conn: &Connection) -> Result<Option<DcShareRecord>, VotingError> {
        Ok(load_dc_share(
            conn,
            self.round_id,
            self.scope.wallet_id(),
            self.bundle_index,
            self.dc_slot,
            self.share_index,
        )?
        .filter(|record| record.nullifier == self.nullifier))
    }

    /// Stores `state` as the share's helper lists.
    fn write_state(
        &self,
        conn: &Connection,
        state: &ShareDeliveryState,
    ) -> Result<(), VotingError> {
        let encode = |urls: &[String]| {
            serde_json::to_string(urls).map_err(|error| VotingError::Internal {
                message: format!("failed to encode DC share helpers: {error}"),
            })
        };
        conn.execute(
            "UPDATE proxy_dc_shares
                SET sent_to_urls = :sent, ambiguous_urls = :ambiguous, attempting_urls = :attempting
              WHERE round_id = :round_id AND wallet_id = :wallet_id
                AND bundle_index = :bundle_index AND dc_slot = :dc_slot
                AND share_index = :share_index AND nullifier = :nullifier",
            named_params! {
                ":sent": encode(state.accepted_urls())?,
                ":ambiguous": encode(state.outcome_unknown_urls())?,
                ":attempting": encode(state.in_flight_urls())?,
                ":round_id": self.round_id,
                ":wallet_id": self.scope.wallet_id(),
                ":bundle_index": i64::from(self.bundle_index),
                ":dc_slot": i64::from(self.dc_slot),
                ":share_index": i64::from(self.share_index),
                ":nullifier": self.nullifier.as_slice(),
            },
        )
        .map(|_| ())
        .map_err(|error| VotingError::from_sqlite("record DC share helpers", &error))
    }
}

impl ShareAttemptJournal for DcShareJournal<'_> {
    fn share(&self) -> ShareKey {
        ShareKey::proxy(self.bundle_index, self.dc_slot, self.share_index)
    }

    fn begin_attempt(
        &self,
        server_url: &str,
        target_count: usize,
        candidates: &[String],
        capacity: ShareAttemptCapacityPolicy,
    ) -> Result<ShareAttemptReservation, VotingError> {
        let server_url = canonicalize_helper_base_url(server_url)?;
        if !candidates.contains(&server_url) {
            return Err(VotingError::InvalidInput {
                message: "attempted helper must belong to the placement fleet".to_string(),
            });
        }
        self.db.write_transaction("reserve DC share helper", |tx| {
            let Some(record) = self.current(tx)? else {
                return Ok(ShareAttemptReservation::StaleGeneration);
            };
            let mut state = ShareDeliveryState::from_url_lists(
                &record.sent_to_urls,
                &record.ambiguous_urls,
                &record.attempting_urls,
            )?;
            let known = state
                .accepted_urls()
                .iter()
                .chain(state.outcome_unknown_urls())
                .chain(state.in_flight_urls())
                .any(|url| url == &server_url);
            if record.confirmed || known {
                return Ok(ShareAttemptReservation::AlreadyRecorded);
            }
            let reserved = state
                .accepted_urls()
                .iter()
                .chain(state.in_flight_urls())
                .filter(|url| candidates.contains(url))
                .count();
            if capacity == ShareAttemptCapacityPolicy::EnforcePlacementTarget
                && reserved >= target_count
            {
                return Ok(ShareAttemptReservation::PlacementCapacityReached);
            }
            // Called unconditionally: the reservation is this call's only
            // durable effect, so it must never sit inside an assertion.
            state.begin(&server_url)?;
            self.write_state(tx, &state)?;
            Ok(ShareAttemptReservation::Started)
        })
    }

    /// Ignores `reset_submit_at`: a DC share keeps its planned `submit_at`
    /// (`proxy_dc_shares_plan_immutable`). That changes no timing, since an
    /// overdue share stays overdue whichever base time it is judged by.
    fn resolve_attempt(
        &self,
        server_url: &str,
        outcome: ShareDeliveryAttemptOutcome,
        _reset_submit_at: bool,
    ) -> Result<bool, VotingError> {
        self.db
            .write_transaction("record DC share helper outcome", |tx| {
                let Some(record) = self.current(tx)? else {
                    return Ok(false);
                };
                let mut state = ShareDeliveryState::from_url_lists(
                    &record.sent_to_urls,
                    &record.ambiguous_urls,
                    &record.attempting_urls,
                )?;
                match outcome {
                    ShareDeliveryAttemptOutcome::Accepted => state.mark_accepted(server_url)?,
                    ShareDeliveryAttemptOutcome::Ambiguous => {
                        state.mark_outcome_unknown(server_url)?
                    }
                    ShareDeliveryAttemptOutcome::DefiniteFailure => {
                        state.mark_definite_failure(server_url)?
                    }
                }
                self.write_state(tx, &state)?;
                Ok(true)
            })
    }
}

/// The `proxy_dc_shares` columns [`share_record`] reads, in its order.
const SHARE_COLUMNS: &str = "share_index, nullifier, target_count, submit_at, sent_to_urls, \
                             ambiguous_urls, attempting_urls, confirmed, handed_off_at, created_at";

/// One row selected as [`SHARE_COLUMNS`] as a record, refusing values the
/// schema cannot hold. A column SQLite cannot read fails as `context`.
fn share_record(
    row: &rusqlite::Row<'_>,
    context: &str,
    bundle_index: u32,
    dc_slot: u8,
) -> Result<DcShareRecord, VotingError> {
    let sqlite = |error: rusqlite::Error| VotingError::from_sqlite(context, &error);
    let corrupt = |what: &str| VotingError::Internal {
        message: format!(
            "a stored share of the DC in slot {dc_slot} of bundle {bundle_index} has {what}"
        ),
    };
    let int = |column: usize| row.get::<_, i64>(column).map_err(sqlite);
    let urls = |column: usize| {
        serde_json::from_str::<Vec<String>>(&row.get::<_, String>(column).map_err(sqlite)?)
            .map_err(|_| corrupt("unreadable helpers"))
    };
    Ok(DcShareRecord {
        share_index: u32::try_from(int(0)?).map_err(|_| corrupt("an invalid share index"))?,
        nullifier: crate::proxy::blob_array(
            "nullifier",
            row.get::<_, Vec<u8>>(1).map_err(sqlite)?,
            corrupt,
        )?,
        target_count: u32::try_from(int(2)?).map_err(|_| corrupt("an invalid target"))?,
        submit_at: u64::try_from(int(3)?).map_err(|_| corrupt("a negative submit_at"))?,
        sent_to_urls: urls(4)?,
        ambiguous_urls: urls(5)?,
        attempting_urls: urls(6)?,
        confirmed: row.get(7).map_err(sqlite)?,
        handed_off_to_reachable_target: row.get::<_, Option<i64>>(8).map_err(sqlite)?.is_some(),
        created_at: u64::try_from(int(9)?).map_err(|_| corrupt("a negative created_at"))?,
    })
}
