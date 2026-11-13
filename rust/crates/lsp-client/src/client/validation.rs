//! URI and method validation.

use super::*;

pub(crate) fn validate_uri(value: &str) -> Result<(), CoreError> {
    if value.trim().is_empty() || value.contains('\0') {
        Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "LSP request requires a valid URI.",
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn normalize_uri_for_comparison(uri: &str) -> String {
    let serialized = url::Url::parse(uri)
        .map(|url| url.to_string())
        .unwrap_or_else(|_| uri.to_owned());
    let bytes = serialized.as_bytes();
    let mut canonical = Vec::with_capacity(bytes.len());
    let mut index = 0;
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                let decoded = (high << 4) | low;
                if decoded.is_ascii_alphanumeric() || b"-._~".contains(&decoded) {
                    canonical.push(decoded);
                } else {
                    canonical.extend_from_slice(&[b'%', HEX[high as usize], HEX[low as usize]]);
                }
                index += 3;
                continue;
            }
        }
        canonical.push(bytes[index]);
        index += 1;
    }
    // The input was UTF-8 and only ASCII escapes were replaced.
    let mut normalized = String::from_utf8(canonical).expect("URI normalization preserves UTF-8");
    const FILE_PREFIX: &str = "file:///";
    if let Some(path) = normalized.strip_prefix(FILE_PREFIX) {
        let bytes = path.as_bytes();
        if bytes.len() >= 4 && bytes[0].is_ascii_alphabetic() && &bytes[1..4] == b"%3A" {
            let drive_index = FILE_PREFIX.len();
            normalized.replace_range(drive_index + 1..drive_index + 4, ":");
        }
        let bytes = normalized.as_bytes();
        let drive_index = FILE_PREFIX.len();
        if bytes.len() > drive_index + 1
            && bytes[drive_index].is_ascii_alphabetic()
            && bytes[drive_index + 1] == b':'
        {
            normalized[drive_index..drive_index + 1].make_ascii_lowercase();
        }
    }
    normalized
}

pub(crate) fn workspace_name_from_uri(uri: &str) -> String {
    let decoded = file_path_from_uri(uri);
    decoded
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .find(|component| !component.is_empty())
        .unwrap_or("workspace")
        .to_string()
}

pub(crate) fn validate_lsp_method(method: &str) -> Result<(), CoreError> {
    match method {
        "textDocument/completion"
        | "textDocument/hover"
        | "textDocument/definition"
        | "textDocument/declaration"
        | "textDocument/typeDefinition"
        | "textDocument/implementation"
        | "textDocument/references"
        | "textDocument/rename"
        | "textDocument/formatting"
        | "textDocument/rangeFormatting"
        | "textDocument/documentSymbol"
        | "textDocument/inlayHint"
        | "textDocument/semanticTokens/full"
        | "textDocument/foldingRange"
        | "textDocument/documentLink"
        | "workspace/symbol"
        | "textDocument/codeLens"
        | "textDocument/codeAction"
        | "completionItem/resolve"
        | "codeAction/resolve"
        | "workspace/executeCommand" => Ok(()),
        _ => Err(CoreError::new(
            ErrorCode::NotSupported,
            "Unsupported LSP client request method.",
        )
        .with_details(method.to_string())),
    }
}
