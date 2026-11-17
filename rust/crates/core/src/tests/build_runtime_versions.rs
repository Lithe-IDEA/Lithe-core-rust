//! A runtime without executables names the tool definition with the same ID,
//! which finds the executable and checks the version itself. Core knows no
//! runtime by name: `go` and `node` follow the same rules as any other ID.

use crate::build::{build_check_tool_version_command, build_invocation_plan_command};
use lithe_plugin_manifest::DeclarativePluginManifest as Manifest;
use serde_json::{json, Value};

fn manifest() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../shared/plugins/contract/v1/fixtures/build-systems/build-toolchains.plugin.json"
    )))
    .expect("Cargo fixture is valid JSON")
}

fn plan(manifest: Value, platform: &str) -> Value {
    build_invocation_plan_command::<Manifest>(json!({
        "manifest": manifest,
        "buildSystemID": "cargo",
        "request": { "root": ".", "platform": platform }
    }))
    .expect("invocation request is valid")
}

fn check(manifest: Value, runtime: &str, output: &str) -> Value {
    build_check_tool_version_command::<Manifest>(json!({
        "manifest": manifest,
        "buildSystemID": "cargo",
        "request": { "runtime": runtime, "output": output }
    }))
    .expect("version request is valid")
}

/// A published manifest that names a runtime without executables keeps
/// validating; the plan resolves it through the tool definition with that
/// ID. `go` and `node` get no host candidate and no Core probe.
#[test]
fn runtime_without_executables_becomes_a_toolchain_candidate() {
    for platform in ["macos", "linux", "windows"] {
        for (id, minimum, path_variable) in [
            ("vm", "8", "LITHE_VM_PATH"),
            ("go", "1.22", "LITHE_GO_PATH"),
            ("node", "20", "LITHE_NODE_PATH"),
        ] {
            let mut manifest = manifest();
            manifest["buildSystems"][0]["invocation"]["runtimeDependencies"] =
                json!([{ "id": id, "minimumVersion": minimum }]);
            let result = plan(manifest, platform);
            assert_eq!(result["status"], "ok", "{platform} {id}: {result}");
            assert_eq!(
                result["invocation"]["runtimeDependencies"][0],
                json!({
                    "id": id,
                    "displayName": null,
                    "pathVariable": path_variable,
                    "executables": [
                        { "kind": "toolchain", "toolchainID": id, "root": "." }
                    ],
                    "version": null,
                    "minimumVersion": minimum
                }),
                "{platform} {id}"
            );
            // The build tool's own probe stays independent of its runtimes.
            assert_eq!(
                result["invocation"]["version"]["arguments"],
                json!(["--version"])
            );
        }
    }
}

#[test]
fn runtime_named_by_tool_definition_has_no_core_probe() {
    for (id, output) in [
        ("vm", "21.0.4\n"),
        ("go", "go version go1.22.3 darwin/arm64\n"),
        ("node", "v20.11.0\n"),
    ] {
        let mut manifest = manifest();
        manifest["buildSystems"][0]["invocation"]["runtimeDependencies"] =
            json!([{ "id": id, "minimumVersion": "8" }]);
        let result = check(manifest, id, output);
        assert_eq!(
            result,
            json!({
                "status": "rejected",
                "error": {
                    "code": "plugin.build.unavailable",
                    "message": format!(
                        "Cargo declares no version probe for {id}; its tool definition checks the version."
                    )
                }
            }),
            "{id}"
        );
    }
}

/// A runtime the plugin describes itself reads its version with its own
/// pattern, whatever its ID.
#[test]
fn described_runtime_reads_its_own_probe() {
    for (output, satisfied, version) in [
        ("go version go1.22.3 darwin/arm64\n", true, "1.22.3"),
        ("go version go1.21.9 linux/amd64\n", false, "1.21.9"),
    ] {
        let mut manifest = manifest();
        manifest["buildSystems"][0]["invocation"]["runtimeDependencies"] = json!([{
            "id": "go",
            "minimumVersion": "1.22",
            "executables": [{ "kind": "command", "command": "go" }],
            "version": { "arguments": ["version"], "pattern": "go(\\d+\\.\\d+(?:\\.\\d+)?)" }
        }]);
        let result = check(manifest, "go", output);
        assert_eq!(result["status"], "ok", "{result}");
        assert_eq!(
            result["version"],
            json!({ "satisfied": satisfied, "version": version }),
            "{output}"
        );
    }
}

/// Only a valid tool-definition ID can stand for a runtime the plugin does
/// not describe.
#[test]
fn runtime_without_executables_needs_a_tool_definition_id() {
    for id in ["Vm", "vm 17", "17-vm"] {
        let mut manifest = manifest();
        manifest["buildSystems"][0]["invocation"]["runtimeDependencies"] =
            json!([{ "id": id, "minimumVersion": "8" }]);
        let result = plan(manifest, "macos");
        assert_eq!(result["status"], "rejected", "{id}: {result}");
        assert_eq!(result["error"]["code"], "plugin.manifest.invalid", "{id}");
    }
}
