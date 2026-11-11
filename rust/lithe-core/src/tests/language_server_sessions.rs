//! Contract tests for plugin-owned language server sessions (API-25).
//!
//! The manifest fixture and the launch-resolution golden live in
//! `shared/plugins/contract/v1/fixtures/` so the plugin documentation
//! repository and other hosts run the same cases.

use crate::plugins::declarative::{
    decode_language_server_launch_command, language_server_launch_plan_command, validate_command,
};
use crate::plugins::marketplace::install_review_command;
use serde_json::{json, Value};

macro_rules! contract_file {
    ($path:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../shared/plugins/contract/v1/",
            $path
        ))
    };
}

const BACKEND_LAUNCH_FIXTURE: &str =
    contract_file!("fixtures/languages/language-server-backend-launch.plugin.json");
const PACKAGED_SERVER_FIXTURE: &str =
    contract_file!("fixtures/languages/language-packaged-server.plugin.json");
const LAUNCH_RESOLUTION: &str =
    contract_file!("fixtures/languages/language-server-launch-resolution.json");

fn parse(source: &str) -> Value {
    serde_json::from_str(source).expect("fixture is valid JSON")
}

/// The capabilities the backend-launch fixture requires.
const FIXTURE_CAPABILITIES: [&str; 4] =
    ["backend", "languageServers", "languages", "toolDefinitions"];

/// Capability offer of a host that runs the backend-launch fixture.
fn full_host() -> Value {
    let mut host = serde_json::Map::new();
    for capability in FIXTURE_CAPABILITIES {
        host.insert(capability.to_string(), json!([0]));
    }
    Value::Object(host)
}

#[test]
fn backend_launch_fixture_keeps_its_session_declaration() {
    let outcome = validate_command(json!({ "manifest": parse(BACKEND_LAUNCH_FIXTURE) })).unwrap();
    assert_eq!(outcome["status"], "ok", "{outcome}");

    let server = &outcome["plugin"]["languageServers"][0];
    assert_eq!(server["launch"]["kind"], "backend");
    assert_eq!(server["launch"]["function"], "metals.resolveLaunch");
    assert_eq!(
        server["requestMethods"],
        json!(["textDocument/definition", "workspace/executeCommand"])
    );
    assert_eq!(server["serializedCommands"], json!(["build-import"]));
    assert_eq!(
        server["forwardNotifications"],
        json!(["metals/executeClientCommand", "metals/status"])
    );
    assert_eq!(server["state"]["retention"]["maxEntries"], 8);
    assert_eq!(server["remote"], "unsupported");
}

#[test]
fn backend_launch_needs_a_host_that_offers_every_required_capability() {
    let manifest = parse(BACKEND_LAUNCH_FIXTURE);

    for capability in FIXTURE_CAPABILITIES {
        let mut host = full_host();
        host.as_object_mut().unwrap().remove(capability);
        let refused =
            validate_command(json!({ "manifest": manifest, "hostCapabilities": host })).unwrap();
        assert_eq!(
            refused["error"]["code"], "plugin.api.unsupported",
            "{capability}: {refused}"
        );
        assert_eq!(
            refused["error"]["field"],
            json!(format!("requires.{capability}")),
            "{capability}: {refused}"
        );
    }

    let accepted =
        validate_command(json!({ "manifest": manifest, "hostCapabilities": full_host() })).unwrap();
    assert_eq!(accepted["status"], "ok", "{accepted}");
}

#[test]
fn install_review_names_the_function_that_chooses_the_command_line() {
    let outcome =
        install_review_command(json!({ "manifest": parse(BACKEND_LAUNCH_FIXTURE) })).unwrap();
    assert_eq!(outcome["status"], "ok", "{outcome}");

    let programs = outcome["review"]["programs"].as_array().unwrap();
    let program = programs
        .iter()
        .find(|program| program["kind"] == "backendLanguageServer")
        .unwrap_or_else(|| panic!("no backend language server program in {programs:?}"));
    assert_eq!(program["function"], "metals.resolveLaunch");
    assert_eq!(program["timeoutMilliseconds"], 30_000);
}

/// Applies a `set` or `remove` edit to one member of the request object.
fn edit_request(request: &mut Value, edit: &Value) {
    let path = edit["path"].as_str().unwrap();
    let key = path
        .strip_prefix('/')
        .unwrap_or_else(|| panic!("edit path {path} is not a JSON Pointer"));
    let members = request.as_object_mut().unwrap();
    match edit["op"].as_str().unwrap() {
        "set" => {
            members.insert(key.to_string(), edit["value"].clone());
        }
        "remove" => {
            members.remove(key);
        }
        other => panic!("unknown edit op {other}"),
    }
}

/// The manifest a golden names by file; only the fixtures these tests use
/// are embedded.
fn manifest_named(file: &str) -> Value {
    match file {
        "language-server-backend-launch.plugin.json" => parse(BACKEND_LAUNCH_FIXTURE),
        "language-packaged-server.plugin.json" => parse(PACKAGED_SERVER_FIXTURE),
        other => panic!("the golden names an unknown plugin {other}"),
    }
}

/// Builds the command payload one golden case describes. A case runs
/// against the fixture's plugin and language unless it names its own.
fn case_payload(fixture: &Value, case: &Value) -> Value {
    let mut request = fixture["request"].clone();
    for edit in case["requestEdits"].as_array().into_iter().flatten() {
        edit_request(&mut request, edit);
    }
    let plugin = case.get("plugin").unwrap_or(&fixture["plugin"]);
    let mut payload = json!({
        "manifest": manifest_named(plugin.as_str().unwrap()),
        "languageID": case.get("languageID").unwrap_or(&fixture["languageID"]),
        "request": request,
    });
    if let Some(result) = case.get("result") {
        payload["result"] = result.clone();
    }
    payload
}

/// The operating system is the machine running the test, so no golden
/// can name it.
fn without_host_os(mut data: Value) -> Value {
    if let Some(platform) = data.pointer_mut("/plan/resolveParams/platform") {
        platform.as_object_mut().unwrap().remove("os");
    }
    data
}

/// What a case compares: the data object of an accepted command, or the
/// stable code of a rejected one.
fn observed(outcome: Value) -> Value {
    match outcome["status"].as_str() {
        Some("ok") => without_host_os(outcome),
        Some("rejected") => json!({
            "status": "rejected",
            "code": outcome["error"]["code"],
        }),
        _ => panic!("unexpected outcome {outcome}"),
    }
}

#[test]
fn launch_resolution_cases_match_the_contract_golden() {
    let fixture = parse(LAUNCH_RESOLUTION);
    let cases = fixture["cases"].as_array().unwrap();
    assert!(!cases.is_empty());

    for case in cases {
        let name = case["name"].as_str().unwrap();
        let payload = case_payload(&fixture, case);
        let outcome = match case["operation"].as_str().unwrap() {
            "languageServerLaunchPlan" => language_server_launch_plan_command(payload),
            "decodeLaunch" => decode_language_server_launch_command(payload),
            other => panic!("{name}: unknown operation {other}"),
        }
        .unwrap_or_else(|error| panic!("{name}: command failed: {error:?}"));

        assert_eq!(
            observed(outcome),
            without_host_os(case["expected"].clone()),
            "{name}"
        );
    }
}

#[test]
fn a_server_launched_without_a_backend_has_no_answer_to_decode() {
    let fixture = parse(LAUNCH_RESOLUTION);
    let mut payload = json!({
        "manifest": parse(PACKAGED_SERVER_FIXTURE),
        "languageID": "php",
        "request": fixture["request"],
        "result": { "executable": { "kind": "packaged", "path": "server/bin/metals" } },
    });
    let outcome = decode_language_server_launch_command(payload.clone()).unwrap();
    assert_eq!(
        outcome["error"]["code"], "plugin.launch.notBackend",
        "{outcome}"
    );

    // The first step of a server that is not resolved by a backend is the
    // ordinary launch plan, which has no `resolveParams`.
    payload.as_object_mut().unwrap().remove("result");
    let plan = language_server_launch_plan_command(payload).unwrap();
    assert_eq!(plan["status"], "ok", "{plan}");
    assert!(plan["plan"].get("resolveParams").is_none(), "{plan}");
}
