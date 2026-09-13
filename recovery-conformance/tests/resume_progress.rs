//! A resumed run must prove it did work, not merely that it ended.
//!
//! The suite's re-drive policy is deliberate: `ChainRecoveryStalled` is not a
//! verdict, so the harness waits and runs again rather than reporting the SDK's
//! own retry advice as a failure. The cost of that policy is that it also
//! absorbs a run which stalled because it did *nothing* — and it did, for a
//! real regression, where the first resumed process normalized an abandoned
//! reservation to `Recovering` and returned without any chain check. The second
//! process resolved it and the matrix reported a pass.
//!
//! These are negative oracles for the rule that closes it. Every one of them
//! must fail if the assertion it exercises is weakened, because an assertion
//! that cannot fail is the thing this whole crate exists to avoid.

use recovery_conformance::assertions::{
    assert_a_background_resume_ran_a_pass, assert_a_stall_attempted_recovery,
};
use recovery_conformance::chain_reads::ChainReadLedger;
use recovery_conformance::run_config::{FailureRecord, RunOutcome, ShareTrackingSummary};
use recovery_conformance::stall::StallTarget;

/// A run that ended on a stalled chain recovery at `step`.
fn stalled(step: &str, stalled_step_chain_reads: usize, chain_reads: usize) -> RunOutcome {
    RunOutcome {
        quiescence: format!("ChainRecoveryStalled {{ step: {step}, .. }}"),
        quiescence_kind: "ChainRecoveryStalled".to_string(),
        stalled_step_chain_reads,
        chain_reads,
        ..RunOutcome::default()
    }
}

#[test]
fn a_stall_with_no_chain_read_for_its_step_is_a_finding() {
    let error = assert_a_stall_attempted_recovery(&stalled(
        "AdvanceVoteBatch { bundle_index: 0, proposal_id: 1 }",
        0,
        0,
    ))
    .expect_err("a stall that asked the chain nothing must not be re-driven");
    let message = format!("{error:#}");
    assert!(message.contains("A6 VIOLATED"), "{message}");
    // The step has to be in the message. A matrix that fails without naming
    // which submission stalled sends the reader back to the log to find out.
    assert!(message.contains("AdvanceVoteBatch"), "{message}");
}

#[test]
fn a_busy_round_cannot_excuse_a_step_that_never_looked() {
    // The reason the count is per step rather than per run. A resumed round
    // drives its other bundles to completion, so a round-wide total is never
    // zero and would cover for exactly the target under test.
    let error = assert_a_stall_attempted_recovery(&stalled(
        "AdvanceVoteBatch { bundle_index: 0, proposal_id: 1 }",
        0,
        41,
    ))
    .expect_err("other bundles' traffic is not evidence about this step");
    assert!(format!("{error:#}").contains("41 elsewhere"));
}

#[test]
fn a_stall_that_scanned_and_found_nothing_is_re_drivable() {
    // The legal shape, and the one the policy exists for: the exact-tree scan
    // ran, the transaction is simply not mined yet, and running again later
    // resolves it.
    assert_a_stall_attempted_recovery(&stalled(
        "AdvanceVoteBatch { bundle_index: 0, proposal_id: 1 }",
        2,
        2,
    ))
    .expect("a recovery that looked and found nothing is not a finding");
}

#[test]
fn only_a_stall_is_held_to_the_read_rule() {
    // Every other ending either finished the round or names a failure the run
    // recorded. Applying the rule to them would fail runs for traffic they had
    // no reason to make.
    for kind in [
        "NoWorkLeft",
        "BackgroundShareWorkOnly",
        "ChainTerminal",
        "Failures",
        "NeedsBallot",
        "Cancelled",
        "PassBudgetExhausted",
    ] {
        let outcome = RunOutcome {
            quiescence: kind.to_string(),
            quiescence_kind: kind.to_string(),
            ..RunOutcome::default()
        };
        assert_a_stall_attempted_recovery(&outcome)
            .unwrap_or_else(|error| panic!("{kind} must not be judged as a stall: {error:#}"));
    }
}

/// A run whose foreground finished and whose last tracking phase expired.
fn budget_expired(passes: u32) -> RunOutcome {
    RunOutcome {
        quiescence: "BackgroundShareWorkOnly".to_string(),
        quiescence_kind: "BackgroundShareWorkOnly".to_string(),
        share_tracking: vec![ShareTrackingSummary {
            quiescence: "SuiteBudgetExpired".to_string(),
            passes,
            ..ShareTrackingSummary::default()
        }],
        ..RunOutcome::default()
    }
}

#[test]
fn a_tracking_phase_that_expired_without_a_pass_is_wedged_not_slow() {
    assert!(budget_expired(0).needs_background_recovery());
    let error = assert_a_background_resume_ran_a_pass(&budget_expired(0))
        .expect_err("a phase that completed no pass will not finish by being repeated");
    assert!(format!("{error:#}").contains("A6 VIOLATED"));
    assert_a_background_resume_ran_a_pass(&budget_expired(3))
        .expect("a phase that was making progress may be resumed");
}

#[test]
fn a_helper_resume_is_not_held_to_the_read_rule() {
    // Deliberately exempt, and the reason belongs in a test rather than only a
    // doc comment: `needs_helper_recovery` already requires a failure the run
    // recorded, which a run that attempted no delivery cannot produce. There is
    // no silent-zero-work shape to catch.
    let outcome = RunOutcome {
        quiescence: "Failures".to_string(),
        quiescence_kind: "Failures".to_string(),
        failures: vec![FailureRecord {
            step: Some("CastVote { bundle_index: 2, proposal_id: 1, choice: 0 }".to_string()),
            bundle_index: Some(2),
            kind: "HelperDeliveryIncomplete".to_string(),
            message: "helper delivery ended with pending shares".to_string(),
        }],
        ..RunOutcome::default()
    };
    assert!(outcome.needs_helper_recovery());
    assert_a_stall_attempted_recovery(&outcome).expect("not a stall");
    assert_a_background_resume_ran_a_pass(&outcome).expect("not a background resume");
}

#[test]
fn an_outcome_written_before_the_counters_existed_still_parses() {
    // The worker and the parent are separate binaries and a stale worker is a
    // real possibility mid-development. A missing field must read as zero
    // rather than as an unparseable outcome, which the parent would report as
    // an infrastructure failure and quietly retry.
    let json = serde_json::json!({
        "quiescence": "NoWorkLeft",
        "quiescence_kind": "NoWorkLeft",
        "failures": [],
        "dispatches": 0
    });
    let outcome: RunOutcome = serde_json::from_value(json).expect("an older outcome must parse");
    assert_eq!(outcome.stalled_step_chain_reads, 0);
    assert_eq!(outcome.chain_reads, 0);
}

#[test]
fn only_chain_reads_are_counted() {
    let ledger = ChainReadLedger::new();
    // A POST is progress, but `dispatches` records it. Counting it here would
    // let a retransmission excuse a missing scan.
    for ignored in [
        None,
        Some(StallTarget::DelegateAndCastPost),
        Some(StallTarget::VotePost),
        Some(StallTarget::DelegationPost),
        Some(StallTarget::SharePost),
        Some(StallTarget::HelperPreflight),
        Some(StallTarget::ShareStatus),
        Some(StallTarget::PirQuery),
    ] {
        ledger.record(ignored);
    }
    assert_eq!(ledger.total(), 0);

    ledger.record(Some(StallTarget::TransactionLookup));
    ledger.record(Some(StallTarget::CommitmentTreeRead));
    assert_eq!(ledger.total(), 2);
}

#[test]
fn reads_are_attributed_to_the_step_that_held_the_route() {
    use zcash_voting::session::NextStep;

    let target = NextStep::AdvanceVoteBatch {
        bundle_index: 0,
        proposal_id: 1,
    };
    let other = NextStep::Delegate { bundle_index: 1 };

    let ledger = ChainReadLedger::new();
    // Planning reads before any step is selected must not be filed under the
    // step that happens to run first.
    ledger.record(Some(StallTarget::CommitmentTreeRead));

    ledger.select(&target);
    // The target is selected and reads nothing: the regression's shape.
    ledger.select(&other);
    for _ in 0..5 {
        ledger.record(Some(StallTarget::TransactionLookup));
    }

    assert_eq!(ledger.reads_for(&target), 0);
    assert_eq!(ledger.reads_for(&other), 5);
    assert_eq!(ledger.unattributed(), 1);
    assert_eq!(ledger.total(), 6);
}

#[test]
fn a_step_that_never_ran_is_not_confused_with_one_that_read_nothing() {
    use zcash_voting::session::NextStep;

    let ledger = ChainReadLedger::new();
    let never = NextStep::Delegate { bundle_index: 7 };
    // Both report zero, which is correct: the assertion's claim is about the
    // run having asked, and a step that was never selected asked nothing
    // either. Pinned so a future "unknown step" sentinel does not silently
    // turn a missing selection into a pass.
    assert_eq!(ledger.reads_for(&never), 0);
}
