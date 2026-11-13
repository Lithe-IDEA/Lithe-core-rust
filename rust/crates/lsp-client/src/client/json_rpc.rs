//! JSON-RPC request, notification, result and error framing.

use super::*;

pub(crate) fn json_rpc_request(id: &str, method: &str, params: Value) -> Result<String, CoreError> {
    encode_json_rpc(json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params
    }))
}

/// Encodes a JSON-RPC notification.
pub fn json_rpc_notification(method: &str, params: Value) -> Result<String, CoreError> {
    encode_json_rpc(json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params
    }))
}

pub(crate) fn json_rpc_message_without_params(
    id: Option<&str>,
    method: &str,
) -> Result<String, CoreError> {
    let mut message = json!({
        "jsonrpc": "2.0",
        "method": method
    });
    if let Some(id) = id {
        message["id"] = Value::String(id.to_string());
    }
    encode_json_rpc(message)
}

/// Encodes a successful JSON-RPC response to request `id`.
pub fn json_rpc_result(id: &Value, result: Value) -> Result<String, CoreError> {
    encode_json_rpc(json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    }))
}

/// A JSON-RPC error whose `data.code` carries a stable plugin-contract code.
pub fn json_rpc_error_with_code(
    id: &Value,
    code: i64,
    message: &str,
    contract_code: &str,
) -> Result<String, CoreError> {
    json_rpc_error_with_data(id, code, message, json!({ "code": contract_code }))
}

/// A JSON-RPC error with the given `data`, which holds at least the stable
/// plugin-contract `code`.
pub fn json_rpc_error_with_data(
    id: &Value,
    code: i64,
    message: &str,
    data: Value,
) -> Result<String, CoreError> {
    encode_json_rpc(json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message, "data": data }
    }))
}

pub(crate) fn json_rpc_error(id: &Value, code: i64, message: &str) -> Result<String, CoreError> {
    encode_json_rpc(json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    }))
}

fn encode_json_rpc(value: Value) -> Result<String, CoreError> {
    serde_json::to_string(&value).map_err(|error| {
        CoreError::new(ErrorCode::Unknown, "Could not encode LSP JSON-RPC message")
            .with_details(error.to_string())
    })
}

pub(crate) fn lsp_message_id(message: &Value) -> Option<String> {
    message.get("id").and_then(|id| match id {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    })
}
