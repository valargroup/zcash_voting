//! The benchmark rejects helper schedules that do not model Vizor.

use stage_bench::run_config::ShareIdentity;
use stage_bench::share_schedule::validate_vizor_schedule;
use zcash_voting::types::ShareDelegationRecord;

fn share(share_index: u32, submit_at: u64, created_at: u64) -> ShareDelegationRecord {
    ShareDelegationRecord {
        round_id: "round".to_string(),
        bundle_index: 2,
        proposal_id: 1,
        share_index,
        sent_to_urls: vec!["https://helper.example".to_string()],
        ambiguous_urls: Vec::new(),
        attempting_urls: Vec::new(),
        target_count: 1,
        nullifier: vec![share_index as u8; 32],
        confirmed: false,
        submit_at,
        created_at,
    }
}

fn designated() -> ShareIdentity {
    ShareIdentity {
        bundle_index: 2,
        proposal_id: 1,
        share_index: 0,
    }
}

#[test]
fn one_designated_submission_and_a_passive_tail_match_vizor() {
    let shares = vec![
        share(0, 0, 1_000),
        share(1, 1_100, 1_000),
        share(2, 1_200, 1_000),
        share(3, 1_300, 1_000),
    ];

    let schedule =
        validate_vizor_schedule(&shares, Some(designated()), 1_050, 100).expect("a Vizor schedule");

    assert_eq!(schedule.total_shares, 4);
    assert_eq!(schedule.designated_immediate_shares, 1);
    assert_eq!(schedule.submit_at_zero_shares, 1);
    assert_eq!(schedule.passive_shares, 3);
    assert_eq!(schedule.future_shares, 3);
    assert_eq!(schedule.due_within_tracking_budget, 1);
    assert_eq!(schedule.earliest_submit_at_seconds, Some(1_100));
    assert_eq!(schedule.p50_delay_seconds, 200);
    assert_eq!(schedule.p95_delay_seconds, 200);
    assert_eq!(schedule.max_delay_seconds, 300);
}

#[test]
fn an_immediate_non_designated_share_is_rejected() {
    let shares = vec![share(0, 0, 1_000), share(1, 0, 1_000)];

    let error = validate_vizor_schedule(&shares, Some(designated()), 1_000, 100)
        .expect_err("an all-immediate plan must not be benchmarked as Vizor");

    assert!(error
        .to_string()
        .contains("non-designated share (2, 1, 1) was scheduled for immediate submission"));
}

#[test]
fn a_missing_designation_is_rejected() {
    let shares = vec![share(0, 0, 1_000), share(1, 1_100, 1_000)];

    let error = validate_vizor_schedule(&shares, None, 1_000, 100)
        .expect_err("the immutable designation is required");

    assert!(error.to_string().contains("no designated immediate share"));
}

#[test]
fn a_schedule_whose_passive_tail_has_already_elapsed_is_rejected() {
    let shares = vec![share(0, 0, 1_000), share(1, 1_001, 1_000)];

    let error = validate_vizor_schedule(&shares, Some(designated()), 1_002, 100)
        .expect_err("the benchmark must still expose passive work");

    assert!(error
        .to_string()
        .contains("every helper submission was already due"));
}
