//! `AdvanceProxyBatch`: when a proved batch is broadcast, how a batch on the
//! wire is planned, and when the hold on the bundle's casts lifts.

use super::fixtures::*;
use crate::phases::{DelegationPhase, VotePhase};
use crate::proxy::batch::{ProxyBatchProgress, ProxyBatchStatus};
use crate::round_planning::{classify_round, plan_from_snapshot, resolve_step, Obligation};
use crate::session::{Decision, NextStep, ProxyBatchPlanStatus, ProxyBatchStage};

/// The `(bundle, signs_registration)` of every `AdvanceProxyBatch`.
fn advanced(obligations: &[Obligation]) -> Vec<(u32, bool)> {
    obligations
        .iter()
        .filter_map(|obligation| match obligation {
            Obligation::AdvanceProxyBatch {
                bundle_index,
                signs_registration,
            } => Some((*bundle_index, *signs_registration)),
            _ => None,
        })
        .collect()
}

/// The `(bundle, signs_delegation)` of every cast.
fn casts(obligations: &[Obligation]) -> Vec<(u32, bool)> {
    obligations
        .iter()
        .filter_map(|obligation| match obligation {
            Obligation::Cast {
                bundle_index,
                signs_delegation,
                ..
            } => Some((*bundle_index, *signs_delegation)),
            _ => None,
        })
        .collect()
}

fn status(progress: ProxyBatchProgress) -> ProxyBatchStatus {
    ProxyBatchStatus::at(progress)
}

/// No batch is broadcast while another bundle's could still fail to prove:
/// a proved batch waits for every batch to be proved.
#[test]
fn every_batch_is_proved_before_any_is_broadcast() {
    let partly_proved = snapshot()
        .bundle(0, DelegationPhase::Proved)
        .bundle(1, DelegationPhase::Proved)
        .proxy_batch(0, ProxyBatchProgress::Proved)
        .proxy_batch(1, ProxyBatchProgress::Planned)
        .build();
    let obligations = classify_round(&partly_proved, &[1]).unwrap().obligations;
    assert!(advanced(&obligations).is_empty());
    assert!(obligations
        .iter()
        .any(|obligation| matches!(obligation, Obligation::ProxyDelegate { bundle_index: 1 })));

    let all_proved = snapshot()
        .bundle(0, DelegationPhase::Proved)
        .bundle(1, DelegationPhase::Confirmed)
        .proxy_batch(0, ProxyBatchProgress::Proved)
        .proxy_batch(1, ProxyBatchProgress::Proved)
        .build();
    assert_eq!(
        advanced(&classify_round(&all_proved, &[1]).unwrap().obligations),
        vec![(0, false), (1, false)]
    );
}

/// A cast that may have spent the VAN holds a proved batch from broadcast:
/// its first DC could never land.
#[test]
fn a_cast_that_may_have_spent_the_van_holds_a_proved_batch() {
    let snapshot = snapshot()
        .bundle(0, DelegationPhase::Confirmed)
        .vote(0, 1, 1, VotePhase::SubmissionManaged)
        .proxy_batch(0, ProxyBatchProgress::Proved)
        .intent(1, Decision::Choice(1))
        .build();

    assert!(advanced(&classify_round(&snapshot, &[1]).unwrap().obligations).is_empty());
}

/// A batch whose first attempt carries the registration asks for the
/// voter's signer, and the plan says so; later attempts do not.
#[test]
fn a_first_attempt_carrying_the_registration_needs_the_signer() {
    let snapshot = snapshot()
        .bundle(0, DelegationPhase::Proved)
        .proxy_status(
            0,
            ProxyBatchStatus {
                needs_registration_signature: true,
                ..status(ProxyBatchProgress::Proved)
            },
        )
        .build();

    let obligations = classify_round(&snapshot, &[1]).unwrap();
    assert_eq!(advanced(&obligations.obligations), vec![(0, true)]);
    let plan = plan_from_snapshot(&snapshot, &[1]).unwrap();
    let step = NextStep::AdvanceProxyBatch { bundle_index: 0 };
    assert_eq!(plan.next_steps, vec![step.clone()]);
    assert!(matches!(
        resolve_step(&obligations.obligations, &step),
        Some(Obligation::AdvanceProxyBatch {
            bundle_index: 0,
            signs_registration: true
        })
    ));
    assert_eq!(plan.delegation_bundles_needing_signing, vec![0]);
    assert!(plan.needs_delegation_signing);

    let in_flight = super::fixtures::snapshot()
        .bundle(0, DelegationPhase::Proved)
        .proxy_batch(0, ProxyBatchProgress::InFlight { refused: false })
        .build();
    let plan = plan_from_snapshot(&in_flight, &[1]).unwrap();
    assert_eq!(plan.next_steps, vec![step]);
    assert_eq!(plan.delegation_bundles_needing_work, vec![0]);
    assert!(plan.delegation_bundles_needing_signing.is_empty());
}

/// A batch on the wire is always advanced, refused or not, until it lands.
/// A landed batch plans nothing more.
#[test]
fn a_batch_on_the_wire_is_advanced_until_it_lands() {
    for (progress, advances) in [
        (ProxyBatchProgress::InFlight { refused: false }, true),
        (ProxyBatchProgress::InFlight { refused: true }, true),
        (ProxyBatchProgress::Landed, false),
    ] {
        let snapshot = snapshot()
            .bundle(0, DelegationPhase::Proved)
            .proxy_batch(0, progress)
            .build();

        let plan = plan_from_snapshot(&snapshot, &[1]).unwrap();

        assert_eq!(
            plan.next_steps
                .contains(&NextStep::AdvanceProxyBatch { bundle_index: 0 }),
            advances,
            "{progress:?}"
        );
    }
}

/// The hold on the bundle's casts lifts only when the batch lands: until
/// then the bundle's share of a choice is withheld.
#[test]
fn the_hold_lifts_only_when_the_batch_lands() {
    for (progress, phase, casts_now) in [
        (ProxyBatchProgress::Planned, DelegationPhase::Proved, false),
        (ProxyBatchProgress::Proved, DelegationPhase::Proved, false),
        (
            ProxyBatchProgress::InFlight { refused: false },
            DelegationPhase::Proved,
            false,
        ),
        (
            ProxyBatchProgress::InFlight { refused: true },
            DelegationPhase::Proved,
            false,
        ),
        (ProxyBatchProgress::Landed, DelegationPhase::Confirmed, true),
    ] {
        let snapshot = snapshot()
            .bundle(0, phase)
            .proxy_batch(0, progress)
            .intent(1, Decision::Choice(1))
            .build();

        let obligations = classify_round(&snapshot, &[1]).unwrap();

        assert_eq!(
            !casts(&obligations.obligations).is_empty(),
            casts_now,
            "{progress:?}"
        );
        assert_eq!(
            obligations.withheld_casts.contains(&1),
            !casts_now,
            "{progress:?}"
        );
    }
}

/// The plan reports every proxy bundle's stage in bundle order.
#[test]
fn the_plan_reports_each_bundle_s_batch_stage() {
    let snapshot = snapshot()
        .bundle(0, DelegationPhase::Confirmed)
        .bundle(1, DelegationPhase::Proved)
        .bundle(2, DelegationPhase::Proved)
        .proxy_batch(0, ProxyBatchProgress::Landed)
        .proxy_batch(1, ProxyBatchProgress::InFlight { refused: true })
        .proxy_batch(2, ProxyBatchProgress::InFlight { refused: false })
        .build();

    let plan = plan_from_snapshot(&snapshot, &[1]).unwrap();

    assert_eq!(
        plan.proxy_batches,
        vec![
            ProxyBatchPlanStatus {
                bundle_index: 0,
                stage: ProxyBatchStage::Landed,
            },
            ProxyBatchPlanStatus {
                bundle_index: 1,
                stage: ProxyBatchStage::Refused,
            },
            ProxyBatchPlanStatus {
                bundle_index: 2,
                stage: ProxyBatchStage::Dispatched,
            },
        ]
    );
}

/// The snapshot reads a batch's stage and whether its first attempt still
/// needs the registration signature from storage in its one transaction.
#[test]
fn the_snapshot_reads_each_batch_s_submission_state() {
    use crate::chain_submission::tests::scripted_chain::sign_attempt;
    use crate::proxy::tests::submission_fixtures::{
        proved_batch, FixtureAnchor, ROUND_ID, WALLET_ID,
    };
    use crate::round_planning::load_round_snapshot;

    let fixture = proved_batch(FixtureAnchor::Registration);
    let db = &fixture.db;
    let read = || {
        db.read_transaction("test", |tx| load_round_snapshot(tx, WALLET_ID, ROUND_ID))
            .unwrap()
            .proxy_batches[&0]
    };
    assert_eq!(
        read(),
        ProxyBatchStatus {
            needs_registration_signature: true,
            ..status(ProxyBatchProgress::Proved)
        }
    );

    sign_attempt(&fixture, 1_000, false);
    assert_eq!(read(), status(ProxyBatchProgress::Proved));

    db.conn()
        .execute(
            "INSERT INTO chain_submissions
                 (identity_key, round_id, wallet_id, network, bundle_index, kind,
                  ordered_batch_digest, generation_digest, state, diagnostic_kind, diagnostic,
                  created_at, updated_at)
             SELECT randomblob(40), round_id, wallet_id, 'regtest', 0, 'proxy_delegation_batch',
                    batch_digest, zeroblob(32), 'recovering', 'proxy_batch_paused', 'paused', 1, 1
               FROM proxy_batch_attempts",
            [],
        )
        .unwrap();
    assert_eq!(
        read(),
        status(ProxyBatchProgress::InFlight { refused: true })
    );
    for (state, kind, progress) in [
        (
            "recovering",
            "ambiguous_dispatch",
            ProxyBatchProgress::InFlight { refused: false },
        ),
        (
            "recovering",
            "chain_rejected",
            ProxyBatchProgress::InFlight { refused: true },
        ),
    ] {
        db.conn()
            .execute(
                "UPDATE chain_submissions SET state = ?1, diagnostic_kind = ?2",
                [state, kind],
            )
            .unwrap();
        assert_eq!(read().progress, progress, "{state} {kind}");
    }
}
