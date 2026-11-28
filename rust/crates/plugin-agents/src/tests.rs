use super::*;
use serde_json::json;

fn agent(value: Value) -> AgentDeclaration {
    serde_json::from_value(value).expect("agent declaration decodes")
}

fn sample() -> Value {
    json!({
        "id": "sample",
        "displayName": "Sample Agent",
        "tool": "sample-cli",
        "native": { "function": "agent.launch" },
        "terminal": { "arguments": [] },
        "update": { "arguments": ["update"] },
        "sessionSources": [{
            "id": "projects",
            "roots": ["${env:SAMPLE_HOME}/sessions", "${userHome}/.sample/sessions"],
            "include": ["**/*.jsonl"],
            "format": "jsonLines",
            "scan": "sessions.scan",
            "read": "sessions.read"
        }]
    })
}

fn tools() -> BTreeSet<String> {
    BTreeSet::from(["sample-cli".to_string()])
}

fn check(agents: &mut [AgentDeclaration], has_backend: bool) -> Validation<()> {
    let required_tools = tools();
    validate_declarations(
        agents,
        &DeclarationContext {
            required_tools: &required_tools,
            has_backend,
        },
    )
}

fn rejected_field(value: Value, has_backend: bool) -> String {
    let error = check(&mut [agent(value)], has_backend).expect_err("declaration is refused");
    assert_eq!(error.code, "plugin.manifest.invalid");
    error.field.expect("rejection names a field")
}

#[test]
fn accepts_a_complete_agent_and_sorts_by_id() {
    let mut second = sample();
    second["id"] = json!("another");
    let mut agents = [agent(sample()), agent(second)];
    check(&mut agents, true).expect("agents validate");
    assert_eq!(agents[0].id, "another");
    assert_eq!(activation_event(&agents[1]), "onAgent:sample");
    assert!(uses_backend(&agents));
    assert!(has_native(&agents));
}

#[test]
fn rejects_unknown_fields() {
    let mut value = sample();
    value["environment"] = json!({});
    assert!(serde_json::from_value::<AgentDeclaration>(value).is_err());
}

#[test]
fn tool_must_be_a_required_tool() {
    let mut value = sample();
    value["tool"] = json!("other-cli");
    assert_eq!(rejected_field(value, true), "agents[0].tool");
}

#[test]
fn an_agent_declares_at_least_one_form() {
    let value = json!({ "id": "sample", "displayName": "Sample", "tool": "sample-cli" });
    assert_eq!(rejected_field(value, true), "agents[0]");
}

#[test]
fn native_sessions_and_sources_need_a_backend() {
    assert_eq!(rejected_field(sample(), false), "agents[0]");
    let terminal_only = json!({
        "id": "sample",
        "displayName": "Sample",
        "tool": "sample-cli",
        "terminal": { "arguments": ["--interactive"] }
    });
    check(&mut [agent(terminal_only)], false).expect("terminal agents need no backend");
}

#[test]
fn duplicate_ids_are_refused() {
    let mut agents = [agent(sample()), agent(sample())];
    let error = check(&mut agents, true).expect_err("duplicate is refused");
    assert_eq!(error.field.as_deref(), Some("agents[1].id"));
}

#[test]
fn launch_timeout_is_bounded() {
    let mut value = sample();
    value["native"]["timeoutMilliseconds"] = json!(500);
    assert_eq!(
        rejected_field(value, true),
        "agents[0].native.timeoutMilliseconds"
    );
    let native: NativeDeclaration =
        serde_json::from_value(json!({ "function": "agent.launch" })).unwrap();
    assert_eq!(
        native.timeout_milliseconds(),
        DEFAULT_LAUNCH_TIMEOUT_MILLISECONDS
    );
}

#[test]
fn update_runs_the_tool_with_arguments() {
    let mut empty = sample();
    empty["update"] = json!({ "arguments": [] });
    assert_eq!(rejected_field(empty, true), "agents[0].update.arguments");

    let mut installer = sample();
    installer["update"] = json!({ "installer": "sample-installer" });
    assert!(serde_json::from_value::<AgentDeclaration>(installer).is_err());
}

#[test]
fn roots_start_with_a_variable_and_stay_relative() {
    for root in [
        "/etc/sessions",
        "${userHome}/../other",
        "${env:lower}/x",
        "${userHome}x",
        "${userHome}/${env:HOME}",
        "~/sessions",
    ] {
        let mut value = sample();
        value["sessionSources"][0]["roots"] = json!([root]);
        assert_eq!(
            rejected_field(value, true),
            "agents[0].sessionSources[0].roots[0]",
            "{root} is refused"
        );
    }
}

#[test]
fn include_patterns_are_relative_globs() {
    for pattern in ["/abs/*.jsonl", "../*.jsonl", "a[", ""] {
        let mut value = sample();
        value["sessionSources"][0]["include"] = json!([pattern]);
        assert_eq!(
            rejected_field(value, true),
            "agents[0].sessionSources[0].include[0]",
            "{pattern} is refused"
        );
    }
}

#[test]
fn resolves_the_first_root_with_a_value() {
    let source = &agent(sample()).session_sources[0];
    let unset = BTreeMap::new();
    assert_eq!(
        resolve_root(&source.roots, "/home/user", &unset),
        Some(PathBuf::from("/home/user/.sample/sessions"))
    );
    let set = BTreeMap::from([("SAMPLE_HOME".to_string(), "/data/sample".to_string())]);
    assert_eq!(
        resolve_root(&source.roots, "/home/user", &set),
        Some(PathBuf::from("/data/sample/sessions"))
    );
    let relative = BTreeMap::from([("SAMPLE_HOME".to_string(), "relative".to_string())]);
    assert_eq!(
        resolve_root(&source.roots, "/home/user", &relative),
        Some(PathBuf::from("/home/user/.sample/sessions"))
    );
    assert_eq!(
        root_variables(&agent(sample())),
        BTreeSet::from(["SAMPLE_HOME".to_string()])
    );
}

#[test]
fn launch_answer_runs_only_the_agent_tool() {
    let declaration = agent(sample());
    let launch = decode_native_launch(
        &declaration,
        json!({ "executable": { "kind": "tool", "tool": "sample-cli" }, "arguments": ["-p"] }),
    )
    .expect("own tool is accepted");
    assert_eq!(launch.arguments, vec!["-p".to_string()]);

    let other = decode_native_launch(
        &declaration,
        json!({ "executable": { "kind": "tool", "tool": "other" } }),
    )
    .expect_err("another tool is refused");
    assert_eq!(other.code, "plugin.agents.undeclaredTool");

    let packaged = decode_native_launch(
        &declaration,
        json!({ "executable": { "kind": "packaged", "path": "bin/cli" } }),
    )
    .expect_err("packaged executables are refused");
    assert_eq!(packaged.code, "plugin.agents.invalidLaunch");

    let environment = decode_native_launch(
        &declaration,
        json!({
            "executable": { "kind": "tool", "tool": "sample-cli" },
            "environment": { "ANTHROPIC_BASE_URL": "http://proxy" }
        }),
    )
    .expect_err("environment is refused");
    assert_eq!(environment.code, "plugin.agents.launchEnvironment");

    let oversized = decode_native_launch(
        &declaration,
        json!({
            "executable": { "kind": "tool", "tool": "sample-cli" },
            "arguments": ["x".repeat(MAX_LAUNCH_ARGUMENT_BYTES + 1)]
        }),
    )
    .expect_err("oversized argument is refused");
    assert_eq!(oversized.code, "plugin.agents.invalidLaunch");
}

#[test]
fn catalog_entry_hides_session_functions() {
    let entry = catalog_entry("dev.lithe.test.agents", &agent(sample()));
    assert_eq!(entry["modes"], json!(["native"]));
    assert_eq!(entry["forms"], json!(["conversation", "terminal"]));
    assert_eq!(entry["readsSessions"], json!(true));
    assert_eq!(entry["sessionRootVariables"], json!(["SAMPLE_HOME"]));
    assert!(entry.get("sessionSources").is_none());
    assert!(entry.get("native").is_none());
}

#[test]
fn launch_params_carry_resume_only_when_given() {
    assert_eq!(
        launch_params("sample", "/work", None),
        json!({ "agentID": "sample", "cwd": "/work" })
    );
    assert_eq!(
        launch_params("sample", "/work", Some("abc"))["resume"],
        json!("abc")
    );
}

fn configured() -> Value {
    let mut value = sample();
    value["configuration"] = json!({
        "instructions": { "roots": ["${userHome}/.sample/INSTRUCTIONS.md"] },
        "skills": { "roots": ["${env:SAMPLE_CONFIG}/skills", "${userHome}/.sample/skills"] },
        "files": [
            { "id": "settings", "roots": ["${userHome}/.sample/settings.json"] },
            { "id": "servers", "roots": ["${userHome}/.sample.json"] }
        ],
        "read": "configuration.read",
        "edit": "configuration.edit"
    });
    value
}

#[test]
fn accepts_configuration_and_reports_it() {
    let mut agents = [agent(configured())];
    check(&mut agents, true).expect("configuration validates");
    assert!(has_configuration(&agents));
    assert!(!has_configuration(&[agent(sample())]));
    assert_eq!(
        root_variables(&agents[0]),
        BTreeSet::from(["SAMPLE_CONFIG".to_string(), "SAMPLE_HOME".to_string()])
    );

    let entry = catalog_entry("dev.lithe.test.agents", &agents[0]);
    assert_eq!(
        entry["configuration"],
        json!({ "instructions": true, "skills": true, "files": true, "editable": true })
    );
    assert!(entry["configuration"].get("read").is_none());

    let programs = programs(&agents[0]);
    let program = programs
        .iter()
        .find(|program| program["kind"] == "agentConfiguration")
        .expect("configuration is in the install review");
    assert_eq!(program["files"][0]["id"], "settings");
    assert_eq!(program["editable"], true);
    assert_eq!(program["skills"][1], "${userHome}/.sample/skills");
}

#[test]
fn configuration_files_need_a_backend() {
    let mut value = configured();
    value.as_object_mut().unwrap().remove("native");
    value.as_object_mut().unwrap().remove("sessionSources");
    assert!(agent(value.clone()).needs_backend());
    assert_eq!(rejected_field(value.clone(), false), "agents[0]");

    let configuration = value["configuration"].as_object_mut().unwrap();
    for field in ["files", "read", "edit"] {
        configuration.remove(field);
    }
    assert!(!agent(value.clone()).needs_backend());
    check(&mut [agent(value)], false).expect("instructions and skills need no backend");
}

#[test]
fn configuration_is_bounded_and_consistent() {
    let cases: [(&str, Value, &str); 9] = [
        ("empty", json!({}), "agents[0].configuration"),
        (
            "a root without a file name",
            json!({ "instructions": { "roots": ["${userHome}"] } }),
            "agents[0].configuration.instructions.roots[0]",
        ),
        (
            "a root leaving the base",
            json!({ "skills": { "roots": ["${userHome}/../skills"] } }),
            "agents[0].configuration.skills.roots[0]",
        ),
        (
            "too many candidates",
            json!({ "skills": { "roots": [
                "${userHome}/a", "${userHome}/b", "${userHome}/c", "${userHome}/d", "${userHome}/e"
            ] } }),
            "agents[0].configuration.skills.roots",
        ),
        (
            "files without read",
            json!({ "files": [{ "id": "settings", "roots": ["${userHome}/s.json"] }] }),
            "agents[0].configuration.read",
        ),
        (
            "read without files",
            json!({ "instructions": { "roots": ["${userHome}/A.md"] }, "read": "configuration.read" }),
            "agents[0].configuration.read",
        ),
        (
            "edit without read",
            json!({ "instructions": { "roots": ["${userHome}/A.md"] }, "edit": "configuration.edit" }),
            "agents[0].configuration.edit",
        ),
        (
            "a duplicate file",
            json!({
                "files": [
                    { "id": "settings", "roots": ["${userHome}/a.json"] },
                    { "id": "settings", "roots": ["${userHome}/b.json"] }
                ],
                "read": "configuration.read"
            }),
            "agents[0].configuration.files[1].id",
        ),
        (
            "an invalid function ID",
            json!({
                "files": [{ "id": "settings", "roots": ["${userHome}/a.json"] }],
                "read": "Configuration Read"
            }),
            "agents[0].configuration.read",
        ),
    ];
    for (name, configuration, field) in cases {
        let mut value = sample();
        value["configuration"] = configuration;
        assert_eq!(rejected_field(value, true), field, "{name}");
    }

    let mut many = sample();
    let files: Vec<Value> = (0..=MAX_CONFIGURATION_FILES)
        .map(|index| json!({ "id": format!("file-{index}"), "roots": [format!("${{userHome}}/{index}.json")] }))
        .collect();
    many["configuration"] = json!({ "files": files, "read": "configuration.read" });
    assert_eq!(rejected_field(many, true), "agents[0].configuration.files");

    let mut unknown = configured();
    unknown["configuration"]["environment"] = json!({});
    assert!(serde_json::from_value::<AgentDeclaration>(unknown).is_err());
}
