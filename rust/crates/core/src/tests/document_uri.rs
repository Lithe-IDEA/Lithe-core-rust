use serde_json::{json, Value};

#[test]
fn document_uri_matches_shared_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/lsp/document-uri-v1.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let response: Value = serde_json::from_str(&crate::execute_json(
            &json!({"command":"lsp.documentUri", "payload":{
                "path":case["path"], "platform":case["platform"]
            }})
            .to_string(),
        ))
        .unwrap();
        if case["uri"].is_null() {
            assert_eq!(response["ok"], false, "{}: {response}", case["name"]);
        } else {
            assert_eq!(response["ok"], true, "{}: {response}", case["name"]);
            assert_eq!(response["data"]["uri"], case["uri"], "{}", case["name"]);
        }
    }
}

#[test]
fn document_uri_accepts_non_ascii_windows_extended_length_paths() {
    let path = format!("C:\\{}\\Main.txt", vec!["目".repeat(100); 120].join("\\"));
    assert!(path.len() > 32_768);
    assert!(path.encode_utf16().count() < 32_767);
    let response: Value = serde_json::from_str(&crate::execute_json(
        &json!({"command":"lsp.documentUri", "payload":{"path":path,"platform":"windows"}})
            .to_string(),
    ))
    .unwrap();
    assert_eq!(response["ok"], true);
    assert!(response["data"]["uri"]
        .as_str()
        .unwrap()
        .starts_with("file:///C:/"));
}
