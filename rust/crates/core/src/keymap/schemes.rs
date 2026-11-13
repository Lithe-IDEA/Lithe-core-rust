//! Named shortcut schemes with independent overrides and an undeletable default.

use super::bindings::{parse_binding, BindingPlatform};
use crate::protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

type Bindings = BTreeMap<String, Vec<String>>;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Scheme {
    id: String,
    /// An empty default name is localized by the host; user names are never translated.
    #[serde(default)]
    name: String,
    #[serde(default)]
    bindings: Bindings,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct State {
    #[serde(default)]
    active: String,
    #[serde(default)]
    schemes: Vec<Scheme>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    #[serde(default)]
    state: State,
    /// Existing single-scheme overrides migrate only when no schemes have been stored.
    #[serde(default)]
    current: Bindings,
    #[serde(default)]
    operation: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    bindings: Bindings,
}

fn rejected(reason: &str) -> CoreError {
    CoreError::new(
        ErrorCode::InvalidRequest,
        "Shortcut scheme operation was rejected",
    )
    .with_details(reason)
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn validate_bindings(bindings: &Bindings) -> Result<(), CoreError> {
    if bindings.len() > 4096 {
        return Err(rejected("too_many_bindings"));
    }
    for (command, keys) in bindings {
        if command.is_empty() || command.len() > 128 || keys.len() > 8 {
            return Err(rejected("invalid_binding"));
        }
        for key in keys {
            // The native catalog already uses explicit meta/ctrl names. Platform aliases
            // have no role in a portable saved scheme; parsing still uses the shared grammar.
            if parse_binding(key, BindingPlatform::Macos).is_err() {
                return Err(rejected("invalid_binding"));
            }
        }
    }
    Ok(())
}

pub(crate) fn reduce_command(payload: Value) -> Result<Value, CoreError> {
    if payload.to_string().len() > 1024 * 1024 {
        return Err(rejected("too_large"));
    }
    let mut request: Request =
        serde_json::from_value(payload).map_err(|_| rejected("invalid_request"))?;
    let state = &mut request.state;
    if state.schemes.is_empty() {
        validate_bindings(&request.current)?;
        state.schemes.push(Scheme {
            id: "default".into(),
            name: String::new(),
            bindings: request.current,
        });
        state.active = "default".into();
    }
    if state.schemes.len() > 16 {
        return Err(rejected("too_many_schemes"));
    }
    let mut ids = BTreeSet::new();
    for scheme in &state.schemes {
        if !valid_id(&scheme.id) || !ids.insert(scheme.id.clone()) || scheme.name.len() > 256 {
            return Err(rejected("invalid_scheme"));
        }
        validate_bindings(&scheme.bindings)?;
    }
    if !ids.contains("default") {
        return Err(rejected("missing_default"));
    }
    if !ids.contains(&state.active) {
        state.active = "default".into();
    }
    let name = request.name.trim();
    if matches!(request.operation.as_str(), "saveAs" | "rename") {
        if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
            return Err(rejected("invalid_name"));
        }
        if state
            .schemes
            .iter()
            .any(|s| s.id != request.id && s.name.to_lowercase() == name.to_lowercase())
        {
            return Err(rejected("duplicate_name"));
        }
    }
    match request.operation.as_str() {
        "" => {}
        "update" => {
            validate_bindings(&request.bindings)?;
            state
                .schemes
                .iter_mut()
                .find(|s| s.id == state.active)
                .expect("active scheme was validated")
                .bindings = request.bindings;
        }
        "select" => {
            if !ids.contains(&request.id) {
                return Err(rejected("missing_scheme"));
            }
            state.active = request.id;
        }
        "saveAs" => {
            if state.schemes.len() == 16 {
                return Err(rejected("too_many_schemes"));
            }
            if !valid_id(&request.id) || ids.contains(&request.id) {
                return Err(rejected("invalid_id"));
            }
            let mut saved = state
                .schemes
                .iter()
                .find(|s| s.id == state.active)
                .expect("active scheme was validated")
                .clone();
            saved.id = request.id.clone();
            saved.name = name.to_string();
            state.schemes.push(saved);
            state.active = request.id;
        }
        "rename" => {
            if request.id == "default" {
                return Err(rejected("default_is_fixed"));
            }
            state
                .schemes
                .iter_mut()
                .find(|s| s.id == request.id)
                .ok_or_else(|| rejected("missing_scheme"))?
                .name = name.to_string();
        }
        "delete" => {
            if request.id == "default" {
                return Err(rejected("default_is_fixed"));
            }
            if !ids.contains(&request.id) {
                return Err(rejected("missing_scheme"));
            }
            state.schemes.retain(|s| s.id != request.id);
            if state.active == request.id {
                state.active = "default".into();
            }
        }
        _ => return Err(rejected("unknown_operation")),
    }
    let effective = &state
        .schemes
        .iter()
        .find(|s| s.id == state.active)
        .expect("active scheme still exists")
        .bindings;
    Ok(json!({"state":state,"bindings":effective}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_copy_switch_and_delete_preserve_independent_overrides() {
        let default = reduce_command(json!({"current":{"search.files":["meta+f"]}})).unwrap();
        let copied = reduce_command(
            json!({"state":default["state"],"operation":"saveAs","id":"mine","name":"My keys"}),
        )
        .unwrap();
        let edited = reduce_command(json!({"state":copied["state"],"operation":"update","bindings":{"search.files":["meta+g"]}})).unwrap();
        let switched =
            reduce_command(json!({"state":edited["state"],"operation":"select","id":"default"}))
                .unwrap();
        assert_eq!(switched["bindings"]["search.files"], json!(["meta+f"]));
        let deleted =
            reduce_command(json!({"state":edited["state"],"operation":"delete","id":"mine"}))
                .unwrap();
        assert_eq!(deleted["state"]["active"], "default");
        assert_eq!(deleted["bindings"], default["bindings"]);
    }
    #[test]
    fn invalid_names_duplicate_names_and_default_deletion_are_rejected() {
        assert!(reduce_command(json!({"operation":"saveAs","id":"a","name":" \n"})).is_err());
        assert!(reduce_command(json!({"operation":"delete","id":"default"})).is_err());
        let state = reduce_command(json!({"operation":"saveAs","id":"a","name":"Mine"})).unwrap()
            ["state"]
            .clone();
        assert_eq!(
            reduce_command(json!({"state":state,"operation":"saveAs","id":"b","name":"mine"}))
                .unwrap_err()
                .details
                .as_deref(),
            Some("duplicate_name")
        );
    }
}
