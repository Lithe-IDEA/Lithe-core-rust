//! Stateful DAP reducer whose byte transport and process lifecycle remain platform owned.

use super::capabilities::{
    capability_states, client_capabilities, effective_body, validate_disabled, HostFeatures,
};
use super::projection_v2::{
    capabilities_projection, scopes_projection, stack_trace_projection, CapabilityState,
};
use super::protocol::{frame_message, parse_messages};
use super::types::*;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use lithe_diagnostics::channel::identity_segment;
use lithe_protocol::{CoreError, ErrorCode};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Mutex, OnceLock};

mod breakpoints;
mod execution;
mod fields;
mod inspection;
mod messages;
mod state;
mod step_filters;

pub use breakpoints::*;
pub use execution::*;
pub(crate) use fields::*;
pub use inspection::*;
use messages::*;
use state::*;
use step_filters::*;

static SESSIONS: OnceLock<Mutex<HashMap<String, DebugSession>>> = OnceLock::new();

/// Creates a session and returns the framed DAP `initialize` request to send.
pub fn create_session(request: CreateSessionRequest) -> Result<DebugSessionUpdate, CoreError> {
    validate_identifier(&request.session_id, "sessionId")?;
    validate_identifier(&request.adapter_id, "adapterId")?;
    validate_path(&request.root_path, "rootPath")?;
    let disabled_capabilities = validate_disabled(&request.disabled_capabilities)?;
    request
        .declared
        .check()
        .map_err(|(field, message)| invalid_request(message).with_details(field.to_string()))?;
    let DeclaredAdapterBehavior { features, defaults } = request.declared;
    let mut sessions = sessions_lock()?;
    if sessions.contains_key(&request.session_id) {
        return Err(invalid_request(
            "A debug session with this sessionId already exists.",
        ));
    }
    let mut breakpoints_muted = false;
    if let Some(parent_id) = &request.parent_session_id {
        validate_identifier(parent_id, "parentSessionId")?;
        let parent = sessions
            .get(parent_id)
            .ok_or_else(|| session_not_found(parent_id))?;
        if matches!(
            parent.state,
            DebugSessionState::Terminating | DebugSessionState::Terminated
        ) {
            return Err(invalid_request(
                "A child debug session needs a running parent session.",
            ));
        }
        breakpoints_muted = parent.breakpoints_muted;
    }
    let mut session = DebugSession {
        id: request.session_id.clone(),
        // Same mapping as plugin facts, so one adapter id names one channel.
        log_adapter: identity_segment(&request.adapter_id),
        logged_terminal_failure: false,
        adapter_id: request.adapter_id,
        root_path: request.root_path,
        state: DebugSessionState::Idle,
        next_request_sequence: 1,
        next_event_sequence: 1,
        read_buffer: Vec::new(),
        pending_requests: BTreeMap::new(),
        request_stop_generations: BTreeMap::new(),
        stop_generation: 0,
        parent_id: request.parent_session_id.clone(),
        child_ids: Vec::new(),
        stopping_children: BTreeSet::new(),
        breakpoints: BTreeMap::new(),
        breakpoint_generations: BTreeMap::new(),
        resolved_source_breakpoints: BTreeMap::new(),
        exception_breakpoints: Vec::new(),
        did_configure_exception_breakpoints: false,
        function_breakpoints: Vec::new(),
        instruction_breakpoints: Vec::new(),
        data_breakpoints: Vec::new(),
        breakpoints_muted,
        supports_run_in_terminal_request: request.supports_run_in_terminal_request,
        pending_run_in_terminal_requests: BTreeMap::new(),
        supports_start_debugging_request: request.supports_start_debugging_request,
        pending_start_debugging_requests: BTreeMap::new(),
        did_receive_initialized: false,
        supports_configuration_done: false,
        capabilities: DebugCapabilities::default(),
        adapter_capabilities: Map::new(),
        disabled_capabilities,
        capability_states: BTreeMap::new(),
        capability_revision: 0,
        stepping_filters: DebugSteppingFilters::unfiltered(),
        features: features.unwrap_or_default(),
        default_stepping_filters: defaults
            .and_then(|defaults| defaults.step_filters)
            .map(|filters| DebugSteppingFilters::from(&filters)),
        debug_request_kind: None,
        pending_launch: None,
        outbound_frames: Vec::new(),
        events: Vec::new(),
    };
    session.log_fact(
        "debug.process.started",
        BTreeMap::new(),
        Some(session.id.clone()),
    );
    session.transition(DebugSessionState::Initializing);
    let mut initialize = json!({
        "clientID": "lithe",
        "clientName": "Lithe",
        "adapterID": session.adapter_id,
        "linesStartAt1": true,
        "columnsStartAt1": true,
        "pathFormat": "path",
        "supportsVariableType": true,
        "supportsVariablePaging": true
    });
    if let (Some(arguments), Value::Object(client)) = (
        initialize.as_object_mut(),
        client_capabilities(HostFeatures {
            run_in_terminal: session.supports_run_in_terminal_request,
            start_debugging: session.supports_start_debugging_request,
        }),
    ) {
        arguments.extend(client);
    }
    session.send_request("initialize", initialize, PendingRequest::Initialize)?;
    let update = session.take_update();
    if let Some(parent) = session
        .parent_id
        .as_ref()
        .and_then(|parent_id| sessions.get_mut(parent_id))
    {
        // The host created the child, so it needs no attach event.
        parent.child_ids.push(session.id.clone());
    }
    sessions.insert(request.session_id, session);
    Ok(update)
}

/// Queues launch or attach now, or stores it until initialize completes.
pub fn launch(request: LaunchRequest) -> Result<DebugSessionUpdate, CoreError> {
    validate_identifier(&request.operation_id, "operationId")?;
    with_session(&request.session_id, |session| {
        match session.state {
            DebugSessionState::Initializing => {
                session.pending_launch = Some((request.operation_id, request.configuration));
            }
            DebugSessionState::Ready => {
                session.perform_launch(request.operation_id, request.configuration)?;
            }
            _ => {
                return Err(invalid_request(
                    "The debug session is not ready to launch or attach.",
                ))
            }
        }
        Ok(session.take_update())
    })
}

/// Returns deterministic adapter defaults or a normalized native-client override.
pub fn stepping_filters(
    request: DebugSteppingFiltersRequest,
) -> Result<DebugSteppingFilters, CoreError> {
    validate_identifier(&request.adapter_id, "adapterId")?;
    normalize_stepping_filters(
        request
            .filters
            .unwrap_or_else(DebugSteppingFilters::unfiltered),
    )
}

/// Ends one caller-owned operation and ignores any later adapter response.
pub fn cancel_operation(request: CancelOperationRequest) -> Result<DebugSessionUpdate, CoreError> {
    validate_identifier(&request.operation_id, "operationId")?;
    with_session(&request.session_id, |session| {
        let pending_sequence = session
            .pending_requests
            .iter()
            .find_map(|(sequence, pending)| {
                (pending.operation_id() == Some(request.operation_id.as_str())).then_some(*sequence)
            });
        let Some(pending_sequence) = pending_sequence else {
            return Ok(session.take_update());
        };
        let pending = session
            .pending_requests
            .remove(&pending_sequence)
            .expect("located pending debug operation should still exist");
        let command = pending.command().to_string();
        let message = match request.reason {
            DebugCancellationReason::Cancelled => "Debug operation was cancelled.",
            DebugCancellationReason::TimedOut => "Debug operation timed out.",
        };
        session.emit(DebugEventBody::OperationFailed {
            operation_id: request.operation_id,
            command,
            code: match request.reason {
                DebugCancellationReason::Cancelled => DebugOperationFailureCode::Cancelled,
                DebugCancellationReason::TimedOut => DebugOperationFailureCode::TimedOut,
            },
            message: message.to_string(),
        });
        if matches!(pending, PendingRequest::Launch { .. }) {
            session.transition(DebugSessionState::Failed);
        }
        if session.capabilities.supports_cancel_request {
            session.send_request(
                "cancel",
                json!({"requestId": pending_sequence}),
                PendingRequest::Cancel,
            )?;
        }
        Ok(session.take_update())
    })
}

/// Reduces one transport byte chunk and returns ordered writes and events.
pub fn receive(request: ReceiveRequest) -> Result<DebugSessionUpdate, CoreError> {
    let bytes = BASE64.decode(request.data_base64).map_err(|error| {
        invalid_request("Debug transport dataBase64 was invalid.").with_details(error.to_string())
    })?;
    with_session(&request.session_id, |session| {
        let messages = match parse_messages(&mut session.read_buffer, &bytes) {
            Ok(messages) => messages,
            Err(error) => {
                session.log_session_failure("protocol");
                session.transition(DebugSessionState::Failed);
                return Err(error);
            }
        };
        for message in messages {
            if let Err(error) = session.handle_message(message) {
                session.log_session_failure("protocol");
                session.transition(DebugSessionState::Failed);
                return Err(error);
            }
        }
        Ok(session.take_update())
    })
}

/// Completes one adapter-originated terminal launch and emits its DAP response.
pub fn run_in_terminal_response(
    request: DebugRunInTerminalResponseRequest,
) -> Result<DebugSessionUpdate, CoreError> {
    validate_identifier(&request.request_id, "requestId")?;
    with_session(&request.session_id, |session| {
        let Some(&request_sequence) = session
            .pending_run_in_terminal_requests
            .get(&request.request_id)
        else {
            // Native terminal creation is asynchronous. A response that races
            // session shutdown, timeout, or a previous completion must not be
            // applied to a later adapter request.
            return Ok(session.take_update());
        };

        if request.success {
            validate_dap_process_id(request.process_id, "processId")?;
            validate_dap_process_id(request.shell_process_id, "shellProcessId")?;
        }
        session
            .pending_run_in_terminal_requests
            .remove(&request.request_id);

        if request.success {
            let mut body = Map::new();
            insert_option(&mut body, "processId", request.process_id);
            insert_option(&mut body, "shellProcessId", request.shell_process_id);
            session.send_response_with_body(
                request_sequence,
                "runInTerminal",
                true,
                None,
                Some(Value::Object(body)),
            )?;
        } else {
            let message = normalize_optional_text(request.message)
                .unwrap_or_else(|| "Lithe could not start the debuggee terminal.".to_string());
            session.send_response_with_body(
                request_sequence,
                "runInTerminal",
                false,
                Some(&message),
                None,
            )?;
        }
        Ok(session.take_update())
    })
}

/// Completes one adapter-originated child session start and emits its DAP response.
pub fn start_debugging_response(
    request: DebugStartDebuggingResponseRequest,
) -> Result<DebugSessionUpdate, CoreError> {
    validate_identifier(&request.request_id, "requestId")?;
    with_session(&request.session_id, |session| {
        let Some(request_sequence) = session
            .pending_start_debugging_requests
            .remove(&request.request_id)
        else {
            // The child start is asynchronous, so a reply can race shutdown or a
            // previous completion and must not answer a later adapter request.
            return Ok(session.take_update());
        };
        if request.success {
            session.send_response(request_sequence, "startDebugging", true, None)?;
        } else {
            let message = normalize_optional_text(request.message)
                .unwrap_or_else(|| "Lithe could not start the child debug session.".to_string());
            session.send_response(request_sequence, "startDebugging", false, Some(&message))?;
        }
        Ok(session.take_update())
    })
}

/// Begins a graceful DAP disconnect while the host keeps transport ownership.
pub fn disconnect(request: DisconnectRequest) -> Result<DebugSessionUpdate, CoreError> {
    with_session(&request.session_id, |session| {
        if matches!(
            session.state,
            DebugSessionState::Terminating | DebugSessionState::Terminated
        ) {
            return Ok(session.take_update());
        }
        session.fail_pending_run_in_terminal_requests(
            "The debug session stopped before the terminal could start.",
        )?;
        session.fail_pending_start_debugging_requests(
            "The debug session stopped before the child session could start.",
        )?;
        session.request_children_stop();
        // An attach session does not own the remote process, so disconnect must
        // never terminate it. A launch session owns the local debuggee and
        // follows the user's explicit choice, defaulting to termination.
        let terminate_debuggee = match session.debug_request_kind {
            Some(DebugRequestKind::Launch) => request.terminate_debuggee.unwrap_or(true),
            Some(DebugRequestKind::Attach) | None => false,
        };
        session.send_request(
            "disconnect",
            json!({"restart": false, "terminateDebuggee": terminate_debuggee}),
            PendingRequest::Disconnect,
        )?;
        session.transition(DebugSessionState::Terminating);
        Ok(session.take_update())
    })
}

/// Removes a session and its whole child tree, returning the removed descendant IDs.
///
/// A child never outlives its parent, so destroying a parent also removes every
/// descendant (leaves first). The host owns the transports and must close the transport of
/// each returned ID; the list is empty for a session without children.
pub fn destroy_session(request: SessionRequest) -> Result<Vec<String>, CoreError> {
    let mut sessions = sessions_lock()?;
    let Some(mut removed) = sessions.remove(&request.session_id) else {
        return Err(session_not_found(&request.session_id));
    };
    removed.transition(DebugSessionState::Terminated);
    if let Some(parent) = removed
        .parent_id
        .as_ref()
        .and_then(|parent_id| sessions.get_mut(parent_id))
    {
        parent.child_ids.retain(|id| id != &removed.id);
        parent.stopping_children.remove(&removed.id);
    }
    let mut descendants = Vec::new();
    let mut pending = removed.child_ids;
    while let Some(child_id) = pending.pop() {
        if let Some(mut child) = sessions.remove(&child_id) {
            child.transition(DebugSessionState::Terminated);
            pending.extend(child.child_ids);
            descendants.push(child_id);
        }
    }
    descendants.reverse();
    Ok(descendants)
}

impl DebugSession {
    fn perform_launch(
        &mut self,
        operation_id: String,
        configuration: DebugLaunchConfiguration,
    ) -> Result<(), CoreError> {
        let request_kind = configuration.request;
        let mut arguments = configuration.arguments;
        let filters = normalize_stepping_filters(
            configuration
                .stepping_filters
                .or_else(|| self.default_stepping_filters.clone())
                .unwrap_or_else(DebugSteppingFilters::unfiltered),
        )?;
        // Only an adapter that declared the argument receives the filters; the
        // engine never invents launch arguments for an undeclared adapter.
        if let Some(feature) = &self.features.step_filters {
            if !arguments.contains_key(&feature.argument) {
                arguments.insert(feature.argument.clone(), step_filter_argument(&filters));
            }
        }
        self.stepping_filters = filters;
        arguments
            .entry("name".to_string())
            .or_insert(Value::String(configuration.name));
        arguments
            .entry("cwd".to_string())
            .or_insert(Value::String(self.root_path.clone()));
        self.debug_request_kind = Some(request_kind);
        self.transition(DebugSessionState::Launching);
        self.send_request(
            request_kind.command(),
            Value::Object(arguments),
            PendingRequest::Launch { operation_id },
        )
    }
}

fn with_session<T>(
    session_id: &str,
    operation: impl FnOnce(&mut DebugSession) -> Result<T, CoreError>,
) -> Result<T, CoreError> {
    let mut sessions = sessions_lock()?;
    let session = sessions
        .get_mut(session_id)
        .ok_or_else(|| session_not_found(session_id))?;
    operation(session)
}

fn sessions_lock(
) -> Result<std::sync::MutexGuard<'static, HashMap<String, DebugSession>>, CoreError> {
    SESSIONS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| CoreError::new(ErrorCode::Unknown, "Debug session state is unavailable."))
}

#[cfg(test)]
mod tests;
