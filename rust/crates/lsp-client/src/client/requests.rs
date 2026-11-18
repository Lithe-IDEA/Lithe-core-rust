//! Parameters of feature requests and the LSP shapes they use.

use super::*;

pub(crate) fn feature_request_params(request: &ClientFeatureRequest) -> Result<Value, CoreError> {
    let text_document = json!({ "uri": request.uri });
    match request.method.as_str() {
        "textDocument/completion" => {
            let context = request.completion_context.as_ref();
            let kind = context
                .and_then(|v| v.get("triggerKind"))
                .and_then(Value::as_u64)
                .unwrap_or(1);
            if !(1..=3).contains(&kind)
                || context.is_some_and(|v| {
                    !v.is_object() || v.get("triggerKind").and_then(Value::as_u64).is_none()
                })
            {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Invalid completion trigger kind",
                ));
            }
            let mut trigger = json!({"triggerKind": kind});
            if kind == 2 {
                let character = context
                    .and_then(|v| v.get("triggerCharacter"))
                    .and_then(Value::as_str)
                    .filter(|v| v.chars().count() == 1)
                    .ok_or_else(|| {
                        CoreError::new(
                            ErrorCode::InvalidRequest,
                            "Completion trigger character is required",
                        )
                    })?;
                trigger["triggerCharacter"] = json!(character);
            }
            Ok(json!({"textDocument": text_document,
                "position": lsp_position_json(required_position(request)?), "context": trigger}))
        }
        "textDocument/hover"
        | "textDocument/definition"
        | "textDocument/declaration"
        | "textDocument/typeDefinition"
        | "textDocument/implementation" => Ok(json!({
            "textDocument": text_document,
            "position": lsp_position_json(required_position(request)?)
        })),
        "textDocument/signatureHelp" => Ok(json!({
            "textDocument": text_document,
            "position": lsp_position_json(required_position(request)?),
            "context": signature_help_context_json(request.signature_help_context.as_ref())?
        })),
        "textDocument/references" => Ok(json!({
            "textDocument": text_document,
            "position": lsp_position_json(required_position(request)?),
            "context": { "includeDeclaration": request.include_declaration.unwrap_or(true) }
        })),
        "textDocument/rename" => Ok(json!({
            "textDocument": text_document,
            "position": lsp_position_json(required_position(request)?),
            "newName": request.new_name.clone().unwrap_or_default()
        })),
        "textDocument/formatting" => Ok(json!({
            "textDocument": text_document,
            "options": formatting_options_json(request.formatting_options.as_ref())?
        })),
        "textDocument/rangeFormatting" => Ok(json!({
            "textDocument": text_document,
            "range": lsp_range_json(required_range(request)?),
            "options": formatting_options_json(request.formatting_options.as_ref())?
        })),
        "textDocument/inlayHint" => Ok(json!({
            "textDocument": text_document,
            "range": lsp_range_json(required_range(request)?)
        })),
        "textDocument/semanticTokens/full"
        | "textDocument/documentSymbol"
        | "textDocument/foldingRange"
        | "textDocument/documentLink"
        | "textDocument/codeLens" => Ok(json!({
            "textDocument": text_document
        })),
        "workspace/symbol" => Ok(json!({
            "query": request.query.clone().unwrap_or_default()
        })),
        "textDocument/codeAction" => Ok(json!({
            "textDocument": text_document,
            "range": lsp_range_json(required_range(request)?),
            "context": code_action_context_json(
                &request.diagnostics,
                request.code_action_context.as_ref(),
            )?
        })),
        "completionItem/resolve" => request.completion_item.clone().ok_or_else(|| {
            CoreError::new(
                ErrorCode::InvalidRequest,
                "This LSP request requires a completion item.",
            )
        }),
        "codeAction/resolve" => code_action_resolve_json(request.code_action.as_ref()),
        "inlayHint/resolve" => inlay_hint_resolve_json(request.inlay_hint.as_ref()),
        "workspace/executeCommand" => request.command.clone().ok_or_else(|| {
            CoreError::new(
                ErrorCode::InvalidRequest,
                "This LSP request requires a command.",
            )
        }),
        _ => Ok(json!({ "textDocument": text_document })),
    }
}

fn required_position(request: &ClientFeatureRequest) -> Result<LspPosition, CoreError> {
    request.position.ok_or_else(|| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "This LSP request requires a text document position.",
        )
    })
}

/// Builds the LSP `FormattingOptions` for a formatting request.
///
/// Without caller options the historical defaults apply. Caller options are
/// merged over them, so a host passing only `tabSize` keeps the other
/// defaults. Values must be booleans, integers or strings as LSP requires,
/// and `tabSize` must be a positive integer.
fn formatting_options_json(supplied: Option<&Value>) -> Result<Value, CoreError> {
    let mut options = json!({
        "tabSize": 4,
        "insertSpaces": true,
        "trimTrailingWhitespace": true,
        "insertFinalNewline": true,
        "trimFinalNewlines": true
    });
    let Some(supplied) = supplied.filter(|value| !value.is_null()) else {
        return Ok(options);
    };
    let invalid = || CoreError::new(ErrorCode::InvalidRequest, "Invalid formatting options");
    let supplied = supplied.as_object().ok_or_else(invalid)?;
    for (key, value) in supplied {
        let valid = match key.as_str() {
            "tabSize" => value
                .as_u64()
                .is_some_and(|size| (1..=1024).contains(&size)),
            "insertSpaces"
            | "trimTrailingWhitespace"
            | "insertFinalNewline"
            | "trimFinalNewlines" => value.is_boolean(),
            _ => value.is_boolean() || value.is_string() || value.is_i64() || value.is_u64(),
        };
        if !valid {
            return Err(invalid().with_details(key.clone()));
        }
        options[key] = value.clone();
    }
    Ok(options)
}

fn required_range(request: &ClientFeatureRequest) -> Result<LspRange, CoreError> {
    request.range.ok_or_else(|| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "This LSP request requires a text document range.",
        )
    })
}

fn lsp_position_json(position: LspPosition) -> Value {
    json!({
        "line": position.line,
        "character": position.utf16_column
    })
}

fn lsp_range_json(range: LspRange) -> Value {
    json!({
        "start": lsp_position_json(range.start),
        "end": lsp_position_json(range.end)
    })
}

fn lsp_diagnostic_json(diagnostic: &LspClientDiagnostic) -> Value {
    json!({
        "range": lsp_range_response_json(diagnostic.range),
        "severity": diagnostic.severity,
        "message": diagnostic.message,
        "source": diagnostic.source,
        "code": diagnostic.code,
        "tags": diagnostic.tags,
        "relatedInformation": diagnostic.related_information.iter().map(|information| {
            json!({
                "location": {
                    "uri": information.location.uri,
                    "range": lsp_range_response_json(information.location.range)
                },
                "message": information.message
            })
        }).collect::<Vec<_>>()
    })
}

fn lsp_range_response_json(range: LspRangeResponse) -> Value {
    json!({
        "start": {
            "line": range.start.line,
            "character": range.start.utf16_column
        },
        "end": {
            "line": range.end.line,
            "character": range.end.utf16_column
        }
    })
}

fn code_action_context_json(
    diagnostics: &[LspClientDiagnostic],
    supplied: Option<&Value>,
) -> Result<Value, CoreError> {
    let mut context = json!({
        "diagnostics": diagnostics.iter().map(lsp_diagnostic_json).collect::<Vec<_>>()
    });
    let Some(supplied) = supplied.filter(|value| !value.is_null()) else {
        return Ok(context);
    };
    let invalid = || CoreError::new(ErrorCode::InvalidRequest, "Invalid code action context");
    let object = supplied.as_object().ok_or_else(invalid)?;
    for (key, value) in object {
        match key.as_str() {
            "only" => {
                let items = value.as_array().ok_or_else(invalid)?;
                if items.is_empty() || items.len() > 32 {
                    return Err(invalid());
                }
                let mut kinds = Vec::with_capacity(items.len());
                for item in items {
                    let kind = item
                        .as_str()
                        .filter(|kind| !kind.trim().is_empty() && kind.len() <= 128)
                        .ok_or_else(invalid)?;
                    kinds.push(kind);
                }
                context["only"] = json!(kinds);
            }
            "triggerKind" => {
                let kind = value.as_u64().ok_or_else(invalid)?;
                if !matches!(kind, 1 | 2) {
                    return Err(invalid());
                }
                context["triggerKind"] = json!(kind);
            }
            _ => return Err(invalid().with_details(key.clone())),
        }
    }
    Ok(context)
}

fn signature_help_context_json(supplied: Option<&Value>) -> Result<Value, CoreError> {
    let Some(supplied) = supplied.filter(|value| !value.is_null()) else {
        return Ok(json!({
            "triggerKind": 1,
            "isRetrigger": false
        }));
    };
    let invalid = || CoreError::new(ErrorCode::InvalidRequest, "Invalid signature help context");
    let object = supplied.as_object().ok_or_else(invalid)?;
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "triggerKind" | "triggerCharacter" | "isRetrigger" | "activeSignatureHelp"
        ) {
            return Err(invalid().with_details(key.clone()));
        }
    }
    let kind = object
        .get("triggerKind")
        .and_then(Value::as_u64)
        .filter(|kind| (1..=3).contains(kind))
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::InvalidRequest,
                "Invalid signature help trigger kind",
            )
        })?;
    let is_retrigger = match object.get("isRetrigger") {
        Some(Value::Bool(value)) => *value,
        None => false,
        Some(_) => return Err(invalid()),
    };
    let mut context = json!({
        "triggerKind": kind,
        "isRetrigger": is_retrigger
    });
    if let Some(character_value) = object.get("triggerCharacter").filter(|v| !v.is_null()) {
        let character = character_value
            .as_str()
            .filter(|text| text.chars().count() == 1)
            .ok_or_else(|| {
                CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Signature help trigger character must be a single character",
                )
            })?;
        context["triggerCharacter"] = json!(character);
    } else if kind == 2 {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Signature help trigger character is required",
        ));
    }
    if let Some(active) = object.get("activeSignatureHelp").filter(|v| !v.is_null()) {
        context["activeSignatureHelp"] =
            sanitize_active_signature_help(active).ok_or_else(invalid)?;
    }
    Ok(context)
}

fn sanitize_active_signature_help(active: &Value) -> Option<Value> {
    let object = active.as_object()?;
    let signatures = object.get("signatures")?.as_array()?;
    let mut mapped_signatures = Vec::with_capacity(signatures.len().min(32));
    for signature in signatures.iter().take(32) {
        let sig_obj = signature.as_object()?;
        let label = sig_obj.get("label")?.as_str()?;
        let mut mapped_sig = json!({ "label": label });
        if let Some(doc) = sig_obj.get("documentation").filter(|v| !v.is_null()) {
            if let Some(text) = doc.as_str() {
                let is_markdown = sig_obj
                    .get("isMarkdown")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if is_markdown {
                    mapped_sig["documentation"] = json!({ "kind": "markdown", "value": text });
                } else {
                    mapped_sig["documentation"] = json!(text);
                }
            } else if doc.is_object() {
                mapped_sig["documentation"] = doc.clone();
            }
        }
        if let Some(parameters) = sig_obj.get("parameters").and_then(Value::as_array) {
            let mut mapped_params = Vec::with_capacity(parameters.len().min(128));
            for param in parameters.iter().take(128) {
                let Some(param_obj) = param.as_object() else {
                    continue;
                };
                let label_value = if let (Some(start), Some(end)) = (
                    param_obj.get("utf16Start").and_then(Value::as_u64),
                    param_obj.get("utf16End").and_then(Value::as_u64),
                ) {
                    json!([start, end])
                } else if let Some(label_value) = param_obj.get("label") {
                    if label_value.is_string() || label_value.is_array() {
                        label_value.clone()
                    } else {
                        continue;
                    }
                } else {
                    continue;
                };
                let mut mapped_param = json!({ "label": label_value });
                if let Some(doc) = param_obj.get("documentation").filter(|v| !v.is_null()) {
                    if let Some(text) = doc.as_str() {
                        let is_markdown = param_obj
                            .get("isMarkdown")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        if is_markdown {
                            mapped_param["documentation"] =
                                json!({ "kind": "markdown", "value": text });
                        } else {
                            mapped_param["documentation"] = json!(text);
                        }
                    } else if doc.is_object() {
                        mapped_param["documentation"] = doc.clone();
                    }
                }
                mapped_params.push(mapped_param);
            }
            mapped_sig["parameters"] = json!(mapped_params);
        }
        if let Some(active_param) = sig_obj.get("activeParameter").and_then(Value::as_u64) {
            mapped_sig["activeParameter"] = json!(active_param);
        }
        mapped_signatures.push(mapped_sig);
    }
    let mut out = json!({ "signatures": mapped_signatures });
    if let Some(active_sig) = object.get("activeSignature").and_then(Value::as_u64) {
        out["activeSignature"] = json!(active_sig);
    }
    if let Some(active_param) = object.get("activeParameter").and_then(Value::as_u64) {
        out["activeParameter"] = json!(active_param);
    }
    Some(out)
}

fn code_action_resolve_json(action: Option<&Value>) -> Result<Value, CoreError> {
    let action = action.ok_or_else(|| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "This LSP request requires a code action.",
        )
    })?;
    let object = action.as_object().ok_or_else(|| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Code action payload must be an object.",
        )
    })?;
    if object.get("title").and_then(Value::as_str).is_none() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Code action payload requires a title.",
        ));
    }
    let mut cleaned = serde_json::Map::new();
    for (key, value) in object {
        if key == "group" {
            continue;
        }
        if value.is_null()
            && matches!(
                key.as_str(),
                "kind" | "edit" | "command" | "data" | "diagnostics" | "disabled"
            )
        {
            continue;
        }
        cleaned.insert(key.clone(), value.clone());
    }
    Ok(Value::Object(cleaned))
}

fn inlay_hint_resolve_json(hint: Option<&Value>) -> Result<Value, CoreError> {
    let hint = hint.ok_or_else(|| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "This LSP request requires an inlay hint.",
        )
    })?;
    // A normalized hint's display label may be shortened or assembled from
    // parts. Resolve must use the server's original position, label and data.
    let hint = hint.get("resolvePayload").unwrap_or(hint);
    let object = hint.as_object().ok_or_else(|| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Inlay hint payload must be an object.",
        )
    })?;
    let position = object
        .get("position")
        .and_then(lsp_wire_position_from_any)
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::InvalidRequest,
                "Inlay hint payload requires a position.",
            )
        })?;
    let label = object.get("label").ok_or_else(|| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Inlay hint payload requires a label.",
        )
    })?;
    if !label.is_string() && !label.is_array() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Inlay hint label must be a string or label part array.",
        ));
    }
    let mut mapped = serde_json::Map::new();
    mapped.insert("position".to_string(), position);
    mapped.insert("label".to_string(), label.clone());
    if let Some(kind) = object.get("kind").and_then(Value::as_i64) {
        mapped.insert("kind".to_string(), json!(kind));
    }
    if let Some(padding_left) = object.get("paddingLeft").and_then(Value::as_bool) {
        mapped.insert("paddingLeft".to_string(), json!(padding_left));
    }
    if let Some(padding_right) = object.get("paddingRight").and_then(Value::as_bool) {
        mapped.insert("paddingRight".to_string(), json!(padding_right));
    }
    if let Some(tooltip) = object.get("tooltip").filter(|v| !v.is_null()) {
        mapped.insert("tooltip".to_string(), tooltip.clone());
    }
    if let Some(edits) = object.get("textEdits").and_then(Value::as_array) {
        let converted: Vec<Value> = edits
            .iter()
            .filter_map(lsp_wire_text_edit_from_any)
            .collect();
        if !converted.is_empty() {
            mapped.insert("textEdits".to_string(), json!(converted));
        }
    }
    if let Some(data) = object.get("data").filter(|v| !v.is_null()) {
        mapped.insert("data".to_string(), data.clone());
    }
    Ok(Value::Object(mapped))
}

fn lsp_wire_position_from_any(value: &Value) -> Option<Value> {
    let line = value.get("line")?.as_i64()?;
    let character = value
        .get("character")
        .or_else(|| value.get("utf16Column"))?
        .as_i64()?;
    Some(json!({ "line": line, "character": character }))
}

fn lsp_wire_text_edit_from_any(value: &Value) -> Option<Value> {
    let range = value.get("range")?;
    let start = lsp_wire_position_from_any(range.get("start")?)?;
    let end = lsp_wire_position_from_any(range.get("end")?)?;
    let new_text = value.get("newText")?.as_str()?;
    Some(json!({
        "range": { "start": start, "end": end },
        "newText": new_text
    }))
}
