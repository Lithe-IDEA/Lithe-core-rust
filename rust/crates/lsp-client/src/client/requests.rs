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
            "context": {
                "diagnostics": request
                    .diagnostics
                    .iter()
                    .map(lsp_diagnostic_json)
                    .collect::<Vec<_>>()
            }
        })),
        "completionItem/resolve" => request.completion_item.clone().ok_or_else(|| {
            CoreError::new(
                ErrorCode::InvalidRequest,
                "This LSP request requires a completion item.",
            )
        }),
        "codeAction/resolve" => request.code_action.clone().ok_or_else(|| {
            CoreError::new(
                ErrorCode::InvalidRequest,
                "This LSP request requires a code action.",
            )
        }),
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
