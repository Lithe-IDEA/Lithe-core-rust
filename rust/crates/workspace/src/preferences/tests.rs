use super::*;

fn request<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).expect("the request decodes")
}

fn details(error: &CoreError) -> Value {
    serde_json::from_str(error.details.as_deref().expect("details")).expect("JSON details")
}

#[test]
fn reading_no_document_reports_automatic_values_and_the_storage() {
    let home = std::env::temp_dir();
    let result = read(request(json!({
        "document": null,
        "homeDirectory": home.to_string_lossy(),
    })))
    .unwrap();
    assert_eq!(
        result["preferences"],
        json!({
            "sshProgram": null,
            "reconnectLimitSeconds": null,
            "allowReverseForwarding": null,
            "closePrograms": null,
        })
    );
    assert_eq!(
        result["closing"],
        json!({ "source": "automatic", "programs": "ask", "idleTimeoutSeconds": 600 })
    );
    assert_eq!(result["ssh"]["source"], "automatic");
    assert_eq!(result["reconnect"]["source"], "automatic");
    assert_eq!(result["reconnect"]["limitSeconds"], 60);
    assert_eq!(
        result["reverseForwarding"],
        json!({ "source": "automatic", "allowed": false })
    );
    assert_eq!(
        result["storage"],
        json!({ "relativePath": "remote/preferences.json", "maximumBytes": 16384 })
    );
    let config = home.join(".ssh").join("config");
    assert_eq!(
        result["sshConfig"]["path"],
        config.to_string_lossy().as_ref()
    );
    assert_eq!(result["sshConfig"]["exists"], config.is_file());

    let without_home = read(request(json!({ "document": null }))).unwrap();
    assert!(without_home.get("sshConfig").is_none());
    let relative = read(request(
        json!({ "document": null, "homeDirectory": "home" }),
    ));
    assert!(matches!(
        relative.unwrap_err().code,
        ErrorCode::InvalidRequest
    ));
}

#[test]
fn a_damaged_document_is_a_parse_failure_with_the_field() {
    for document in ["not json", "{\"version\":1,\"sshProgram\":\"ssh\"}"] {
        let error = read(request(json!({ "document": document }))).unwrap_err();
        assert!(matches!(error.code, ErrorCode::ParseFailed));
        assert!(details(&error)["field"].is_string());
        let error = ssh_program(None, Some(document)).unwrap_err();
        assert!(matches!(error.code, ErrorCode::ParseFailed));
    }
}

#[test]
fn an_edit_changes_one_value_and_publishes_the_document() {
    let limit = edit(request(json!({
        "document": null,
        "reconnectLimit": { "seconds": 120 },
    })))
    .unwrap();
    assert_eq!(limit["changed"], true);
    let document = limit["document"].as_str().unwrap().to_owned();
    let state = read(request(json!({ "document": document }))).unwrap();
    assert_eq!(state["preferences"]["reconnectLimitSeconds"], 120);
    assert_eq!(state["reconnect"]["source"], "setting");

    let same = edit(request(json!({
        "document": document,
        "reconnectLimit": { "seconds": 120 },
    })))
    .unwrap();
    assert_eq!(same, json!({ "changed": false, "document": null }));

    let automatic = edit(request(json!({
        "document": document,
        "reconnectLimit": { "seconds": null },
    })))
    .unwrap();
    assert_eq!(automatic["document"], "{\n  \"version\": 1\n}\n");
}

#[test]
fn reverse_forwards_stay_refused_until_the_switch_allows_them() {
    assert!(!reverse_forwarding_allowed(None).unwrap());
    let allowed = edit(request(json!({
        "document": null,
        "reverseForwarding": { "allowed": true },
    })))
    .unwrap();
    let document = allowed["document"].as_str().unwrap().to_owned();
    assert!(reverse_forwarding_allowed(Some(&document)).unwrap());
    let state = read(request(json!({ "document": document }))).unwrap();
    assert_eq!(
        state["reverseForwarding"],
        json!({ "source": "setting", "allowed": true })
    );

    let refused = edit(request(json!({
        "document": document,
        "reverseForwarding": { "allowed": false },
    })))
    .unwrap();
    let document = refused["document"].as_str().unwrap().to_owned();
    assert!(!reverse_forwarding_allowed(Some(&document)).unwrap());

    let error = reverse_forwarding_allowed(Some("not json")).unwrap_err();
    assert!(matches!(error.code, ErrorCode::ParseFailed));
}

#[test]
fn the_closing_choice_is_edited_and_read_back() {
    let edited = edit(request(json!({
        "document": null,
        "closePrograms": { "programs": "keep" },
    })))
    .unwrap();
    assert_eq!(edited["changed"], true);
    let document = edited["document"].as_str().unwrap().to_owned();
    let state = read(request(json!({ "document": document }))).unwrap();
    assert_eq!(state["preferences"]["closePrograms"], "keep");
    assert_eq!(
        state["closing"],
        json!({ "source": "setting", "programs": "keep", "idleTimeoutSeconds": 600 })
    );
    let restored = edit(request(json!({
        "document": document,
        "closePrograms": { "programs": null },
    })))
    .unwrap();
    let state = read(request(json!({ "document": restored["document"] }))).unwrap();
    assert_eq!(state["closing"]["source"], "automatic");
}

#[test]
fn an_edit_names_exactly_one_value_and_refuses_bad_programs_by_field() {
    for both_or_neither in [
        json!({ "document": null }),
        json!({
            "document": null,
            "sshProgram": { "path": null },
            "reconnectLimit": { "seconds": null },
        }),
        json!({
            "document": null,
            "reconnectLimit": { "seconds": null },
            "reverseForwarding": { "allowed": true },
        }),
        json!({
            "document": null,
            "reverseForwarding": { "allowed": true },
            "closePrograms": { "programs": "end" },
        }),
    ] {
        let error = edit(request(both_or_neither)).unwrap_err();
        assert!(matches!(error.code, ErrorCode::InvalidRequest));
        assert_eq!(details(&error)["field"], "operation");
    }
    let missing = std::env::temp_dir().join("lithe-remote-preferences-no-such-ssh");
    let error = edit(request(json!({
        "document": null,
        "sshProgram": { "path": missing.to_string_lossy() },
    })))
    .unwrap_err();
    assert!(matches!(error.code, ErrorCode::InvalidRequest));
    assert_eq!(
        details(&error),
        json!({ "field": "sshProgram", "reason": "missing" })
    );
    let error = edit(request(json!({
        "document": null,
        "reconnectLimit": { "seconds": 3601 },
    })))
    .unwrap_err();
    assert_eq!(
        details(&error),
        json!({ "field": "reconnectLimitSeconds", "reason": "outOfRange" })
    );
}

#[test]
fn the_setting_is_the_program_every_ssh_start_uses() {
    let directory = std::env::temp_dir();
    let program = directory.join("lithe-remote-preferences-chosen-ssh");
    let document = json!({ "version": 1, "sshProgram": program }).to_string();
    assert_eq!(
        ssh_program(None, Some(&document)).unwrap(),
        program.to_string_lossy()
    );
    assert_eq!(
        ssh_program(Some("/stand-in/ssh"), Some("not json")).unwrap(),
        "/stand-in/ssh"
    );
}
