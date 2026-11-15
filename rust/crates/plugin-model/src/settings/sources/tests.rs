//! Source projection and confirmed-edit regressions use inert synthetic facts.

use super::*;

fn snapshot() -> Value {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/plugins/settings-sources.json"
    )))
    .unwrap();
    fixture["snapshot"].clone()
}

#[test]
fn inheritance_is_visible_without_manufacturing_an_override() {
    let state = state_command(json!({"result":snapshot()})).unwrap();
    assert_eq!(state["status"], "ok", "{state}");
    let inputs = &state["state"]["inputs"];
    assert_eq!(inputs["root"]["target"]["choice"], "one");
    assert_eq!(inputs["root"]["target"]["inherit"], true);
    assert_eq!(inputs["root"]["checks"]["on"], true);
    let edits: Vec<_> = inputs
        .as_object()
        .unwrap()
        .iter()
        .map(|(id, values)| json!({"sourceID":id,"values":values}))
        .collect();
    let result =
        edit_command(json!({"snapshot":snapshot(),"edits":edits,"confirmed":true})).unwrap();
    assert_eq!(result["writes"], json!([]), "{result}");
}

#[test]
fn a_module_edit_does_not_touch_project_defaults_or_sibling_sources() {
    let result = edit_command(json!({"snapshot":snapshot(),"confirmed":true,"edits":[
        {"sourceID":"child","values":{"target":{"choice":"one"}}}
    ]}))
    .unwrap();
    assert_eq!(result["status"], "ok", "{result}");
    assert_eq!(result["writes"].as_array().unwrap().len(), 1);
    assert_eq!(result["writes"][0]["path"], "child/tool.json");
    assert_eq!(result["writes"][0]["values"], json!({"target":"one"}));
}

#[test]
fn resetting_one_override_emits_null_for_that_field_only() {
    let result = edit_command(json!({"snapshot":snapshot(),"confirmed":true,"edits":[
        {"sourceID":"child","values":{"target":{"inherit":true}}}
    ]}))
    .unwrap();
    assert_eq!(result["writes"][0]["values"], json!({"target":null}));
}

#[test]
fn file_list_editing_preserves_spaces_and_windows_path_spelling() {
    let result = edit_command(json!({"snapshot":snapshot(),"confirmed":true,"edits":[
        {"sourceID":"root","values":{"files":{"text":"C:\\Example Files\\one.jar\n\nlib/two.jar\n"}}}
    ]})).unwrap();
    assert_eq!(
        result["writes"][0]["values"]["files"],
        json!(["C:\\Example Files\\one.jar", "lib/two.jar"])
    );
}

#[test]
fn unconfirmed_invalid_read_only_and_unbound_edits_are_rejected_atomically() {
    for (confirmed, source, field, value, read_only) in [
        (false, "child", "target", "one", false),
        (true, "unknown", "target", "one", false),
        (true, "child", "unknown", "one", false),
        (true, "child", "target", "unknown", false),
        (true, "child", "target", "one", true),
    ] {
        let mut snapshot = snapshot();
        if read_only {
            snapshot["sources"][1]["writable"] = json!(false);
        }
        let result = edit_command(json!({"snapshot":snapshot,"confirmed":confirmed,"edits":[
            {"sourceID":source,"values":{(field):{"choice":value}}}
        ]}))
        .unwrap();
        assert_eq!(result["status"], "rejected", "{result}");
        assert!(result.get("writes").is_none());
    }
}

#[test]
fn unsafe_source_names_duplicate_bindings_and_invalid_values_never_reach_ui() {
    for path in [
        "../outside",
        "/outside",
        "C:/outside",
        "a/../outside",
        "a//b",
        "a\\b",
    ] {
        let mut snapshot = snapshot();
        snapshot["sources"][0]["path"] = json!(path);
        let result = state_command(json!({"result":snapshot})).unwrap();
        assert_eq!(result["status"], "rejected", "{path}: {result}");
    }
    for kind in ["duplicateID", "duplicatePath", "wrongValue", "unknownGroup"] {
        let mut bad = snapshot();
        match kind {
            "duplicateID" => bad["sources"][1]["id"] = json!("root"),
            "duplicatePath" => bad["sources"][1]["path"] = json!("tool.json"),
            "wrongValue" => bad["sources"][0]["fields"][0]["value"] = json!(17),
            _ => bad["sources"][0]["group"] = json!("unknown"),
        }
        assert_eq!(
            state_command(json!({"result":bad})).unwrap()["status"],
            "rejected",
            "{kind}"
        );
    }
}

#[test]
fn staging_receipts_are_bound_to_the_confirmed_source_and_revision() {
    let write = json!({"sourceID":"child","path":"child/tool.json","revision":"missing","values":{"target":"one"}});
    let receipt = json!({"format":"litheSettingsSourceEdit","version":0,"sourceID":"child",
        "revision":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","changed":true});
    assert_eq!(
        receipt_command(json!({"write":write,"result":receipt})).unwrap()["status"],
        "ok"
    );
    for field in ["sourceID", "revision", "format"] {
        let mut bad = receipt.clone();
        bad[field] = json!("different");
        assert_eq!(
            receipt_command(json!({"write":write,"result":bad})).unwrap()["status"],
            "rejected"
        );
    }
}

#[test]
fn inheritance_and_current_effective_values_remain_distinct_from_an_override() {
    let mut fixture = snapshot();
    fixture["sources"][1]["fields"][0]["effectiveValue"] = json!("one");
    let result = state_command(json!({"result":fixture})).unwrap();
    assert_eq!(
        result["state"]["inputs"]["child"]["target"]["choice"],
        "two"
    );
    assert_eq!(
        result["state"]["inheritedInputs"]["child"]["target"]["choice"],
        "one"
    );
    assert_eq!(
        result["state"]["effectiveDisplays"]["child"]["target"],
        json!({"kind":"text","text":"Version 1"})
    );
}
