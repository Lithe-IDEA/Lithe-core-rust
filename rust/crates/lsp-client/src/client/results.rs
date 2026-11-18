//! Parsing feature results into Core shapes.

use super::*;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::io::{self, Write};

pub(crate) fn lsp_feature_result_for_method(
    method: Option<&str>,
    result: Option<&Value>,
    open_documents: &BTreeMap<String, LspClientDocument>,
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
        Some("textDocument/signatureHelp") => Some(json!({
            "signatureHelp": parse_signature_help(result)
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
        Some("inlayHint/resolve") => parse_inlay_hint(result).map(|hint| {
            json!({
                "hint": hint
            })
        }),
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
            "actions": parse_code_actions(result, open_documents)
        })),
        Some("codeAction/resolve") => parse_code_action(result, open_documents).map(|action| {
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

const MAX_SIGNATURES: usize = 32;
const MAX_PARAMETERS: usize = 128;
const MAX_SIGNATURE_LABEL_CHARS: usize = 512;
const MAX_SIGNATURE_DOC_BYTES: usize = 2048;

const MAX_INLAY_HINTS_PER_VIEWPORT: usize = 512;
const MAX_INLAY_HINTS_PER_LINE: usize = 8;
const MAX_INLAY_HINT_LABEL_CHARS: usize = 48;
const MAX_INLAY_HINT_TOOLTIP_BYTES: usize = 2048;
const MAX_INLAY_HINT_RESOLVE_BYTES: usize = 16 * 1024;
const MAX_INLAY_HINT_TEXT_EDITS: usize = 16;

const MAX_CODE_ACTIONS: usize = 256;
const MAX_CODE_ACTION_TITLE_BYTES: usize = 512;

fn parse_signature_help(result: &Value) -> Option<Value> {
    if result.is_null() {
        return None;
    }
    let raw_signatures = result.get("signatures")?.as_array()?;
    if raw_signatures.is_empty() {
        return None;
    }
    let fallback_active_param = result.get("activeParameter").and_then(Value::as_u64);
    let requested_active = result.get("activeSignature").and_then(Value::as_u64);
    let mut signatures = Vec::with_capacity(raw_signatures.len().min(MAX_SIGNATURES));
    let mut active_signature = 0;
    let mut active_fallback = None;
    for (index, signature) in raw_signatures.iter().take(MAX_SIGNATURES).enumerate() {
        if let Some((parsed, fallback)) =
            parse_signature_information(signature, fallback_active_param)
        {
            if signatures.is_empty() || requested_active == Some(index as u64) {
                active_signature = signatures.len();
                active_fallback = fallback;
            }
            signatures.push(parsed);
        }
    }
    if signatures.is_empty() {
        return None;
    }
    if let Some(active_parameter) = active_fallback {
        signatures[active_signature]["activeParameter"] = json!(active_parameter);
    }
    let active_parameter = signatures[active_signature]
        .get("activeParameter")
        .cloned()
        .unwrap_or(Value::Null);
    Some(json!({
        "signatures": signatures,
        "activeSignature": active_signature,
        "activeParameter": active_parameter
    }))
}

#[derive(Clone, Copy)]
struct LabelBoundary {
    raw_u16: usize,
    norm_byte: usize,
    norm_u16: usize,
}

fn normalize_single_line_with_boundaries(
    raw: &str,
    max_chars: usize,
) -> (String, Vec<LabelBoundary>, bool) {
    let mut normalized = String::new();
    let mut boundaries = Vec::new();
    let mut chars = raw.char_indices().peekable();
    let mut raw_u16 = 0_usize;
    let mut norm_u16 = 0_usize;
    let mut emitted_chars = 0_usize;
    let truncated = raw.chars().nth(max_chars).is_some();
    let char_limit = if truncated {
        max_chars.saturating_sub(1)
    } else {
        max_chars
    };

    while let Some((_, ch)) = chars.next() {
        if emitted_chars == char_limit {
            break;
        }
        boundaries.push(LabelBoundary {
            raw_u16,
            norm_byte: normalized.len(),
            norm_u16,
        });
        if ch == '\r' && chars.peek().is_some_and(|(_, next)| *next == '\n') {
            raw_u16 += 1;
            let _ = chars.next();
            boundaries.push(LabelBoundary {
                raw_u16,
                norm_byte: normalized.len(),
                norm_u16,
            });
            raw_u16 += 1;
            normalized.push(' ');
            norm_u16 += 1;
            emitted_chars += 1;
            continue;
        }
        raw_u16 += ch.len_utf16();
        let out_ch = if ch == '\r' || ch == '\n' || ch == '\t' || ch.is_control() {
            ' '
        } else {
            ch
        };
        normalized.push(out_ch);
        norm_u16 += out_ch.len_utf16();
        emitted_chars += 1;
    }
    boundaries.push(LabelBoundary {
        raw_u16,
        norm_byte: normalized.len(),
        norm_u16,
    });
    if truncated {
        normalized.push('…');
    }
    (normalized, boundaries, truncated)
}

fn parse_signature_information(
    signature: &Value,
    fallback_active_param: Option<u64>,
) -> Option<(Value, Option<usize>)> {
    let raw_label = signature.get("label")?.as_str()?;
    let (label, boundaries, label_truncated) =
        normalize_single_line_with_boundaries(raw_label, MAX_SIGNATURE_LABEL_CHARS);
    if label.trim().is_empty() {
        return None;
    }
    let (documentation, is_markdown) =
        bounded_documentation(signature.get("documentation"), MAX_SIGNATURE_DOC_BYTES);
    let active_field = signature.get("activeParameter");
    let requested = match active_field {
        Some(value) => value.as_u64(),
        None => fallback_active_param,
    };
    let mut parameters = Vec::new();
    let mut selected = 0;
    let mut search_from_byte = 0_usize;
    if let Some(raw_params) = signature.get("parameters").and_then(Value::as_array) {
        for (index, param) in raw_params.iter().take(MAX_PARAMETERS).enumerate() {
            if let Some(parsed) = parse_parameter_information(
                param,
                &label,
                &boundaries,
                label_truncated,
                &mut search_from_byte,
            ) {
                if requested == Some(index as u64) {
                    selected = parameters.len();
                }
                parameters.push(parsed);
            }
        }
    }
    let has_active_parameter = !parameters.is_empty() && !active_field.is_some_and(Value::is_null);
    // The legacy top-level index only belongs to the active signature.
    let default_parameter = if active_field.is_none() { 0 } else { selected };
    let active_parameter = has_active_parameter.then_some(default_parameter);
    let fallback_parameter =
        (has_active_parameter && active_field.is_none() && fallback_active_param.is_some())
            .then_some(selected);
    Some((
        json!({
            "label": label,
            "documentation": documentation,
            "isMarkdown": is_markdown,
            "parameters": parameters,
            "activeParameter": active_parameter
        }),
        fallback_parameter,
    ))
}

fn parse_parameter_information(
    param: &Value,
    signature_label: &str,
    boundaries: &[LabelBoundary],
    label_truncated: bool,
    search_from_byte: &mut usize,
) -> Option<Value> {
    let label_field = param.get("label")?;
    let (param_label, offsets) = match label_field {
        Value::Array(pair) if pair.len() == 2 => {
            let start_u16 = pair[0].as_u64()? as usize;
            let end_u16 = pair[1].as_u64()? as usize;
            if start_u16 > end_u16 {
                return None;
            }
            let last = boundaries.last()?.raw_u16;
            let boundary = |position| {
                boundaries
                    .binary_search_by_key(&position, |b| b.raw_u16)
                    .ok()
                    .map(|index| boundaries[index])
            };
            let start = boundary(start_u16.min(last));
            let end = boundary(end_u16.min(last));
            if !label_truncated && end_u16 > last {
                return None;
            }
            let (start, end) = (start?, end?);
            let start_b = start.norm_byte;
            let end_b = end.norm_byte;
            if start_b < end_b {
                *search_from_byte = end_b;
                (
                    signature_label[start_b..end_b].to_string(),
                    Some((start.norm_u16, end.norm_u16, start_b, end_b)),
                )
            } else {
                (String::new(), None)
            }
        }
        Value::String(raw_sub) => {
            let (norm_sub, _, _) = normalize_single_line_with_boundaries(raw_sub, 128);
            if norm_sub.is_empty() {
                return None;
            }
            let found_byte = signature_label
                .get(*search_from_byte..)
                .and_then(|tail| tail.find(&norm_sub).map(|rel| *search_from_byte + rel))
                .or_else(|| signature_label.find(&norm_sub));
            if let Some(start_b) = found_byte {
                let end_b = start_b + norm_sub.len();
                *search_from_byte = end_b;
                let start_u16 = signature_label[..start_b].encode_utf16().count();
                let end_u16 = start_u16 + signature_label[start_b..end_b].encode_utf16().count();
                (norm_sub, Some((start_u16, end_u16, start_b, end_b)))
            } else {
                (norm_sub, None)
            }
        }
        _ => return None,
    };
    let (documentation, is_markdown) =
        bounded_documentation(param.get("documentation"), MAX_SIGNATURE_DOC_BYTES);
    let (utf16_start, utf16_end, byte_start, byte_end) = match offsets {
        Some((us, ue, bs, be)) => (Some(us), Some(ue), Some(bs), Some(be)),
        None => (None, None, None, None),
    };
    Some(json!({
        "label": param_label,
        "utf16Start": utf16_start,
        "utf16End": utf16_end,
        "byteStart": byte_start,
        "byteEnd": byte_end,
        "documentation": documentation,
        "isMarkdown": is_markdown
    }))
}

fn bounded_documentation(value: Option<&Value>, max_bytes: usize) -> (Option<String>, bool) {
    let Some(value) = value else {
        return (None, false);
    };
    let (raw, is_markdown) = match value {
        Value::String(text) => (text.as_str(), false),
        Value::Object(object) => {
            let Some(text) = object.get("value").and_then(Value::as_str) else {
                return (None, false);
            };
            let is_markdown = object
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind == "markdown");
            (text, is_markdown)
        }
        _ => return (None, false),
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return (None, false);
    }
    (Some(truncate_utf8_bytes(trimmed, max_bytes)), is_markdown)
}

pub(super) fn truncate_utf8_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let ellipsis = '…';
    let budget = max_bytes.saturating_sub(ellipsis.len_utf8());
    let mut end = 0;
    for (idx, ch) in text.char_indices() {
        if idx + ch.len_utf8() > budget {
            break;
        }
        end = idx + ch.len_utf8();
    }
    let mut out = text[..end].to_string();
    out.push(ellipsis);
    out
}

fn parse_inlay_hints(result: &Value) -> Vec<LspInlayHintResponse> {
    let Some(values) = result.as_array() else {
        return Vec::new();
    };
    let mut hints: Vec<_> = values
        .iter()
        .filter_map(|hint| {
            let position = parse_lsp_position(hint.get("position")?)?;
            (position.line >= 0 && position.utf16_column >= 0).then_some((position, hint))
        })
        .collect();
    hints.sort_by_key(|(position, _)| (position.line, position.utf16_column));
    let mut culled = Vec::with_capacity(hints.len().min(MAX_INLAY_HINTS_PER_VIEWPORT));
    let mut current_line: Option<i64> = None;
    let mut line_count = 0_usize;
    for (position, raw_hint) in hints {
        if current_line != Some(position.line) {
            current_line = Some(position.line);
            line_count = 0;
        }
        if line_count >= MAX_INLAY_HINTS_PER_LINE {
            continue;
        }
        let Some(hint) = parse_inlay_hint(raw_hint) else {
            continue;
        };
        line_count += 1;
        culled.push(hint);
        if culled.len() >= MAX_INLAY_HINTS_PER_VIEWPORT {
            break;
        }
    }
    culled
}

fn parse_inlay_hint(hint: &Value) -> Option<LspInlayHintResponse> {
    let raw_label = parse_inlay_hint_label(hint.get("label")?)?;
    let (label, _, _) =
        normalize_single_line_with_boundaries(&raw_label, MAX_INLAY_HINT_LABEL_CHARS);
    if label.trim().is_empty() {
        return None;
    }
    let tooltip = completion_documentation(hint.get("tooltip"))
        .map(|text| truncate_utf8_bytes(text.trim(), MAX_INLAY_HINT_TOOLTIP_BYTES))
        .filter(|text| !text.is_empty());
    let resolve_payload = bounded_inlay_resolve_payload(hint);
    let position = parse_lsp_position(hint.get("position")?)?;
    if position.line < 0 || position.utf16_column < 0 {
        return None;
    }
    let text_edits = hint
        .get("textEdits")
        .and_then(Value::as_array)
        .filter(|edits| edits.len() <= MAX_INLAY_HINT_TEXT_EDITS)
        .and_then(|edits| {
            let mut bytes = 0;
            edits
                .iter()
                .map(|edit| {
                    parse_bounded_text_edit(edit, &mut bytes, MAX_INLAY_HINT_RESOLVE_BYTES).ok()
                })
                .collect::<Option<Vec<_>>>()
        })
        .unwrap_or_default();
    Some(LspInlayHintResponse {
        position,
        label,
        kind: hint.get("kind").and_then(Value::as_i64),
        tooltip,
        padding_left: hint
            .get("paddingLeft")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        padding_right: hint
            .get("paddingRight")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        text_edits,
        data: resolve_payload
            .as_ref()
            .and_then(|payload| payload.get("data"))
            .filter(|value| !value.is_null())
            .cloned(),
        resolve_payload,
    })
}

fn parse_inlay_hint_label(value: &Value) -> Option<Cow<'_, str>> {
    match value {
        Value::String(label) => Some(Cow::Borrowed(label)),
        Value::Array(parts) => {
            let label = parts
                .iter()
                .filter_map(|part| part.get("value").and_then(Value::as_str))
                .flat_map(str::chars)
                .take(MAX_INLAY_HINT_LABEL_CHARS + 1)
                .collect::<String>();
            (!label.is_empty()).then_some(Cow::Owned(label))
        }
        _ => None,
    }
}

fn bounded_inlay_resolve_payload(hint: &Value) -> Option<Value> {
    struct ByteBudget(usize);
    impl Write for ByteBudget {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self
                .0
                .checked_sub(bytes.len())
                .ok_or(io::ErrorKind::FileTooLarge)?;
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(ByteBudget(MAX_INLAY_HINT_RESOLVE_BYTES), hint).ok()?;
    Some(hint.clone())
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

fn parse_code_actions(
    result: &Value,
    open_documents: &BTreeMap<String, LspClientDocument>,
) -> Vec<Value> {
    let Some(values) = result.as_array() else {
        return Vec::new();
    };
    let mut actions: Vec<Value> = values
        .iter()
        .filter_map(|action| parse_code_action(action, open_documents))
        .collect();
    actions.sort_by_key(|action| {
        let preferred = action
            .get("isPreferred")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let group_rank = match action.get("group").and_then(Value::as_str) {
            Some("quickfix") => 0_u8,
            Some("refactor") => 1_u8,
            Some("source") => 2_u8,
            _ => 3_u8,
        };
        (!preferred, group_rank)
    });
    actions.truncate(MAX_CODE_ACTIONS);
    actions
}

fn code_action_group(kind: Option<&str>) -> &'static str {
    let Some(kind) = kind else {
        return "other";
    };
    if kind == "quickfix" || kind.starts_with("quickfix.") {
        "quickfix"
    } else if kind == "refactor" || kind.starts_with("refactor.") {
        "refactor"
    } else if kind == "source" || kind.starts_with("source.") {
        "source"
    } else {
        "other"
    }
}

fn parse_code_action(
    action: &Value,
    open_documents: &BTreeMap<String, LspClientDocument>,
) -> Option<Value> {
    if action.get("disabled").is_some_and(|value| !value.is_null()) {
        return None;
    }
    let raw_title = action.get("title").and_then(Value::as_str)?;
    let single_line_title: String = raw_title
        .chars()
        .map(|ch| {
            if ch == '\r' || ch == '\n' || ch == '\t' || ch.is_control() {
                ' '
            } else {
                ch
            }
        })
        .collect();
    let trimmed_title = single_line_title.trim();
    if trimmed_title.is_empty() {
        return None;
    }
    let title = truncate_utf8_bytes(trimmed_title, MAX_CODE_ACTION_TITLE_BYTES);
    let kind = action.get("kind").and_then(Value::as_str);
    let group = code_action_group(kind);
    let command = if action.get("command").and_then(Value::as_str).is_some() {
        parse_lsp_command(action)
    } else {
        action.get("command").and_then(parse_lsp_command)
    };
    // A code action must be applied as a unit. Never expose only the supported
    // subset of a malformed or partly unsupported WorkspaceEdit.
    let edit = match action.get("edit").filter(|edit| !edit.is_null()) {
        Some(edit) => Some(WorkspaceEdit::parse(edit, open_documents).ok()?),
        None => None,
    };
    Some(json!({
        "title": title,
        "kind": kind,
        "group": group,
        "isPreferred": action
            .get("isPreferred")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        "edit": edit,
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

fn append_file_edits(
    changes: &mut serde_json::Map<String, Value>,
    path: String,
    mut edits: Vec<Value>,
) {
    if let Some(Value::Array(existing)) = changes.get_mut(&path) {
        existing.append(&mut edits);
    } else {
        changes.insert(path, Value::Array(edits));
    }
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
            append_file_edits(&mut changes, file_path_from_uri(uri), parsed);
        }
    }
    if let Some(document_changes) = result.get("documentChanges").and_then(Value::as_array) {
        for change in document_changes {
            if change.get("kind").is_some() {
                continue;
            }
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
            append_file_edits(&mut changes, file_path_from_uri(uri), parsed);
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
