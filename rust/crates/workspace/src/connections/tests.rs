use super::*;

fn list_request(value: Value) -> ListRequest {
    serde_json::from_value(value).expect("valid list request")
}

fn edit_request(value: Value) -> EditRequest {
    serde_json::from_value(value).expect("valid edit request")
}

fn details(error: &CoreError) -> Value {
    serde_json::from_str(error.details.as_deref().expect("details")).expect("JSON details")
}

#[test]
fn a_missing_document_lists_nothing_and_says_where_to_keep_one() {
    let result = list(list_request(json!({ "document": null }))).unwrap();
    assert_eq!(result["connections"], json!([]));
    assert_eq!(result["sections"], json!([]));
    assert_eq!(
        result["storage"],
        json!({ "relativePath": "remote/connections.json", "maximumBytes": 262144 })
    );
}

#[test]
fn saving_returns_the_document_to_write_and_the_new_id() {
    let result = edit(edit_request(json!({
        "document": null,
        "save": { "name": "Build box", "connection": { "host": "build.example", "user": "dev" } },
    })))
    .unwrap();
    assert_eq!(result["changed"], json!(true));
    let id = result["id"].as_str().expect("id");
    assert_eq!(id.len(), 32);
    assert!(id.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(result["connections"][0]["id"], json!(id));

    let document = result["document"].as_str().expect("document");
    let listed = list(list_request(json!({ "document": document }))).unwrap();
    assert_eq!(listed["connections"], result["connections"]);
}

#[test]
fn colors_and_groups_are_saved_and_listed_by_section() {
    let first = edit(edit_request(json!({
        "document": null,
        "save": {
            "name": "Web",
            "connection": { "host": "web.example" },
            "color": "green",
            "group": " Production ",
        },
    })))
    .unwrap();
    let web = first["id"].as_str().expect("id").to_owned();
    assert_eq!(first["connections"][0]["color"], json!("green"));
    assert_eq!(first["connections"][0]["group"], json!("Production"));
    let second = edit(edit_request(json!({
        "document": first["document"],
        "save": { "name": "Lab", "connection": { "host": "lab.example" } },
    })))
    .unwrap();
    let lab = second["id"].as_str().expect("id").to_owned();
    assert!(second["connections"][1].get("color").is_none());
    assert!(second["connections"][1].get("group").is_none());
    let sections = json!([
        { "group": "Production", "ids": [web] },
        { "group": null, "ids": [lab] },
    ]);
    assert_eq!(second["sections"], sections);
    let listed = list(list_request(json!({ "document": second["document"] }))).unwrap();
    assert_eq!(listed["sections"], sections);

    let error = edit(edit_request(json!({
        "document": null,
        "save": { "name": "Box", "connection": { "host": "box" }, "group": "a\u{7}b" },
    })))
    .expect_err("control character");
    assert_eq!(
        details(&error),
        json!({ "field": "group", "reason": "invalid" })
    );
    assert!(serde_json::from_value::<EditRequest>(json!({
        "document": null,
        "save": { "name": "Box", "connection": { "host": "box" }, "color": "pink" },
    }))
    .is_err());
}

#[test]
fn an_edit_that_changes_nothing_returns_no_document() {
    let result = edit(edit_request(json!({
        "document": null,
        "remove": { "id": "absent" },
    })))
    .unwrap();
    assert_eq!(result["changed"], json!(false));
    assert_eq!(result["document"], Value::Null);
    assert!(result.get("id").is_none());
}

#[test]
fn each_edit_carries_exactly_one_operation() {
    for request in [
        json!({ "document": null }),
        json!({
            "document": null,
            "remove": { "id": "a" },
            "rememberFolder": { "id": "a", "path": "/srv" },
        }),
    ] {
        let error = edit(edit_request(request)).expect_err("refused");
        assert!(matches!(error.code, ErrorCode::InvalidRequest));
        assert_eq!(
            details(&error),
            json!({ "field": "operation", "reason": "invalid" })
        );
    }
}

#[test]
fn a_damaged_source_is_a_parse_failure_and_a_bad_edit_is_a_bad_request() {
    let error = list(list_request(
        json!({ "document": "{\"version\":2,\"connections\":[]}" }),
    ))
    .expect_err("unknown version");
    assert!(matches!(error.code, ErrorCode::ParseFailed));
    assert_eq!(
        details(&error),
        json!({ "field": "version", "reason": "invalid" })
    );

    let error = edit(edit_request(json!({
        "document": null,
        "save": { "name": "Box", "connection": { "host": "dev@box" } },
    })))
    .expect_err("host with a user");
    assert!(matches!(error.code, ErrorCode::InvalidRequest));
    assert_eq!(
        details(&error),
        json!({ "field": "host", "reason": "invalid" })
    );
}

#[test]
fn requests_cannot_carry_a_secret_or_an_unknown_field() {
    for request in [
        json!({ "document": null, "save": { "name": "Box", "connection": { "host": "box", "password": "x" } } }),
        json!({ "document": null, "save": { "name": "Box", "password": "x", "connection": { "host": "box" } } }),
        json!({ "document": null, "extra": true }),
    ] {
        assert!(
            serde_json::from_value::<EditRequest>(request.clone()).is_err(),
            "{request} was accepted"
        );
    }
}
