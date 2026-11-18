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
        json!({"kind":"text","text":"Version 1","inherited":false})
    );
}

#[test]
fn absent_boolean_values_expose_inheritance_and_unknown_effective_values() {
    let mut fixture = snapshot();
    fixture["sources"][0]["fields"][1]["value"] = Value::Null;
    fixture["sources"][0]["fields"][1]["effectiveValue"] = Value::Null;
    let result = state_command(json!({"result":fixture})).unwrap();
    assert_eq!(
        result["state"]["effectiveDisplays"]["root"]["checks"],
        json!({"kind":"unknown","inherited":true})
    );
}

#[test]
fn source_editor_bindings_follow_the_public_id_field_spelling() {
    let value = json!({"editFunction":"compiler.prepare","buildSystemID":"fictional","languageID":"fictional"});
    let declaration: crate::settings::SourceEditorDeclaration =
        serde_json::from_value(value.clone()).unwrap();
    assert_eq!(declaration.build_system_id.as_deref(), Some("fictional"));
    assert_eq!(declaration.language_id.as_deref(), Some("fictional"));
    assert_eq!(serde_json::to_value(declaration).unwrap(), value);
}

fn modules() -> Value {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/plugins/settings-sources-modules.json"
    )))
    .unwrap();
    fixture["snapshot"].clone()
}

fn module_edit(values: Value) -> Value {
    edit_command(json!({"snapshot":modules(),"confirmed":true,"edits":[
        {"sourceID":"app","values":values}
    ]}))
    .unwrap()
}

fn dependency_entries() -> Value {
    state_command(json!({"result":modules()})).unwrap()["state"]["inputs"]["app"]["dependencies"]
        ["entries"]
        .clone()
}

#[test]
fn sidebar_modules_keep_their_nesting_sections_and_ordered_entries() {
    let state = state_command(json!({"result":modules()})).unwrap();
    assert_eq!(state["status"], "ok", "{state}");
    let state = &state["state"];
    assert_eq!(state["snapshot"]["sources"][1]["parent"], "root");
    assert_eq!(
        state["snapshot"]["sources"][1]["fields"][0]["section"],
        "Sources"
    );
    let entries = &state["inputs"]["app"]["dependencies"]["entries"];
    assert_eq!(entries[0]["value"], "lib/one.zip");
    assert_eq!(entries[1]["title"], "Legacy library");
    assert_eq!(entries[1]["attributes"]["exported"], true);
    // An inherited, read-only list is shown by count, like a file list.
    assert_eq!(
        state["effectiveDisplays"]["core"]["dependencies"],
        json!({"kind":"list","count":1,"inherited":true})
    );
    assert_eq!(
        state["layout"],
        json!([{"group":0,"sources":[0,1,2]},{"group":1,"sources":[3]}])
    );
}

#[test]
fn unchanged_entries_with_display_text_produce_no_write() {
    let entries = dependency_entries();
    let result = module_edit(json!({"dependencies":{"entries":entries}}));
    assert_eq!(result["writes"], json!([]), "{result}");
}

#[test]
fn added_reordered_and_flagged_entries_reach_the_plugin_without_display_text() {
    let mut entries = dependency_entries().as_array().unwrap().clone();
    entries.swap(0, 1);
    entries[1]["attributes"] = json!({"exported":true,"scope":"test"});
    entries.push(json!({"kind":"library","value":"lib:shared","title":"Shared library"}));
    entries.push(json!({"kind":"module","value":"core"}));
    entries.push(json!({"kind":"archive","value":"C:\\Example Files\\two.jar"}));
    let result = module_edit(json!({"dependencies":{"entries":entries}}));
    assert_eq!(result["status"], "ok", "{result}");
    assert_eq!(
        result["writes"][0]["values"]["dependencies"],
        json!([
            {"kind":"library","value":"lib:legacy","attributes":{"exported":true}},
            {"kind":"archive","value":"lib/one.zip","attributes":{"exported":true,"scope":"test"}},
            {"kind":"library","value":"lib:shared"},
            {"kind":"module","value":"core"},
            {"kind":"archive","value":"C:\\Example Files\\two.jar"}
        ])
    );
}

#[test]
fn removing_every_entry_writes_an_empty_list_not_an_absent_override() {
    let result = module_edit(json!({"dependencies":{"entries":[]}}));
    assert_eq!(result["writes"][0]["values"], json!({"dependencies":[]}));
}

#[test]
fn entries_the_plugin_neither_offers_nor_holds_are_rejected() {
    for added in [
        json!({"kind":"library","value":"lib:invented"}),
        json!({"kind":"module","value":"app"}),
        json!({"kind":"unknown","value":"lib/one.zip"}),
        json!({"kind":"archive","value":"lib/x.zip","attributes":{"unknown":true}}),
        json!({"kind":"archive","value":"lib/x.zip","attributes":{"exported":"yes"}}),
        json!({"kind":"archive","value":"lib/x.zip","attributes":{"scope":"runtime"}}),
        json!({"kind":"archive","value":"lib/one.zip"}),
        json!({"kind":"archive","value":" "}),
    ] {
        let mut entries = dependency_entries().as_array().unwrap().clone();
        entries.push(added.clone());
        let result = module_edit(json!({"dependencies":{"entries":entries}}));
        assert_eq!(result["status"], "rejected", "{added}: {result}");
    }
}

#[test]
fn read_only_modules_reject_entry_edits() {
    let result = edit_command(json!({"snapshot":modules(),"confirmed":true,"edits":[
        {"sourceID":"core","values":{"dependencies":{"entries":[]}}}
    ]}))
    .unwrap();
    assert_eq!(result["status"], "rejected", "{result}");
}

#[test]
fn malformed_module_trees_sections_and_entry_declarations_never_reach_ui() {
    type Case = (&'static str, fn(&mut Value));
    let cases: [Case; 12] = [
        ("parent outside a sidebar", |s| {
            s["sources"][3]["parent"] = json!("root")
        }),
        ("unknown parent", |s| {
            s["sources"][1]["parent"] = json!("missing")
        }),
        ("parent after child", |s| {
            s["sources"][0]["parent"] = json!("app");
            s["sources"][1]["parent"] = Value::Null;
        }),
        ("parent in another group", |s| {
            s["groups"][1]["layout"] = json!("sidebar");
            s["sources"][3]["parent"] = json!("root");
        }),
        ("section in a table", |s| {
            s["groups"][1]["layout"] = json!("table")
        }),
        ("entry kinds on a text field", |s| {
            s["sources"][1]["fields"][1]["entryKinds"] =
                json!([{"id":"a","title":"A","input":"file"}])
        }),
        ("entry list without kinds", |s| {
            s["sources"][1]["fields"][0]["entryKinds"] = json!([]);
        }),
        ("extensions on a choice", |s| {
            s["sources"][1]["fields"][3]["entryKinds"][2]["extensions"] = json!(["zip"])
        }),
        ("choices on a file", |s| {
            s["sources"][1]["fields"][3]["entryKinds"][0]["choices"] =
                json!([{"value":"x","title":"X"}])
        }),
        ("unknown input", |s| {
            s["sources"][1]["fields"][3]["entryKinds"][1]["input"] = json!("text")
        }),
        ("duplicate entry", |s| {
            s["sources"][1]["fields"][0]["value"][1] = json!({"kind":"folder","value":"src"})
        }),
        ("enum attribute without options", |s| {
            s["sources"][1]["fields"][3]["attributes"][1]["options"] = json!([])
        }),
    ];
    for (name, change) in cases {
        let mut bad = modules();
        change(&mut bad);
        if bad["sources"][1]["parent"].is_null() {
            bad["sources"][1].as_object_mut().unwrap().remove("parent");
        }
        assert_eq!(
            state_command(json!({"result":bad})).unwrap()["status"],
            "rejected",
            "{name}"
        );
    }
}

#[test]
fn sidebar_nesting_is_bounded() {
    let mut deep = modules();
    let template = deep["sources"][2].clone();
    let sources = deep["sources"].as_array_mut().unwrap();
    let mut parent = "core".to_string();
    for depth in 0..8 {
        let mut source = template.clone();
        let id = format!("level{depth}");
        source["id"] = json!(id);
        source["path"] = json!(format!("{id}/build.file"));
        source["parent"] = json!(parent);
        sources.push(source);
        parent = id;
    }
    assert_eq!(
        state_command(json!({"result":deep})).unwrap()["status"],
        "rejected"
    );
}
