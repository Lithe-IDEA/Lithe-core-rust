use super::support::{
    apply_run_changes, changed_run_document, changed_run_paths, edit_plugin_run_document,
    seed_generated, temporary_root, write_run_document, Cleanup,
};
use crate::execute_json;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

#[test]
fn generic_preference_edits_preserve_run_metadata_without_creating_legacy_defaults_or_files() {
    let root = temporary_root("generic-preference-editor");
    fs::create_dir_all(&root).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let call = |command: &str, payload: Value| -> Value {
        serde_json::from_str(&execute_json(
            &serde_json::json!({
                "id":"generic-preference-editor", "command":command, "payload":payload
            })
            .to_string(),
        ))
        .unwrap()
    };
    let edited = call(
        "toolchains.editConsumerPreferences",
        serde_json::json!({
            "document":{"version":1}, "directory":"services/api", "consumer":"dev.example.build/compile",
            "tool":"dev.example.runtime:sdk", "value":{"inherit":"consumer"}
        }),
    );
    assert_eq!(edited["ok"], true, "{edited}");
    assert_eq!(
        edited["data"]["edit"]["preferences"]["stored"],
        serde_json::json!({"inherit":"consumer"})
    );
    let local = root.join(".lithe/run/local.yaml");
    for original in [
        serde_json::json!({"version":2,"configurations":[]}),
        serde_json::json!({"version":2,"configurations":[{"id":"user:legacy","name":"Legacy","provider":"example.main","entry":"fixture.Main","cwd":".","future":"preserve"}]}),
        serde_json::json!({"version":2,"configurations":[{"id":"user:task","env":{"TOKEN":"fixture-value"},"future":[1,2]}],
            "futureMetadata":{"keep":true},"toolchain":{"runtime":{"homePath":"/fixture/legacy-runtime"},"futureToolchain":{"keep":true}}}),
    ] {
        write_run_document(&root, ".lithe/run/local.yaml", &original);
        let before = fs::read(&local).unwrap();
        let result = call(
            "runConfig.updateToolPreferences",
            serde_json::json!({"root":root,"preferences":edited["data"]["edit"]["document"]}),
        );
        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(
            changed_run_paths(&result["data"]),
            [".lithe/run/local.yaml"]
        );
        let saved = changed_run_document(&result["data"], ".lithe/run/local.yaml").unwrap();
        assert_eq!(
            saved["toolchain"]["preferences"],
            edited["data"]["edit"]["document"]
        );
        let mut without_preferences = saved;
        without_preferences["toolchain"]
            .as_object_mut()
            .unwrap()
            .remove("preferences");
        if original.get("toolchain").is_none() {
            without_preferences
                .as_object_mut()
                .unwrap()
                .remove("toolchain");
        }
        assert_eq!(without_preferences, original);
        // Core describes the edit; only the host writes it.
        assert_eq!(fs::read(&local).unwrap(), before);
    }

    // A JSON layer of an earlier release is migrated by its first edit: the
    // YAML document replaces it, keeping the fields Core does not know.
    fs::remove_file(&local).unwrap();
    write_run_document(
        &root,
        ".lithe/run/local.json",
        &serde_json::json!({"version":1,"configurations":[{"id":"user:legacy","type":"example.main","entry":"fixture.Main","workingDirectory":".","future":"preserve"}]}),
    );
    let migrated = call(
        "runConfig.updateToolPreferences",
        serde_json::json!({"root":root,"preferences":edited["data"]["edit"]["document"]}),
    );
    assert_eq!(migrated["ok"], true, "{migrated}");
    assert_eq!(
        changed_run_paths(&migrated["data"]),
        [".lithe/run/local.yaml", ".lithe/run/local.json"]
    );
    let saved = changed_run_document(&migrated["data"], ".lithe/run/local.yaml").unwrap();
    assert_eq!(saved["version"], 2);
    assert_eq!(saved["configurations"][0]["provider"], "example.main");
    assert_eq!(saved["configurations"][0]["cwd"], ".");
    assert_eq!(saved["configurations"][0]["future"], "preserve");
    apply_run_changes(&root, &migrated["data"]);
    assert!(!root.join(".lithe/run/local.json").exists());
    assert!(local.exists());
}

#[test]
fn generic_preference_editor_rejects_invalid_documents_and_references_without_writing() {
    let root = temporary_root("invalid-generic-preference-editor");
    fs::create_dir_all(&root).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let local = root.join(".lithe/run/local.yaml");
    for (document, preferences) in [
        (
            serde_json::json!({"version":4294967298_u64,"configurations":[]}),
            serde_json::json!({"version":1}),
        ),
        (
            serde_json::json!({"version":999,"configurations":[]}),
            serde_json::json!({"version":1}),
        ),
        (
            serde_json::json!({"version":2,"configurations":{}}),
            serde_json::json!({"version":1}),
        ),
        (
            serde_json::json!({"version":2,"configurations":[],"toolchain":42}),
            serde_json::json!({"version":1}),
        ),
        (
            serde_json::json!({"version":2,"configurations":[]}),
            serde_json::json!({"version":1,"workspace":{"tools":{"runtime":{"inherit":"workspace"}}}}),
        ),
    ] {
        write_run_document(&root, ".lithe/run/local.yaml", &document);
        let before = fs::read(&local).unwrap();
        let result: Value = serde_json::from_str(&execute_json(
            &serde_json::json!({
                "id":"invalid-generic-preferences", "command":"runConfig.updateToolPreferences",
                "payload":{"root":root,"preferences":preferences}
            })
            .to_string(),
        ))
        .unwrap();
        assert_eq!(result["ok"], false, "{document}: {result}");
        assert!(result["data"].is_null());
        assert_eq!(fs::read(&local).unwrap(), before);
    }
    // The document the host used to send is no longer accepted.
    let result: Value = serde_json::from_str(&execute_json(
        &serde_json::json!({
            "id":"invalid-generic-preferences", "command":"runConfig.updateToolPreferences",
            "payload":{"root":root,"localDocument":{"version":2,"configurations":[]},"preferences":{"version":1}}
        })
        .to_string(),
    ))
    .unwrap();
    assert_eq!(result["ok"], false, "{result}");
}

#[test]
fn project_tool_preferences_preserve_symbolic_choices_across_legacy_editor_saves_and_explicit_reset(
) {
    let root = temporary_root("project-tool-preferences");
    fs::create_dir_all(&root).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/toolchains/consumer-preferences.json"
    ))
    .unwrap();
    let call = |command: &str, payload: Value| -> Value {
        serde_json::from_str(&execute_json(
            &serde_json::json!({"id":"preferences-store","command":command,"payload":payload})
                .to_string(),
        ))
        .unwrap()
    };
    let document = serde_json::json!({"version":2,"configurations":[],"toolchain":{"preferences":fixture["document"]}});
    write_run_document(&root, ".lithe/run/local.yaml", &document);
    // An editor that knows nothing of preferences leaves them alone.
    let changed = call(
        "runConfig.updateOptions",
        serde_json::json!({"root":root,"scope":"local","configurationId":"",
        "toolchain":{"legacyHomePath":"/fixture/changed-home"}}),
    );
    assert_eq!(changed["ok"], true, "{changed}");
    assert!(changed_run_paths(&changed["data"]).is_empty(), "{changed}");
    let inspected = call(
        "runConfig.projectToolchain",
        serde_json::json!({"root":root,"path":"."}),
    );
    assert_eq!(inspected["ok"], true, "{inspected}");
    assert_eq!(inspected["data"]["toolPreferences"], fixture["document"]);
    let reset = call(
        "runConfig.updateOptions",
        serde_json::json!({"root":root,"scope":"local","configurationId":"",
        "toolchain":{"preferences":{"version":1}}}),
    );
    assert_eq!(reset["ok"], true, "{reset}");
    let saved = changed_run_document(&reset["data"], ".lithe/run/local.yaml").unwrap();
    assert_eq!(
        saved["toolchain"]["preferences"],
        serde_json::json!({"version":1})
    );
    apply_run_changes(&root, &reset["data"]);
    let inspected = call(
        "runConfig.projectToolchain",
        serde_json::json!({"root":root,"path":"."}),
    );
    assert_eq!(
        inspected["data"]["toolPreferences"],
        serde_json::json!({"version":1})
    );
    assert!(!root.join(".lithe/run/local.json").exists());
}

#[test]
fn project_tool_preferences_reject_invalid_references_in_reads_and_edits_without_clearing_sources()
{
    let root = temporary_root("invalid-tool-preferences");
    fs::create_dir_all(&root).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let invalid = serde_json::json!({"version":1,"workspace":{"tools":{"dev.example.runtime:sdk":{"inherit":"workspace"}}}});
    for (command, payload) in [
        (
            "runConfig.projectToolchain",
            serde_json::json!({"root":root,"path":".","localDocument":{"version":2,"configurations":[],"toolchain":{"preferences":invalid}}}),
        ),
        (
            "runConfig.updateOptions",
            serde_json::json!({"root":root,"scope":"local","configurationId":"","localDocument":{"version":2,"configurations":[]},"toolchain":{"preferences":invalid}}),
        ),
    ] {
        let reply: Value = serde_json::from_str(&execute_json(
            &serde_json::json!({"id":"invalid-preferences","command":command,"payload":payload})
                .to_string(),
        ))
        .unwrap();
        assert_eq!(reply["ok"], false, "{reply}");
        assert!(reply["data"].is_null());
    }
    assert!(!root.join(".lithe/run/local.json").exists());
}

/// Native editors format exact argv without shell expansion; an empty quoted
/// item must survive the common document mutation path on both platforms.
#[test]
fn argument_editor_text_roundtrips_the_shared_argv_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/run-configuration/argument-text.json"
    ))
    .unwrap();
    let root = temporary_root("run-argument-text");
    fs::create_dir_all(&root).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let document = serde_json::json!({"version":2,"configurations":[{"id":"user:arguments","name":"Arguments",
        "provider":"process.executable","execution":"task","extensions":{"process":{"program":"tool"}}}]});
    write_run_document(&root, ".lithe/run/local.yaml", &document);
    for case in fixture["cases"].as_array().unwrap() {
        let reply: Value = serde_json::from_str(&execute_json(&serde_json::json!({"id":"argument-text","command":"runConfig.updateOptions",
            "payload":{"root":root,"scope":"local","configurationId":"user:arguments","arguments":case["text"]}}).to_string())).unwrap();
        assert_eq!(reply["ok"], true, "{reply}");
        let result = changed_run_document(&reply["data"], ".lithe/run/local.yaml").unwrap();
        assert_eq!(result["configurations"][0]["args"], case["values"]);
    }
    assert!(!root.join(".lithe/run/local.json").exists());
}

#[test]
fn resolve_prefers_a_host_provided_local_document() {
    let root = temporary_root("run-config-host-local");
    fs::create_dir_all(root.join(".lithe/run")).unwrap();
    seed_generated(
        &root,
        r#"{"version":2,"configurations":[{"id":"current-file","name":"Current File","provider":"example.current-file","execution":"application","toolchains":{"runtime":"project-runtime"}}]}"#,
    );
    fs::write(
        root.join(".lithe/run/local.json"),
        r#"{"version":2,"configurations":[{"id":"current-file","name":"Project Local","provider":"example.current-file","cwd":"."}]}"#,
    )
    .unwrap();

    let resolve: Value = serde_json::from_str(&execute_json(
        &serde_json::json!({
            "id": "resolve-host-local",
            "command": "runConfig.resolve",
            "payload": {
                "root": root,
                "localDocument": {
                    "version": 2,
                    "configurations": [{
                        "id": "current-file",
                        "name": "This PC",
                        "provider": "example.current-file",
                        "cwd": "."
                    }]
                }
            }
        })
        .to_string(),
    ))
    .unwrap();
    assert_eq!(resolve["ok"], true, "{resolve}");
    let current = resolve["data"]["configurations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["id"] == "current-file")
        .unwrap();
    assert_eq!(current["name"], "This PC");

    // A legacy v1 local layer supplied by the host migrates like the on-disk
    // document, so an old `.lithe/run/local.json` read by the adapter still works.
    let legacy: Value = serde_json::from_str(&execute_json(
        &serde_json::json!({
            "id": "resolve-host-local-v1",
            "command": "runConfig.resolve",
            "payload": {
                "root": root,
                "localDocument": {
                    "version": 1,
                    "configurations": [{
                        "id": "current-file",
                        "name": "Legacy This PC",
                        "type": "example.current-file",
                        "programArguments": ["--dev"]
                    }]
                }
            }
        })
        .to_string(),
    ))
    .unwrap();
    assert_eq!(legacy["ok"], true, "{legacy}");
    let legacy_current = legacy["data"]["configurations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["id"] == "current-file")
        .unwrap();
    assert_eq!(legacy_current["name"], "Legacy This PC");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn editor_save_updates_generic_local_toolchains_without_dropping_other_entries() {
    let root = temporary_root("run-config-editor-runtime-toolchain");
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(root.join(".lithe/run")).unwrap();
    fs::create_dir_all(root.join(".lithe/toolchains")).unwrap();
    fs::create_dir_all(root.join("web")).unwrap();
    seed_generated(
        &root,
        r#"{"version":2,"configurations":[{"id":"npm:dev","name":"dev","provider":"npm.script","execution":"service","command":"npm","args":["run","dev"],"cwd":"web","toolchains":{"runtime":"project-node"}}]}"#,
    );
    fs::write(
        root.join(".lithe/toolchains/local.json"),
        r#"{"version":1,"toolchains":{"project-go":{"executable":"C:/Go/bin/go.exe"}}}"#,
    )
    .unwrap();

    let save = |node_path: &str| -> Value {
        run_config_command(
            "runConfig.saveEditorChanges",
            serde_json::json!({
                "root": root,
                "scope": "local",
                "configurationId": "npm:dev",
                "workingDirectory": "web",
                "toolchain": {
                    "runtimeExecutablePaths": {"project-node": node_path}
                }
            }),
        )
    };

    let saved = save("C:/Program Files/nodejs/node.exe");
    assert_eq!(saved["ok"], true, "{saved}");
    let toolchains = changed_run_document(&saved["data"], ".lithe/toolchains/local.json")
        .expect("the editor save writes the local toolchains");
    assert_eq!(
        toolchains["toolchains"]["project-node"]["executable"],
        "C:/Program Files/nodejs/node.exe"
    );
    assert_eq!(
        toolchains["toolchains"]["project-go"]["executable"],
        "C:/Go/bin/go.exe"
    );
    apply_run_changes(&root, &saved["data"]);

    let removed = save("");
    assert_eq!(removed["ok"], true, "{removed}");
    let removed_toolchains =
        changed_run_document(&removed["data"], ".lithe/toolchains/local.json").unwrap();
    assert!(removed_toolchains["toolchains"]
        .get("project-node")
        .is_none());
    assert_eq!(
        removed_toolchains["toolchains"]["project-go"]["executable"],
        "C:/Go/bin/go.exe"
    );

    // The second save was not applied, so the first one is what is read.
    let inspected = run_config_command("runConfig.inspect", serde_json::json!({"root": root}));
    assert_eq!(
        inspected["data"]["localToolchains"]["toolchains"]["project-node"]["executable"],
        "C:/Program Files/nodejs/node.exe"
    );
}

#[test]
fn run_configuration_inspect_reports_malformed_and_unsupported_documents() {
    let root = temporary_root("run-config-errors");
    fs::create_dir_all(root.join(".lithe/run")).unwrap();
    fs::write(root.join(".lithe/run/configurations.json"), "{").unwrap();

    let inspect = |id: &str| -> Value {
        serde_json::from_str(&execute_json(
            &serde_json::json!({
                "id": id,
                "command": "runConfig.inspect",
                "payload": {"root": root}
            })
            .to_string(),
        ))
        .unwrap()
    };
    let malformed = inspect("malformed");
    assert_eq!(malformed["ok"], false);
    assert_eq!(malformed["error"]["code"], "parse_failed");

    fs::write(
        root.join(".lithe/run/configurations.json"),
        r#"{"version":3,"configurations":[]}"#,
    )
    .unwrap();
    let unsupported = inspect("unsupported");
    assert_eq!(unsupported["ok"], false);
    assert_eq!(unsupported["error"]["code"], "not_supported");
    assert!(unsupported["error"]["details"]
        .as_str()
        .unwrap()
        .contains("found 3"));

    fs::write(
        root.join(".lithe/run/configurations.json"),
        r#"{"version":1,"configurations":[]}"#,
    )
    .unwrap();
    fs::create_dir_all(root.join(".lithe/toolchains")).unwrap();
    fs::write(root.join(".lithe/toolchains/local.json"), "{").unwrap();
    let malformed_toolchains = inspect("malformed-toolchains");
    assert_eq!(malformed_toolchains["ok"], false);
    assert_eq!(malformed_toolchains["error"]["code"], "parse_failed");
    assert!(malformed_toolchains["error"]["message"]
        .as_str()
        .unwrap()
        .contains(".lithe/toolchains/local.json"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn run_configuration_resolve_marks_unsafe_working_directories_unavailable() {
    let root = temporary_root("run-config-unsafe-paths");
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(root.join(".lithe/run")).unwrap();
    seed_generated(
        &root,
        r#"{"version":1,"configurations":[{"id":"current-file","name":"Current File","type":"example.current-file","toolchains":{"runtime":"project-runtime"}}]}"#,
    );

    let resolve = || run_config_command("runConfig.resolve", serde_json::json!({"root": root}));
    let safe = resolve();
    assert_eq!(safe["ok"], true, "{safe}");
    assert!(safe["data"]["diagnostics"].as_array().unwrap().is_empty());

    write_run_document(
        &root,
        ".lithe/run/local.yaml",
        &serde_json::json!({"version":2,"configurations":[{"id":"current-file","cwd":"../outside"}]}),
    );
    // One bad entry stays listed and explains itself instead of failing the list.
    let unsafe_path = resolve();
    assert_eq!(unsafe_path["ok"], true, "{unsafe_path}");
    let current = unsafe_path["data"]["configurations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["id"] == "current-file")
        .expect("the invalid entry is still listed");
    assert_eq!(current["availability"]["status"], "unavailable");
    assert_eq!(current["availability"]["code"], "invalidConfiguration");
    assert!(unsafe_path["data"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value["id"] == "current-file" && value["code"] == "invalidConfiguration"));

    let plan = run_config_command(
        "runConfig.createLaunchPlan",
        serde_json::json!({"root": root, "configurationId": "current-file"}),
    );
    assert_eq!(plan["ok"], false, "{plan}");
    assert_eq!(plan["error"]["message"], "Run configuration is unavailable");
}

#[test]
fn shared_run_configuration_fixtures_have_the_versioned_contract_shape() {
    let directory =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/run-configuration");
    let mut fixture_count = 0;
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        fixture_count += 1;
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["version"], 1, "{}", path.display());
        assert!(value["expected"].is_object(), "{}", path.display());
        if let Some(generated) = value.get("generated") {
            assert!(generated["version"].is_number(), "{}", path.display());
            assert!(generated["configurations"].is_array(), "{}", path.display());
        }
    }
    assert!(fixture_count >= 6);
}

#[test]
fn shared_editor_save_fixture_executes_the_document_contract() {
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/run-configuration/editor-save.json");
    let fixture: Value = serde_json::from_str(&fs::read_to_string(fixture_path).unwrap()).unwrap();
    let root = temporary_root("run-config-editor-fixture");
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(root.join(".lithe/run")).unwrap();
    fs::create_dir_all(root.join("backend")).unwrap();
    seed_generated(
        &root,
        r#"{"version":2,"configurations":[{"id":"server","name":"Server","provider":"build.service","execution":"service","toolchains":{"runtime":"project-runtime"},"extensions":{"build":{"module":"."}}}]}"#,
    );

    let mut request = fixture["request"].clone();
    request["payload"]["root"] = serde_json::json!(root);
    let response: Value = serde_json::from_str(&execute_json(&request.to_string())).unwrap();

    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(
        serde_json::json!(changed_run_paths(&response["data"])),
        fixture["expected"]["changedPaths"]
    );
    apply_run_changes(&root, &response["data"]);
    let resolved = run_config_command("runConfig.resolve", serde_json::json!({"root": root}));
    assert_eq!(resolved["ok"], true, "{resolved}");
    let server = &resolved["data"]["configurations"][0];
    assert_eq!(server["cwd"], "backend");
    assert_eq!(server["args"], serde_json::json!(["--dev"]));
    assert_eq!(server["env"]["APP_ENV"], "dev");
    assert_eq!(
        resolved["data"]["localToolchains"]["toolchains"]["project-runtime"]["executable"],
        "C:/lithe-fixture/runtime/bin/runtime.exe"
    );
}

/// `project.json` and the toolchain files sit under `.lithe` and carry their
/// own `version: 1`, unrelated to the run-configuration schema. Migration
/// must not touch them, or resolve rejects a perfectly valid project.
#[test]
fn migration_leaves_sidecar_documents_at_their_own_version() {
    let root = temporary_root("run-config-sidecar");
    fs::create_dir_all(root.join(".lithe/run")).unwrap();
    fs::create_dir_all(root.join(".lithe/toolchains")).unwrap();
    seed_generated(
        &root,
        r#"{"version":1,"configurations":[{"id":"current-file","name":"Current File","type":"example.current-file"}]}"#,
    );
    fs::write(
        root.join(".lithe/project.json"),
        r#"{"version":1,"defaultRunConfiguration":"current-file"}"#,
    )
    .unwrap();
    fs::write(
        root.join(".lithe/toolchains/local.json"),
        r#"{"version":1,"toolchains":{}}"#,
    )
    .unwrap();

    let resolved: Value = serde_json::from_str(&execute_json(
        &serde_json::json!({
            "id": "sidecar",
            "command": "runConfig.resolve",
            "payload": {"root": root}
        })
        .to_string(),
    ))
    .unwrap();
    assert_eq!(resolved["ok"], true, "{resolved}");
    assert_eq!(resolved["data"]["version"], 2);
    assert_eq!(resolved["data"]["defaultRunConfiguration"], "current-file");

    fs::remove_dir_all(root).unwrap();
}

/// A service that names no toolchain reaches a launch plan with its own
/// command and no toolchain binding.
#[test]
fn process_configurations_launch_without_a_toolchain() {
    let root = temporary_root("run-config-process");
    fs::create_dir_all(root.join(".lithe/run")).unwrap();
    fs::create_dir_all(root.join("frontend")).unwrap();
    seed_generated(
        &root,
        r#"{"version":2,"configurations":[{
            "id":"npm:dev",
            "name":"web dev",
            "provider":"npm.script",
            "execution":"service",
            "confidence":"declared",
            "command":"npm",
            "args":["run","dev"],
            "cwd":"frontend",
            "env":{"PORT":"3000"},
            "toolchains":{}
        }]}"#,
    );

    let plan: Value = serde_json::from_str(&execute_json(
        &serde_json::json!({
            "id": "process-plan",
            "command": "runConfig.createLaunchPlan",
            "payload": {"root": root, "configurationId": "npm:dev"}
        })
        .to_string(),
    ))
    .unwrap();
    assert_eq!(plan["ok"], true, "{plan}");
    assert_eq!(plan["data"]["executable"]["command"], "npm");
    assert!(plan["data"]["executable"]["toolchain"].is_null());
    assert_eq!(plan["data"]["arguments"], serde_json::json!(["run", "dev"]));
    assert_eq!(plan["data"]["workingDirectory"], "frontend");
    assert_eq!(plan["data"]["env"]["PORT"], "3000");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn toolchain_backed_process_uses_the_generic_runtime_binding() {
    let root = temporary_root("run-config-go-toolchain");
    fs::create_dir_all(root.join(".lithe/run")).unwrap();
    seed_generated(
        &root,
        r#"{"version":2,"configurations":[{
            "id":"go:api","name":"Go API","provider":"go.main",
            "execution":"application","args":["run","./cmd/api"],"cwd":".",
            "env":{"APP_ENV":"dev"},"toolchains":{"runtime":"project-go"}
        }]}"#,
    );

    let plan: Value = serde_json::from_str(&execute_json(
        &serde_json::json!({
            "id": "go-toolchain-plan",
            "command": "runConfig.createLaunchPlan",
            "payload": {"root": root, "configurationId": "go:api"}
        })
        .to_string(),
    ))
    .unwrap();
    assert_eq!(plan["ok"], true, "{plan}");
    assert_eq!(plan["data"]["executable"]["toolchain"], "project-go");
    assert!(plan["data"]["executable"]["command"].is_null());
    assert_eq!(
        plan["data"]["arguments"],
        serde_json::json!(["run", "./cmd/api"])
    );
    assert_eq!(plan["data"]["env"]["APP_ENV"], "dev");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unknown_provider_without_an_executable_is_rejected() {
    let root = temporary_root("run-config-unknown-provider");
    fs::create_dir_all(root.join(".lithe/run")).unwrap();
    seed_generated(
        &root,
        r#"{"version":2,"configurations":[{
            "id":"zig:app","name":"Zig App","provider":"zig.main",
            "args":[],"cwd":".","toolchains":{}
        }]}"#,
    );
    let plan: Value = serde_json::from_str(&execute_json(
        &serde_json::json!({
            "id": "unknown-provider-plan",
            "command": "runConfig.createLaunchPlan",
            "payload": {"root": root, "configurationId": "zig:app"}
        })
        .to_string(),
    ))
    .unwrap();
    assert_eq!(plan["ok"], false, "{plan}");
    assert_eq!(plan["error"]["code"], "invalid_request");
    assert!(plan["error"]["message"]
        .as_str()
        .unwrap_or("")
        .contains("command or runtime toolchain"));

    fs::remove_dir_all(root).unwrap();
}

/// Generic editor options must patch the common process shape. Writing
/// them into a provider extension makes the UI appear to save successfully
/// while process launch plans continue using the old arguments.
#[test]
fn process_options_update_common_arguments_and_environment() {
    let root = temporary_root("run-config-process-options");
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(root.join(".lithe/run")).unwrap();
    fs::create_dir_all(root.join("backend")).unwrap();
    seed_generated(
        &root,
        r#"{"version":2,"configurations":[{
            "id":"python:api","name":"API","provider":"python.script",
            "command":"python3","args":["app.py"],"cwd":".","toolchains":{}
        }]}"#,
    );

    let updated = run_config_command(
        "runConfig.updateOptions",
        serde_json::json!({
            "root": root,
            "scope": "local",
            "configurationId": "python:api",
            "workingDirectory": "backend",
            "arguments": "app.py --port 9000",
            "environment": {"APP_ENV": "test"}
        }),
    );
    assert_eq!(updated["ok"], true, "{updated}");
    assert_eq!(
        changed_run_paths(&updated["data"]),
        [".lithe/run/local.yaml"]
    );
    let document = changed_run_document(&updated["data"], ".lithe/run/local.yaml").unwrap();
    let patch = &document["configurations"][0];
    assert_eq!(
        patch["args"],
        serde_json::json!(["app.py", "--port", "9000"])
    );
    assert_eq!(patch["env"]["APP_ENV"], "test");
    assert!(patch.get("extensions").is_none(), "{patch}");

    apply_run_changes(&root, &updated["data"]);
    let plan = run_config_command(
        "runConfig.createLaunchPlan",
        serde_json::json!({"root": root, "configurationId": "python:api"}),
    );
    assert_eq!(plan["ok"], true, "{plan}");
    assert_eq!(
        plan["data"]["arguments"],
        serde_json::json!(["app.py", "--port", "9000"])
    );
    assert_eq!(plan["data"]["env"]["APP_ENV"], "test");
}

/// An absolute or relative path would let a project manifest point the IDE
/// at an executable of its choosing. Commands resolve on PATH only.
#[test]
fn process_configurations_reject_path_qualified_commands() {
    let root = temporary_root("run-config-process-path");
    fs::create_dir_all(root.join(".lithe/run")).unwrap();
    seed_generated(
        &root,
        r#"{"version":2,"configurations":[{
            "id":"evil","name":"evil","provider":"shell.command",
            "command":"../../../usr/bin/curl","args":[],"cwd":".","toolchains":{}
        }]}"#,
    );

    let plan: Value = serde_json::from_str(&execute_json(
        &serde_json::json!({
            "id": "evil-plan",
            "command": "runConfig.createLaunchPlan",
            "payload": {"root": root, "configurationId": "evil"}
        })
        .to_string(),
    ))
    .unwrap();
    assert_eq!(plan["ok"], false);
    assert_eq!(plan["error"]["code"], "invalid_request");

    fs::remove_dir_all(root).unwrap();
}

/// Without a default in `.lithe/project.json`, a generated service is chosen
/// before a plain application; a project default still wins when available.
#[test]
fn resolve_chooses_generated_default_unless_the_project_names_one() {
    let root = temporary_root("run-config-generated-default");
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(&root).unwrap();
    seed_generated(
        &root,
        serde_json::json!({"version": 2, "configurations": [
            {"id": "current-file", "name": "Current File", "provider": "example.current-file",
             "execution": "application", "toolchains": {}},
            {"id": "example-main:demo.Tool", "name": "Tool", "provider": "example.main",
             "execution": "application", "toolchains": {}},
            {"id": "build.service:.", "name": "app", "provider": "build.service",
             "execution": "service", "toolchains": {}}
        ]}),
    );
    let generated = run_config_command("runConfig.resolve", serde_json::json!({ "root": root }));
    assert_eq!(generated["ok"], true, "{generated}");
    assert_eq!(
        generated["data"]["defaultRunConfiguration"],
        "build.service:."
    );

    fs::create_dir_all(root.join(".lithe")).unwrap();
    fs::write(
        root.join(".lithe/project.json"),
        r#"{"version":1,"defaultRunConfiguration":"example-main:demo.Tool"}"#,
    )
    .unwrap();
    let named = run_config_command("runConfig.resolve", serde_json::json!({ "root": root }));
    assert_eq!(
        named["data"]["defaultRunConfiguration"],
        "example-main:demo.Tool"
    );
}

fn run_config_command(command: &str, payload: Value) -> Value {
    serde_json::from_str(&execute_json(
        &serde_json::json!({ "id": command, "command": command, "payload": payload }).to_string(),
    ))
    .unwrap()
}

/// Issue #861: resolution hides a configuration whose `cwd` is missing, so a
/// value that cannot name a project directory is rejected when saved, where
/// the editor shows the error, instead of making the entry vanish. Editor
/// variables are not expanded even when the directory they point at exists.
#[test]
fn working_directory_override_must_name_an_existing_project_directory() {
    let root = temporary_root("run-config-invalid-cwd");
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(root.join(".lithe/run")).unwrap();
    fs::create_dir_all(root.join("backend")).unwrap();
    seed_generated(
        &root,
        r#"{"version":2,"configurations":[{
            "id":"python:api","name":"API","provider":"python.script",
            "command":"python3","args":["app.py"],"cwd":".","toolchains":{}
        }]}"#,
    );

    for (directory, message) in [
        (
            "${workspaceFolder}/backend",
            "Working directory variables are not supported; use a path relative to the project",
        ),
        (
            "$PROJECT_DIR$/backend",
            "Working directory variables are not supported; use a path relative to the project",
        ),
        ("frontend", "Project configuration directory does not exist"),
        (
            "../outside",
            "Project configuration paths must stay inside the project",
        ),
    ] {
        for scope in ["local", "project"] {
            let updated = run_config_command(
                "runConfig.updateOptions",
                serde_json::json!({
                    "root": root, "scope": scope, "configurationId": "python:api",
                    "workingDirectory": directory
                }),
            );
            assert_eq!(updated["ok"], false, "{directory}: {updated}");
            assert_eq!(updated["error"]["code"], "invalid_request");
            assert_eq!(updated["error"]["message"], message, "{directory}");
        }
    }

    let absolute = root.join("backend").canonicalize().unwrap();
    let updated = run_config_command(
        "runConfig.updateOptions",
        serde_json::json!({
            "root": root, "scope": "local", "configurationId": "python:api",
            "workingDirectory": absolute
        }),
    );
    assert_eq!(updated["ok"], true, "{updated}");
    let document = changed_run_document(&updated["data"], ".lithe/run/local.yaml").unwrap();
    assert_eq!(document["configurations"][0]["cwd"], "backend");
}

/// Saving a selector never stores an executable, generates a model, or loses local defaults.
#[test]
fn build_provider_configurations_persist_before_generation_and_reuse_identity() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/run-configuration/build-provider.json"
    ))
    .unwrap();
    let root = temporary_root("saved-build-provider");
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(root.join("app")).unwrap();
    write_run_document(&root, ".lithe/run/local.yaml", &fixture["unrelated"]);
    let local = root.join(".lithe/run/local.yaml");
    let unrelated = fs::read(&local).unwrap();
    // A build run is recorded with the plugin that provides it, apart from
    // the Lithe document.
    let plugin_path = ".lithe/run/plugin/dev.example.gradle.local.toml";
    let call = |command: &str, payload: Value| run_config_command(command, payload);
    let save = |reference: Value| {
        call(
            "runConfig.createUserConfiguration",
            serde_json::json!({"root":root,"scope":"local","type":"buildProvider","name":fixture["name"],"build":reference}),
        )
    };
    let saved = save(fixture["reference"].clone());
    assert_eq!(saved["ok"], true, "{saved}");
    assert_eq!(changed_run_paths(&saved["data"]), [plugin_path]);
    let document = changed_run_document(&saved["data"], plugin_path).unwrap();
    assert_eq!(document["plugin"]["id"], "dev.example.gradle");
    assert_eq!(
        document["configurations"][0]["extensions"]["build"],
        fixture["reference"]
    );
    assert!(document["configurations"][0].get("command").is_none());
    // The Core mutation is read-only until the platform commits it.
    assert_eq!(fs::read(&local).unwrap(), unrelated);
    assert!(!root.join(plugin_path).exists());
    apply_run_changes(&root, &saved["data"]);
    edit_plugin_run_document(&root, plugin_path, |document| {
        document["configurations"][0]["extensions"]["annotations"] =
            serde_json::json!({"label":"keep user metadata"});
    });
    let inspected = call("runConfig.inspect", serde_json::json!({"root":root}));
    assert_eq!(
        inspected["data"]["status"], fixture["expected"]["inspectionStatus"],
        "{inspected}"
    );
    assert_eq!(
        inspected["data"]["requiresGeneration"],
        fixture["expected"]["requiresGeneration"]
    );
    let resolved = call("runConfig.resolve", serde_json::json!({"root":root}));
    assert_eq!(resolved["ok"], true, "{resolved}");
    assert_eq!(
        resolved["data"]["configurations"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        resolved["data"]["configurations"][0]["plugin"]["id"],
        "dev.example.gradle"
    );
    assert!(!root.join(".lithe/run/generated.json").exists());
    let mut changed = fixture["reference"].clone();
    changed["userArguments"] = serde_json::json!(["new argument"]);
    let updated = save(changed);
    assert_eq!(updated["data"]["id"], saved["data"]["id"]);
    let updated_doc = changed_run_document(&updated["data"], plugin_path).unwrap();
    assert_eq!(updated_doc["configurations"].as_array().unwrap().len(), 1);
    assert_eq!(
        updated_doc["configurations"][0]["extensions"]["annotations"],
        serde_json::json!({"label":"keep user metadata"})
    );
    for execution in fixture["executionKinds"].as_array().unwrap() {
        let mut reference = fixture["reference"].clone();
        reference["execution"] = execution.clone();
        let saved_kind = save(reference);
        assert_eq!(saved_kind["ok"], true, "{saved_kind}");
        assert_eq!(saved_kind["data"]["id"], saved["data"]["id"]);
        let kind_document = changed_run_document(&saved_kind["data"], plugin_path).unwrap();
        assert_eq!(kind_document["configurations"][0]["execution"], *execution);
        apply_run_changes(&root, &saved_kind["data"]);
        let kind_resolved = call("runConfig.resolve", serde_json::json!({"root":root}));
        assert_eq!(kind_resolved["ok"], true, "{kind_resolved}");
        assert_eq!(
            kind_resolved["data"]["configurations"][0]["execution"],
            *execution
        );
        let inputs = call(
            "runConfig.providerLaunchInputs",
            serde_json::json!({"root":root,"configurationId":saved_kind["data"]["id"],"providerRoot":"app"}),
        );
        assert_eq!(inputs["ok"], true, "{inputs}");
        assert_eq!(inputs["data"]["launchOptions"]["execution"], *execution);
        assert_eq!(inputs["data"]["reference"]["execution"], *execution);
    }
    for (field, value) in [
        ("root", serde_json::json!("../escape")),
        ("root", serde_json::json!("/outside")),
        ("root", serde_json::json!("app\\bad")),
        ("projectID", serde_json::json!("")),
        ("userArguments", serde_json::json!(["bad\u{0}"])),
        ("userArguments", serde_json::json!(vec!["a"; 65])),
        ("command", serde_json::json!("runtime")),
    ] {
        let mut invalid = fixture["reference"].clone();
        invalid[field] = value;
        assert_eq!(save(invalid)["ok"], false, "{field}");
    }
    assert_eq!(fs::read(&local).unwrap(), unrelated);
    edit_plugin_run_document(&root, plugin_path, |document| {
        document["configurations"][0]["extensions"]["build"]
            .as_object_mut()
            .unwrap()
            .remove("userArguments");
    });
    let normalized = call("runConfig.resolve", serde_json::json!({"root":root}));
    assert_eq!(normalized["ok"], true, "{normalized}");
    assert_eq!(
        normalized["data"]["configurations"][0]["extensions"]["build"]["userArguments"],
        serde_json::json!([])
    );
    // A hand-edited entry that cannot run is listed as unavailable; it never
    // reaches the provider.
    edit_plugin_run_document(&root, plugin_path, |document| {
        document["configurations"][0]["command"] = serde_json::json!("runtime");
    });
    let invalid = call("runConfig.resolve", serde_json::json!({"root":root}));
    assert_eq!(invalid["ok"], true, "{invalid}");
    assert_eq!(
        invalid["data"]["configurations"][0]["availability"]["code"],
        "invalidConfiguration"
    );
    let inputs = call(
        "runConfig.providerLaunchInputs",
        serde_json::json!({"root":root,"configurationId":saved["data"]["id"],"providerRoot":"app"}),
    );
    assert_eq!(inputs["ok"], false, "{inputs}");
    assert_eq!(
        inputs["error"]["message"],
        "Run configuration is unavailable"
    );
}

/// Persisted identity and selector identity serve different purposes: imported IDs
/// retain defaults/overrides, while the selector removes only the duplicate auto entry.
#[test]
fn explicit_provider_references_keep_arbitrary_ids_and_shadow_only_current_selector() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/run-configuration/build-provider-stable-identities.json"
    ))
    .unwrap();
    let root = temporary_root("provider-stable-identity");
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(root.join("app")).unwrap();
    fs::create_dir_all(root.join(".lithe/run")).unwrap();
    let call = |command: &str, payload: Value| -> Value {
        let reply: Value = serde_json::from_str(&execute_json(
            &serde_json::json!({"id":"stable","command":command,"payload":payload}).to_string(),
        ))
        .unwrap();
        assert_eq!(reply["ok"], true, "{reply}");
        reply["data"].clone()
    };
    let stable = fixture["expected"]["stableID"].clone();
    let reference = fixture["reference"].clone();
    let team = serde_json::json!({"version":2,"configurations":[{"id":stable,"name":"Team service","provider":"build.provider","execution":"service","cwd":"app","extensions":{"build":reference}}]});
    let team_path = root.join(".lithe/run/configurations.json");
    let local_path = root.join(".lithe/run/local.json");
    fs::write(&team_path, team.to_string()).unwrap();
    let inspected = call("runConfig.inspect", serde_json::json!({"root":root}));
    assert_eq!(inspected["status"], "ready");
    assert_eq!(inspected["requiresGeneration"], true);
    let before = call("runConfig.resolve", serde_json::json!({"root":root}));
    assert_eq!(before["configurations"][0]["id"], stable);
    assert!(before["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .all(|d| d["code"] != "orphanedOverride"));
    let mut second = reference.clone();
    second["targetID"] = serde_json::json!("another/opaque-target");
    let generated = call(
        "runConfig.generate",
        serde_json::json!({"root":root,"providerRuns":[
            {"projectPath":"app","name":"Auto A","reference":reference},{"projectPath":"app","name":"Auto B","reference":second}
        ]}),
    );
    assert_eq!(
        generated["generated"]["configurations"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    fs::write(
        root.join(".lithe/project.json"),
        serde_json::json!({"version":1,"defaultRunConfiguration":stable}).to_string(),
    )
    .unwrap();
    fs::write(
        &local_path,
        serde_json::json!({"version":2,"configurations":[{"id":stable,"env":{"EXAMPLE":"value"}}]})
            .to_string(),
    )
    .unwrap();
    let resolved = call("runConfig.resolve", serde_json::json!({"root":root}));
    let configs = resolved["configurations"].as_array().unwrap();
    assert_eq!(configs.len(), 2);
    assert_eq!(resolved["defaultRunConfiguration"], stable);
    assert_eq!(
        configs
            .iter()
            .filter(|c| c["extensions"]["build"]["targetID"] == reference["targetID"])
            .count(),
        1
    );
    assert!(resolved["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .all(|d| d["code"] != "orphanedOverride"));
    // A local selector patch replaces the effective claim; it must not hide
    // the formerly claimed auto target too.
    fs::write(&local_path,serde_json::json!({"version":2,"configurations":[{"id":stable,"extensions":{"build":{"targetID":second["targetID"]}}}]}).to_string()).unwrap();
    let replaced = call("runConfig.resolve", serde_json::json!({"root":root}));
    let configs = replaced["configurations"].as_array().unwrap();
    assert_eq!(configs.len(), 2);
    assert_eq!(
        configs
            .iter()
            .filter(|c| c["extensions"]["build"]["targetID"] == second["targetID"])
            .count(),
        1
    );
    assert_eq!(
        configs
            .iter()
            .filter(|c| c["extensions"]["build"]["targetID"] == reference["targetID"])
            .count(),
        1
    );
    let original_team = fs::read(&team_path).unwrap();
    let original_local = fs::read(&local_path).unwrap();
    let prepared = serde_json::json!({"version":2,"configurations":[{"id":"prepared:stable","name":"Prepared","provider":"build.provider","execution":"service","extensions":{"build":reference}}]});
    let preview = call(
        "runConfig.resolve",
        serde_json::json!({"root":root,"teamDocument":prepared,"localDocument":{"version":2,"configurations":[]}}),
    );
    assert!(preview["configurations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["id"] == "prepared:stable"));
    assert_eq!(fs::read(&team_path).unwrap(), original_team);
    assert_eq!(fs::read(&local_path).unwrap(), original_local);
    // A malformed prepared entry previews as unavailable instead of failing
    // the whole list.
    let mut invalid = prepared.clone();
    invalid["configurations"][0]["extensions"]["build-run"] = serde_json::json!({"entrypoint":17});
    let previewed = call(
        "runConfig.resolve",
        serde_json::json!({"root":root,"teamDocument":invalid,"localDocument":{"version":2,"configurations":[]}}),
    );
    let prepared_entry = previewed["configurations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "prepared:stable")
        .expect("the malformed entry is still listed");
    assert_eq!(
        prepared_entry["availability"]["code"],
        "invalidConfiguration"
    );
    assert_eq!(fs::read(&team_path).unwrap(), original_team);
}

/// Portable choices survive document edits without becoming selector identity or cached launch facts.
#[test]
fn provider_run_options_preserve_exact_arguments_and_layered_build_choices() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/run-configuration/build-provider-options.json"
    ))
    .unwrap();
    let root = temporary_root("provider-run-options");
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(root.join("app/work")).unwrap();
    fs::create_dir_all(root.join(".lithe/run")).unwrap();
    let call = |command: &str, payload: Value| -> Value {
        let reply = run_config_command(command, payload);
        assert_eq!(reply["ok"], true, "{reply}");
        reply["data"].clone()
    };
    let plugin_path = ".lithe/run/plugin/dev.example.engine.local.toml";
    let save = || {
        call(
            "runConfig.createUserConfiguration",
            serde_json::json!({"root":root,"scope":"local","type":"buildProvider","name":"Service","build":fixture["reference"]}),
        )
    };
    let saved = save();
    let id = saved["id"].clone();
    assert_eq!(changed_run_paths(&saved), [plugin_path]);
    apply_run_changes(&root, &saved);
    let team = serde_json::json!({"version":2,"configurations":[{"id":id,"extensions":{"build-run":fixture["choices"]}}]});
    write_run_document(&root, ".lithe/run/configurations.yaml", &team);
    let team_path = root.join(".lithe/run/configurations.yaml");
    let original_team = fs::read(&team_path).unwrap();
    let read = || {
        call(
            "runConfig.providerLaunchInputs",
            serde_json::json!({"root":root,"configurationId":id,"providerRoot":"app"}),
        )
    };
    let initial = read();
    assert_eq!(initial["tools"], fixture["choices"]["tools"]);
    assert!(initial["launchOptions"].get("tools").is_none());
    assert_eq!(
        initial["buildSystemID"],
        fixture["expected"]["buildSystemID"]
    );
    assert_eq!(
        initial["launchOptions"]["execution"],
        fixture["expected"]["execution"]
    );
    assert_eq!(
        initial["launchOptions"]["runtimeArguments"],
        fixture["choices"]["runtimeArguments"]
    );
    assert_eq!(
        initial["launchOptions"]["entrypoint"],
        fixture["choices"]["entrypoint"]
    );
    assert_eq!(initial["optionOverrides"], fixture["choices"]["options"]);
    assert_eq!(
        initial["launchOptions"]["arguments"],
        fixture["reference"]["userArguments"]
    );
    let update = |choices: Option<Value>| {
        let mut payload = serde_json::json!({"root":root,"scope":"local","configurationId":id,
            "arguments":"--application=\"one two\" 雪","environment":fixture["environment"],"workingDirectory":fixture["workingDirectory"]});
        if let Some(choices) = choices {
            payload["providerRunOptions"] = choices;
        }
        call("runConfig.updateOptions", payload)
    };
    // Local edits stay with the plugin's local record; the team layer is
    // only read.
    let one_tool = update(Some(
        serde_json::json!({"tools":{"dev.example.engine:compiler":fixture["changed"]["tools"]["dev.example.engine:compiler"]}}),
    ));
    assert_eq!(changed_run_paths(&one_tool), [plugin_path]);
    apply_run_changes(&root, &one_tool);
    let inherited = read();
    assert_eq!(
        inherited["tools"]["dev.example.engine:compiler"],
        fixture["changed"]["tools"]["dev.example.engine:compiler"]
    );
    assert_eq!(
        inherited["tools"]["dev.example.engine:vm"],
        fixture["choices"]["tools"]["dev.example.engine:vm"]
    );
    let document = update(Some(fixture["changed"].clone()));
    apply_run_changes(&root, &document);
    let changed = read();
    assert_eq!(changed["tools"], fixture["expected"]["tools"]);
    let toolchain = call(
        "runConfig.projectToolchain",
        serde_json::json!({"root":root,"path":"app","configurationId":id}),
    );
    assert_eq!(toolchain["definedTools"], changed["tools"]);
    assert!(
        !root.join(".lithe/local.json").exists(),
        "Run choices must not create directory pins"
    );
    assert_eq!(
        changed["launchOptions"]["runtimeArguments"],
        fixture["changed"]["runtimeArguments"]
    );
    assert!(
        changed["launchOptions"].get("entrypoint").is_none(),
        "Explicit empty clears the team entrypoint"
    );
    assert_eq!(
        changed["optionOverrides"],
        serde_json::json!({}),
        "An explicit empty map clears team options"
    );
    assert_eq!(changed["launchOptions"]["arguments"], fixture["arguments"]);
    assert_eq!(
        changed["launchOptions"]["environment"],
        fixture["environment"]
    );
    assert_eq!(
        changed["launchOptions"]["workingDirectory"],
        fixture["workingDirectory"]
    );
    assert_eq!(changed["reference"], fixture["reference"]);
    // The plugin record spells a cleared tool choice in TOML, so compare what
    // Core reads back rather than the bytes.
    let omitted = update(None);
    apply_run_changes(&root, &omitted);
    assert_eq!(read(), changed, "Omitted choices keep the saved ones");
    let resaved = save();
    assert_eq!(resaved["id"], id);
    apply_run_changes(&root, &resaved);
    let resaved = read();
    assert_eq!(
        resaved["launchOptions"]["workingDirectory"],
        fixture["workingDirectory"]
    );
    assert_eq!(resaved["tools"], changed["tools"]);
    assert_eq!(
        resaved["launchOptions"]["runtimeArguments"],
        changed["launchOptions"]["runtimeArguments"]
    );
    assert_eq!(resaved["optionOverrides"], changed["optionOverrides"]);
    assert_eq!(fs::read(&team_path).unwrap(), original_team);
    assert!(!root.join(".lithe/run/generated.json").exists());
}

/// Malformed portable choices fail before a provider request; unknown fields cannot smuggle launch facts.
#[test]
fn provider_run_options_reject_malformed_or_oversized_values() {
    let root = temporary_root("provider-run-options-invalid");
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(root.join(".lithe/run")).unwrap();
    let reference = serde_json::json!({"pluginID":"dev.example","buildSystemID":"engine","root":".","run":"serve","projectID":"p","targetID":"t"});
    for choices in [
        serde_json::json!(null),
        serde_json::json!({"runtimeArguments":null}),
        serde_json::json!({"runtimeArguments":["bad\u{0}"]}),
        serde_json::json!({"runtimeArguments":vec!["a";1025]}),
        serde_json::json!({"runtimeArguments":["a".repeat(65537)]}),
        serde_json::json!({"entrypoint":null}),
        serde_json::json!({"entrypoint":"a".repeat(4097)}),
        serde_json::json!({"options":null}),
        serde_json::json!({"options":{"bad key":true}}),
        serde_json::json!({"options":{"opaque":"a".repeat(65536)}}),
        serde_json::json!({"options":(0..65).map(|n|(format!("key-{n}"),serde_json::json!(false))).collect::<std::collections::BTreeMap<_,_>>()}),
        serde_json::json!({"tools":null}),
        serde_json::json!({"tools":{"bad key":{"home":"/kit"}}}),
        serde_json::json!({"tools":{"dev.example:compiler":{"home":"relative/kit"}}}),
        serde_json::json!({"tools":{"dev.example:compiler":{"executable":"/a/../b"}}}),
        serde_json::json!({"tools":{"dev.example:compiler":{"home":"/kit","executable":"/bin/compiler"}}}),
        serde_json::json!({"tools":{"dev.example:compiler":{"home":"/kit","version":"21"}}}),
        serde_json::json!({"tools":(0..65).map(|n|(format!("dev.example:tool-{n}"),serde_json::json!(null))).collect::<std::collections::BTreeMap<_,_>>()}),
        serde_json::json!({"command":"cached"}),
        serde_json::json!({"debug":{"adapter":"cached"}}),
    ] {
        write_run_document(
            &root,
            ".lithe/run/local.yaml",
            &serde_json::json!({"version":2,"configurations":[{"id":"user:service","name":"Service","provider":"build.provider","execution":"application","cwd":".","extensions":{"build":reference,"build-run":choices}}]}),
        );
        // The list still resolves, with the entry marked unavailable.
        let reply = run_config_command("runConfig.resolve", serde_json::json!({"root":root}));
        assert_eq!(reply["ok"], true, "{choices}: {reply}");
        let entry = reply["data"]["configurations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == "user:service")
            .expect("the invalid entry is still listed");
        assert_eq!(
            entry["availability"]["code"], "invalidConfiguration",
            "Invalid choice accepted: {choices}"
        );
        let inputs = run_config_command(
            "runConfig.providerLaunchInputs",
            serde_json::json!({"root":root,"configurationId":"user:service","providerRoot":"."}),
        );
        assert_eq!(
            inputs["ok"], false,
            "Invalid choice reached the provider: {choices}"
        );
    }
    // The mutation port must reject explicit null as well as persisted null,
    // rather than treating malformed input as an omitted legacy field.
    write_run_document(
        &root,
        ".lithe/run/local.yaml",
        &serde_json::json!({"version":2,"configurations":[{"id":"user:service","name":"Service","provider":"build.provider","execution":"application","cwd":".","extensions":{"build":reference}}]}),
    );
    for options in [
        serde_json::json!(null),
        serde_json::json!({"runtimeArguments":null}),
        serde_json::json!({"entrypoint":"bad\u{0}"}),
    ] {
        let reply = run_config_command(
            "runConfig.updateOptions",
            serde_json::json!({"root":root,"scope":"local","configurationId":"user:service","providerRunOptions":options}),
        );
        assert_eq!(reply["ok"], false, "Invalid mutation accepted");
    }
}

/// A build provider owns discovery; regenerating labels must retain user choices,
/// while withdrawing a target must not resurrect it from the previous generated layer.
#[test]
fn generic_provider_runs_regenerate_without_language_facts_and_keep_stable_overrides() {
    let root = temporary_root("generic-provider-generation");
    fs::create_dir_all(root.join("app")).unwrap();
    fs::create_dir_all(root.join(".lithe/run")).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let call = |command: &str, payload: Value| -> Value {
        let result: Value = serde_json::from_str(&execute_json(
            &serde_json::json!({"id":"generic","command":command,"payload":payload}).to_string(),
        ))
        .unwrap();
        assert_eq!(result["ok"], true, "{result}");
        result["data"].clone()
    };
    let generate = |runs: Value| {
        call(
            "runConfig.generate",
            serde_json::json!({
                "root":root,"providerRuns":runs
            }),
        )
    };
    let reference = serde_json::json!({"pluginID":"dev.example.engine","buildSystemID":"engine","root":"app",
        "run":"serve","projectID":"project/opaque:1","targetID":"target/opaque:2","execution":"service","userArguments":[]});
    let mut runs =
        serde_json::json!([{"projectPath":"app","name":"Initial label","reference":reference}]);
    let first = generate(runs.clone());
    let configuration = &first["generated"]["configurations"][0];
    let id = configuration["id"].clone();
    assert_eq!(configuration["provider"], "build.provider");
    assert!(configuration["toolchains"].as_object().unwrap().is_empty());
    assert!(
        configuration["extensions"]
            .as_object()
            .unwrap()
            .keys()
            .all(|key| key == "build"),
        "{configuration}"
    );
    fs::write(
        root.join(".lithe/run/local.json"),
        serde_json::json!({"version":2,"configurations":[
            {"id":id,"env":{"EXAMPLE":"preserved"},"args":["user choice"],"cwd":"app"}
        ]})
        .to_string(),
    )
    .unwrap();
    let resolved = call("runConfig.resolve", serde_json::json!({"root":root}));
    assert_eq!(resolved["defaultRunConfiguration"], id);
    runs[0]["name"] = serde_json::json!("Renamed upstream label");
    runs[0]["reference"]["execution"] = serde_json::json!("task");
    let renamed = generate(runs.clone());
    assert_eq!(renamed["generated"]["configurations"][0]["id"], id);
    assert_ne!(
        renamed["generated"]["generator"]["providerRunFingerprint"],
        first["generated"]["generator"]["providerRunFingerprint"]
    );
    let inputs = call(
        "runConfig.providerLaunchInputs",
        serde_json::json!({"root":root,"configurationId":id,"providerRoot":"app"}),
    );
    assert_eq!(inputs["launchOptions"]["execution"], "task");
    assert_eq!(inputs["launchOptions"]["workingDirectory"], "app");
    assert_eq!(
        inputs["launchOptions"]["arguments"],
        serde_json::json!(["user choice"])
    );
    assert_eq!(
        inputs["launchOptions"]["environment"],
        serde_json::json!({"EXAMPLE":"preserved"})
    );
    let withdrawn = generate(serde_json::json!([]));
    assert!(!withdrawn["generated"]["configurations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == id));
    let resolved = call("runConfig.resolve", serde_json::json!({"root":root}));
    assert!(resolved["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["code"] == "orphanedOverride" && item["id"] == id));
    assert!(!resolved["configurations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == id));
    let restored = generate(runs);
    assert_eq!(restored["generated"]["configurations"][0]["id"], id);
    let inputs = call(
        "runConfig.providerLaunchInputs",
        serde_json::json!({"root":root,"configurationId":id,"providerRoot":"app"}),
    );
    assert_eq!(
        inputs["launchOptions"]["arguments"],
        serde_json::json!(["user choice"])
    );
    assert!(!root.join(".lithe/run/generated.json").exists());
}

/// Providers receive saved typed choices; Core does not assemble a build goal or reread a build file.
#[test]
fn provider_launch_inputs_preserve_document_layers_and_current_selector() {
    let root = temporary_root("provider-launch-inputs");
    fs::create_dir_all(root.join(".lithe/run")).unwrap();
    fs::create_dir_all(root.join("nested/app")).unwrap();
    fs::create_dir_all(root.join("tools")).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    let _cleanup = Cleanup(root.clone());
    fs::write(
        root.join("nested/build.manifest"),
        "deliberately not a manifest",
    )
    .unwrap();
    let generated = serde_json::json!({"version":2,"configurations":[{"id":"service","name":"Service","provider":"build.provider","execution":"service",
        "cwd":"nested","env":{},"extensions":{"build":{"pluginID":"dev.example.engine","buildSystemID":"engine","root":"nested","run":"serve",
        "projectID":"project","targetID":"target","execution":"service"},"build-run":{"runtimeArguments":["--message=base value"]}}}]});
    seed_generated(&root, generated.to_string());
    let local = serde_json::json!({"version":2,"configurations":[{"id":"service","cwd":"tools","env":{"EXAMPLE":"value"},
        "args":["a b","雪",""],"extensions":{"build-run":{"runtimeArguments":["--message=hello world","C:\\literal"],"entrypoint":"opaque/entry:1","options":{"features":["run-feature"],"strict":false}}}}]});
    let call = |provider_root: &str, local: Option<Value>| -> Value {
        let request = serde_json::json!({"id":"inputs","command":"runConfig.providerLaunchInputs","payload":{
            "root":root,"configurationId":"service","providerRoot":provider_root,"localDocument":local}});
        serde_json::from_str(&execute_json(&request.to_string())).unwrap()
    };
    let inspect = || -> Value {
        serde_json::from_str(&execute_json(
            &serde_json::json!({"id":"inspect-launch-inputs","command":"runConfig.inspect",
            "payload":{"root":root}})
            .to_string(),
        ))
        .unwrap()
    };
    let baseline = inspect()["data"]["generated"].clone();
    let original = call("nested", None);
    assert_eq!(original["ok"], true, "{original}");
    assert_eq!(original["data"]["projectPath"], "nested");
    assert_eq!(
        original["data"]["launchOptions"]["runtimeArguments"],
        serde_json::json!(["--message=base value"])
    );
    assert!(original["data"]["launchOptions"]
        .get("workingDirectory")
        .is_none());
    let resolved = call("nested", Some(local.clone()));
    assert_eq!(resolved["ok"], true, "{resolved}");
    assert_eq!(resolved["data"]["provider"], "serve");
    assert_eq!(
        resolved["data"]["launchOptions"],
        serde_json::json!({"execution":"service","runtimeArguments":["--message=hello world","C:\\literal"],
        "arguments":["a b","雪",""],"environment":{"EXAMPLE":"value"},"entrypoint":"opaque/entry:1","workingDirectory":"tools"})
    );
    assert_eq!(
        resolved["data"]["optionOverrides"],
        serde_json::json!({"features":["run-feature"],"strict":false})
    );
    assert!(resolved["data"].get("executable").is_none());
    assert_eq!(call(".", Some(local.clone()))["ok"], false);
    assert_eq!(call("../outside", Some(local.clone()))["ok"], false);
    fs::remove_dir_all(root.join("tools")).unwrap();
    assert_eq!(call("nested", Some(local))["ok"], false);
    assert_eq!(inspect()["data"]["generated"], baseline);
    assert!(!root.join(".lithe/run/generated.json").exists());
}

/// Historical discriminators are data for a temporary host import, never a
/// capability to make Core interpret profiles, frameworks or module launches.
#[test]
fn provider_launch_inputs_require_a_typed_reference_after_domain_migration() {
    let root = temporary_root("unmigrated-provider-inputs");
    fs::create_dir_all(&root).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    for provider in ["legacy.module", "legacy.framework", "legacy.service"] {
        let generated = serde_json::json!({"version":2,"configurations":[{"id":"legacy","name":"Legacy",
            "provider":provider,"execution":"task","extensions":{"legacy":{"module":".","profiles":["saved"],"skipTests":false}}}]});
        seed_generated(&root, generated.to_string());
        let reply: Value = serde_json::from_str(&execute_json(
            &serde_json::json!({"id":"unmigrated","command":"runConfig.providerLaunchInputs",
            "payload":{"root":root,"configurationId":"legacy","providerRoot":"."}})
            .to_string(),
        ))
        .unwrap();
        assert_eq!(reply["ok"], false, "{reply}");
        assert_eq!(reply["error"]["code"], "invalid_request");
    }
    assert!(!root.join(".lithe/run/local.json").exists());
}

/// Generation materializes only the provider runs. Names that repeat across
/// roots are qualified by the root they run in, and inspection reports a layer
/// generated from another provider snapshot as stale.
#[test]
fn provider_run_generation_qualifies_repeated_names_and_reports_stale_snapshots() {
    let root = temporary_root("provider-run-generation");
    for directory in ["services/api", "services/worker", "tools"] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let call = |command: &str, payload: Value| -> Value {
        let result: Value = serde_json::from_str(&execute_json(
            &serde_json::json!({"id":"provider-runs","command":command,"payload":payload})
                .to_string(),
        ))
        .unwrap();
        assert_eq!(result["ok"], true, "{result}");
        result["data"].clone()
    };
    let run = |root: &str, name: &str, target: &str| {
        serde_json::json!({"projectPath":root,"name":name,"reference":{
            "pluginID":"dev.lithe.test.engine","buildSystemID":"engine","root":root,
            "run":"serve","projectID":root,"targetID":target,"userArguments":[]}})
    };
    let runs = serde_json::json!([
        run("services/api", "serve", "api"),
        run("services/worker", "serve", "worker"),
        run("tools", "lint", "lint"),
    ]);

    let generated = call(
        "runConfig.generate",
        serde_json::json!({"root":root,"providerRuns":runs}),
    );
    assert_eq!(generated["entryCount"], 3);
    let mut names = generated["generated"]["configurations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            assert_eq!(item["provider"], "build.provider");
            item["name"].as_str().unwrap().to_string()
        })
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(
        names,
        ["lint", "serve (services/api)", "serve (services/worker)"]
    );

    let stale = |runs: Value| -> bool {
        call(
            "runConfig.inspect",
            serde_json::json!({"root":root,"providerRuns":runs}),
        )["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["code"] == "staleProviderModel")
    };
    assert!(!stale(runs.clone()));
    assert!(stale(serde_json::json!([run("tools", "lint", "lint")])));

    // Without a provider snapshot Core has nothing to generate.
    let empty = call("runConfig.generate", serde_json::json!({"root":root}));
    assert_eq!(empty["entryCount"], 0);
    assert!(empty["generated"]["configurations"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!root.join(".lithe").exists());
}

/// Issue #2100: shell, PowerShell and batch scripts and programs run without
/// any plugin. Core describes their editor, checks and places a save in the
/// local or project layer, and plans the launch; the host only applies the
/// returned changes.
#[test]
fn built_in_process_runs_are_created_edited_moved_and_deleted_through_the_editor_form() {
    let root = temporary_root("process-run-editor");
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::write(root.join("scripts/build.sh"), "#!/bin/bash -e\necho hi\n").unwrap();
    let call = |command: &str, payload: Value| run_config_command(command, payload);
    let local = ".lithe/run/local.yaml";
    let project = ".lithe/run/configurations.yaml";

    // A project with no build model and no saved run has an empty list, so the
    // first configuration can be created there.
    let empty = call("runConfig.resolve", serde_json::json!({"root": root}));
    assert_eq!(empty["ok"], true, "{empty}");
    assert_eq!(empty["data"]["configurations"], serde_json::json!([]));
    assert_eq!(empty["data"]["diagnostics"], serde_json::json!([]));

    let types = call("runConfig.configurationTypes", serde_json::json!({}));
    let providers: Vec<_> = types["data"]["types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["provider"].as_str().unwrap())
        .collect();
    // Every platform creates every kind; only batch says it does not run
    // outside Windows.
    assert_eq!(
        providers,
        [
            "process.shell",
            "process.powershell",
            "process.batch",
            "process.executable"
        ]
    );
    for entry in types["data"]["types"].as_array().unwrap() {
        let unavailable = entry["provider"] == "process.batch" && !cfg!(windows);
        assert_eq!(
            entry["availability"]["code"],
            if unavailable {
                serde_json::json!("unsupportedPlatform")
            } else {
                Value::Null
            },
            "{entry}"
        );
    }

    // A new editor shows the automatic values the typed script implies.
    let form = call(
        "runConfig.editorForm",
        serde_json::json!({"root":root,"provider":"process.shell","values":{"script":"scripts/build.sh"}}),
    );
    assert_eq!(form["ok"], true, "{form}");
    assert_eq!(form["data"]["formVersion"], 1);
    let field = |form: &Value, id: &str| -> Value {
        form["data"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|field| field["id"] == id)
            .cloned()
            .unwrap_or(Value::Null)
    };
    assert_eq!(field(&form, "name")["automatic"], "build");
    let interpreter = if cfg!(windows) { "bash" } else { "/bin/bash" };
    assert_eq!(field(&form, "interpreter")["automatic"], interpreter);
    assert_eq!(field(&form, "interpreterArguments")["automatic"], "-e");
    assert_eq!(field(&form, "script")["visibleWhen"]["equals"], "file");
    assert!(field(&form, "executionPolicyBypass").is_null());
    assert!(field(&form, "program").is_null());

    // The interpreter field names the family of shell that runs the kind, so a
    // host can offer the installed shells of it; a program has no interpreter.
    for (provider, family) in [
        ("process.shell", "posix"),
        ("process.powershell", "powershell"),
        ("process.batch", "cmd"),
    ] {
        let form = call(
            "runConfig.editorForm",
            serde_json::json!({"root":root,"provider":provider}),
        );
        assert_eq!(form["ok"], true, "{form}");
        assert_eq!(
            field(&form, "interpreter")["shellFamily"],
            family,
            "{provider}"
        );
    }
    let program = call(
        "runConfig.editorForm",
        serde_json::json!({"root":root,"provider":"process.executable"}),
    );
    assert_eq!(program["ok"], true, "{program}");
    assert!(field(&program, "interpreter").is_null());
    assert!(field(&program, "program")["shellFamily"].is_null());

    // Wrong values come back by field and change nothing.
    let invalid = call(
        "runConfig.saveConfiguration",
        serde_json::json!({"root":root,"provider":"process.shell","values":{
            "script":"missing.sh","workingDirectory":"nowhere",
            "environment":[{"name":"A","value":"1"},{"name":"A","value":"2"}]
        }}),
    );
    assert_eq!(invalid["data"]["status"], "invalid", "{invalid}");
    assert_eq!(
        invalid["data"]["errors"],
        serde_json::json!([
            {"field":"script","code":"notFound"},
            {"field":"workingDirectory","code":"notFound"},
            {"field":"environment","code":"duplicate"}
        ])
    );
    // An empty text is reported once, on the field the chosen source shows.
    let empty_text = call(
        "runConfig.saveConfiguration",
        serde_json::json!({"root":root,"provider":"process.shell","values":{
            "name":"inline","source":"text","text":"  "
        }}),
    );
    assert_eq!(
        empty_text["data"]["errors"],
        serde_json::json!([{"field":"text","code":"required"}]),
        "{empty_text}"
    );

    let saved = call(
        "runConfig.saveConfiguration",
        serde_json::json!({"root":root,"provider":"process.shell","values":{
            "name":"","storeAsProjectFile":false,"source":"file","script":"scripts/build.sh",
            "scriptOptions":"--fast 'a b'","workingDirectory":"scripts",
            "environment":[{"name":"MODE","value":"dev"},{"name":"","value":""}],
            "interpreter":"","interpreterArguments":""
        }}),
    );
    assert_eq!(saved["data"]["status"], "saved", "{saved}");
    assert_eq!(saved["data"]["id"], "user:build");
    assert_eq!(changed_run_paths(&saved["data"]), [local]);
    let document = changed_run_document(&saved["data"], local).unwrap();
    assert_eq!(
        document["configurations"][0],
        serde_json::json!({
            "id":"user:build","name":"build","provider":"process.shell","confidence":"declared",
            "args":["--fast","a b"],"cwd":"scripts","env":{"MODE":"dev"},
            "extensions":{"process":{"script":"scripts/build.sh"}}
        })
    );
    assert!(!root.join(local).exists(), "Core writes nothing");
    apply_run_changes(&root, &saved["data"]);

    let resolved = call("runConfig.resolve", serde_json::json!({"root":root}));
    let configuration = &resolved["data"]["configurations"][0];
    assert_eq!(configuration["id"], "user:build", "{resolved}");
    assert!(configuration["availability"].is_null());
    let plan = call(
        "runConfig.createLaunchPlan",
        serde_json::json!({"root":root,"configurationId":"user:build"}),
    );
    assert_eq!(plan["ok"], true, "{plan}");
    assert_eq!(plan["data"]["workingDirectory"], "scripts");
    let arguments = plan["data"]["arguments"].as_array().unwrap();
    assert_eq!(arguments[0], "-e");
    assert_eq!(
        arguments[2..],
        [serde_json::json!("--fast"), serde_json::json!("a b")]
    );

    // A later release's keys survive an edit, and the entry moves whole to
    // the project file when it is stored there.
    let mut stored: Value =
        serde_yaml_ng::from_slice(&fs::read(root.join(local)).unwrap()).unwrap();
    stored["configurations"][0]["extensions"]["process"]["future"] = serde_json::json!(true);
    stored["configurations"][0]["extensions"]["annotations"] = serde_json::json!({"keep":1});
    write_run_document(&root, local, &stored);
    let edit = call(
        "runConfig.editorForm",
        serde_json::json!({"root":root,"configurationId":"user:build"}),
    );
    let mut values = edit["data"]["values"].clone();
    assert_eq!(values["scriptOptions"], "--fast \"a b\"", "{edit}");
    assert_eq!(values["storeAsProjectFile"], false);
    assert_eq!(
        values["environment"],
        serde_json::json!([{"name":"MODE","value":"dev"}])
    );
    values["storeAsProjectFile"] = serde_json::json!(true);
    values["interpreter"] = serde_json::json!("zsh");
    let moved = call(
        "runConfig.saveConfiguration",
        serde_json::json!({"root":root,"provider":"process.shell","configurationId":"user:build","values":values}),
    );
    assert_eq!(moved["data"]["status"], "saved", "{moved}");
    assert_eq!(changed_run_paths(&moved["data"]), [project, local]);
    let document = changed_run_document(&moved["data"], project).unwrap();
    let entry = &document["configurations"][0];
    assert_eq!(entry["extensions"]["process"]["future"], true);
    assert_eq!(entry["extensions"]["process"]["interpreter"], "zsh");
    assert_eq!(entry["extensions"]["annotations"]["keep"], 1);
    apply_run_changes(&root, &moved["data"]);
    let edit = call(
        "runConfig.editorForm",
        serde_json::json!({"root":root,"configurationId":"user:build"}),
    );
    assert_eq!(edit["data"]["values"]["storeAsProjectFile"], true);
    // A named shell interpreter does not take the `#!` line's options.
    assert!(field(&edit, "interpreterArguments")["automatic"].is_null());

    // Inline text needs a host temporary location to be written to.
    let inline = call(
        "runConfig.saveConfiguration",
        serde_json::json!({"root":root,"provider":"process.shell","values":{
            "name":"Greet","source":"text","text":"echo hi","script":"ignored.sh"
        }}),
    );
    assert_eq!(inline["data"]["id"], "user:greet", "{inline}");
    apply_run_changes(&root, &inline["data"]);
    let base = root.join("tmp-run").to_string_lossy().into_owned();
    let plan = call(
        "runConfig.createLaunchPlan",
        serde_json::json!({"root":root,"configurationId":"user:greet","inlineScriptBase":base}),
    );
    assert_eq!(
        plan["data"]["inlineScript"]["content"], "echo hi\n",
        "{plan}"
    );
    let missing = call(
        "runConfig.createLaunchPlan",
        serde_json::json!({"root":root,"configurationId":"user:greet"}),
    );
    assert_eq!(missing["ok"], false);

    let deleted = call(
        "runConfig.deleteConfiguration",
        serde_json::json!({"root":root,"configurationId":"user:build"}),
    );
    assert_eq!(changed_run_paths(&deleted["data"]), [project], "{deleted}");
    apply_run_changes(&root, &deleted["data"]);
    let resolved = call("runConfig.resolve", serde_json::json!({"root":root}));
    let ids: Vec<_> = resolved["data"]["configurations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["id"].clone())
        .collect();
    assert_eq!(ids, [serde_json::json!("user:greet")]);
    let again = call(
        "runConfig.deleteConfiguration",
        serde_json::json!({"root":root,"configurationId":"user:build"}),
    );
    assert_eq!(again["ok"], false);
}

/// Batch scripts run only on Windows. Elsewhere a saved one stays listed as
/// unavailable, so a project shared with Windows keeps it, and it can still be
/// created, edited and saved; launching it is refused.
#[test]
fn batch_runs_are_editable_but_unavailable_outside_windows() {
    if cfg!(windows) {
        return;
    }
    let root = temporary_root("process-run-batch");
    let _cleanup = Cleanup(root.clone());
    write_run_document(
        &root,
        ".lithe/run/configurations.yaml",
        &serde_json::json!({"version":2,"configurations":[{
            "id":"user:setup","name":"setup","provider":"process.batch",
            "extensions":{"process":{"script":"setup.cmd"}}
        }]}),
    );
    fs::write(root.join("setup.cmd"), "@echo off\r\necho setup\r\n").unwrap();
    let resolved = run_config_command("runConfig.resolve", serde_json::json!({"root":root}));
    assert_eq!(resolved["ok"], true, "{resolved}");
    let listed = &resolved["data"]["configurations"][0];
    assert_eq!(listed["id"], "user:setup", "{resolved}");
    assert_eq!(listed["availability"]["status"], "unavailable");
    assert_eq!(listed["availability"]["code"], "unsupportedPlatform");
    assert_eq!(resolved["data"]["diagnostics"], serde_json::json!([]));

    let form = run_config_command(
        "runConfig.editorForm",
        serde_json::json!({"root":root,"configurationId":"user:setup"}),
    );
    assert_eq!(form["ok"], true, "{form}");
    assert_eq!(form["data"]["provider"], "process.batch");
    assert_eq!(form["data"]["values"]["script"], "setup.cmd");
    assert_eq!(form["data"]["availability"]["code"], "unsupportedPlatform");
    let new = run_config_command(
        "runConfig.editorForm",
        serde_json::json!({"root":root,"provider":"process.batch"}),
    );
    assert_eq!(new["ok"], true, "{new}");
    assert_eq!(new["data"]["availability"]["code"], "unsupportedPlatform");

    let mut values = form["data"]["values"].clone();
    values["scriptOptions"] = serde_json::json!("--fast");
    let saved = run_config_command(
        "runConfig.saveConfiguration",
        serde_json::json!({"root":root,"provider":"process.batch","configurationId":"user:setup","values":values}),
    );
    assert_eq!(saved["ok"], true, "{saved}");
    assert_eq!(saved["data"]["status"], "saved", "{saved}");

    let plan = run_config_command(
        "runConfig.createLaunchPlan",
        serde_json::json!({"root":root,"configurationId":"user:setup"}),
    );
    assert_eq!(plan["ok"], false, "{plan}");
}

/// A saved run whose working directory was removed stays listed as unavailable,
/// so the user can fix or delete it; launching it is refused.
#[test]
fn saved_runs_with_a_missing_working_directory_stay_listed_as_unavailable() {
    let root = temporary_root("process-run-missing-cwd");
    let _cleanup = Cleanup(root.clone());
    write_run_document(
        &root,
        ".lithe/run/local.yaml",
        &serde_json::json!({"version":2,"configurations":[{
            "id":"user:list","name":"list","provider":"process.executable",
            "cwd":"gone","extensions":{"process":{"program":"ls"}}
        }]}),
    );
    let resolved = run_config_command("runConfig.resolve", serde_json::json!({"root":root}));
    assert_eq!(resolved["ok"], true, "{resolved}");
    let configuration = &resolved["data"]["configurations"][0];
    assert_eq!(configuration["id"], "user:list", "{resolved}");
    assert_eq!(configuration["disabled"], false);
    assert_eq!(
        configuration["availability"]["code"],
        "missingWorkingDirectory"
    );
    assert_eq!(
        resolved["data"]["diagnostics"][0]["code"],
        "missingWorkingDirectory"
    );
    let plan = run_config_command(
        "runConfig.createLaunchPlan",
        serde_json::json!({"root":root,"configurationId":"user:list"}),
    );
    assert_eq!(plan["ok"], false, "{plan}");
}

/// PowerShell bypasses the execution policy unless the user unticks it, and a
/// program is kept as a `PATH` name or saved as a path inside the project.
#[test]
fn powershell_and_program_editors_store_their_own_choices() {
    let root = temporary_root("process-run-kinds");
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("tool"), "").unwrap();
    let call = |command: &str, payload: Value| run_config_command(command, payload);
    let ids = |form: &Value| -> Vec<String> {
        form["data"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|field| field["id"].as_str().unwrap().to_string())
            .collect()
    };

    let powershell = call(
        "runConfig.editorForm",
        serde_json::json!({"root":root,"provider":"process.powershell"}),
    );
    assert_eq!(
        powershell["data"]["values"]["executionPolicyBypass"], true,
        "{powershell}"
    );
    let bypass = powershell["data"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["id"] == "executionPolicyBypass")
        .unwrap();
    assert_eq!(bypass["note"], "windowsOnly");
    let saved = call(
        "runConfig.saveConfiguration",
        serde_json::json!({"root":root,"provider":"process.powershell","values":{
            "name":"Deploy","source":"text","text":"Write-Host hi","executionPolicyBypass":false
        }}),
    );
    let document = changed_run_document(&saved["data"], ".lithe/run/local.yaml").unwrap();
    assert_eq!(
        document["configurations"][0]["extensions"]["process"],
        serde_json::json!({"text":"Write-Host hi","executionPolicyBypass":false}),
        "{saved}"
    );

    let program = call(
        "runConfig.editorForm",
        serde_json::json!({"root":root,"provider":"process.executable"}),
    );
    assert_eq!(
        ids(&program),
        [
            "name",
            "storeAsProjectFile",
            "program",
            "programArguments",
            "workingDirectory",
            "environment",
            "beforeLaunch"
        ]
    );
    let save_program = |program: &str| {
        let saved = call(
            "runConfig.saveConfiguration",
            serde_json::json!({"root":root,"provider":"process.executable","values":{"program":program}}),
        );
        changed_run_document(&saved["data"], ".lithe/run/local.yaml")
            .map(|document| document["configurations"][0].clone())
            .unwrap_or(saved)
    };
    let by_name = save_program("ls");
    assert_eq!(by_name["name"], "ls");
    assert_eq!(by_name["extensions"]["process"]["program"], "ls");
    assert_eq!(
        save_program(&root.join("tool").to_string_lossy())["extensions"]["process"]["program"],
        "./tool"
    );
    assert_eq!(
        save_program("./missing")["data"]["errors"],
        serde_json::json!([{"field":"program","code":"notFound"}])
    );
}

/// An entrypoint run (`buildProviders` 1) is saved without a target, with the
/// entrypoint it starts; each entrypoint is its own configuration, saving the
/// same one again reuses it, and the launch inputs carry the entrypoint.
#[test]
fn entrypoint_runs_are_saved_per_entrypoint_without_a_target() {
    let root = temporary_root("saved-entrypoint-run");
    let _cleanup = Cleanup(root.clone());
    fs::create_dir_all(root.join("app")).unwrap();
    let plugin_path = ".lithe/run/plugin/dev.example.gradle.local.toml";
    let reference = serde_json::json!({
        "pluginID": "dev.example.gradle", "buildSystemID": "gradle", "root": "app",
        "run": "main", "projectID": "project-id", "userArguments": []
    });
    let save = |name: &str, entrypoint: &str| {
        run_config_command(
            "runConfig.createUserConfiguration",
            serde_json::json!({"root":root,"scope":"local","type":"buildProvider","name":name,
                "build":reference,"entrypoint":entrypoint}),
        )
    };
    let tool = save("Tool", "example.Tool");
    assert_eq!(tool["ok"], true, "{tool}");
    let document = changed_run_document(&tool["data"], plugin_path).unwrap();
    let saved = &document["configurations"][0];
    assert_eq!(
        saved["extensions"]["build-run"]["entrypoint"],
        "example.Tool"
    );
    assert!(
        saved["extensions"]["build"]
            .get("targetID")
            .is_none_or(|id| id == ""),
        "{saved}"
    );
    apply_run_changes(&root, &tool["data"]);

    let again = save("Tool", "example.Tool");
    assert_eq!(again["data"]["id"], tool["data"]["id"]);
    let other = save("Other", "example.Other");
    assert_eq!(other["ok"], true, "{other}");
    assert_ne!(other["data"]["id"], tool["data"]["id"]);
    apply_run_changes(&root, &other["data"]);

    let resolved = run_config_command("runConfig.resolve", serde_json::json!({"root":root}));
    assert_eq!(resolved["ok"], true, "{resolved}");
    assert_eq!(
        resolved["data"]["configurations"].as_array().unwrap().len(),
        2
    );
    let inputs = run_config_command(
        "runConfig.providerLaunchInputs",
        serde_json::json!({"root":root,"configurationId":tool["data"]["id"],"providerRoot":"app"}),
    );
    assert_eq!(inputs["ok"], true, "{inputs}");
    assert_eq!(
        inputs["data"]["launchOptions"]["entrypoint"],
        "example.Tool"
    );
    assert_eq!(save("Bad", "bad\u{0}")["ok"], false);
}
