//! The agent library: saving with revisions, MCP servers and hooks through a
//! staged copy the synthetic plugin edits, instructions, skills and grants.

use super::*;

const CONFIGURATION_GRANTS: &[&str] = &[
    "agents.native",
    "agents.run",
    "agents.sessions.read",
    "agents.configuration",
    "backend.launch",
];

/// How the synthetic plugin's `configuration.edit` behaves.
#[derive(Clone)]
enum Edit {
    /// Applies the change to the staged copy.
    Apply,
    /// Writes something else to the staged copy.
    Wrong,
    /// Applies the change, and something else changes the real file
    /// meanwhile.
    Race(PathBuf),
}

/// The synthetic plugin with configuration declared for its agent.
fn configuration_manifest() -> Value {
    let mut manifest = manifest();
    manifest["permissions"] = json!(CONFIGURATION_GRANTS);
    manifest["agents"][0]["configuration"] = json!({
        "instructions": { "roots": ["${userHome}/.sample/INSTRUCTIONS.md"] },
        "skills": { "roots": ["${userHome}/.sample/skills"] },
        "files": [{ "id": "main", "roots": ["${userHome}/.sample/config.json"] }],
        "read": "configuration.read",
        "edit": "configuration.edit"
    });
    manifest
}

/// The synthetic plugin's configuration file: the snapshot's servers keyed
/// by name and its hooks, as JSON.
fn read_file(path: &Path) -> Value {
    match fs::read(path) {
        Ok(bytes) if !bytes.is_empty() => {
            serde_json::from_slice(&bytes).expect("configuration is JSON")
        }
        _ => json!({}),
    }
}

fn configuration_read(params: &Value) -> Result<Value, AgentCallFailure> {
    let file = params["files"]
        .as_array()
        .and_then(|files| files.iter().find(|file| file["id"] == "main"))
        .ok_or_else(|| failure("plugin.params.invalid"))?;
    let config = read_file(Path::new(file["path"].as_str().unwrap()));
    let servers: Vec<Value> = config["servers"]
        .as_object()
        .map(|servers| {
            servers
                .iter()
                .map(|(name, server)| {
                    let mut row = server.clone();
                    row["name"] = json!(name);
                    if row.get("origin").is_none() {
                        row["origin"] = json!("user");
                    }
                    row
                })
                .collect()
        })
        .unwrap_or_default();
    let hooks: Vec<Value> = config["hooks"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|mut hook| {
            if hook.get("origin").is_none() {
                hook["origin"] = json!("user");
            }
            hook
        })
        .collect();
    Ok(json!({
        "format": "litheAgentConfiguration",
        "version": 0,
        "mcpServers": servers,
        "hooks": hooks,
        "hookEvents": ["PreToolUse", "Stop"],
        "skills": [{ "name": "provided-skill", "provider": "sample-plugin" }],
        "targets": { "mcpServers": "main", "hooks": "main" }
    }))
}

fn same_hook(left: &Value, right: &Value) -> bool {
    left["event"] == right["event"]
        && left["command"] == right["command"]
        && left.get("matcher") == right.get("matcher")
}

fn configuration_edit(params: &Value, edit: &Edit) -> Result<Value, AgentCallFailure> {
    let staging = PathBuf::from(params["file"]["staging"].as_str().unwrap());
    let mut config = read_file(&staging);
    let before = config.clone();
    let change = &params["change"];
    match (change["category"].as_str(), change["op"].as_str()) {
        (Some("mcpServers"), Some("put")) => {
            let mut server = change["server"].clone();
            let name = server["name"].as_str().unwrap().to_string();
            server.as_object_mut().unwrap().remove("name");
            config["servers"][name.as_str()] = server;
        }
        (Some("mcpServers"), Some("remove")) => {
            if let Some(servers) = config["servers"].as_object_mut() {
                servers.remove(change["name"].as_str().unwrap());
            }
        }
        (Some("hooks"), Some("put")) => {
            let hooks = config["hooks"].as_array().cloned().unwrap_or_default();
            if !hooks.iter().any(|hook| same_hook(hook, &change["hook"])) {
                let mut hooks = hooks;
                hooks.push(change["hook"].clone());
                config["hooks"] = json!(hooks);
            }
        }
        (Some("hooks"), Some("remove")) => {
            let mut hooks = config["hooks"].as_array().cloned().unwrap_or_default();
            hooks.retain(|hook| !same_hook(hook, &change["hook"]));
            config["hooks"] = json!(hooks);
        }
        _ => return Err(failure("plugin.params.invalid")),
    }
    match edit {
        Edit::Apply => {}
        Edit::Wrong => config = json!({ "servers": before["servers"], "noise": true }),
        Edit::Race(real) => fs::write(real, br#"{"servers":{}}"#).unwrap(),
    }
    let changed = config != before;
    if changed {
        fs::write(&staging, serde_json::to_vec(&config).unwrap()).unwrap();
    }
    Ok(json!({ "format": "litheAgentConfigurationEdit", "version": 0, "changed": changed }))
}

/// A harness with the configuration plugin granted `grants` and the library
/// configured, and the synthetic plugin's configuration functions.
struct LibraryTest {
    harness: Harness,
    edit: Arc<Mutex<Edit>>,
    home: PathBuf,
    library: PathBuf,
}

impl LibraryTest {
    fn new(name: &str, grants: &[&str]) -> Self {
        let harness = Harness::new(name, ALL_GRANTS);
        harness.run(
            "agents.configure",
            json!({ "plugins": [{ "manifest": configuration_manifest(), "grantedPermissions": grants }] }),
        );
        let home = harness.dir.path().join("home");
        let library = harness.dir.path().join("library");
        harness.run(
            "agentLibrary.configure",
            json!({ "directory": path_text(&library), "userHome": path_text(&home) }),
        );
        let edit = Arc::new(Mutex::new(Edit::Apply));
        let current = edit.clone();
        harness
            .link
            .set_answer(move |function, params| match function {
                "configuration.read" => configuration_read(params),
                "configuration.edit" => {
                    let edit = current.lock().unwrap().clone();
                    configuration_edit(params, &edit)
                }
                other => sample_backend(other, params),
            });
        fs::create_dir_all(home.join(".sample")).unwrap();
        Self {
            harness,
            edit,
            home,
            library,
        }
    }

    fn read(&self) -> Value {
        self.harness.run("agentLibrary.read", json!({}))
    }

    fn save(&self, library: Value) -> String {
        let revision = self.read()["revision"].clone();
        let saved = self.harness.run(
            "agentLibrary.save",
            json!({ "revision": revision, "library": library }),
        );
        saved["revision"].as_str().unwrap().to_string()
    }

    fn apply(&self, change: Value) -> bool {
        self.harness.run(
            "agentLibrary.apply",
            json!({ "agent": AGENT_KEY, "change": change }),
        )["changed"]
            .as_bool()
            .unwrap()
    }

    fn refused_change(&self, change: Value) -> String {
        self.harness.refused(
            "agentLibrary.apply",
            json!({ "agent": AGENT_KEY, "change": change }),
        )
    }

    fn config_file(&self) -> PathBuf {
        self.home.join(".sample").join("config.json")
    }

    fn staged_files(&self) -> usize {
        fs::read_dir(self.library.join("staging")).map_or(0, |entries| entries.count())
    }
}

fn library_with(servers: Value, hooks: Value) -> Value {
    json!({ "mcpServers": servers, "hooks": hooks })
}

#[test]
fn library_saves_only_unchanged_revisions_and_only_for_the_user() {
    let test = LibraryTest::new("library save", CONFIGURATION_GRANTS);
    let read = test.read();
    assert_eq!(read["revision"], "empty");
    let agent = &read["agents"][0];
    assert_eq!(agent["agent"], AGENT_KEY);
    assert_eq!(agent["displayName"], "Sample Agent");
    assert_eq!(agent["mayConfigure"], true);
    assert_eq!(agent["editable"], true);
    assert_eq!(agent["instructions"]["state"], "absent");
    assert_eq!(
        agent["targets"],
        json!({ "mcpServers": true, "hooks": true })
    );
    assert_eq!(agent["hookEvents"], json!(["PreToolUse", "Stop"]));
    assert_eq!(agent["providedSkills"][0]["provider"], "sample-plugin");

    let server = json!([{ "name": "docs", "transport": "stdio", "command": "docs-mcp", "args": ["--stdio"] }]);
    let saved = test.harness.run(
        "agentLibrary.save",
        json!({ "revision": "empty", "library": library_with(server.clone(), json!([])) }),
    );
    let revision = saved["revision"].as_str().unwrap();
    assert_ne!(revision, "empty");
    let read = test.read();
    assert_eq!(read["revision"], revision);
    assert_eq!(read["library"]["mcpServers"], server);

    let stale = test.harness.refused(
        "agentLibrary.save",
        json!({ "revision": "empty", "library": {} }),
    );
    assert_eq!(stale, "agentLibrary.revisionChanged");
    let invalid = test.harness.refused(
        "agentLibrary.save",
        json!({ "revision": revision, "library": library_with(json!([{ "name": "docs", "transport": "stdio" }]), json!([])) }),
    );
    assert_eq!(invalid, "agentLibrary.invalidLibrary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(test.library.join("library.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "the library file is the owner's only");
    }

    let previous = trace::replace(trace::Context {
        op: None,
        source: Source::Background,
    });
    let save = test.harness.refused(
        "agentLibrary.save",
        json!({ "revision": revision, "library": {} }),
    );
    let apply =
        test.refused_change(json!({ "category": "mcpServers", "op": "put", "name": "docs" }));
    trace::replace(previous);
    assert_eq!(save, "agentLibrary.userActionRequired");
    assert_eq!(apply, "agentLibrary.userActionRequired");
    assert!(test.harness.link.calls_of("configuration.edit").is_empty());
}

#[test]
fn servers_and_hooks_change_only_through_a_checked_staged_copy() {
    let test = LibraryTest::new("library servers", CONFIGURATION_GRANTS);
    let file = test.config_file();
    fs::write(
        &file,
        br#"{"servers":{"plugin-tools":{"transport":"http","url":"https://tools.example.test/mcp","origin":"agent","provider":"sample-plugin"}},"keep":1}"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
    }
    test.save(library_with(
        json!([{ "name": "docs", "transport": "stdio", "command": "docs-mcp", "args": ["--stdio"] }]),
        json!([
            { "id": "check-on-stop", "event": "Stop", "command": "sample-check" },
            { "id": "on-end", "event": "SessionEnd", "command": "sample-end" }
        ]),
    ));
    let read = test.read();
    assert_eq!(read["states"]["mcpServers"]["docs"][AGENT_KEY], "off");
    assert_eq!(read["states"]["hooks"]["check-on-stop"][AGENT_KEY], "off");
    assert_eq!(read["states"]["hooks"]["on-end"][AGENT_KEY], "unsupported");
    assert_eq!(read["agents"][0]["mcpServers"][0]["origin"], "agent");

    assert!(test.apply(json!({ "category": "mcpServers", "op": "put", "name": "docs" })));
    let written = read_file(&file);
    assert_eq!(written["servers"]["docs"]["command"], "docs-mcp");
    assert_eq!(written["servers"]["plugin-tools"]["origin"], "agent");
    assert_eq!(
        written["keep"], 1,
        "the plugin keeps what it does not change"
    );
    let edits = test.harness.link.calls_of("configuration.edit");
    let staging = PathBuf::from(edits[0]["file"]["staging"].as_str().unwrap());
    assert!(
        staging.starts_with(test.library.join("staging")),
        "the plugin edits only a staged copy"
    );
    assert_eq!(edits[0]["file"]["id"], "main");
    assert_eq!(test.staged_files(), 0);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o640,
            "the replaced file keeps its permissions"
        );
    }
    let read = test.read();
    assert_eq!(read["states"]["mcpServers"]["docs"][AGENT_KEY], "on");
    assert!(read["agents"][0]["mcpServers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|server| server["name"] == "docs" && server["inLibrary"] == true));
    assert!(!test.apply(json!({ "category": "mcpServers", "op": "put", "name": "docs" })));

    assert!(test.apply(json!({ "category": "hooks", "op": "put", "id": "check-on-stop" })));
    assert_eq!(
        test.read()["states"]["hooks"]["check-on-stop"][AGENT_KEY],
        "on"
    );
    assert!(test.apply(json!({ "category": "hooks", "op": "remove", "hook": { "event": "Stop", "command": "sample-check" } })));
    assert_eq!(
        test.read()["states"]["hooks"]["check-on-stop"][AGENT_KEY],
        "off"
    );
    assert_eq!(
        test.refused_change(json!({ "category": "hooks", "op": "put", "id": "on-end" })),
        "agentLibrary.unsupportedEvent"
    );
    assert_eq!(
        test.refused_change(json!({ "category": "mcpServers", "op": "put", "name": "absent" })),
        "agentLibrary.unknownItem"
    );
    assert_eq!(
        test.refused_change(
            json!({ "category": "mcpServers", "op": "remove", "name": "plugin-tools" })
        ),
        "agentLibrary.notInAgent",
        "what the CLI installed itself is only shown"
    );
    assert_eq!(
        test.refused_change(json!({ "category": "mcpServers", "op": "link", "name": "docs" })),
        "agentLibrary.invalidChange"
    );

    let before = fs::read(&file).unwrap();
    *test.edit.lock().unwrap() = Edit::Wrong;
    assert_eq!(
        test.refused_change(json!({ "category": "mcpServers", "op": "remove", "name": "docs" })),
        "agentLibrary.editNotApplied"
    );
    assert_eq!(
        fs::read(&file).unwrap(),
        before,
        "a wrong edit never reaches the file"
    );
    *test.edit.lock().unwrap() = Edit::Race(file.clone());
    assert_eq!(
        test.refused_change(json!({ "category": "mcpServers", "op": "remove", "name": "docs" })),
        "agentLibrary.fileChanged"
    );
    assert_eq!(
        fs::read(&file).unwrap(),
        br#"{"servers":{}}"#,
        "the other change is kept"
    );
    assert_eq!(test.staged_files(), 0);
}

#[cfg(unix)]
#[test]
fn a_linked_configuration_file_is_replaced_where_it_points() {
    let test = LibraryTest::new("library linked file", CONFIGURATION_GRANTS);
    let real = test.home.join("dotfiles-config.json");
    fs::write(&real, b"{}").unwrap();
    std::os::unix::fs::symlink(&real, test.config_file()).unwrap();
    test.save(library_with(
        json!([{ "name": "web", "transport": "http", "url": "https://web.example.test/mcp" }]),
        json!([]),
    ));
    assert!(test.apply(json!({ "category": "mcpServers", "op": "put", "name": "web" })));
    assert!(fs::symlink_metadata(test.config_file())
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        read_file(&real)["servers"]["web"]["url"],
        "https://web.example.test/mcp"
    );
}

#[test]
fn instructions_follow_the_library_and_keep_text_lithe_did_not_write() {
    let test = LibraryTest::new("library instructions", CONFIGURATION_GRANTS);
    let file = test.home.join(".sample").join("INSTRUCTIONS.md");
    fs::write(&file, b"My own notes\n").unwrap();
    let library = |text: &str, enabled: bool| {
        json!({
            "instructions": {
                "sets": [{ "id": "default", "name": "Default", "text": text }],
                "active": "default",
                "agents": { AGENT_KEY: { "enabled": enabled, "extra": "Only for the sample agent." } }
            }
        })
    };
    test.save(library("Shared rule.\n", true));
    assert_eq!(test.read()["agents"][0]["instructions"]["state"], "other");
    assert_eq!(
        test.refused_change(json!({ "category": "instructions", "op": "write" })),
        "agentLibrary.otherInstructions"
    );
    assert_eq!(fs::read(&file).unwrap(), b"My own notes\n");

    assert!(test.apply(json!({ "category": "instructions", "op": "write", "replace": true })));
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "Shared rule.\n\nOnly for the sample agent.\n"
    );
    assert_eq!(test.read()["agents"][0]["instructions"]["state"], "current");

    test.save(library("Shared rule, revised.", true));
    assert_eq!(test.read()["agents"][0]["instructions"]["state"], "written");
    assert!(
        test.apply(json!({ "category": "instructions", "op": "write" })),
        "Lithe rewrites what it wrote"
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "Shared rule, revised.\n\nOnly for the sample agent.\n"
    );
    assert!(!test.apply(json!({ "category": "instructions", "op": "write" })));

    test.save(library("Shared rule, revised.", false));
    assert_eq!(
        test.refused_change(json!({ "category": "instructions", "op": "write" })),
        "agentLibrary.instructionsOff"
    );
}

#[cfg(unix)]
#[test]
fn skills_are_imported_without_links_and_linked_into_agents() {
    let test = LibraryTest::new("library skills", CONFIGURATION_GRANTS);
    let source = test.harness.dir.path().join("source").join("demo");
    fs::create_dir_all(source.join("references")).unwrap();
    fs::write(
        source.join("SKILL.md"),
        "---\nname: demo-skill\ndescription: >\n  Checks the\n  sample project.\n---\nBody\n",
    )
    .unwrap();
    fs::write(source.join("references").join("notes.md"), "Notes\n").unwrap();
    std::os::unix::fs::symlink(test.harness.dir.path(), source.join("outside")).unwrap();

    let imported = test.harness.run(
        "agentLibrary.importSkill",
        json!({ "directory": path_text(&source) }),
    );
    assert_eq!(
        imported,
        json!({ "name": "demo-skill", "files": 2, "skipped": 1 })
    );
    assert!(!test
        .library
        .join("skills")
        .join("demo-skill")
        .join("outside")
        .exists());
    let read = test.read();
    assert_eq!(read["skills"][0]["name"], "demo-skill");
    assert_eq!(
        read["skills"][0]["description"],
        "Checks the sample project."
    );
    assert_eq!(read["skills"][0]["agents"][AGENT_KEY], "off");

    let skills = test.home.join(".sample").join("skills");
    assert!(test.apply(json!({ "category": "skills", "op": "link", "name": "demo-skill" })));
    assert!(fs::symlink_metadata(skills.join("demo-skill"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(!test.apply(json!({ "category": "skills", "op": "link", "name": "demo-skill" })));
    let read = test.read();
    assert_eq!(read["skills"][0]["agents"][AGENT_KEY], "on");
    assert_eq!(read["agents"][0]["skills"]["items"][0]["kind"], "library");

    fs::create_dir_all(skills.join("own-skill")).unwrap();
    fs::write(
        skills.join("own-skill").join("SKILL.md"),
        "---\nname: own-skill\n---\n",
    )
    .unwrap();
    let own = test.harness.run(
        "agentLibrary.importSkill",
        json!({ "agent": AGENT_KEY, "name": "own-skill" }),
    );
    assert_eq!(own["files"], 1);
    assert_eq!(
        test.harness.refused(
            "agentLibrary.importSkill",
            json!({ "agent": AGENT_KEY, "name": "own-skill" }),
        ),
        "agentLibrary.skillExists"
    );
    let read = test.read();
    let own_row = read["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|skill| skill["name"] == "own-skill")
        .unwrap()
        .clone();
    assert_eq!(own_row["agents"][AGENT_KEY], "conflict");
    assert_eq!(
        test.refused_change(json!({ "category": "skills", "op": "unlink", "name": "own-skill" })),
        "agentLibrary.notLibraryLink"
    );
    assert_eq!(
        test.refused_change(json!({ "category": "skills", "op": "link", "name": "own-skill" })),
        "agentLibrary.agentSkillExists"
    );
    assert!(
        skills.join("own-skill").join("SKILL.md").is_file(),
        "the agent's own skill stays"
    );

    let removed = test
        .harness
        .run("agentLibrary.removeSkill", json!({ "name": "demo-skill" }));
    assert_eq!(removed["unlinked"], 1);
    assert!(fs::symlink_metadata(skills.join("demo-skill")).is_err());
    assert!(!test.library.join("skills").join("demo-skill").exists());
    assert_eq!(test.staged_files(), 0);
}

#[test]
fn agents_without_the_grant_are_listed_but_never_read_or_changed() {
    let unconfigured = Harness::new("library unconfigured", ALL_GRANTS);
    assert_eq!(
        unconfigured.refused("agentLibrary.read", json!({})),
        "agentLibrary.notConfigured"
    );
    drop(unconfigured);

    let test = LibraryTest::new("library not granted", ALL_GRANTS);
    let read = test.read();
    let agent = &read["agents"][0];
    assert_eq!(agent["mayConfigure"], false);
    assert!(agent.get("instructions").is_none());
    assert!(agent.get("mcpServers").is_none());
    assert!(test.harness.link.calls_of("configuration.read").is_empty());
    assert_eq!(
        test.refused_change(json!({ "category": "instructions", "op": "write" })),
        "agentLibrary.notGranted"
    );
    assert_eq!(
        test.harness.refused(
            "agentLibrary.apply",
            json!({ "agent": "dev.lithe.test.agents/absent", "change": { "category": "instructions", "op": "write" } }),
        ),
        "agentLibrary.unknownAgent"
    );

    let granted = LibraryTest::new("library backend stopped", CONFIGURATION_GRANTS);
    granted.harness.link.set_backend(None);
    let read = granted.read();
    assert_eq!(
        read["agents"][0]["readError"],
        "agentLibrary.backendUnavailable"
    );
    assert_eq!(
        read["agents"][0]["instructions"]["state"], "absent",
        "Core still reads its own files"
    );
}

#[test]
fn skill_front_matter_and_instruction_text_are_read_as_written() {
    use crate::library::{compose_for_test, front_matter_for_test};
    assert_eq!(
        front_matter_for_test("---\nname: \"quoted name\"\ndescription: 'It''s short'\n---\n"),
        (Some("quoted name".into()), Some("It's short".into()))
    );
    assert_eq!(
        front_matter_for_test("---\ndescription: |\n  Line one\n  Line two\nname: block\n---\n"),
        (Some("block".into()), Some("Line one\nLine two".into()))
    );
    assert_eq!(
        front_matter_for_test("name: no front matter\n"),
        (None, None)
    );
    let library = json!({
        "instructions": { "sets": [], "agents": { "a/b": { "enabled": true, "extra": "  Extra only.  " } } }
    });
    assert_eq!(compose_for_test(library.clone(), "a/b"), "Extra only.\n");
    assert_eq!(compose_for_test(library, "a/c"), "");
}

#[test]
fn malformed_requests_and_a_damaged_library_file_are_refused_each_with_its_own_code() {
    let test = LibraryTest::new("library refusals", CONFIGURATION_GRANTS);
    assert_eq!(
        test.harness.refused(
            "agentLibrary.configure",
            json!({ "directory": "relative/library", "userHome": path_text(&test.home) }),
        ),
        "agentLibrary.invalidDirectory"
    );
    assert_eq!(
        test.harness.refused("agentLibrary.importSkill", json!({})),
        "agentLibrary.invalidImport"
    );
    assert_eq!(
        test.harness.refused(
            "agentLibrary.importSkill",
            json!({ "directory": "relative/skill" }),
        ),
        "agentLibrary.invalidSkillDirectory"
    );
    assert_eq!(
        test.harness
            .refused("agentLibrary.removeSkill", json!({ "name": "absent" })),
        "agentLibrary.unknownItem"
    );
    fs::create_dir_all(&test.library).unwrap();
    fs::write(
        test.library.join("library.json"),
        b"{\"version\":1,\"library\":{}}",
    )
    .unwrap();
    assert_eq!(
        test.harness.refused("agentLibrary.read", json!({})),
        "agentLibrary.invalidLibraryFile"
    );
}
