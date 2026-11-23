//! Fresh owner-only directories for short-lived native helper evidence.

use rand::RngCore;
use std::io;
use std::path::{Path, PathBuf};

/// Owns a fresh directory, removed after its caller has stopped the helper.
pub struct PrivateDirectory(PathBuf);

impl PrivateDirectory {
    /// Creates one directory in the platform temporary directory. No shared
    /// cache, installed resource or user preference is changed.
    pub fn new() -> io::Result<Self> {
        let mut bytes = [0u8; 16];
        rand::rngs::OsRng
            .try_fill_bytes(&mut bytes)
            .map_err(io::Error::other)?;
        let suffix: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = std::env::temp_dir().join(format!("lithe-evidence-{suffix}"));
        create(&path)?;
        Ok(Self(path))
    }

    /// Private parent for caller-owned evidence files.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        crate::failure_log::cleanup(
            "vcs.host.cleanup_failed",
            "evidence_cleanup",
            std::fs::remove_dir_all(&self.0),
        );
    }
}

#[cfg(unix)]
fn create(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(windows)]
fn create(path: &Path) -> io::Result<()> {
    crate::authentication::windows::private_directory(path)
}
