//! Build options: the user's stored choices checked against the
//! declaration, their effective values, the arguments and environment they
//! contribute, and the fingerprint hosts use to notice a changed build
//! configuration.

use super::manifest::{BuildSystemDeclaration, OptionDeclaration, OptionKind};
use super::template::{self, Values};
use lithe_plugin_model::{PluginRejection, Validation};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const FINGERPRINT_DOMAIN: &str = "lithe.build.options/1";

/// Model-supplied facts for `choicesFromModel` options: available values,
/// values active when the option is unset, and per-option exclusion prefixes.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ModelOptionFacts<'a> {
    pub(crate) values: Option<&'a BTreeMap<String, Vec<String>>>,
    pub(crate) defaults: Option<&'a BTreeMap<String, Vec<String>>>,
    pub(crate) exclude_prefixes: Option<&'a BTreeMap<String, String>>,
}

/// One choice of a select option as resolved for display and editing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedChoice {
    pub(crate) value: String,
    pub(crate) title: Option<String>,
    pub(crate) from_model: bool,
    pub(crate) custom: bool,
    pub(crate) default_active: bool,
    pub(crate) checked: bool,
}

/// Effective option values of one request, by option ID. Options with no
/// effective value contribute nothing and are absent.
#[derive(Debug, Clone, Default)]
pub(crate) struct ResolvedOptions {
    pub(crate) values: BTreeMap<String, Value>,
    /// Which values the user chose, as opposed to declaration defaults.
    pub(crate) chosen: Vec<String>,
    /// The accepted user values, including empty text and empty selections
    /// that contribute no argument. The host needs them to tell an explicit
    /// clear from "no stored value".
    pub(crate) chosen_values: BTreeMap<String, Value>,
    /// Options whose stored value the declaration refuses in whole or in
    /// part, so the host can store the effective value instead.
    pub(crate) dropped: Vec<String>,
}

/// Effective exclusion prefix of `option`, preferring the declaration's own.
pub(crate) fn effective_exclude_prefix<'a>(
    option: &'a OptionDeclaration,
    facts: &ModelOptionFacts<'a>,
) -> Option<&'a str> {
    option.exclude_prefix.as_deref().or_else(|| {
        facts
            .exclude_prefixes
            .and_then(|prefixes| prefixes.get(&option.id).map(String::as_str))
            .filter(|prefix| option.is_valid_exclude_prefix(prefix))
    })
}

/// Checks stored values against the declaration. `model_values` are the
/// model's `optionValues`, or `None` when no model is loaded yet.
pub(crate) fn resolve(
    build_system: &BuildSystemDeclaration,
    stored: &BTreeMap<String, Value>,
    model_values: Option<&BTreeMap<String, Vec<String>>>,
) -> ResolvedOptions {
    resolve_with_facts(
        build_system,
        stored,
        ModelOptionFacts {
            values: model_values,
            ..ModelOptionFacts::default()
        },
    )
}

/// Checks stored values against the declaration and full model option facts.
pub(crate) fn resolve_with_facts(
    build_system: &BuildSystemDeclaration,
    stored: &BTreeMap<String, Value>,
    facts: ModelOptionFacts<'_>,
) -> ResolvedOptions {
    let mut resolved = ResolvedOptions::default();
    for id in stored.keys() {
        if build_system.option(id).is_none() {
            resolved.dropped.push(id.clone());
        }
    }
    for option in &build_system.options {
        let known = facts
            .values
            .map(|values| values.get(&option.id).map(Vec::as_slice).unwrap_or(&[]));
        let exclude_prefix = effective_exclude_prefix(option, &facts);
        let user = stored
            .get(&option.id)
            .map(|value| accepted(option, value, known, exclude_prefix));
        let value = match user {
            Some(Some((value, discarded))) => {
                resolved.chosen.push(option.id.clone());
                resolved
                    .chosen_values
                    .insert(option.id.clone(), value.clone());
                if discarded {
                    resolved.dropped.push(option.id.clone());
                }
                Some(value)
            }
            Some(None) => {
                resolved.dropped.push(option.id.clone());
                default_value(option)
            }
            None => default_value(option),
        };
        if let Some(value) = value.filter(|value| contributes(option, value)) {
            resolved.values.insert(option.id.clone(), value);
        }
    }
    resolved.dropped.sort();
    resolved
}

fn default_value(option: &OptionDeclaration) -> Option<Value> {
    option.default.clone()
}

/// The normalized value and whether part of the stored value was
/// discarded, or `None` when the declaration refuses all of it. A
/// multi-select keeps the items it still accepts, so a feature removed from
/// the build file does not clear the rest of the selection; its values are
/// sorted so equal selections fingerprint equally.
fn accepted(
    option: &OptionDeclaration,
    value: &Value,
    known: Option<&[String]>,
    exclude_prefix: Option<&str>,
) -> Option<(Value, bool)> {
    match (option.kind, value) {
        (OptionKind::Boolean, Value::Bool(_)) => Some((value.clone(), false)),
        (OptionKind::SingleSelect, Value::String(text)) => option
            .accepts_value_with_prefix(text, known, exclude_prefix)
            .then(|| (value.clone(), false)),
        (OptionKind::MultiSelect, Value::Array(items)) => {
            let mut values = Vec::new();
            let mut discarded = false;
            for item in items {
                let text = item.as_str()?;
                if option.accepts_value_with_prefix(text, known, exclude_prefix) {
                    values.push(text.to_string());
                } else {
                    discarded = true;
                }
            }
            if discarded && values.is_empty() {
                return None;
            }
            if let Some(prefix) = exclude_prefix {
                let positives: BTreeSet<String> = values
                    .iter()
                    .filter(|item| !item.starts_with(prefix))
                    .cloned()
                    .collect();
                let before = values.len();
                values.retain(|item| {
                    item.strip_prefix(prefix)
                        .is_none_or(|rest| !positives.contains(rest))
                });
                discarded |= values.len() != before;
            }
            values.sort();
            values.dedup();
            Some((json!(values), discarded))
        }
        (OptionKind::Text | OptionKind::File | OptionKind::Directory, Value::String(text)) => {
            (text.is_empty() || super::manifest::is_option_value(text))
                .then(|| (value.clone(), false))
        }
        _ => None,
    }
}

/// False for values that mean "not set": `false`, `""`, and `[]`.
fn contributes(option: &OptionDeclaration, value: &Value) -> bool {
    match (option.kind, value) {
        (OptionKind::Boolean, Value::Bool(flag)) => *flag,
        (_, Value::String(text)) => !text.is_empty(),
        (_, Value::Array(items)) => !items.is_empty(),
        _ => false,
    }
}

/// Text substituted for `${value}`.
fn value_text(option: &OptionDeclaration, value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => Some(
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(option.separator()),
        ),
        _ => None,
    }
}

/// Arguments and environment of every option with an effective value, in
/// declaration order.
pub(crate) struct OptionContribution {
    pub(crate) arguments: Vec<(String, Vec<String>)>,
    pub(crate) environment: BTreeMap<String, String>,
}

pub(crate) fn contribution(
    build_system: &BuildSystemDeclaration,
    resolved: &ResolvedOptions,
) -> Validation<OptionContribution> {
    let mut arguments = Vec::new();
    let mut environment = BTreeMap::new();
    for option in &build_system.options {
        let Some(value) = resolved.values.get(&option.id) else {
            continue;
        };
        let text = value_text(option, value);
        let values = Values {
            value: text.as_deref(),
            ..Values::default()
        };
        let expanded = option
            .arguments
            .iter()
            .map(|argument| template::expand_text(argument, &values))
            .collect::<Validation<Vec<_>>>()?;
        arguments.push((option.id.clone(), expanded));
        for (name, value) in &option.environment {
            environment.insert(name.clone(), template::expand_text(value, &values)?);
        }
    }
    Ok(OptionContribution {
        arguments,
        environment,
    })
}

/// Stable digest of the effective values; equal for equal configurations
/// whichever order the host stored them in.
pub(crate) fn fingerprint(
    build_system: &BuildSystemDeclaration,
    resolved: &ResolvedOptions,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(FINGERPRINT_DOMAIN.as_bytes());
    hasher.update([0]);
    hasher.update(build_system.id.as_bytes());
    for (id, value) in &resolved.values {
        hasher.update([0]);
        hasher.update(id.as_bytes());
        hasher.update([0]);
        // `serde_json` writes object keys in insertion order; option values
        // are scalars or arrays of strings, which have one encoding.
        hasher.update(value.to_string().as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// Positive choice values that are active when `option` has no user override:
/// the declaration's `default`, else the model's `optionDefaults`.
pub(crate) fn default_choice_set(
    option: &OptionDeclaration,
    facts: &ModelOptionFacts<'_>,
) -> BTreeSet<String> {
    let mut defaults = BTreeSet::new();
    match (&option.kind, &option.default) {
        (OptionKind::SingleSelect, Some(Value::String(value))) if !value.is_empty() => {
            defaults.insert(value.clone());
            return defaults;
        }
        (OptionKind::MultiSelect, Some(Value::Array(items))) => {
            for item in items.iter().filter_map(Value::as_str) {
                defaults.insert(item.to_string());
            }
            return defaults;
        }
        _ => {}
    }
    if option.choices_from_model {
        let exclude_prefix = effective_exclude_prefix(option, facts);
        let known = facts
            .values
            .and_then(|values| values.get(&option.id).map(Vec::as_slice));
        if let Some(values) = facts.defaults.and_then(|map| map.get(&option.id)) {
            for value in values {
                if option.accepts_positive_value(value, known, exclude_prefix)
                    && known.is_none_or(|list| list.iter().any(|item| item == value))
                {
                    defaults.insert(value.clone());
                }
            }
        }
    }
    defaults
}

/// Ordered choices of `option`, with whether each is active by default and
/// currently checked.
pub(crate) fn choice_state(
    option: &OptionDeclaration,
    resolved: &ResolvedOptions,
    facts: &ModelOptionFacts<'_>,
) -> Vec<ResolvedChoice> {
    let exclude_prefix = effective_exclude_prefix(option, facts);
    let defaults = default_choice_set(option, facts);
    let chosen = resolved.chosen.contains(&option.id);
    let stored_positives: BTreeSet<&str> = resolved
        .chosen_values
        .get(&option.id)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|value| !exclude_prefix.is_some_and(|prefix| value.starts_with(prefix)))
        .collect();
    let is_checked = |value: &str| {
        if option.kind != OptionKind::MultiSelect {
            return false;
        }
        if chosen {
            stored_positives.contains(value)
        } else {
            defaults.contains(value)
        }
    };

    let mut seen = BTreeSet::new();
    let mut choices = Vec::new();
    for declared in &option.choices {
        if seen.insert(declared.value.clone()) {
            choices.push(ResolvedChoice {
                checked: is_checked(&declared.value),
                default_active: defaults.contains(&declared.value),
                value: declared.value.clone(),
                title: declared.title.clone(),
                from_model: false,
                custom: false,
            });
        }
    }
    if option.choices_from_model {
        for value in facts
            .values
            .and_then(|values| values.get(&option.id))
            .into_iter()
            .flatten()
        {
            if exclude_prefix.is_some_and(|prefix| value.starts_with(prefix)) {
                continue;
            }
            if seen.insert(value.clone()) {
                choices.push(ResolvedChoice {
                    checked: is_checked(value),
                    default_active: defaults.contains(value),
                    value: value.clone(),
                    title: None,
                    from_model: true,
                    custom: false,
                });
            }
        }
    }
    if option.kind == OptionKind::MultiSelect && option.allow_custom {
        for value in &stored_positives {
            if seen.insert((*value).to_string()) {
                choices.push(ResolvedChoice {
                    value: (*value).to_string(),
                    title: None,
                    from_model: false,
                    custom: true,
                    default_active: false,
                    checked: true,
                });
            }
        }
    }
    choices
}

/// Turns the positive choices the user checked for a `multiSelect` option into
/// the stored JSON value (`Value::Null` when no team override exists and the
/// checked set equals the default active set).
pub(crate) fn encode_selected_choices(
    option: &OptionDeclaration,
    selected: &[String],
    has_team_override: bool,
    facts: &ModelOptionFacts<'_>,
) -> Validation<Value> {
    if option.kind != OptionKind::MultiSelect {
        return Err(PluginRejection::new(
            "plugin.build.invalidOption",
            "selectedChoices applies to multiSelect options only",
        ));
    }
    let exclude_prefix = effective_exclude_prefix(option, facts);
    let known = facts
        .values
        .map(|values| values.get(&option.id).map(Vec::as_slice).unwrap_or(&[]));
    let mut checked = Vec::with_capacity(selected.len());
    for value in selected {
        if !option.accepts_positive_value(value, known, exclude_prefix) {
            return Err(PluginRejection::new(
                "plugin.build.invalidOption",
                "The option value is not accepted by its declaration",
            ));
        }
        checked.push(value.clone());
    }
    checked.sort();
    checked.dedup();
    let defaults: Vec<String> = default_choice_set(option, facts).into_iter().collect();
    if !has_team_override && checked == defaults {
        return Ok(Value::Null);
    }
    let mut items = checked.clone();
    if let Some(prefix) = exclude_prefix {
        for default in &defaults {
            if checked.binary_search(default).is_err() {
                items.push(format!("{prefix}{default}"));
            }
        }
    }
    items.sort();
    items.dedup();
    Ok(json!(items))
}

/// The `build.resolveOptions` view with model defaults and exclusion prefixes.
pub(crate) fn describe_with_facts(
    build_system: &BuildSystemDeclaration,
    resolved: &ResolvedOptions,
    facts: ModelOptionFacts<'_>,
) -> Value {
    let options: Vec<Value> = build_system
        .options
        .iter()
        .map(|option| {
            let resolved_choices = choice_state(option, resolved, &facts);
            let choices: Vec<Value> = resolved_choices
                .iter()
                .map(|choice| {
                    let mut entry = if choice.from_model {
                        json!({ "value": choice.value, "fromModel": true })
                    } else if choice.custom {
                        json!({ "value": choice.value, "custom": true })
                    } else {
                        json!({ "value": choice.value, "title": choice.title })
                    };
                    if choice.default_active {
                        entry["defaultActive"] = json!(true);
                    }
                    entry
                })
                .collect();
            let defaults = default_choice_set(option, &facts);
            let exclude_prefix = effective_exclude_prefix(option, &facts);
            let source = if resolved.chosen.contains(&option.id) {
                "user"
            } else if option.default.is_some() || !defaults.is_empty() {
                "default"
            } else {
                "unset"
            };
            let value = if resolved.chosen.contains(&option.id) {
                // An explicit empty value has no contribution but must not
                // be described as unset, or the host would drop it and a
                // non-empty default would come back.
                resolved
                    .chosen_values
                    .get(&option.id)
                    .cloned()
                    .unwrap_or(Value::Null)
            } else {
                resolved
                    .values
                    .get(&option.id)
                    .cloned()
                    .unwrap_or(match option.kind {
                        OptionKind::Boolean => json!(false),
                        OptionKind::MultiSelect => json!([]),
                        _ => Value::Null,
                    })
            };
            let mut entry = json!({
                "id": option.id,
                "displayName": option.display_name,
                "kind": option.kind,
                "value": value,
                "source": source,
                "choices": choices,
                "allowCustom": option.allow_custom,
            });
            if option.kind == OptionKind::MultiSelect
                && (!defaults.is_empty()
                    || exclude_prefix.is_some()
                    || resolved_choices.iter().any(|choice| choice.custom))
            {
                let selected: Vec<&str> = resolved_choices
                    .iter()
                    .filter(|choice| choice.checked)
                    .map(|choice| choice.value.as_str())
                    .collect();
                entry["selectedChoices"] = json!(selected);
            }
            if option.scope == super::manifest::OptionScope::Instance {
                entry["scope"] = json!("instance");
            }
            entry
        })
        .collect();
    json!({
        "options": options,
        "dropped": resolved.dropped,
        "fingerprint": fingerprint(build_system, resolved),
    })
}
