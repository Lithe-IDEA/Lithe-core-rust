//! Run configuration generation, resolution, and launch planning.

mod argument_text;
mod configuration;
mod context_selection;
pub mod generated_layer;
mod provider_run_options;
mod provider_runs;
mod scan;
mod title_action;
mod types;

pub use argument_text::{format_argument_text, ArgumentTextRequest};
pub use configuration::*;
pub use context_selection::{select_context_configuration, ContextSelectionRequest};
pub use scan::{scan_ignoring, DirectoryContext};
pub use title_action::{decide_title_action, TitleActionRequest};
pub use types::Execution;

pub mod workspace_scan;
pub use workspace_scan::IgnoredPaths;

mod environment;
pub use environment::{
    resolve_launch_environment, resolve_search_path, LaunchEnvironmentRequest,
    LaunchEnvironmentResponse, SearchPathCandidate, SearchPathRequest,
};
