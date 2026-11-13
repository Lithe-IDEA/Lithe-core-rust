//! Host failure codes, bounded details and refusals.

use super::*;

/// The JSON-RPC error code of a host-service code. Each code has its own
/// number, so a backend can branch on the number alone: codes with a
/// standard meaning keep the JSON-RPC or LSP number, and the rest take
/// numbers from the server-defined range, after -32000, which backends use
/// for their own failures.
pub fn host_rpc_code(code: &str) -> i64 {
    match code {
        codes::HOST_UNAVAILABLE => -32601,
        codes::HOST_INVALID_PARAMS => -32602,
        codes::HOST_CANCELLED => -32800,
        codes::HOST_PERMISSION_DENIED => -32010,
        codes::HOST_CONTEXT_EXPIRED => -32011,
        codes::HOST_AUTHORITY_UNSUPPORTED => -32012,
        codes::HOST_VERSION_CONFLICT => -32013,
        codes::HOST_LIMIT_EXCEEDED => -32014,
        codes::HOST_TIMEOUT => -32015,
        codes::HOST_NOT_FOUND => -32016,
        codes::HOST_READ_ONLY => -32017,
        codes::HOST_UNSUPPORTED_OPTION => -32018,
        codes::HOST_PRECONDITION_FAILED => -32019,
        codes::HOST_BUSY => -32020,
        codes::HOST_RATE_LIMITED => -32021,
        codes::HOST_LANGUAGE_SERVER_UNAVAILABLE => -32022,
        codes::HOST_LANGUAGE_SERVER_ERROR => -32023,
        codes::HOST_METHOD_NOT_DECLARED => -32024,
        codes::HOST_TOOL_UNRESOLVED => -32025,
        codes::HOST_FOREIGN_CONTRIBUTION => -32026,
        codes::NOT_DECLARED => -32027,
        codes::NOT_EXPORTED => -32028,
        codes::PROVIDER_DISABLED => -32029,
        codes::VERSION_MISMATCH => -32030,
        codes::CALL_CYCLE => -32065,
        codes::DEPTH_EXCEEDED => -32066,
        codes::TOO_MANY_CALLS => -32067,
        codes::PAYLOAD_TOO_LARGE => -32068,
        codes::PROVIDER_UNAVAILABLE => -32069,
        // A relayed call's own failure is not a host failure: the caller
        // must tell the provider's error from Core's by the number alone.
        codes::FAILED => -32070,
        codes::INVALID_RESULT => -32071,
        _ => -32603,
    }
}

/// The `details` of a host failure a backend receives in
/// `error.data.details`: an object of at most `MAX_HOST_DETAILS_BYTES`
/// serialized, such as the current version of a `versionConflict` or the
/// index of the edit a range error names. Anything else is dropped, so a
/// host cannot inflate the reply or send a value a backend must not parse.
pub fn bounded_host_details(details: Option<&Value>) -> Option<Value> {
    let details = details?;
    if !details.is_object() {
        return None;
    }
    let size = serde_json::to_vec(details).map(|bytes| bytes.len()).ok()?;
    (size <= MAX_HOST_DETAILS_BYTES).then(|| details.clone())
}

/// The code and JSON-RPC error code a backend receives when the
/// host failed a forwarded request. Only the codes a host may report pass
/// through; anything else, including none, is `plugin.host.failed`, so a
/// host cannot impersonate Core's own refusals of other kinds.
pub fn host_failure(code: Option<&str>) -> (&'static str, i64) {
    let code = match code {
        Some(codes::HOST_UNAVAILABLE) => codes::HOST_UNAVAILABLE,
        Some(codes::HOST_PERMISSION_DENIED) => codes::HOST_PERMISSION_DENIED,
        Some(codes::HOST_INVALID_PARAMS) => codes::HOST_INVALID_PARAMS,
        Some(codes::HOST_CONTEXT_EXPIRED) => codes::HOST_CONTEXT_EXPIRED,
        Some(codes::HOST_AUTHORITY_UNSUPPORTED) => codes::HOST_AUTHORITY_UNSUPPORTED,
        Some(codes::HOST_VERSION_CONFLICT) => codes::HOST_VERSION_CONFLICT,
        Some(codes::HOST_LIMIT_EXCEEDED) => codes::HOST_LIMIT_EXCEEDED,
        Some(codes::HOST_CANCELLED) => codes::HOST_CANCELLED,
        Some(codes::HOST_TIMEOUT) => codes::HOST_TIMEOUT,
        Some(codes::HOST_NOT_FOUND) => codes::HOST_NOT_FOUND,
        Some(codes::HOST_READ_ONLY) => codes::HOST_READ_ONLY,
        Some(codes::HOST_UNSUPPORTED_OPTION) => codes::HOST_UNSUPPORTED_OPTION,
        Some(codes::HOST_PRECONDITION_FAILED) => codes::HOST_PRECONDITION_FAILED,
        Some(codes::HOST_BUSY) => codes::HOST_BUSY,
        Some(codes::HOST_RATE_LIMITED) => codes::HOST_RATE_LIMITED,
        Some(codes::HOST_LANGUAGE_SERVER_UNAVAILABLE) => codes::HOST_LANGUAGE_SERVER_UNAVAILABLE,
        Some(codes::HOST_LANGUAGE_SERVER_ERROR) => codes::HOST_LANGUAGE_SERVER_ERROR,
        Some(codes::HOST_METHOD_NOT_DECLARED) => codes::HOST_METHOD_NOT_DECLARED,
        Some(codes::HOST_TOOL_UNRESOLVED) => codes::HOST_TOOL_UNRESOLVED,
        Some(codes::HOST_FOREIGN_CONTRIBUTION) => codes::HOST_FOREIGN_CONTRIBUTION,
        // A backend exit is reported to pages, never to the backend itself.
        Some(codes::BACKEND_EXITED) => codes::HOST_FAILED,
        _ => codes::HOST_FAILED,
    };
    (code, host_rpc_code(code))
}

/// Key of a stored secret, also the alphabet of subscription and context
/// IDs. It excludes path separators so a host may use the value in a store
/// item or file name without escaping.
pub(crate) fn is_credential_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CREDENTIAL_KEY_BYTES
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | ':' | '-')
        })
}

/// Truncates a host's failure message like a backend's.
pub fn bounded_host_message(value: &str) -> String {
    bounded_message(value)
}

/// IDs of a descriptor array, refusing malformed descriptors, duplicates,
/// IDs already in `existing`, and a registry beyond the session limit.
pub(crate) fn descriptor_ids(
    descriptors: &Value,
    existing: &BTreeSet<String>,
) -> Result<BTreeSet<String>, &'static str> {
    let descriptors = descriptors
        .as_array()
        .ok_or("`functions` must be an array.")?;
    let mut ids = BTreeSet::new();
    for descriptor in descriptors {
        // Unknown descriptor fields are ignored within a version, as for
        // every negotiated object, so a backend can add optional metadata.
        let id = descriptor
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| is_function_id(id))
            .ok_or("Every function needs a valid `id`.")?;
        if existing.contains(id) || !ids.insert(id.to_string()) {
            return Err("A function ID is registered twice.");
        }
    }
    if existing.len() + ids.len() > MAX_FUNCTIONS {
        return Err("Too many functions are registered.");
    }
    Ok(ids)
}

/// The manifest's plugin ID alphabet: lowercase letters, digits, `.` and `-`.
pub fn is_plugin_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_PLUGIN_ID_BYTES
        && value.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '.'
                || character == '-'
        })
}

pub(crate) fn is_plugin_error_code(value: &str) -> bool {
    let mut characters = value.chars();
    value.len() <= MAX_PLUGIN_ERROR_CODE_BYTES
        && characters
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
}

/// Truncates at a character boundary so a hostile message cannot inflate
/// every event that carries it.
pub(crate) fn bounded_message(value: &str) -> String {
    if value.len() <= MAX_MESSAGE_BYTES {
        return value.to_string();
    }
    let mut end = MAX_MESSAGE_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

pub(crate) fn invalid_result(message: &str) -> FunctionFailure {
    FunctionFailure {
        code: codes::INVALID_RESULT,
        message: message.to_string(),
        plugin_error_code: None,
    }
}

pub(crate) fn refusal(category: ErrorCode, message: &str, code: &str) -> CoreError {
    CoreError::new(category, message).with_details(code)
}
