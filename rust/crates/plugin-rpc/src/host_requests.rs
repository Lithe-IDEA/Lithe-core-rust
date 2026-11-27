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
    /// An agent-session method (plan API-45): Core's agent runtime checks
    /// that the session belongs to the backend and answers it. The host never
    /// sees the request.
    Agents,
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
    /// `{ context, rootIDs? }`: a well-formed context reference and at most
    /// 256 distinct root IDs (plan API-40).
    HostPaths,
    /// Build option lookup or build configuration subscription.
    BuildConfiguration { watch: bool },
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
    /// `{ context, documentID, expectedVersion? }` (plan API-11).
    DocumentSave,
    /// `{ context, documentID, target }`: a save to a new file of the same
    /// root's files (plan API-11).
    DocumentSaveAs,
    /// `{ context, documentID }` (plan API-11).
    DocumentClose,
    /// `{ context, resource, kind, text?, linkTarget? }`: a new file,
    /// directory or symbolic link below a root (plan API-11).
    WorkspaceCreate,
    /// `{ context, resource, target }`: a rename or copy within one root
    /// (plan API-11).
    WorkspaceMove,
    /// `{ context, resource }`: a move to the system trash (plan API-11).
    WorkspaceTrash,
    /// `{ context, resource, cursor?, limit? }` (plan API-06).
    WorkspaceReadDirectory,
    /// `{ context, resource }`, including the root (plan API-06).
    WorkspaceStat,
    /// `{ context, resource, maximumBytes? }` (plan API-06).
    WorkspaceReadFile,
    /// `{ context, resource, recursive? }` (plan API-07).
    WorkspaceWatch,
    /// `{ context, resource, query, fileMask?, cursor?, limit? }` (plan API-08).
    WorkspaceFindFiles,
    /// A bounded text search at the workspace root (plan API-08).
    WorkspaceFindText,
    /// Strict, bounded interactive, presentation and managed-network requests.
    Workflow,
    /// Any object whose `context`, when present, is a well-formed
    /// reference. The service checks the rest and answers
    /// `plugin.host.invalidParams` or, for a field it does not know,
    /// `plugin.host.unsupportedOption` itself; for methods whose params
    /// Core cannot type without owning their domain.
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
        method: "lithe/host/window/showQuickPick",
        service: "window",
        permission: Some("window.input"),
        params: HostParams::Workflow,
    },
    HostMethod {
        method: "lithe/host/window/showInputBox",
        service: "window",
        permission: Some("window.input"),
        params: HostParams::Workflow,
    },
    HostMethod {
        method: "lithe/host/preview/openMarkdown",
        service: "preview",
        permission: Some("preview.open"),
        params: HostParams::Workflow,
    },
    HostMethod {
        method: "lithe/host/editor/showDiff",
        service: "editor",
        permission: Some("editor.open"),
        params: HostParams::Workflow,
    },
    HostMethod {
        method: "lithe/host/views/refresh",
        service: "views",
        permission: Some("views.refresh"),
        params: HostParams::Workflow,
    },
    HostMethod {
        method: "lithe/host/output/create",
        service: "output",
        permission: Some("output.write"),
        params: HostParams::Workflow,
    },
    HostMethod {
        method: "lithe/host/output/append",
        service: "output",
        permission: Some("output.write"),
        params: HostParams::Workflow,
    },
    HostMethod {
        method: "lithe/host/output/clear",
        service: "output",
        permission: Some("output.write"),
        params: HostParams::Workflow,
    },
    HostMethod {
        method: "lithe/host/output/show",
        service: "output",
        permission: Some("output.write"),
        params: HostParams::Workflow,
    },
    HostMethod {
        method: "lithe/host/output/dispose",
        service: "output",
        permission: Some("output.write"),
        params: HostParams::Workflow,
    },
    HostMethod {
        method: "lithe/host/progress/create",
        service: "progress",
        permission: Some("window.progress"),
        params: HostParams::Workflow,
    },
    HostMethod {
        method: "lithe/host/progress/report",
        service: "progress",
        permission: Some("window.progress"),
        params: HostParams::Workflow,
    },
    HostMethod {
        method: "lithe/host/progress/end",
        service: "progress",
        permission: Some("window.progress"),
        params: HostParams::Workflow,
    },
    HostMethod {
        method: "lithe/host/statusBar/setItem",
        service: "statusBar",
        permission: Some("statusBar.write"),
        params: HostParams::Workflow,
    },
    HostMethod {
        method: "lithe/host/statusBar/removeItem",
        service: "statusBar",
        permission: Some("statusBar.write"),
        params: HostParams::Workflow,
    },
    HostMethod {
        method: "lithe/host/clipboard/readText",
        service: "clipboard",
        permission: Some("clipboard.read"),
        params: HostParams::Workflow,
    },
    HostMethod {
        method: "lithe/host/clipboard/writeText",
        service: "clipboard",
        permission: Some("clipboard.write"),
        params: HostParams::Workflow,
    },
    HostMethod {
        method: "lithe/host/workspace/reveal",
        service: "workspace",
        permission: Some("workspace.reveal"),
        params: HostParams::Workflow,
    },
    HostMethod {
        method: "lithe/host/workspace/setDirectoryTypes",
        service: "workspace",
        permission: Some("workspace.directoryMarks"),
        params: HostParams::Workflow,
    },
    HostMethod {
        method: "lithe/host/workspace/setDirectoryMarks",
        service: "workspace",
        permission: Some("workspace.directoryMarks"),
        params: HostParams::Workflow,
    },
    HostMethod {
        method: "lithe/host/network/request",
        service: "network",
        permission: Some("network.request"),
        params: HostParams::Workflow,
    },
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
        method: "lithe/host/build/getOptions",
        service: "build",
        permission: Some("build.options.read"),
        params: HostParams::BuildConfiguration { watch: false },
    },
    HostMethod {
        method: "lithe/host/build/watchConfiguration",
        service: "build",
        permission: Some("build.configuration.watch"),
        params: HostParams::BuildConfiguration { watch: true },
    },
    HostMethod {
        method: WORKSPACE_GET_HOST_PATHS,
        service: HOST_SERVICE_WORKSPACE,
        permission: Some("workspace.metadata"),
        params: HostParams::HostPaths,
    },
    HostMethod {
        method: "lithe/host/workspace/readDirectory",
        service: "workspace",
        permission: Some("workspace.files.read"),
        params: HostParams::WorkspaceReadDirectory,
    },
    HostMethod {
        method: "lithe/host/workspace/stat",
        service: "workspace",
        permission: Some("workspace.files.read"),
        params: HostParams::WorkspaceStat,
    },
    HostMethod {
        method: "lithe/host/workspace/readFile",
        service: "workspace",
        permission: Some("workspace.files.read"),
        params: HostParams::WorkspaceReadFile,
    },
    HostMethod {
        method: "lithe/host/workspace/watch",
        service: "workspace",
        permission: Some("workspace.files.watch"),
        params: HostParams::WorkspaceWatch,
    },
    HostMethod {
        method: "lithe/host/workspace/findFiles",
        service: "workspace",
        permission: Some("workspace.search"),
        params: HostParams::WorkspaceFindFiles,
    },
    HostMethod {
        method: "lithe/host/workspace/findText",
        service: "workspace",
        permission: Some("workspace.search"),
        params: HostParams::WorkspaceFindText,
    },
    // Plan API-11: file changes the host makes on the backend's behalf. The
    // host never overwrites a file, saves an open document before it moves
    // or trashes its file, and moves to the trash instead of deleting.
    HostMethod {
        method: "lithe/host/workspace/create",
        service: "workspace",
        permission: Some(PERMISSION_WORKSPACE_FILES_WRITE),
        params: HostParams::WorkspaceCreate,
    },
    HostMethod {
        method: "lithe/host/workspace/rename",
        service: "workspace",
        permission: Some(PERMISSION_WORKSPACE_FILES_WRITE),
        params: HostParams::WorkspaceMove,
    },
    HostMethod {
        method: "lithe/host/workspace/copy",
        service: "workspace",
        permission: Some(PERMISSION_WORKSPACE_FILES_WRITE),
        params: HostParams::WorkspaceMove,
    },
    HostMethod {
        method: "lithe/host/workspace/trash",
        service: "workspace",
        permission: Some(PERMISSION_WORKSPACE_FILES_WRITE),
        params: HostParams::WorkspaceTrash,
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
    // Plan API-11: saving and closing an open document through its owner,
    // which keeps the document's version, dirty state and undo history.
    HostMethod {
        method: "lithe/host/documents/save",
        service: "documents",
        permission: Some(PERMISSION_DOCUMENTS_SAVE),
        params: HostParams::DocumentSave,
    },
    HostMethod {
        method: "lithe/host/documents/saveAs",
        service: "documents",
        permission: Some(PERMISSION_DOCUMENTS_SAVE),
        params: HostParams::DocumentSaveAs,
    },
    HostMethod {
        method: "lithe/host/documents/close",
        service: "documents",
        permission: Some(PERMISSION_DOCUMENTS_SAVE),
        params: HostParams::DocumentClose,
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
        params: HostParams::Workflow,
    },
    HostMethod {
        method: "lithe/host/browser/openExternal",
        service: "browser",
        permission: Some("browser.open"),
        params: HostParams::Workflow,
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
    // Plan API-45: the native relay of the plugin's own agent sessions, and
    // the usage totals of consented agents. Core's agent runtime answers
    // them and checks session ownership.
    HostMethod {
        method: AGENTS_WRITE,
        service: HOST_SERVICE_AGENTS,
        permission: Some(PERMISSION_AGENTS_NATIVE),
        params: HostParams::Object,
    },
    HostMethod {
        method: AGENTS_REPORT,
        service: HOST_SERVICE_AGENTS,
        permission: Some(PERMISSION_AGENTS_NATIVE),
        params: HostParams::Object,
    },
    HostMethod {
        method: AGENTS_USAGE,
        service: HOST_SERVICE_AGENTS,
        permission: Some(PERMISSION_AGENTS_USAGE),
        params: HostParams::Object,
    },
];

/// Writes lines to the standard input of one of the backend's agent CLIs.
pub const AGENTS_WRITE: &str = "lithe/host/agents/write";

/// Reports session events the backend translated from CLI output.
pub const AGENTS_REPORT: &str = "lithe/host/agents/report";

/// Reads usage and quota totals of the agents the user consented to.
pub const AGENTS_USAGE: &str = "lithe/host/agents/usage";

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
    if entry.params == HostParams::Workflow {
        workflow::check_params(method, params.unwrap_or(&Value::Null))?;
    } else {
        check_host_params(entry.params, params)?;
    }
    if entry.method == GET_CAPABILITIES {
        return Ok(HostRoute::Answer(capabilities_answer(version, granted)));
    }
    if entry.service == HOST_SERVICE_FUNCTIONS {
        return Ok(HostRoute::Relay);
    }
    if entry.service == HOST_SERVICE_AGENTS {
        return Ok(HostRoute::Agents);
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

#[cfg(test)]
mod build_configuration_tests {
    use super::*;

    #[test]
    fn the_build_methods_have_separate_grants_and_strict_contexts() {
        let get = "lithe/host/build/getOptions";
        let watch = "lithe/host/build/watchConfiguration";
        let granted = [get.to_owned()].into_iter().collect();
        let context = json!({"contextID":"context-1","generation":1});
        let mut options = json!({"context": context, "buildSystemID":"make", "root":"."});
        assert_eq!(
            host_request(get, Some(&options), Some(0), &granted).unwrap(),
            HostRoute::Forward("build")
        );
        let files = json!({"context": context, "files":[]});
        assert_eq!(
            host_request(watch, Some(&files), Some(0), &granted)
                .unwrap_err()
                .code,
            codes::HOST_PERMISSION_DENIED
        );
        options["extra"] = json!(true);
        assert_eq!(
            host_request(get, Some(&options), Some(0), &granted)
                .unwrap_err()
                .code,
            codes::HOST_UNSUPPORTED_OPTION
        );
        let granted = [watch.to_owned()].into_iter().collect();
        let wrong = json!({"context": context, "files":["f","g","h","i","j"]});
        assert_eq!(
            host_request(watch, Some(&wrong), Some(0), &granted)
                .unwrap_err()
                .code,
            codes::HOST_INVALID_PARAMS
        );
        assert_eq!(
            host_request(watch, Some(&files), Some(0), &granted).unwrap(),
            HostRoute::Forward("build")
        );
    }
}

#[cfg(test)]
mod workspace_read_tests {
    use super::*;

    #[test]
    fn workspace_read_methods_check_options_context_resources_and_bounds() {
        let context = json!({"contextID":"context-1","generation":1});
        for (method, extra) in [
            (
                "lithe/host/workspace/readDirectory",
                json!({"cursor":"opaque","limit":1000}),
            ),
            ("lithe/host/workspace/stat", json!({})),
            ("lithe/host/workspace/watch", json!({"recursive":true})),
            (
                "lithe/host/workspace/findFiles",
                json!({"query":"alpha","limit":200}),
            ),
            (
                "lithe/host/workspace/findText",
                json!({"pattern":"alpha","wholeWords":true}),
            ),
            (
                "lithe/host/workspace/readFile",
                json!({"maximumBytes":131072}),
            ),
        ] {
            let granted = [method.to_owned()].into_iter().collect();
            let mut params =
                json!({"context":context,"resource":{"rootID":"root-1","path":"src/a"}});
            if method.ends_with("findFiles") || method.ends_with("findText") {
                params["resource"]["path"] = json!("");
            }
            for (key, value) in extra.as_object().unwrap() {
                params[key] = value.clone();
            }
            assert_eq!(
                host_request(method, Some(&params), Some(0), &granted).unwrap(),
                HostRoute::Forward("workspace")
            );
            let mut unknown = params.clone();
            unknown["unknown"] = json!(true);
            assert_eq!(
                host_request(method, Some(&unknown), Some(0), &granted)
                    .unwrap_err()
                    .code,
                codes::HOST_UNSUPPORTED_OPTION
            );
            let mut unknown = params.clone();
            unknown["resource"]["unknown"] = json!(true);
            assert_eq!(
                host_request(method, Some(&unknown), Some(0), &granted)
                    .unwrap_err()
                    .code,
                codes::HOST_UNSUPPORTED_OPTION
            );
            for resource in [
                json!({"rootID":"other","path":"../escape"}),
                json!({"rootID":"","path":"a"}),
                json!({"rootID":"root-1","path":"a\\b"}),
            ] {
                let mut invalid = params.clone();
                invalid["resource"] = resource;
                assert_eq!(
                    host_request(method, Some(&invalid), Some(0), &granted)
                        .unwrap_err()
                        .code,
                    codes::HOST_INVALID_PARAMS
                );
            }
            let mut invalid = params.clone();
            invalid["context"] = Value::Null;
            assert_eq!(
                host_request(method, Some(&invalid), Some(0), &granted)
                    .unwrap_err()
                    .code,
                codes::HOST_INVALID_PARAMS
            );
        }
        for (method, key, value) in [
            ("lithe/host/workspace/readDirectory", "limit", json!(0)),
            ("lithe/host/workspace/readDirectory", "limit", json!(1001)),
            ("lithe/host/workspace/readDirectory", "cursor", json!(null)),
            (
                "lithe/host/workspace/readFile",
                "maximumBytes",
                json!(131073),
            ),
            ("lithe/host/workspace/watch", "recursive", json!("true")),
            ("lithe/host/workspace/findFiles", "limit", json!(201)),
            ("lithe/host/workspace/findText", "wholeWords", json!(1)),
            ("lithe/host/workspace/findText", "pattern", json!("")),
            (
                "lithe/host/workspace/findFiles",
                "resource",
                json!({"rootID":"r1","path":"sub"}),
            ),
        ] {
            let mut params = json!({"context":context,"resource":{"rootID":"root-1","path":"a"}});
            if method.ends_with("findFiles") || method.ends_with("findText") {
                params["resource"]["path"] = json!("");
                params[if method.ends_with("findFiles") {
                    "query"
                } else {
                    "pattern"
                }] = json!("alpha");
            }
            params[key] = value;
            assert_eq!(
                host_request(
                    method,
                    Some(&params),
                    Some(0),
                    &[method.to_owned()].into_iter().collect()
                )
                .unwrap_err()
                .code,
                codes::HOST_INVALID_PARAMS,
                "{method}: {params}"
            );
        }
    }
}
