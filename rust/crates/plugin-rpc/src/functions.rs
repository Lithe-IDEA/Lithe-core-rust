//! Function registration, invocation planning, outcomes and progress.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
/// Address of one plugin function: the owning plugin plus its function ID.
/// The host routes by plugin identity first, so a reference can never reach
/// a session of another plugin.
pub struct FunctionReference {
    /// Plugin that registered the function.
    #[serde(rename = "pluginID")]
    pub plugin_id: String,
    /// Function ID within that plugin.
    #[serde(rename = "functionID")]
    pub function_id: String,
}

/// What a session knows when an invocation is planned.
pub struct FunctionSession<'a> {
    /// Plugin that started the session; `None` for host-owned servers.
    pub plugin_id: Option<&'a str>,
    /// Whether the backend negotiated a supported protocol version.
    pub negotiated: bool,
    /// Currently registered function IDs.
    pub functions: &'a BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Terminal failure of one invocation, before it becomes a runtime error.
pub struct FunctionFailure {
    /// Stable contract code of the failure.
    pub code: &'static str,
    /// Bounded message for the user.
    pub message: String,
    /// Plugin-defined code from `error.data.code`, for the plugin's own UI.
    /// Lithe never interprets it.
    pub plugin_error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// One accepted progress report.
pub struct FunctionProgress {
    /// Operation the report belongs to.
    pub operation_id: String,
    /// Bounded status text, when the backend sent one.
    pub message: Option<String>,
    /// Whole percent in `0..=100`.
    pub percentage: Option<u64>,
}

/// The client half of the negotiation, placed under `capabilities.experimental`.
pub fn client_experimental_capabilities() -> (&'static str, Value) {
    (
        EXPERIMENTAL_KEY,
        json!({ "versions": supported_versions() }),
    )
}

/// The version and functions of a backend that answered a version this
/// client offered, or `None` when the protocol was not negotiated. A
/// malformed function list rejects the whole answer, so a session never
/// starts with half a registry.
pub fn negotiate(capabilities: &Value) -> Option<(u64, BTreeSet<String>)> {
    let answer = capabilities
        .get("experimental")
        .and_then(|experimental| experimental.get(EXPERIMENTAL_KEY))?;
    let version = answer.get("version").and_then(Value::as_u64)?;
    if !supported_versions().contains(&version) {
        return None;
    }
    let functions = match answer.get("functions") {
        None => BTreeSet::new(),
        Some(descriptors) => descriptor_ids(descriptors, &BTreeSet::new()).ok()?,
    };
    Some((version, functions))
}

/// The registry after a `lithe/function/register` request. The request is
/// all or nothing: one malformed, duplicate, or already registered function,
/// or exceeding the session limit, refuses every function in it.
pub fn apply_registration(
    current: &BTreeSet<String>,
    params: Option<&Value>,
) -> Result<BTreeSet<String>, &'static str> {
    let descriptors = params
        .and_then(|params| params.get("functions"))
        .filter(|functions| functions.as_array().is_some_and(|items| !items.is_empty()))
        .ok_or("`functions` must be a nonempty array.")?;
    let added = descriptor_ids(descriptors, current)?;
    let mut registry = current.clone();
    registry.extend(added);
    Ok(registry)
}

/// The registry and the withdrawn IDs after a `lithe/function/unregister`
/// request, all or nothing like registration.
pub fn apply_unregistration(
    current: &BTreeSet<String>,
    params: Option<&Value>,
) -> Result<(BTreeSet<String>, Vec<String>), &'static str> {
    let ids = params
        .and_then(|params| params.get("ids"))
        .and_then(Value::as_array)
        .filter(|ids| !ids.is_empty())
        .ok_or("`ids` must be a nonempty array.")?;
    let mut removed = BTreeSet::new();
    for id in ids {
        let id = id.as_str().ok_or("Every ID must be a string.")?;
        if !current.contains(id) {
            return Err("Only registered functions can be unregistered.");
        }
        if !removed.insert(id.to_string()) {
            return Err("An ID is listed twice.");
        }
    }
    let registry = current.difference(&removed).cloned().collect();
    Ok((registry, removed.into_iter().collect()))
}

/// Params of the `lithe/function/invoke` request for `reference`, or the
/// refusal with its stable code in `details`. Checks run in a fixed order so
/// every host reports the same code for the same request.
pub fn plan_invocation(
    session: &FunctionSession<'_>,
    reference: &FunctionReference,
    operation_id: &str,
    params: &Value,
) -> Result<Value, CoreError> {
    if !is_plugin_id(&reference.plugin_id) || !is_function_id(&reference.function_id) {
        return Err(refusal(
            ErrorCode::InvalidRequest,
            "The function reference is malformed.",
            codes::INVALID_REFERENCE,
        ));
    }
    if !params.is_object() {
        return Err(refusal(
            ErrorCode::InvalidRequest,
            "Function params must be a JSON object.",
            codes::INVALID_PARAMS,
        ));
    }
    let Some(owner) = session.plugin_id else {
        return Err(refusal(
            ErrorCode::NotSupported,
            "This session does not belong to a plugin.",
            codes::UNAVAILABLE,
        ));
    };
    if owner != reference.plugin_id {
        return Err(refusal(
            ErrorCode::PermissionDenied,
            "The function belongs to another plugin.",
            codes::WRONG_PLUGIN,
        ));
    }
    if !session.negotiated {
        return Err(refusal(
            ErrorCode::NotSupported,
            "The plugin process did not agree on a plugin-function protocol version.",
            codes::UNAVAILABLE,
        ));
    }
    if !session.functions.contains(&reference.function_id) {
        return Err(refusal(
            ErrorCode::NotSupported,
            "The plugin has not registered this function.",
            codes::UNKNOWN,
        ));
    }
    Ok(json!({
        "functionID": reference.function_id,
        "operationID": operation_id,
        "params": params
    }))
}

/// Normalizes the backend's JSON-RPC response to an invocation. A success is
/// wrapped as `{ "value": result }` like build-provider replies, so `null` is
/// a valid result distinct from a missing one.
pub fn invocation_outcome(response: &Value) -> Result<Value, FunctionFailure> {
    if let Some(error) = response.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .map(bounded_message)
            .filter(|message| !message.is_empty())
            .unwrap_or_else(|| "The plugin function failed.".to_string());
        let plugin_error_code = error
            .get("data")
            .and_then(|data| data.get("code"))
            .and_then(Value::as_str)
            .filter(|code| is_plugin_error_code(code))
            .map(ToString::to_string);
        return Err(FunctionFailure {
            code: codes::FAILED,
            message,
            plugin_error_code,
        });
    }
    let Some(result) = response.get("result") else {
        return Err(invalid_result("The plugin function returned no result."));
    };
    // Serialized length is what crosses the C ABI and the host bridge.
    if result.to_string().len() > MAX_RESULT_BYTES {
        return Err(invalid_result(
            "The plugin function returned more data than a function result may hold.",
        ));
    }
    Ok(json!({ "value": result }))
}

/// A progress report that is well formed, or `None` to ignore it. Whether the
/// operation is still running is the engine's check, not this function's.
pub fn parse_progress(params: Option<&Value>) -> Option<FunctionProgress> {
    let params = params?.as_object()?;
    let operation_id = params.get("operationID")?.as_str()?.to_string();
    let message = match params.get("message") {
        None => None,
        Some(message) => Some(bounded_message(message.as_str()?)),
    };
    let percentage = match params.get("percentage") {
        None => None,
        Some(percentage) => Some(percentage.as_u64().filter(|value| *value <= 100)?),
    };
    Some(FunctionProgress {
        operation_id,
        message,
        percentage,
    })
}

/// The stable code for a backend request the host refuses on principle, or
/// `None` for any other method. Domain dispatch can route back into the
/// requesting backend, whose handler would then wait on itself; host
/// capabilities are offered only to a plugin-owned session that negotiated
/// the protocol.
pub fn refused_backend_request(method: &str) -> Option<&'static str> {
    if method == INVOKE_METHOD
        || method.starts_with("lithe/build/")
        || method.starts_with("lithe/keymap/")
    {
        Some(codes::RECURSIVE_CALL)
    } else if method.starts_with("lithe/host/") {
        Some(codes::HOST_UNAVAILABLE)
    } else {
        None
    }
}
