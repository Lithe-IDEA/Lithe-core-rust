//! Regressions for the user-visible search contract using real workspace files.

use crate::{files, project_search_page, search_page};
use serde_json::{json, Value};
use std::path::PathBuf;

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        let root = crate::test_support::temporary_root("idea-search");
        std::fs::create_dir_all(&root).unwrap();
        Self(root.canonicalize().unwrap())
    }
    fn write(&self, path: &str, bytes: impl AsRef<[u8]>) {
        let file = self.0.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, bytes).unwrap();
    }
    fn page(&self, mut request: Value) -> Value {
        request["root"] = json!(self.0);
        request["projectPage"] = json!(true);
        project_search_page(serde_json::from_value(request).unwrap()).unwrap()
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        crate::search_index::invalidate_root(&self.0);
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn project_pages_continue_within_one_file_and_bind_unsaved_snapshots() {
    let workspace = Workspace::new();
    workspace.write("src/a.txt", "absent on disk\n");
    workspace.write("other/b.txt", "needle excluded by scope\n");
    let mut request = json!({"query":"needle", "pageSize":2, "scopePaths":["src"],
        "textOverrides":{"src/a.txt":"needle 1\nneedle 2\nneedle 3\nneedle 4\nneedle 5\n"}});
    let mut lines = Vec::new();
    for _ in 0..4 {
        let page = workspace.page(request.clone());
        for item in page["matches"].as_array().unwrap() {
            assert_eq!(item["path"], "src/a.txt");
            lines.push(item["line"].as_u64().unwrap());
        }
        if page["nextCursor"].is_null() {
            break;
        }
        request["cursor"] = page["nextCursor"].clone();
    }
    assert_eq!(lines, [1, 2, 3, 4, 5]);
    request["textOverrides"]["src/a.txt"] = json!("changed needle");
    request["root"] = json!(workspace.0);
    request["projectPage"] = json!(true);
    assert!(project_search_page(serde_json::from_value(request).unwrap()).is_err());
    assert_eq!(
        std::fs::read_to_string(workspace.0.join("src/a.txt")).unwrap(),
        "absent on disk\n"
    );
}

#[test]
fn changing_file_budget_rebuilds_postings_and_replacement_reads_the_same_text() {
    let workspace = Workspace::new();
    workspace.write("large.txt", "needle in an ordinary file\r\n");
    assert!(
        workspace.page(json!({"query":"needle","maxFileBytes":4}))["matches"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let page = workspace.page(json!({"query":"needle","maxFileBytes":128}));
    assert_eq!(page["matches"].as_array().unwrap().len(), 1);
    let preview = files::replace_preview(
        serde_json::from_value(
            json!({"root":workspace.0,"query":"needle","replacement":"found","maxFileBytes":128}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        preview.files[0].replacement_text,
        "found in an ordinary file\r\n"
    );
}

#[test]
fn complete_counts_include_occurrences_outside_the_first_page_and_unsaved_text() {
    let workspace = Workspace::new();
    let fixture: Value =
        serde_json::from_str(include_str!("../../../fixtures/search/idea-search-v1.json")).unwrap();
    let case = &fixture["completeCounts"];
    for (path, text) in case["files"].as_object().unwrap() {
        workspace.write(path, text.as_str().unwrap());
    }
    assert_eq!(workspace.page(case["request"].clone()), case["expected"]);
}

#[cfg(unix)]
#[test]
fn search_follows_internal_links_without_cycles_or_external_targets() {
    let workspace = Workspace::new();
    let outside = Workspace::new();
    workspace.write("source/a.txt", "needle\n");
    workspace.write("source/private/secret.txt", "needle\n");
    crate::indexing::configure(
        serde_json::from_value(
            json!({"root":workspace.0,"manualExcludedDirectories":["source/private"],"includedDirectories":[],"gitIgnoreEnabled":false}),
        )
        .unwrap(),
        |_| Ok(()),
    )
    .unwrap();
    outside.write("private.txt", "needle\n");
    std::os::unix::fs::symlink(workspace.0.join("source"), workspace.0.join("alias")).unwrap();
    std::os::unix::fs::symlink(&workspace.0, workspace.0.join("source/loop")).unwrap();
    std::os::unix::fs::symlink(&outside.0, workspace.0.join("outside")).unwrap();
    let page = workspace.page(json!({"query":"needle", "followSymlinks":true}));
    let paths: Vec<_> = page["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["alias/a.txt", "source/a.txt"]);
    let listing = files::list_directories(
        serde_json::from_value(
            json!({"root":workspace.0,"paths":["", "alias", "source/loop", "outside"]}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(listing.directories.len(), 2);
    assert_eq!(listing.directories[1].path, "alias");
    assert!(listing.directories[1].linked);
    assert_eq!(listing.directories[1].children[0].path, "alias/a.txt");
    let ordinary = workspace.page(json!({"query":"needle"}));
    assert_eq!(ordinary["matches"].as_array().unwrap().len(), 1);
    let unsaved = json!({"query":"unsaved", "followSymlinks":true,
        "textOverrides":{"source/a.txt":"unsaved unsaved\n"}});
    assert_eq!(
        workspace.page(unsaved.clone())["matches"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let mut counts = unsaved.clone();
    counts["countOnly"] = json!(true);
    assert_eq!(workspace.page(counts)["occurrences"], 4);
    let mut replacement = unsaved;
    replacement["root"] = json!(workspace.0);
    replacement["replacement"] = json!("edited");
    let preview = files::replace_preview(serde_json::from_value(replacement).unwrap()).unwrap();
    assert_eq!(preview.files.len(), 1);
    assert_eq!(preview.files[0].replacement_text, "edited edited\n");
}

#[test]
fn idea_name_queries_and_line_suffix_find_real_paths() {
    let workspace = Workspace::new();
    workspace.write(
        "desktop/internal/workbench/search_view.go",
        "package fixture\n",
    );
    workspace.write("src/UserServiceImpl.java", "class UserServiceImpl {}\n");
    workspace.write("resources/application-dev.yml", "key: value\n");
    for (query, path) in [
        ("search view", "desktop/internal/workbench/search_view.go"),
        ("wb/sv", "desktop/internal/workbench/search_view.go"),
        ("user service", "src/UserServiceImpl.java"),
        ("usrsvc", "src/UserServiceImpl.java"),
        ("app-dev", "resources/application-dev.yml"),
        ("application dev", "resources/application-dev.yml"),
        ("*dev.yml", "resources/application-dev.yml"),
        (
            "search_view.go:12",
            "desktop/internal/workbench/search_view.go",
        ),
    ] {
        let page = search_page(
            serde_json::from_value(
                json!({"root":workspace.0,"query":query,"pageSize":2,"resultKinds":["file"]}),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(
            page["matches"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["path"] == path),
            "{query}: {page}"
        );
        if query.ends_with(":12") {
            assert_eq!(page["matches"][0]["line"], 12);
        }
    }
}

#[test]
fn search_preferences_and_presentation_follow_the_shared_fixture() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../fixtures/search/idea-search-v1.json")).unwrap();
    for case in fixture["preferences"].as_array().unwrap() {
        let actual = crate::resolve_search_preferences(
            serde_json::from_value(case["request"].clone()).unwrap(),
        )
        .unwrap();
        assert_eq!(serde_json::to_value(actual).unwrap(), case["expected"]);
    }
    for request in fixture["invalidPreferences"].as_array().unwrap() {
        assert!(crate::resolve_search_preferences(
            serde_json::from_value(request.clone()).unwrap()
        )
        .is_err());
    }
    let case = &fixture["presentation"];
    let actual =
        crate::search_presentation(serde_json::from_value(case["request"].clone()).unwrap())
            .unwrap();
    assert_eq!(serde_json::to_value(actual).unwrap(), case["expected"]);
}

#[test]
fn provider_discovery_uses_real_names_without_opening_source_files() {
    let workspace = Workspace::new();
    let fixture: Value =
        serde_json::from_str(include_str!("../../../fixtures/search/idea-search-v1.json")).unwrap();
    let case = &fixture["languageDiscovery"];
    for (path, text) in case["files"].as_object().unwrap() {
        workspace.write(path, text.as_str().unwrap());
    }
    assert_eq!(workspace.page(case["request"].clone()), case["expected"]);
}
