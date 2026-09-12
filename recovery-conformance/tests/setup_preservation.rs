//! Negative oracles for the delegation-setup comparison.
//!
//! An assertion that has never been observed failing is not yet evidence, and
//! this one guards a value whose loss is unrecoverable — so every rule it
//! applies is pinned here by a case that must fail, alongside the cases that
//! must pass. The two real-schema tests at the bottom matter as much: a column
//! renamed in the SDK would otherwise quietly stop being compared, and the
//! assertion would go on reporting green over ground it no longer covers.

use std::collections::{BTreeMap, BTreeSet};

use recovery_conformance::setup_preservation::{
    assert_delegation_setup_preserved, BundleSetup, BROADCAST_EVIDENCE, SETUP_COLUMNS,
};

/// A bundle whose every setup column holds `filler`, with the given evidence.
fn bundle(index: i64, filler: &str, evidence: &[&str]) -> BundleSetup {
    BundleSetup {
        wallet_id: "wallet".into(),
        round_id: "round".into(),
        bundle_index: index,
        columns: SETUP_COLUMNS
            .iter()
            .map(|column| ((*column).to_string(), Some(filler.to_string())))
            .collect(),
        evidence: evidence.iter().map(|name| (*name).to_string()).collect(),
        rejection_streak: 0,
    }
}

/// The same bundle with one column altered.
fn with_column(mut bundle: BundleSetup, column: &str, value: Option<&str>) -> BundleSetup {
    bundle.columns.insert(
        column.to_string(),
        value.map(std::string::ToString::to_string),
    );
    bundle
}

#[test]
fn identical_setup_passes_and_reports_what_it_compared() {
    let before = vec![bundle(0, "aa", &["chain_submission"])];
    let comparison = assert_delegation_setup_preserved(&before, &before.clone()).unwrap();
    assert_eq!(comparison.frozen_bundles, 1);
    assert_eq!(comparison.compared_columns, SETUP_COLUMNS.len());
    assert!(comparison.rebuilt.is_empty());
    assert!(!comparison.is_vacuous());
}

#[test]
fn a_changed_van_comm_rand_under_broadcast_evidence_fails() {
    let before = vec![bundle(0, "aa", &["chain_submission"])];
    let after = vec![with_column(
        bundle(0, "aa", &["chain_submission"]),
        "van_comm_rand",
        Some("bb"),
    )];
    let error = assert_delegation_setup_preserved(&before, &after).unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("van_comm_rand"), "{message}");
    assert!(message.contains("bundle 0"), "{message}");
    assert!(message.contains("chain_submission"), "{message}");
}

/// Every column carries the rule, not just the one that names the plan.
#[test]
fn any_frozen_setup_column_that_changes_fails() {
    for column in SETUP_COLUMNS {
        let before = vec![bundle(0, "aa", &["vote"])];
        let after = vec![with_column(bundle(0, "aa", &["vote"]), column, Some("bb"))];
        let error = assert_delegation_setup_preserved(&before, &after).unwrap_err();
        assert!(
            format!("{error:#}").contains(column),
            "{column} went unchecked"
        );
    }
}

#[test]
fn a_cleared_column_under_broadcast_evidence_fails() {
    let before = vec![bundle(0, "aa", &["delegation_tx_hash"])];
    let after = vec![with_column(
        bundle(0, "aa", &["delegation_tx_hash"]),
        "alpha",
        None,
    )];
    let error = assert_delegation_setup_preserved(&before, &after).unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("alpha"), "{message}");
    assert!(message.contains("cleared"), "{message}");
}

/// Each kind of evidence freezes a bundle on its own.
#[test]
fn every_kind_of_broadcast_evidence_freezes_setup() {
    for evidence in BROADCAST_EVIDENCE {
        let before = vec![bundle(0, "aa", &[evidence])];
        let after = vec![with_column(
            bundle(0, "aa", &[evidence]),
            "van_comm_rand",
            Some("bb"),
        )];
        assert!(
            assert_delegation_setup_preserved(&before, &after).is_err(),
            "{evidence} did not freeze setup"
        );
    }
}

#[test]
fn null_becoming_a_value_is_setup_completing_normally() {
    let before = vec![with_column(
        bundle(0, "aa", &["chain_submission"]),
        "pczt_sighash",
        None,
    )];
    let after = vec![bundle(0, "aa", &["chain_submission"])];
    let comparison = assert_delegation_setup_preserved(&before, &after).unwrap();
    // Every column but the one that was NULL is compared.
    assert_eq!(comparison.compared_columns, SETUP_COLUMNS.len() - 1);
}

/// The legal rebuild path: no evidence, so an exchange is permitted — but it is
/// recorded, because a rebuild must never happen unnoticed.
#[test]
fn a_pre_broadcast_rebuild_is_reported_rather_than_asserted() {
    let before = vec![bundle(0, "aa", &[])];
    let after = vec![with_column(
        bundle(0, "aa", &[]),
        "van_comm_rand",
        Some("bb"),
    )];
    let comparison = assert_delegation_setup_preserved(&before, &after).unwrap();
    assert_eq!(comparison.frozen_bundles, 0);
    assert_eq!(comparison.compared_columns, 0);
    assert_eq!(comparison.rebuilt, vec!["bundle 0: van_comm_rand"]);
    assert!(comparison.is_vacuous());
    assert!(format!("{comparison}").contains("pre-broadcast rebuild observed"));
}

/// Evidence only accumulates, so a bundle that gained it during the interval is
/// held to the frozen rule too. Without this, a setup change racing the row
/// that records a POST would pass.
#[test]
fn evidence_appearing_only_after_the_crash_still_freezes_setup() {
    let before = vec![bundle(0, "aa", &[])];
    let after = vec![with_column(
        bundle(0, "aa", &["chain_submission"]),
        "van_comm_rand",
        Some("bb"),
    )];
    let error = assert_delegation_setup_preserved(&before, &after).unwrap_err();
    assert!(format!("{error:#}").contains("van_comm_rand"));
}

#[test]
fn a_vanished_bundle_fails_whatever_its_evidence() {
    for evidence in [vec![], vec!["chain_submission"]] {
        let before = vec![bundle(0, "aa", &evidence), bundle(1, "aa", &evidence)];
        let after = vec![bundle(0, "aa", &evidence)];
        let error = assert_delegation_setup_preserved(&before, &after).unwrap_err();
        assert!(format!("{error:#}").contains("bundle 1"));
    }
}

/// A retirement destroys the vote recovery, the helper plans, the share rows,
/// the authorization and the submission row — and must leave setup untouched.
/// The growing streak is how the suite knows that path ran at all.
#[test]
fn a_growing_rejection_streak_counts_as_an_observed_retirement() {
    let before = vec![bundle(0, "aa", &["chain_submission"])];
    let mut after = bundle(0, "aa", &["chain_submission"]);
    after.rejection_streak = 1;
    let comparison = assert_delegation_setup_preserved(&before, &[after]).unwrap();
    assert_eq!(comparison.retired_bundles, 1);
    assert!(format!("{comparison}").contains("survived a generation retirement"));

    let mut lost = with_column(bundle(0, "aa", &["chain_submission"]), "alpha", None);
    lost.rejection_streak = 1;
    assert!(assert_delegation_setup_preserved(&before, &[lost]).is_err());
}

#[test]
fn a_comparison_that_examined_nothing_reports_itself_vacuous() {
    let comparison = assert_delegation_setup_preserved(&[], &[]).unwrap();
    assert!(comparison.is_vacuous());
    assert_eq!(comparison.compared_columns, 0);
}

// --- Real schema ------------------------------------------------------------

/// Isolated fixture files, removed even when an assertion unwinds.
struct FixtureDirectory(std::path::PathBuf);

impl FixtureDirectory {
    fn new() -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "recovery-setup-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A sidecar carrying the SDK's real current schema.
fn migrated_sidecar(directory: &FixtureDirectory) -> rusqlite::Connection {
    migrated_sidecar_at(directory.0.join("sidecar.db").to_str().unwrap())
}

/// A migrated, empty sidecar at an explicit path.
fn migrated_sidecar_at(sidecar: &str) -> rusqlite::Connection {
    // Opening through the SDK is what runs the migration ladder, so these tests
    // read the schema the wallet actually writes rather than a copy that can
    // drift away from it.
    drop(zcash_voting::round::VotingDb::open(sidecar).unwrap());
    rusqlite::Connection::open(sidecar).unwrap()
}

/// Anti-rot: a setup column renamed or dropped in the SDK must fail here rather
/// than quietly drop out of the comparison.
#[test]
fn every_named_setup_column_exists_on_the_bundles_table() {
    let directory = FixtureDirectory::new();
    let connection = migrated_sidecar(&directory);
    let mut statement = connection
        .prepare("select name from pragma_table_info('bundles')")
        .unwrap();
    let present: BTreeSet<String> = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    for column in SETUP_COLUMNS {
        assert!(
            present.contains(*column),
            "SETUP_COLUMNS names {column}, which the bundles table no longer has"
        );
    }
}

#[test]
fn read_all_fingerprints_values_distinguishes_null_and_finds_evidence() {
    let directory = FixtureDirectory::new();
    let connection = migrated_sidecar(&directory);
    connection
        .execute_batch(
            "insert into rounds (round_id, wallet_id, network, snapshot_height, ea_pk,
                                 nc_root, nullifier_imt_root, created_at)
             values ('round', 'wallet', 'testnet', 1, x'00', x'00', x'00', 0);
             insert into bundles (round_id, wallet_id, bundle_index, van_comm_rand, alpha)
             values ('round', 'wallet', 0, x'AABB', NULL);
             insert into bundles (round_id, wallet_id, bundle_index, van_comm_rand,
                                  delegation_tx_hash)
             values ('round', 'wallet', 1, x'AABB', 'hash');",
        )
        .unwrap();

    let bundles = BundleSetup::read_all(&connection).unwrap();
    assert_eq!(bundles.len(), 2);

    // A NULL column is None; a present one is a fingerprint, not the value.
    assert_eq!(bundles[0].columns["alpha"], None);
    let van = bundles[0].columns["van_comm_rand"].clone().unwrap();
    assert_eq!(van.len(), 64, "expected a hex SHA-256, got {van}");
    assert_ne!(van, "aabb", "the raw value must never be stored");

    // Equal values fingerprint equally, which is what the comparison rests on.
    assert_eq!(bundles[1].columns["van_comm_rand"], Some(van));

    assert!(!bundles[0].is_frozen(), "no evidence yet");
    assert!(bundles[1].is_frozen(), "a delegation tx hash freezes setup");
    assert!(bundles[1].evidence.contains("delegation_tx_hash"));
}

#[test]
fn a_vote_row_alone_freezes_a_bundles_setup() {
    let directory = FixtureDirectory::new();
    let connection = migrated_sidecar(&directory);
    connection
        .execute_batch(
            "insert into rounds (round_id, wallet_id, network, snapshot_height, ea_pk,
                                 nc_root, nullifier_imt_root, created_at)
             values ('round', 'wallet', 'testnet', 1, x'00', x'00', x'00', 0);
             insert into bundles (round_id, wallet_id, bundle_index, van_comm_rand)
             values ('round', 'wallet', 0, x'AABB');
             insert into votes (round_id, wallet_id, bundle_index, proposal_id, choice,
                                commitment, created_at)
             values ('round', 'wallet', 0, 1, 0, x'00', 0);",
        )
        .unwrap();
    let bundles = BundleSetup::read_all(&connection).unwrap();
    assert!(bundles[0].evidence.contains("vote"));
    assert!(bundles[0].is_frozen());
}

/// The fingerprint must separate types, so an integer column cannot compare
/// equal to a text column holding the same digits.
#[test]
fn fingerprints_distinguish_a_number_from_its_text() {
    let directory = FixtureDirectory::new();
    let connection = migrated_sidecar(&directory);
    connection
        .execute_batch(
            "insert into rounds (round_id, wallet_id, network, snapshot_height, ea_pk,
                                 nc_root, nullifier_imt_root, created_at)
             values ('round', 'wallet', 'testnet', 1, x'00', x'00', x'00', 0);
             insert into bundles (round_id, wallet_id, bundle_index, total_note_value,
                                  van_comm_rand)
             values ('round', 'wallet', 0, 125000000, '125000000');",
        )
        .unwrap();
    let bundles = BundleSetup::read_all(&connection).unwrap();
    assert_ne!(
        bundles[0].columns["total_note_value"],
        bundles[0].columns["van_comm_rand"]
    );
}

/// The map is keyed by column name, so the assertion cannot silently compare
/// the wrong pair after a reordering of `SETUP_COLUMNS`.
#[test]
fn setup_columns_are_read_by_name_not_by_position() {
    let directory = FixtureDirectory::new();
    let connection = migrated_sidecar(&directory);
    connection
        .execute_batch(
            "insert into rounds (round_id, wallet_id, network, snapshot_height, ea_pk,
                                 nc_root, nullifier_imt_root, created_at)
             values ('round', 'wallet', 'testnet', 1, x'00', x'00', x'00', 0);
             insert into bundles (round_id, wallet_id, bundle_index, van_comm_rand, alpha)
             values ('round', 'wallet', 0, x'11', x'22');",
        )
        .unwrap();
    let bundles = BundleSetup::read_all(&connection).unwrap();
    let expected: BTreeMap<String, Option<String>> = SETUP_COLUMNS
        .iter()
        .map(|column| ((*column).to_string(), bundles[0].columns[*column].clone()))
        .collect();
    assert_eq!(bundles[0].columns, expected);
    assert_ne!(
        bundles[0].columns["van_comm_rand"],
        bundles[0].columns["alpha"]
    );
}

/// End to end, against the real schema: flipping one byte of `van_comm_rand` in
/// a sidecar whose bundle has broadcast evidence must fail, naming the column.
///
/// The struct-level oracles above prove the rules; this proves the rules are
/// reached through the reader that live runs actually use.
#[test]
fn a_single_flipped_byte_in_a_real_sidecar_is_caught() {
    let directory = FixtureDirectory::new();
    let connection = migrated_sidecar(&directory);
    connection
        .execute_batch(
            "insert into rounds (round_id, wallet_id, network, snapshot_height, ea_pk,
                                 nc_root, nullifier_imt_root, created_at)
             values ('round', 'wallet', 'testnet', 1, x'00', x'00', x'00', 0);
             insert into bundles (round_id, wallet_id, bundle_index, van_comm_rand,
                                  delegation_tx_hash)
             values ('round', 'wallet', 0,
                     x'0101010101010101010101010101010101010101010101010101010101010101',
                     'hash');",
        )
        .unwrap();
    let before = BundleSetup::read_all(&connection).unwrap();

    connection
        .execute(
            "update bundles set van_comm_rand =
                 x'0201010101010101010101010101010101010101010101010101010101010101'",
            [],
        )
        .unwrap();
    let after = BundleSetup::read_all(&connection).unwrap();

    let error = assert_delegation_setup_preserved(&before, &after).unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("van_comm_rand"), "{message}");
    assert!(
        message.contains("strands the bundle's voting weight"),
        "{message}"
    );
}

/// The host-reset probe, end to end and hermetically.
///
/// Worth running without staging because the alternative is discovering a
/// broken copy, open or compare forty minutes into a live matrix. The state it
/// builds is the one a `before-broadcast` crash leaves: setup written, and a
/// `submitting` reservation whose bytes may already be on the wire.
#[test]
fn a_host_reset_leaves_setup_frozen_by_an_abandoned_reservation() {
    let directory = FixtureDirectory::new();
    let path = directory.0.join("crashed.db");
    let sidecar = path.to_str().unwrap();
    {
        let connection = migrated_sidecar_at(sidecar);
        connection
            .execute_batch(
                "insert into rounds (round_id, wallet_id, network, snapshot_height, ea_pk,
                                     nc_root, nullifier_imt_root, created_at)
                 values ('round', 'wallet', 'testnet', 1, x'00', x'00', x'00', 0);
                 insert into bundles (round_id, wallet_id, bundle_index, note_positions_blob,
                                      van_comm_rand, gov_comm, alpha, total_note_value)
                 values ('round', 'wallet', 0, x'00',
                         x'0101010101010101010101010101010101010101010101010101010101010101',
                         x'02', x'03', 125000000);
                 insert into chain_submissions (identity_key, round_id, wallet_id, network,
                                                bundle_index, kind, generation_digest, state,
                                                created_at, updated_at)
                 values (x'1111111111111111111111111111111111111111111111111111111111111111',
                         'round', 'wallet', 'testnet', 0, 'delegation',
                         x'2222222222222222222222222222222222222222222222222222222222222222',
                         'submitting', 0, 0);",
            )
            .unwrap();
    }

    // Captured before the probe runs, so the no-mutation check below compares
    // the whole snapshot rather than spot-checking one column.
    let before = BundleSetup::read_all(&rusqlite::Connection::open(sidecar).unwrap()).unwrap();

    let held = recovery_conformance::setup_preservation::assert_a_host_reset_preserved_crash_state(
        std::path::Path::new(sidecar),
        "wallet",
        "round",
    )
    .expect("a host reset must not take setup frozen by an abandoned reservation");
    assert_eq!(held.frozen_bundles, 1);
    assert!(held.compared_columns > 0, "{held}");

    // The probe runs the reset against a copy, so the sidecar it was handed must
    // come back byte-identical. This is what lets a live matrix call it between
    // a crash and the resume without perturbing the case around it — and it is
    // asserted over every column and every bundle, because a probe that mutated
    // one untracked field would otherwise pass here and corrupt a staging run.
    let after = BundleSetup::read_all(&rusqlite::Connection::open(sidecar).unwrap()).unwrap();
    assert_eq!(
        before, after,
        "the probe must leave the sidecar it was given exactly as it found it"
    );
    assert!(after[0].evidence.contains("chain_submission"));
}

/// A probe run against a round with no frozen setup proves nothing, and says so
/// rather than passing.
#[test]
fn a_host_reset_probe_over_unfrozen_setup_reports_itself_vacuous() {
    let directory = FixtureDirectory::new();
    let path = directory.0.join("fresh.db");
    let sidecar = path.to_str().unwrap();
    {
        let connection = migrated_sidecar_at(sidecar);
        connection
            .execute_batch(
                "insert into rounds (round_id, wallet_id, network, snapshot_height, ea_pk,
                                     nc_root, nullifier_imt_root, created_at)
                 values ('round', 'wallet', 'testnet', 1, x'00', x'00', x'00', 0);
                 insert into bundles (round_id, wallet_id, bundle_index, note_positions_blob,
                                      van_comm_rand)
                 values ('round', 'wallet', 0, x'00', x'01');",
            )
            .unwrap();
    }
    let error =
        recovery_conformance::setup_preservation::assert_a_host_reset_preserved_crash_state(
            std::path::Path::new(sidecar),
            "wallet",
            "round",
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("could not have been refused anything"));
}
