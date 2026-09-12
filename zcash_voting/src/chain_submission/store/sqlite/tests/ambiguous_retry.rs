//! Ambiguous retry reservation: which hashless `Recovering` rows may reserve
//! the next same-generation POST.

use super::super::*;
use super::fixtures::*;

fn hashless_recovering_store(
    label: &str,
    observation: SubmissionObservation,
) -> (
    String,
    SqliteChainSubmissionStore,
    ChainSubmissionGeneration,
) {
    let path = temporary_path(label);
    let store = SqliteChainSubmissionStore::new(open_prepared(&path));
    let StoreAdmission::Ready { derived, .. } = store
        .admit(&StoreAdvancementRequest::vote(identity()), true, 1, 10)
        .unwrap()
    else {
        panic!("fresh admission")
    };
    let generation = derived.generation().clone();
    let record = store.classify_post(&generation, observation, 11).unwrap();
    assert_eq!(record.durable_state(), ChainSubmissionState::Recovering);
    assert_eq!(record.committed_post_reservations(), 1);
    (path, store, generation)
}

#[test]
fn invalid_protocol_response_ambiguity_reserves_the_next_retry() {
    let (path, store, generation) = hashless_recovering_store(
        "invalid-protocol-retry",
        SubmissionObservation::PossiblyDispatched(
            ChainSubmissionDiagnostic::from_redacted_message(
                ChainSubmissionDiagnosticKind::InvalidProtocolResponse,
                "accepted vote-chain response omitted a canonical transaction hash",
            ),
        ),
    );

    let reserved = store.reserve_ambiguous_retry(&generation, 12).unwrap();

    assert_eq!(reserved.durable_state(), ChainSubmissionState::Recovering);
    assert_eq!(reserved.committed_post_reservations(), 2);
    assert_eq!(
        reserved.diagnostic().unwrap().kind(),
        ChainSubmissionDiagnosticKind::InvalidProtocolResponse
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn definite_rejection_recovery_refuses_an_ambiguous_retry() {
    let (path, store, generation) = hashless_recovering_store(
        "rejected-no-retry",
        SubmissionObservation::DefiniteRejection(ChainSubmissionDiagnostic::from_redacted_message(
            ChainSubmissionDiagnosticKind::ChainRejected,
            "vote chain rejected transaction with code 7",
        )),
    );

    let failure = match store.reserve_ambiguous_retry(&generation, 12) {
        Ok(_) => panic!("definite rejection must not reserve an ambiguous retry"),
        Err(failure) => failure,
    };

    assert_eq!(
        failure.kind(),
        ChainSubmissionFailureKind::InvariantViolation
    );
    assert_eq!(
        failure.strongest_state().unwrap().state(),
        ChainSubmissionState::Recovering
    );
    assert_eq!(
        store
            .admit(&StoreAdvancementRequest::vote(identity()), true, 1, 13)
            .map(|admission| match admission {
                StoreAdmission::Ready { record, .. } => record.committed_post_reservations(),
                StoreAdmission::Authoritative(record) => record.committed_post_reservations(),
                _ => panic!("row must persist"),
            })
            .unwrap(),
        1
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn non_dispatch_after_ambiguity_survives_database_reopen() {
    let (path, store, generation) = hashless_recovering_store(
        "ingress-timeout-after-ambiguity",
        SubmissionObservation::PossiblyDispatched(
            ChainSubmissionDiagnostic::from_redacted_message(
                ChainSubmissionDiagnosticKind::AmbiguousDispatch,
                "response lost after broadcast",
            ),
        ),
    );
    store.reserve_ambiguous_retry(&generation, 12).unwrap();
    let record = store
        .reconcile(
            &generation,
            SubmissionObservation::DefinitelyNotDispatched,
            None,
            13,
        )
        .unwrap();
    assert_eq!(record.durable_state(), ChainSubmissionState::Recovering);
    drop(store);
    let reopened = SqliteChainSubmissionStore::new(open_prepared(&path));
    let StoreAdmission::Ready { record, .. } = reopened
        .admit(&StoreAdvancementRequest::vote(identity()), true, 1, 14)
        .unwrap()
    else {
        panic!("earlier ambiguity must remain recoverable")
    };
    assert_eq!(record.durable_state(), ChainSubmissionState::Recovering);
    assert_eq!(record.committed_post_reservations(), 2);
    assert_eq!(
        record.diagnostic().unwrap().kind(),
        ChainSubmissionDiagnosticKind::AmbiguousDispatch
    );
    drop(reopened);
    let _ = std::fs::remove_file(path);
}
