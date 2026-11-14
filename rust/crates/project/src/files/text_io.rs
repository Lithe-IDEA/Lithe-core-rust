//! Reading, decoding, encoding and writing workspace files.

use super::*;

/// Reads one bounded, workspace-contained text file.
///
/// Without `encoding` the codec is detected from the byte order mark and
/// the bytes; with one, the bytes are decoded leniently through it.
pub fn read_file(request: FileReadRequest) -> Result<FileReadResponse, CoreError> {
    let root = existing_root(&request.root)?;
    let path = safe_relative_path(&root, &request.path)?;
    let requested = request
        .encoding
        .as_deref()
        .map(DocumentEncoding::parse)
        .transpose()?;
    let metadata = fs::metadata(&path)?;
    if metadata.len() > MAX_OPEN_FILE_SIZE {
        return Err(too_large());
    }
    let bytes = fs::read(&path)?;
    let decoded = decode_text(&bytes, requested)?;
    Ok(FileReadResponse {
        path: relative_path(&path, &root),
        text: decoded.text,
        encoding: decoded.encoding,
        has_bom: decoded.has_bom,
        lossy: decoded.lossy,
        identity: decoded.identity,
    })
}

/// Bytes a host already holds, as base64, to decode as `file.read` would.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextDecodeRequest {
    /// Standard base64 of the exact bytes.
    pub bytes: String,
    /// Stable encoding id or alias to decode with; absent means detect.
    #[serde(default)]
    pub encoding: Option<String>,
}

/// `file.read`'s answer without a path.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDecodeResponse {
    /// Decoded text without the byte order mark.
    pub text: String,
    /// Stable id of the encoding used to decode `text`.
    pub encoding: String,
    /// A leading byte order mark was present and is not part of `text`.
    pub has_bom: bool,
    /// Invalid bytes were replaced with U+FFFD (explicit `encoding` only).
    pub lossy: bool,
    /// SHA-256 (lowercase hex) of the exact bytes decoded.
    pub identity: String,
}

/// Text to encode as `file.write` would, without a file.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextEncodeRequest {
    /// Text to encode.
    pub text: String,
    /// Stable encoding id or alias to encode with; absent means UTF-8.
    #[serde(default)]
    pub encoding: Option<String>,
    /// Overrides the encoding's own byte-order-mark choice (Unicode encodings only).
    #[serde(default)]
    pub bom: Option<bool>,
}

/// `file.write`'s answer with the encoded bytes in place of a path.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextEncodeResponse {
    /// Standard base64 of the exact encoded bytes, including any byte order mark.
    pub bytes: String,
    /// Stable id of the encoding the text was encoded with.
    pub encoding: String,
    /// A byte order mark was written.
    pub has_bom: bool,
    /// SHA-256 (lowercase hex) of the encoded bytes.
    pub identity: String,
}

/// `text.decode`: the decoding of `file.read` for bytes the host read itself,
/// so a host that guards its own file access needs no temporary file.
pub fn decode_text_bytes(request: TextDecodeRequest) -> Result<TextDecodeResponse, CoreError> {
    use base64::Engine as _;
    // Base64 is 4 characters per 3 bytes: anything longer is over the limit
    // before it is decoded into memory.
    if request.bytes.len() as u64 > MAX_OPEN_FILE_SIZE.div_ceil(3) * 4 {
        return Err(too_large());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(request.bytes.as_bytes())
        .map_err(|error| {
            CoreError::new(ErrorCode::InvalidRequest, "Invalid text decode request")
                .with_details(error.to_string())
        })?;
    if bytes.len() as u64 > MAX_OPEN_FILE_SIZE {
        return Err(too_large());
    }
    let requested = request
        .encoding
        .as_deref()
        .map(DocumentEncoding::parse)
        .transpose()?;
    decode_text(&bytes, requested)
}

/// `text.encode`: the encoding of `file.write`, returning the bytes instead of
/// writing them. An unmappable character fails the same way.
pub fn encode_text(request: TextEncodeRequest) -> Result<TextEncodeResponse, CoreError> {
    use base64::Engine as _;
    let selected = request
        .encoding
        .as_deref()
        .map(DocumentEncoding::parse)
        .transpose()?
        .unwrap_or(DocumentEncoding::Utf8);
    let bytes = encoding::encode(&request.text, selected, request.bom)?;
    Ok(TextEncodeResponse {
        encoding: selected.id().to_string(),
        has_bom: writes_bom(selected, request.bom),
        identity: encoding::bytes_identity(&bytes),
        bytes: base64::engine::general_purpose::STANDARD.encode(&bytes),
    })
}

/// Decodes document bytes and refuses anything that is not plain text.
fn decode_text(
    bytes: &[u8],
    requested: Option<DocumentEncoding>,
) -> Result<TextDecodeResponse, CoreError> {
    let decoded = encoding::decode(bytes, requested)?;
    if !is_plain_text(&decoded.text) {
        return Err(CoreError::new(
            ErrorCode::ParseFailed,
            "File is not plain text",
        ));
    }
    Ok(TextDecodeResponse {
        text: decoded.text,
        encoding: decoded.encoding.id().to_string(),
        has_bom: decoded.had_bom,
        lossy: decoded.lossy,
        identity: encoding::bytes_identity(bytes),
    })
}

fn too_large() -> CoreError {
    CoreError::new(ErrorCode::ParseFailed, "File is too large to open")
        .with_details("The maximum supported file size is 32 MB")
}

/// Whether encoding with `selected` and the `bom` override writes a byte order mark.
fn writes_bom(selected: DocumentEncoding, bom: Option<bool>) -> bool {
    bom.unwrap_or(matches!(
        selected,
        DocumentEncoding::Utf8Bom | DocumentEncoding::Utf16LeBom | DocumentEncoding::Utf16BeBom
    ))
}

/// Replaces one workspace-contained file and refreshes its cached search entry.
///
/// Without `encoding`/`bom` the text is written as plain UTF-8, as before.
pub fn write_file(request: FileWriteRequest) -> Result<FileWriteResponse, CoreError> {
    let root = existing_root(&request.root)?;
    let path = writable_relative_path(&root, &request.path)?;
    let selected = request
        .encoding
        .as_deref()
        .map(DocumentEncoding::parse)
        .transpose()?
        .unwrap_or(DocumentEncoding::Utf8);
    // Encode before touching the file system so an unmappable character leaves
    // the existing file and directory tree untouched.
    let bytes = encoding::encode(&request.text, selected, request.bom)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, &bytes).map_err(|error| {
        CoreError::new(ErrorCode::PermissionDenied, "Could not write file")
            .with_details(error.to_string())
    })?;
    search_index::update_cached_file(&root, &path);
    let has_bom = writes_bom(selected, request.bom);
    Ok(FileWriteResponse {
        path: relative_path(&path, &root),
        bytes_written: bytes.len(),
        encoding: selected.id().to_string(),
        has_bom,
        identity: encoding::bytes_identity(&bytes),
    })
}
