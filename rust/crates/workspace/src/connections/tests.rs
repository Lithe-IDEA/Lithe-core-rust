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
fn saved_target_fingerprint_changes_only_with_the_route() {
    let first = edit(edit_request(json!({
        "document": null,
        "save": { "name": "Build box", "connection": { "host": "build.example", "user": "dev" } },
    })))
    .unwrap();
    let id = &first["id"];
    let fingerprint = first["connections"][0]["targetFingerprint"]
        .as_str()
        .unwrap();
    assert_eq!(fingerprint.len(), 64);
    assert!(!first["document"].as_str().unwrap().contains(fingerprint));
    let revise = |connection: Value| {
        let response = edit(edit_request(json!({
            "document": first["document"],
            "save": { "id": id, "name": "Renamed", "connection": connection },
        })))
        .unwrap();
        assert_eq!(response["connections"][0]["id"], *id);
        response["connections"][0]["targetFingerprint"].clone()
    };
    assert_eq!(
        revise(json!({"host": "build.example", "user": "dev",
        "authentication": "agent", "connectTimeoutSeconds": 20})),
        fingerprint
    );
    for connection in [
        json!({"host": "new.example", "user": "dev"}),
        json!({"host": "build.example", "user": "ops"}),
        json!({"host": "build.example", "user": "dev", "port": 2222}),
        json!({"host": "build.example", "user": "dev", "proxyJump": "jump.example"}),
    ] {
        assert_ne!(revise(connection), fingerprint);
    }
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
fn a_query_lists_only_the_matching_sections() {
    let first = edit(edit_request(json!({
        "document": null,
        "save": { "name": "Web", "connection": { "host": "web.example" }, "group": "Production" },
    })))
    .unwrap();
    let second = edit(edit_request(json!({
        "document": first["document"],
        "save": { "name": "Lab", "connection": { "host": "lab.example", "user": "dev" } },
    })))
    .unwrap();
    let lab = second["id"].clone();

    let listed = list(list_request(json!({
        "document": second["document"],
        "query": "DEV lab",
    })))
    .unwrap();

    assert_eq!(listed["sections"], json!([{ "group": null, "ids": [lab] }]));
    assert_eq!(listed["connections"].as_array().unwrap().len(), 2);
}

#[test]
fn an_address_is_split_into_fields_or_names_the_field_to_fix() {
    let address =
        parse_address(serde_json::from_value(json!({ "text": "dev@build-box:2222" })).unwrap())
            .unwrap();
    assert_eq!(
        address,
        json!({ "host": "build-box", "user": "dev", "port": 2222 })
    );

    let error = parse_address(serde_json::from_value(json!({ "text": "box:99999" })).unwrap())
        .expect_err("port out of range");
    assert_eq!(
        details(&error),
        json!({ "field": "port", "reason": "invalid" })
    );
}

#[test]
fn an_import_adds_entries_under_free_names_and_fills_labels() {
    let first = edit(edit_request(json!({
        "document": null,
        "save": { "name": "Web", "connection": { "host": "web.example" } },
    })))
    .unwrap();
    let web = first["id"].as_str().unwrap().to_owned();

    let imported = edit(edit_request(json!({
        "document": first["document"],
        "import": {
            "entries": [{
                "name": "web",
                "connection": { "host": "web2.example" },
                "color": "blue",
                "group": "Imported",
            }],
            "fills": [{ "id": web, "color": "green", "group": "Production" }],
        },
    })))
    .unwrap();

    assert_eq!(imported["changed"], json!(true));
    let ids = imported["ids"].as_array().unwrap();
    assert_eq!(ids.len(), 1);
    let connections = imported["connections"].as_array().unwrap();
    assert_eq!(connections[0]["color"], json!("green"));
    assert_eq!(connections[0]["group"], json!("Production"));
    assert_eq!(connections[1]["id"], ids[0]);
    assert_eq!(connections[1]["name"], json!("web (2)"));
    assert_eq!(connections[1]["group"], json!("Imported"));
}

#[test]
fn marking_a_connection_records_when_it_was_last_connected() {
    let first = edit(edit_request(json!({
        "document": null,
        "save": { "name": "Web", "connection": { "host": "web.example" } },
    })))
    .unwrap();
    let web = first["id"].clone();

    let marked = edit(edit_request(json!({
        "document": first["document"],
        "markConnected": { "id": web },
    })))
    .unwrap();

    let at = marked["connections"][0]["lastConnectedAt"]
        .as_u64()
        .expect("timestamp");
    assert!(at > 1_700_000_000, "{at}");
    let error = edit(edit_request(json!({
        "document": first["document"],
        "markConnected": { "id": "absent" },
    })))
    .expect_err("unknown entry");
    assert_eq!(
        details(&error),
        json!({ "field": "id", "reason": "unknown" })
    );
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
