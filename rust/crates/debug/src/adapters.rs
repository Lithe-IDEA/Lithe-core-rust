//! `debugAdapters[]` of an API level 4 plugin manifest: what a host must know
//! about a debug adapter before starting any process for it.
//!
//! A declaration names the adapter, how its process is started and reached
//! (stdio, a loopback TCP endpoint, or a port the plugin's own language server
//! opens on request), and optional matching, default DAP arguments and
//! capability suppression. It carries no behavior: there is no
//! resolver, workflow or host integration, so a plugin can only choose among
//! facts Core already understands. DAP capabilities belong to the running
//! adapter and are negotiated in `initialize`; `capabilities.disabled` can
//! only switch standard ones off.
//!
//! Validation has three layers. `validate_declarations` checks the manifest
//! on its own. `check_package_resources` checks the declaration against the
//! files of the installed package. `plan` checks one debug start against the
//! facts of that moment (target, registration and runtime generation).
//!
//! Note: the decision is recorded in .agents/notes/proposed/architecture/2026-10-25-unified-debug-engine-and-adapter-registry.md

use super::types::{DebugAdapterDefaults, DebugAdapterFeatures, DeclaredAdapterBehavior};
use lithe_plugin_model::process::{validate_process_arguments, ServerProcess, ServerRole};
use lithe_plugin_model::settings::DeclaredSettings;
use lithe_plugin_model::{invalid, is_safe_relative_path, PluginRejection, Validation};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeSet, HashSet};
use std::net::IpAddr;

mod checks;
mod declaration;
mod plan;
mod process;
mod resources;
mod start;
mod validate_adapter;
mod validate_configuration;
mod validate_launch;

pub(crate) use checks::*;
pub use declaration::*;
pub use plan::*;
pub use process::*;
pub use resources::*;
pub use start::*;
pub use validate_adapter::*;

/// Most adapters one plugin may declare.
const MAX_ADAPTERS: usize = 16;

/// Longest display name or DAP adapter ID, in characters.
const MAX_NAME: usize = 128;

/// Longest adapter-local ID, in characters.
const MAX_LOCAL_ID: usize = 64;

/// Most selectors one adapter may declare.
const MAX_SELECTORS: usize = 16;

/// Most imported `launch.json` type aliases one adapter may declare.
const MAX_CONFIGURATION_TYPES: usize = 16;

/// Longest announced-endpoint prefix, in bytes.
const MAX_ENDPOINT_PREFIX: usize = 256;

/// Longest language server command ID, in characters.
const MAX_ENDPOINT_COMMAND: usize = 128;

/// Most console commands one adapter may declare.
const MAX_CONSOLE_COMMANDS: usize = 1024;

/// Longest console command name, in characters.
const MAX_CONSOLE_COMMAND_NAME: usize = 128;

/// Longest console command description, in characters.
const MAX_CONSOLE_COMMAND_DESCRIPTION: usize = 240;

/// Longest console command prefix, in characters.
const MAX_CONSOLE_COMMAND_PREFIX: usize = 8;

/// Most places an executable lookup may list.
const MAX_EXECUTABLE_CANDIDATES: usize = 8;

/// Longest executable candidate, in characters.
const MAX_EXECUTABLE_CANDIDATE: usize = 1024;

/// The `targetBindings.program` source that reads the executable the host
/// resolved, instead of the target's own path.
const TARGET_EXECUTABLE: &str = "target.executable";

/// The target kind of a remote `host:port` the debuggee listens on.
const TARGET_CONNECTION: &str = "connection";

/// Most names an argument path joins, for example `connect.host`.
const MAX_ARGUMENT_PATH_DEPTH: usize = 4;

/// Longest single name in an argument path, in characters.
const MAX_ARGUMENT_NAME: usize = 64;

/// Most path mappings one remote target may carry.
const MAX_PATH_MAPPINGS: usize = 16;

/// Longest local or remote root of a path mapping, in characters.
const MAX_PATH_MAPPING_ROOT: usize = 1024;

/// Longest host name, as DNS limits it.
const MAX_HOST: usize = 253;

/// Longest label of a host name, as DNS limits it.
const MAX_HOST_LABEL: usize = 63;

/// Variables an executable candidate may name, and the only ones the host
/// can answer: the workspace root and the open source file's location.
const EXECUTABLE_VARIABLES: &[&str] = &[
    "workspaceFolder",
    "fileDirectory",
    "fileBasenameNoExtension",
];

/// What a candidate may start with, so a lookup stays beside the user's work.
const EXECUTABLE_ROOTS: &[&str] = &["${workspaceFolder}/", "${fileDirectory}/"];

/// Target kinds a selector may match.
const TARGET_KINDS: &[&str] = &["connection", "executable", "file", "process", "runTarget"];

/// `process` keys of a debug adapter launch. `initializationOptions` is an
/// LSP field and has no meaning for a DAP transport.
const PROCESS_KEYS: &[&str] = &[
    "launch",
    "arguments",
    "validationArguments",
    "runtimeDependencies",
];

/// Standard DAP `Capabilities` booleans plus the exception filter list; the
/// only names `capabilities.disabled` accepts.
const DAP_CAPABILITIES: &[&str] = &[
    "exceptionBreakpointFilters",
    "supportSuspendDebuggee",
    "supportTerminateDebuggee",
    "supportsANSIStyling",
    "supportsBreakpointLocationsRequest",
    "supportsCancelRequest",
    "supportsClipboardContext",
    "supportsCompletionsRequest",
    "supportsConditionalBreakpoints",
    "supportsConfigurationDoneRequest",
    "supportsDataBreakpointBytes",
    "supportsDataBreakpoints",
    "supportsDelayedStackTraceLoading",
    "supportsDisassembleRequest",
    "supportsEvaluateForHovers",
    "supportsExceptionFilterOptions",
    "supportsExceptionInfoRequest",
    "supportsExceptionOptions",
    "supportsFunctionBreakpoints",
    "supportsGotoTargetsRequest",
    "supportsHitConditionalBreakpoints",
    "supportsInstructionBreakpoints",
    "supportsLoadedSourcesRequest",
    "supportsLogPoints",
    "supportsModulesRequest",
    "supportsReadMemoryRequest",
    "supportsRestartFrame",
    "supportsRestartRequest",
    "supportsSetExpression",
    "supportsSetVariable",
    "supportsSingleThreadExecutionRequests",
    "supportsStepBack",
    "supportsStepInTargetsRequest",
    "supportsSteppingGranularity",
    "supportsTerminateRequest",
    "supportsTerminateThreadsRequest",
    "supportsValueFormattingOptions",
    "supportsWriteMemoryRequest",
];

/// What validation needs from the manifest around the declarations.
pub struct DeclarationContext<'a> {
    /// Settings the manifest declares, which process arguments and the executable
    /// lookup may reference.
    pub declared_settings: &'a DeclaredSettings,
    /// Language IDs of the plugin's `languageServers[]`.
    pub language_servers: &'a BTreeSet<String>,
    /// Whether tool names are host runtimes or tool-definition IDs.
    pub runtime_tools: lithe_plugin_model::process::RuntimeTools<'a>,
}

/// Validates every declaration and sorts them by ID.
pub fn validate_declarations(
    adapters: &mut [DebugAdapterDeclaration],
    context: &DeclarationContext<'_>,
) -> Validation<()> {
    if adapters.is_empty() {
        return Ok(());
    }
    if adapters.len() > MAX_ADAPTERS {
        return Err(invalid(
            "debugAdapters",
            format!("A plugin declares at most {MAX_ADAPTERS} debug adapters."),
        ));
    }
    let mut ids = BTreeSet::new();
    for (index, adapter) in adapters.iter_mut().enumerate() {
        let field = format!("debugAdapters[{index}]");
        adapter.validate(&field, context)?;
        if !ids.insert(adapter.id.clone()) {
            return Err(invalid(
                format!("{field}.id"),
                "A debug adapter is declared more than once.",
            ));
        }
    }
    adapters.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(())
}
