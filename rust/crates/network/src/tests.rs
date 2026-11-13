//! Tests for the shared network proxy policy normalization.

use serde_json::{json, Value};

use crate::{bypasses, proxy_policy, NetworkProxyPolicyRequest};

fn policy_for(request: Value) -> Value {
    let request: NetworkProxyPolicyRequest =
        serde_json::from_value(request).expect("policy request is valid");
    serde_json::to_value(proxy_policy(request)).expect("policy response encodes")
}

/// The fixture is the cross-platform contract: macOS consumes the policy
/// today and Windows is expected to adopt the same cases in a follow-up.
#[test]
fn fixture_cases_match_the_shared_contract() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/network/proxy-policy-v1.json"
    )))
    .expect("proxy policy fixture is JSON");
    for case in fixture["cases"].as_array().expect("fixture cases") {
        let actual = policy_for(case["request"].clone());
        assert_eq!(actual, case["expected"], "case: {}", case["name"]);
    }
}

/// Stored credentials belong to the custom proxy only. After a mode switch
/// back to the system policy the plan must not carry them, so no network
/// stack can forward them to a different proxy during authentication.
#[test]
fn system_mode_never_carries_stale_custom_credentials() {
    let policy = policy_for(json!({
        "mode": "system",
        "proxyType": "http",
        "host": "proxy.example",
        "port": 3128,
        "username": "lithe",
        "password": "secret",
        "exclusions": ["127.0.0.1"]
    }));
    assert_eq!(policy["policy"], json!("system"));
    assert_eq!(policy["username"], json!(""));
    assert_eq!(policy["password"], json!(""));
    assert_eq!(policy["exclusions"], json!([]));
}

#[test]
fn passwords_keep_every_character_while_identifiers_are_trimmed() {
    let policy = policy_for(json!({
        "mode": "custom",
        "host": " proxy.example ",
        "port": 3128,
        "username": " lithe ",
        "password": " s p "
    }));
    assert_eq!(policy["host"], json!("proxy.example"));
    assert_eq!(policy["username"], json!("lithe"));
    assert_eq!(policy["password"], json!(" s p "));
}

#[test]
fn saved_password_cannot_be_rebound_by_changing_the_endpoint_or_account() {
    let mut request = json!({
        "mode": "custom", "proxyType": "http", "host": "proxy.example", "port": 3128,
        "username": "lithe", "password": "sample-value",
        "passwordIdentity": {
            "proxyType": "http", "host": " Proxy.Example ", "port": 3128, "username": " lithe "
        }
    });
    assert_eq!(policy_for(request.clone())["password"], request["password"]);
    for (field, value) in [
        ("host", json!("other.example")),
        ("port", json!(8080)),
        ("username", json!("Lithe")),
        ("proxyType", json!("socks5")),
    ] {
        let mut changed = request.clone();
        changed[field] = value;
        assert_eq!(policy_for(changed)["password"], "", "changed {field}");
    }
    request["host"] = json!("other.example");
    request.as_object_mut().unwrap().remove("passwordIdentity");
    // A new explicit password belongs to the candidate the user entered.
    assert_eq!(policy_for(request.clone())["password"], request["password"]);
}

/// Every case of the bypass fixture: the rules and invalid entries a custom policy carries,
/// and which hosts they send direct. Hosts replay the same file with their comparison.
#[test]
fn bypass_fixture_cases_match_the_shared_contract() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/network/proxy-bypass-v1.json"
    )))
    .expect("proxy bypass fixture is JSON");
    for case in fixture["cases"].as_array().expect("fixture cases") {
        let name = &case["name"];
        let request: NetworkProxyPolicyRequest = serde_json::from_value(json!({
            "mode": "custom", "host": "proxy.example", "port": 3128,
            "exclusions": case["exclusions"],
        }))
        .expect("policy request is valid");
        let policy = proxy_policy(request);
        assert_eq!(
            serde_json::to_value(&policy.exclusion_rules).expect("rules encode"),
            case["exclusionRules"],
            "rules of {name}"
        );
        assert_eq!(
            json!(policy.invalid_exclusions),
            case["invalidExclusions"],
            "invalid entries of {name}"
        );
        for (host, direct) in case["hosts"].as_object().expect("hosts") {
            assert_eq!(
                bypasses(host, &policy.exclusion_rules),
                direct.as_bool().expect("bypass flag"),
                "{name}: {host}"
            );
        }
    }
}
