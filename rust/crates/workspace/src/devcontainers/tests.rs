//! Shared examples for the form's standard configuration and rejected fields.

use super::*;

#[test]
fn basic_forms_follow_the_shared_configuration_fixture() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/remote/devcontainer-configuration-v1.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let request: PrepareRequest = serde_json::from_value(case["form"].clone()).unwrap();
        let result = prepare(request);
        if let Some(field) = case["invalidField"].as_str() {
            let error = result.unwrap_err();
            assert!(matches!(error.code, ErrorCode::InvalidRequest));
            let details: serde_json::Value =
                serde_json::from_str(error.details.as_deref().unwrap()).unwrap();
            assert_eq!(details["field"], field, "{}", case["id"]);
        } else {
            let prepared = result.unwrap();
            assert_eq!(prepared.path, case["path"], "{}", case["id"]);
            let configuration: serde_json::Value = serde_json::from_str(&prepared.content).unwrap();
            assert_eq!(configuration, case["configuration"], "{}", case["id"]);
        }
    }
    let rejected = configs(ConfigsRequest {
        folder: ".".into(),
        runtime: Some(RuntimeTarget {
            engine: ContainerEngine::Podman,
            context: Some("docker-only".into()),
            wsl: None,
        }),
    });
    assert!(matches!(
        rejected.unwrap_err().code,
        ErrorCode::InvalidRequest
    ));
}
