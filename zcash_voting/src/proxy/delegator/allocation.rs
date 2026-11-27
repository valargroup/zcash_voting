//! Client API preview and commit of the round's one allocation.

use rand::rngs::OsRng;

use crate::{
    proxy::{batch_layout, commit_allocation, preview_allocation, AllocationRequest},
    storage::VotingDb,
    types::validate_vote_round_id_hex,
    vote_work::RoundHostContext,
    wire::{
        DelegateCheck, ProxyAllocationPreview, ProxyAllocationRequest, ProxyDelegationError,
        ProxyDelegationErrorKind,
    },
};

/// Plans `request` for the current wallet's bundles without recording it,
/// using the host's round timing for the layout. Refused without timing,
/// after the vote end, and once the round has an allocation.
pub fn preview(
    db: &VotingDb,
    request: &ProxyAllocationRequest,
    host: &RoundHostContext,
) -> Result<ProxyAllocationPreview, ProxyDelegationError> {
    let planner_request = admit(request, host)?;
    let plan = preview_allocation(db, &request.round_id, &planner_request, batch_layout(host))?;
    Ok(ProxyAllocationPreview::from(&plan))
}

/// Records `request` as the round's one, final allocation (D2).
///
/// `confirmed` is the preview the delegator confirmed. The commit plans
/// again under `host`'s round timing, and a plan that no longer previews the
/// same is refused with [`ProxyDelegationErrorKind::PlanChanged`]. `checks`
/// are the [`check_delegates`](crate::proxy::registry::check_delegates)
/// verdicts made just before, one per choice, and each must be `Ok`
/// (`CannotVerify` refuses with that kind). The commit's own transaction can
/// still refuse with `AlreadyVoted` or `RegistrationUnusable`, and with
/// `KeptShareUnanswered` when the plan keeps ballots and some proposal in
/// `roster`, the round's authenticated proposal ids, has no recorded ballot
/// intent (see [`commit_allocation`]). Slots are drawn from the OS CSPRNG.
pub fn commit(
    db: &VotingDb,
    request: &ProxyAllocationRequest,
    confirmed: &ProxyAllocationPreview,
    checks: &[DelegateCheck],
    host: &RoundHostContext,
    roster: &[u32],
) -> Result<(), ProxyDelegationError> {
    let planner_request = admit(request, host)?;
    require_allowed(request, checks)?;
    commit_allocation(
        db,
        &request.round_id,
        &planner_request,
        batch_layout(host),
        confirmed,
        roster,
        &mut OsRng,
    )?;
    Ok(())
}

/// The planner's request for `request`, refusing a malformed round id,
/// missing timing or an ended vote.
fn admit(
    request: &ProxyAllocationRequest,
    host: &RoundHostContext,
) -> Result<AllocationRequest, ProxyDelegationError> {
    validate_vote_round_id_hex(&request.round_id)?;
    let (Some(_), Some(vote_end)) = (host.ceremony_start_seconds, host.vote_end_time_seconds)
    else {
        return Err(ProxyDelegationError::other(
            "the round's timing is unknown, so its last-moment window is unknown",
        ));
    };
    if host.now_seconds >= vote_end {
        return Err(ProxyDelegationError::new(
            ProxyDelegationErrorKind::VoteEnded,
            format!(
                "voting ended at {vote_end}; the host clock reads {}",
                host.now_seconds
            ),
        ));
    }
    Ok(AllocationRequest::from(request))
}

/// Refuses unless `checks` hold one allowing verdict per choice.
fn require_allowed(
    request: &ProxyAllocationRequest,
    checks: &[DelegateCheck],
) -> Result<(), ProxyDelegationError> {
    if checks.len() != request.choices.len() {
        return Err(ProxyDelegationError::other(format!(
            "{} registry checks for {} choices",
            checks.len(),
            request.choices.len()
        )));
    }
    for (choice, check) in request.choices.iter().zip(checks) {
        match check {
            DelegateCheck::Ok => {}
            DelegateCheck::CannotVerify => {
                return Err(ProxyDelegationError::new(
                    ProxyDelegationErrorKind::CannotVerify,
                    format!(
                        "delegate {}'s entry could not be verified",
                        choice.delegate_index
                    ),
                ))
            }
            DelegateCheck::Suspended
            | DelegateCheck::Retired
            | DelegateCheck::NotFound
            | DelegateCheck::Mismatch => {
                return Err(ProxyDelegationError::other(format!(
                    "delegate {} is refused: {check:?}",
                    choice.delegate_index
                )))
            }
        }
    }
    Ok(())
}
