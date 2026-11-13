//! Transport-neutral Debug Adapter Protocol state and normalized debugger models.

pub mod adapters;
mod backend_start;
mod breakpoint_relocation;
mod capabilities;
mod disassembly_paging;
mod endpoint;
mod engine;
mod memory_window;
mod projection_v2;
mod protocol;
mod shortcut;
mod transport_lifecycle;
mod types;
mod variable_paging;

pub use backend_start::decode_backend_configuration_command;
pub use breakpoint_relocation::{relocate_breakpoints, RelocateBreakpointsRequest};
pub use disassembly_paging::{plan_disassembly_window, reduce_disassembly_page};
pub use endpoint::validate_adapter_endpoint;
pub use engine::{
    cancel_operation, create_session, data_breakpoint_info, destroy_session, disconnect, execute,
    inspect, launch, receive, run_in_terminal_response, set_breakpoints, set_breakpoints_muted,
    set_data_breakpoints, set_exception_breakpoints, set_function_breakpoints,
    set_instruction_breakpoints, set_variable, start_debugging_response, stepping_filters,
};
pub use memory_window::{locate_memory_window, plan_memory_window};
pub use shortcut::start_or_continue;
pub use transport_lifecycle::{handle_transport_event, release_transport, transport_policy};
pub use types::{
    CancelOperationRequest, CreateSessionRequest, DataBreakpointInfoRequest,
    DebugRunInTerminalResponseRequest, DebugStartDebuggingResponseRequest,
    DebugSteppingFiltersRequest, DisconnectRequest, ExecuteRequest, InspectRequest, LaunchRequest,
    ReceiveRequest, SessionRequest, SetBreakpointsRequest, SetDataBreakpointsRequest,
    SetExceptionBreakpointsRequest, SetFunctionBreakpointsRequest,
    SetInstructionBreakpointsRequest, SetVariableRequest,
};
pub use variable_paging::{plan_variable_page, reduce_variable_page};

use lithe_plugin_model::PluginManifest;

/// What `debug.decodeBackendConfiguration` reads from one plugin manifest that
/// the plugin host validated; the host's manifest type implements it.
pub trait DebugManifest: PluginManifest {
    /// The manifest's validated `debugAdapters`.
    fn debug_adapters(&self) -> &[adapters::DebugAdapterDeclaration];
}
