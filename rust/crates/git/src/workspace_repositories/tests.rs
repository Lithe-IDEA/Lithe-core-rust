//! Workspace repository projections against their shared fixtures.

use super::{
    branch_plan_command, change_groups_command, common_references_command, history_merge_command,
    selection_command, MAX_REPOSITORIES,
};
use lithe_protocol::CoreError;
use serde_json::{json, Value};

fn check_fixture(fixture: &str, command: fn(Value) -> Result<Value, CoreError>) {
    let fixture: Value = serde_json::from_str(fixture).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let response = command(case["request"].clone())
            .unwrap_or_else(|error| panic!("{}: {error:?}", case["name"]));
        assert_eq!(response, case["response"], "{}", case["name"]);
    }
    for case in fixture["invalid"].as_array().unwrap() {
        let error =
            command(case["request"].clone()).expect_err(case["name"].as_str().unwrap_or_default());
        assert_eq!(
            serde_json::to_value(error.code).unwrap(),
            "invalid_request",
            "{}",
            case["name"]
        );
    }
}

#[test]
fn selection_matches_the_shared_fixture() {
    check_fixture(
        include_str!("../../../../fixtures/git/workspace-repository-selection-v1.json"),
        selection_command,
    );
}

#[test]
fn common_references_match_the_shared_fixture() {
    check_fixture(
        include_str!("../../../../fixtures/git/workspace-common-references-v1.json"),
        common_references_command,
    );
}

#[test]
fn branch_plan_matches_the_shared_fixture() {
    check_fixture(
        include_str!("../../../../fixtures/git/workspace-branch-plan-v1.json"),
        branch_plan_command,
    );
}

#[test]
fn change_groups_match_the_shared_fixture() {
    check_fixture(
        include_str!("../../../../fixtures/git/workspace-change-groups-v1.json"),
        change_groups_command,
    );
}

#[test]
fn history_merge_matches_the_shared_fixture() {
    check_fixture(
        include_str!("../../../../fixtures/git/workspace-history-merge-v1.json"),
        history_merge_command,
    );
}

/// The loaded commits of one repository: `count` commits a minute apart, newest
/// first, each the child of the next. The last loaded one has a parent unless
/// the walk is complete, as a page cut mid-history does.
fn history(id: &str, count: usize, newest: i64, complete: bool) -> Value {
    let commits = (0..count)
        .map(|index| {
            let parents = if index + 1 < count || !complete {
                vec![format!("{:x}{:06x}", id.len(), index + 1)]
            } else {
                Vec::new()
            };
            json!({
                "hash": format!("{:x}{index:06x}", id.len()),
                "parentHashes": parents,
                "timestamp": newest - 60 * index as i64,
            })
        })
        .collect::<Vec<_>>();
    json!({"id": id, "complete": complete, "commits": commits})
}

#[test]
fn history_merge_rows_only_grow_at_the_end_as_pages_arrive() {
    let first = history_merge_command(json!({"repositories": [
        history("a", 100, 10_000, false),
        history("bb", 100, 9_000, true),
    ]}))
    .unwrap();
    let second = history_merge_command(json!({"repositories": [
        history("a", 200, 10_000, false),
        history("bb", 100, 9_000, true),
    ]}))
    .unwrap();
    let placed = first["entries"].as_array().unwrap();
    assert_eq!(first["pending"], json!([0]));
    assert_eq!(
        &second["entries"].as_array().unwrap()[..placed.len()],
        placed
    );
    assert!(second["entries"].as_array().unwrap().len() > placed.len());
}

#[test]
fn history_merges_past_the_commit_bound_are_rejected() {
    let half = crate::history::MAX_LOG_COMMITS / 2;
    let request = json!({"repositories": [
        history("a", half + 1, 100_000_000, true),
        history("bb", half, 100_000_000, true),
    ]});
    assert!(history_merge_command(request).is_err());
}

#[test]
fn repository_lists_past_the_bound_are_rejected() {
    let repositories = (0..=MAX_REPOSITORIES)
        .map(|index| {
            let id = format!("r{index}");
            json!({"path": format!("/w/{id}"), "id": id, "name": id, "role": "nested"})
        })
        .collect::<Vec<_>>();
    assert!(selection_command(json!({ "repositories": repositories })).is_err());
}

#[test]
fn ids_past_the_byte_bound_are_rejected() {
    let id = "a".repeat(super::MAX_ID_BYTES + 1);
    let request =
        json!({"repositories": [{"path": "/w/a", "id": id, "name": "a", "role": "nested"}]});
    assert!(selection_command(request).is_err());
}

#[test]
fn a_repository_at_a_filesystem_root_still_parents_nested_repositories() {
    let response = change_groups_command(json!({"repositories": [
        {"path": "/", "id": "..", "name": "/", "role": "workspace", "changes": [
            {"path": "w/sub", "staged": false, "worktree": true, "canToggleStaging": true,
             "submodule": {"commitChanged": false}}
        ]},
        {"path": "/w/sub", "id": "sub", "name": "sub", "role": "nested", "changes": [
            {"path": "a", "staged": false, "worktree": true, "canToggleStaging": true}
        ]}
    ]}))
    .unwrap();
    assert_eq!(response["groups"][1]["parentReference"], "unchanged");
}

#[test]
fn reference_lists_past_the_repository_bound_are_rejected() {
    let repositories = (0..=MAX_REPOSITORIES)
        .map(|index| json!({"id": format!("r{index}"), "references": []}))
        .collect::<Vec<_>>();
    assert!(common_references_command(json!({ "repositories": repositories })).is_err());
    let plan = json!({"operation": "checkout", "reference": "refs/heads/main",
        "repositories": json!(repositories)});
    assert!(branch_plan_command(plan).is_err());
}

#[test]
fn reference_lists_past_the_reference_bound_are_rejected() {
    use super::common_references::{validate_bounded, RepositoryReferences};
    let repository = |id: &str| -> RepositoryReferences {
        serde_json::from_value(json!({"id": id, "references": [
            {"fullName": "refs/heads/main", "shortName": "main", "kind": "local"},
            {"fullName": "refs/remotes/origin/main", "shortName": "origin/main", "kind": "remote"}
        ]}))
        .unwrap()
    };
    // The bound counts every repository's references together, not each one's.
    let repositories = [repository("a"), repository("b")];
    assert!(validate_bounded(&repositories, 4).is_ok());
    assert!(validate_bounded(&repositories, 3).is_err());
    assert_eq!(super::common_references::MAX_REFERENCES, 1_000_000);
}
