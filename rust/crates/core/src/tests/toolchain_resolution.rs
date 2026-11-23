//! Exercises shared tool selection through production commands and inert native facts.

use super::toolchain_vectors::{
    local_pins, query_request, query_response, HOME, MANUAL_HOME, WORKSPACE,
};
use crate::toolchain::{
    candidates_command, check_version_command, discovery_plan_command, failure_reply_command,
    resolve_command,
};
use lithe_plugin_manifest::validated_manifest_json;
use serde_json::{json, Value};

const OLD: &str = "/opt/lithe-fixtures/installs/java/17.0.12";

/// Reads review data as test inputs, never as an installed plugin or live path.
fn contract(name: &str) -> Value {
    let source = match name {
        "java" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../shared/plugins/contract/v1/fixtures/toolchains/tool-definition.plugin.json"
        )),
        "mise" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../shared/plugins/contract/v1/fixtures/toolchains/toolchain-provider.plugin.json"
        )),
        _ => panic!("unknown fixture"),
    };
    serde_json::from_str(source).unwrap()
}

/// Retains the complete production manifest used by the shared contract fixture.
fn manifest(fragment: &Value) -> Value {
    fragment.clone()
}

/// Records only host-observed filesystem metadata; no native file is created.
fn fact(path: &str, kind: &str, executable: bool) -> Value {
    json!({"path":path,"canonicalPath":path,"kind":kind,"executable":executable})
}

/// Starts with an enabled definition and provider but no selected installation.
fn input() -> Value {
    json!({"registry":{"plugins":[{"manifest":manifest(&contract("java"))},{"manifest":manifest(&contract("mise"))}]},
        "operationID":"resolve-1","generation":1,"context":{"contextID":"context-1","generation":1},"tool":"java","directory":".","platform":"macos","workspaceRoot":WORKSPACE,
        "filesystem":[fact(WORKSPACE,"directory",false)],"probes":[],"queries":[],"systemInstallations":[],"environment":{},"commandHomes":{}})
}

/// Adds a usable installation and its bounded version-probe result.
fn installation(input: &mut Value, home: &str, version: &str) {
    let facts = input["filesystem"].as_array_mut().unwrap();
    facts.extend([
        fact(home, "directory", false),
        fact(&format!("{home}/bin"), "directory", false),
        fact(&format!("{home}/bin/java"), "file", true),
    ]);
    input["probes"].as_array_mut().unwrap().push(json!({"executable":format!("{home}/bin/java"),"output":format!("openjdk version \"{version}\""),"exitCode":0}));
}

/// Resolves through the exported command envelope, preserving rejection diagnostics.
fn resolve(input: &Value) -> Value {
    let result = resolve_command(input.clone(), validated_manifest_json).unwrap();
    assert_eq!(result["status"], "ok", "{result}");
    result["resolution"].clone()
}

/// Captures a fake backend exchange that the resolver must decode itself.
fn query(input: &Value) -> Value {
    json!({"registry":input["registry"],"pluginID":"dev.lithe.fixture.mise","request":query_request(),"response":query_response(),
        "currentGeneration":1,"platform":"macos","filesystem":input["filesystem"]})
}

#[test]
fn consumer_fixed_installation_precedes_directory_pins_without_granting_a_fallback() {
    let mut value = input();
    installation(&mut value, HOME, "21.0.4");
    installation(&mut value, OLD, "17.0.12");
    value["local"] = json!({"version":1,"toolchains":{},"pins":{".":{"java":{"home":HOME,"version":"21","provider":"host:manual"}}}});
    value["fixed"] = json!({"home":OLD});
    // Broken lower-layer answers have no authority over this explicit choice.
    value["queries"] = json!([{"invalid":"lower layer"}]);
    let selected = resolve(&value);
    assert_eq!(selected["outcome"]["selection"]["home"], OLD);
    assert_eq!(selected["trace"][0]["candidateID"], "consumerFixed");
    value["fixed"] = json!({"home":"/opt/lithe-fixtures/missing"});
    assert_eq!(resolve(&value)["outcome"]["kind"], "terminalFailure");
}

#[test]
fn consumer_fixed_executable_preserves_its_argv_identity_and_has_no_invented_home() {
    let mut value = input();
    let executable = "/opt/lithe-fixtures/custom tools/launcher";
    value["fixed"] = json!({"executable":executable});
    value["filesystem"]
        .as_array_mut()
        .unwrap()
        .push(fact(executable, "file", true));
    let candidates =
        candidates_command(value_for_candidates(&value), validated_manifest_json).unwrap();
    assert_eq!(candidates["status"], "ok", "{candidates}");
    assert!(candidates["candidates"]["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value["reason"] == "probeFailed" && value["executable"] == executable));
    value["probes"] =
        json!([{"executable":executable,"output":"openjdk version \"21.0.4\"","exitCode":0}]);
    let result = resolve(&value);
    let selected = &result["outcome"]["selection"];
    assert_eq!(selected["executable"], executable);
    assert_eq!(selected["version"], "21.0.4");
    assert!(selected["home"].is_null());
    assert_eq!(
        selected["environment"],
        json!({"LITHE_JAVA_PATH":executable})
    );
    assert_eq!(selected["pathPrepend"], json!([]));
    value["minimumVersion"] = json!("23");
    assert_eq!(
        resolve(&value)["outcome"]["reason"]["code"],
        "versionUnsatisfied"
    );
}

fn value_for_candidates(value: &Value) -> Value {
    let mut result = json!({});
    for key in [
        "registry",
        "tool",
        "platform",
        "filesystemCaseSensitive",
        "workspaceRoot",
        "workspaceRoots",
        "filesystem",
        "minimumVersion",
        "probes",
        "systemInstallations",
        "environment",
        "fixed",
        "projectCodeAllowed",
    ] {
        if let Some(value) = value.get(key) {
            result[key] = value.clone();
        }
    }
    result
}

#[test]
fn consumer_fixed_project_files_require_the_current_action_before_any_version_probe() {
    for alias in [false, true] {
        let mut value = input();
        let inside = format!("{WORKSPACE}/scripts/launcher");
        let executable = if alias {
            "/opt/lithe-fixtures/alias"
        } else {
            &inside
        };
        value["fixed"] = json!({"executable":executable});
        value["filesystem"].as_array_mut().unwrap().push(
            json!({"path":executable,"canonicalPath":inside,"kind":"file","executable":true}),
        );
        let candidates =
            candidates_command(value_for_candidates(&value), validated_manifest_json).unwrap();
        assert_eq!(candidates["status"], "ok", "{candidates}");
        assert!(!candidates["candidates"]["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["executable"] == executable));
        assert_eq!(
            resolve(&value)["outcome"]["reason"]["code"],
            "projectCodeRequiresAction"
        );
        value["projectCodeAllowed"] = json!(true);
        value["probes"] =
            json!([{"executable":executable,"output":"openjdk version \"21.0.4\"","exitCode":0}]);
        assert_eq!(resolve(&value)["outcome"]["selection"]["projectCode"], true);
    }
}

#[test]
fn consumer_fixed_choices_reject_ambiguous_relative_and_authority_smuggling_fields() {
    for fixed in [
        Value::Null,
        json!({}),
        json!({"home":HOME,"executable":"/opt/tool"}),
        json!({"executable":"relative/tool"}),
        json!({"home":"/opt/../tool"}),
        json!({"executable":"/opt/tool","version":"21"}),
        json!({"home":HOME,"provider":"host:official"}),
    ] {
        let mut value = input();
        value["fixed"] = fixed.clone();
        assert_eq!(
            resolve_command(value.clone(), validated_manifest_json).unwrap()["status"],
            "rejected",
            "{fixed}"
        );
        assert_eq!(
            candidates_command(value_for_candidates(&value), validated_manifest_json).unwrap()
                ["status"],
            "rejected",
            "{fixed}"
        );
    }
}

#[test]
fn toolchain_resolution_pins_inherit_nearest_ancestor_without_sibling_prefixes() {
    let mut value = input();
    let manual = MANUAL_HOME;
    installation(&mut value, HOME, "21.0.4");
    installation(&mut value, manual, "17.0.12");
    value["local"] = local_pins();
    value["directory"] = json!("apps/web/src");
    let result = resolve(&value);
    assert_eq!(result["outcome"]["selection"]["home"], manual);
    assert_eq!(result["outcome"]["selection"]["layer"], "pinned");
    assert_eq!(
        result["outcome"]["selection"]["environment"]["JAVA_HOME"],
        manual
    );
    value["directory"] = json!("apps/website");
    assert_eq!(resolve(&value)["outcome"]["selection"]["home"], HOME);
}

#[test]
fn toolchain_resolution_invalid_pin_stops_otherwise_usable_fallback() {
    let mut value = input();
    installation(&mut value, HOME, "21.0.4");
    value["local"] = json!({"version":1,"toolchains":{},"pins":{".":{"java":{"home":"/opt/lithe-fixtures/missing","version":"99","provider":"host:manual"}}}});
    value["commandHomes"]["java"] = json!(HOME);
    let result = resolve(&value);
    assert_eq!(result["outcome"]["kind"], "terminalFailure");
    assert!(result["outcome"].get("selection").is_none());
    assert!(result["trace"]
        .as_array()
        .unwrap()
        .iter()
        .skip(1)
        .all(|row| row["state"] == "shadowed"));
}

#[test]
fn toolchain_resolution_valid_pin_does_not_depend_on_provider_availability() {
    let mut value = input();
    installation(&mut value, HOME, "21.0.4");
    value["local"] = json!({"version":1,"toolchains":{},"pins":{".":{"java":{"home":HOME,"version":"21.0.4","provider":"host:manual"}}}});
    // A selected pin does not require querying, decoding or probing lower layers.
    value["queries"] = json!([{"response":{"error":{"code":-32072}}}]);
    let result = resolve(&value);
    assert_eq!(result["outcome"]["selection"]["home"], HOME);
    assert_eq!(result["outcome"]["selection"]["layer"], "pinned");
}

#[test]
fn toolchain_resolution_review_keeps_pin_when_project_paths_exceed_the_native_limit() {
    let mut value = input();
    installation(&mut value, HOME, "21.0.4");
    value["local"] = local_pins();
    let definition = &mut value["registry"]["plugins"][0]["manifest"]["toolDefinitions"][0];
    let runner = "long-but-valid-fixture-wrapper";
    definition["wrappers"] = json!([{"path":runner,"platforms":["macos"]}]);
    definition["executables"].as_array_mut().unwrap().insert(
        1,
        json!({"id":"wrapper","kind":"projectFile","path":runner,"platforms":["macos"]}),
    );
    // The root is within the native path budget, but appending the optional layout is not.
    let root = format!("/{}fixture", "sample-project/".repeat(272));
    value["workspaceRoot"] = json!(root);
    value["filesystem"].as_array_mut().unwrap()[0] = fact(&root, "directory", false);

    let result = resolve(&value);
    assert_eq!(result["outcome"]["selection"]["home"], HOME);
    assert_eq!(result["outcome"]["selection"]["layer"], "pinned");
    assert_eq!(result["projectCandidates"], json!([]));
}

#[test]
fn toolchain_resolution_optional_provider_timeout_keeps_shell_fallback_visible() {
    let mut value = input();
    installation(&mut value, HOME, "21.0.4");
    value["commandHomes"]["java"] = json!(HOME);
    let mut backend = query(&value);
    backend["response"] = json!({"jsonrpc":"2.0","id":backend["request"]["id"],"error":{"code":-32072,"message":"Fixture timeout.","data":{"code":"plugin.toolchain.timeout","operationID":backend["request"]["params"]["operationID"],"providerID":"mise"}}});
    value["queries"] = json!([backend]);
    let result = resolve(&value);
    assert_eq!(result["outcome"]["selection"]["provider"], "host:shell");
    assert_eq!(result["unmet"][0]["code"], "providerUnavailable");
}

#[test]
fn toolchain_resolution_host_failure_reply_names_the_provider_and_its_cause() {
    let mut value = input();
    installation(&mut value, HOME, "21.0.4");
    value["commandHomes"]["java"] = json!(HOME);
    for code in [
        "plugin.toolchain.timeout",
        "plugin.toolchain.providerUnavailable",
    ] {
        let mut backend = query(&value);
        let reply =
            failure_reply_command(json!({"params":backend["request"]["params"],"code":code}))
                .unwrap();
        assert_eq!(reply["status"], "ok", "{reply}");
        backend["request"]["id"] = reply["reply"]["id"].clone();
        backend["response"] = reply["reply"]["response"].clone();
        value["queries"] = json!([backend]);
        let result = resolve(&value);
        assert_eq!(result["outcome"]["selection"]["provider"], "host:shell");
        assert_eq!(
            result["unmet"],
            json!([{"code":"providerUnavailable","tool":"java",
                "provider":"dev.lithe.fixture.mise:mise","cause":code}])
        );
        // A provider that was not asked about this tool leaves it nothing to report.
        backend["request"]["params"]["tools"] = json!(["node"]);
        value["queries"] = json!([backend]);
        assert_eq!(resolve(&value)["unmet"], json!([]));
    }
    let params = query(&value)["request"]["params"].clone();
    let mut nameless = params.clone();
    nameless["providerID"] = json!("");
    // Cancellation is not a provider that could not answer, and an identity is required.
    for input in [
        json!({"params":params,"code":"plugin.toolchain.cancelled"}),
        json!({"params":nameless,"code":"plugin.toolchain.timeout"}),
        json!({"params":params,"code":"plugin.toolchain.timeout","message":"extra"}),
    ] {
        let rejected = failure_reply_command(input).unwrap();
        assert_eq!(rejected["status"], "rejected");
        assert_eq!(rejected["error"]["code"], "plugin.toolchain.invalidParams");
    }
}

#[test]
fn toolchain_resolution_uninstalled_project_requirement_survives_shell_fallback() {
    let mut value = input();
    installation(&mut value, HOME, "21.0.4");
    value["filesystem"].as_array_mut().unwrap().push(fact(
        &format!("{WORKSPACE}/mise.toml"),
        "file",
        false,
    ));
    value["commandHomes"]["java"] = json!(HOME);
    let mut backend = query(&value);
    backend["response"]["result"]["installations"] = json!([]);
    backend["response"]["result"]["statements"][0]["installation"] = Value::Null;
    backend["response"]["result"]["statements"][0]["request"] = json!("99");
    value["queries"] = json!([backend]);
    let result = resolve(&value);
    assert_eq!(result["outcome"]["selection"]["home"], HOME);
    assert_eq!(result["outcome"]["selection"]["layer"], "global");
    assert!(result["unmet"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason["code"] == "requestedNotInstalled" && reason["request"] == "99"));
}

#[test]
fn toolchain_resolution_provider_reply_goes_through_real_decoder() {
    let mut value = input();
    installation(&mut value, HOME, "21.0.4");
    value["filesystem"].as_array_mut().unwrap().push(fact(
        &format!("{WORKSPACE}/mise.toml"),
        "file",
        false,
    ));
    value["queries"] = json!([query(&value)]);
    let result = resolve(&value);
    assert_eq!(result["outcome"]["selection"]["home"], HOME);
    assert_eq!(result["outcome"]["selection"]["layer"], "project");
    assert_eq!(
        result["outcome"]["selection"]["provider"],
        "dev.lithe.fixture.mise:mise"
    );
    value["queries"][0]["response"]["result"]["operationID"] = json!("expired-query");
    let rejected = resolve_command(value, validated_manifest_json).unwrap();
    assert_eq!(rejected["status"], "rejected");
    assert_eq!(
        rejected["error"]["code"],
        "plugin.toolchain.operationExpired"
    );
}

#[test]
fn toolchain_resolution_query_cannot_replace_outer_generation_or_context_policy() {
    let mut value = input();
    installation(&mut value, HOME, "21.0.4");
    value["filesystem"].as_array_mut().unwrap().push(fact(
        &format!("{WORKSPACE}/mise.toml"),
        "file",
        false,
    ));
    value["queries"] = json!([query(&value)]);
    value["generation"] = json!(2);
    let rejected = resolve_command(value.clone(), validated_manifest_json).unwrap();
    assert_eq!(rejected["status"], "rejected");
    assert_eq!(rejected["error"]["code"], "plugin.toolchain.invalidParams");
    value["generation"] = json!(1);
    value["context"]["contextID"] = json!("replacement-context");
    let rejected = resolve_command(value, validated_manifest_json).unwrap();
    assert_eq!(rejected["status"], "rejected");
    assert_eq!(rejected["error"]["code"], "plugin.toolchain.invalidParams");
}

#[test]
fn toolchain_resolution_conflicting_global_providers_follow_shell_not_plugin_id() {
    let mut value = input();
    installation(&mut value, HOME, "21.0.4");
    installation(&mut value, OLD, "17.0.12");
    let mut second = manifest(&contract("mise"));
    second["id"] = json!("dev.lithe.fixture.alternative");
    value["registry"]["plugins"]
        .as_array_mut()
        .unwrap()
        .push(json!({"manifest":second}));
    for source in [
        "/opt/lithe-fixtures/global/mise.toml",
        "/opt/lithe-fixtures/alternative/mise.toml",
    ] {
        value["filesystem"]
            .as_array_mut()
            .unwrap()
            .push(fact(source, "file", false));
    }
    let mut first = query(&value);
    first["response"]["result"]["statements"][0]["scope"] = json!("global");
    first["response"]["result"]["statements"][0]["source"] =
        json!("/opt/lithe-fixtures/global/mise.toml");
    let mut second = first.clone();
    second["pluginID"] = json!("dev.lithe.fixture.alternative");
    second["response"]["result"]["installations"][0]["home"] = json!(OLD);
    second["response"]["result"]["statements"][0]["installation"] = json!(OLD);
    second["response"]["result"]["statements"][0]["source"] =
        json!("/opt/lithe-fixtures/alternative/mise.toml");
    value["queries"] = json!([first, second]);
    value["commandHomes"]["java"] = json!(HOME);
    let result = resolve(&value);
    assert_eq!(result["outcome"]["selection"]["provider"], "host:shell");
    assert_eq!(result["conflicts"][0]["scope"], "global");
    assert_eq!(result["conflicts"][0]["action"], "chooseProvider");
    value["providerChoices"]["global"] = json!("dev.lithe.fixture.alternative:mise");
    let chosen = resolve(&value);
    assert_eq!(chosen["outcome"]["selection"]["home"], OLD);
    assert_eq!(chosen["conflicts"], json!([]));
}

#[test]
fn toolchain_commands_cross_the_json_dispatch_boundary() {
    for (command, payload, key) in [
        (
            "toolchains.registry",
            input()["registry"].clone(),
            "registry",
        ),
        (
            "toolchains.resolvePins",
            json!({"document":local_pins(),"directory":"apps/web/src","tool":"java"}),
            "resolution",
        ),
        (
            "toolchains.checkVersion",
            json!({"registry":input()["registry"],"tool":"java","output":"openjdk version \"21.0.4\""}),
            "version",
        ),
        (
            "toolchains.discoveryPlan",
            json!({"registry":input()["registry"],"platform":"macos"}),
            "plan",
        ),
        ("toolchains.resolve", input(), "resolution"),
        ("toolchains.candidates", listing(&input()), "candidates"),
        (
            "toolchains.settingsView",
            json!({"tools":[{"id":"java","displayName":"JDK","pluginID":"dev.lithe.fixture","reason":"notFound"}],"query":{"filter":"problems"}}),
            "view",
        ),
    ] {
        let response: Value = serde_json::from_str(&crate::execute_json(
            &json!({"id":"toolchains","command":command,"payload":payload}).to_string(),
        ))
        .unwrap();
        assert_eq!(response["ok"], true, "{response}");
        assert_eq!(response["data"]["status"], "ok", "{response}");
        assert!(response["data"].get(key).is_some(), "{response}");
    }
}

#[test]
fn toolchain_resolution_automatic_uses_numeric_probe_not_display_order() {
    let mut value = input();
    installation(&mut value, HOME, "21.0.4");
    installation(&mut value, OLD, "9.0.99");
    value["systemInstallations"] = json!([{"tool":"java","home":OLD},{"tool":"java","home":HOME}]);
    let result = resolve(&value);
    assert_eq!(result["outcome"]["selection"]["home"], HOME);
    assert_eq!(result["outcome"]["selection"]["layer"], "automatic");
    value["minimumVersion"] = json!("22");
    assert_eq!(resolve(&value)["outcome"]["kind"], "skip");
}

#[test]
fn toolchain_resolution_equal_versions_are_ambiguous_and_manual_facts_not_automatic() {
    let mut value = input();
    installation(&mut value, HOME, "21.0.4");
    installation(&mut value, OLD, "21.0.4");
    assert_eq!(resolve(&value)["outcome"]["kind"], "skip");
    value["systemInstallations"] = json!([{"tool":"java","home":OLD},{"tool":"java","home":HOME}]);
    let result = resolve(&value);
    assert_eq!(result["outcome"]["kind"], "skip");
    assert_eq!(result["conflicts"][0]["scope"], "automatic");
    assert_eq!(result["conflicts"][0]["action"], "chooseInstallation");
    assert_eq!(
        result["conflicts"][0]["installations"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn toolchain_resolution_version_capture_is_definition_driven_for_java8_and_custom_tool() {
    let value = input();
    let checked=check_version_command(json!({"registry":value["registry"],"tool":"java","output":"java version \"1.8.0_422\"","minimumVersion":"8"}), validated_manifest_json).unwrap();
    assert_eq!(checked["status"], "ok", "{checked}");
    assert_eq!(checked["version"]["version"], "8.0");
    assert_eq!(checked["version"]["satisfied"], true);
    let mut registry = value["registry"].clone();
    registry["plugins"].as_array_mut().unwrap().truncate(1);
    let definition = &mut registry["plugins"][0]["manifest"]["toolDefinitions"][0];
    definition["id"] = json!("dev.lithe.fixture.java-support:renderer");
    definition["version"]["pattern"] = json!("release=([0-9]+(?:\\.[0-9]+)*)");
    let checked=check_version_command(json!({"registry":registry,"tool":"dev.lithe.fixture.java-support:renderer","output":"render release=10.2","minimumVersion":"9.99"}), validated_manifest_json).unwrap();
    assert_eq!(checked["status"], "ok", "{checked}");
    assert_eq!(checked["version"]["version"], "10.2");
    assert_eq!(checked["version"]["satisfied"], true);
}

#[test]
fn toolchain_resolution_discovery_comes_only_from_enabled_definition_data() {
    let mut value = input();
    let scans = json!([{"root":{"kind":"home"},"pattern":"runtimes/*.fixture","platforms":["macos"]},{"root":{"kind":"home"},"pattern":"other/*.fixture","platforms":["windows"]}]);
    value["registry"]["plugins"][0]["manifest"]["toolDefinitions"][0]["discovery"] = scans.clone();
    let result = discovery_plan_command(
        json!({"registry":value["registry"],"platform":"macos"}),
        validated_manifest_json,
    )
    .unwrap();
    assert_eq!(result["status"], "ok", "{result}");
    assert_eq!(result["plan"]["tools"][0]["scans"], json!([scans[0]]));
    assert_eq!(
        result["plan"]["tools"][0]["providers"],
        json!(["dev.lithe.fixture.mise:mise"])
    );
    value["registry"]["plugins"]
        .as_array_mut()
        .unwrap()
        .remove(0);
    let result = discovery_plan_command(
        json!({"registry":value["registry"],"platform":"macos"}),
        validated_manifest_json,
    )
    .unwrap();
    assert_eq!(result["plan"]["tools"], json!([]));
}

#[test]
fn toolchain_discovery_plan_carries_path_entries_and_platform_home_variables() {
    let result = discovery_plan_command(
        json!({"registry":input()["registry"],"platform":"macos"}),
        validated_manifest_json,
    )
    .unwrap();
    let tool = &result["plan"]["tools"][0];
    assert_eq!(tool["pathPrepend"], json!(["${home}/bin"]));
    assert_eq!(
        tool["environment"],
        json!([{"variable":"JAVA_HOME","path":"bin/java"}])
    );
    let result = discovery_plan_command(
        json!({"registry":input()["registry"],"platform":"windows"}),
        validated_manifest_json,
    )
    .unwrap();
    assert_eq!(
        result["plan"]["tools"][0]["environment"],
        json!([{"variable":"JAVA_HOME","path":"bin/java.exe"}])
    );
}

/// Narrows a resolver input to the settings listing's directory-free fields.
fn listing(input: &Value) -> Value {
    let mut value = json!({"tool":"java"});
    for key in [
        "registry",
        "platform",
        "workspaceRoot",
        "filesystem",
        "probes",
        "systemInstallations",
        "environment",
    ] {
        value[key] = input[key].clone();
    }
    if let Some(reported) = input.get("providerInstallations") {
        value["providerInstallations"] = reported.clone();
    }
    value
}

fn candidates(input: &Value) -> Value {
    let result = candidates_command(listing(input), validated_manifest_json).unwrap();
    assert_eq!(result["status"], "ok", "{result}");
    result["candidates"].clone()
}

#[test]
fn toolchain_candidates_return_probe_targets_before_versions_are_known() {
    let mut value = input();
    installation(&mut value, HOME, "21.0.4");
    installation(&mut value, OLD, "17.0.12");
    value["systemInstallations"] = json!([{"tool":"java","home":OLD},{"tool":"java","home":HOME},
        {"tool":"java","home":"/opt/lithe-fixtures/missing"}]);
    value["environment"]["JAVA_HOME"] = json!(OLD);
    value["probes"] = json!([]);
    let listed = candidates(&value);
    assert_eq!(listed["installations"], json!([]));
    assert_eq!(listed["selected"], Value::Null);
    let skipped = listed["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 3);
    let probe_targets: Vec<_> = skipped
        .iter()
        .filter(|row| row["reason"] == "probeFailed")
        .map(|row| row["executable"].clone())
        .collect();
    assert_eq!(
        probe_targets,
        vec![
            json!(format!("{OLD}/bin/java")),
            json!(format!("{HOME}/bin/java"))
        ]
    );
    let missing = skipped
        .iter()
        .find(|row| row["reason"] == "unsafePath")
        .unwrap();
    assert!(missing.get("executable").is_none());
    assert_eq!(listed["shell"][0]["executable"], format!("{OLD}/bin/java"));
    assert_eq!(listed["shell"][0]["reason"], "probeFailed");
}

#[test]
fn toolchain_candidates_list_versions_and_share_the_resolver_choice() {
    let mut value = input();
    installation(&mut value, HOME, "21.0.4");
    installation(&mut value, OLD, "17.0.12");
    value["systemInstallations"] = json!([{"tool":"java","home":OLD},{"tool":"java","home":HOME}]);
    value["environment"]["JAVA_HOME"] = json!(OLD);
    let listed = candidates(&value);
    assert_eq!(
        listed["installations"],
        json!([
            {"home":HOME,"executable":format!("{HOME}/bin/java"),"version":"21.0.4","provider":"host:system"},
            {"home":OLD,"executable":format!("{OLD}/bin/java"),"version":"17.0.12","provider":"host:system"}
        ])
    );
    assert_eq!(listed["selected"], HOME);
    assert_eq!(listed["skipped"], json!([]));
    assert_eq!(
        listed["shell"],
        json!([{"candidateID":"home","variable":"JAVA_HOME","home":OLD,
            "executable":format!("{OLD}/bin/java"),"version":"17.0.12"}])
    );
    // The login shell wins in resolution; the listing still reports the automatic choice.
    assert_eq!(resolve(&value)["outcome"]["selection"]["home"], OLD);
    value["environment"] = json!({});
    assert_eq!(resolve(&value)["outcome"]["selection"]["home"], HOME);
}

#[test]
fn toolchain_candidates_offer_shell_homes_that_no_discovery_scan_covers() {
    // A JDK reached only through JAVA_HOME or the PATH is still one the resolver runs.
    let mut value = input();
    installation(&mut value, HOME, "21.0.4");
    installation(&mut value, OLD, "17.0.12");
    value["environment"]["JAVA_HOME"] = json!(OLD);
    let mut listing = listing(&value);
    listing["commandHomes"] = json!({"java":HOME});
    let result = candidates_command(listing.clone(), validated_manifest_json).unwrap();
    assert_eq!(result["status"], "ok", "{result}");
    let listed = &result["candidates"];
    assert_eq!(
        listed["installations"],
        json!([
            {"home":HOME,"executable":format!("{HOME}/bin/java"),"version":"21.0.4","provider":"host:shell"},
            {"home":OLD,"executable":format!("{OLD}/bin/java"),"version":"17.0.12","provider":"host:shell"}
        ])
    );
    assert_eq!(
        listed["shell"],
        json!([
            {"candidateID":"home","variable":"JAVA_HOME","home":OLD,
                "executable":format!("{OLD}/bin/java"),"version":"17.0.12"},
            {"candidateID":"shell","command":"java","home":HOME,
                "executable":format!("{HOME}/bin/java"),"version":"21.0.4"}
        ])
    );
    // Shell homes are choices, not the automatic layer's own installations.
    assert_eq!(listed["selected"], Value::Null);

    // One home reached through both routes, or also scanned, is listed once and keeps
    // the scan's attribution.
    listing["commandHomes"] = json!({"java":OLD});
    listing["systemInstallations"] = json!([{"tool":"java","home":OLD}]);
    let listed =
        candidates_command(listing.clone(), validated_manifest_json).unwrap()["candidates"].clone();
    assert_eq!(
        listed["installations"],
        json!([{"home":OLD,"executable":format!("{OLD}/bin/java"),"version":"17.0.12","provider":"host:system"}])
    );

    // A shell home that fails its check is reported but never offered.
    listing["systemInstallations"] = json!([]);
    listing["minimumVersion"] = json!("21");
    let listed =
        candidates_command(listing.clone(), validated_manifest_json).unwrap()["candidates"].clone();
    assert_eq!(listed["installations"], json!([]));
    assert_eq!(listed["shell"][0]["reason"], "versionUnsatisfied");

    listing["commandHomes"] = json!({"java":7});
    assert_eq!(
        candidates_command(listing, validated_manifest_json).unwrap()["status"],
        "rejected"
    );
}

#[test]
fn toolchain_candidates_keep_provider_attribution_and_count_one_home_once() {
    let mut value = input();
    self_provided(&mut value);
    installation(&mut value, HOME, "21.0.4");
    installation(&mut value, OLD, "17.0.12");
    let provider = "dev.lithe.fixture.java-support:installs";
    value["providerInstallations"] = json!([{"tool":"java","home":HOME,"provider":provider}]);
    value["systemInstallations"] = json!([{"tool":"java","home":HOME},{"tool":"java","home":OLD}]);
    let listed = candidates(&value);
    let installations = listed["installations"].as_array().unwrap();
    assert_eq!(installations.len(), 2);
    assert_eq!(installations[0]["home"], HOME);
    assert_eq!(installations[0]["provider"], provider);
    assert_eq!(installations[1]["provider"], "host:system");
    assert_eq!(listed["selected"], HOME);
}

#[test]
fn toolchain_candidates_refuse_a_provider_outside_the_registry() {
    let mut value = input();
    self_provided(&mut value);
    for provider in ["dev.example.other:installs", "installs", "host:system"] {
        value["providerInstallations"] = json!([{"tool":"java","home":HOME,"provider":provider}]);
        let result = candidates_command(listing(&value), validated_manifest_json).unwrap();
        assert_eq!(result["status"], "rejected", "{provider}");
    }
}

#[test]
fn toolchain_candidates_report_equal_versions_as_a_conflict() {
    let mut value = input();
    installation(&mut value, HOME, "21.0.4");
    installation(&mut value, OLD, "21.0.4");
    value["systemInstallations"] = json!([{"tool":"java","home":OLD},{"tool":"java","home":HOME}]);
    let listed = candidates(&value);
    assert_eq!(listed["selected"], Value::Null);
    assert_eq!(listed["conflicts"][0]["action"], "chooseInstallation");
    assert_eq!(listed["installations"].as_array().unwrap().len(), 2);
    let mut unknown = listing(&value);
    unknown["tool"] = json!("dev.lithe.fixture.java-support:missing");
    assert_eq!(
        candidates_command(unknown, validated_manifest_json).unwrap()["status"],
        "rejected"
    );
    let mut directory = listing(&value);
    directory["directory"] = json!(".");
    assert_eq!(
        candidates_command(directory, validated_manifest_json).unwrap()["status"],
        "rejected"
    );
}

/// Turns the definition plugin into its own installation-only provider, without scans.
fn self_provided(input: &mut Value) {
    let manifest = &mut input["registry"]["plugins"][0]["manifest"];
    manifest["toolDefinitions"][0]
        .as_object_mut()
        .unwrap()
        .remove("discovery");
    manifest["requires"] = json!({"toolDefinitions":0,"toolchainProviders":0,"backend":0});
    manifest["permissions"] = json!(["backend.launch", "toolchains.provide"]);
    manifest["toolchainProviders"] =
        json!([{"id":"installs","displayName":"Installed releases","tools":["java"]}]);
    manifest["backend"] = json!({"launch":{"kind":"packaged","path":"bin/installs"}});
    input["registry"]["plugins"]
        .as_array_mut()
        .unwrap()
        .truncate(1);
}

/// Captures an installation-only answer from the self-provider for `homes`.
fn installs_query(input: &Value, homes: &[(&str, &str)]) -> Value {
    let mut value = query(input);
    value["pluginID"] = json!("dev.lithe.fixture.java-support");
    value["request"]["params"]["providerID"] = json!("installs");
    let result = &mut value["response"]["result"];
    result["providerID"] = json!("installs");
    result["installations"] = json!(homes
        .iter()
        .map(|(home, version)| json!({"tool":"java","version":version,"home":home}))
        .collect::<Vec<_>>());
    result["statements"] = json!([]);
    result["watch"] = json!([]);
    value
}

/// A provider may name a version manager's alias link. The host reads only the paths the
/// reply and definition name, under the link; Core derives the canonical target facts.
#[test]
fn toolchain_resolution_accepts_a_provider_home_reached_through_an_alias_link() {
    let mut value = input();
    self_provided(&mut value);
    installation(&mut value, HOME, "21.0.4");
    let alias = "/opt/lithe-fixtures/installs/java/latest";
    let facts = value["filesystem"].as_array_mut().unwrap();
    facts.retain(|fact| !fact["path"].as_str().unwrap().starts_with(HOME));
    facts.push(json!({"path":alias,"canonicalPath":HOME,"kind":"directory"}));
    facts.push(json!({"path":format!("{alias}/bin"),"canonicalPath":format!("{HOME}/bin"),"kind":"directory"}));
    facts.push(
        json!({"path":format!("{alias}/bin/java"),"canonicalPath":format!("{HOME}/bin/java"),
        "kind":"file","executable":true}),
    );
    value["queries"] = json!([installs_query(&value, &[(alias, "21.0.4")])]);
    let result = resolve(&value);
    assert_eq!(result["outcome"]["selection"]["home"], HOME, "{result}");
    assert_eq!(
        result["outcome"]["selection"]["executable"],
        format!("{HOME}/bin/java")
    );
}

/// A multi-call program picks its tool from the name it starts with: rustup links `cargo`
/// to itself and runs cargo, but started as `rustup` it runs rustup. The selection keeps
/// the layout's file name beneath the canonical home instead of the link target.
#[test]
fn toolchain_resolution_starts_a_multi_call_link_by_its_layout_name() {
    let mut value = input();
    installation(&mut value, HOME, "21.0.4");
    let link = format!("{HOME}/bin/java");
    let target = format!("{HOME}/bin/multiplexer");
    let facts = value["filesystem"].as_array_mut().unwrap();
    facts.retain(|fact| fact["path"] != link.as_str());
    facts.push(json!({"path":link,"canonicalPath":target,"kind":"file","executable":true}));
    value["fixed"] = json!({"home":HOME});
    let mut unprobed = value.clone();
    unprobed["probes"] = json!([]);
    let candidates =
        candidates_command(value_for_candidates(&unprobed), validated_manifest_json).unwrap();
    assert!(
        candidates["candidates"]["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["reason"] == "probeFailed" && row["executable"] == link.as_str()),
        "{candidates}"
    );
    // Started by its target's name the program answers as itself.
    value["probes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"executable":target,"output":"multiplexer 1.28.2","exitCode":0}));
    let result = resolve(&value);
    let selected = &result["outcome"]["selection"];
    assert_eq!(selected["home"], HOME, "{result}");
    assert_eq!(selected["executable"], link.as_str());
    assert_eq!(selected["version"], "21.0.4");
}

/// An environment outside the workspace whose program links to its base installation,
/// as `python -m venv` makes one on macOS and Linux.
const ENVIRONMENT: &str = "/opt/lithe-fixtures/environments/app";

/// Lets the fixture definition's program link outside its home.
fn links_outside_home(value: &mut Value) {
    value["registry"]["plugins"][0]["manifest"]["toolDefinitions"][0]
        ["executableLinksOutsideHome"] = json!(true);
}

/// Records `home`, its `bin` directory resolving to `bin`, and `bin/java` resolving to
/// `target`, the facts a host observes along the layout path, with a probe of the
/// layout path.
fn linked_home(value: &mut Value, home: &str, bin: &str, target: &str) {
    value["filesystem"].as_array_mut().unwrap().extend([
        fact(home, "directory", false),
        json!({"path":format!("{home}/bin"),"canonicalPath":bin,"kind":"directory"}),
        json!({"path":format!("{home}/bin/java"),"canonicalPath":target,"kind":"file","executable":true}),
    ]);
    value["probes"].as_array_mut().unwrap().push(
        json!({"executable":format!("{home}/bin/java"),"output":"openjdk version \"21.0.4\"","exitCode":0}),
    );
}

/// The reason a resolution with only an explicit `{home}` choice fails.
fn fixed_home_failure(value: &Value, home: &str) -> Value {
    let mut value = value.clone();
    value["fixed"] = json!({ "home": home });
    let result = resolve(&value);
    assert_eq!(result["outcome"]["kind"], "terminalFailure", "{result}");
    result["outcome"]["reason"]["code"].clone()
}

/// The environment's program starts from its layout path, not the base program it links
/// to: Python finds `pyvenv.cfg` from the path it starts with, so the target would drop
/// the environment. Without the field the same link is refused.
#[test]
fn toolchain_resolution_starts_an_environment_linked_outside_its_home_from_its_layout() {
    let mut value = input();
    installation(&mut value, HOME, "21.0.4");
    let program = format!("{ENVIRONMENT}/bin/java");
    linked_home(
        &mut value,
        ENVIRONMENT,
        &format!("{ENVIRONMENT}/bin"),
        &format!("{HOME}/bin/java"),
    );
    assert_eq!(fixed_home_failure(&value, ENVIRONMENT), "unsafePath");

    links_outside_home(&mut value);
    let mut unprobed = value.clone();
    unprobed["fixed"] = json!({ "home": ENVIRONMENT });
    unprobed["probes"] = json!([]);
    let listed =
        candidates_command(value_for_candidates(&unprobed), validated_manifest_json).unwrap();
    assert!(
        listed["candidates"]["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["reason"] == "probeFailed" && row["executable"] == program.as_str()),
        "{listed}"
    );
    for (key, choice) in [
        ("fixed", json!({ "home": ENVIRONMENT })),
        ("environment", json!({ "JAVA_HOME": ENVIRONMENT })),
    ] {
        let mut routed = value.clone();
        routed[key] = choice;
        let result = resolve(&routed);
        let selected = &result["outcome"]["selection"];
        assert_eq!(selected["home"], ENVIRONMENT, "{key}: {result}");
        assert_eq!(selected["executable"], program.as_str(), "{key}");
        assert_eq!(
            selected["environment"],
            json!({"JAVA_HOME":ENVIRONMENT,"LITHE_JAVA_PATH":program}),
            "{key}"
        );
        assert_eq!(
            selected["pathPrepend"],
            json!([format!("{ENVIRONMENT}/bin")]),
            "{key}"
        );
    }
}

/// Homebrew's `libexec/bin/python3` links to its framework, a sibling of `libexec`.
/// Discovery and a pin through the `opt` alias both select the canonical home and start
/// the program from the layout path beneath it.
#[test]
fn toolchain_resolution_accepts_a_program_linked_to_a_sibling_of_its_home() {
    let keg = "/opt/lithe-fixtures/cellar/tool/21.0.4";
    let home = format!("{keg}/libexec");
    let alias = "/opt/lithe-fixtures/opt/tool/libexec";
    let target = format!("{keg}/Frameworks/bin/java21");
    let mut value = input();
    links_outside_home(&mut value);
    linked_home(&mut value, &home, &format!("{home}/bin"), &target);
    value["filesystem"].as_array_mut().unwrap().extend([
        json!({"path":alias,"canonicalPath":home,"kind":"directory"}),
        json!({"path":format!("{alias}/bin"),"canonicalPath":format!("{home}/bin"),"kind":"directory"}),
        json!({"path":format!("{alias}/bin/java"),"canonicalPath":target,"kind":"file","executable":true}),
    ]);
    let program = format!("{home}/bin/java");
    value["systemInstallations"] = json!([{"tool":"java","home":home}]);
    let discovered = resolve(&value);
    assert_eq!(
        discovered["outcome"]["selection"]["executable"],
        program.as_str(),
        "{discovered}"
    );
    value["systemInstallations"] = json!([]);
    value["local"] = json!({"version":1,"toolchains":{},"pins":{".":{"java":{"home":alias,"version":"21","provider":"host:manual"}}}});
    let pinned = resolve(&value);
    let selected = &pinned["outcome"]["selection"];
    assert_eq!(selected["home"], home.as_str(), "{pinned}");
    assert_eq!(selected["executable"], program.as_str());
}

/// The field lets only the program leave the home: a target in the workspace, a `bin`
/// directory that links elsewhere and a `bin` directory the host did not observe are
/// all refused before any probe.
#[test]
fn toolchain_resolution_refuses_links_outside_the_home_that_reach_the_workspace_or_skip_bin() {
    let elsewhere = "/opt/lithe-fixtures/elsewhere/bin";
    let cases = [
        (
            format!("{ENVIRONMENT}/bin"),
            format!("{WORKSPACE}/tools/java"),
            true,
        ),
        (elsewhere.to_owned(), format!("{elsewhere}/java"), true),
        (
            format!("{ENVIRONMENT}/bin"),
            format!("{HOME}/bin/java"),
            false,
        ),
    ];
    for (bin, target, observed) in cases {
        let mut value = input();
        links_outside_home(&mut value);
        installation(&mut value, HOME, "21.0.4");
        linked_home(&mut value, ENVIRONMENT, &bin, &target);
        value["filesystem"].as_array_mut().unwrap().extend([
            fact(&format!("{WORKSPACE}/tools/java"), "file", true),
            fact(elsewhere, "directory", false),
            fact(&format!("{elsewhere}/java"), "file", true),
        ]);
        if !observed {
            value["filesystem"]
                .as_array_mut()
                .unwrap()
                .retain(|fact| fact["path"] != format!("{ENVIRONMENT}/bin").as_str());
        }
        assert_eq!(
            fixed_home_failure(&value, ENVIRONMENT),
            "unsafePath",
            "{bin} -> {target}"
        );
    }
}

#[test]
fn toolchain_resolution_definition_plugin_can_be_its_own_installation_provider() {
    let mut value = input();
    self_provided(&mut value);
    installation(&mut value, HOME, "21.0.4");
    let plan = discovery_plan_command(
        json!({"registry":value["registry"],"platform":"macos"}),
        validated_manifest_json,
    )
    .unwrap();
    assert_eq!(plan["plan"]["tools"][0]["scans"], json!([]));
    assert_eq!(
        plan["plan"]["tools"][0]["providers"],
        json!(["dev.lithe.fixture.java-support:installs"])
    );
    value["queries"] = json!([installs_query(&value, &[(HOME, "21.0.4")])]);
    let result = resolve(&value);
    assert_eq!(result["outcome"]["selection"]["home"], HOME);
    assert_eq!(result["outcome"]["selection"]["layer"], "automatic");
    assert_eq!(
        result["outcome"]["selection"]["provider"],
        "dev.lithe.fixture.java-support:installs"
    );
    // Without watched files no reader can anchor a requirement statement.
    value["filesystem"].as_array_mut().unwrap().push(fact(
        &format!("{WORKSPACE}/mise.toml"),
        "file",
        false,
    ));
    value["queries"][0]["filesystem"] = value["filesystem"].clone();
    value["queries"][0]["response"]["result"]["statements"] =
        query(&value)["response"]["result"]["statements"].clone();
    let rejected = resolve_command(value, validated_manifest_json).unwrap();
    assert_eq!(rejected["status"], "rejected");
    assert_eq!(rejected["error"]["code"], "plugin.toolchain.invalidReply");
    assert_eq!(
        rejected["error"]["field"],
        "response.result.statements.reader"
    );
}

#[test]
fn toolchain_resolution_scan_and_provider_routes_merge_by_canonical_home() {
    let mut value = input();
    self_provided(&mut value);
    installation(&mut value, HOME, "21.0.4");
    installation(&mut value, OLD, "21.0.4");
    value["queries"] = json!([installs_query(&value, &[(HOME, "21.0.4")])]);
    value["systemInstallations"] = json!([{"tool":"java","home":HOME}]);
    let result = resolve(&value);
    assert_eq!(result["outcome"]["selection"]["home"], HOME);
    assert_eq!(
        result["outcome"]["selection"]["provider"],
        "dev.lithe.fixture.java-support:installs"
    );
    assert_eq!(result["conflicts"], json!([]));
    value["systemInstallations"] = json!([{"tool":"java","home":OLD},{"tool":"java","home":HOME}]);
    let result = resolve(&value);
    assert_eq!(result["outcome"]["kind"], "skip");
    assert_eq!(
        result["conflicts"][0]["providers"],
        json!(["dev.lithe.fixture.java-support:installs", "host:system"])
    );
    value["providerChoices"]["automatic"] = json!(OLD);
    let chosen = resolve(&value);
    assert_eq!(chosen["outcome"]["selection"]["home"], OLD);
    assert_eq!(chosen["outcome"]["selection"]["provider"], "host:system");
}

#[test]
fn toolchain_resolution_linked_provider_home_resolves_against_the_same_snapshot() {
    let mut value = input();
    self_provided(&mut value);
    let link = "/opt/lithe-fixtures/installs/java/latest";
    let alias = |path: &str, target: &str, kind: &str, executable: bool| json!({"path":path,"canonicalPath":target,"kind":kind,"executable":executable});
    // Only the link spellings are observed; the resolved target has no fact of its own.
    value["filesystem"].as_array_mut().unwrap().extend([
        alias(link, HOME, "directory", false),
        alias(
            &format!("{link}/bin"),
            &format!("{HOME}/bin"),
            "directory",
            false,
        ),
        alias(
            &format!("{link}/bin/java"),
            &format!("{HOME}/bin/java"),
            "file",
            true,
        ),
    ]);
    value["probes"] = json!([{"executable":format!("{HOME}/bin/java"),"output":"openjdk version \"21.0.4\"","exitCode":0}]);
    value["queries"] = json!([installs_query(&value, &[(link, "21.0.4")])]);
    let result = resolve(&value);
    let selection = &result["outcome"]["selection"];
    assert_eq!(selection["home"], HOME);
    assert_eq!(selection["executable"], format!("{HOME}/bin/java"));
    assert_eq!(selection["pathPrepend"], json!([format!("{HOME}/bin")]));
    // Facts that disagree about the resolved target are not reconciled.
    value["filesystem"]
        .as_array_mut()
        .unwrap()
        .push(fact(HOME, "file", false));
    value["queries"][0]["filesystem"] = value["filesystem"].clone();
    assert_eq!(
        resolve_command(value, validated_manifest_json).unwrap()["status"],
        "rejected"
    );
}

#[test]
fn toolchain_resolution_environment_candidate_uses_its_declared_layout() {
    let mut value = input();
    let definition = &mut value["registry"]["plugins"][0]["manifest"]["toolDefinitions"][0];
    definition["executableLayouts"]
        .as_array_mut()
        .unwrap()
        .push(json!({"path":"alt/bin/java","platforms":["macos","linux"]}));
    definition["executables"][4]["path"] = json!("alt/bin/java");
    installation(&mut value, HOME, "21.0.4");
    value["filesystem"].as_array_mut().unwrap().extend([
        fact(&format!("{HOME}/alt"), "directory", false),
        fact(&format!("{HOME}/alt/bin"), "directory", false),
        fact(&format!("{HOME}/alt/bin/java"), "file", true),
    ]);
    value["probes"].as_array_mut().unwrap().push(json!({"executable":format!("{HOME}/alt/bin/java"),"output":"openjdk version \"17.0.12\"","exitCode":0}));
    value["environment"]["JAVA_HOME"] = json!(HOME);
    let result = resolve(&value);
    let selection = &result["outcome"]["selection"];
    assert_eq!(selection["provider"], "host:shell");
    assert_eq!(selection["executable"], format!("{HOME}/alt/bin/java"));
    assert_eq!(selection["version"], "17.0.12");
}

#[test]
fn toolchain_resolution_disabled_definition_cannot_reactivate_a_saved_pin() {
    let mut value = input();
    installation(&mut value, HOME, "21.0.4");
    value["local"] = local_pins();
    value["registry"]["plugins"]
        .as_array_mut()
        .unwrap()
        .remove(0);
    value["commandHomes"]["java"] = json!(HOME);
    let before = value["local"].clone();
    let result = resolve(&value);
    assert_eq!(result["outcome"]["kind"], "skip");
    assert_eq!(result["outcome"]["reason"]["code"], "definitionUnavailable");
    assert!(result["outcome"].get("selection").is_none());
    assert_eq!(value["local"], before);
}

/// A short ID claimed by two enabled plugins has no owner, so every consumer sees no definition.
#[test]
fn toolchain_resolution_contested_tool_id_is_unowned_for_every_consumer() {
    let mut value = input();
    let tool = value["tool"].as_str().unwrap().to_owned();
    installation(&mut value, HOME, "21.0.4");
    value["local"] = local_pins();
    value["commandHomes"][tool.as_str()] = json!(HOME);
    // The rival is a copy of the defining plugin under another ID.
    let mut rival = value["registry"]["plugins"][0]["manifest"].clone();
    rival["id"] = json!("io.example.rival");
    value["registry"]["plugins"]
        .as_array_mut()
        .unwrap()
        .push(json!({"manifest":rival}));
    let plan = discovery_plan_command(
        json!({"registry":value["registry"],"platform":"macos"}),
        validated_manifest_json,
    )
    .unwrap();
    assert_eq!(plan["status"], "ok", "{plan}");
    assert_eq!(plan["plan"]["tools"], json!([]));
    let request = json!({"registry":value["registry"],"tool":tool,"output":"21.0.4"});
    let checked = check_version_command(request, validated_manifest_json).unwrap();
    assert_eq!(checked["status"], "rejected", "{checked}");
    let result = resolve(&value);
    assert_eq!(result["outcome"]["kind"], "skip");
    assert_eq!(result["outcome"]["reason"]["code"], "definitionUnavailable");
}

#[test]
fn toolchain_resolution_nearest_unmet_project_source_masks_parent_requirement() {
    let mut value = input();
    installation(&mut value, HOME, "21.0.4");
    installation(&mut value, OLD, "17.0.12");
    value["directory"] = json!("apps/web");
    for (path, kind) in [
        (format!("{WORKSPACE}/apps/web"), "directory"),
        (format!("{WORKSPACE}/apps/web/mise.toml"), "file"),
        (format!("{WORKSPACE}/mise.toml"), "file"),
    ] {
        value["filesystem"]
            .as_array_mut()
            .unwrap()
            .push(fact(&path, kind, false));
    }
    let mut backend = query(&value);
    backend["request"]["params"]["workspace"]["directory"] = json!("apps/web");
    backend["response"]["result"]["statements"][0]["installation"] = json!(OLD);
    backend["response"]["result"]["installations"][0]["home"] = json!(OLD);
    backend["response"]["result"]["statements"]
        .as_array_mut()
        .unwrap()
        .push(json!({"tool":"java","scope":"project","source":format!("{WORKSPACE}/apps/web/mise.toml"),
            "request":"99","installation":null,"kind":"declaration","reader":"native"}));
    value["queries"] = json!([backend]);
    value["commandHomes"]["java"] = json!(HOME);
    let result = resolve(&value);
    assert_eq!(result["outcome"]["selection"]["home"], HOME);
    assert_eq!(result["outcome"]["selection"]["layer"], "global");
    assert!(result["unmet"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason["request"] == "99"));
}

#[test]
fn toolchain_resolution_definition_cannot_omit_away_an_explicit_invalid_pin() {
    let mut value = input();
    value["registry"]["plugins"][0]["manifest"]["toolDefinitions"][0]["executables"]
        .as_array_mut()
        .unwrap()
        .remove(0);
    installation(&mut value, HOME, "21.0.4");
    value["local"] = json!({"version":1,"toolchains":{},"pins":{".":{"java":{
        "home":"/opt/lithe-fixtures/unavailable","version":"99","provider":"host:manual"}}}});
    value["commandHomes"]["java"] = json!(HOME);
    let result = resolve(&value);
    assert_eq!(result["outcome"]["kind"], "terminalFailure");
    assert!(result["outcome"].get("selection").is_none());
}

#[test]
fn toolchain_resolution_wrapper_requires_action_and_never_invents_installation_home() {
    let mut value = input();
    let definition = &mut value["registry"]["plugins"][0]["manifest"]["toolDefinitions"][0];
    definition["wrappers"] = json!([{"path":"fixture-wrapper","platforms":["macos"]}]);
    definition["executables"].as_array_mut().unwrap().insert(
        1,
        json!({"id":"wrapper","kind":"projectFile","path":"fixture-wrapper","platforms":["macos"]}),
    );
    let wrapper = format!("{WORKSPACE}/fixture-wrapper");
    value["filesystem"]
        .as_array_mut()
        .unwrap()
        .push(fact(&wrapper, "file", true));
    let blocked = resolve(&value);
    assert_eq!(blocked["outcome"]["kind"], "skip");
    assert!(blocked["trace"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["reason"]["code"] == "projectCodeRequiresAction"));
    // No probe observation is supplied: passive resolution cannot require running it.
    assert_eq!(value["probes"], json!([]));
    value["projectCodeAllowed"] = json!(true);
    let allowed = resolve(&value);
    let selection = &allowed["outcome"]["selection"];
    assert_eq!(allowed["outcome"]["kind"], "resolved");
    assert_eq!(selection["home"], Value::Null);
    assert_eq!(selection["version"], Value::Null);
    assert_eq!(selection["projectCode"], true);
    assert!(selection["environment"].get("JAVA_HOME").is_none());
    assert_eq!(selection["environment"]["LITHE_JAVA_PATH"], wrapper);
    assert_eq!(selection["pathPrepend"], json!([]));
}

#[test]
fn toolchain_resolution_review_reports_the_first_project_failure_and_still_falls_back() {
    let mut value = input();
    let definition = &mut value["registry"]["plugins"][0]["manifest"]["toolDefinitions"][0];
    definition["wrappers"] = json!([{"path":"fixture-wrapper","platforms":["macos"]}]);
    definition["executables"].as_array_mut().unwrap().splice(
        1..1,
        [
            json!({"id":"wrapper","kind":"projectFile","path":"fixture-wrapper","platforms":["macos"]}),
            json!({"id":"env","kind":"projectFile","projectKind":"environment","home":"env","path":"env/bin/java","platforms":["macos","linux"]}),
        ],
    );
    let executable = format!("{WORKSPACE}/env/bin/java");
    value["projectCodeAllowed"] = json!(true);
    value["filesystem"].as_array_mut().unwrap().extend([
        fact(&format!("{WORKSPACE}/fixture-wrapper"), "file", false),
        fact(&format!("{WORKSPACE}/env"), "directory", false),
        fact(&format!("{WORKSPACE}/env/bin"), "directory", false),
        fact(&executable, "file", true),
    ]);

    let skipped = resolve(&value);
    assert_eq!(skipped["outcome"]["reason"]["code"], "notExecutable");
    assert_eq!(skipped["projectCandidates"][0]["reason"], "notExecutable");
    assert_eq!(skipped["projectCandidates"][1]["reason"], "probeFailed");

    value["probes"] = json!([{
        "executable":executable,
        "output":"openjdk version \"21.0.4\"",
        "exitCode":0
    }]);
    assert_eq!(
        resolve(&value)["outcome"]["selection"]["executable"],
        executable
    );
}

#[test]
fn toolchain_resolution_review_subdirectory_masks_wrappers_at_filesystem_roots() {
    for (platform, root) in [
        ("macos", "/"),
        ("windows", "C:/"),
        ("windows", "//server/share/"),
    ] {
        let mut value = input();
        let definition = &mut value["registry"]["plugins"][0]["manifest"]["toolDefinitions"][0];
        let runner = "fixture-wrapper";
        definition["wrappers"] = json!([{"path":runner,"platforms":[platform]}]);
        definition["executables"].as_array_mut().unwrap().insert(
            1,
            json!({"id":"wrapper","kind":"projectFile","path":runner,"platforms":[platform]}),
        );
        value["platform"] = json!(platform);
        value["workspaceRoot"] = json!(root);
        value["directory"] = json!("sub");
        value["projectCodeAllowed"] = json!(true);
        let child = format!("{root}sub");
        let executable = format!("{child}/{runner}");
        value["filesystem"] = json!([
            fact(root, "directory", false),
            fact(&format!("{root}{runner}"), "file", true),
            fact(&child, "directory", false),
            fact(&executable, "file", false)
        ]);
        let result = resolve(&value);
        assert_eq!(
            result["outcome"]["reason"]["code"], "notExecutable",
            "{root}"
        );
        assert_eq!(
            result["projectCandidates"],
            json!([{"candidateID":"wrapper","projectKind":"wrapper","path":executable,"reason":"notExecutable"}]),
            "{root}"
        );
    }
}

#[test]
fn toolchain_resolution_project_environment_home_cannot_symlink_out_of_workspace() {
    let mut value = input();
    let definition = &mut value["registry"]["plugins"][0]["manifest"]["toolDefinitions"][0];
    definition["executables"].as_array_mut().unwrap().insert(
        1,
        json!({"id":"project-env","kind":"projectFile","projectKind":"environment","home":".venv","path":".venv/bin/java","platforms":["macos","linux"]}),
    );
    let home = format!("{WORKSPACE}/.venv");
    let executable = format!("{home}/bin/java");
    value["filesystem"].as_array_mut().unwrap().extend([
        json!({"path":home,"canonicalPath":"/opt/lithe-fixtures/outside-environment","kind":"directory","executable":false}),
        fact(&executable,"file",true),
    ]);
    value["projectCodeAllowed"] = json!(true);
    let result = resolve(&value);
    assert_eq!(result["outcome"]["kind"], "skip");
    assert!(result["outcome"].get("selection").is_none());
    assert!(result["trace"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["reason"]["code"] == "unsafePath"));
}

#[test]
fn consumer_preferences_json_dispatcher_preserves_the_value_boundary() {
    let payload = json!({"document":{"version":1,"workspace":{"tools":{"dev.example.runtime:sdk":{"home":"/fixture/sdk-one"}},
        "consumers":{"dev.example.build/engine":{"dev.example.runtime:sdk":{"inherit":"workspace"}}}},
        "directories":{"nested":{"tools":{"dev.example.runtime:sdk":{"home":"/fixture/directory-sdk"}}}}},
        "tool":"dev.example.runtime:sdk","directory":"nested/child","consumer":"dev.example.build/engine"});
    let response: Value = serde_json::from_str(&crate::execute_json(&json!({
        "id":"consumer-preferences","command":"toolchains.resolveConsumerPreferences","payload":payload
    }).to_string())).unwrap();
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(
        response["data"]["preferences"]["fixed"],
        json!({"home":"/fixture/sdk-one"})
    );
}

#[test]
fn global_toolchain_discovery_and_defaults_need_no_project_directory() {
    let mut value = input();
    value["global"] = json!(true);
    value["workspaceRoot"] = json!("");
    value["filesystem"] = json!([]);
    installation(&mut value, HOME, "21.0.4");
    value["systemInstallations"] = json!([{"tool":"java","home":HOME}]);
    value["globalDefault"] = json!({"home":HOME});
    let result = resolve_command(value.clone(), validated_manifest_json).unwrap();
    assert_eq!(result["status"], "ok", "{result}");
    assert_eq!(result["resolution"]["outcome"]["selection"]["home"], HOME);
    let mut listing = value_for_candidates(&value);
    listing["global"] = json!(true);
    let candidates = candidates_command(listing, validated_manifest_json).unwrap();
    assert_eq!(candidates["status"], "ok", "{candidates}");
    assert_eq!(candidates["candidates"]["installations"][0]["home"], HOME);
}

#[test]
fn global_toolchain_resolution_binds_provider_replies_to_their_scope() {
    let scope: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/toolchains/global-query-resolution.json"
    ))
    .unwrap();
    let mut value = input();
    self_provided(&mut value);
    for field in ["directory", "global", "workspaceRoot", "context"] {
        value[field] = scope[field].clone();
    }
    value["filesystem"] = json!([]);
    installation(&mut value, HOME, "21.0.4");
    let mut backend = installs_query(&value, &[(HOME, "21.0.4")]);
    let workspace_scope = backend["request"]["params"]["workspace"].take();
    let workspace_context = backend["request"]["params"]["context"].take();
    for field in ["context", "workspace"] {
        backend["request"]["params"][field] = scope["queryScope"][field].clone();
    }
    value["queries"] = json!([backend]);
    let selection = &resolve(&value)["outcome"]["selection"];
    assert_eq!(selection["home"], HOME);
    assert_eq!(selection["layer"], scope["expectedLayer"]);
    assert_eq!(selection["provider"], scope["expectedProvider"]);

    let mut with_context = value.clone();
    with_context["context"] = workspace_context.clone();
    with_context["queries"][0]["request"]["params"]["context"] = workspace_context;
    let mut with_workspace = value.clone();
    with_workspace["queries"][0]["request"]["params"]["workspace"] = workspace_scope;
    let mut in_workspace = value.clone();
    in_workspace["global"] = json!(false);
    in_workspace["workspaceRoot"] = json!(WORKSPACE);
    in_workspace["filesystem"]
        .as_array_mut()
        .unwrap()
        .push(fact(WORKSPACE, "directory", false));
    in_workspace["queries"][0]["filesystem"] = in_workspace["filesystem"].clone();
    for rejected in [with_context, with_workspace, in_workspace] {
        let result = resolve_command(rejected, validated_manifest_json).unwrap();
        assert_eq!(result["status"], "rejected", "{result}");
        assert_eq!(result["error"]["code"], "plugin.toolchain.invalidParams");
    }
}

#[test]
fn global_toolchain_mode_refuses_project_pins_and_ignores_project_executables() {
    let mut value = input();
    value["global"] = json!(true);
    value["workspaceRoot"] = json!("");
    value["filesystem"] = json!([]);
    installation(&mut value, HOME, "21.0.4");
    value["systemInstallations"] = json!([{"tool":"java","home":HOME}]);
    value["globalDefault"] = json!({"home":HOME});
    value["registry"]["plugins"][0]["manifest"]["toolDefinitions"][0]["executables"].as_array_mut().unwrap().insert(1,
        json!({"id":"project-env","kind":"projectFile","projectKind":"environment","home":".venv","path":".venv/bin/java","platforms":["macos","linux"]}));
    value["projectCodeAllowed"] = json!(true);
    let result = resolve(&value);
    assert_eq!(result["outcome"]["selection"]["home"], HOME);
    assert!(!result["trace"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["candidate"] == "project-env"));
    value["local"] = local_pins();
    let rejected = resolve_command(value, validated_manifest_json).unwrap();
    assert_eq!(rejected["status"], "rejected", "{rejected}");
}

#[test]
fn toolchain_discovery_plan_exposes_platform_filtered_project_files() {
    let mut value = input();
    let definition = &mut value["registry"]["plugins"][0]["manifest"]["toolDefinitions"][0];
    definition["executableLayouts"] = json!([
        {"path":"bin/java","platforms":["macos","linux"]},
        {"path":"Scripts/java.exe","platforms":["windows"]},
        {"path":"bin/java.exe","platforms":["windows"]}
    ]);
    definition["wrappers"] = json!([
        {"path":"mvnw","requires":[".mvn/wrapper/maven-wrapper.properties"],"platforms":["macos","linux"]},
        {"path":"mvnw.cmd","platforms":["windows"]}
    ]);
    definition["executables"] = json!([
        {"id":"pinned","kind":"setting","settingID":"executable","layer":"project"},
        {"id":"wrapper-unix","kind":"projectFile","path":"mvnw","requires":[".mvn/wrapper/maven-wrapper.properties"],"platforms":["macos","linux"]},
        {"id":"wrapper-win","kind":"projectFile","path":"mvnw.cmd","platforms":["windows"]},
        {"id":"venv-unix","kind":"projectFile","projectKind":"environment","home":".venv","path":".venv/bin/java","requires":[".venv/pyvenv.cfg"],"platforms":["macos","linux"]},
        {"id":"venv-win","kind":"projectFile","projectKind":"environment","home":".venv","path":".venv/Scripts/java.exe","requires":[".venv/pyvenv.cfg"],"platforms":["windows"]},
        {"id":"shell","kind":"command","command":"java"},
        {"id":"home","kind":"environment","variable":"JAVA_HOME","path":"bin/java","platforms":["macos","linux"]},
        {"id":"home-windows","kind":"environment","variable":"JAVA_HOME","path":"bin/java.exe","platforms":["windows"]}
    ]);
    let macos = discovery_plan_command(
        json!({"registry":value["registry"],"platform":"macos"}),
        validated_manifest_json,
    )
    .unwrap();
    assert_eq!(macos["status"], "ok", "{macos}");
    assert_eq!(
        macos["plan"]["tools"][0]["projectFiles"],
        json!([
            {"id":"wrapper-unix","projectKind":"wrapper","path":"mvnw","requires":[".mvn/wrapper/maven-wrapper.properties"]},
            {"id":"venv-unix","projectKind":"environment","home":".venv","path":".venv/bin/java","requires":[".venv/pyvenv.cfg"]}
        ])
    );
    let windows = discovery_plan_command(
        json!({"registry":value["registry"],"platform":"windows"}),
        validated_manifest_json,
    )
    .unwrap();
    assert_eq!(windows["status"], "ok", "{windows}");
    assert_eq!(
        windows["plan"]["tools"][0]["projectFiles"],
        json!([
            {"id":"wrapper-win","projectKind":"wrapper","path":"mvnw.cmd","requires":[]},
            {"id":"venv-win","projectKind":"environment","home":".venv","path":".venv/Scripts/java.exe","requires":[".venv/pyvenv.cfg"]}
        ])
    );
}

#[test]
fn toolchain_resolution_project_environment_allows_symlinked_interpreter_and_preserves_lexical_path(
) {
    // Covers Unix single-hop and multi-hop interpreter links (resolved to an external base
    // interpreter) and Windows copied or linked `.venv\Scripts` interpreters.
    for (platform, workspace, venv_rel_exec, external_target, extra_missing_prepend) in [
        (
            "macos",
            WORKSPACE,
            "bin/java",
            "/opt/lithe-fixtures/uv/runtime/21.0.4/bin/java-real",
            "Scripts",
        ),
        (
            "linux",
            WORKSPACE,
            "bin/java",
            "/usr/lib/lithe-fixtures/runtime/bin/java21",
            "Scripts",
        ),
        (
            "windows",
            "C:/Projects/sample",
            "Scripts/java.exe",
            "C:/Projects/sample/.venv/Scripts/java.exe",
            "bin",
        ),
        (
            "windows",
            "C:/Projects/sample",
            "Scripts/java.exe",
            "C:/Users/dev/AppData/Local/uv/runtime/21.0.4/java.exe",
            "bin",
        ),
    ] {
        let mut value = input();
        value["platform"] = json!(platform);
        value["workspaceRoot"] = json!(workspace);
        let definition = &mut value["registry"]["plugins"][0]["manifest"]["toolDefinitions"][0];
        definition["executableLayouts"] = json!([
            {"path":"bin/java","platforms":["macos","linux"]},
            {"path":"Scripts/java.exe","platforms":["windows"]},
            {"path":"bin/java.exe","platforms":["windows"]}
        ]);
        definition["pathPrepend"] = json!(["${home}/bin", "${home}/Scripts"]);
        definition["executables"].as_array_mut().unwrap().insert(
            1,
            json!({
                "id":"venv",
                "kind":"projectFile",
                "projectKind":"environment",
                "home":".venv",
                "path":format!(".venv/{venv_rel_exec}"),
                "requires":[".venv/pyvenv.cfg"],
                "platforms":[platform]
            }),
        );
        let home = format!("{workspace}/.venv");
        let parent_dir = if platform == "windows" {
            format!("{home}/Scripts")
        } else {
            format!("{home}/bin")
        };
        let lexical_exec = format!("{home}/{venv_rel_exec}");
        let cfg_marker = format!("{home}/pyvenv.cfg");
        let missing_prepend = format!("{home}/{extra_missing_prepend}");
        value["filesystem"] = json!([
            fact(workspace, "directory", false),
            fact(&home, "directory", false),
            fact(&parent_dir, "directory", false),
            fact(&cfg_marker, "file", false),
            {"path":missing_prepend,"canonicalPath":missing_prepend,"kind":"missing"},
            {"path":lexical_exec,"canonicalPath":external_target,"kind":"file","executable":true}
        ]);

        // Before workspace trust is granted, no probe target is emitted and resolution skips.
        let untrusted = resolve(&value);
        assert_eq!(untrusted["outcome"]["kind"], "skip", "{platform}");
        assert_eq!(
            untrusted["outcome"]["reason"]["code"], "projectCodeRequiresAction",
            "{platform}"
        );
        assert_eq!(
            untrusted["projectCandidates"],
            json!([{
                "candidateID":"venv",
                "projectKind":"environment",
                "path":lexical_exec,
                "home":home,
                "reason":"projectCodeRequiresAction"
            }]),
            "{platform}"
        );

        // Once trusted, the first pass without probes requests probing the lexical interpreter.
        value["projectCodeAllowed"] = json!(true);
        let unprobed = resolve(&value);
        assert_eq!(unprobed["outcome"]["kind"], "skip", "{platform}");
        assert_eq!(
            unprobed["projectCandidates"],
            json!([{
                "candidateID":"venv",
                "projectKind":"environment",
                "path":lexical_exec,
                "home":home,
                "executable":lexical_exec,
                "reason":"probeFailed"
            }]),
            "{platform}"
        );

        // With the probe result on the lexical path, selection keeps the lexical executable
        // so virtual environments resolve pyvenv.cfg and sys.prefix.
        value["probes"] = json!([{
            "executable":lexical_exec,
            "output":"openjdk version \"21.0.4\"",
            "exitCode":0
        }]);
        let resolved = resolve(&value);
        assert_eq!(resolved["outcome"]["kind"], "resolved", "{platform}");
        let selection = &resolved["outcome"]["selection"];
        assert_eq!(selection["executable"], lexical_exec, "{platform}");
        assert_eq!(selection["home"], home, "{platform}");
        assert_eq!(selection["version"], "21.0.4", "{platform}");
        assert_eq!(selection["layer"], "project", "{platform}");
        assert_eq!(selection["provider"], "host:project", "{platform}");
        assert_eq!(selection["projectCode"], true, "{platform}");
        assert_eq!(
            selection["environment"],
            json!({"JAVA_HOME":home,"LITHE_JAVA_PATH":lexical_exec}),
            "{platform}"
        );
        assert_eq!(selection["pathPrepend"], json!([parent_dir]), "{platform}");
    }
}

#[test]
fn toolchain_resolution_project_environment_rejects_escaping_parent_or_marker_or_non_executable_target(
) {
    let setup_venv = |platform: &str, workspace: &str, rel_dir: &str, rel_exe: &str| {
        let mut value = input();
        value["platform"] = json!(platform);
        value["workspaceRoot"] = json!(workspace);
        value["projectCodeAllowed"] = json!(true);
        let definition = &mut value["registry"]["plugins"][0]["manifest"]["toolDefinitions"][0];
        definition["executableLayouts"] = json!([
            {"path":format!("{rel_dir}/{rel_exe}"),"platforms":[platform]},
            {"path":"bin/java","platforms":["macos","linux"]},
            {"path":"bin/java.exe","platforms":["windows"]}
        ]);
        definition["pathPrepend"] = json!([format!("${{home}}/{rel_dir}")]);
        definition["executables"].as_array_mut().unwrap().insert(
            1,
            json!({
                "id":"venv",
                "kind":"projectFile",
                "projectKind":"environment",
                "home":".venv",
                "path":format!(".venv/{rel_dir}/{rel_exe}"),
                "requires":[".venv/pyvenv.cfg"],
                "platforms":[platform]
            }),
        );
        value
    };

    for (platform, workspace, outside, rel_dir, rel_exe) in [
        (
            "macos",
            WORKSPACE,
            "/opt/lithe-fixtures/outside",
            "bin",
            "java",
        ),
        (
            "windows",
            "C:/Projects/sample",
            "C:/Outside",
            "Scripts",
            "java.exe",
        ),
    ] {
        let home = format!("{workspace}/.venv");
        let bin = format!("{home}/{rel_dir}");
        let exe = format!("{bin}/{rel_exe}");
        let cfg = format!("{home}/pyvenv.cfg");
        let external_interp = format!("{outside}/runtime/{rel_exe}");

        // 1. Intermediate `bin` / `Scripts` parent directory escapes `.venv` (outside workspace or sibling dir).
        for escaped_bin in [format!("{outside}/{rel_dir}"), format!("{workspace}/other")] {
            let mut value = setup_venv(platform, workspace, rel_dir, rel_exe);
            value["filesystem"] = json!([
                fact(workspace, "directory", false),
                fact(&home, "directory", false),
                {"path":bin,"canonicalPath":escaped_bin,"kind":"directory"},
                fact(&cfg, "file", false),
                {"path":exe,"canonicalPath":external_interp,"kind":"file","executable":true}
            ]);
            let result = resolve(&value);
            assert_eq!(result["outcome"]["kind"], "skip", "{platform}");
            assert_eq!(result["outcome"]["reason"]["code"], "unsafePath");
            assert!(result["projectCandidates"][0].get("executable").is_none());
        }

        // 2. `requires` marker escapes workspace via symlink.
        let mut value = setup_venv(platform, workspace, rel_dir, rel_exe);
        value["filesystem"] = json!([
            fact(workspace, "directory", false),
            fact(&home, "directory", false),
            fact(&bin, "directory", false),
            {"path":cfg,"canonicalPath":format!("{outside}/pyvenv.cfg"),"kind":"file","executable":false},
            {"path":exe,"canonicalPath":external_interp,"kind":"file","executable":true}
        ]);
        let result = resolve(&value);
        assert_eq!(result["outcome"]["kind"], "skip", "{platform}");
        assert_eq!(result["outcome"]["reason"]["code"], "unsafePath");

        // 3. Executable symlink points to a non-executable file.
        let mut value = setup_venv(platform, workspace, rel_dir, rel_exe);
        value["filesystem"] = json!([
            fact(workspace, "directory", false),
            fact(&home, "directory", false),
            fact(&bin, "directory", false),
            fact(&cfg, "file", false),
            {"path":exe,"canonicalPath":external_interp,"kind":"file","executable":false}
        ]);
        let result = resolve(&value);
        assert_eq!(result["outcome"]["kind"], "skip", "{platform}");
        assert_eq!(result["outcome"]["reason"]["code"], "notExecutable");
        assert!(result["projectCandidates"][0].get("executable").is_none());

        // 4. Environment root `.venv` itself escapes via symlink (outside workspace or sibling dir).
        for escaped_home in [format!("{outside}/venv"), format!("{workspace}/other")] {
            let mut value = setup_venv(platform, workspace, rel_dir, rel_exe);
            value["filesystem"] = json!([
                fact(workspace, "directory", false),
                {"path":home,"canonicalPath":escaped_home,"kind":"directory"},
                fact(&bin, "directory", false),
                fact(&cfg, "file", false),
                {"path":exe,"canonicalPath":external_interp,"kind":"file","executable":true}
            ]);
            let result = resolve(&value);
            assert_eq!(result["outcome"]["kind"], "skip", "{platform}");
            assert_eq!(result["outcome"]["reason"]["code"], "unsafePath");
            assert!(result["projectCandidates"][0].get("executable").is_none());
        }

        // 5. Unobserved intermediate `bin` / `Scripts` directory fails closed as `unsafePath`.
        let mut value = setup_venv(platform, workspace, rel_dir, rel_exe);
        value["filesystem"] = json!([
            fact(workspace, "directory", false),
            fact(&home, "directory", false),
            fact(&cfg, "file", false),
            {"path":exe,"canonicalPath":external_interp,"kind":"file","executable":true}
        ]);
        let result = resolve(&value);
        assert_eq!(result["outcome"]["kind"], "skip", "{platform}");
        assert_eq!(result["outcome"]["reason"]["code"], "unsafePath");
        assert!(result["projectCandidates"][0].get("executable").is_none());

        // 6. Escaping `pathPrepend` directory fails closed before probing (`executable` omitted).
        let mut value = setup_venv(platform, workspace, rel_dir, rel_exe);
        value["registry"]["plugins"][0]["manifest"]["toolDefinitions"][0]["pathPrepend"] =
            json!([format!("${{home}}/{rel_dir}"), "${home}/extra"]);
        let extra = format!("{home}/extra");
        value["filesystem"] = json!([
            fact(workspace, "directory", false),
            fact(&home, "directory", false),
            fact(&bin, "directory", false),
            {"path":extra,"canonicalPath":format!("{outside}/extra"),"kind":"directory"},
            fact(&cfg, "file", false),
            {"path":exe,"canonicalPath":external_interp,"kind":"file","executable":true}
        ]);
        let result = resolve(&value);
        assert_eq!(result["outcome"]["kind"], "skip", "{platform}");
        assert_eq!(result["outcome"]["reason"]["code"], "unsafePath");
        assert!(result["projectCandidates"][0].get("executable").is_none());
    }
}

#[test]
fn toolchain_resolution_wrapper_allows_workspace_internal_link_but_rejects_external_link() {
    for (platform, workspace, outside, wrapper_name) in [
        ("macos", WORKSPACE, "/opt/lithe-fixtures/outside", "mvnw"),
        ("windows", "C:/Projects/sample", "C:/Outside", "mvnw.cmd"),
    ] {
        let mut value = input();
        value["platform"] = json!(platform);
        value["workspaceRoot"] = json!(workspace);
        value["directory"] = json!("sub");
        value["projectCodeAllowed"] = json!(true);
        let definition = &mut value["registry"]["plugins"][0]["manifest"]["toolDefinitions"][0];
        definition["launch"] = json!(["${executable}", "${arguments}"]);
        definition["wrappers"] = json!([{"path":wrapper_name,"platforms":[platform]}]);
        definition["executables"].as_array_mut().unwrap().insert(
            1,
            json!({"id":"wrapper","kind":"projectFile","path":wrapper_name,"platforms":[platform]}),
        );
        let sub = format!("{workspace}/sub");
        let sub_wrapper = format!("{sub}/{wrapper_name}");
        let root_wrapper = format!("{workspace}/{wrapper_name}");

        // Workspace-internal link (e.g., `sub/mvnw -> ../mvnw`) resolves and preserves `sub/mvnw`.
        value["filesystem"] = json!([
            fact(workspace, "directory", false),
            fact(&sub, "directory", false),
            fact(&root_wrapper, "file", true),
            {"path":sub_wrapper,"canonicalPath":root_wrapper,"kind":"file","executable":true}
        ]);
        let internal = resolve(&value);
        assert_eq!(internal["outcome"]["kind"], "resolved", "{platform}");
        assert_eq!(internal["outcome"]["selection"]["executable"], sub_wrapper);

        // Link escaping workspace (`sub/mvnw -> /opt/.../mvnw`) is rejected as `unsafePath`.
        value["filesystem"] = json!([
            fact(workspace, "directory", false),
            fact(&sub, "directory", false),
            {"path":sub_wrapper,"canonicalPath":format!("{outside}/{wrapper_name}"),"kind":"file","executable":true}
        ]);
        let external = resolve(&value);
        assert_eq!(external["outcome"]["kind"], "skip", "{platform}");
        assert_eq!(external["outcome"]["reason"]["code"], "unsafePath");
    }
}

#[test]
fn toolchain_candidates_and_multi_hop_environment_links_use_synthetic_tool_definition() {
    let synthetic_manifest = json!({
        "schemaVersion": 0,
        "id": "dev.lithe.test.runtime-support",
        "displayName": "Synthetic Runtime",
        "version": "0.1.0",
        "pluginAPIVersion": 0,
        "vendor": {"id": "dev.lithe.test", "displayName": "Test", "signatureRequirement": "publisherPackage"},
        "permissions": [],
        "requires": {"toolDefinitions": 0},
        "toolDefinitions": [{
            "id": "dev.lithe.test.runtime-support:vm",
            "displayName": "Synthetic VM",
            "executableLayouts": [
                {"path": "bin/vm3", "platforms": ["macos", "linux"]},
                {"path": "bin/vm", "platforms": ["macos", "linux"]},
                {"path": "Scripts/vm.exe", "platforms": ["windows"]},
                {"path": "vm.exe", "platforms": ["windows"]}
            ],
            "executables": [
                {"id": "pinned", "kind": "setting", "settingID": "executable", "layer": "project"},
                {"id": "venv-unix-3", "kind": "projectFile", "projectKind": "environment", "home": ".venv", "path": ".venv/bin/vm3", "requires": [".venv/env.cfg"], "platforms": ["macos", "linux"]},
                {"id": "venv-unix", "kind": "projectFile", "projectKind": "environment", "home": ".venv", "path": ".venv/bin/vm", "requires": [".venv/env.cfg"], "platforms": ["macos", "linux"]},
                {"id": "venv-win", "kind": "projectFile", "projectKind": "environment", "home": ".venv", "path": ".venv/Scripts/vm.exe", "requires": [".venv/env.cfg"], "platforms": ["windows"]},
                {"id": "global", "kind": "setting", "settingID": "executable", "layer": "global"},
                {"id": "shell", "kind": "command", "command": "vm"},
                {"id": "installed", "kind": "toolchain", "scope": "installed"}
            ],
            "version": {"arguments": ["--version"], "pattern": "SyntheticVM ([0-9]+\\.[0-9]+\\.[0-9]+)"},
            "commands": ["vm"],
            "exports": [
                {"name": "SYNTHETIC_VM_HOME", "value": "${home}"},
                {"name": "SYNTHETIC_VM_EXEC", "value": "${executable}"}
            ],
            "pathPrepend": ["${home}/bin", "${home}/Scripts"],
            "launch": ["${executable}", "${runtimeArguments}", "${entry}", "${arguments}"]
        }]
    });

    for (platform, workspace, rel_dir, primary, hop, base_exec, missing_dir, sys_home, sys_exec) in [
        (
            "macos",
            "/opt/lithe-fixtures/synthetic-ws",
            "bin",
            "vm3",
            Some("vm"),
            "/opt/lithe-fixtures/base-vm/3.12.7/bin/vm-real",
            "Scripts",
            "/opt/lithe-fixtures/system-vm/3.11.9",
            "/opt/lithe-fixtures/system-vm/3.11.9/bin/vm3",
        ),
        (
            "linux",
            "/opt/lithe-fixtures/synthetic-ws",
            "bin",
            "vm3",
            Some("vm"),
            "/usr/lib/lithe-fixtures/base-vm/3.12.7/bin/vm-real",
            "Scripts",
            "/usr/lib/lithe-fixtures/system-vm/3.11.9",
            "/usr/lib/lithe-fixtures/system-vm/3.11.9/bin/vm3",
        ),
        (
            "windows",
            "C:/Projects/synthetic-ws",
            "Scripts",
            "vm.exe",
            None,
            "C:/Projects/synthetic-ws/.venv/Scripts/vm.exe",
            "bin",
            "C:/ProgramData/SyntheticVM/3.11.9",
            "C:/ProgramData/SyntheticVM/3.11.9/vm.exe",
        ),
    ] {
        let home = format!("{workspace}/.venv");
        let dir = format!("{home}/{rel_dir}");
        let lexical_primary = format!("{dir}/{primary}");
        let marker = format!("{home}/env.cfg");
        let missing = format!("{home}/{missing_dir}");

        let mut fs = vec![
            fact(workspace, "directory", false),
            fact(&home, "directory", false),
            fact(&dir, "directory", false),
            fact(&marker, "file", false),
            json!({"path": missing, "canonicalPath": missing, "kind": "missing"}),
            json!({"path": lexical_primary, "canonicalPath": base_exec, "kind": "file", "executable": true}),
            fact(sys_home, "directory", false),
            fact(sys_exec, "file", true),
            json!({"path": format!("{sys_home}/Scripts"), "canonicalPath": format!("{sys_home}/Scripts"), "kind": "missing"}),
        ];
        if platform != "windows" {
            fs.push(fact(&format!("{sys_home}/bin"), "directory", false));
        } else {
            fs.push(json!({"path": format!("{sys_home}/bin"), "canonicalPath": format!("{sys_home}/bin"), "kind": "missing"}));
        }
        if let Some(second_link) = hop {
            let lexical_hop = format!("{dir}/{second_link}");
            // Multi-hop symlink chain in `.venv/bin` (`vm -> vm3 -> base_exec`), plus explicit base target fact.
            fs.push(json!({"path": lexical_hop, "canonicalPath": base_exec, "kind": "file", "executable": true}));
            fs.push(fact(base_exec, "file", true));
        }

        let registry = json!({"plugins": [{"manifest": synthetic_manifest}]});
        let mut cand_req = json!({
            "registry": registry,
            "tool": "dev.lithe.test.runtime-support:vm",
            "platform": platform,
            "workspaceRoot": workspace,
            "filesystem": fs,
            "probes": [{"executable": sys_exec, "output": "SyntheticVM 3.11.9", "exitCode": 0}],
            "systemInstallations": [{"tool": "dev.lithe.test.runtime-support:vm", "home": sys_home}],
            "projectCodeAllowed": false
        });

        // 1. Untrusted `candidates`: `project` reports `projectCodeRequiresAction` without `executable`.
        let untrusted_res = candidates_command(cand_req.clone(), validated_manifest_json).unwrap();
        assert_eq!(untrusted_res["status"], "ok", "{untrusted_res}");
        let untrusted_cand = untrusted_res["candidates"].clone();
        assert_eq!(
            untrusted_cand["project"][0]["reason"], "projectCodeRequiresAction",
            "{platform}"
        );
        assert!(
            untrusted_cand["project"][0].get("executable").is_none(),
            "{platform}"
        );
        assert_eq!(untrusted_cand["installations"].as_array().unwrap().len(), 1);
        assert_eq!(untrusted_cand["installations"][0]["home"], sys_home);

        // 2. Trusted `candidates` before probing `.venv`: emits lexical `executable` with `probeFailed`.
        cand_req["projectCodeAllowed"] = json!(true);
        let unprobed_cand = candidates_command(cand_req.clone(), validated_manifest_json).unwrap()
            ["candidates"]
            .clone();
        assert_eq!(
            unprobed_cand["project"][0]["executable"], lexical_primary,
            "{platform}"
        );
        assert_eq!(
            unprobed_cand["project"][0]["reason"], "probeFailed",
            "{platform}"
        );

        // 3. Trusted `candidates` after probing lexical paths: reports `version`, never mixes `.venv` into `installations`.
        let mut probes = vec![
            json!({"executable": sys_exec, "output": "SyntheticVM 3.11.9", "exitCode": 0}),
            json!({"executable": lexical_primary, "output": "SyntheticVM 3.12.7", "exitCode": 0}),
        ];
        if let Some(second_link) = hop {
            probes.push(json!({"executable": format!("{dir}/{second_link}"), "output": "SyntheticVM 3.12.7", "exitCode": 0}));
        }
        cand_req["probes"] = json!(probes);
        let probed_cand = candidates_command(cand_req.clone(), validated_manifest_json).unwrap()
            ["candidates"]
            .clone();
        assert_eq!(probed_cand["project"][0]["version"], "3.12.7", "{platform}");
        assert!(probed_cand["project"][0].get("reason").is_none());
        assert_eq!(probed_cand["installations"].as_array().unwrap().len(), 1);

        // 4. Full resolution selects `.venv` over `installed` and preserves the lexical interpreter path.
        let resolved = resolve(&json!({
            "registry": registry,
            "operationID": "synth-1",
            "generation": 1,
            "context": {"contextID": "ctx-1", "generation": 1},
            "tool": "dev.lithe.test.runtime-support:vm",
            "directory": ".",
            "platform": platform,
            "workspaceRoot": workspace,
            "filesystem": fs,
            "probes": probes,
            "queries": [],
            "systemInstallations": [{"tool": "dev.lithe.test.runtime-support:vm", "home": sys_home}],
            "environment": {},
            "commandHomes": {},
            "projectCodeAllowed": true
        }));
        assert_eq!(resolved["outcome"]["kind"], "resolved", "{platform}");
        let selection = &resolved["outcome"]["selection"];
        assert_eq!(selection["executable"], lexical_primary, "{platform}");
        assert_eq!(selection["home"], home, "{platform}");
        assert_eq!(selection["version"], "3.12.7", "{platform}");
        assert_eq!(selection["layer"], "project", "{platform}");
        assert_eq!(
            selection["environment"],
            json!({"SYNTHETIC_VM_HOME": home, "SYNTHETIC_VM_EXEC": lexical_primary}),
            "{platform}"
        );
        assert_eq!(selection["pathPrepend"], json!([dir]), "{platform}");
    }
}
