//! Instantiating a toolchain's launch items.

use super::*;

/// Longest argument list or string the launch expansion accepts, in items and bytes.
const MAX_LAUNCH_ITEMS: usize = 1024;

const MAX_LAUNCH_ITEM_BYTES: usize = 65536;

/// Reads an optional bounded string argument that never carries a NUL.
fn launch_text<'a>(input: &'a Value, key: &str) -> Validation<Option<&'a str>> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .filter(|text| !text.is_empty() && text.len() <= 4096 && !text.contains('\0'))
            .map(Some)
            .ok_or_else(|| reject(key)),
    }
}

fn launch_list(input: &Value, key: &str) -> Validation<Vec<String>> {
    let items = array(input, key, MAX_LAUNCH_ITEMS)?;
    items
        .iter()
        .map(|item| {
            item.as_str()
                .filter(|text| text.len() <= MAX_LAUNCH_ITEM_BYTES && !text.contains('\0'))
                .map(str::to_owned)
                .ok_or_else(|| reject(key))
        })
        .collect()
}

fn launch_mismatch(message: &str) -> PluginRejection {
    PluginRejection::new("plugin.toolchain.launchMismatch", message)
}

/// Expands an installed definition's `launch` template for a selected executable.
///
/// The definition alone says how its tool starts an entry or takes arguments, so a plugin and a
/// host never branch on a tool name. A part the caller supplies that the template cannot carry,
/// or a variable the caller leaves out, is `plugin.toolchain.launchMismatch`.
pub(crate) fn instantiate_launch(
    input: &Value,
    validate_manifest: ManifestValidator,
) -> Validation<Value> {
    fields(
        input,
        &[
            "registry",
            "tool",
            "executable",
            "home",
            "entry",
            "runtimeArguments",
            "arguments",
        ],
    )?;
    let registry = definitions::registry(&input["registry"], validate_manifest)?;
    let tool = input["tool"].as_str().ok_or_else(|| reject("tool"))?;
    let definition = enabled(&registry, tool).ok_or_else(|| reject("tool"))?;
    let executable = launch_text(input, "executable")?.ok_or_else(|| reject("executable"))?;
    let home = launch_text(input, "home")?;
    let entry = launch_text(input, "entry")?;
    let runtime_arguments = launch_list(input, "runtimeArguments")?;
    let arguments = launch_list(input, "arguments")?;
    let mut consumed = BTreeSet::new();
    let mut expanded = Vec::new();
    for token in definition["launch"].as_array().into_iter().flatten() {
        let token = token.as_str().ok_or_else(|| reject("launch"))?;
        match token {
            "${executable}" => expanded.push(executable.to_owned()),
            "${home}" => expanded.push(
                home.ok_or_else(|| launch_mismatch("The tool's launch needs its home."))?
                    .to_owned(),
            ),
            "${entry}" => {
                consumed.insert("entry");
                expanded.push(
                    entry
                        .ok_or_else(|| launch_mismatch("The tool's launch needs an entry."))?
                        .to_owned(),
                );
            }
            "${runtimeArguments}" => {
                consumed.insert("runtimeArguments");
                expanded.extend(runtime_arguments.iter().cloned());
            }
            "${arguments}" => {
                consumed.insert("arguments");
                expanded.extend(arguments.iter().cloned());
            }
            literal => expanded.push(literal.to_owned()),
        }
    }
    if entry.is_some() && !consumed.contains("entry") {
        return Err(launch_mismatch("The tool does not launch an entry."));
    }
    if !runtime_arguments.is_empty() && !consumed.contains("runtimeArguments") {
        return Err(launch_mismatch("The tool takes no runtime arguments."));
    }
    if !arguments.is_empty() && !consumed.contains("arguments") {
        return Err(launch_mismatch("The tool takes no program arguments."));
    }
    let rest = expanded.split_off(1);
    Ok(json!({
        "executable": expanded.into_iter().next().unwrap_or_default(),
        "arguments": rest,
        "versionArguments": definition["version"]["arguments"],
    }))
}
