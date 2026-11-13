//! Run-configuration schemas, layered overrides, and deterministic generation.

mod project_toolchain;
pub use project_toolchain::{project_toolchain, ProjectToolchainRequest};

use super::generated_layer::{self, GeneratedLayer};
use super::types::{Confidence, Execution, RunCategory};
use lithe_protocol::{invalid_relative_path, CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

mod document;
mod editing;
mod generation;
mod inputs;
mod launch;
mod layers;
mod merge;
mod requests;
mod requirements;
mod resolve;
mod validation;

pub use document::*;
pub use editing::*;
pub use generation::*;
pub(crate) use inputs::*;
pub use launch::*;
pub(crate) use layers::*;
pub(crate) use merge::*;
pub use requests::*;
pub use requirements::*;
pub use resolve::*;
pub use validation::*;

const VERSION: u32 = 2;

const LEGACY_VERSION: u32 = 1;

// Bumped when the detected layer changes; stale cached facts must regenerate.
const GENERATOR_REVISION: &str = "20";

/// Toolchain requirements and `project.json` are separate documents that happen
/// to live under `.lithe`. Their schema did not change with run-config v2, so
/// they keep their own version and must not be validated against `VERSION`.
const SIDECAR_VERSION: u32 = 1;

#[cfg(test)]
mod incomplete_scan_tests {
    use super::*;

    #[test]
    fn unknown_inputs_retain_only_affected_configurations_and_hashes() {
        let previous: RunConfigurationDocument = serde_json::from_value(json!({
            "version": 2,
            "generator": {"fingerprint": "old", "inputs": {
                "blocked/package.json": "old-blocked", "removed/package.json": "old-removed"
            }},
            "configurations": [
                {"id":"blocked", "name":"Blocked", "provider":"npm.script", "source":"blocked/package.json"},
                {"id":"removed", "name":"Removed", "provider":"npm.script", "source":"removed/package.json"}
            ]
        })).unwrap();
        let mut configurations = Vec::new();
        let mut snapshot = ProjectInputs {
            inputs: BTreeMap::from([("visible/package.json".into(), "new".into())]),
            unreadable_paths: BTreeSet::from(["blocked".into()]),
        };
        retain_unknown_inputs(&mut configurations, &mut snapshot, previous);
        assert_eq!(configurations.len(), 1);
        assert_eq!(configurations[0].id, "blocked");
        assert_eq!(
            snapshot
                .inputs
                .get("blocked/package.json")
                .map(String::as_str),
            Some("old-blocked")
        );
        assert!(!snapshot.inputs.contains_key("removed/package.json"));
        assert!(!under_unknown(
            "blocked-sibling/package.json",
            &snapshot.unreadable_paths
        ));
    }
}
