//! Parsing published diagnostics.

use super::*;

pub(crate) fn parse_diagnostics(value: Option<&Value>) -> Vec<LspClientDiagnostic> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(LspClientDiagnostic {
                        range: parse_lsp_range(item.get("range")?)?,
                        severity: item.get("severity").and_then(Value::as_i64),
                        message: item
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        source: item
                            .get("source")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        code: item.get("code").and_then(|code| match code {
                            Value::String(value) => Some(value.clone()),
                            Value::Number(value) => Some(value.to_string()),
                            _ => None,
                        }),
                        tags: item
                            .get("tags")
                            .and_then(Value::as_array)
                            .map(|tags| tags.iter().filter_map(Value::as_i64).collect())
                            .unwrap_or_default(),
                        related_information: parse_diagnostic_related_information(
                            item.get("relatedInformation"),
                        ),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_diagnostic_related_information(
    value: Option<&Value>,
) -> Vec<LspClientDiagnosticRelatedInformation> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let location = item.get("location")?;
                    Some(LspClientDiagnosticRelatedInformation {
                        location: LspClientDiagnosticLocation {
                            uri: location.get("uri")?.as_str()?.to_string(),
                            range: parse_lsp_range(location.get("range")?)?,
                        },
                        message: item.get("message")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn parse_lsp_range(value: &Value) -> Option<LspRangeResponse> {
    Some(LspRangeResponse {
        start: parse_lsp_position(value.get("start")?)?,
        end: parse_lsp_position(value.get("end")?)?,
    })
}

pub(crate) fn parse_lsp_position(value: &Value) -> Option<LspPositionResponse> {
    Some(LspPositionResponse {
        line: value.get("line")?.as_i64()?,
        utf16_column: value.get("character")?.as_i64()?,
    })
}
