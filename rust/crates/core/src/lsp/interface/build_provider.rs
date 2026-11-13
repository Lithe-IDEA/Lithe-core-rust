//! The build-provider protocol a plugin's LSP base-protocol process speaks
//! next to, or instead of, a language server.
//!
//! The manifest only says how to start the process; the protocol version is
//! the `buildProviders` capability version, negotiated in `initialize` under
//! `capabilities.experimental.litheBuild`. The host offers the versions of
//! that capability it implements and the provider answers the version it
//! chose. Build requests are written only to a session that answered a version
//! the host offered, so a shared provider that does not speak the protocol
//! keeps serving its language. The protocol covers models and dependencies,
//! task and test execution with progress and cancellation, opaque test
//! selectors and inline test reports, run and debug preparation, typed run
//! options and runtime debug connections, host-resolved model inputs, and
//! source-backed native configuration. The rules live in the plugin contract
//! (`shared/plugins/contract/v1/README.md`, "Build providers").

use crate::plugins::capabilities::{versions as capability_versions, BUILD_PROVIDERS};
use serde_json::{json, Value};

/// Key under `capabilities.experimental` on both sides of the exchange.
const EXPERIMENTAL_KEY: &str = "litheBuild";

/// Build-provider protocol versions this client speaks: the versions of the
/// `buildProviders` capability.
fn supported_versions() -> Vec<u64> {
    super::offered_versions(capability_versions(BUILD_PROVIDERS))
}

/// Feature name recorded in the session's server capabilities once a
/// provider answered a supported version. Every build method is available to
/// a session that carries it.
pub(crate) const BUILD_PROVIDER_FEATURE: &str = "litheBuild";

/// Task execution shares the workspace session and operation lifecycle.
pub(crate) const EXECUTE_METHOD: &str = "lithe/build/execute";
/// Prepares a bounded launch, while the host owns the application.
pub(crate) const PREPARE_RUN_METHOD: &str = "lithe/build/prepareRun";
/// Returns non-sensitive native settings and source/write metadata.
pub(crate) const CONFIGURATION_METHOD: &str = "lithe/build/configuration";
/// Provider-owned native editing; only a receipt crosses the wire.
pub(crate) const EDIT_CONFIGURATION_METHOD: &str = "lithe/build/editConfiguration";
/// Prepares a native debug target, never an adapter process.
pub(crate) const PREPARE_DEBUG_METHOD: &str = "lithe/build/prepareDebug";
/// Runs tests while retaining the bounded execution lifecycle.
pub(crate) const TEST_METHOD: &str = "lithe/build/test";
/// Returns exact selectors; decoding remains a separate Core command.
pub(crate) const DISCOVER_TESTS_METHOD: &str = "lithe/build/discoverTests";
/// Progress never uses caller-chosen host operation IDs.
pub(crate) const PROGRESS_METHOD: &str = "lithe/build/progress";

const MODEL_METHODS: [&str; 2] = ["lithe/build/model", "lithe/build/dependencies"];

/// The client half of the negotiation, placed under `capabilities.experimental`.
pub(crate) fn client_experimental_capabilities() -> Value {
    json!({ EXPERIMENTAL_KEY: { "versions": supported_versions() } })
}

/// Whether the server's `initialize` capabilities chose a version this client
/// offered. Unknown fields in the answer are ignored, as the contract requires.
pub(crate) fn negotiated(capabilities: &Value) -> bool {
    capabilities
        .get("experimental")
        .and_then(|experimental| experimental.get(EXPERIMENTAL_KEY))
        .and_then(|answer| answer.get("version"))
        .and_then(Value::as_u64)
        .is_some_and(|version| supported_versions().contains(&version))
}

/// Whether `method` is a request of the negotiated protocol.
pub(crate) fn is_build_method(method: &str) -> bool {
    MODEL_METHODS.contains(&method)
        || is_execution_method(method)
        || method == DISCOVER_TESTS_METHOD
        || method == PREPARE_RUN_METHOD
        || method == PREPARE_DEBUG_METHOD
        || method == CONFIGURATION_METHOD
        || method == EDIT_CONFIGURATION_METHOD
}

/// Whether the method shares bounded output, progress, and final result handling.
pub(crate) fn is_execution_method(method: &str) -> bool {
    matches!(method, EXECUTE_METHOD | TEST_METHOD)
}

/// A preparation has progress and cancellation, but its reply is a launch plan.
pub(crate) fn is_progress_method(method: &str) -> bool {
    is_execution_method(method) || method == PREPARE_RUN_METHOD || method == PREPARE_DEBUG_METHOD
}

/// Normalizes bounded progress; the engine checks the pending request identity.
pub(crate) fn progress(params: Option<&Value>) -> Option<Value> {
    let params = params?.as_object()?;
    let id = params.get("operationID")?.as_str()?;
    let mut result = json!({ "operationID": id });
    for (field, limit) in [("message", 4096), ("output", 65536)] {
        if let Some(value) = params.get(field) {
            let text = value.as_str()?;
            if text.len() > limit {
                return None;
            }
            result[field] = json!(text);
        }
    }
    if let Some(value) = params.get("percentage") {
        result["percentage"] = json!(value.as_u64().filter(|value| *value <= 100)?);
    }
    Some(result)
}

/// Stable outcome for a provider-owned task. Tool failure is an exit code,
/// while malformed replies and JSON-RPC errors are protocol failures.
pub(crate) fn execution_outcome(reply: &Value) -> Result<Value, (&'static str, String)> {
    if reply.get("error").is_some() {
        return Err((
            "plugin.build.failed",
            reply
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("The build provider rejected the task.")
                .chars()
                .take(4096)
                .collect(),
        ));
    }
    let invalid = (
        "plugin.build.invalidResult",
        "The build provider returned an invalid task result.".to_string(),
    );
    let value = reply.get("result").ok_or_else(|| invalid.clone())?;
    let exit = value
        .get("exitCode")
        .and_then(Value::as_i64)
        .filter(|code| i32::try_from(*code).is_ok())
        .ok_or_else(|| invalid.clone())?;
    let output = value
        .get("output")
        .and_then(Value::as_str)
        .filter(|text| text.len() <= 32 * 1024 * 1024)
        .ok_or_else(|| invalid.clone())?;
    let mut result = json!({ "exitCode": exit, "output": output });
    if let Some(reports) = value.get("testResults") {
        if !reports.is_object()
            || serde_json::to_vec(reports).map_or(usize::MAX, |value| value.len()) > 8 * 1024 * 1024
        {
            return Err(invalid);
        }
        result["testResults"] = reports.clone();
    }
    Ok(json!({ "value": result }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_offered_version_counts_as_negotiated() {
        assert!(negotiated(
            &json!({ "experimental": { "litheBuild": { "version": 0, "future": true } } })
        ));
        // No answer, a version the host never offered, or a malformed answer
        // must keep build requests off the session.
        assert!(!negotiated(&json!({})));
        assert!(!negotiated(
            &json!({ "experimental": { "litheBuild": {} } })
        ));
        assert!(!negotiated(
            &json!({ "experimental": { "litheBuild": { "version": 1 } } })
        ));
        assert!(!negotiated(
            &json!({ "experimental": { "litheBuild": { "version": 99 } } })
        ));
        assert!(!negotiated(
            &json!({ "experimental": { "litheBuild": { "version": "0" } } })
        ));
        assert!(!negotiated(
            &json!({ "experimental": { "litheBuild": { "versions": [0] } } })
        ));
    }

    #[test]
    fn the_host_offers_only_version_zero() {
        assert_eq!(
            client_experimental_capabilities(),
            json!({ "litheBuild": { "versions": [0] } })
        );
    }

    #[test]
    fn only_typed_provider_requests_are_accepted() {
        for method in [
            "lithe/build/model",
            "lithe/build/dependencies",
            "lithe/build/execute",
            "lithe/build/test",
            "lithe/build/discoverTests",
            "lithe/build/prepareRun",
            "lithe/build/prepareDebug",
            "lithe/build/configuration",
            "lithe/build/editConfiguration",
        ] {
            assert!(is_build_method(method), "{method}");
        }
        assert!(!is_build_method("workspace/executeCommand"));
    }

    #[test]
    fn inline_reports_are_retained_without_host_parsing() {
        let result = json!({"result":{"exitCode":1,"output":"failed","testResults":{"format":"litheTestResults","version":0,"tests":[]}}});
        assert_eq!(
            execution_outcome(&result).unwrap()["value"],
            result["result"]
        );
        for invalid in [json!(null), json!([]), json!("x")] {
            let mut reply = result.clone();
            reply["result"]["testResults"] = invalid;
            assert!(execution_outcome(&reply).is_err());
        }
    }

    #[test]
    fn execution_replies_and_progress_match_the_shared_contract() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../../../shared/plugins/contract/v1/fixtures/build-providers/build-execution-protocol.json"
        ))
        .unwrap();
        assert_eq!(
            execution_outcome(&json!({"result": fixture["result"]})).unwrap(),
            json!({"value": fixture["result"]})
        );
        for result in fixture["invalidResults"].as_array().unwrap() {
            assert_eq!(
                execution_outcome(&json!({"result": result})).unwrap_err().0,
                "plugin.build.invalidResult"
            );
        }
        assert_eq!(
            execution_outcome(&json!({"error": {"message": "private"}}))
                .unwrap_err()
                .0,
            "plugin.build.failed"
        );
        assert_eq!(
            progress(Some(&fixture["progress"])),
            Some(fixture["progress"].clone())
        );
        for invalid in fixture["invalidProgress"].as_array().unwrap() {
            assert!(progress(Some(invalid)).is_none());
        }
        assert!(progress(Some(&json!({"operationID":"1","output":"x".repeat(65537)}))).is_none());
        assert!(execution_outcome(&json!({
            "result": {"exitCode": 0,"output":"x".repeat(32*1024*1024+1)}
        }))
        .is_err());
    }
}
