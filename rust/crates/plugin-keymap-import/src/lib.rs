//! Shared validation for normalized shortcut import provider replies.

use lithe_plugin_model::PluginManifest;
use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Maximum number of import providers one plugin may declare or negotiate.
pub const MAX_PROVIDERS: usize = 8;
const MAX_BYTES: usize = 1024 * 1024;
const MAX_COMMANDS: usize = 4096;
const MAX_KEYS_PER_COMMAND: usize = 8;
const MAX_INPUT_SELECTORS: usize = 16;
const MAX_RECOGNITION_PROBES: usize = 16;
const MAX_INPUT_FILES: usize = 128;
/// Largest declared input file a provider may read.
pub const MAX_INPUT_FILE_BYTES: usize = 1_048_576;
const MAX_INPUT_TOTAL_BYTES: usize = 8_388_608;
/// Largest chunk of an input file one host read returns.
pub const MAX_INPUT_CHUNK_BYTES: usize = 131_072;
const MAX_DIRECTORY_ENTRIES: usize = 4_096;
const MAX_DIRECTORY_MATCHES: usize = 64;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// One plugin-owned importer; the identifier is local to its package.
pub struct ProviderDeclaration {
    id: String,
    display_name: String,
    recognition: Vec<RecognitionProbe>,
    inputs: Vec<InputSelector>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum RecognitionProbe {
    File {
        path: String,
    },
    Directory {
        path: String,
    },
    Root {
        base: RecognitionBase,
        components: Vec<String>,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
enum RecognitionBase {
    UserConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum InputSelector {
    File { path: String },
    Directory { path: String, extension: String },
}

fn safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && path.chars().count() <= 256
        && !path
            .chars()
            .any(|character| character.is_control() || "\\:*?".contains(character))
        && path.split('/').all(|part| {
            !part.is_empty() && part != "." && part != ".." && !part.ends_with(['.', ' '])
        })
}

fn valid_selector(selector: &InputSelector) -> bool {
    match selector {
        InputSelector::File { path } => safe_relative_path(path),
        InputSelector::Directory { path, extension } => {
            safe_relative_path(path)
                && !extension.is_empty()
                && extension.len() <= 16
                && extension
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        }
    }
}

/// Plugin-local provider IDs are opaque and cannot be paths.
pub fn valid_provider_id(id: &str) -> bool {
    id.len() <= 64
        && id.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Validates and orders provider declarations without knowing their source formats.
pub fn validate_declarations(
    providers: &mut [ProviderDeclaration],
) -> lithe_plugin_model::Validation<()> {
    let mut ids = BTreeSet::new();
    for provider in providers.iter() {
        if !valid_provider_id(&provider.id)
            || !ids.insert(&provider.id)
            || provider.display_name.trim().is_empty()
            || provider.display_name.chars().count() > 64
            || provider.display_name.chars().any(char::is_control)
            || provider.recognition.is_empty()
            || provider.recognition.len() > MAX_RECOGNITION_PROBES
            || provider.inputs.is_empty()
            || provider.inputs.len() > MAX_INPUT_SELECTORS
        {
            return Err(lithe_plugin_model::invalid(
                "keymapImportProviders",
                "Invalid or duplicate shortcut import provider.",
            ));
        }
        let mut probes = BTreeSet::new();
        if provider.recognition.iter().any(|probe| {
            let valid = match probe {
                RecognitionProbe::File { path } | RecognitionProbe::Directory { path } => {
                    safe_relative_path(path)
                }
                RecognitionProbe::Root { components, .. } => {
                    !components.is_empty()
                        && components.len() <= 8
                        && components
                            .iter()
                            .map(|part| part.chars().count())
                            .sum::<usize>()
                            <= 256
                        && components
                            .iter()
                            .filter(|part| part.as_str() == "*")
                            .count()
                            <= 1
                        && components.first().is_some_and(|part| part != "*")
                        && components
                            .iter()
                            .position(|part| part == "*")
                            .is_none_or(|index| index + 1 == components.len())
                        && components.iter().all(|part| {
                            part == "*" || (!part.contains('/') && safe_relative_path(part))
                        })
                }
            };
            !valid || !probes.insert(probe)
        }) {
            return Err(lithe_plugin_model::invalid(
                "keymapImportProviders.recognition",
                "Invalid or duplicate shortcut source recognition probe.",
            ));
        }
        let mut inputs = BTreeSet::new();
        if provider
            .inputs
            .iter()
            .any(|input| !valid_selector(input) || !inputs.insert(input))
        {
            return Err(lithe_plugin_model::invalid(
                "keymapImportProviders.inputs",
                "Invalid or duplicate shortcut source selector.",
            ));
        }
    }
    providers.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    #[serde(rename = "commandIDs")]
    command_ids: Vec<String>,
    reply: Reply,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    bindings: Vec<CommandBindings>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CommandBindings {
    #[serde(rename = "commandID")]
    command_id: String,
    keys: Vec<Shortcut>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
struct Shortcut {
    key: String,
    modifiers: Vec<Modifier>,
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
enum Modifier {
    Control,
    Alt,
    Shift,
    Meta,
}

fn invalid() -> CoreError {
    CoreError::new(
        ErrorCode::InvalidRequest,
        "Invalid shortcut import provider reply.",
    )
}

fn identifier(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

fn supported_key(key: &str) -> bool {
    if key.len() == 1 {
        return key.as_bytes()[0].is_ascii_graphic() && !key.as_bytes()[0].is_ascii_uppercase();
    }
    if let Some(number) = key
        .strip_prefix('f')
        .and_then(|value| value.parse::<u8>().ok())
    {
        return (1..=24).contains(&number) && key == format!("f{number}");
    }
    matches!(
        key,
        "up" | "down"
            | "left"
            | "right"
            | "return"
            | "tab"
            | "space"
            | "backspace"
            | "delete"
            | "escape"
            | "pageup"
            | "pagedown"
            | "home"
            | "end"
            | "insert"
    )
}

/// Validates an untrusted provider reply against the host's current action IDs.
/// This does not inspect source formats, resolve conditions or apply the bindings.
pub fn decode_command(payload: Value) -> Result<Value, CoreError> {
    if serde_json::to_vec(&payload).map_err(|_| invalid())?.len() > MAX_BYTES {
        return Err(invalid());
    }
    let mut request: Request = serde_json::from_value(payload).map_err(|_| invalid())?;
    if request.command_ids.len() > MAX_COMMANDS || request.reply.bindings.len() > MAX_COMMANDS {
        return Err(invalid());
    }
    let mut allowed = BTreeSet::new();
    for id in &request.command_ids {
        if !identifier(id) || !allowed.insert(id) {
            return Err(invalid());
        }
    }
    let mut seen = BTreeSet::new();
    for entry in &mut request.reply.bindings {
        if !allowed.contains(&entry.command_id)
            || !seen.insert(&entry.command_id)
            || entry.keys.len() > MAX_KEYS_PER_COMMAND
        {
            return Err(invalid());
        }
        let mut keys = BTreeSet::new();
        for key in &mut entry.keys {
            if !supported_key(&key.key) || key.modifiers.len() > 4 {
                return Err(invalid());
            }
            key.modifiers.sort();
            if key.modifiers.windows(2).any(|pair| pair[0] == pair[1])
                || (key.key.len() == 1 || key.key == "space")
                    && !key.modifiers.iter().any(|modifier| {
                        matches!(modifier, Modifier::Control | Modifier::Alt | Modifier::Meta)
                    })
                || !keys.insert(key.clone())
            {
                return Err(invalid());
            }
        }
    }
    // Action ordering is canonical, but key order determines the primary binding.
    request
        .reply
        .bindings
        .sort_by(|a, b| a.command_id.cmp(&b.command_id));
    serde_json::to_value(request.reply).map_err(|_| invalid())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BindingPlanRequest {
    current: Vec<CurrentBindings>,
    incoming: Reply,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CurrentBindings {
    #[serde(rename = "commandID")]
    command_id: String,
    customized: bool,
    keys: Vec<Shortcut>,
}

/// Plans only the keys displaced by an import; existing custom owners and
/// duplicate imported claims are conflicts. The host applies the result only
/// after rechecking the current settings and source revisions.
pub fn plan_bindings_command(payload: Value) -> Result<Value, CoreError> {
    if serde_json::to_vec(&payload).map_err(|_| invalid())?.len() > MAX_BYTES {
        return Err(invalid());
    }
    let mut request: BindingPlanRequest = serde_json::from_value(payload).map_err(|_| invalid())?;
    if request.current.len() > MAX_COMMANDS {
        return Err(invalid());
    }
    let mut ids = BTreeSet::new();
    let mut owners: BTreeMap<Shortcut, Vec<(String, bool)>> = BTreeMap::new();
    for entry in &mut request.current {
        if !identifier(&entry.command_id)
            || !ids.insert(entry.command_id.clone())
            || entry.keys.len() > MAX_KEYS_PER_COMMAND
        {
            return Err(invalid());
        }
        let mut seen = BTreeSet::new();
        for key in &mut entry.keys {
            // Existing native bindings may use keys outside the portable
            // import vocabulary. Keep them as opaque, bounded facts: only
            // an exact match with a validated imported key can be displaced.
            if !identifier(&key.key) || key.modifiers.len() > 4 {
                return Err(invalid());
            }
            key.modifiers.sort();
            if key.modifiers.windows(2).any(|pair| pair[0] == pair[1]) || !seen.insert(key.clone())
            {
                return Err(invalid());
            }
            owners
                .entry(key.clone())
                .or_default()
                .push((entry.command_id.clone(), entry.customized));
        }
    }
    let validated =
        decode_command(serde_json::json!({ "commandIDs": ids, "reply": request.incoming }))?;
    let incoming: Reply = serde_json::from_value(validated).map_err(|_| invalid())?;
    let replacing: BTreeSet<_> = incoming
        .bindings
        .iter()
        .map(|entry| entry.command_id.as_str())
        .collect();
    let mut imported_owners: BTreeMap<Shortcut, String> = BTreeMap::new();
    let mut conflicts: BTreeSet<(String, String, Shortcut)> = BTreeSet::new();
    let mut displaced: BTreeMap<String, BTreeSet<Shortcut>> = BTreeMap::new();
    let mut comparisons = 0usize;
    for entry in &incoming.bindings {
        for key in &entry.keys {
            if let Some(first) = imported_owners.get(key) {
                conflicts.insert((entry.command_id.clone(), first.clone(), key.clone()));
                if conflicts.len() > MAX_COMMANDS {
                    return Err(invalid());
                }
            } else {
                imported_owners.insert(key.clone(), entry.command_id.clone());
            }
            for (owner, customized) in owners.get(key).into_iter().flatten() {
                comparisons += 1;
                if comparisons > MAX_COMMANDS * MAX_KEYS_PER_COMMAND * 4 {
                    return Err(invalid());
                }
                if replacing.contains(owner.as_str()) {
                    continue;
                }
                if *customized {
                    conflicts.insert((entry.command_id.clone(), owner.clone(), key.clone()));
                } else {
                    displaced
                        .entry(owner.clone())
                        .or_default()
                        .insert(key.clone());
                }
                if conflicts.len() > MAX_COMMANDS {
                    return Err(invalid());
                }
            }
        }
    }
    // A conflicting import has no partial plan the host could accidentally apply.
    if !conflicts.is_empty() {
        displaced.clear();
    }
    let displaced: Vec<_> = displaced
        .into_iter()
        .map(|(command_id, keys)| serde_json::json!({ "commandID": command_id, "keys": keys }))
        .collect();
    let conflicts: Vec<_> = conflicts.into_iter().map(|(command_id, owner_id, key)| {
        serde_json::json!({ "commandID": command_id, "ownerID": owner_id, "key": key })
    }).collect();
    Ok(serde_json::json!({ "displaced": displaced, "conflicts": conflicts }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourcePlanRequest {
    manifest: Value,
    #[serde(rename = "providerID")]
    provider_id: String,
}

/// What a source plan reads from one plugin manifest that the plugin host
/// validated; the host's manifest type implements it.
pub trait KeymapImportManifest: PluginManifest {
    /// The manifest's validated `keymapImportProviders`.
    fn keymap_import_providers(&self) -> &[ProviderDeclaration];
}

/// Plans the declared inputs for a plugin-local import provider. The caller
/// supplies a previously accepted manifest; this operation also validates it.
pub fn plan_sources_command<M: KeymapImportManifest>(payload: Value) -> Result<Value, CoreError> {
    let bad_plan = || {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid shortcut import source plan.",
        )
    };
    if serde_json::to_vec(&payload).map_err(|_| bad_plan())?.len() > MAX_BYTES {
        return Err(bad_plan());
    }
    let request: SourcePlanRequest = serde_json::from_value(payload).map_err(|_| bad_plan())?;
    if !valid_provider_id(&request.provider_id) {
        return Err(bad_plan());
    }
    let manifest = M::validate(request.manifest).map_err(|_| bad_plan())?;
    let provider = manifest
        .keymap_import_providers()
        .iter()
        .find(|provider| provider.id == request.provider_id)
        .ok_or_else(bad_plan)?;
    let provider = serde_json::to_value(provider).map_err(|_| bad_plan())?;
    Ok(serde_json::json!({
        "inputs": provider["inputs"],
        "recognition": provider["recognition"],
        "limits": {
            "selectors": MAX_INPUT_SELECTORS, "probes": MAX_RECOGNITION_PROBES, "files": MAX_INPUT_FILES,
            "fileBytes": MAX_INPUT_FILE_BYTES, "totalBytes": MAX_INPUT_TOTAL_BYTES,
            "chunkBytes": MAX_INPUT_CHUNK_BYTES, "directoryEntries": MAX_DIRECTORY_ENTRIES,
            "directoryMatches": MAX_DIRECTORY_MATCHES
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::decode_command;
    use serde_json::{json, Value};

    #[test]
    fn normalized_import_provider_replies_follow_shared_fixtures() {
        let fixtures: Value = serde_json::from_str(include_str!(
            "../../../fixtures/keymap/import-provider-v1.json"
        ))
        .unwrap();
        for case in fixtures["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let actual = decode_command(case["request"].clone());
            if let Some(expected) = case.get("expected") {
                assert_eq!(
                    actual.unwrap_or_else(|e| panic!("{name}: {e:?}")),
                    *expected,
                    "{name}"
                );
            } else {
                assert!(actual.is_err(), "{name}");
            }
        }
    }

    #[test]
    fn provider_reply_limits_are_checked_before_application() {
        let keys: Vec<Value> = (1..=9)
            .map(|number| json!({"key": format!("f{number}"), "modifiers": []}))
            .collect();
        let mut request = json!({"commandIDs": ["find-next"], "reply": {"bindings": [
            {"commandID": "find-next", "keys": keys}
        ]}});
        assert!(decode_command(request.clone()).is_err());
        let _ = request["reply"]["bindings"][0]["keys"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert!(decode_command(request).is_ok());

        let oversized = "a".repeat(1024 * 1024);
        assert!(
            decode_command(json!({"commandIDs": ["find-next"], "reply": {"bindings": [
                {"commandID": "find-next", "keys": [{"key": oversized, "modifiers": ["meta"]}]}
            ]}}))
            .is_err()
        );
    }
}
