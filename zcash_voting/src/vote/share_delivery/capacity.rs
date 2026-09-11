//! Admission shared by every singleton and multi-proposal initial delivery.

use crate::{share_policy::SHARE_HELPER_MAX_CONCURRENT_POSTS, VotingError};
use std::{sync::LazyLock, time::Duration};
use tokio::sync::{Semaphore, SemaphorePermit};

/// Maximum active share workflows, including fleets with small placement targets.
pub(super) const MAX_CONCURRENT_SHARE_DELIVERIES: usize = 50;
// Scale the budget so both limits remain exact when 128 is not divisible by 50.
// Each workflow consumes at least 1/50 of capacity and each target at least 1/128.
const DELIVERY_CAPACITY: usize =
    SHARE_HELPER_MAX_CONCURRENT_POSTS * MAX_CONCURRENT_SHARE_DELIVERIES;
static DELIVERY_PERMITS: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(DELIVERY_CAPACITY));

/// Waits for capacity without creating durable reservations. Cancellation is
/// observed within 50 ms even while all permits remain occupied. The caller
/// supplies the validated plan's target (bounded by the protocol helper cap)
/// and holds the full charge until its share's outcomes have been journaled.
/// Atomic weighted admission keeps queued fan-out outside the delivery deadline
/// without partially acquired capacity blocking other wallets indefinitely.
pub(super) async fn acquire(
    planned_target: u32,
    cancel: &(dyn Fn() -> bool + Send + Sync),
) -> Result<Option<SemaphorePermit<'static>>, VotingError> {
    let charge = (planned_target * MAX_CONCURRENT_SHARE_DELIVERIES as u32)
        .max(SHARE_HELPER_MAX_CONCURRENT_POSTS as u32);
    let admission = DELIVERY_PERMITS.acquire_many(charge);
    tokio::pin!(admission);
    loop {
        if cancel() {
            return Ok(None);
        }
        tokio::select! {
            biased;
            permit = &mut admission => {
                let permit = permit.map_err(|_| VotingError::Internal { message: "helper-share delivery semaphore closed".to_string() })?;
                return Ok((!cancel()).then_some(permit));
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }
}
