//! Native-backed pin documents: bounded read plans and pure complete-source edits.
//!
//! Hosts own filesystem identity, metadata, cancellation and conditional atomic
//! publication. Reading or editing these documents never grants execution authority.

use super::pins;
use crate::plugins::declarative::{PluginRejection, Validation};
use crate::protocol::json_document::parse_unique_json;
use ignore::gitignore::GitignoreBuilder;
use serde::Deserialize;
use serde_json::{json, Value};

const DOCUMENT_PATH: &str = ".lithe/toolchains/local.json";
const IGNORE_PATH: &str = ".lithe/.gitignore";
const IGNORE_ENTRY: &str = "/toolchains/local.json";
const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;
const MAX_IGNORE_BYTES: usize = 64 * 1024;
const MAX_IGNORE_LINES: usize = 4096;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecodeRequest {
    /// Null means the native source does not exist; an empty string is explicit.
    document: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EditRequest {
    document: Value,
    ignore_document: Value,
    directory: String,
    /// Passed unchanged to the existing bounded pin edit validator.
    edits: Value,
}

fn invalid(field: &str) -> PluginRejection {
    PluginRejection::at(
        "plugin.toolchain.invalidParams",
        field,
        "Expected complete native source strings or null and a pin edit request.",
    )
}

fn limit(field: &str) -> PluginRejection {
    PluginRejection::at(
        "plugin.toolchain.limitExceeded",
        field,
        "Toolchain source exceeds its byte or line budget.",
    )
}

fn invalid_document(field: &str) -> PluginRejection {
    PluginRejection::at(
        "plugin.toolchain.invalidDocument",
        field,
        "Expected a bounded version 1 toolchain document with unique JSON keys.",
    )
}

fn invalid_ignore() -> PluginRejection {
    PluginRejection::at(
        "plugin.toolchain.invalidIgnore",
        "ignoreDocument",
        "Expected a bounded text file of valid Git ignore patterns.",
    )
}

fn source<'a>(value: &'a Value, field: &str, maximum: usize) -> Validation<Option<&'a str>> {
    match value {
        Value::Null => Ok(None),
        Value::String(text) if text.len() <= maximum => Ok(Some(text)),
        Value::String(_) => Err(limit(field)),
        _ => Err(invalid(field)),
    }
}

fn document(content: Option<&str>) -> Validation<Value> {
    let value = match content {
        None => json!({"version": 1, "toolchains": {}}),
        Some(text) => parse_unique_json(text).map_err(|_| invalid_document("document"))?,
    };
    pins::validate(&value)
        .map_err(|failure| invalid_document(failure.field.as_deref().unwrap_or("document")))?;
    Ok(value)
}

/// Gives native adapters the workspace-relative sources and their read budgets.
/// The source paths are owned by this contract, not inferred from an app package.
pub(crate) fn storage_plan(input: &Value) -> Validation<Value> {
    if !input.as_object().is_some_and(|object| object.is_empty()) {
        return Err(invalid("input"));
    }
    Ok(json!({
        "documentRelativePath": DOCUMENT_PATH,
        "ignoreRelativePath": IGNORE_PATH,
        "maximumDocumentBytes": MAX_DOCUMENT_BYTES,
        "maximumIgnoreBytes": MAX_IGNORE_BYTES,
    }))
}

/// Parses one complete source or supplies the absent-source default. This also
/// validates dormant entries when no active tool currently consumes them.
pub(crate) fn decode(input: &Value) -> Validation<Value> {
    let request: DecodeRequest =
        serde_json::from_value(input.clone()).map_err(|_| invalid("input"))?;
    document(source(&request.document, "document", MAX_DOCUMENT_BYTES)?)
}

/// Edits captured sources as one plan. No semantic pin change preserves exact
/// source bytes and does not create a document or ignore file for a missing source.
pub(crate) fn edit(input: &Value) -> Validation<Value> {
    let request: EditRequest =
        serde_json::from_value(input.clone()).map_err(|_| invalid("input"))?;
    let original = source(&request.document, "document", MAX_DOCUMENT_BYTES)?;
    let ignore = source(&request.ignore_document, "ignoreDocument", MAX_IGNORE_BYTES)?;
    let parsed = document(original)?;
    let mut edited = pins::edit(&json!({
        "document": parsed,
        "directory": request.directory,
        "edits": request.edits,
    }))?["document"]
        .clone();
    if edited == parsed {
        return Ok(json!({
            "document": original,
            "documentChanged": false,
            "ignoreDocument": ignore,
            "ignoreChanged": false,
        }));
    }
    // Keep source output deterministic across serde_json map feature unification.
    edited.sort_all_objects();
    let encoded = serde_json::to_string(&edited).map_err(|_| invalid_document("document"))?;
    if encoded.len() > MAX_DOCUMENT_BYTES {
        return Err(limit("document"));
    }
    let (ignore_document, ignore_changed) = prepare_ignore(ignore)?;
    Ok(json!({
        "document": encoded,
        "documentChanged": true,
        "ignoreDocument": ignore_document,
        "ignoreChanged": ignore_changed,
    }))
}

fn prepare_ignore(original: Option<&str>) -> Validation<(String, bool)> {
    let content = original.unwrap_or("");
    if content.contains('\0') {
        return Err(invalid_ignore());
    }
    let mut builder = GitignoreBuilder::new("");
    for (index, line) in content.lines().enumerate() {
        if index >= MAX_IGNORE_LINES {
            return Err(limit("ignoreDocument"));
        }
        // UTF-8 BOM is an encoding marker in Git's ignore files, not a glob.
        let line = if index == 0 {
            line.strip_prefix('\u{feff}').unwrap_or(line)
        } else {
            line
        };
        builder.add_line(None, line).map_err(|_| invalid_ignore())?;
    }
    let matcher = builder.build().map_err(|_| invalid_ignore())?;
    if matcher
        .matched_path_or_any_parents("toolchains/local.json", false)
        .is_ignore()
    {
        return Ok((content.to_owned(), false));
    }
    let newline = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut updated = content.to_owned();
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push_str(newline);
    }
    updated.push_str(IGNORE_ENTRY);
    updated.push_str(newline);
    if updated.len() > MAX_IGNORE_BYTES || updated.lines().count() > MAX_IGNORE_LINES {
        return Err(limit("ignoreDocument"));
    }
    Ok((updated, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_edits_match_shared_fixture() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../../fixtures/toolchain-pin-storage.json"))
                .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let response: Value = serde_json::from_str(&crate::execute_json(
                &json!({"command": case["command"], "payload": case["input"]}).to_string(),
            ))
            .unwrap();
            if let Some(code) = case.get("error") {
                assert_eq!(response["data"]["status"], "rejected", "{case}: {response}");
                assert_eq!(
                    &response["data"]["error"]["code"], code,
                    "{case}: {response}"
                );
            } else {
                assert_eq!(response["data"], case["expected"], "{case}: {response}");
            }
        }
    }

    #[test]
    fn ignore_growth_cannot_publish_a_document_the_next_edit_would_refuse() {
        let full = "#\n".repeat(MAX_IGNORE_LINES);
        assert_eq!(
            prepare_ignore(Some(&full)).unwrap_err().code(),
            "plugin.toolchain.limitExceeded"
        );
    }

    #[test]
    fn oversized_sources_are_rejected_before_schema_or_glob_parsing() {
        assert_eq!(
            decode(&json!({"document":" ".repeat(MAX_DOCUMENT_BYTES + 1)}))
                .unwrap_err()
                .code(),
            "plugin.toolchain.limitExceeded"
        );
        assert_eq!(
            edit(
                &json!({"document":null,"ignoreDocument":" ".repeat(MAX_IGNORE_BYTES + 1),
            "directory":".","edits":[{"tool":"dev.example:compiler","pin":null}]})
            )
            .unwrap_err()
            .code(),
            "plugin.toolchain.limitExceeded"
        );
    }
}
