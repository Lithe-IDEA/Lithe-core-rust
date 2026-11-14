//! System and provider installations offered as candidates.

use super::*;

/// Host-observed string maps (login-shell variables, command homes, explicit choices).
pub(crate) fn string_maps(input: &Value, keys: &[&str]) -> Validation<()> {
    for key in keys {
        if let Some(value) = input.get(*key) {
            let map = value
                .as_object()
                .filter(|map| map.len() <= 256)
                .ok_or_else(|| reject(key))?;
            if map.values().any(|value| {
                !value
                    .as_str()
                    .is_some_and(|value| value.is_empty() || bounded(value, 4096))
            }) {
                return Err(reject(key));
            }
        }
    }
    Ok(())
}

/// Homes the host found by expanding this tool's `discovery` scans.
pub(crate) fn system_installations(input: &Value, tool: &str) -> Validation<Vec<Value>> {
    let mut installations = Vec::new();
    for value in array(input, "systemInstallations", 512)? {
        fields(value, &["tool", "home"])?;
        if value["tool"] == tool {
            let home = value["home"]
                .as_str()
                .ok_or_else(|| reject("systemInstallations"))?;
            installations.push(json!({"tool":tool,"home":home,"provider":SYSTEM_PROVIDER}));
        }
    }
    Ok(installations)
}

/// Homes `toolchains.decodeReply` accepted from a provider declaring `tool`, kept
/// under that provider's identity; one the registry does not list is refused.
pub(crate) fn provider_installations(
    input: &Value,
    tool: &str,
    registry: &Value,
) -> Validation<Vec<Value>> {
    let mut installations = Vec::new();
    for value in array(input, "providerInstallations", 512)? {
        fields(value, &["tool", "home", "provider"])?;
        if value["tool"] != tool {
            continue;
        }
        let home = value["home"]
            .as_str()
            .ok_or_else(|| reject("providerInstallations"))?;
        let provider = value["provider"]
            .as_str()
            .and_then(|provider| provider.rsplit_once(':'))
            .filter(|(plugin, provider)| {
                registry["providers"].as_array().is_some_and(|entries| {
                    entries.iter().any(|entry| {
                        entry["pluginID"] == *plugin
                            && entry["provider"]["id"] == *provider
                            && entry["provider"]["tools"]
                                .as_array()
                                .is_some_and(|tools| tools.iter().any(|known| known == tool))
                    })
                })
            })
            .ok_or_else(|| reject("providerInstallations"))?;
        installations.push(
            json!({"tool":tool,"home":home,"provider":format!("{}:{}", provider.0, provider.1)}),
        );
    }
    Ok(installations)
}
