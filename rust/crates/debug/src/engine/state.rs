//! Session state: the reduced DAP session, the requests it waits on, lifecycle
//! transitions and the update it publishes.

use super::*;

#[derive(Debug)]
pub(crate) struct DebugSession {
    pub(crate) id: String,
    pub(crate) adapter_id: String,
    /// Catalog-safe adapter identity; never derived from DAP payload or user output.
    pub(crate) log_adapter: String,
    /// A failing launch operation already owns the session failure record.
    pub(crate) logged_terminal_failure: bool,
    pub(crate) root_path: String,
    pub(crate) state: DebugSessionState,
    pub(crate) next_request_sequence: i64,
    pub(crate) next_event_sequence: u64,
    pub(crate) read_buffer: Vec<u8>,
    pub(crate) pending_requests: BTreeMap<i64, PendingRequest>,
    /// Pause generation each request was issued under; frame and variable references die with it.
    pub(crate) request_stop_generations: BTreeMap<i64, u64>,
    pub(crate) stop_generation: u64,
    pub(crate) parent_id: Option<String>,
    /// Live child sessions in creation order; the host owns their transports.
    pub(crate) child_ids: Vec<String>,
    /// Children already asked to stop, so a retry or a later `terminated` does not repeat it.
    pub(crate) stopping_children: BTreeSet<String>,
    pub(crate) breakpoints: BTreeMap<String, Vec<SourceBreakpoint>>,
    /// Rejects replies to superseded per-file breakpoint sets.
    pub(crate) breakpoint_generations: BTreeMap<String, u64>,
    /// Adapter IDs correlate location-free events with the original source request.
    pub(crate) resolved_source_breakpoints: BTreeMap<i64, DebugBreakpoint>,
    pub(crate) exception_breakpoints: Vec<ExceptionBreakpoint>,
    pub(crate) did_configure_exception_breakpoints: bool,
    pub(crate) function_breakpoints: Vec<FunctionBreakpoint>,
    pub(crate) instruction_breakpoints: Vec<InstructionBreakpoint>,
    pub(crate) data_breakpoints: Vec<DataBreakpoint>,
    /// Muted breakpoints stay stored but every kind is sent empty, so the adapter stops on
    /// none of them; a child starts muted when its parent is.
    pub(crate) breakpoints_muted: bool,
    pub(crate) supports_run_in_terminal_request: bool,
    pub(crate) pending_run_in_terminal_requests: BTreeMap<String, i64>,
    pub(crate) supports_start_debugging_request: bool,
    pub(crate) pending_start_debugging_requests: BTreeMap<String, i64>,
    pub(crate) did_receive_initialized: bool,
    pub(crate) supports_configuration_done: bool,
    pub(crate) capabilities: DebugCapabilities,
    /// Raw capability flags exactly as the adapter reported them (merged with `capabilities` events).
    pub(crate) adapter_capabilities: Map<String, Value>,
    pub(crate) disabled_capabilities: BTreeSet<String>,
    pub(crate) capability_states: BTreeMap<String, CapabilityState>,
    pub(crate) capability_revision: u64,
    pub(crate) stepping_filters: DebugSteppingFilters,
    /// DAP extensions the adapter declared; empty for an undeclared adapter.
    pub(crate) features: DebugAdapterFeatures,
    /// Declared filters used when a launch names none.
    pub(crate) default_stepping_filters: Option<DebugSteppingFilters>,
    pub(crate) debug_request_kind: Option<DebugRequestKind>,
    pub(crate) pending_launch: Option<(String, DebugLaunchConfiguration)>,
    pub(crate) outbound_frames: Vec<Vec<u8>>,
    pub(crate) events: Vec<DebugEvent>,
}

#[derive(Debug)]
pub(crate) enum PendingRequest {
    Initialize,
    Launch {
        operation_id: String,
    },
    SetBreakpoints {
        source_path: String,
        requested: Vec<SourceBreakpoint>,
        generation: u64,
    },
    SetExceptionBreakpoints,
    SetFunctionBreakpoints {
        requested: Vec<FunctionBreakpoint>,
    },
    SetInstructionBreakpoints {
        requested: Vec<InstructionBreakpoint>,
    },
    DataBreakpointInfo {
        operation_id: String,
    },
    SetDataBreakpoints {
        requested: Vec<DataBreakpoint>,
    },
    SetVariable {
        operation_id: String,
        name: String,
    },
    Cancel,
    ConfigurationDone,
    Execute {
        operation_id: String,
        command: DebugExecutionCommand,
        single_thread: bool,
    },
    Inspect {
        operation_id: String,
        kind: DebugInspectKind,
        /// Identify the v2 projection of stack traces and scopes.
        thread_id: Option<i64>,
        frame_id: Option<i64>,
    },
    Disconnect,
}

impl DebugSession {
    pub(crate) fn transition(&mut self, state: DebugSessionState) {
        if self.state == state {
            return;
        }
        self.state = state;
        let event = match state {
            DebugSessionState::Ready => "debug.session.initialized",
            DebugSessionState::Terminated => "debug.session.finished",
            DebugSessionState::Failed => {
                self.log_session_failure("adapter_rejected");
                self.emit(DebugEventBody::StateChanged { state });
                return;
            }
            _ => "debug.session.state_changed",
        };
        let fields = if event == "debug.session.state_changed" {
            BTreeMap::from([(
                "state".into(),
                serde_json::to_value(state).expect("DAP state encodes"),
            )])
        } else {
            BTreeMap::new()
        };
        self.log_fact(event, fields, Some(self.id.clone()));
        self.emit(DebugEventBody::StateChanged { state });
    }

    pub(crate) fn default_exception_breakpoints(&self) -> Vec<ExceptionBreakpoint> {
        self.capabilities
            .exception_breakpoint_filters
            .iter()
            .map(|filter| ExceptionBreakpoint {
                filter: filter.filter.clone(),
                enabled: filter.default,
                condition: None,
            })
            .collect()
    }

    /// Asks the host to stop each child once; the host owns the child transports.
    pub(crate) fn request_children_stop(&mut self) {
        for child_session_id in self.child_ids.clone() {
            if self.stopping_children.insert(child_session_id.clone()) {
                self.emit(DebugEventBody::ChildSessionStopRequested { child_session_id });
            }
        }
    }

    /// Recomputes the effective capabilities and publishes them when any state changed.
    ///
    /// With `only_changes` false the first negotiation is always published.
    pub(crate) fn refresh_capabilities(&mut self, only_changes: bool) {
        let states = capability_states(&self.adapter_capabilities, &self.disabled_capabilities);
        let capabilities = parse_capabilities(&effective_body(&self.adapter_capabilities, &states));
        // Non-boolean fields such as exception filters change without any flag flipping.
        if only_changes && states == self.capability_states && capabilities == self.capabilities {
            return;
        }
        self.capabilities = capabilities;
        self.capability_states = states;
        if only_changes && !self.did_configure_exception_breakpoints {
            self.exception_breakpoints = self.default_exception_breakpoints();
        }
        self.capability_revision += 1;
        self.emit(DebugEventBody::Capabilities {
            capabilities: self.capabilities.clone(),
        });
        if let Some(projection) = capabilities_projection(
            &self.id,
            self.stop_generation,
            self.capability_revision,
            self.capability_states.clone(),
        ) {
            self.emit(DebugEventBody::Projection { projection });
        }
    }

    pub(crate) fn log_fact(
        &self,
        event: &str,
        mut fields: BTreeMap<String, Value>,
        op: Option<String>,
    ) {
        fields.insert("adapter".into(), json!(self.log_adapter));
        if lithe_diagnostics::core_events::record(event, fields, op).is_err() {
            lithe_diagnostics::core_events::note_dropped();
        }
    }

    pub(crate) fn log_session_failure(&mut self, code: &str) {
        if self.logged_terminal_failure {
            return;
        }
        self.logged_terminal_failure = true;
        self.log_fact(
            "debug.session.failed",
            BTreeMap::from([("failure_code".into(), json!(code))]),
            Some(self.id.clone()),
        );
    }

    pub(crate) fn emit(&mut self, body: DebugEventBody) {
        if let DebugEventBody::OperationFailed {
            operation_id,
            command,
            code,
            ..
        } = &body
        {
            let (event, failure) = match code {
                DebugOperationFailureCode::Cancelled => ("debug.operation.cancelled", "cancelled"),
                DebugOperationFailureCode::StaleReference => {
                    ("debug.operation.cancelled", "stale_reference")
                }
                DebugOperationFailureCode::TimedOut => ("debug.operation.failed", "timed_out"),
                DebugOperationFailureCode::AdapterRejected => {
                    ("debug.operation.failed", "adapter_rejected")
                }
            };
            // A launch cancellation is an expected terminal too: the following
            // Failed state must not turn it into a second product error.
            self.logged_terminal_failure |= matches!(command.as_str(), "launch" | "attach");
            self.log_fact(
                event,
                BTreeMap::from([("failure_code".into(), json!(failure))]),
                Some(operation_id.clone()),
            );
        }
        let sequence = self.next_event_sequence;
        self.next_event_sequence += 1;
        self.events.push(DebugEvent { sequence, body });
    }

    pub(crate) fn take_update(&mut self) -> DebugSessionUpdate {
        DebugSessionUpdate {
            session_id: self.id.clone(),
            state: self.state,
            outbound_frames: std::mem::take(&mut self.outbound_frames)
                .into_iter()
                .map(|frame| BASE64.encode(frame))
                .collect(),
            events: std::mem::take(&mut self.events),
        }
    }
}

impl PendingRequest {
    pub(crate) fn command(&self) -> &'static str {
        match self {
            Self::Initialize => "initialize",
            Self::Launch { .. } => "launch",
            Self::SetBreakpoints { .. } => "setBreakpoints",
            Self::SetExceptionBreakpoints => "setExceptionBreakpoints",
            Self::SetFunctionBreakpoints { .. } => "setFunctionBreakpoints",
            Self::SetInstructionBreakpoints { .. } => "setInstructionBreakpoints",
            Self::DataBreakpointInfo { .. } => "dataBreakpointInfo",
            Self::SetDataBreakpoints { .. } => "setDataBreakpoints",
            Self::SetVariable { .. } => "setVariable",
            Self::Cancel => "cancel",
            Self::ConfigurationDone => "configurationDone",
            Self::Execute { command, .. } => command.command(),
            Self::Inspect { kind, .. } => kind.command(),
            Self::Disconnect => "disconnect",
        }
    }

    /// Frame IDs and variables references are only valid until the next stop or resume.
    pub(crate) fn is_reference_bound(&self) -> bool {
        match self {
            Self::SetVariable { .. } => true,
            Self::Inspect { kind, .. } => matches!(
                kind,
                DebugInspectKind::StackTrace
                    | DebugInspectKind::Scopes
                    | DebugInspectKind::Variables
                    | DebugInspectKind::Evaluate
                    | DebugInspectKind::StepInTargets
                    | DebugInspectKind::GotoTargets
                    | DebugInspectKind::Completions
                    | DebugInspectKind::Disassemble
                    | DebugInspectKind::ReadMemory
            ),
            _ => false,
        }
    }

    pub(crate) fn operation_id(&self) -> Option<&str> {
        match self {
            Self::Launch { operation_id }
            | Self::Execute { operation_id, .. }
            | Self::Inspect { operation_id, .. }
            | Self::DataBreakpointInfo { operation_id }
            | Self::SetVariable { operation_id, .. } => Some(operation_id),
            _ => None,
        }
    }
}
