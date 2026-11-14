//! Host notifications Core sends to backends.

use super::*;

/// Lifecycle of a language server the plugin declares.
pub(crate) const LANGUAGE_SERVER_DID_CHANGE_STATE: &str =
    "lithe/host/languageServer/didChangeState";

/// A server notification the language server's `forwardNotifications` lists.
pub(crate) const LANGUAGE_SERVER_DID_RECEIVE_NOTIFICATION: &str =
    "lithe/host/languageServer/didReceiveNotification";

/// How Core checks the params of one host-to-backend notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NotificationParams {
    /// Exactly `{ revision }`, a positive integer.
    Revision,
    /// Exactly `{ subscriptionID, sequence, event }`: the subscription the
    /// backend holds, a positive sequence that grows per subscription, and
    /// the service's event object.
    SubscriptionEvent,
    /// `{ commandID, context? }`: one of the plugin's own commands and the
    /// workspace context of the window it ran in.
    Command,
    /// `{ runID, exitCode?, signal? }`: a Run session the backend started
    /// ended with an exit status, a signal, or neither when the host lost it.
    RunExit,
    /// `{ context, serverID, state, generation }`: a lifecycle change of one
    /// of the plugin's language servers.
    LanguageServerState,
    /// `{ context, serverID, generation, method, params? | truncated }`: a
    /// forwarded server notification, without params when too large.
    LanguageServerNotification,
}

/// One row of the host-notification table.
pub(crate) struct HostNotification {
    pub method: &'static str,
    /// Service whose grant admits sending it; implicit services need none.
    pub service: &'static str,
    /// Host method whose individual grant admits it instead of `service`,
    /// for a notification that reports on a method granted per permission.
    pub grant: Option<&'static str>,
    pub params: NotificationParams,
}

impl HostNotification {
    /// Whether the session's grants admit sending this notification.
    pub(crate) fn admitted(&self, granted: &BTreeSet<String>) -> bool {
        match self.grant {
            Some(method) => granted.contains(method),
            None => is_implicit_service(self.service) || holds_service(self.service, granted),
        }
    }
}

const WORKSPACE_DID_CHANGE_CONTEXTS: &str = "lithe/host/workspace/didChangeContexts";

pub(crate) const SUBSCRIPTIONS_NOTIFY: &str = "lithe/host/subscriptions/notify";

const COMMANDS_DID_RUN: &str = "lithe/host/commands/didRun";

/// Starts a host-owned Run session; its grant also admits `RUN_DID_EXIT`.
pub(crate) const RUN_START: &str = "lithe/host/run/start";

const RUN_DID_EXIT: &str = "lithe/host/run/didExit";

/// Host-owned Run sessions a backend starts and stops.
pub(crate) const HOST_SERVICE_RUN: &str = "run";

/// Longest opaque run ID a host may hand out.
const MAX_RUN_ID_BYTES: usize = 128;

/// The plugin's own UI commands reaching its backend. Every negotiated
/// plugin-owned session has it; a command names only the plugin's own IDs.
pub(crate) const HOST_SERVICE_COMMANDS: &str = "commands";

/// Every host-to-backend notification the host may send through
/// `lsp.hostNotify`. A notification missing here never reaches a backend,
/// so the host cannot use the channel to call into the backend's own
/// protocol (functions, build or VCS methods).
pub(crate) const HOST_NOTIFICATIONS: &[HostNotification] = &[
    HostNotification {
        method: WORKSPACE_DID_CHANGE_CONTEXTS,
        service: HOST_SERVICE_WORKSPACE,
        grant: None,
        params: NotificationParams::Revision,
    },
    HostNotification {
        method: SUBSCRIPTIONS_NOTIFY,
        service: HOST_SERVICE_SUBSCRIPTIONS,
        grant: None,
        params: NotificationParams::SubscriptionEvent,
    },
    HostNotification {
        method: COMMANDS_DID_RUN,
        service: HOST_SERVICE_COMMANDS,
        grant: None,
        params: NotificationParams::Command,
    },
    HostNotification {
        method: RUN_DID_EXIT,
        service: HOST_SERVICE_RUN,
        grant: Some(RUN_START),
        params: NotificationParams::RunExit,
    },
    HostNotification {
        method: LANGUAGE_SERVER_DID_CHANGE_STATE,
        service: HOST_SERVICE_LANGUAGE_SERVER,
        grant: None,
        params: NotificationParams::LanguageServerState,
    },
    HostNotification {
        method: LANGUAGE_SERVER_DID_RECEIVE_NOTIFICATION,
        service: HOST_SERVICE_LANGUAGE_SERVER,
        grant: None,
        params: NotificationParams::LanguageServerNotification,
    },
];

/// Whether the session holds a service: its aggregate grant, or, for a
/// service granted per method, any of its methods.
fn holds_service(service: &str, granted: &BTreeSet<String>) -> bool {
    granted.contains(service)
        || HOST_METHODS.iter().any(|entry| {
            entry.service == service && entry.permission.is_some() && granted.contains(entry.method)
        })
}

/// A language server state a `didChangeState` notification may carry.
fn is_language_server_state(value: &str) -> bool {
    matches!(value, "starting" | "running" | "stopped" | "crashed")
}

/// Whether a host notification reaches this session. `Ok(false)` means the
/// backend negotiated no protocol version (`version` is `None`), so the host
/// may skip it; a method outside the table, one whose service the session
/// was not granted, or malformed params are the host's mistake and refused.
pub fn host_notification(
    method: &str,
    params: &Value,
    version: Option<u64>,
    granted: &BTreeSet<String>,
) -> Result<bool, CoreError> {
    let invalid = |message: &str| {
        refusal(
            ErrorCode::InvalidRequest,
            message,
            codes::HOST_INVALID_PARAMS,
        )
    };
    let Some(entry) = HOST_NOTIFICATIONS
        .iter()
        .find(|entry| entry.method == method)
    else {
        return Err(refusal(
            ErrorCode::InvalidRequest,
            "The host may not send this notification to a plugin process.",
            codes::HOST_UNAVAILABLE,
        ));
    };
    if !entry.admitted(granted) {
        return Err(refusal(
            ErrorCode::InvalidRequest,
            "The session was not granted the service of this notification.",
            codes::HOST_PERMISSION_DENIED,
        ));
    }
    let Some(object) = params.as_object() else {
        return Err(invalid("Notification params must be an object."));
    };
    if serde_json::to_vec(params).map_or(true, |bytes| bytes.len() > MAX_HOST_NOTIFICATION_BYTES) {
        return Err(invalid(
            "Notification params exceed the notification bound.",
        ));
    }
    let positive = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_u64)
            .is_some_and(|value| value >= 1)
    };
    let only = |keys: &[&str]| object.keys().all(|key| keys.contains(&key.as_str()));
    let well_formed = match entry.params {
        NotificationParams::Revision => only(&["revision"]) && positive("revision"),
        NotificationParams::SubscriptionEvent => {
            only(&["subscriptionID", "sequence", "event"])
                && object
                    .get("subscriptionID")
                    .and_then(Value::as_str)
                    .is_some_and(is_credential_key)
                && positive("sequence")
                && object.get("event").is_some_and(Value::is_object)
        }
        NotificationParams::Command => {
            only(&["commandID", "context"])
                && object
                    .get("commandID")
                    .and_then(Value::as_str)
                    .is_some_and(is_command_id)
                && object.get("context").is_none_or(is_context_reference)
        }
        NotificationParams::RunExit => {
            only(&["runID", "exitCode", "signal"])
                && object
                    .get("runID")
                    .and_then(Value::as_str)
                    .is_some_and(|id| id.len() <= MAX_RUN_ID_BYTES && is_credential_key(id))
                && object.get("exitCode").is_none_or(|code| {
                    code.as_i64()
                        .is_some_and(|code| i32::try_from(code).is_ok())
                })
                && object.get("signal").is_none_or(|signal| {
                    signal
                        .as_u64()
                        .is_some_and(|signal| (1..=127).contains(&signal))
                })
        }
        NotificationParams::LanguageServerState => {
            only(&["context", "serverID", "state", "generation"])
                && object.get("context").is_some_and(is_context_reference)
                && object
                    .get("serverID")
                    .and_then(Value::as_str)
                    .is_some_and(is_protocol_name)
                && object
                    .get("state")
                    .and_then(Value::as_str)
                    .is_some_and(is_language_server_state)
                && positive("generation")
        }
        NotificationParams::LanguageServerNotification => {
            let truncated = object.get("truncated");
            only(&[
                "context",
                "serverID",
                "generation",
                "method",
                "params",
                "truncated",
            ]) && object.get("context").is_some_and(is_context_reference)
                && object
                    .get("serverID")
                    .and_then(Value::as_str)
                    .is_some_and(is_protocol_name)
                && object
                    .get("method")
                    .and_then(Value::as_str)
                    .is_some_and(is_protocol_name)
                && positive("generation")
                // A truncated notification drops its params entirely.
                && match truncated {
                    None => true,
                    Some(Value::Bool(true)) => !object.contains_key("params"),
                    Some(_) => false,
                }
        }
    };
    if !well_formed {
        return Err(invalid(
            "Notification params do not match the notification.",
        ));
    }
    Ok(version.is_some())
}
