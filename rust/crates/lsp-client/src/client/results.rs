//! Parsing feature results into Core shapes.

use super::*;

pub(crate) fn lsp_feature_result_for_method(
    method: Option<&str>,
    result: Option<&Value>,
) -> Option<Value> {
    let result = result?;
    match method {
        Some("textDocument/completion") => Some(json!({
            "items": parse_completion_items(result),
            "isIncomplete": result.get("isIncomplete").and_then(Value::as_bool).unwrap_or(false)
        })),
        Some("completionItem/resolve") => parse_completion_item(result).map(|item| {
            json!({
                "item": item
            })
        }),
        Some("textDocument/hover") => Some(json!({
            "hover": parse_hover(result)
        })),
        Some("textDocument/rename") => Some(json!({
            "changes": parse_workspace_edit(result)
        })),
        Some("textDocument/formatting" | "textDocument/rangeFormatting") => Some(json!({
            "edits": result
                .as_array()
                .map(|edits| edits.iter().filter_map(parse_lsp_text_edit_value).collect::<Vec<_>>())
                .unwrap_or_default()
        })),
        Some("textDocument/documentSymbol") => Some(json!({
            "symbols": parse_document_symbols(result)
        })),
        Some("textDocument/inlayHint") => Some(json!({
            "hints": parse_inlay_hints(result)
        })),
        Some("textDocument/foldingRange") => Some(json!({
            "ranges": parse_folding_ranges(result)
        })),
        Some("textDocument/documentLink") => Some(json!({
            "links": parse_document_links(result)
        })),
        Some("workspace/symbol") => Some(json!({
            "symbols": parse_workspace_symbols(result)
        })),
        Some("textDocument/codeLens") => Some(json!({
            "lenses": parse_code_lenses(result)
        })),
        Some("textDocument/codeAction") => Some(json!({
            "actions": parse_code_actions(result)
        })),
        Some("codeAction/resolve") => parse_code_action(result).map(|action| {
            json!({
                "action": action
            })
        }),
        Some("workspace/executeCommand") => Some(json!({ "value": result })),
        // Wrapped like a command result so no provider-specific normalizer
        // reinterprets fields of the document Core decodes.
        Some(method)
            if crate::build_provider::is_build_method(method)
                || method == lithe_plugin_rpc::INVOKE_METHOD =>
        {
            Some(json!({ "value": result }))
        }
        Some("textDocument/definition")
        | Some("textDocument/declaration")
        | Some("textDocument/typeDefinition")
        | Some("textDocument/implementation")
        | Some("textDocument/references") => Some(json!({
            "locations": parse_locations(result)
        })),
        _ => Some(result.clone()),
    }
}

fn parse_completion_items(result: &Value) -> Vec<Value> {
    let values = result
        .as_array()
        .or_else(|| result.get("items").and_then(Value::as_array));
    let Some(values) = values else {
        return Vec::new();
    };
    values.iter().filter_map(parse_completion_item).collect()
}

fn parse_completion_item(item: &Value) -> Option<Value> {
    let label = item.get("label").and_then(Value::as_str)?;
    let insert_text = item
        .get("insertText")
        .and_then(Value::as_str)
        .or_else(|| {
            item.get("textEdit")
                .and_then(|edit| edit.get("newText"))
                .and_then(Value::as_str)
        })
        .unwrap_or(label);
    let mut mapped = json!({
        "label": label,
        "insertText": insert_text,
        "insertTextFormat": item.get("insertTextFormat").and_then(Value::as_u64).unwrap_or(1)
    });
    if let Some(kind) = item.get("kind").and_then(Value::as_i64) {
        mapped["kind"] = json!(kind);
    }
    if let Some(detail) = item.get("detail").and_then(Value::as_str) {
        mapped["detail"] = json!(detail);
    }
    if let Some(documentation) = completion_documentation(item.get("documentation")) {
        mapped["documentation"] = json!(documentation);
    }
    if let Some(sort_text) = item.get("sortText").and_then(Value::as_str) {
        mapped["sortText"] = json!(sort_text);
    }
    if let Some(filter_text) = item.get("filterText").and_then(Value::as_str) {
        mapped["filterText"] = json!(filter_text);
    }
    if let Some(edit) = item.get("textEdit").and_then(parse_lsp_text_edit_value) {
        mapped["textEdit"] = edit;
    }
    if let Some(edits) = item
        .get("additionalTextEdits")
        .and_then(Value::as_array)
        .map(|edits| {
            edits
                .iter()
                .filter_map(parse_lsp_text_edit_value)
                .collect::<Vec<_>>()
        })
        .filter(|edits| !edits.is_empty())
    {
        mapped["additionalTextEdits"] = json!(edits);
    }
    // `data: null` makes some servers treat resolve as a no-op, so only
    // forward a real resolve token. Namespace display lives in labelDetails;
    // import `use`/`import` edits usually arrive only from completionItem/resolve.
    if let Some(data) = item.get("data").filter(|value| !value.is_null()) {
        mapped["data"] = data.clone();
    }
    if let Some(details) = item.get("labelDetails").filter(|value| value.is_object()) {
        mapped["labelDetails"] = details.clone();
    }
    Some(mapped)
}

fn completion_documentation(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Object(object) => object
            .get("value")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        _ => None,
    }
}

fn parse_hover(result: &Value) -> Option<Value> {
    if result.is_null() {
        return None;
    }
    let contents = hover_contents(result.get("contents").unwrap_or(result))?;
    let range = result.get("range").and_then(parse_lsp_range_value);
    Some(json!({
        "contents": contents.0,
        "isMarkdown": contents.1,
        "range": range
    }))
}

fn hover_contents(value: &Value) -> Option<(String, bool)> {
    match value {
        Value::String(text) => Some((text.clone(), false)),
        Value::Object(object) => {
            if let Some(value) = object.get("value").and_then(Value::as_str) {
                let is_markdown = object
                    .get("kind")
                    .and_then(Value::as_str)
                    .map(|kind| kind == "markdown")
                    .unwrap_or(false);
                Some((value.to_string(), is_markdown))
            } else {
                object
                    .get("language")
                    .and_then(Value::as_str)
                    .map(|value| (value.to_string(), true))
            }
        }
        Value::Array(values) => {
            let parts: Vec<_> = values
                .iter()
                .filter_map(hover_contents)
                .map(|(text, _)| text)
                .collect();
            if parts.is_empty() {
                None
            } else {
                Some((parts.join("\n\n"), true))
            }
        }
        _ => None,
    }
}

fn parse_inlay_hints(result: &Value) -> Vec<LspInlayHintResponse> {
    result
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(|hint| {
                    Some(LspInlayHintResponse {
                        position: parse_lsp_position(hint.get("position")?)?,
                        label: parse_inlay_hint_label(hint.get("label")?)?,
                        kind: hint.get("kind").and_then(Value::as_i64),
                        tooltip: completion_documentation(hint.get("tooltip")),
                        padding_left: hint
                            .get("paddingLeft")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        padding_right: hint
                            .get("paddingRight")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        text_edits: hint
                            .get("textEdits")
                            .and_then(Value::as_array)
                            .map(|edits| {
                                edits.iter().filter_map(parse_lsp_text_edit_value).collect()
                            })
                            .unwrap_or_default(),
                        data: hint.get("data").cloned(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_inlay_hint_label(value: &Value) -> Option<String> {
    match value {
        Value::String(label) => Some(label.clone()),
        Value::Array(parts) => {
            let label = parts
                .iter()
                .filter_map(|part| part.get("value").and_then(Value::as_str))
                .collect::<String>();
            (!label.is_empty()).then_some(label)
        }
        _ => None,
    }
}

/// Normalizes a `textDocument/documentSymbol` result into a unified tree.
///
/// Servers answer with either `DocumentSymbol[]` (hierarchical) or
/// `SymbolInformation[]` (flat); the two are told apart per item by the
/// presence of `location`. Conversion rules:
/// - `DocumentSymbol` keeps `name`, `detail`, `kind`, `range`,
///   `selectionRange` and recursively normalized `children`.
/// - `SymbolInformation` becomes a childless symbol whose `range` and
///   `selectionRange` are both `location.range`, with `containerName` kept as
///   a hint; Core does not guess a hierarchy from container names.
/// - `deprecated` is true for the legacy `deprecated` flag or tag `1`.
/// - Positions use `{ line, utf16Column }`. Items with a missing name, kind or
///   range are dropped; `null` or a non-array result yields an empty list.
fn parse_document_symbols(result: &Value) -> Vec<Value> {
    result
        .as_array()
        .map(|items| items.iter().filter_map(parse_document_symbol).collect())
        .unwrap_or_default()
}

fn parse_document_symbol(item: &Value) -> Option<Value> {
    let name = item.get("name").and_then(Value::as_str)?;
    let kind = item.get("kind").and_then(Value::as_i64)?;
    let tags: Vec<i64> = item
        .get("tags")
        .and_then(Value::as_array)
        .map(|tags| tags.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default();
    let deprecated = item
        .get("deprecated")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || tags.contains(&1);
    let (range, selection_range, children, container) = if let Some(location) = item.get("location")
    {
        let range = parse_lsp_range_value(location.get("range")?)?;
        (range.clone(), range, Vec::new(), item.get("containerName"))
    } else {
        let range = parse_lsp_range_value(item.get("range")?)?;
        let selection = item
            .get("selectionRange")
            .and_then(parse_lsp_range_value)
            .unwrap_or_else(|| range.clone());
        let children = item
            .get("children")
            .and_then(Value::as_array)
            .map(|children| children.iter().filter_map(parse_document_symbol).collect())
            .unwrap_or_default();
        (range, selection, children, None)
    };
    Some(json!({
        "name": name,
        "detail": item.get("detail").and_then(Value::as_str),
        "kind": kind,
        "tags": tags,
        "deprecated": deprecated,
        "range": range,
        "selectionRange": selection_range,
        "containerName": container.and_then(Value::as_str),
        "children": children
    }))
}

fn parse_folding_ranges(result: &Value) -> Vec<LspFoldingRangeResponse> {
    result
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(|range| {
                    Some(LspFoldingRangeResponse {
                        start_line: range.get("startLine")?.as_i64()?,
                        start_utf16_column: range.get("startCharacter").and_then(Value::as_i64),
                        end_line: range.get("endLine")?.as_i64()?,
                        end_utf16_column: range.get("endCharacter").and_then(Value::as_i64),
                        kind: range
                            .get("kind")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        collapsed_text: range
                            .get("collapsedText")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Accepts both `SymbolInformation[]` and `WorkspaceSymbol[]`. A location with
/// only a URI keeps a null range, because the server expects `workspaceSymbol/resolve`.
fn parse_workspace_symbols(result: &Value) -> Vec<LspWorkspaceSymbolResponse> {
    result
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(|symbol| {
                    let location = symbol.get("location")?;
                    Some(LspWorkspaceSymbolResponse {
                        name: symbol.get("name")?.as_str()?.to_string(),
                        kind: symbol.get("kind").and_then(Value::as_i64),
                        container_name: symbol
                            .get("containerName")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        uri: location.get("uri")?.as_str()?.to_string(),
                        range: location.get("range").and_then(parse_lsp_range),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Links without a range are dropped; a missing target stays `None` because the
/// server may supply it only through `documentLink/resolve`, which Core does not call.
fn parse_document_links(result: &Value) -> Vec<LspDocumentLinkResponse> {
    result
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(|link| {
                    Some(LspDocumentLinkResponse {
                        range: parse_lsp_range(link.get("range")?)?,
                        target: link
                            .get("target")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        tooltip: link
                            .get("tooltip")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_code_lenses(result: &Value) -> Vec<LspCodeLensResponse> {
    result
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(|lens| {
                    Some(LspCodeLensResponse {
                        range: parse_lsp_range(lens.get("range")?)?,
                        command: lens.get("command").and_then(parse_lsp_command),
                        data: lens.get("data").cloned(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_locations(result: &Value) -> Vec<Value> {
    let values: Vec<&Value> = if let Some(array) = result.as_array() {
        array.iter().collect()
    } else if result.is_object() {
        vec![result]
    } else {
        Vec::new()
    };
    values
        .into_iter()
        .filter_map(|location| {
            let uri = location
                .get("uri")
                .or_else(|| location.get("targetUri"))
                .and_then(Value::as_str)?;
            let range = location
                .get("range")
                .or_else(|| location.get("targetSelectionRange"))
                .or_else(|| location.get("targetRange"))
                .and_then(parse_lsp_range_value)?;
            let file_path = file_path_for_uri(uri);
            let is_read_only = file_path.is_none();
            Some(json!({
                "uri": uri,
                "filePath": file_path,
                "range": range,
                "isReadOnly": is_read_only,
                "displayPath": Value::Null
            }))
        })
        .collect()
}

fn parse_code_actions(result: &Value) -> Vec<Value> {
    let Some(values) = result.as_array() else {
        return Vec::new();
    };
    values.iter().filter_map(parse_code_action).collect()
}

fn parse_code_action(action: &Value) -> Option<Value> {
    let title = action.get("title").and_then(Value::as_str)?;
    let command = if action.get("command").and_then(Value::as_str).is_some() {
        parse_lsp_command(action)
    } else {
        action.get("command").and_then(parse_lsp_command)
    };
    Some(json!({
        "title": title,
        "kind": action.get("kind").and_then(Value::as_str),
        "isPreferred": action
            .get("isPreferred")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        "edit": action.get("edit").map(|edit| json!({
            "changes": parse_workspace_edit(edit)
        })),
        "command": command,
        "data": action.get("data").cloned().unwrap_or(Value::Null)
    }))
}

fn parse_lsp_command(value: &Value) -> Option<Value> {
    Some(json!({
        "title": value.get("title").and_then(Value::as_str).unwrap_or_default(),
        "command": value.get("command").and_then(Value::as_str)?,
        "arguments": value
            .get("arguments")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }))
}

fn parse_workspace_edit(result: &Value) -> serde_json::Map<String, Value> {
    let mut changes = serde_json::Map::new();
    if let Some(entries) = result.get("changes").and_then(Value::as_object) {
        for (uri, edits) in entries {
            let parsed = edits
                .as_array()
                .map(|edits| {
                    edits
                        .iter()
                        .filter_map(parse_lsp_text_edit_value)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            changes.insert(file_path_from_uri(uri), json!(parsed));
        }
    }
    if let Some(document_changes) = result.get("documentChanges").and_then(Value::as_array) {
        for change in document_changes {
            let Some(uri) = change
                .get("textDocument")
                .and_then(|document| document.get("uri"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let parsed = change
                .get("edits")
                .and_then(Value::as_array)
                .map(|edits| {
                    edits
                        .iter()
                        .filter_map(parse_lsp_text_edit_value)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            changes.insert(file_path_from_uri(uri), json!(parsed));
        }
    }
    changes
}

fn parse_lsp_text_edit_value(value: &Value) -> Option<Value> {
    Some(json!({
        "range": parse_lsp_range_value(value.get("range")?)?,
        "newText": value.get("newText").and_then(Value::as_str).unwrap_or_default()
    }))
}

fn parse_lsp_range_value(value: &Value) -> Option<Value> {
    Some(json!({
        "start": parse_lsp_position_value(value.get("start")?)?,
        "end": parse_lsp_position_value(value.get("end")?)?
    }))
}

fn parse_lsp_position_value(value: &Value) -> Option<Value> {
    Some(json!({
        "line": value.get("line").and_then(Value::as_i64).unwrap_or(0),
        "utf16Column": value
            .get("character")
            .or_else(|| value.get("utf16Column"))
            .and_then(Value::as_i64)
            .unwrap_or(0)
    }))
}
