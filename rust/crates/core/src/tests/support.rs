use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    OnceLock,
};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn fixture() -> Value {
    let fixture_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/search/basic.json");
    serde_json::from_str(&fs::read_to_string(fixture_path).expect("fixture should be readable"))
        .expect("fixture should be valid JSON")
}

/// Installs `document` as the run configurations a host generated for `root`
/// this session. `root` must exist: the layer is keyed by its canonical path.
pub(crate) fn seed_generated(root: &Path, document: impl ToString) {
    let document = serde_json::from_str(&document.to_string()).expect("generated fixture");
    crate::execution::generated_layer::seed(root, document);
}

/// Writes `document` as the run configuration file `relative` below `root`,
/// such as `.lithe/run/local.yaml`. JSON text is valid YAML.
pub(crate) fn write_run_document(root: &Path, relative: &str, document: &Value) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("documents live in a directory")).unwrap();
    fs::write(path, document.to_string()).unwrap();
}

/// Applies the `changes` of a run configuration edit reply's `data` the way
/// the host does: each one only while its file still has the bytes the change
/// was computed from.
pub(crate) fn apply_run_changes(root: &Path, data: &Value) {
    use sha2::Digest;
    for change in data["changes"].as_array().expect("edits return changes") {
        let path = root.join(change["path"].as_str().expect("changes name a path"));
        let current = fs::read(&path)
            .ok()
            .map(|bytes| format!("{:x}", sha2::Sha256::digest(bytes)));
        assert_eq!(
            current.as_deref(),
            change["baseSha256"].as_str(),
            "{}",
            change["path"]
        );
        match change["content"].as_str() {
            Some(content) => {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, content).unwrap();
            }
            None => fs::remove_file(path).unwrap(),
        }
    }
}

/// The document an edit reply's `data` writes to `path`, parsed by the
/// format its extension names (YAML for run documents), or `None` when the edit leaves that
/// file alone or deletes it.
pub(crate) fn changed_run_document(data: &Value, path: &str) -> Option<Value> {
    let change = data["changes"]
        .as_array()?
        .iter()
        .find(|change| change["path"] == path)?;
    let content = change["content"].as_str()?;
    Some(if path.ends_with(".toml") {
        toml::from_str(content).expect("changed plugin documents parse")
    } else if path.ends_with(".json") {
        serde_json::from_str(content).expect("changed sidecars parse")
    } else {
        serde_yaml_ng::from_str(content).expect("changed documents parse")
    })
}

/// Rewrites the plugin run document `relative` below `root` after `edit`
/// changed it, the way a user editing the TOML file would.
pub(crate) fn edit_plugin_run_document(root: &Path, relative: &str, edit: impl FnOnce(&mut Value)) {
    let path = root.join(relative);
    let mut document: Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    edit(&mut document);
    fs::write(path, toml::to_string(&document).unwrap()).unwrap();
}

/// The paths an edit reply's `data` changes, in order.
pub(crate) fn changed_run_paths(data: &Value) -> Vec<String> {
    data["changes"]
        .as_array()
        .expect("edits return changes")
        .iter()
        .map(|change| change["path"].as_str().unwrap().to_string())
        .collect()
}

/// Removes a temporary workspace when dropped, so a failing assertion does
/// not leave it behind.
pub(crate) struct Cleanup(pub(crate) PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn temporary_root(label: &str) -> PathBuf {
    static NONCE: OnceLock<u128> = OnceLock::new();
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    // A clock tick is not a unique ID: parallel fixtures must never share cleanup ownership.
    let nonce = NONCE.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be valid")
            .as_nanos()
    });
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "lithe-core-{label}-{}-{nonce}-{sequence}",
        std::process::id()
    ))
}

#[test]
fn temporary_roots_do_not_alias_with_the_same_label_and_process_timestamp() {
    let roots: std::collections::HashSet<_> =
        (0..10_000).map(|_| temporary_root("same-label")).collect();
    assert_eq!(roots.len(), 10_000, "Each fixture must own a distinct path");
}
