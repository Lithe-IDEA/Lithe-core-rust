//! Probe captures and version checks of a candidate.

use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Probe {
    pub(crate) executable: String,
    pub(crate) output: String,
    pub(crate) exit_code: i32,
}

pub(crate) fn fields(input: &Value, allowed: &[&str]) -> Validation<()> {
    if !input
        .as_object()
        .is_some_and(|object| object.keys().all(|key| allowed.contains(&key.as_str())))
    {
        return Err(reject("input"));
    }
    Ok(())
}

/// Numeric releases are tool-neutral; display aliases never enter comparison.
pub(crate) fn components(value: &str) -> Option<Vec<u32>> {
    if value.is_empty() || value.len() > 64 {
        return None;
    }
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() > 8 {
        return None;
    }
    let mut numbers = parts
        .iter()
        .map(|part| {
            (!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| part.parse().ok())
                .flatten()
        })
        .collect::<Option<Vec<u32>>>()?;
    while numbers.len() > 1 && numbers.last() == Some(&0) {
        numbers.pop();
    }
    Some(numbers)
}

pub(crate) fn reject(field: &str) -> PluginRejection {
    PluginRejection::at(
        "plugin.toolchain.invalidParams",
        field,
        "Invalid toolchain snapshot or native evidence.",
    )
}

pub(crate) fn bounded(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

pub(crate) fn array<'a>(input: &'a Value, key: &str, max: usize) -> Validation<&'a [Value]> {
    match input.get(key) {
        None => Ok(&[]),
        Some(value) => value
            .as_array()
            .filter(|items| items.len() <= max)
            .map(Vec::as_slice)
            .ok_or_else(|| reject(key)),
    }
}

pub(crate) fn enabled<'a>(registry: &'a Value, tool: &str) -> Option<&'a Value> {
    registry["definitions"]
        .as_array()?
        .iter()
        .find(|entry| entry["definition"]["id"] == tool)
        .map(|entry| &entry["definition"])
}

pub(crate) fn capture(
    definition: &Value,
    output: &str,
    minimum: Option<&str>,
) -> Validation<Value> {
    if output.len() > 128 * 1024 {
        return Err(reject("output"));
    }
    let minimum = minimum
        .map(|value| components(value).ok_or_else(|| reject("minimumVersion")))
        .transpose()?;
    let expression = RegexBuilder::new(definition["version"]["pattern"].as_str().unwrap_or(""))
        .size_limit(1024 * 1024)
        .build()
        .map_err(|_| reject("version.pattern"))?;
    let captured = expression
        .captures(output)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str());
    let version = captured.filter(|value| components(value).is_some());
    let satisfied = version.is_some_and(|version| {
        minimum
            .as_ref()
            .is_none_or(|minimum| components(version).as_ref() >= Some(minimum))
    });
    Ok(json!({"satisfied":satisfied,"version":version}))
}

/// Uses only the installed definition's regular expression, never a tool-name branch.
pub(crate) fn check_version(
    input: &Value,
    validate_manifest: ManifestValidator,
) -> Validation<Value> {
    fields(input, &["registry", "tool", "output", "minimumVersion"])?;
    let registry = definitions::registry(&input["registry"], validate_manifest)?;
    let tool = input["tool"].as_str().ok_or_else(|| reject("tool"))?;
    let definition = enabled(&registry, tool).ok_or_else(|| reject("tool"))?;
    let output = input["output"].as_str().ok_or_else(|| reject("output"))?;
    let minimum = input
        .get("minimumVersion")
        .map(|value| value.as_str().ok_or_else(|| reject("minimumVersion")))
        .transpose()?;
    capture(definition, output, minimum)
}
