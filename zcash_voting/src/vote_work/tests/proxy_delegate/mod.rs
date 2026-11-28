//! The `ProxyDelegate` step: it plans the batch once and proves it in
//! phases, ZKP1 before every ZKP4, and refuses what a new batch may not do.

mod fixtures;
mod real_proofs;

use fixtures::{interrupting_host, note_total, proxy_database, proxy_executor};

use super::fixtures::{host, RecordingProgress, ROUND_ID};
use crate::{
    phases::DelegationPhase, session::NextStep, ChainSubmissionControl, RoundStepDisposition,
    RoundStepFailureKind, RoundStepProgress,
};

/// The step every fixture round plans first.
const STEP: NextStep = NextStep::ProxyDelegate { bundle_index: 0 };

/// Every slot's nullifier, in slot order; `None` where batch planning has not
/// written one.
fn slot_nullifiers(db: &crate::round::VotingDb) -> Vec<Option<Vec<u8>>> {
    let conn = db.conn();
    let mut statement = conn
        .prepare("SELECT dc_slot_nf FROM proxy_slots ORDER BY bundle_index, dc_slot")
        .unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// Whether a proved batch is stored for bundle 0.
fn batch_is_proved(db: &crate::round::VotingDb) -> bool {
    db.conn()
        .query_row("SELECT EXISTS (SELECT 1 FROM proxy_batches)", [], |row| {
            row.get(0)
        })
        .unwrap()
}

/// ZKP1 runs before any ZKP4: an interruption raised as the registration's
/// proof is stored ends the step before the ZKP4 phase, with the batch
/// planned, the registration proved and nothing more.
#[tokio::test]
async fn the_registration_is_proved_before_any_dc() {
    let db = proxy_database(note_total());
    let executor = proxy_executor(db.clone(), true);
    let control = ChainSubmissionControl::new(1);
    let host = interrupting_host(&control, &db);
    let progress = RecordingProgress::default();

    let outcome = executor
        .advance_step_in_epoch(STEP, &host, &control, control.operation_epoch(), &progress)
        .await
        .unwrap();

    assert_eq!(outcome.disposition, RoundStepDisposition::Cancelled);
    assert!(
        slot_nullifiers(&db).iter().all(Option::is_some),
        "planning precedes both proof phases"
    );
    assert_eq!(
        db.delegation_phase(ROUND_ID, 0).unwrap(),
        DelegationPhase::Proved
    );
    assert!(!batch_is_proved(&db), "no ZKP4 ran after the interruption");
    let events = progress.events.lock().unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        RoundStepProgress::Delegation {
            bundle_index: 0,
            ..
        }
    )));
    assert!(!events
        .iter()
        .any(|event| matches!(event, RoundStepProgress::ProxyBatchProved { .. })));
    drop(events);
    // The batch is still owed.
    assert_eq!(executor.plan().unwrap().next_steps, vec![STEP]);
}

/// After the authenticated vote end no new batch is planned or proved.
#[tokio::test]
async fn a_batch_after_the_vote_end_is_refused() {
    let db = proxy_database(note_total());
    let executor = proxy_executor(db.clone(), true);
    let control = ChainSubmissionControl::new(1);
    let mut host = interrupting_host(&control, &db);
    host.now_seconds = host.vote_end_time_seconds.unwrap();

    let failure = executor
        .advance_step_in_epoch(
            STEP,
            &host,
            &control,
            control.operation_epoch(),
            &RecordingProgress::default(),
        )
        .await
        .unwrap_err();

    assert_eq!(failure.kind, RoundStepFailureKind::VoteEnded);
    assert!(slot_nullifiers(&db).iter().all(Option::is_none));
}

/// Without the round hotkey there is nothing to derive the slot nullifiers
/// and DC secrets from.
#[tokio::test]
async fn a_batch_needs_the_round_hotkey() {
    let db = proxy_database(note_total());
    let executor = proxy_executor(db.clone(), false);
    let control = ChainSubmissionControl::new(1);

    let failure = executor
        .advance_step_in_epoch(
            STEP,
            &host(),
            &control,
            control.operation_epoch(),
            &RecordingProgress::default(),
        )
        .await
        .unwrap_err();

    assert_eq!(failure.kind, RoundStepFailureKind::InvalidInput);
    assert!(failure.message.contains("hotkey"), "{}", failure.message);
    assert!(slot_nullifiers(&db).iter().all(Option::is_none));
}

/// A bundle whose registration already landed proves no ZKP1: its batch
/// spends the confirmed VAN, so the step syncs the vote tree for its witness
/// after planning the batch.
#[tokio::test]
async fn a_confirmed_registration_anchors_its_batch_in_the_vote_tree() {
    let db = proxy_database(note_total());
    db.store_delegation_tx_hash(ROUND_ID, 0, "dtx").unwrap();
    db.store_van_position(ROUND_ID, 0, 7).unwrap();
    let transport = std::sync::Arc::new(super::fixtures::UnreachableTreeTransport {
        requests: std::sync::atomic::AtomicUsize::new(0),
        cancel_on_request: None,
    });
    let executor = proxy_executor(db.clone(), true).with_tree_transport(transport.clone());
    assert_eq!(executor.plan().unwrap().next_steps, vec![STEP]);
    let control = ChainSubmissionControl::new(1);

    // No delegation inputs: a ZKP1 phase would refuse for want of a driver.
    let failure = executor
        .advance_step_in_epoch(
            STEP,
            &host(),
            &control,
            control.operation_epoch(),
            &RecordingProgress::default(),
        )
        .await
        .unwrap_err();

    assert!(
        failure.message.contains("vote tree sync"),
        "the step went to the tree, not to ZKP1: {}",
        failure.message
    );
    assert!(transport.requests.load(std::sync::atomic::Ordering::SeqCst) > 0);
    assert!(slot_nullifiers(&db).iter().all(Option::is_some));
    assert!(!batch_is_proved(&db));
}
