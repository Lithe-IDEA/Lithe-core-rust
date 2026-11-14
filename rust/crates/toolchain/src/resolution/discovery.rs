//! The discovery plan the host runs before resolution.

use super::*;

/// Exposes both installation routes per tool: definition-owned scans the host expands,
/// and enabled providers the host queries. Native expansion and checks stay in hosts.
pub(crate) fn discovery_plan(
    input: &Value,
    validate_manifest: ManifestValidator,
) -> Validation<Value> {
    fields(input, &["registry", "platform"])?;
    let registry = definitions::registry(&input["registry"], validate_manifest)?;
    let platform = input["platform"]
        .as_str()
        .ok_or_else(|| reject("platform"))?;
    if !matches!(platform, "macos" | "windows" | "linux") {
        return Err(reject("platform"));
    }
    let tools = registry["definitions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| {
            let definition = &entry["definition"];
            let scans: Vec<_> = definition["discovery"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|scan| applicable(scan, platform))
                .cloned()
                .collect();
            let providers: Vec<_> = registry["providers"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|provider| {
                    provider["provider"]["tools"]
                        .as_array()
                        .is_some_and(|tools| tools.contains(&definition["id"]))
                })
                .map(|provider| {
                    format!(
                        "{}:{}",
                        provider["pluginID"].as_str().unwrap_or(""),
                        provider["provider"]["id"].as_str().unwrap_or("")
                    )
                })
                .collect();
            // Login-shell home variables the host reads, with the layout each one selects.
            let environment: Vec<_> = definition["executables"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|value| value["kind"] == "environment" && applicable(value, platform))
                .map(|value| json!({"variable":value["variable"],"path":value.get("path")}))
                .collect();
            json!({"tool":definition["id"],"pluginID":entry["pluginID"],"scans":scans,"providers":providers,
            "homeLayouts":definition.get("homeLayouts").cloned().unwrap_or(json!([])),
            "executableLayouts":definition["executableLayouts"],"version":definition["version"],
            "pathPrepend":definition.get("pathPrepend").cloned().unwrap_or(json!([])),
            "environment":environment})
        })
        .collect::<Vec<_>>();
    Ok(json!({"tools":tools,"limits":{"roots":256,"entries":4096,"ancestors":64}}))
}
