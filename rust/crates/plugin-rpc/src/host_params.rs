//! Parameter checks of host methods and the context and caller Core attaches.

use super::*;

/// A UI command ID as the manifest schema spells it: `[a-z0-9.-]+`.
pub(crate) fn is_command_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_FUNCTION_ID_BYTES
        && value.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '.' | '-')
        })
}

pub(crate) fn check_host_params(
    rule: HostParams,
    params: Option<&Value>,
) -> Result<(), HostRequestRefusal> {
    let invalid = |message| HostRequestRefusal {
        rpc_code: -32602,
        code: codes::HOST_INVALID_PARAMS,
        message,
        details: None,
    };
    let empty = serde_json::Map::new();
    let params = match params {
        None | Some(Value::Null) if rule == HostParams::Empty => &empty,
        Some(Value::Object(params)) => params,
        _ => return Err(invalid("Params must be an object.")),
    };
    let allowed: &[&str] = match rule {
        HostParams::Empty => &[],
        HostParams::Credential { with_secret: true } => &["key", "secret"],
        HostParams::Credential { with_secret: false } => &["key"],
        HostParams::Subscription => &["subscriptionID"],
        HostParams::Context => &["context"],
        HostParams::FunctionProviders => &["functionID"],
        HostParams::FunctionInvoke => &[
            "pluginID",
            "functionID",
            "context",
            "params",
            "timeoutMilliseconds",
            "parentOperationID",
        ],
        HostParams::KeymapSnapshot { chunk: false } => &["snapshotID", "providerID"],
        HostParams::KeymapSnapshot { chunk: true } => &[
            "snapshotID",
            "providerID",
            "fileID",
            "offset",
            "maximumBytes",
        ],
        HostParams::LanguageServerRequest => &[
            "context",
            "serverID",
            "method",
            "params",
            "timeoutMilliseconds",
        ],
        HostParams::LanguageServerNotify => &["context", "serverID", "method", "params"],
        HostParams::LanguageServerStatus => {
            &["context", "serverID", "state", "message", "percentage"]
        }
        HostParams::ContentProvider => &["context", "scheme", "function"],
        HostParams::Object => {
            return match params.get("context") {
                Some(context) if !is_context_reference(context) => {
                    Err(invalid(CONTEXT_REFERENCE_MESSAGE))
                }
                _ => Ok(()),
            };
        }
    };
    if let Some(option) = params.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(HostRequestRefusal {
            rpc_code: host_rpc_code(codes::HOST_UNSUPPORTED_OPTION),
            code: codes::HOST_UNSUPPORTED_OPTION,
            message: "Params name an option this host does not support.",
            details: Some(json!({ "option": option })),
        });
    }
    match rule {
        HostParams::Credential { with_secret } => {
            if !params
                .get("key")
                .and_then(Value::as_str)
                .is_some_and(is_credential_key)
            {
                return Err(invalid(
                    "A credential key is 1-128 ASCII letters, digits, '.', '_', ':' or '-'.",
                ));
            }
            if with_secret
                && params
                    .get("secret")
                    .and_then(Value::as_str)
                    .is_none_or(|secret| secret.len() > MAX_CREDENTIAL_SECRET_BYTES)
            {
                return Err(invalid("A secret is a string of at most 16 KiB."));
            }
        }
        HostParams::Subscription => {
            if !params
                .get("subscriptionID")
                .and_then(Value::as_str)
                .is_some_and(is_credential_key)
            {
                return Err(invalid(
                    "A subscription ID is 1-128 ASCII letters, digits, '.', '_', ':' or '-'.",
                ));
            }
        }
        HostParams::Context => {
            if !params.get("context").is_some_and(is_context_reference) {
                return Err(invalid(CONTEXT_REFERENCE_MESSAGE));
            }
        }
        HostParams::FunctionProviders => {
            if !params
                .get("functionID")
                .and_then(Value::as_str)
                .is_some_and(is_function_id)
            {
                return Err(invalid(FUNCTION_ID_MESSAGE));
            }
        }
        HostParams::FunctionInvoke => check_function_invoke_params(params)?,
        HostParams::KeymapSnapshot { chunk } => {
            let opaque_id = |key: &str| {
                params.get(key).and_then(Value::as_str).is_some_and(|id| {
                    !id.is_empty()
                        && id.len() <= 128
                        && id
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
                })
            };
            let provider = params
                .get("providerID")
                .and_then(Value::as_str)
                .is_some_and(lithe_plugin_keymap_import::valid_provider_id);
            let valid_chunk = !chunk
                || (opaque_id("fileID")
                    && params
                        .get("offset")
                        .and_then(Value::as_u64)
                        .is_some_and(|offset| {
                            offset <= lithe_plugin_keymap_import::MAX_INPUT_FILE_BYTES as u64
                        })
                    && params
                        .get("maximumBytes")
                        .and_then(Value::as_u64)
                        .is_some_and(|size| {
                            size > 0
                                && size <= lithe_plugin_keymap_import::MAX_INPUT_CHUNK_BYTES as u64
                        }));
            if !opaque_id("snapshotID") || !provider || !valid_chunk {
                return Err(invalid(
                    "Snapshot source parameters are malformed or exceed their bounds.",
                ));
            }
        }
        HostParams::LanguageServerRequest
        | HostParams::LanguageServerNotify
        | HostParams::LanguageServerStatus => {
            check_language_server_params(rule, params, &invalid)?;
        }
        HostParams::ContentProvider => {
            if !params.get("context").is_some_and(is_context_reference) {
                return Err(invalid(CONTEXT_REFERENCE_MESSAGE));
            }
            let scheme = params.get("scheme").and_then(Value::as_str);
            if !scheme.is_some_and(is_content_scheme) {
                return Err(invalid(
                    "A content scheme is 1-64 lowercase ASCII letters, digits, '+', '-' or '.', starts with a letter, and is not a scheme the host owns.",
                ));
            }
            if !params
                .get("function")
                .and_then(Value::as_str)
                .is_some_and(is_function_id)
            {
                return Err(invalid(
                    "The content function must be a plugin function ID.",
                ));
            }
        }
        HostParams::Empty | HostParams::Object => {}
    }
    Ok(())
}

/// Checks the params the `languageServer` host methods share: the context,
/// the server, the method, and each method's own fields. Whether the server
/// belongs to the plugin and the method is declared is the engine's answer,
/// since only it holds the session policy.
fn check_language_server_params(
    rule: HostParams,
    params: &serde_json::Map<String, Value>,
    invalid: &dyn Fn(&'static str) -> HostRequestRefusal,
) -> Result<(), HostRequestRefusal> {
    if !params.get("context").is_some_and(is_context_reference) {
        return Err(invalid(CONTEXT_REFERENCE_MESSAGE));
    }
    if !params
        .get("serverID")
        .and_then(Value::as_str)
        .is_some_and(is_protocol_name)
    {
        return Err(invalid(
            "serverID must name one of the plugin's language servers.",
        ));
    }
    if rule == HostParams::LanguageServerStatus {
        if !params
            .get("state")
            .and_then(Value::as_str)
            .is_some_and(is_status_state)
        {
            return Err(invalid(
                "state is one of preparing, ready, degraded, or failed.",
            ));
        }
        if params
            .get("message")
            .is_some_and(|message| !message.is_string())
        {
            return Err(invalid("message must be a string."));
        }
        if params
            .get("percentage")
            .is_some_and(|value| value.as_u64().is_none_or(|percentage| percentage > 100))
        {
            return Err(invalid("percentage is a whole number from 0 to 100."));
        }
        return Ok(());
    }
    if !params
        .get("method")
        .and_then(Value::as_str)
        .is_some_and(is_protocol_name)
    {
        return Err(invalid("method must be a non-empty LSP method name."));
    }
    if rule == HostParams::LanguageServerRequest
        && params.get("timeoutMilliseconds").is_some_and(|value| {
            !value.as_u64().is_some_and(|milliseconds| {
                (1..=lithe_plugin_language_servers::MAX_REQUEST_TIMEOUT_MILLISECONDS)
                    .contains(&milliseconds)
            })
        })
    {
        return Err(invalid(
            "timeoutMilliseconds is a whole number from 1 to 300000.",
        ));
    }
    Ok(())
}

const FUNCTION_ID_MESSAGE: &str =
    "A function ID is dot-separated lowerCamelCase segments of at most 128 bytes.";

/// The shape of a `lithe/host/functions/invoke` request. Whether the caller
/// declared the call and the provider exports it is the relay's check, which
/// needs the plugin snapshot.
fn check_function_invoke_params(
    params: &serde_json::Map<String, Value>,
) -> Result<(), HostRequestRefusal> {
    let invalid = |message| HostRequestRefusal {
        rpc_code: -32602,
        code: codes::HOST_INVALID_PARAMS,
        message,
        details: None,
    };
    if !params
        .get("pluginID")
        .and_then(Value::as_str)
        .is_some_and(is_plugin_id)
    {
        return Err(invalid(
            "A plugin ID is lowercase letters, digits, '.' and '-'.",
        ));
    }
    if !params
        .get("functionID")
        .and_then(Value::as_str)
        .is_some_and(is_function_id)
    {
        return Err(invalid(FUNCTION_ID_MESSAGE));
    }
    if !params.get("context").is_some_and(is_context_reference) {
        return Err(invalid(CONTEXT_REFERENCE_MESSAGE));
    }
    if params.get("params").is_some_and(|value| !value.is_object()) {
        return Err(invalid("Function params must be a JSON object."));
    }
    if params
        .get("timeoutMilliseconds")
        .is_some_and(|value| value.as_u64().is_none_or(|milliseconds| milliseconds < 1))
    {
        return Err(invalid("A timeout is a positive number of milliseconds."));
    }
    // Only the shape is checked here. Whether the ID names a call this backend
    // is serving needs the session's pending requests, so the relay decides.
    if params.get("parentOperationID").is_some_and(|value| {
        !value
            .as_str()
            .is_some_and(|id| (1..=128).contains(&id.len()))
    }) {
        return Err(invalid(
            "A parent operation ID is the operationID of a call this backend is serving, 1-128 bytes.",
        ));
    }
    Ok(())
}

/// A preparation state a plugin may report for its language server.
fn is_status_state(value: &str) -> bool {
    matches!(value, "preparing" | "ready" | "degraded" | "failed")
}

/// Schemes the host or the platform resolves itself. A plugin that served
/// one would replace what the user expects the URI to open.
const RESERVED_CONTENT_SCHEMES: &[&str] = &[
    "about",
    "blob",
    "data",
    "file",
    "ftp",
    "http",
    "https",
    "javascript",
    "lithe",
    "mailto",
    "untitled",
    "ws",
    "wss",
];

/// A URI scheme a plugin may serve (RFC 3986, lowercase, bounded).
pub(crate) fn is_content_scheme(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
        && value.len() <= 64
        && characters.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '+' | '-' | '.')
        })
        && !RESERVED_CONTENT_SCHEMES.contains(&value)
}

const CONTEXT_REFERENCE_MESSAGE: &str =
    "A context is { contextID, generation }: an ID of 1-128 ASCII letters, digits, '.', '_', ':' or '-', and a positive generation.";

/// `{ contextID, generation }` naming one generation of one workspace
/// context. Only the shape is checked here; whether it is still current is
/// the host's answer (`plugin.host.contextExpired`).
pub(crate) fn is_context_reference(value: &Value) -> bool {
    let Some(reference) = value.as_object() else {
        return false;
    };
    reference.len() == 2
        && reference
            .get("contextID")
            .and_then(Value::as_str)
            .is_some_and(is_credential_key)
        && reference
            .get("generation")
            .and_then(Value::as_u64)
            .is_some_and(|generation| generation >= 1)
}

/// Adds the workspace context the host named to the params of one
/// `lithe/function/invoke`. A malformed reference is refused, and a call
/// without a context is written unchanged.
pub fn attach_context(invoke_params: &mut Value, context: Option<&Value>) -> Result<(), CoreError> {
    let Some(context) = context else {
        return Ok(());
    };
    if !is_context_reference(context) {
        return Err(refusal(
            ErrorCode::InvalidRequest,
            "The workspace context reference is malformed.",
            codes::INVALID_PARAMS,
        ));
    }
    if let Some(params) = invoke_params.as_object_mut() {
        params.insert("context".to_string(), context.clone());
    }
    Ok(())
}

/// Writes the calling plugin into the params of a relayed
/// `lithe/function/invoke` as `caller: { "pluginID": ... }`, a sibling of
/// `functionID`, `operationID`, `params` and `context`. Only Core writes it,
/// from the identity of the session that made the request, so a callee can
/// key state by caller without trusting anything in the params. A call the
/// host itself starts (`caller` is `None`) never carries one: its absence
/// means the host.
pub fn attach_caller(invoke_params: &mut Value, caller: Option<&str>) {
    let Some(caller) = caller else {
        return;
    };
    if let Some(params) = invoke_params.as_object_mut() {
        params.insert("caller".to_string(), json!({ "pluginID": caller }));
    }
}
