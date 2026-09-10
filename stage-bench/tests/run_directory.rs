//! The run directory as the interface it is.
//!
//! A finished run is read back by `analyze`, by a later comparison, and by a
//! person. Everything it holds therefore has to survive a round trip through
//! disk without the workload and the numbers drifting apart.

use std::path::PathBuf;

use recovery_conformance::helper_fleet::HelperFleetPlan;
use recovery_conformance::run_config::Endpoints;
use stage_bench::ballot::Ballot;
use stage_bench::metrics::{render, Metrics};
use stage_bench::run_config::{BenchOutcome, BenchRunConfig, TrackingSummary};
use stage_bench::Manifest;

fn scratch(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "stage-bench-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    directory
}

fn config(run_dir: &std::path::Path) -> BenchRunConfig {
    BenchRunConfig {
        sidecar: run_dir.join("sidecar.db"),
        wallet_db: PathBuf::from("/tmp/voter.db"),
        warm_pir_from: Some(PathBuf::from("/tmp/pir-warm.db")),
        round_id: "0123456789abcdef".to_string(),
        account_uuid: "8b29d4e6-7940-4570-b2c2-3c7a25ba6922".to_string(),
        endpoints: Endpoints {
            chain_rpc: "https://stage.vote-rpc-primary.valargroup.org".to_string(),
            vote_servers: vec!["https://stage.vote-chain-primary.valargroup.org".to_string()],
            pir_urls: vec!["https://stage.pir.valargroup.org".to_string()],
            helper_urls: vec!["https://stage.vote-chain-primary.valargroup.org".to_string()],
            lightwalletd: "https://testnet.zec.rocks:443".to_string(),
        },
        ballot: Ballot::synthetic(37, &[2, 3, 4]).expect("a benchmark ballot"),
        fleet: HelperFleetPlan::none(),
        vote_end_time_seconds: 1_800_000_000,
        bundle_concurrency: 3,
        proof_concurrency: 3,
        chain_repoll_milliseconds: 2000,
        tracking_budget_seconds: 30 * 60,
        confirm_mode: stage_bench::run_config::ConfirmMode::Immediate,
        confirm_concurrency: 8,
        max_dispatches: 8_192,
        max_records: 262_144,
        run_dir: run_dir.to_path_buf(),
    }
}

fn outcome() -> BenchOutcome {
    BenchOutcome {
        quiescence: "BackgroundShareWorkOnly { shares: 3 }".to_string(),
        quiescence_kind: "BackgroundShareWorkOnly".to_string(),
        failures: Vec::new(),
        notes: 11,
        bundles: 3,
        proposals: 37,
        immediate_share: Some(stage_bench::run_config::ShareIdentity {
            bundle_index: 2,
            proposal_id: 1,
            share_index: 0,
        }),
        completed_proposals: 37,
        tracking: vec![TrackingSummary {
            quiescence: "NothingToTrack".to_string(),
            passes: 2,
            confirmed: 1_776,
            ..TrackingSummary::default()
        }],
        round_drive_seconds: 402.5,
        tracking_seconds: 51.0,
    }
}

#[test]
fn a_run_configuration_survives_the_file_it_is_passed_through() {
    let run_dir = scratch("config");
    let original = config(&run_dir);

    let path = BenchRunConfig::path_in(&run_dir);
    original.write(&path).expect("writing the configuration");
    let read = BenchRunConfig::read(&path).expect("reading it back");

    assert_eq!(read.round_id, original.round_id);
    assert_eq!(read.ballot, original.ballot);
    assert_eq!(read.ballot.len(), 37);
    assert_eq!(read.endpoints.vote_servers, original.endpoints.vote_servers);
    assert_eq!(read.vote_end_time_seconds, original.vote_end_time_seconds);
    assert_eq!(read.max_records, original.max_records);

    let _ = std::fs::remove_dir_all(&run_dir);
}

/// The configuration file carries nothing a `ps` listing must not show.
///
/// Credentials reach the worker through the inherited environment. If a seed,
/// a mnemonic, or a signing key ever appeared here it would be written to disk
/// on every run and kept in the run directory indefinitely.
#[test]
fn a_run_configuration_holds_no_secret() {
    // Not named for what it checks: the directory path is inside the file, so a
    // scratch directory called "secrets" would fail this test on its own name.
    let run_dir = scratch("redaction");
    let path = BenchRunConfig::path_in(&run_dir);
    config(&run_dir).write(&path).expect("writing it");

    let raw = std::fs::read_to_string(&path).expect("reading it");
    for forbidden in ["mnemonic", "seed", "secret", "hotkey", "VOTE_SDK_VOTER"] {
        assert!(
            !raw.to_lowercase().contains(&forbidden.to_lowercase()),
            "the run configuration mentions {forbidden}"
        );
    }

    let _ = std::fs::remove_dir_all(&run_dir);
}

#[test]
fn an_outcome_names_the_states_a_finished_round_may_end_in() {
    let mut outcome = outcome();
    assert!(outcome.is_complete());

    outcome.quiescence_kind = "NoWorkLeft".to_string();
    assert!(outcome.is_complete());

    // Every other quiescence needs the host to act or names a fault, and a
    // benchmark over one of those timed a round that did not finish.
    outcome.quiescence_kind = "PassBudgetExhausted".to_string();
    assert!(!outcome.is_complete());

    outcome.quiescence_kind = "NoWorkLeft".to_string();
    outcome
        .failures
        .push(stage_bench::run_config::FailureRecord {
            step: None,
            bundle_index: Some(0),
            kind: "HelperDeliveryIncomplete".to_string(),
            message: "a helper never accepted".to_string(),
        });
    assert!(!outcome.is_complete());
}

#[test]
fn a_manifest_records_the_workload_beside_the_numbers() {
    let run_dir = scratch("manifest");
    let config = config(&run_dir);
    let manifest = Manifest::build(&config, &outcome(), 1_700_000_000, 21_600);
    manifest.write(&run_dir).expect("writing the manifest");

    let read = Manifest::read(&run_dir).expect("reading it back");
    assert_eq!(
        read.confirm_mode, "immediate",
        "the wallet's behaviour: one designated share, not the whole tail"
    );
    assert_eq!(read.tracking_budget_seconds, 30 * 60);
    assert_eq!(read.proposals, 37);
    assert_eq!(read.bundles, 3);
    assert_eq!(read.ballot.len(), 37);
    assert_eq!(read.configured_helpers, 1);
    assert!(!read.synthetic_fleet);
    assert_eq!(read.vote_window_seconds, 21_600);
    assert!(read.warm_pir);
    assert_eq!(read.quiescence_kind, "BackgroundShareWorkOnly");
    assert_eq!(read.completed_proposals, 37);
    // A debug build's proving times measure the compiler, so which profile
    // produced a number is part of the number.
    assert!(read.profile == "debug" || read.profile == "release");

    let _ = std::fs::remove_dir_all(&run_dir);
}

/// A finished directory renders without needing anything else.
///
/// This is what `analyze` does. The table is built from the manifest and the
/// snapshots alone, so a run archived weeks ago still reports.
#[test]
fn a_finished_run_directory_renders_its_report() {
    let run_dir = scratch("render");
    let config = config(&run_dir);
    let manifest = Manifest::build(&config, &outcome(), 1_700_000_000, 21_600);
    manifest.write(&run_dir).expect("writing the manifest");

    let snapshot = serde_json::json!({
        "operation": "round::run",
        "started_at_unix_us": 1_700_000_000_000_000u64,
        "round_id": "0123456789abcdef",
        "elapsed_us": 402_500_000u64,
        "outcome": "succeeded",
        "records": [
            {
                "id": 1, "parent_id": null, "stage": "helper::active_delivery",
                "attribution": { "bundle_index": 0, "proposal_id": 1, "share_index": 0 },
                "started_after_us": 0, "elapsed_us": 250_000, "outcome": "succeeded",
                "error_kind": null, "http_status": null, "endpoint_index": 0, "attempt": null
            },
            {
                "id": 2, "parent_id": 1, "stage": "helper.http.post_json",
                "attribution": { "bundle_index": 0, "proposal_id": 1, "share_index": 0 },
                "started_after_us": 10_000, "elapsed_us": 200_000, "outcome": "succeeded",
                "error_kind": null, "http_status": 200, "endpoint_index": 0, "attempt": 1
            }
        ],
        "summaries": [],
        "records_dropped": 0,
        "summary_updates_dropped": 0,
        "active_stages_dropped": 0
    });
    std::fs::write(
        run_dir.join("round.observability.json"),
        serde_json::to_vec(&snapshot).expect("encoding the snapshot"),
    )
    .expect("writing the snapshot");

    let snapshots = stage_bench::read_snapshots(&run_dir).expect("reading the snapshots");
    assert_eq!(snapshots.len(), 1);

    let metrics = Metrics::derive(&snapshots, &[]);
    assert!(metrics.complete);
    assert_eq!(metrics.delivery.active_shares.peak, 1);
    assert_eq!(metrics.delivery.initial_http.samples, 1);
    assert_eq!(metrics.delivery.http_status.get(&200), Some(&1));

    let table = render(&manifest, &metrics);
    assert!(table.contains("0123456789abcdef"));
    assert!(table.contains("helper::active_delivery"));
    assert!(table.contains("37 proposals x 3 bundles"));
    assert!(table.contains("3 bundles and 3 proofs wide"));
    assert!(!table.contains("INCOMPLETE CAPTURE"));

    let _ = std::fs::remove_dir_all(&run_dir);
}

/// A capped capture says so where the numbers are read, not only in the JSON.
#[test]
fn an_incomplete_capture_is_announced_in_the_table() {
    let run_dir = scratch("incomplete");
    let config = config(&run_dir);
    let manifest = Manifest::build(&config, &outcome(), 1_700_000_000, 21_600);

    let snapshot = serde_json::json!({
        "operation": "round::run",
        "started_at_unix_us": 0u64,
        "round_id": null,
        "elapsed_us": 1u64,
        "outcome": "succeeded",
        "records": [],
        "summaries": [],
        "records_dropped": 12,
        "summary_updates_dropped": 0,
        "active_stages_dropped": 3
    });
    let captured = stage_bench::CapturedSnapshot {
        source: "round.observability.json".to_string(),
        snapshot: serde_json::from_value(snapshot).expect("a decodable snapshot"),
    };
    let metrics = Metrics::derive(&[captured], &[]);

    assert!(!metrics.complete);
    let table = render(&manifest, &metrics);
    assert!(table.contains("INCOMPLETE CAPTURE"));
    assert!(table.contains("12 records"));
    assert!(table.contains("3 stage starts"));

    let _ = std::fs::remove_dir_all(&run_dir);
}

/// The concurrent confirmation mode writes one array, not a file per share.
///
/// Each focused confirmation freezes its own report and their record ids are
/// invocation-local, so they cannot be merged into one snapshot. Reading them
/// back as separate captures is what keeps the timeline honest.
#[test]
fn confirmation_snapshots_expand_from_their_array() {
    let run_dir = scratch("confirm");

    let one = |anchor: u64| {
        serde_json::json!({
            "operation": "confirm_pending_share",
            "started_at_unix_us": anchor,
            "round_id": "r",
            "elapsed_us": 1_000u64,
            "outcome": "succeeded",
            "records": [{
                "id": 1, "parent_id": null, "stage": "helper::share_status",
                "attribution": { "bundle_index": 0, "proposal_id": 1, "share_index": 0 },
                "started_after_us": 0, "elapsed_us": 900, "outcome": "succeeded",
                "error_kind": null, "http_status": null, "endpoint_index": 0, "attempt": null
            }],
            "summaries": [], "records_dropped": 0,
            "summary_updates_dropped": 0, "active_stages_dropped": 0
        })
    };
    std::fs::write(
        run_dir.join(stage_bench::CONFIRM_SNAPSHOTS),
        serde_json::to_vec(&serde_json::json!([one(1_000), one(2_000), one(3_000)]))
            .expect("encoding the array"),
    )
    .expect("writing the array");

    let snapshots = stage_bench::read_snapshots(&run_dir).expect("reading them back");
    assert_eq!(snapshots.len(), 3);
    assert!(snapshots[0]
        .source
        .starts_with(stage_bench::CONFIRM_SNAPSHOTS));
    assert_eq!(
        snapshots[2].source,
        format!("{}#2", stage_bench::CONFIRM_SNAPSHOTS)
    );

    let metrics = Metrics::derive(&snapshots, &[]);
    let stage = metrics
        .stage("helper::share_status")
        .expect("the status stage");
    assert_eq!(stage.calls, 3);
    // Anchored a microsecond apart each, so they do not collapse onto one instant.
    assert_eq!(stage.wall_span_us, 2_900);

    let _ = std::fs::remove_dir_all(&run_dir);
}

/// The default is what a wallet does, and the report says which mode ran.
///
/// A round designates one immediate share; confirming it is what decides
/// whether a vote reads as cast. Chasing the whole tail is an experiment, and a
/// reader must not have to infer which of the two produced a number.
#[test]
fn the_default_confirmation_mode_is_the_wallets_and_is_named_in_the_report() {
    use stage_bench::run_config::ConfirmMode;

    assert_eq!(ConfirmMode::default(), ConfirmMode::Immediate);
    assert!(ConfirmMode::Immediate.is_shipped_behaviour());
    assert!(!ConfirmMode::All.is_shipped_behaviour());
    assert!(!ConfirmMode::Concurrent.is_shipped_behaviour());

    let run_dir = scratch("mode");
    let mut config = config(&run_dir);
    let manifest = Manifest::build(&config, &outcome(), 1_700_000_000, 21_600);
    let table = render(&manifest, &Metrics::derive(&[], &[]));
    assert!(table.contains("as a wallet does"));
    assert!(!table.contains("EXPERIMENT"));

    config.confirm_mode = ConfirmMode::Concurrent;
    let manifest = Manifest::build(&config, &outcome(), 1_700_000_000, 21_600);
    let table = render(&manifest, &Metrics::derive(&[], &[]));
    assert!(table.contains("EXPERIMENT"));
    assert!(table.contains("concurrent"));

    let _ = std::fs::remove_dir_all(&run_dir);
}

/// The report states how much of the round preceded the share a voter waits on.
///
/// This is the falsifiable check on immediate-share dispatch: the designated
/// share should be first, and a run where it is not should say so in numbers
/// rather than leaving it to be recomputed by hand.
#[test]
fn the_report_ranks_the_designated_share_against_the_rest_of_the_round() {
    // A share is ranked by the POST it actually made, under its workflow.
    let share = |id: u64, bundle: u32, proposal: u32, index: u32, start: u64| {
        let attribution = serde_json::json!({
            "bundle_index": bundle, "proposal_id": proposal, "share_index": index
        });
        vec![
            serde_json::json!({
                "id": id, "parent_id": null, "stage": "helper::active_delivery",
                "attribution": attribution, "started_after_us": start,
                "elapsed_us": 1_000, "outcome": "succeeded", "error_kind": null,
                "http_status": null, "endpoint_index": 0, "attempt": null
            }),
            serde_json::json!({
                "id": id + 1_000, "parent_id": id, "stage": "helper.http.post_json",
                "attribution": attribution, "started_after_us": start,
                "elapsed_us": 500, "outcome": "succeeded", "error_kind": null,
                "http_status": 200, "endpoint_index": 0, "attempt": 1
            }),
        ]
    };
    let captured = |records: Vec<serde_json::Value>| stage_bench::CapturedSnapshot {
        source: "round.observability.json".to_string(),
        snapshot: serde_json::from_value(serde_json::json!({
            "operation": "run", "started_at_unix_us": 0u64, "round_id": "r",
            "elapsed_us": 10_000u64, "outcome": "succeeded", "records": records,
            "summaries": [], "records_dropped": 0,
            "summary_updates_dropped": 0, "active_stages_dropped": 0
        }))
        .expect("a decodable snapshot"),
    };

    // Designated first: nothing preceded it.
    let mut records = share(1, 2, 1, 0, 0);
    records.extend(share(2, 0, 1, 0, 100));
    records.extend(share(3, 1, 1, 0, 200));
    let metrics = Metrics::derive_for(&[captured(records)], &[], Some((2, 1, 0)));
    let ranked = metrics.immediate_dispatch.expect("a ranked designation");
    assert_eq!(ranked.shares_dispatched_before, 0);
    assert_eq!(ranked.shares_total, 3);
    assert!(ranked.dispatched_after_first_seconds.abs() < f64::EPSILON);

    // Dispatched last, as it was before delivery ordered it.
    let mut records = share(1, 0, 1, 0, 0);
    records.extend(share(2, 1, 1, 0, 100));
    records.extend(share(3, 2, 1, 0, 500_000));
    let metrics = Metrics::derive_for(&[captured(records)], &[], Some((2, 1, 0)));
    let ranked = metrics.immediate_dispatch.expect("a ranked designation");
    assert_eq!(ranked.shares_dispatched_before, 2);
    assert!((ranked.dispatched_after_first_seconds - 0.5).abs() < 1e-9);

    // A run that recorded no designation ranks nothing rather than guessing.
    let metrics = Metrics::derive_for(&[captured(share(1, 0, 1, 0, 0))], &[], None);
    assert!(metrics.immediate_dispatch.is_none());

    // A truncated capture may simply be missing the POST that came first, so no
    // ordering verdict is printed from it.
    let mut records = share(1, 2, 1, 0, 0);
    records.extend(share(2, 0, 1, 0, 100));
    let truncated = stage_bench::CapturedSnapshot {
        source: "round.observability.json".to_string(),
        snapshot: serde_json::from_value(serde_json::json!({
            "operation": "run", "started_at_unix_us": 0u64, "round_id": "r",
            "elapsed_us": 10_000u64, "outcome": "succeeded", "records": records,
            "summaries": [], "records_dropped": 12,
            "summary_updates_dropped": 0, "active_stages_dropped": 0
        }))
        .expect("a decodable snapshot"),
    };
    let metrics = Metrics::derive_for(&[truncated], &[], Some((2, 1, 0)));
    assert!(!metrics.complete);
    let ranked = metrics.immediate_dispatch.expect("a ranked designation");
    assert_eq!(ranked.shares_dispatched_before, 0);
    let run_dir = scratch("indeterminate");
    let manifest = Manifest::build(&config(&run_dir), &outcome(), 1_700_000_000, 21_600);
    let table = render(&manifest, &metrics);
    assert!(
        table.contains("INDETERMINATE: capture incomplete"),
        "a truncated capture must not yield an ordering verdict"
    );
    assert!(!table.contains("first, as intended"));
    let _ = std::fs::remove_dir_all(&run_dir);

    // Two POSTs in the same truncated microsecond are not evidence of order.
    // Nothing "preceded" the designated share, but claiming it went first would
    // assert a sequence the data does not contain.
    let mut records = share(1, 2, 1, 0, 400);
    records.extend(share(2, 0, 1, 0, 400));
    records.extend(share(3, 1, 1, 0, 900));
    let metrics = Metrics::derive_for(&[captured(records)], &[], Some((2, 1, 0)));
    let ranked = metrics.immediate_dispatch.expect("a ranked designation");
    assert_eq!(ranked.shares_dispatched_before, 0);
    assert_eq!(
        ranked.shares_dispatched_same_microsecond, 1,
        "a tie is reported rather than broken"
    );
}

/// A run whose worker died before writing an outcome still has a directory.
#[test]
fn snapshots_absent_from_a_directory_are_not_an_error() {
    let run_dir = scratch("empty");
    assert!(stage_bench::read_snapshots(&run_dir)
        .expect("an empty directory reads")
        .is_empty());
    let _ = std::fs::remove_dir_all(&run_dir);
}

/// A corrupted run must not analyse as a valid one.
///
/// An absent outcome is a run that never got that far and analyses fine. A
/// present but unreadable outcome is different: reporting it as "no designation"
/// would drop the dispatch measurement while still printing a complete-looking
/// analysis, which is the failure this crate exists to avoid.
#[test]
fn a_malformed_outcome_is_reported_rather_than_read_as_no_designation() {
    let run_dir = scratch("malformed");
    let manifest = Manifest::build(&config(&run_dir), &outcome(), 1_700_000_000, 21_600);
    manifest.write(&run_dir).expect("writing the manifest");

    // Absent: analysable.
    assert!(!BenchOutcome::path_in(&run_dir).exists());
    assert!(BenchOutcome::read(&BenchOutcome::path_in(&run_dir)).is_err());

    // Truncated mid-object, as an interrupted write leaves it.
    std::fs::write(
        BenchOutcome::path_in(&run_dir),
        b"{\"quiescence\": \"NoWorkL",
    )
    .expect("writing a truncated outcome");
    let read = BenchOutcome::read(&BenchOutcome::path_in(&run_dir));
    assert!(
        read.is_err(),
        "a truncated outcome must not deserialize into a default"
    );

    // A complete outcome from before the field existed still reads, through the
    // field's serde default, and reports no designation rather than failing.
    std::fs::write(
        BenchOutcome::path_in(&run_dir),
        serde_json::to_vec(&serde_json::json!({
            "quiescence": "NoWorkLeft", "quiescence_kind": "NoWorkLeft",
            "failures": [], "notes": 11, "bundles": 3, "proposals": 37,
            "completed_proposals": 37, "tracking": [],
            "round_drive_seconds": 1.0, "tracking_seconds": 1.0
        }))
        .expect("encoding an older outcome"),
    )
    .expect("writing an older outcome");
    let older = BenchOutcome::read(&BenchOutcome::path_in(&run_dir)).expect("an older outcome");
    assert!(older.immediate_share.is_none());

    let _ = std::fs::remove_dir_all(&run_dir);
}

/// A share that never reached the transport is not a dispatch.
///
/// A delivery workflow opens its stage before it POSTs, so ranking admitted
/// workflows would count a share that emitted no request — and could rank the
/// designated share first when it was never sent, which is precisely the claim
/// the metric exists to make.
#[test]
fn the_rank_counts_only_shares_that_actually_posted() {
    let workflow = |id: u64, bundle: u32, proposal: u32, share: u32, start: u64| {
        serde_json::json!({
            "id": id, "parent_id": null, "stage": "helper::active_delivery",
            "attribution": {
                "bundle_index": bundle, "proposal_id": proposal, "share_index": share
            },
            "started_after_us": start, "elapsed_us": 10, "outcome": "succeeded",
            "error_kind": null, "http_status": null, "endpoint_index": 0, "attempt": null
        })
    };
    let post = |id: u64, parent: u64, bundle: u32, proposal: u32, share: u32, start: u64| {
        serde_json::json!({
            "id": id, "parent_id": parent, "stage": "helper.http.post_json",
            "attribution": {
                "bundle_index": bundle, "proposal_id": proposal, "share_index": share
            },
            "started_after_us": start, "elapsed_us": 5, "outcome": "succeeded",
            "error_kind": null, "http_status": 200, "endpoint_index": 0, "attempt": 1
        })
    };
    let captured = |records: Vec<serde_json::Value>| stage_bench::CapturedSnapshot {
        source: "round.observability.json".to_string(),
        snapshot: serde_json::from_value(serde_json::json!({
            "operation": "run", "started_at_unix_us": 0u64, "round_id": "r",
            "elapsed_us": 1_000u64, "outcome": "succeeded", "records": records,
            "summaries": [], "records_dropped": 0,
            "summary_updates_dropped": 0, "active_stages_dropped": 0
        }))
        .expect("a decodable snapshot"),
    };

    // The designated workflow is admitted first but never POSTs; two others do.
    let never_sent = captured(vec![
        workflow(1, 2, 1, 0, 0),
        workflow(2, 0, 1, 0, 100),
        post(3, 2, 0, 1, 0, 110),
        workflow(4, 1, 1, 0, 200),
        post(5, 4, 1, 1, 0, 210),
    ]);
    let metrics = Metrics::derive_for(&[never_sent], &[], Some((2, 1, 0)));
    assert!(
        metrics.immediate_dispatch.is_none(),
        "a designated share that never POSTed must not be ranked first"
    );

    // With its own POST it ranks, and only POSTing shares are counted.
    let sent = captured(vec![
        workflow(1, 2, 1, 0, 0),
        post(2, 1, 2, 1, 0, 10),
        workflow(3, 0, 1, 0, 100),
        post(4, 3, 0, 1, 0, 110),
        // Admitted, never sent: excluded from the total.
        workflow(5, 1, 1, 0, 200),
    ]);
    let metrics = Metrics::derive_for(&[sent], &[], Some((2, 1, 0)));
    let ranked = metrics.immediate_dispatch.expect("a ranked designation");
    assert_eq!(ranked.shares_dispatched_before, 0);
    assert_eq!(
        ranked.shares_total, 2,
        "only shares that POSTed are counted"
    );
}
