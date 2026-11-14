//! Host methods a backend may call and how Core routes them.

use super::*;

/// Why a backend's `lithe/host/*` request is not forwarded to the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRequestRefusal {
    /// JSON-RPC error code of the answer.
    pub rpc_code: i64,
    /// Stable contract code, sent as `error.data.code`.
    pub code: &'static str,
    /// Message of the JSON-RPC error.
    pub message: &'static str,
    /// Sent as `error.data.details`, such as the `option` a host does not
    /// support.
    pub details: Option<Value>,
}

/// Where an accepted `lithe/host/*` request goes.
#[derive(Debug, Clone, PartialEq)]
pub enum HostRoute {
    /// To the host, as a `hostRequest` event of this service.
    Forward(&'static str),
    /// Answered by Core with this result; the host never sees the request.
    Answer(Value),
    /// Another plugin's function: Core's relay checks, dispatches, and
    /// answers it. The host sees only the start request for a provider whose
    /// backend is not running.
    Relay,
}

/// How Core checks the params of one host method before the host sees them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostParams {
    /// Absent, `null`, or `{}`.
    Empty,
    /// `{ key }`, or `{ key, secret }` when `with_secret`.
    Credential { with_secret: bool },
    /// `{ subscriptionID }`.
    Subscription,
    /// Exactly `{ context }`, a well-formed context reference.
    Context,
    /// Snapshot source listing or one bounded chunk from a selected source.
    KeymapSnapshot { chunk: bool },
    /// `{ context, serverID, method, params?, timeoutMilliseconds? }`: one
    /// request to the plugin's own language server (plan API-25).
    LanguageServerRequest,
    /// `{ context, serverID, method, params? }`: one notification to it.
    LanguageServerNotify,
    /// `{ context, serverID, state, message?, percentage? }`: the plugin's
    /// preparation status of the server.
    LanguageServerStatus,
    /// `{ context, scheme, function }`: the function that supplies the text
    /// of a URI scheme (plan API-12).
    ContentProvider,
    /// Exactly `{ functionID }`, a well-formed function ID.
    FunctionProviders,
    /// `{ pluginID, functionID, context, params?, timeoutMilliseconds? }`.
    FunctionInvoke,
    /// Any object whose `context`, when present, is a well-formed
    /// reference. The service checks the rest and answers
    /// `plugin.host.invalidParams` or, for a field it does not know,
    /// `plugin.host.unsupportedOption` itself; for methods whose params
    /// Core cannot type without owning their domain.
    // No row uses it yet; the file, editor, and window services that follow
    // add their rows with it.
    #[allow(dead_code)]
    Object,
}

/// One row of the host-method table.
pub(crate) struct HostMethod {
    pub method: &'static str,
    /// Service whose grant admits the method.
    pub service: &'static str,
    pub params: HostParams,
    /// The method's own grant, or `None` when the service's aggregate grant admits it.
    pub permission: Option<&'static str>,
}

/// Every backend-to-host method Core admits. A method missing here is
/// `plugin.host.unavailable` whatever the host could do, so a new host
/// service adds its rows here, its grant to `GRANTABLE_HOST_SERVICES` and
/// the manifest permission, and its handler to the host's dispatcher.
pub(crate) const HOST_METHODS: &[HostMethod] = &[
    HostMethod {
        method: RUN_START,
        service: HOST_SERVICE_RUN,
        permission: Some("run.start"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/run/stop",
        service: HOST_SERVICE_RUN,
        permission: Some("run.start"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/debug/start",
        service: "debug",
        permission: Some("debug.start"),
        params: HostParams::Object,
    },
    HostMethod {
        method: FUNCTIONS_PROVIDERS,
        service: HOST_SERVICE_FUNCTIONS,
        permission: None,
        params: HostParams::FunctionProviders,
    },
    HostMethod {
        method: FUNCTIONS_INVOKE,
        service: HOST_SERVICE_FUNCTIONS,
        permission: None,
        params: HostParams::FunctionInvoke,
    },
    HostMethod {
        method: "lithe/host/keymapImport/listInputs",
        service: "keymapImport",
        permission: Some("keymaps.readSelectedSources"),
        params: HostParams::KeymapSnapshot { chunk: false },
    },
    HostMethod {
        method: "lithe/host/keymapImport/readChunk",
        service: "keymapImport",
        permission: Some("keymaps.readSelectedSources"),
        params: HostParams::KeymapSnapshot { chunk: true },
    },
    HostMethod {
        method: "lithe/host/terminal/reopen",
        service: "terminal",
        permission: Some("terminal.create"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/terminal/dispose",
        service: "terminal",
        permission: Some("terminal.create"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/terminal/create",
        service: "terminal",
        permission: Some("terminal.create"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/terminal/show",
        service: "terminal",
        permission: Some("terminal.create"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/terminal/close",
        service: "terminal",
        permission: Some("terminal.create"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/terminal/onExit",
        service: "terminal",
        permission: Some("terminal.create"),
        params: HostParams::Object,
    },
    HostMethod {
        method: CREDENTIALS_GET,
        service: HOST_SERVICE_CREDENTIALS,
        permission: None,
        params: HostParams::Credential { with_secret: false },
    },
    HostMethod {
        method: CREDENTIALS_SET,
        service: HOST_SERVICE_CREDENTIALS,
        permission: None,
        params: HostParams::Credential { with_secret: true },
    },
    HostMethod {
        method: CREDENTIALS_DELETE,
        service: HOST_SERVICE_CREDENTIALS,
        permission: None,
        params: HostParams::Credential { with_secret: false },
    },
    HostMethod {
        method: GET_CAPABILITIES,
        service: HOST_SERVICE_HOST,
        permission: None,
        params: HostParams::Empty,
    },
    HostMethod {
        method: SUBSCRIPTIONS_DISPOSE,
        service: HOST_SERVICE_SUBSCRIPTIONS,
        permission: None,
        params: HostParams::Subscription,
    },
    HostMethod {
        method: WORKSPACE_LIST_CONTEXTS,
        service: HOST_SERVICE_WORKSPACE,
        permission: None,
        params: HostParams::Empty,
    },
    HostMethod {
        method: WORKSPACE_GET_CONTEXT,
        service: HOST_SERVICE_WORKSPACE,
        permission: None,
        params: HostParams::Context,
    },
    HostMethod {
        method: WORKSPACE_GET_FOLDERS,
        service: HOST_SERVICE_WORKSPACE,
        permission: None,
        params: HostParams::Context,
    },
    HostMethod {
        method: "lithe/host/workspace/readDirectory",
        service: "workspace",
        permission: Some("workspace.files.read"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/workspace/stat",
        service: "workspace",
        permission: Some("workspace.files.read"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/workspace/readFile",
        service: "workspace",
        permission: Some("workspace.files.read"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/workspace/watch",
        service: "workspace",
        permission: Some("workspace.files.watch"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/workspace/findFiles",
        service: "workspace",
        permission: Some("workspace.search"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/workspace/findText",
        service: "workspace",
        permission: Some("workspace.search"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/documents/list",
        service: "documents",
        permission: Some("documents.read"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/documents/getSnapshot",
        service: "documents",
        permission: Some("documents.read"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/editor/registerActions",
        service: "editor",
        permission: Some("editor.actions"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/editor/registerProviders",
        service: "editor",
        permission: Some("editor.providers"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/editor/getActive",
        service: "editor",
        permission: Some("documents.read"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/editor/getSelections",
        service: "editor",
        permission: Some("documents.read"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/editor/applyEdits",
        service: "editor",
        permission: Some("editor.edit"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/editor/replaceSelections",
        service: "editor",
        permission: Some("editor.edit"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/editor/insertText",
        service: "editor",
        permission: Some("editor.edit"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/editor/open",
        service: "editor",
        permission: Some("editor.open"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/editor/reveal",
        service: "editor",
        permission: Some("editor.open"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/preview/openHTML",
        service: "preview",
        permission: Some("preview.open"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/browser/openExternal",
        service: "browser",
        permission: Some("browser.open"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/window/showInformation",
        service: "window",
        permission: Some("window.messages"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/window/showWarning",
        service: "window",
        permission: Some("window.messages"),
        params: HostParams::Object,
    },
    HostMethod {
        method: "lithe/host/window/showError",
        service: "window",
        permission: Some("window.messages"),
        params: HostParams::Object,
    },
    // Plan API-25: the plugin's own language servers. The host resolves
    // `{ context, serverID }` to the session and Core's
    // `lsp.pluginServerRequest` enforces the declared session policy.
    HostMethod {
        method: LANGUAGE_SERVER_REQUEST,
        service: HOST_SERVICE_LANGUAGE_SERVER,
        permission: Some(PERMISSION_LANGUAGE_SERVER_REQUEST),
        params: HostParams::LanguageServerRequest,
    },
    HostMethod {
        method: LANGUAGE_SERVER_NOTIFY,
        service: HOST_SERVICE_LANGUAGE_SERVER,
        permission: Some(PERMISSION_LANGUAGE_SERVER_REQUEST),
        params: HostParams::LanguageServerNotify,
    },
    HostMethod {
        method: LANGUAGE_SERVER_SET_STATUS,
        service: HOST_SERVICE_LANGUAGE_SERVER,
        permission: Some(PERMISSION_LANGUAGE_SERVER_REQUEST),
        params: HostParams::LanguageServerStatus,
    },
    // Plan API-12: read-only text of a URI scheme the backend serves.
    HostMethod {
        method: DOCUMENTS_REGISTER_CONTENT_PROVIDER,
        service: "documents",
        permission: Some(PERMISSION_DOCUMENTS_CONTENT_PROVIDER),
        params: HostParams::ContentProvider,
    },
];

/// Host service of the plugin's own language servers (plan API-25).
pub(crate) const HOST_SERVICE_LANGUAGE_SERVER: &str = "languageServer";

/// Forwards one declared request to the plugin's language server.
pub(crate) const LANGUAGE_SERVER_REQUEST: &str = "lithe/host/languageServer/request";

/// Forwards one notification to the plugin's language server.
pub(crate) const LANGUAGE_SERVER_NOTIFY: &str = "lithe/host/languageServer/notify";

/// Reports the plugin's preparation state of a language server to the user.
pub(crate) const LANGUAGE_SERVER_SET_STATUS: &str = "lithe/host/languageServer/setStatus";

/// Registers a backend function as the content provider of a URI scheme.
pub(crate) const DOCUMENTS_REGISTER_CONTENT_PROVIDER: &str =
    "lithe/host/documents/registerContentProvider";

/// Services every negotiated plugin-owned session has without a permission:
/// they reveal nothing beyond the session's own grants.
pub(crate) fn is_implicit_service(service: &str) -> bool {
    service == HOST_SERVICE_HOST
        || service == HOST_SERVICE_SUBSCRIPTIONS
        || service == HOST_SERVICE_COMMANDS
}

/// Where a backend request for a host service goes, or why it is refused.
/// `version` is the negotiated protocol version, `None` when the backend did
/// not negotiate one, and `granted` the services the plugin's permissions
/// grant the session. The params are checked here, so the host only ever sees
/// a well-formed request. A field the method does not define is
/// `plugin.host.unsupportedOption` naming it, so a later host can add it as
/// an option without a new version and a backend can tell an option this
/// host lacks from a malformed request.
pub fn host_request(
    method: &str,
    params: Option<&Value>,
    version: Option<u64>,
    granted: &BTreeSet<String>,
) -> Result<HostRoute, HostRequestRefusal> {
    let entry = version.and_then(|_| HOST_METHODS.iter().find(|entry| entry.method == method));
    let (Some(version), Some(entry)) = (version, entry) else {
        return Err(HostRequestRefusal {
            rpc_code: -32601,
            code: codes::HOST_UNAVAILABLE,
            message: "The host offers no such service to this plugin process.",
            details: None,
        });
    };
    if !is_implicit_service(entry.service)
        && !granted.contains(if entry.permission.is_some() {
            entry.method
        } else {
            entry.service
        })
    {
        return Err(HostRequestRefusal {
            rpc_code: host_rpc_code(codes::HOST_PERMISSION_DENIED),
            code: codes::HOST_PERMISSION_DENIED,
            message: "The plugin does not declare the permission for this host service.",
            details: None,
        });
    }
    if params.is_some_and(|value| value.to_string().len() > MAX_RESULT_BYTES) {
        return Err(HostRequestRefusal {
            rpc_code: host_rpc_code(codes::HOST_LIMIT_EXCEEDED),
            code: codes::HOST_LIMIT_EXCEEDED,
            message: "Host request exceeds the byte limit.",
            details: None,
        });
    }
    check_host_params(entry.params, params)?;
    if entry.method == GET_CAPABILITIES {
        return Ok(HostRoute::Answer(capabilities_answer(version, granted)));
    }
    if entry.service == HOST_SERVICE_FUNCTIONS {
        return Ok(HostRoute::Relay);
    }
    Ok(HostRoute::Forward(entry.service))
}

/// The params the host receives for an accepted request: the backend's own,
/// except that a status message is cut to the bound a progress message has,
/// so a hostile backend cannot inflate every event that carries it.
pub fn forwarded_host_params(method: &str, params: Option<&Value>) -> Value {
    let mut forwarded = params.cloned().unwrap_or(Value::Null);
    if method == LANGUAGE_SERVER_SET_STATUS {
        if let Some(message) = forwarded.get_mut("message") {
            if let Some(text) = message.as_str() {
                *message = Value::String(bounded_message(text));
            }
        }
    }
    forwarded
}

/// The answer to `lithe/host/getCapabilities`: the negotiated version, the
/// services and methods this session may call, where the backend runs, and
/// the bounds Core enforces. It lists only what the session is granted, so
/// it reveals nothing about other plugins. `granted` is what the host
/// passed to `lsp.startServer.hostServices`; a host passes only services it
/// serves, so a listed method never answers `plugin.host.unavailable` for
/// want of a handler.
pub(crate) fn capabilities_answer(version: u64, granted: &BTreeSet<String>) -> Value {
    let admitted = |service: &str| is_implicit_service(service) || granted.contains(service);
    let mut services = serde_json::Map::new();
    let mut add = |service: &str, kind: &str, method: &str| {
        let entry = services
            .entry(service.to_string())
            .or_insert_with(|| json!({ "methods": [], "notifications": [] }));
        if let Some(list) = entry[kind].as_array_mut() {
            list.push(json!(method));
        }
    };
    for entry in HOST_METHODS.iter().filter(|entry| {
        if entry.permission.is_some() {
            granted.contains(entry.method)
        } else {
            admitted(entry.service)
        }
    }) {
        add(entry.service, "methods", entry.method);
    }
    for entry in HOST_NOTIFICATIONS
        .iter()
        .filter(|entry| entry.admitted(granted))
    {
        add(entry.service, "notifications", entry.method);
    }
    json!({
        "protocolVersion": version,
        // The host starts every plugin backend on the machine that runs
        // Lithe, also for remote workspaces.
        "backendAuthority": "local",
        "services": services,
        "limits": {
            "pendingHostRequests": MAX_PENDING_HOST_REQUESTS,
            "functionResultBytes": MAX_RESULT_BYTES,
            "hostRequestBytes": MAX_RESULT_BYTES,
            "hostResultBytes": MAX_RESULT_BYTES,
            "errorMessageBytes": MAX_MESSAGE_BYTES,
            "errorDetailsBytes": MAX_HOST_DETAILS_BYTES,
            "notificationBytes": MAX_HOST_NOTIFICATION_BYTES,
        },
    })
}
