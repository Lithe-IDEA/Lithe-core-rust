//! Folder names enabled plugins declare through `workspace.configureVisibility`
//! are hidden by every workspace walk, and clearing them shows the folders again.
//!
//! The declarations are process-wide, so this binary holds the only test that sets
//! them; Core's unit tests run with none.

use lithe_core::execute_json;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn response(command: &str, payload: Value) -> Value {
    serde_json::from_str(&execute_json(
        &json!({
            "id": "configure-visibility",
            "command": command,
            "timeoutMilliseconds": 5000,
            "payload": payload,
        })
        .to_string(),
    ))
    .expect("Core should answer with JSON")
}

fn data(command: &str, payload: Value) -> Value {
    let response = response(command, payload);
    assert_eq!(response["ok"], true, "{command}: {response}");
    response["data"].clone()
}

fn configure(plugins: Vec<Value>) -> Value {
    let plugins: Vec<Value> = plugins
        .into_iter()
        .map(|manifest| json!({ "manifest": manifest }))
        .collect();
    data(
        "workspace.configureVisibility",
        json!({ "plugins": plugins }),
    )
}

/// Clears the declarations when the test ends, also after a failed assertion.
struct Declarations;

impl Drop for Declarations {
    fn drop(&mut self) {
        configure(Vec::new());
    }
}

struct Workspace(PathBuf);

impl Workspace {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be valid")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "lithe-configure-visibility-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("workspace should be creatable");
        Self(root.canonicalize().expect("workspace should resolve"))
    }

    fn write(&self, path: &str, content: &str) -> &Self {
        let file = self.0.join(path);
        fs::create_dir_all(file.parent().expect("file should have a parent"))
            .expect("parent should be creatable");
        fs::write(file, content).expect("file should be writable");
        self
    }

    fn git_init(&self, path: &str) {
        let directory = self.0.join(path);
        fs::create_dir_all(&directory).expect("repository should be creatable");
        let result = data(
            "git.command",
            json!({
                "root": directory,
                "arguments": ["init", "--template=", "-q", "-b", "main"],
            }),
        );
        assert_eq!(result["exitCode"], 0, "{result}");
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn manifest(name: &str) -> Value {
    serde_json::from_str(match name {
        "build" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../shared/plugins/contract/v1/fixtures/build-systems/build-static-file.plugin.json"
        )),
        "vcs" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../shared/plugins/contract/v1/fixtures/vcs/vcs-provider.plugin.json"
        )),
        _ => unreachable!("unknown fixture {name}"),
    })
    .expect("fixture manifest should be JSON")
}

/// A build system that hides its output folders, a VCS provider whose `.svn`
/// marker is hidden, and a build system with an invalid folder name.
fn declaring_plugins() -> Vec<Value> {
    let mut build = manifest("build");
    build["id"] = json!("dev.lithe.test.generated-folders");
    build["buildSystems"][0]["layout"]["hiddenDirectoryNames"] = json!(["target", "Generated"]);
    let mut vcs = manifest("vcs");
    vcs["id"] = json!("dev.lithe.test.metadata-folders");
    let mut invalid = manifest("build");
    invalid["id"] = json!("dev.lithe.test.invalid-folder-name");
    invalid["buildSystems"][0]["layout"]["hiddenDirectoryNames"] = json!(["out/classes"]);
    vec![build, vcs, invalid]
}

fn strings(values: &Value) -> Vec<String> {
    values
        .as_array()
        .expect("value should be an array")
        .iter()
        .map(|value| value.as_str().expect("entry should be text").to_string())
        .collect()
}

fn snapshot_files(root: &Path) -> Vec<String> {
    let mut files = strings(&data("workspace.snapshot", json!({ "root": root }))["files"]);
    files.sort();
    files
}

fn file_names(root: &Path, include_generated: bool) -> Vec<String> {
    let found = data(
        "workspace.searchEverywhere",
        json!({
            "root": root, "query": "Report", "maxResults": 100, "maxFileResults": 100,
            "maxContentResults": 0, "maxSymbolResults": 0,
            "includeGeneratedFileNames": include_generated,
        }),
    );
    let mut paths: Vec<String> = found["matches"]
        .as_array()
        .expect("search should list matches")
        .iter()
        .map(|item| {
            item["path"]
                .as_str()
                .expect("path should be text")
                .to_string()
        })
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

fn content_matches(root: &Path) -> Vec<String> {
    let found = data(
        "workspace.searchEverywhere",
        json!({
            "root": root, "query": "report body", "maxResults": 100, "maxFileResults": 0,
            "maxContentResults": 100, "maxSymbolResults": 0, "includeGeneratedFileNames": true,
        }),
    );
    let mut paths: Vec<String> = found["matches"]
        .as_array()
        .expect("search should list matches")
        .iter()
        .map(|item| {
            item["path"]
                .as_str()
                .expect("path should be text")
                .to_string()
        })
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

fn build_instances(root: &Path) -> Vec<String> {
    let detected = data(
        "build.detect",
        json!({
            "workspaceRoot": root,
            "plugins": [{ "manifest": declaring_plugins()[0] }],
        }),
    );
    assert_eq!(detected["status"], "ok", "{detected}");
    strings(
        &detected["detection"]["instances"]
            .as_array()
            .expect("detection should list instances")
            .iter()
            .map(|instance| instance["id"].clone())
            .collect::<Value>(),
    )
}

fn vcs_roots(root: &Path) -> Vec<String> {
    let detected = data(
        "vcs.detectRoots",
        json!({
            "workspaceRoot": root,
            "plugins": [{ "manifest": declaring_plugins()[1] }],
        }),
    );
    assert_eq!(detected["status"], "ok", "{detected}");
    let mut roots: Vec<String> = detected["detection"]["roots"]
        .as_array()
        .expect("detection should list roots")
        .iter()
        .filter(|root| root["vcs"] == "svn")
        .map(|root| root["id"].as_str().expect("id should be text").to_string())
        .collect();
    roots.sort();
    roots
}

fn repositories(root: &Path) -> Vec<String> {
    let mut paths: Vec<String> = data("workspace.repositories", json!({ "root": root }))
        ["repositories"]
        .as_array()
        .expect("discovery should list repositories")
        .iter()
        .map(|entry| {
            entry["path"]
                .as_str()
                .expect("path should be text")
                .to_string()
        })
        .collect();
    paths.sort();
    paths
}

#[test]
fn plugin_declared_folders_are_hidden_by_every_walk_until_cleared() {
    let workspace = Workspace::new();
    let root = workspace.0.clone();
    workspace
        .write("Cargo.toml", "[package]\nname = \"app\"\n")
        .write("Makefile", "all:\n\ttrue\n")
        .write("src/Report.txt", "report body\n")
        .write("target/Makefile", "all:\n\ttrue\n")
        .write("target/debug/Report.txt", "report body\n")
        .write(
            "target/debug/Cargo.toml",
            "[package]\nname = \"generated\"\n",
        )
        // Declared names match one path component, ignoring ASCII case.
        .write("module/generated/Report.txt", "report body\n")
        .write("legacy/.svn/Report.txt", "report body\n")
        .write("target/checkout/.svn/wc.db", "");
    workspace.git_init(".");
    workspace.git_init("target/repository");
    let checkout = |path: &str| {
        lithe_git::simplified_canonical_path(root.join(path).canonicalize().unwrap())
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    };

    // Without declarations, only Core's own folders are hidden.
    let everything = [
        "legacy/.svn/Report.txt",
        "module/generated/Report.txt",
        "src/Report.txt",
        "target/debug/Report.txt",
    ];
    assert_eq!(file_names(&root, false), everything);
    assert_eq!(build_instances(&root), ["cargo:.", "cargo:target/debug"]);
    assert_eq!(vcs_roots(&root), ["legacy", "target/checkout"]);
    assert_eq!(
        repositories(&root),
        [checkout("."), checkout("target/repository")]
    );

    let _declarations = Declarations;
    let configured = configure(declaring_plugins());
    assert_eq!(
        configured["hiddenDirectoryNames"],
        json!([".svn", "Generated", "target"])
    );
    let rejected = configured["rejected"].as_array().unwrap();
    assert_eq!(rejected.len(), 1, "{configured}");
    assert_eq!(rejected[0]["index"], 2);
    assert_eq!(rejected[0]["error"]["code"], "plugin.path.invalid");

    // Hosts read the folders to display them and classify paths without a copy.
    let defaults = data("workspace.visibilityDefaults", json!({}));
    let names = strings(&defaults["hiddenDirectoryNames"]);
    for name in [
        ".git",
        ".worktree",
        ".worktrees",
        ".svn",
        "Generated",
        "target",
    ] {
        assert!(names.iter().any(|entry| entry == name), "{name}: {names:?}");
    }
    let classified = data(
        "workspace.classifyVisibility",
        json!({ "entries": [
            { "path": "target", "isDirectory": true },
            { "path": "module/GENERATED/Report.txt", "isDirectory": false },
            { "path": "src/target.txt", "isDirectory": false },
        ]}),
    );
    assert_eq!(classified["hidden"], json!([true, true, false]));
    // `!` turns a plugin-declared folder off like one of Core's own.
    let turned_off = data(
        "workspace.classifyVisibility",
        json!({
            "hiddenFilePatterns": ["!target"],
            "entries": [{ "path": "target/debug", "isDirectory": true }],
        }),
    );
    assert_eq!(turned_off["hidden"], json!([false]));

    // The tree and its listing skip the folders without reading them.
    assert_eq!(
        snapshot_files(&root),
        ["Cargo.toml", "Makefile", "src/Report.txt"]
    );
    let listing = data(
        "workspace.listDirectories",
        json!({ "root": root, "paths": ["", "legacy", "module"] }),
    )
    .to_string();
    for hidden in ["\"target\"", "\"generated\"", "\".svn\""] {
        assert!(!listing.contains(hidden), "{hidden}: {listing}");
    }

    // Search Everywhere lists generated file names on request, never VCS metadata,
    // and content search reads none of them.
    assert_eq!(file_names(&root, false), ["src/Report.txt"]);
    assert_eq!(
        file_names(&root, true),
        [
            "module/generated/Report.txt",
            "src/Report.txt",
            "target/debug/Report.txt"
        ]
    );
    assert_eq!(content_matches(&root), ["src/Report.txt"]);

    // The cached name index follows additions, deletions and folder invalidation
    // inside a generated folder.
    let update = |paths: &[&str]| {
        data(
            "workspace.searchIndex.update",
            json!({ "root": root, "paths": paths }),
        )
    };
    workspace.write("target/debug/ReportGuide.txt", "");
    update(&["target/debug/ReportGuide.txt"]);
    assert!(file_names(&root, true).contains(&"target/debug/ReportGuide.txt".to_string()));
    fs::remove_file(root.join("target/debug/ReportGuide.txt")).unwrap();
    update(&["target/debug/ReportGuide.txt"]);
    assert!(!file_names(&root, true).contains(&"target/debug/ReportGuide.txt".to_string()));
    assert_eq!(update(&["target/debug"])["rebuilt"], true);
    assert!(file_names(&root, true).contains(&"target/debug/Report.txt".to_string()));

    // File events inside the folders raise no refresh.
    assert!(!lithe_core::is_workspace_event_relevant(
        "target/debug/Report.txt"
    ));
    assert!(!lithe_core::is_workspace_event_relevant(
        "legacy/.svn/wc.db"
    ));
    assert!(lithe_core::is_workspace_event_relevant("src/Report.txt"));

    // Run discovery, build detection, VCS roots and Git repositories skip them too.
    assert_eq!(build_instances(&root), ["cargo:."]);
    assert_eq!(vcs_roots(&root), ["legacy"]);
    assert_eq!(repositories(&root), [checkout(".")]);

    // An empty list clears the declarations, and every walk sees the folders again.
    let cleared = configure(Vec::new());
    assert_eq!(
        cleared,
        json!({ "hiddenDirectoryNames": [], "rejected": [] })
    );
    assert_eq!(file_names(&root, false), everything);
    assert_eq!(vcs_roots(&root), ["legacy", "target/checkout"]);

    // A malformed payload is refused and leaves the current declarations alone.
    configure(declaring_plugins());
    let refused = response("workspace.configureVisibility", json!({ "plugin": [] }));
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["error"]["code"], "invalid_request", "{refused}");
    assert_eq!(file_names(&root, false), ["src/Report.txt"]);
}
