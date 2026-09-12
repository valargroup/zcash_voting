//! A standalone, consistent copy of a sidecar.
//!
//! Two exercises need to ask "what would this operation do to the state a crash
//! left?" without actually doing it to the run in progress: the host-reset probe
//! and signer-less target recovery. Both answer it against a copy.
//!
//! `VACUUM INTO` rather than a file copy, because the sidecar is in WAL mode.
//! Its committed state lives across three files at the moment a crashed child
//! leaves it, and copying only the database would silently drop whatever the
//! crash had most recently committed — which is exactly the state under test.
//! `tests/sidecar_durability.rs` pins that this carries uncheckpointed commits.

use anyhow::{Context, Result};
use rusqlite::Connection;

/// A copy of a sidecar, removed along with its WAL sidecars on drop.
pub struct SidecarCopy(std::path::PathBuf);

impl SidecarCopy {
    /// Copies `sidecar`, naming the copy after `purpose` so two probes over one
    /// run cannot collide.
    pub fn of(sidecar: &std::path::Path, purpose: &str) -> Result<Self> {
        let path = sidecar.with_extension(format!("{purpose}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let source =
            Connection::open_with_flags(sidecar, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .context("opening the sidecar to copy it")?;
        source
            .execute("vacuum into ?1", [path.to_string_lossy().as_ref()])
            .with_context(|| format!("copying the sidecar for {purpose}"))?;
        Ok(Self(path))
    }

    pub fn path(&self) -> &std::path::Path {
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
