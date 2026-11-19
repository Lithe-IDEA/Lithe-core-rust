//! Normalizes discovered test selectors without inferring test semantics.
//! Every item must round-trip through the ordinary typed test plan, so discovery
//! cannot expose an item the declaration is unable to run exactly.

use super::decode::{invalid_output, MAX_OUTPUT_BYTES};
use super::manifest::{BuildSystemDeclaration, OutputChannel};
use super::outputs;
use super::plans::{self, PlanBase, SelectedEntity, TestRunRequest};
use lithe_plugin_model::Validation;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Version of the `litheTestDiscovery` document.
const DISCOVERY_VERSION: u64 = 0;

const MAX_TESTS: usize = 10_000;

/// The plan's configuration and the discovery source's canonical result.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DecodeTestDiscoveryRequest {
    #[serde(flatten)]
    pub(crate) base: PlanBase,
    /// Provider discovery's inline JSON-RPC result.
    pub(crate) result: Option<Value>,
    /// The fresh host-owned output path passed to the command plan.
    pub(crate) output_file: Option<String>,
}

/// Selectors have no host-generated command or cached upstream project model.
#[derive(Deserialize)]
struct Selector {
    suite: String,
    case: String,
    project: Option<SelectedEntity>,
    target: Option<SelectedEntity>,
}

/// Rejects the entire malformed result rather than publishing a partial list
/// whose apparent completeness could hide tests or widen a selected run.
pub(crate) fn decode(
    build_system: &BuildSystemDeclaration,
    request: &DecodeTestDiscoveryRequest,
) -> Validation<Value> {
    let test = build_system
        .test
        .as_ref()
        .filter(|test| test.discovers_tests())
        .ok_or_else(|| plans::unavailable(build_system, "test discovery"))?;
    // Validate host context even for a valid empty discovery result.
    let context = plans::PlanContext::new(build_system, &request.base)?;
    let command_result;
    let result = if test.discovery_command.is_some() {
        if request.result.is_some() {
            return Err(plans::invalid_selection(
                "Command test discovery reads only outputFile.",
            ));
        }
        let text = outputs::command_output(
            OutputChannel::OutputFile,
            None,
            request.output_file.as_deref(),
        )?;
        command_result = serde_json::from_str::<Value>(&text)
            .map_err(|_| invalid_output("Invalid test discovery document."))?;
        &command_result
    } else {
        if request.output_file.is_some() {
            return Err(plans::invalid_selection(
                "A provider returns test discovery directly.",
            ));
        }
        let result = request
            .result
            .as_ref()
            .ok_or_else(|| invalid_output("Invalid test discovery document."))?;
        if serde_json::to_vec(result).map_or(usize::MAX, |bytes| bytes.len()) > MAX_OUTPUT_BYTES {
            return Err(invalid_output("Invalid test discovery document."));
        }
        result
    };
    if result["format"] != "litheTestDiscovery" || result["version"] != DISCOVERY_VERSION {
        return Err(invalid_output("Invalid test discovery document."));
    }
    let tests = result["tests"]
        .as_array()
        .filter(|tests| tests.len() <= MAX_TESTS)
        .ok_or_else(|| invalid_output("Test discovery requires at most 10000 selectors."))?;
    let mut normalized = BTreeMap::new();
    for item in tests {
        let selector: Selector = serde_json::from_value(item.clone())
            .map_err(|_| invalid_output("Invalid test selector."))?;
        let request = TestRunRequest {
            selector_kind: None,
            selectors: Vec::new(),
            project: selector.project,
            target: selector.target,
            suite: Some(selector.suite),
            case: Some(selector.case),
            user_arguments: vec![],
            projects: vec![],
        };
        let (_, selection) =
            plans::test_plan_with_selection(&context, test, &request).map_err(|_| {
                invalid_output("A discovered selector cannot be run by this declaration.")
            })?;
        let mut selected = json!({"suite":request.suite,"case":request.case});
        for (field, entity) in [("project", selection.project), ("target", selection.target)] {
            if let Some(entity) = entity {
                selected[field] = entity.test_selector();
            }
        }
        let key = serde_json::to_string(&selected).expect("selector is JSON");
        // Identity includes project and target, so equal suite/case names in
        // sibling modules remain independently runnable. Order is deterministic.
        selected["id"] = json!(format!("test:{:x}", Sha256::digest(key.as_bytes())));
        normalized.insert(key, selected);
    }
    Ok(json!({"tests":normalized.into_values().collect::<Vec<_>>()}))
}
