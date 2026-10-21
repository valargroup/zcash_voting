//! The version selecting the ladder must belong to the acquired write lock.

use std::{cell::RefCell, sync::mpsc, time::Duration};

use super::*;

struct MigrationWait {
    reached: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}

thread_local! {
    static MIGRATION_WAIT: RefCell<Option<MigrationWait>> = const { RefCell::new(None) };
}

fn wait_for_other_migration(_: i32) -> bool {
    MIGRATION_WAIT.with(|slot| {
        let Some(wait) = slot.borrow_mut().take() else {
            return false;
        };
        wait.reached.send(()).is_ok() && wait.release.recv_timeout(Duration::from_secs(10)).is_ok()
    })
}

#[test]
fn waiting_opener_uses_the_committed_version_not_its_initial_read() {
    for winning_version in [19, CURRENT_VERSION, CURRENT_VERSION + 1] {
        let temp = v17_file(|conn| {
            queries::insert_round(
                conn,
                "wallet",
                crate::Network::Testnet,
                &test_params(),
                None,
            )
            .unwrap();
        });
        let mut winner = Connection::open(temp.path()).unwrap();
        winner.execute_batch("PRAGMA journal_mode=WAL").unwrap();
        let transaction = winner
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let path = temp.path().to_owned();
        let (reached, waiting) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let contender = std::thread::spawn(move || {
            let mut conn = Connection::open(path).unwrap();
            MIGRATION_WAIT.with(|slot| {
                *slot.borrow_mut() = Some(MigrationWait {
                    reached,
                    release: released,
                });
            });
            conn.busy_handler(Some(wait_for_other_migration)).unwrap();
            migrate(&mut conn)
        });

        // The contender has read version 17 and is now inside BEGIN IMMEDIATE.
        waiting.recv_timeout(Duration::from_secs(10)).unwrap();
        for (from, sql) in INCREMENTAL_MIGRATIONS {
            if *from >= 17 && *from < winning_version {
                transaction.execute_batch(sql).unwrap();
            }
        }
        transaction
            .pragma_update(None, "user_version", winning_version)
            .unwrap();
        transaction.commit().unwrap();
        release.send(()).unwrap();
        let opened = contender.join().unwrap();
        if winning_version > CURRENT_VERSION {
            assert!(opened
                .unwrap_err()
                .to_string()
                .contains("unsupported newer database version"));
        } else {
            opened.unwrap();
        }
        assert_eq!(
            winner
                .query_row("SELECT count(*) FROM rounds", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            winner
                .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                .unwrap(),
            winning_version.max(CURRENT_VERSION)
        );
    }
}
