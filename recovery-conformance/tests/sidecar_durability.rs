//! The storage settings every crash assertion silently depends on.
//!
//! This suite's whole method is: kill a process, reopen the sidecar, and read
//! what survived. That method is only sound if a committed transaction really
//! is on disk when the process dies — so the settings that decide it are worth
//! pinning rather than assuming, especially as none of them is set explicitly.
//!
//! `zcash_voting` sets `journal_mode=WAL` and `foreign_keys=ON` when it opens a
//! sidecar and says nothing about `synchronous`, which leaves it at the value
//! the SQLite build was compiled with. That is `FULL` here. A build configured
//! with `SQLITE_DEFAULT_WAL_SYNCHRONOUS=1` would quietly drop it to `NORMAL`,
//! which is still safe against a killed process — the OS page cache outlives
//! it — but not against power loss or a kernel panic. Nothing in this
//! repository would otherwise notice that change.
//!
//! `foreign_keys` is pinned for a different reason: `bundles` references
//! `rounds` with `ON DELETE CASCADE`, and several of the SDK's deletion guards
//! rely on that cascade to remove dependent rows. With foreign keys off, a
//! round deletion would leave orphaned bundles holding setup that nothing can
//! reach.

/// Isolated fixture files, removed even when an assertion unwinds.
struct FixtureDirectory(std::path::PathBuf);

impl FixtureDirectory {
    fn new() -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "recovery-durability-{}-{}-{}",
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

fn pragma(connection: &rusqlite::Connection, name: &str) -> String {
    connection
        .query_row(&format!("pragma {name}"), [], |row| {
            row.get_ref(0).map(|value| match value {
                rusqlite::types::ValueRef::Text(text) => String::from_utf8_lossy(text).into_owned(),
                rusqlite::types::ValueRef::Integer(number) => number.to_string(),
                other => format!("{other:?}"),
            })
        })
        .unwrap()
}

/// A sidecar the SDK opened is in WAL mode with foreign keys enforced.
#[test]
fn the_sdk_opens_a_sidecar_in_wal_mode_with_foreign_keys_on() {
    let directory = FixtureDirectory::new();
    let path = directory.0.join("sidecar.db");
    let sidecar = path.to_str().unwrap();
    let database = zcash_voting::round::VotingDb::open(sidecar).unwrap();
    let connection = database.conn();
    assert_eq!(pragma(&connection, "journal_mode"), "wal");
    assert_eq!(
        pragma(&connection, "foreign_keys"),
        "1",
        "bundles cascade from rounds; with foreign keys off a deleted round \
         leaves orphaned delegation setup behind"
    );
}

/// A committed write is durable against power loss, not merely against a
/// killed process.
///
/// `synchronous` is never set by the SDK, so this reads what the linked SQLite
/// build defaults to. `FULL` (2) fsyncs the WAL on every commit. `NORMAL` (1)
/// does not, and a commit can then be lost to power loss or a kernel panic
/// while still surviving every fault this suite injects — so the weakening
/// would be invisible to every other test here.
#[test]
fn committed_sidecar_writes_are_synced_on_commit() {
    let directory = FixtureDirectory::new();
    let path = directory.0.join("sidecar.db");
    let sidecar = path.to_str().unwrap();
    let database = zcash_voting::round::VotingDb::open(sidecar).unwrap();
    let synchronous = pragma(&database.conn(), "synchronous");
    assert_eq!(
        synchronous, "2",
        "sidecar commits are no longer fsynced (synchronous={synchronous}, expected 2 = FULL). \
         Every fault this suite injects kills a process rather than a machine, so a \
         committed row would still survive all of them while being lost to power loss."
    );
}

/// `VACUUM INTO` produces a standalone copy that carries committed WAL frames.
///
/// The host-reset probe copies a crashed sidecar this way, and the crash state
/// it is meant to inspect lives in the WAL at that moment. A plain file copy of
/// the `.db` alone would silently drop it and the probe would inspect a round
/// that never crashed — passing, and proving nothing.
#[test]
fn a_vacuum_into_copy_carries_uncheckpointed_commits() {
    let directory = FixtureDirectory::new();
    let path = directory.0.join("sidecar.db");
    let sidecar = path.to_str().unwrap();
    let database = zcash_voting::round::VotingDb::open(sidecar).unwrap();
    database
        .conn()
        .execute(
            "insert into rounds (round_id, wallet_id, network, snapshot_height, ea_pk,
                                 nc_root, nullifier_imt_root, created_at)
             values ('probe', 'w', 'testnet', 1, x'00', x'00', x'00', 0)",
            [],
        )
        .unwrap();

    // Held open, so the commit is still in the WAL rather than checkpointed
    // back into the database file — the state a crashed sidecar is read in.
    let copy = directory.0.join("copy.db");
    database
        .conn()
        .execute("vacuum into ?1", [copy.to_string_lossy().as_ref()])
        .unwrap();

    let copied = rusqlite::Connection::open(&copy).unwrap();
    let rows: i64 = copied
        .query_row(
            "select count(*) from rounds where round_id = 'probe'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rows, 1, "the copy lost a commit that was still in the WAL");
}
