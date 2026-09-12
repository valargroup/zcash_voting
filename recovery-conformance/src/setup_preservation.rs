//! Whether a crash cost a bundle the delegation setup it can never rebuild.
//!
//! Every other assertion in this suite asks what a round still *owes*. This one
//! asks what it still *has*, and it exists because the two are not the same
//! question. A round can lose `van_comm_rand` and go on planning, proving and
//! confirming exactly as a healthy one would, right up to the point where the
//! VAN it reconstructs no longer matches the VAN already on chain — by which
//! time the governance nullifiers are spent and the bundle's voting weight is
//! stranded with no way back.
//!
//! Nothing in the schema stops that. `chain_submissions`,
//! `delegate_cast_recovery` and `round_immediate_share` each carry
//! `RAISE(ABORT)` triggers; the `bundles` table carries none, no `CHECK`, and
//! no write-once constraint. The setup `UPDATE` assigns `van_comm_rand`
//! directly rather than through `COALESCE`, so the whole write-once property
//! rests on two Rust guards — a "this write changes nothing" probe and a
//! broadcast-evidence check — inside `store_delegation_setup_within`. Those
//! guards are correct and conservative today. This module is what would notice
//! if they stopped being.
//!
//! # Why fingerprints, not bytes
//!
//! These columns are live key material. The harness writes per-case
//! diagnostics to logs and archives them on retry, so the comparison is made
//! over SHA-256 of each value and the raw bytes never leave SQLite. A `None`
//! entry means the column was SQL `NULL`, and that distinction carries the
//! assertion: `NULL` becoming a value is setup completing normally, while a
//! value becoming `NULL` is the loss this module exists to catch.
//!
//! # Why the broadcast-evidence condition
//!
//! "Setup never changes" is false, and asserting it would fail honest runs. A
//! bundle whose delegation has not reached the network may legitimately have
//! its setup exchanged wholesale —
//! `replace_unbroadcast_delegation_setup` discards and rewrites it in one
//! immediate transaction, which is how a wallet recovers from a target
//! mismatch. What must never happen is that exchange running against a bundle
//! the chain has already seen. So the rule keys off exactly what the SDK's own
//! guard keys off: once any broadcast evidence exists for a bundle, its setup
//! is frozen. Before that, a change is legal — and *reported*, so a rebuild is
//! never silent.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use rusqlite::types::ValueRef;
use rusqlite::Connection;
use sha2::{Digest, Sha256};

/// The `bundles` columns that make up a delegation's unrebuildable setup.
///
/// This is the same set `store_delegation_setup_within` writes and
/// `clear_unsigned_delegation_setup_fields` nulls, which is what makes a
/// comparison over it meaningful: a partial list would let the untracked
/// remainder change unobserved.
///
/// `van_comm_rand` leads it because it is the one with no way back. In the
/// hotkey regime it is re-derivable from the stored secret and the exact note
/// set; under `with_round_bound_voting_target` there is no such secret and the
/// value is sampled from `pallas::Base::random`, so a lost or altered value is
/// unrecoverable outright.
pub const SETUP_COLUMNS: &[&str] = &[
    "van_comm_rand",
    "gov_comm",
    "alpha",
    "rseed_signed",
    "rseed_output",
    "nf_signed",
    "rho_signed",
    "cmx_new",
    "dummy_nullifiers",
    "padded_note_data",
    "padded_note_secrets",
    "gov_nullifiers_blob",
    "rk",
    "note_positions_blob",
    "note_identity_hashes_blob",
    "total_note_value",
    "address_index",
    "pczt_sighash",
    "tx1_effects",
    "delegation_pczt",
];

/// One kind of durable proof that a bundle's delegation may have reached the
/// chain.
///
/// Named rather than collapsed into a boolean so a failure says *why* a
/// bundle's setup was frozen, and so a scenario that changes which evidence
/// exists fails as a changed name rather than as a mysterious verdict. The set
/// mirrors `first_broadcast_evidence` in the SDK; any one member is enough.
pub const BROADCAST_EVIDENCE: &[&str] = &[
    "delegation_tx_hash",
    "van_leaf_position",
    "chain_submission",
    "vote",
    "share_delegation",
];

/// One bundle's setup as a reopened sidecar shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleSetup {
    pub wallet_id: String,
    pub round_id: String,
    pub bundle_index: i64,
    /// Per-column SHA-256, or `None` where the column is SQL `NULL`.
    pub columns: BTreeMap<String, Option<String>>,
    /// Every kind of broadcast evidence this bundle carries. Empty means the
    /// delegation is not known to have reached the network, which is the one
    /// condition under which its setup may legally be exchanged.
    pub evidence: BTreeSet<String>,
    /// Consecutive combined-cast rejections recorded for this bundle.
    ///
    /// Read so that a retirement is visible. `retire_rejected_combined_generation`
    /// destroys the vote recovery, the helper plans, the share rows, the
    /// authorization and the submission row in one transaction while
    /// deliberately preserving setup — so a streak that grew across a resume is
    /// evidence that the most destructive path in the SDK ran, and that this
    /// bundle's setup survived it.
    pub rejection_streak: i64,
}

impl BundleSetup {
    /// Whether any durable evidence suggests this bundle's delegation reached
    /// the chain, and its setup is therefore frozen.
    pub fn is_frozen(&self) -> bool {
        !self.evidence.is_empty()
    }

    /// Identity, for cross-snapshot pairing and for failure messages.
    pub fn key(&self) -> (String, String, i64) {
        (
            self.wallet_id.clone(),
            self.round_id.clone(),
            self.bundle_index,
        )
    }

    /// Reads every bundle's setup through the caller's transaction.
    pub fn read_all(connection: &Connection) -> Result<Vec<Self>> {
        let columns = SETUP_COLUMNS.join(", ");
        let mut statement = connection
            .prepare(&format!(
                "select wallet_id, round_id, bundle_index,
                        delegation_tx_hash is not null, van_leaf_position is not null,
                        {columns}
                 from bundles
                 order by wallet_id, round_id, bundle_index"
            ))
            .context("querying bundle setup")?;
        let mut bundles = statement
            .query_map([], |row| {
                let mut evidence = BTreeSet::new();
                if row.get::<_, bool>(3)? {
                    evidence.insert("delegation_tx_hash".to_string());
                }
                if row.get::<_, bool>(4)? {
                    evidence.insert("van_leaf_position".to_string());
                }
                let mut values = BTreeMap::new();
                for (offset, column) in SETUP_COLUMNS.iter().enumerate() {
                    values.insert((*column).to_string(), fingerprint(row.get_ref(5 + offset)?));
                }
                Ok(Self {
                    wallet_id: row.get(0)?,
                    round_id: row.get(1)?,
                    bundle_index: row.get(2)?,
                    columns: values,
                    evidence,
                    rejection_streak: 0,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);

        for bundle in &mut bundles {
            let scope = rusqlite::params![bundle.wallet_id, bundle.round_id, bundle.bundle_index];
            // Each of these is independently sufficient, so they are read as
            // separate existence checks rather than one join: a failure names
            // the evidence that froze the bundle, and a table this build
            // expects but the sidecar lacks contributes nothing rather than
            // failing the read.
            for (name, table) in [
                ("chain_submission", "chain_submissions"),
                ("vote", "votes"),
                ("share_delegation", "share_delegations"),
            ] {
                let present: bool = connection
                    .query_row(
                        &format!(
                            "select exists(select 1 from {table}
                             where wallet_id = ?1 and round_id = ?2 and bundle_index = ?3)"
                        ),
                        scope,
                        |row| row.get(0),
                    )
                    .unwrap_or(false);
                if present {
                    bundle.evidence.insert(name.to_string());
                }
            }
            bundle.rejection_streak = connection
                .query_row(
                    "select consecutive_rejections from combined_cast_rejections
                     where wallet_id = ?1 and round_id = ?2 and bundle_index = ?3",
                    scope,
                    |row| row.get(0),
                )
                .unwrap_or(0);
        }
        Ok(bundles)
    }
}

/// SHA-256 of a column's value, or `None` for SQL `NULL`.
///
/// Every type is folded through the same hash so the comparison is uniform and
/// no raw value is ever held in memory longer than the hash takes. Integers are
/// hashed in a fixed byte order rather than through their text form, so a value
/// cannot compare equal to a differently-typed column holding the same digits.
fn fingerprint(value: ValueRef<'_>) -> Option<String> {
    let digest = match value {
        ValueRef::Null => return None,
        ValueRef::Integer(value) => Sha256::digest(value.to_le_bytes()),
        ValueRef::Real(value) => Sha256::digest(value.to_le_bytes()),
        ValueRef::Text(value) => Sha256::digest(value),
        ValueRef::Blob(value) => Sha256::digest(value),
    };
    Some(format!("{digest:x}"))
}

/// What a setup comparison actually looked at.
///
/// Returned rather than discarded because a comparison that examined nothing
/// would satisfy the assertion while proving nothing — the vacuous-pass shape
/// this suite has been caught by before. A caller reports these counts every
/// run, so a case that stops reaching real setup is visible as a number rather
/// than as a silent green.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SetupComparison {
    /// Bundles present in both snapshots and carrying broadcast evidence, so
    /// their setup was compared under the frozen rule.
    pub frozen_bundles: usize,
    /// Non-`NULL` columns actually compared across those bundles.
    pub compared_columns: usize,
    /// Legal pre-broadcast exchanges, named `bundle N: column`. Reported, never
    /// asserted: this is the rebuild path working as designed, and the point of
    /// listing it is that it can never happen unnoticed.
    pub rebuilt: Vec<String>,
    /// Bundles whose combined-cast rejection streak grew between the two
    /// snapshots, meaning a generation retirement ran and its setup was held to
    /// the frozen rule.
    pub retired_bundles: usize,
}

impl SetupComparison {
    /// Whether this comparison examined any real setup at all.
    pub fn is_vacuous(&self) -> bool {
        self.compared_columns == 0
    }
}

impl std::fmt::Display for SetupComparison {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} columns compared across {} frozen bundle(s)",
            self.compared_columns, self.frozen_bundles
        )?;
        if self.retired_bundles > 0 {
            write!(
                formatter,
                "; {} bundle(s) survived a generation retirement",
                self.retired_bundles
            )?;
        }
        if !self.rebuilt.is_empty() {
            write!(
                formatter,
                "; pre-broadcast rebuild observed: {}",
                self.rebuilt.join(", ")
            )?;
        }
        Ok(())
    }
}

/// A bundle whose delegation reached the chain keeps every byte of its setup.
///
/// Judged per bundle, per column, against `before` rather than against the
/// control: two rounds have different setup by construction, so the only honest
/// comparison is of one round against its own earlier self.
///
/// The four rules, in the order they are applied:
///
/// 1. A bundle present in `before` and gone from `after` is a violation
///    whatever its evidence. Bundles are removed only by an explicit,
///    evidence-checked deletion, and nothing a resume does may take that path.
/// 2. A non-`NULL` column on a bundle carrying broadcast evidence must be
///    byte-identical. Changed or nulled, both lose the round.
/// 3. A non-`NULL` column on a bundle with no evidence may change: that is
///    `replace_unbroadcast_delegation_setup`, and it is legal. Recorded in
///    [`SetupComparison::rebuilt`] and never asserted on.
/// 4. A `NULL` column may become anything. That is setup being written for the
///    first time, which is what a resume is supposed to do.
pub fn assert_delegation_setup_preserved(
    before: &[BundleSetup],
    after: &[BundleSetup],
) -> Result<SetupComparison> {
    let settled: BTreeMap<_, _> = after.iter().map(|bundle| (bundle.key(), bundle)).collect();
    let mut comparison = SetupComparison::default();

    for bundle in before {
        let key = bundle.key();
        let after_bundle = settled.get(&key).with_context(|| {
            format!(
                "SETUP VIOLATED: bundle {} vanished across resume, taking its delegation setup with it",
                bundle.bundle_index
            )
        })?;

        if after_bundle.rejection_streak > bundle.rejection_streak {
            comparison.retired_bundles += 1;
        }

        // Evidence only ever accumulates, so a bundle frozen in `after` was
        // frozen for at least part of the interval even if `before` had not yet
        // recorded it. Taking either side keeps a setup change from slipping
        // through the window between a POST and the row that records it.
        let frozen = bundle.is_frozen() || after_bundle.is_frozen();
        if frozen {
            comparison.frozen_bundles += 1;
        }

        for column in SETUP_COLUMNS {
            let Some(original) = bundle.columns.get(*column).and_then(Option::as_ref) else {
                continue;
            };
            let current = after_bundle.columns.get(*column).and_then(Option::as_ref);
            if !frozen {
                if current != Some(original) {
                    comparison
                        .rebuilt
                        .push(format!("bundle {}: {column}", bundle.bundle_index));
                }
                continue;
            }
            comparison.compared_columns += 1;
            let current = current.with_context(|| {
                format!(
                    "SETUP VIOLATED: {column} for bundle {} was cleared across resume, \
                     though the bundle carries broadcast evidence ({}). \
                     Its delegation is on chain and this value cannot be rebuilt.",
                    bundle.bundle_index,
                    evidence_list(&after_bundle.evidence)
                )
            })?;
            anyhow::ensure!(
                current == original,
                "SETUP VIOLATED: {column} for bundle {} changed across resume, \
                 though the bundle carries broadcast evidence ({}). \
                 Its delegation is on chain, so the VAN it commits to is fixed \
                 and a new value strands the bundle's voting weight.",
                bundle.bundle_index,
                evidence_list(&after_bundle.evidence)
            );
        }
    }

    Ok(comparison)
}

fn evidence_list(evidence: &BTreeSet<String>) -> String {
    if evidence.is_empty() {
        return "none".to_string();
    }
    evidence.iter().cloned().collect::<Vec<_>>().join(", ")
}

/// A host reset must not clear setup a crash left mid-submission.
///
/// `reset_voting_session_state` is the host's "the Keystone request is lost,
/// rebuild the setup" escape, and it is `pub` and takes no round lease. Its
/// guard set excludes bundles carrying a successful proof, a keystone
/// signature, a delegation hash, a VAN position, or any `chain_submissions`
/// row — which is what should make it safe against exactly the state a crash
/// mid-submission leaves behind.
///
/// The SDK's own tests cover that guard set against states assembled by
/// fixtures. This covers it against a state a real killed process produced,
/// which is the combination nothing else exercises: after a `before-broadcast`
/// crash the sidecar holds an abandoned reservation and setup that is now
/// load-bearing, and a reset that cleared it would strand a delegation whose
/// bytes may already be on the wire.
///
/// # Why a copy
///
/// The reset runs against a `VACUUM INTO` copy rather than the live sidecar.
/// Clearing *unbroadcast* setup is what this call is for, so on the round's
/// other bundles it may legitimately succeed — and letting it do so to the live
/// sidecar would make the surrounding crash case re-prove two delegations it
/// had already built, changing what that case exercises and what it costs. The
/// copy keeps the question narrow: given exactly this crash state, would a
/// reset take the bundle the crash interrupted?
///
/// Whether the reset returns an error or succeeds while changing nothing is
/// deliberately not asserted. Both are honest refusals, and requiring one would
/// pin the SDK's error surface rather than its safety. What must hold is that
/// the frozen setup is still there afterwards.
pub fn assert_a_host_reset_preserved_crash_state(
    sidecar: &std::path::Path,
    account_uuid: &str,
    round_id: &str,
) -> Result<SetupComparison> {
    let copy = SidecarCopy::of(sidecar)?;
    let before = read_setup(copy.path())?;

    let database = zcash_voting::round::VotingDb::open_path(copy.path())
        .map_err(|error| anyhow::anyhow!("reopening the sidecar copy: {error:?}"))?;
    database.set_wallet_id(account_uuid);
    let outcome = zcash_voting::prelude::reset_voting_session_state(&database, round_id);
    drop(database);

    let after = read_setup(copy.path())?;
    let comparison = assert_delegation_setup_preserved(&before, &after).with_context(|| {
        format!(
            "a host reset cleared delegation setup that a crash had left mid-submission \
             (the reset itself returned {})",
            match &outcome {
                Ok(()) => "success".to_string(),
                Err(error) => format!("{error:?}"),
            }
        )
    })?;
    anyhow::ensure!(
        !comparison.is_vacuous(),
        "a host reset was run against a sidecar holding no frozen delegation setup, \
         so it could not have been refused anything"
    );
    Ok(comparison)
}

fn read_setup(sidecar: &std::path::Path) -> Result<Vec<BundleSetup>> {
    let connection = Connection::open(sidecar).context("opening a sidecar to read setup")?;
    BundleSetup::read_all(&connection)
}

/// A standalone, consistent copy of a sidecar, removed on drop.
///
/// `VACUUM INTO` rather than a file copy: the sidecar is in WAL mode, so its
/// committed state lives across three files and copying only the database would
/// silently lose everything the crash had most recently committed — which is
/// exactly the state under test.
struct SidecarCopy(std::path::PathBuf);

impl SidecarCopy {
    fn of(sidecar: &std::path::Path) -> Result<Self> {
        let path = sidecar.with_extension(format!("reset-probe-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let source =
            Connection::open_with_flags(sidecar, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .context("opening the sidecar to copy it")?;
        source
            .execute("vacuum into ?1", [path.to_string_lossy().as_ref()])
            .context("copying the sidecar for a host-reset probe")?;
        Ok(Self(path))
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for SidecarCopy {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("db-wal"));
        let _ = std::fs::remove_file(self.0.with_extension("db-shm"));
    }
}
