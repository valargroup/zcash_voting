//! Interrupt the actual migration transaction, without production fault switches.
use super::*;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn released_database() -> TempDb {
    let temp = v17_file(|_| {});
    if let Some(source) = std::env::var_os("MIGRATION_RELEASE_FIXTURE") {
        // A live capture is always copied. The retained original is read-only.
        std::fs::remove_file(temp.path()).unwrap();
        let source =
            Connection::open_with_flags(source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        assert!(
            matches!(released_version(&source), 13 | 17),
            "unsupported capture schema"
        );
        source.execute("VACUUM INTO ?1", [temp.path()]).unwrap();
        return temp;
    }
    let conn = Connection::open(temp.path()).unwrap();
    conn.execute_batch(RESET_SQL).unwrap();
    conn.execute_batch(include_str!("../historical/v3_0_0_completed_round.sql"))
        .unwrap();
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
        .unwrap();
    temp
}

fn released_version(conn: &Connection) -> u32 {
    conn.pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap()
}

fn fault_boundaries() -> [&'static str; 3] {
    let database = released_database();
    let conn = Connection::open(database.path()).unwrap();
    if released_version(&conn) == 17 {
        [
            "chain_submissions",
            "round_immediate_share",
            "combined_cast_rejections",
        ]
    } else {
        [
            "pir_proof_cache",
            "helper_share_plans",
            "combined_cast_rejections",
        ]
    }
}

fn durable_rows(conn: &Connection) -> Vec<(String, Vec<Vec<String>>)> {
    let tables = conn.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap().query_map([], |r| r.get::<_, String>(0)).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
    tables
        .into_iter()
        .map(|table| {
            let rows = dump_table(conn, &table);
            (table, rows)
        })
        .collect()
}

#[test]
fn denied_migration_statements_roll_back_all_prior_rungs() {
    for boundary in fault_boundaries() {
        let temp = released_database();
        let mut conn = Connection::open(temp.path()).unwrap();
        let original_version = released_version(&conn);
        let schema = schema_objects(&conn);
        let rows = durable_rows(&conn);
        conn.authorizer(Some(move |context: AuthContext<'_>| {
            if matches!(context.action, AuthAction::CreateTable { table_name } if table_name == boundary) {
                Authorization::Deny
            } else { Authorization::Allow }
        }));
        assert!(migrate(&mut conn).is_err(), "fault must fire at {boundary}");
        conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        assert_eq!(schema_objects(&conn), schema, "schema at {boundary}");
        assert!(durable_rows(&conn) == rows, "rows changed at {boundary}");
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                .unwrap(),
            original_version
        );
        migrate(&mut conn).unwrap();
    }
}

/// Runs only when the parent supplies a private temporary database and boundary.
#[test]
fn migration_process_worker() {
    let Ok(path) = std::env::var("MIGRATION_TEST_DATABASE") else {
        return;
    };
    let boundary = std::env::var("MIGRATION_TEST_BOUNDARY").unwrap();
    let marker = format!("{path}.boundary");
    let mut conn = Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
        .unwrap();
    let committed_boundary = boundary == "after-commit";
    let transaction_marker = marker.clone();
    conn.authorizer(Some(move |context: AuthContext<'_>| {
        if matches!(context.action, AuthAction::CreateTable { table_name } if table_name == boundary) {
            std::fs::write(&transaction_marker, b"inside migration transaction").unwrap();
            loop { std::thread::park(); }
        }
        Authorization::Allow
    }));
    migrate(&mut conn).unwrap();
    if committed_boundary {
        std::fs::write(&marker, b"migration committed").unwrap();
        loop {
            std::thread::park();
        }
    }
    panic!("migration completed without reaching the requested crash boundary");
}

#[test]
fn killed_migration_process_preserves_the_released_database() {
    for boundary in fault_boundaries().into_iter().chain(["after-commit"]) {
        let temp = released_database();
        // Compute the committed expectation on another copy, keeping the crash
        // subject at the released schema until the child opens it.
        let expected = released_database();
        let mut conn = Connection::open(expected.path()).unwrap();
        let expected_version = if boundary == "after-commit" {
            migrate(&mut conn).unwrap();
            CURRENT_VERSION
        } else {
            released_version(&conn)
        };
        let schema = schema_objects(&conn);
        let rows = durable_rows(&conn);
        drop(conn);
        let marker = format!("{}.boundary", temp.path());
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "storage::migrations::tests::interruption::migration_process_worker",
                "--nocapture",
            ])
            .env("MIGRATION_TEST_DATABASE", temp.path())
            .env("MIGRATION_TEST_BOUNDARY", boundary)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !std::path::Path::new(&marker).exists() && Instant::now() < deadline {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let reached = std::path::Path::new(&marker).exists();
        let _ = child.kill();
        let status = child.wait().unwrap();
        assert!(reached, "worker never reached {boundary}");
        assert!(!status.success());
        std::fs::remove_file(marker).unwrap();
        let mut reopened = Connection::open(temp.path()).unwrap();
        assert_eq!(schema_objects(&reopened), schema);
        assert!(
            durable_rows(&reopened) == rows,
            "interrupted migration changed durable rows"
        );
        assert_eq!(
            reopened
                .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                .unwrap(),
            expected_version
        );
        migrate(&mut reopened).unwrap();
        migrate(&mut reopened).unwrap();
        assert_eq!(
            reopened
                .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
    }
}
