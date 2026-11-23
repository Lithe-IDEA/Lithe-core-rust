//! Checks the file-association contract without native default-handler mutations.

use lithe_core_rules::{execute, RuleErrorCode};
use serde_json::{json, Value};

const FIXTURE: &str = include_str!("../../../fixtures/application/file-associations-v1.json");

#[test]
fn shared_catalog_and_registration_plans_match() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(fixture["version"], 1);
    for (section, command) in [
        ("catalog", "application.fileAssociations.catalog"),
        ("plans", "application.fileAssociations.plan"),
    ] {
        for case in fixture[section].as_array().unwrap() {
            let actual = execute(command, case["payload"].clone()).unwrap();
            assert_eq!(actual, case["expected"], "{}", case["name"]);
        }
    }
    for case in fixture["errors"].as_array().unwrap() {
        let error =
            execute(case["command"].as_str().unwrap(), case["payload"].clone()).unwrap_err();
        assert_eq!(
            error.code,
            RuleErrorCode::InvalidRequest,
            "{}",
            case["name"]
        );
    }
}

#[test]
fn association_catalog_input_is_bounded() {
    let languages: Vec<Value> = (0..129)
        .map(|_| json!({"id": "text", "highlighted": true, "extensions": []}))
        .collect();
    let error = execute(
        "application.fileAssociations.catalog",
        json!({"languages": languages}),
    )
    .unwrap_err();
    assert_eq!(error.code, RuleErrorCode::InvalidRequest);
}
