//! Deterministic local-pin validation and component-wise inheritance regressions.

use super::toolchain_vectors::local_pins;
use crate::toolchain::pins;
use serde_json::{json, Value};

/// Exercises the pure input boundary with a selected directory and arbitrary tool.
fn resolve(document: &Value, directory: &str, tool: &str) -> Value {
    pins::resolve(&json!({"document":document,"directory":directory,"tool":tool})).unwrap()
}

/// Produces a valid inert pin for independent validation scenarios.
fn pin(home: &str) -> Value {
    json!({"home":home,"version":"21.0.4","provider":"host:manual"})
}

#[test]
fn toolchain_pins_inherit_the_nearest_ancestor() {
    let document = local_pins();
    pins::validate(&document).unwrap();
    for (directory, expected) in [
        (".", "."),
        ("apps", "."),
        ("apps/web", "apps/web"),
        ("apps/web/src/main", "apps/web"),
        (".config/tools/settings", ".config/tools"),
    ] {
        let result = resolve(&document, directory, "java");
        assert_eq!(result["sourceDirectory"], expected, "{directory}");
        assert_eq!(result["pin"], document["pins"][expected]["java"]);
    }
}

#[test]
fn toolchain_pins_do_not_match_sibling_prefixes_or_other_tools() {
    let document = local_pins();
    assert_eq!(
        resolve(&document, "apps/website/src", "java")["sourceDirectory"],
        "."
    );
    assert_eq!(
        resolve(&document, "apps/web/src", "node"),
        json!({"pin":null,"sourceDirectory":null})
    );
    assert_eq!(
        resolve(&json!({"version":1,"toolchains":{}}), ".", "java"),
        json!({"pin":null,"sourceDirectory":null})
    );
    assert_eq!(
        resolve(&json!({"version":1,"toolchains":{},"pins":{}}), ".", "java"),
        json!({"pin":null,"sourceDirectory":null})
    );
}

#[test]
fn toolchain_pins_retain_dormant_namespaced_values_and_legacy_data() {
    let document = json!({
        "version":1,
        "toolchains":{"project-runtime":{"home":"relative legacy home","legacyHome":"legacy runtime"},"custom old key":{"executable":"legacy-command"}},
        "pins":{".":{"dev.lithe.fixture.disabled:compiler":pin("/opt/lithe-fixtures/compiler")}}
    });
    let before = document.clone();
    let result = resolve(&document, "src", "dev.lithe.fixture.disabled:compiler");
    assert_eq!(
        result["pin"],
        document["pins"]["."]["dev.lithe.fixture.disabled:compiler"]
    );
    assert_eq!(document, before);
    assert!(resolve(&document, ".", "java")["pin"].is_null());
}

#[test]
fn toolchain_pins_validate_every_entry_not_only_the_requested_tool() {
    let mut document = local_pins();
    document["pins"]["unrelated"] =
        json!({"node":{"home":"relative","version":"22","provider":"host:manual"}});
    let error =
        pins::resolve(&json!({"document":document,"directory":".","tool":"java"})).unwrap_err();
    assert_eq!(error.code(), "plugin.toolchain.invalidParams");
    assert_eq!(error.field(), Some("document.pins.unrelated.node.home"));
}

#[test]
fn toolchain_pins_reject_non_normalized_directories_but_allow_dotted_names() {
    for directory in [
        "",
        "/root",
        "../parent",
        "src/../lib",
        "src/./lib",
        "src//lib",
        "src/",
        "src\\lib",
        "C:/root",
        "src\nlib",
    ] {
        let mut document = local_pins();
        document["pins"][directory] = json!({"java":pin("/opt/lithe-fixtures/java")});
        assert!(
            pins::validate(&document).is_err(),
            "document key {directory:?}"
        );
        assert!(
            pins::resolve(&json!({"document":local_pins(),"directory":directory,"tool":"java"}))
                .is_err(),
            "requested directory {directory:?}"
        );
    }
    for directory in [".config/tools", "foo.bar/.hidden", "a..b", "."] {
        assert!(
            pins::resolve(&json!({"document":local_pins(),"directory":directory,"tool":"java"}))
                .is_ok(),
            "{directory}"
        );
    }
}

#[test]
fn toolchain_pins_accept_cross_platform_absolute_paths_without_io() {
    for home in [
        "/opt/lithe-fixtures/java",
        "/",
        "C:\\Toolchains\\Java",
        "D:/Tools/Java",
        "C:\\",
        "\\\\server\\share\\tools",
        "\\\\server\\share",
        "\\\\server\\share\\",
        "//server/share/tools",
    ] {
        let document = json!({"version":1,"toolchains":{},"pins":{".":{"java":pin(home)}}});
        assert_eq!(resolve(&document, ".", "java")["pin"]["home"], home);
    }
    for home in [
        "relative",
        "~/java",
        "C:relative",
        "\\rooted",
        "\\\\server",
        "\\\\?\\C:\\tools",
        "\\\\.\\tools",
        "C:/tools/../java",
        "/opt/../java",
        "/opt//java",
        "//",
        "C://",
        "/opt/./java",
        "C:/tools:stream",
        "C:/tools./java",
        "C:/tools /java",
        "/opt/ja\nva",
    ] {
        let document = json!({"version":1,"toolchains":{},"pins":{".":{"java":pin(home)}}});
        assert!(pins::validate(&document).is_err(), "{home:?}");
    }
}

#[test]
fn toolchain_pins_reject_invalid_tools_providers_and_unknown_fields() {
    for tool in [
        "",
        "Java",
        "dev..fixture:tool",
        "dev.fixture:",
        "dev.fixture:Tool",
        "a:b:c",
        "a/b",
    ] {
        assert!(
            pins::resolve(&json!({"document":local_pins(),"directory":".","tool":tool})).is_err(),
            "{tool:?}"
        );
        let document = json!({"version":1,"toolchains":{},"pins":{".":{tool:pin("/opt/lithe-fixtures/java")}}});
        assert!(pins::validate(&document).is_err(), "{tool:?}");
    }
    for provider in [
        "mise",
        "host:",
        "host:Manual",
        "Host:manual",
        "1host:manual",
        "dev..fixture:mise",
        "host:manual:extra",
    ] {
        let mut document = local_pins();
        document["pins"]["."]["java"]["provider"] = json!(provider);
        assert!(pins::validate(&document).is_err(), "{provider}");
    }
    let mut document = local_pins();
    document["extra"] = json!(true);
    assert!(pins::validate(&document).is_err());
    let mut document = local_pins();
    document["pins"]["."]["java"]["extra"] = json!(true);
    assert!(pins::validate(&document).is_err());
    // Legacy entries are opaque string records: any key is kept, but a value must
    // be a string and an entry holds at most 16 properties.
    let document = json!({"version":1,"toolchains":{"old":{"unknown":"path"}}});
    assert!(pins::validate(&document).is_ok());
    let document = json!({"version":1,"toolchains":{"old":{"unknown":true}}});
    assert!(pins::validate(&document).is_err());
    let crowded: serde_json::Map<String, Value> = (0..17)
        .map(|index| (format!("key{index}"), json!("path")))
        .collect();
    let document = json!({"version":1,"toolchains":{"old":crowded}});
    assert!(pins::validate(&document).is_err());
    assert!(pins::resolve(
        &json!({"document":local_pins(),"directory":".","tool":"java","unknown":true})
    )
    .is_err());
}

#[test]
fn toolchain_pins_reject_missing_fields_and_wrong_shapes() {
    for document in [
        Value::Null,
        json!({}),
        json!({"version":2,"toolchains":{}}),
        json!({"version":1,"toolchains":[]}),
        json!({"version":1,"toolchains":{},"pins":null}),
        json!({"version":1,"toolchains":{},"pins":{".":[]}}),
    ] {
        assert!(pins::validate(&document).is_err(), "{document}");
    }
    for property in ["home", "version", "provider"] {
        let mut document = local_pins();
        document["pins"]["."]["java"]
            .as_object_mut()
            .unwrap()
            .remove(property);
        assert!(pins::validate(&document).is_err(), "missing {property}");
        let mut document = local_pins();
        document["pins"]["."]["java"][property] = json!(42);
        assert!(pins::validate(&document).is_err(), "numeric {property}");
    }
}

#[test]
fn toolchain_pins_enforce_collection_and_text_limits_before_resolution() {
    let mut document = json!({"version":1,"toolchains":{},"pins":{}});
    for index in 0..1025 {
        document["pins"][format!("dir-{index}")] = json!({});
    }
    assert_eq!(
        pins::validate(&document).unwrap_err().code(),
        "plugin.toolchain.limitExceeded"
    );
    let mut document = json!({"version":1,"toolchains":{},"pins":{".":{}}});
    for index in 0..65 {
        document["pins"]["."][format!("tool-{index}")] = pin("/opt/lithe-fixtures/tool");
    }
    assert_eq!(
        pins::validate(&document).unwrap_err().code(),
        "plugin.toolchain.limitExceeded"
    );
    for (property, length) in [("home", 4097), ("version", 129), ("provider", 257)] {
        let mut document = local_pins();
        document["pins"]["."]["java"][property] = json!("x".repeat(length));
        assert_eq!(
            pins::validate(&document).unwrap_err().code(),
            "plugin.toolchain.limitExceeded",
            "{property}"
        );
    }
    let mut document = local_pins();
    document["pins"]["x".repeat(2049)] = json!({});
    assert_eq!(
        pins::validate(&document).unwrap_err().code(),
        "plugin.toolchain.limitExceeded"
    );
}

/// Calls the public JSON boundary so an unregistered command cannot pass.
fn edit(payload: Value) -> Value {
    serde_json::from_str(&crate::execute_json(
        &json!({"id":"pin-edit","command":"toolchains.editPins","payload":payload}).to_string(),
    ))
    .unwrap()
}

#[test]
fn toolchain_pin_edits_preserve_siblings_and_remove_only_the_selected_override() {
    let document = local_pins();
    let changed = edit(json!({"document":document,"directory":"apps/web","edits":[
        {"tool":"java","pin":null},
        {"tool":"node","pin":pin("/opt/lithe-fixtures/node")}
    ]}));
    assert_eq!(changed["ok"], true, "{changed}");
    assert_eq!(changed["data"]["status"], "ok", "{changed}");
    let result = &changed["data"]["edit"]["document"];
    assert_eq!(result["toolchains"], document["toolchains"]);
    assert_eq!(result["pins"]["."], document["pins"]["."]);
    assert_eq!(
        result["pins"][".config/tools"],
        document["pins"][".config/tools"]
    );
    assert!(result["pins"]["apps/web"].get("java").is_none());
    assert_eq!(
        result["pins"]["apps/web"]["node"],
        pin("/opt/lithe-fixtures/node")
    );
    assert_eq!(
        resolve(result, "apps/web/src", "java")["sourceDirectory"],
        "."
    );
}

#[test]
fn toolchain_pin_edits_reject_invalid_batches_without_a_partial_document() {
    for edits in [
        json!([]),
        json!([{"tool":"java","pin":pin("relative")}]),
        json!([{"tool":"java","pin":null},{"tool":"java","pin":null}]),
        json!([{"tool":"java"}]),
        json!([{"tool":"java","pin":null,"extra":true}]),
    ] {
        let response = edit(json!({"document":local_pins(),"directory":".","edits":edits}));
        assert_eq!(response["data"]["status"], "rejected", "{response}");
        assert!(response["data"].get("edit").is_none(), "{response}");
    }
    let response = edit(
        json!({"document":local_pins(),"directory":"../outside","edits":[{"tool":"java","pin":null}]}),
    );
    assert_eq!(response["data"]["status"], "rejected", "{response}");
}

#[test]
fn toolchain_pin_edits_prune_empty_containers_and_leave_absent_removals_inert() {
    let document =
        json!({"version":1,"toolchains":{},"pins":{".":{"java":pin("/opt/lithe-fixtures/java")}}});
    let response =
        edit(json!({"document":document,"directory":".","edits":[{"tool":"java","pin":null}]}));
    assert_eq!(
        response["data"]["edit"]["document"],
        json!({"version":1,"toolchains":{}})
    );
    let document = json!({"version":1,"toolchains":{}});
    let response =
        edit(json!({"document":document,"directory":"src","edits":[{"tool":"node","pin":null}]}));
    assert_eq!(response["data"]["edit"]["document"], document);
}

#[test]
fn toolchain_pin_edits_match_the_shared_boundary_fixture() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../fixtures/toolchain-pin-edit.json")).unwrap();
    let response = edit(fixture["request"].clone());
    assert_eq!(response["data"]["edit"]["document"], fixture["document"]);
}

#[test]
fn toolchain_pin_edits_enforce_final_collection_limits_and_validate_the_source() {
    let mut request =
        json!({"document":local_pins(),"directory":".","edits":[{"tool":"java","pin":null}]});
    request["document"]["unknown"] = json!(true);
    assert_eq!(edit(request)["data"]["status"], "rejected");
    let edits: Vec<_> = (0..65)
        .map(|index| json!({"tool":format!("tool-{index}"),"pin":null}))
        .collect();
    let response = edit(json!({"document":local_pins(),"directory":".","edits":edits}));
    assert_eq!(
        response["data"]["error"]["code"],
        "plugin.toolchain.limitExceeded"
    );
    let tools: serde_json::Map<String, Value> = (0..64)
        .map(|index| (format!("tool-{index}"), pin("/fixture/tool")))
        .collect();
    let document = json!({"version":1,"toolchains":{},"pins":{".":tools}});
    let response = edit(
        json!({"document":document,"directory":".","edits":[{"tool":"extra","pin":pin("/fixture/extra")}]}),
    );
    assert_eq!(
        response["data"]["error"]["code"],
        "plugin.toolchain.limitExceeded"
    );
    assert!(response["data"].get("edit").is_none());
}
