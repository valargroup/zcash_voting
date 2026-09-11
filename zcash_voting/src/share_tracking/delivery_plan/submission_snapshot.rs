//! Operation-scoped, consistent reads for a confirmed unit's delivery plans.

use super::*;

/// One handle whose immutable plan must match the durable recovery generation.
pub(crate) struct DeliveryPlanRequest<'a> {
    pub round_id: &'a str,
    pub bundle_index: u32,
    pub proposal_id: u32,
    pub generation: &'a str,
    pub payloads: &'a [SharePayload],
}

/// Shared checks are valid only inside the read transaction that produced them.
pub(super) struct RoundDeliveryAudit {
    pub(super) decisions: BTreeMap<u32, Decision>,
    pub(super) immediate_key: Option<ImmediateShareKey>,
}

impl RoundDeliveryAudit {
    pub(super) fn load(
        conn: &Connection,
        wallet_id: &str,
        round_id: &str,
    ) -> Result<Self, VotingError> {
        // Keep authoritative generation reconstruction even when every intent
        // exists: it also rejects corrupt batch membership and recovery state.
        let decisions = durable_decisions(conn, round_id, wallet_id)?;
        let immediate_key =
            match crate::share_tracking::round_immediate_share(conn, round_id, wallet_id)? {
                Some(key) => Some(key),
                None => {
                    let choices = decisions
                        .iter()
                        .filter_map(|(&proposal, decision)| {
                            matches!(decision, Decision::Choice(_)).then_some(proposal)
                        })
                        .collect::<Vec<_>>();
                    immediate_key_for_choices(conn, round_id, wallet_id, &choices)?
                }
            };
        validate_round_immediate_plans(conn, round_id, wallet_id, immediate_key)?;
        Ok(Self {
            decisions,
            immediate_key,
        })
    }
}

/// Load and audit every requested plan in one deferred read transaction.
/// Successful round audits are shared only within this call. A local failure
/// retains its position without suppressing other proposals. No writes occur;
/// dispatch must still revalidate the exact generation and reserve each POST.
/// The connection and transaction are released before payload wire validation.
pub(crate) fn load_delivery_plans(
    db: &VotingDb,
    scope: &ShareOperationScope,
    requests: &[DeliveryPlanRequest<'_>],
    current_fleet: &[String],
    observations: &crate::ObservationScope,
) -> Vec<Result<(ShareDeliveryPlan, String), VotingError>> {
    let mut conn = db.conn();
    let tx = match conn.transaction_with_behavior(TransactionBehavior::Deferred) {
        Ok(tx) => tx,
        Err(error) => {
            return requests
                .iter()
                .map(|_| {
                    Err(VotingError::from_sqlite(
                        "begin delivery-plan snapshot failed",
                        &error,
                    ))
                })
                .collect()
        }
    };
    let mut round_audits = BTreeMap::new();
    requests
        .iter()
        .map(|request| {
            load_share_delivery_plan(
                &tx,
                scope.wallet_id(),
                request,
                current_fleet,
                &mut round_audits,
                observations,
            )
        })
        .collect()
}
