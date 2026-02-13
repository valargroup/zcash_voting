---
name: Replace SQLite with flat files
overview: Replace SQLite with a flat binary file for nullifier storage and a small checkpoint file for crash recovery, eliminating the rusqlite/r2d2 dependencies entirely.
todos:
  - id: file-store
    content: Create `file_store.rs` with checkpoint, append, truncate, and bulk-read functions
    status: pending
  - id: sync-nullifiers
    content: Rewrite `sync_nullifiers.rs` to use file_store instead of SQLite
    status: pending
  - id: tree-db
    content: Rewrite `tree_db.rs` to read from flat file
    status: pending
  - id: ingest-bin
    content: Simplify `ingest_nfs.rs` -- remove SQLite, use file paths
    status: pending
  - id: server-bin
    content: Simplify `server.rs` -- replace DB_PATH fallback with flat file
    status: pending
  - id: test-bin
    content: Update `test_non_inclusion.rs` to use flat file
    status: pending
  - id: cleanup
    content: Delete `db.rs`, update `lib.rs`, remove SQLite deps from Cargo.toml
    status: pending
  - id: integration-tests
    content: Update integration tests if they reference SQLite
    status: pending
isProject: false
---

# Replace SQLite with Flat File Storage

## Motivation

The database is 4.1 GB / 50M rows, and a full table scan through SQLite's B-tree is the bottleneck on cold start. SQLite's features (transactions, indexing, SQL queries) are overkill here -- the only access patterns are bulk append (ingest) and bulk read (tree build). A flat binary file gives us sequential I/O at memory bandwidth.

## File Format

Two files replace `nullifiers.db`:

- `**nullifiers.bin**` -- append-only, raw concatenation of 32-byte nullifier blobs. File size = `count * 32`. No header, no framing.
- `**nullifiers.checkpoint**` -- 16 bytes: `(height: u64 LE, byte_offset: u64 LE)`. Written atomically via write-to-temp + rename.

### Crash Safety

Write sequence per batch:

1. Append nullifier bytes to `nullifiers.bin`
2. `fsync` the data file
3. Write `(new_height, new_file_length)` to a temp file, `fsync`, rename to `nullifiers.checkpoint`

Recovery:

1. Read checkpoint -> `(height, offset)`
2. Truncate `nullifiers.bin` to `offset` (discards any partial batch)
3. Resume sync from `height`

This provides the same atomicity guarantee as SQLite's `BEGIN`/`COMMIT` -- uncommitted partial batches are rolled back on restart.

## Changes

### 1. New module: `[service/src/file_store.rs](nullifier-ingest/service/src/file_store.rs)`

Replaces `db.rs` with flat file equivalents:

- `save_checkpoint(dir, height, offset)` -- atomic write via temp + rename
- `load_checkpoint(dir) -> Option<(u64, u64)>`
- `append_nullifiers(path, &[(u64, Vec<u8>)])` -- append raw 32-byte blobs (heights discarded, only needed for the batch boundary tracked by checkpoint)
- `truncate_to_checkpoint(path, offset)` -- recovery truncation
- `load_all_nullifiers(path) -> Vec<Fp>` -- bulk read + parallel `Fp::from_repr` via Rayon (replaces the slow SQLite full-table scan)

### 2. Rewrite `[service/src/sync_nullifiers.rs](nullifier-ingest/service/src/sync_nullifiers.rs)`

- `sync()` takes a directory path instead of `&Connection`
- Replace `connection.execute_batch("BEGIN")` / `insert_nullifiers` / `save_checkpoint` / `COMMIT` with `append_nullifiers` + `save_checkpoint`
- `resume_height()` reads the checkpoint file instead of querying SQLite
- Delete `insert_nullifiers`, `migrate_nullifiers_table`, `rebuild_index` (no longer needed)

### 3. Rewrite `[service/src/tree_db.rs](nullifier-ingest/service/src/tree_db.rs)`

- `tree_from_db(&Connection)` becomes `tree_from_file(path)` -- calls `file_store::load_all_nullifiers` then `build_sentinel_tree`
- Delete `list_nf_ranges` and `load_all_nullifiers` (moved to `file_store`)

### 4. Simplify `[service/src/bin/ingest_nfs.rs](nullifier-ingest/service/src/bin/ingest_nfs.rs)`

- Remove all SQLite setup (Connection, Pool, PRAGMAs, `create_schema`, `migrate`, `rebuild_index`)
- Pass the data directory path to `sync()`
- Count nullifiers from file size (`file_len / 32`) instead of `SELECT COUNT(*)`

### 5. Simplify `[service/src/bin/server.rs](nullifier-ingest/service/src/bin/server.rs)`

- Replace the `DB_PATH` / SQLite fallback block with a `NF_PATH` flat file fallback
- Keep the sidecar `.tree` logic (it still makes sense -- flat file read + sort + tree build is fast but sidecar is instant)

### 6. Update `[service/src/bin/test_non_inclusion.rs](nullifier-ingest/service/src/bin/test_non_inclusion.rs)`

- Read from flat file or `.tree` sidecar instead of SQLite

### 7. Delete `[service/src/db.rs](nullifier-ingest/service/src/db.rs)`

- All its functions (create_schema, save_checkpoint, load_checkpoint, delete_nullifiers_at_height) are replaced by `file_store.rs`

### 8. Update `[service/src/lib.rs](nullifier-ingest/service/src/lib.rs)`

- Remove `pub mod db`, add `pub mod file_store`

### 9. Update `[service/Cargo.toml](nullifier-ingest/service/Cargo.toml)`

- Remove `rusqlite`, `r2d2`, `r2d2_sqlite`
- Add `rayon = "1"` (for parallel Fp parsing in `load_all_nullifiers`)

### 10. Update integration tests

- `[service/tests/api_integration.rs](nullifier-ingest/service/tests/api_integration.rs)` -- if tests use SQLite, update to use flat files

## Performance Estimate

Cold-start read: **~50M * 32 bytes = 1.6 GB sequential read** from a flat file, then parallel `Fp::from_repr`. This should complete in seconds vs the current multi-minute SQLite scan. Ingest writes become simple appends with no B-tree overhead.