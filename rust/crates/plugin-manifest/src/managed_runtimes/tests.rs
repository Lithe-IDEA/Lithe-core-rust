//! Deterministic contract checks for runtime selection and resolver answers.
//! Native download, archive and filesystem evidence belong to host tests.

use super::*;

fn fixture() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/plugins/managed-runtimes.json"
    )))
    .unwrap()
}

fn plan(manifest: Value) -> Value {
    managed_runtime_plan_command(json!({"manifest": manifest, "platform": fixture()["platform"]}))
        .unwrap()
}

fn resolution(result: Value, os: &str) -> Value {
    managed_runtime_resolution_command(json!({
        "manifest": fixture()["manifest"],
        "platform": {"os": os, "arch": "x86_64"},
        "result": result
    }))
    .unwrap()
}

#[test]
fn selects_exact_artifacts_and_normalizes_defaults() {
    let result = plan(fixture()["manifest"].clone());
    assert_eq!(result["status"], "ok", "{result}");
    let dependencies = result["plan"]["dependencies"].as_array().unwrap();
    assert_eq!(dependencies.len(), 2);
    assert_eq!(dependencies[0]["os"], "linux");
    assert_eq!(dependencies[0]["sha256"], "a".repeat(64));
    assert_eq!(dependencies[0]["home"], "");
    assert_eq!(dependencies[0]["tool"], "fixture-runtime");
    assert_eq!(dependencies[1]["id"], "server");
    assert_eq!(dependencies[1]["executable"], "");
    assert_eq!(result["plan"]["resolve"]["timeoutMilliseconds"], 30_000);
    assert!(dependencies[0]["cacheKey"]
        .as_str()
        .unwrap()
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(dependencies[0]["cacheKey"].as_str().unwrap().len(), 64);
}

#[test]
fn plugin_updates_and_declaration_order_preserve_cache_identity() {
    let mut manifest = fixture()["manifest"].clone();
    let original = plan(manifest.clone());
    manifest["version"] = json!("2.0.0");
    manifest["managedRuntimes"]["dependencies"]
        .as_array_mut()
        .unwrap()
        .reverse();
    for dependency in manifest["managedRuntimes"]["dependencies"]
        .as_array_mut()
        .unwrap()
    {
        dependency["artifacts"].as_array_mut().unwrap().reverse();
    }
    assert_eq!(plan(manifest)["plan"], original["plan"]);
}

#[test]
fn upstream_identity_digest_target_and_layout_change_cache_identity() {
    let manifest = fixture()["manifest"].clone();
    let original = plan(manifest.clone())["plan"]["dependencies"][0]["cacheKey"].clone();
    for (pointer, value) in [
        ("/managedRuntimes/dependencies/0/version", json!("26.0.0")),
        (
            "/managedRuntimes/dependencies/0/artifacts/1/sha256",
            json!("d".repeat(64)),
        ),
        (
            "/managedRuntimes/dependencies/0/artifacts/1/home",
            json!("Contents/Home"),
        ),
        (
            "/managedRuntimes/dependencies/0/artifacts/1/url",
            json!("https://example.invalid/rebuilt.tar.gz"),
        ),
    ] {
        let mut changed = manifest.clone();
        if pointer.ends_with("/home") {
            changed["managedRuntimes"]["dependencies"][0]["artifacts"][1]["home"] = value;
        } else {
            *changed.pointer_mut(pointer).unwrap() = value;
        }
        let result = plan(changed);
        assert_eq!(result["status"], "ok", "{pointer}: {result}");
        assert_ne!(result["plan"]["dependencies"][0]["cacheKey"], original);
    }
    let windows = managed_runtime_plan_command(json!({
        "manifest": manifest, "platform": {"os": "windows", "arch": "x86_64"}
    }))
    .unwrap();
    assert_ne!(windows["plan"]["dependencies"][0]["cacheKey"], original);
}

#[test]
fn absent_capability_preserves_old_manifests_and_old_hosts_refuse_new_ones() {
    let mut manifest = fixture()["manifest"].clone();
    let rejected = validate_command(json!({
        "manifest": manifest,
        "hostCapabilities": {"backend": [0], "toolDefinitions": [0]}
    }))
    .unwrap();
    assert_eq!(rejected["error"]["code"], "plugin.api.unsupported");
    assert_eq!(rejected["error"]["field"], "requires.managedRuntimes");
    manifest.as_object_mut().unwrap().remove("managedRuntimes");
    manifest["requires"]
        .as_object_mut()
        .unwrap()
        .remove("managedRuntimes");
    manifest["permissions"] = json!(["backend.launch"]);
    manifest["activationEvents"] = json!(["onWorkspaceOpen"]);
    assert_eq!(plan(manifest)["plan"], json!({"dependencies": []}));
}

#[test]
fn declarations_require_approval_and_self_contained_resolver() {
    for (pointer, value) in [
        ("/permissions", json!(["backend.launch"])),
        (
            "/backend/launch",
            json!({"kind":"systemCommand", "commands":["resolver"]}),
        ),
        ("/managedRuntimes/resolve/function", json!("")),
        ("/managedRuntimes/resolve/timeoutMilliseconds", json!(0)),
    ] {
        let mut manifest = fixture()["manifest"].clone();
        if pointer.ends_with("timeoutMilliseconds") {
            manifest["managedRuntimes"]["resolve"]["timeoutMilliseconds"] = value;
        } else {
            *manifest.pointer_mut(pointer).unwrap() = value;
        }
        assert_eq!(plan(manifest)["status"], "rejected", "{pointer}");
    }
}

#[test]
fn rejects_unsafe_archives_and_ambiguous_dependency_bindings() {
    for (field, value) in [
        ("url", json!("http://example.invalid/runtime.zip")),
        (
            "url",
            json!("https://user:password@example.invalid/runtime.zip"),
        ),
        ("url", json!("https://example.invalid/runtime.zip#fragment")),
        ("sha256", json!("abc")),
        ("archive", json!("exe")),
        ("stripPrefix", json!("../root")),
        ("home", json!("C:/outside")),
        ("home", json!("/outside")),
        ("executable", json!("bin\\runtime")),
        ("executable", json!("")),
    ] {
        let mut manifest = fixture()["manifest"].clone();
        manifest["managedRuntimes"]["dependencies"][0]["artifacts"][1][field] = value;
        assert_eq!(
            plan(manifest)["error"]["code"],
            "plugin.manifest.invalid",
            "{field}"
        );
    }
    let mut manifest = fixture()["manifest"].clone();
    manifest["managedRuntimes"]["dependencies"][1]["id"] = json!("engine");
    assert_eq!(plan(manifest)["status"], "rejected");
    let mut manifest = fixture()["manifest"].clone();
    manifest["managedRuntimes"]["dependencies"][0]["tool"] = json!("undeclared");
    assert_eq!(plan(manifest)["status"], "rejected");
}

#[test]
fn unsupported_platform_is_an_explicit_unavailable_outcome() {
    for platform in [
        json!({"os":"linux", "arch":"arm64"}),
        json!({"os":"freebsd", "arch":"x86_64"}),
    ] {
        let result = managed_runtime_plan_command(
            json!({"manifest":fixture()["manifest"], "platform":platform}),
        )
        .unwrap();
        assert_eq!(result["error"]["code"], "plugin.managedRuntime.unavailable");
    }
}

#[test]
fn resolver_partitions_are_complete_and_use_native_absolute_homes() {
    let fixture = fixture();
    for (key, os) in [("resolution", "linux"), ("windowsResolution", "windows")] {
        let result = resolution(fixture[key].clone(), os);
        assert_eq!(result["status"], "ok", "{result}");
        assert_eq!(result["resolution"], fixture[key]);
    }
    for result in [
        json!({"runtimes":[], "missing":["engine"]}),
        json!({"runtimes":[], "missing":["engine", "engine"]}),
        json!({"runtimes":[], "missing":["engine", "other"]}),
        json!({"runtimes":[{"id":"engine", "home":"relative", "version":"25"}], "missing":["server"]}),
        json!({"runtimes":[{"id":"engine", "home":"/opt/../runtime", "version":"25"}], "missing":["server"]}),
        json!({"runtimes":[{"id":"engine", "home":"/opt/runtime", "version":""}], "missing":["server"]}),
        json!({"runtimes":[], "missing":["engine", "server"], "executable":"/arbitrary"}),
    ] {
        assert_eq!(
            resolution(result, "linux")["error"]["code"],
            "plugin.managedRuntime.invalidResolution"
        );
    }
    assert_eq!(
        resolution(fixture["windowsResolution"].clone(), "linux")["status"],
        "rejected"
    );
    assert_eq!(
        resolution(fixture["resolution"].clone(), "windows")["status"],
        "rejected"
    );
}

#[test]
fn review_programs_include_every_origin_digest_and_resolver() {
    let manifest = fixture()["manifest"].clone();
    let programs = validate_manifest_value(manifest.clone())
        .unwrap()
        .programs();
    let runtimes: Vec<&Value> = programs
        .iter()
        .filter(|program| program["kind"] == "managedRuntime")
        .collect();
    assert_eq!(runtimes.len(), 2);
    assert_eq!(
        runtimes[0]["dependency"]["artifacts"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        runtimes[0]["resolve"]["function"],
        "fixture.resolveRuntimes"
    );
    let mut changed = manifest;
    changed["managedRuntimes"]["resolve"]["function"] = json!("fixture.newResolver");
    assert_ne!(
        validate_manifest_value(changed).unwrap().programs(),
        programs
    );
}

#[test]
fn mirrors_require_version_1_preserve_cache_key_and_reject_invalid_urls() {
    let base_manifest = fixture()["manifest"].clone();
    let base_plan = plan(base_manifest.clone());
    assert_eq!(base_plan["status"], "ok", "{base_plan}");
    let base_cache_key = base_plan["plan"]["dependencies"][0]["cacheKey"]
        .as_str()
        .unwrap()
        .to_string();

    // Declaring mirrors with managedRuntimes: 0 is rejected.
    let mut v0_with_mirrors = base_manifest.clone();
    v0_with_mirrors["managedRuntimes"]["dependencies"][0]["artifacts"][0]["mirrors"] =
        json!(["https://mirror.example.org/engine.tar.gz"]);
    assert_eq!(plan(v0_with_mirrors)["status"], "rejected");

    // With managedRuntimes: 1, mirrors appear in the plan without changing cacheKey.
    let mut v1 = base_manifest.clone();
    v1["requires"]["managedRuntimes"] = json!(1);
    let linux_index = v1["managedRuntimes"]["dependencies"][0]["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .position(|artifact| artifact["os"] == "linux")
        .unwrap();
    v1["managedRuntimes"]["dependencies"][0]["artifacts"][linux_index]["mirrors"] = json!([
        "https://mirror1.example.org/engine.tar.gz",
        "https://mirror2.example.org/engine.tar.gz"
    ]);
    let v1_plan = plan(v1.clone());
    assert_eq!(v1_plan["status"], "ok", "{v1_plan}");
    assert_eq!(
        v1_plan["plan"]["dependencies"][0]["cacheKey"].as_str(),
        Some(base_cache_key.as_str())
    );
    assert_eq!(
        v1_plan["plan"]["dependencies"][0]["mirrors"],
        json!([
            "https://mirror1.example.org/engine.tar.gz",
            "https://mirror2.example.org/engine.tar.gz"
        ])
    );

    // Non-HTTPS, credentials, duplicates, or >8 mirrors are rejected.
    let primary = v1["managedRuntimes"]["dependencies"][0]["artifacts"][linux_index]["url"]
        .as_str()
        .unwrap()
        .to_string();
    for bad in [
        json!(["http://mirror.example.org/engine.tar.gz"]),
        json!(["https://user:pass@mirror.example.org/engine.tar.gz"]),
        json!(["https://mirror.example.org/engine.tar.gz#frag"]),
        json!([primary]),
        json!([
            "https://mirror.example.org/engine.tar.gz",
            "https://mirror.example.org/engine.tar.gz"
        ]),
        json!([
            "https://m1.example.org/e",
            "https://m2.example.org/e",
            "https://m3.example.org/e",
            "https://m4.example.org/e",
            "https://m5.example.org/e",
            "https://m6.example.org/e",
            "https://m7.example.org/e",
            "https://m8.example.org/e",
            "https://m9.example.org/e"
        ]),
    ] {
        let mut invalid_manifest = v1.clone();
        invalid_manifest["managedRuntimes"]["dependencies"][0]["artifacts"][linux_index]
            ["mirrors"] = bad;
        assert_eq!(plan(invalid_manifest)["status"], "rejected");
    }
}
