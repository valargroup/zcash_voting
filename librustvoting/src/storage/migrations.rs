use rusqlite::Connection;

use crate::VotingError;

const CURRENT_VERSION: u32 = 1;

pub fn migrate(conn: &Connection) -> Result<(), VotingError> {
    let version: u32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(|e| VotingError::Internal {
            message: format!("failed to read database version: {}", e),
        })?;

    if version < 1 {
        conn.execute_batch(include_str!("migrations/001_init.sql"))
            .map_err(|e| VotingError::Internal {
                message: format!("migration 001_init failed: {}", e),
            })?;
        conn.pragma_update(None, "user_version", 1)
            .map_err(|e| VotingError::Internal {
                message: format!("failed to update database version: {}", e),
            })?;
    }

    let final_version: u32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(|e| VotingError::Internal {
            message: format!("failed to verify database version: {}", e),
        })?;

    if final_version != CURRENT_VERSION {
        return Err(VotingError::Internal {
            message: format!(
                "unexpected database version after migration: expected {}, got {}",
                CURRENT_VERSION, final_version
            ),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migrate_fresh_database() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let version: u32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_VERSION);
    }

    #[test]
    fn test_migrate_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();

        let version: u32 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_VERSION);
    }

    #[test]
    fn test_tables_created() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();

        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();

        assert!(tables.contains(&"rounds".to_string()));
        assert!(tables.contains(&"cached_tree_state".to_string()));
        assert!(tables.contains(&"proofs".to_string()));
        assert!(tables.contains(&"votes".to_string()));
    }
}
