//! Portable AI CLI configuration parsing and HTTP request planning; hosts own I/O and secrets.
//!
//! This crate holds every `ai.*` command of Lithe Core: commit message generation, pull
//! request descriptions, inline edits and AI CLI configuration import. It plans HTTP requests
//! and decodes provider responses; it never sends a request, reads a file or holds a
//! credential. Hosts resolve credentials, perform the request and pass the response back.
//!
//! `lithe-core` links it as a normal dependency and routes exactly [`COMMANDS`] here through
//! its JSON dispatch, so hosts reach these commands over the usual Core channel. Requests and
//! responses reuse the Core envelope: `{id, command, payload}` in and `{id, ok, data | error}`
//! out, with the same stable error codes.
//!
//! Contracts: `rust/contracts/ai-commit.md`, `rust/contracts/ai-generation-requests.md` and the
//! "AI configuration" section of `rust/contracts/rust-core-api.md`. Fixtures live in
//! `rust/fixtures/ai/`.

mod agent_configuration;
mod commands;
mod configuration;
mod dispatch;
mod error;
mod generation;
mod inline_edit;
mod provider_request;
mod pull_request;

pub use dispatch::{execute, execute_json, is_ai_command, COMMANDS};
pub use error::{AiError, AiErrorCode};
use generation::plan_commit_bounded;
pub use generation::{commit_input_budget, sensitive_path as is_sensitive_commit_path};

pub use configuration::{
    configuration_sources, detect, is_configured, parse_claude, parse_codex, ConfigurationFile,
    ConfigurationFiles, ConfigurationSourceFile, CredentialLocator, DetectedConfiguration,
};
pub use generation::{
    decode_message, plan_commit, ChatTokenLimitField, CommitFile, CommitOptions, CommitRequestPlan,
    Provider,
};
pub use pull_request::{
    decode_pull_request, plan_pull_request, PullRequestComparison, PullRequestDescription,
    PullRequestOptions,
};

#[cfg(test)]
mod request_tests;
#[cfg(test)]
mod tests;
