//! Generated run configurations, held in memory for each workspace.
//!
//! Generation derives the generated layer from the runs build provider plugins
//! report. That makes it session state, not a project document: it lives here,
//! keyed by workspace, and is replaced by each generation. Nothing is written
//! into the workspace or a cache, so opening a project never fails on a
//! read-only checkout and never leaves files for version control. Only
//! configurations a user saves are project documents.

use super::configuration::RunConfigurationDocument;
#[cfg(any(test, feature = "test-support"))]
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

static LAYERS: OnceLock<Mutex<HashMap<PathBuf, Arc<RunConfigurationDocument>>>> = OnceLock::new();

fn layers() -> &'static Mutex<HashMap<PathBuf, Arc<RunConfigurationDocument>>> {
    LAYERS.get_or_init(Default::default)
}

/// Spellings of one workspace (a trailing separator, a symlinked path) share
/// one layer.
fn key(root: &Path) -> PathBuf {
    fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

/// The document the latest generation produced for `root`, if any this session.
pub(super) fn get(root: &Path) -> Option<Arc<RunConfigurationDocument>> {
    layers()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key(root))
        .cloned()
}

/// Replaces the generated layer of `root`.
pub(super) fn store(root: &Path, document: RunConfigurationDocument) {
    layers()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key(root), Arc::new(document));
}

/// Replaces the generated layer of `root` as if generation had produced
/// `document`.
#[cfg(any(test, feature = "test-support"))]
pub fn seed(root: &Path, mut document: Value) {
    super::configuration::migrate_document_value(&mut document);
    store(
        root,
        serde_json::from_value(document).expect("seeded run configurations"),
    );
}
