//! `git.workspaceStatusMerge` keeps failed snapshots and confirms only successes.

use serde_json::json;

fn call(payload: serde_json::Value) -> serde_json::Value {
    serde_json::from_str(&crate::execute_json(
        &json!({"id":"status", "command":"git.workspaceStatusMerge", "payload":payload})
            .to_string(),
    ))
    .unwrap()
}

#[test]
fn keeps_failed_existing_snapshots_and_only_confirms_successes() {
    let response = call(
        json!({"roots":["a","b","c"], "previousRoots":["a","b","removed"], "outcomes":[
            {"root":"a","success":true}, {"root":"b","success":false}, {"root":"c","success":false}
        ]}),
    );
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(
        response["data"],
        json!({"entries":[{"root":"a","source":"current"},{"root":"b","source":"previous"},{"root":"c","source":"missing"}], "confirmedRoots":["a"], "failedRoots":["b","c"]})
    );
}

#[test]
fn rejects_incomplete_duplicate_and_foreign_outcomes() {
    for payload in [
        json!({"roots":["a","b"],"previousRoots":[],"outcomes":[{"root":"a","success":true}]}),
        json!({"roots":["a","b"],"previousRoots":[],"outcomes":[{"root":"a","success":true},{"root":"a","success":false}]}),
        json!({"roots":["a"],"previousRoots":[],"outcomes":[{"root":"b","success":true}]}),
    ] {
        assert_eq!(call(payload)["ok"], false);
    }
}
