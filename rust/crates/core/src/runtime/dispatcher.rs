//! Validation and routing from versioned command names to their owning domains.
//!
//! Each command family with its own enum in [`crate::protocol`] dispatches in the
//! child module named after it; the remaining small families dispatch here.

use crate::project::{
    self, DocumentLifecycleRequest, FileReadRequest, FileWriteRequest, HtmlPreviewRequest,
    HtmlPreviewSiteRequest, MarkdownReferenceRequest, MarkdownRenderRequest,
};
use crate::protocol::{CoreCommand, CoreError, CoreRequest, CoreResponse, ErrorCode, GitCommand};
use lithe_github::{NormalizeResponseRequest, ParseRemoteRequest, RequestPlanRequest};
/// The manifest type every domain command validates plugin manifests with.
use lithe_plugin_manifest::DeclarativePluginManifest as Manifest;
use serde_json::json;

mod ai;
mod build;
mod debug;
mod diagnostics;
mod editor;
mod git;
mod history;
mod keymap;
mod lsp;
mod plugins;
mod remote;
mod run_config;
mod toolchains;
mod workspace;

pub fn execute_json(request: &str) -> String {
    execute_document_json(request, None)
}

/// Borrows an immutable document for one request; no text is retained after dispatch.
pub(crate) fn execute_document_json(request: &str, document: Option<&str>) -> String {
    crate::runtime::helper_failures::install();
    let response = execute(request, document);
    serde_json::to_string(&response).unwrap_or_else(|_| {
        serde_json::to_string(&CoreResponse::failure(
            None,
            CoreError::new(ErrorCode::Unknown, "Could not encode core response"),
        ))
        .expect("fallback response should encode")
    })
}

/// Decode an agent management payload and run `operation` on it.
/// Declarative plugin commands decode their own payloads so manifest content
/// problems surface as `rejected` outcomes rather than Core failures.
fn plugin_response(
    id: Option<String>,
    payload: serde_json::Value,
    operation: fn(serde_json::Value) -> Result<serde_json::Value, CoreError>,
) -> CoreResponse {
    match operation(payload) {
        Ok(data) => CoreResponse::success(id, data),
        Err(error) => CoreResponse::failure(id, error),
    }
}

/// Runs one rule in `lithe-core-rules`.
fn rules_response(id: Option<String>, command: &str, payload: serde_json::Value) -> CoreResponse {
    match lithe_core_rules::execute(command, payload) {
        Ok(data) => CoreResponse::success(id, data),
        Err(error) => CoreResponse::failure(id, crate::protocol::rule_error(error)),
    }
}

/// Decodes a typed plan/reduce payload, runs the pure `operation` on it, and
/// encodes its result; a payload that does not decode is `InvalidRequest`.
fn reducer_response<T: serde::de::DeserializeOwned, R: serde::Serialize>(
    id: Option<String>,
    payload: serde_json::Value,
    invalid_message: &str,
    operation: fn(T) -> Result<R, CoreError>,
) -> CoreResponse {
    match serde_json::from_value::<T>(payload)
        .map_err(|error| {
            CoreError::new(ErrorCode::InvalidRequest, invalid_message)
                .with_details(error.to_string())
        })
        .and_then(operation)
    {
        Ok(data) => CoreResponse::success(
            id,
            serde_json::to_value(data).expect("Reducer results always encode"),
        ),
        Err(error) => CoreResponse::failure(id, error),
    }
}

fn execute(request: &str, document: Option<&str>) -> CoreResponse {
    let parsed: CoreRequest = match serde_json::from_str(request) {
        Ok(request) => request,
        Err(error) => {
            return CoreResponse::failure(
                None,
                CoreError::new(ErrorCode::InvalidRequest, "Invalid JSON request")
                    .with_details(error.to_string()),
            );
        }
    };
    // The trace spans every scope below and records once, after the final response.
    let trace = crate::runtime::request_trace::Request::begin(&parsed);
    let response = execute_parsed(parsed, document);
    trace.finish(&response);
    response
}

fn execute_parsed(mut parsed: CoreRequest, document: Option<&str>) -> CoreResponse {
    let id = parsed.id.clone();
    let response_id = id.clone();
    let operation_id = parsed.operation_id.clone().or_else(|| id.clone());
    let _cancellation_scope = crate::protocol::cancellation::Scope::begin(
        operation_id.clone(),
        parsed.timeout_milliseconds,
    );
    lithe_request_context::events::request_started(operation_id.as_deref());
    if let Err(error) = crate::protocol::cancellation::check() {
        return CoreResponse::failure(id, error);
    }
    if parsed.remote_workspace.is_some() && !has_remote_workspace_adapter(&parsed.command) {
        return CoreResponse::failure(
            id,
            CoreError::new(
                ErrorCode::NotSupported,
                "This command has no remote workspace adapter",
            ),
        );
    }
    let _workspace = match lithe_workspace::Scope::begin(parsed.remote_workspace) {
        Ok(scope) => scope,
        Err(error) => return CoreResponse::failure(id, error),
    };
    let _git_policy = match lithe_git::execution_policy::Scope::begin(parsed.git_execution) {
        Ok(scope) => scope,
        Err(error) => return CoreResponse::failure(id, error),
    };
    let Some(command) = CoreCommand::parse(&parsed.command) else {
        return CoreResponse::failure(
            id,
            CoreError::new(ErrorCode::NotSupported, "Unsupported core command")
                .with_details(parsed.command),
        );
    };

    if let Err(error) =
        lithe_document_uri::resolve_request_paths(&parsed.command, &mut parsed.payload)
    {
        return CoreResponse::failure(id, error);
    }

    let preserve_workspace_outcome =
        matches!(command, CoreCommand::Git(GitCommand::WorkspaceCommitStep));
    let response = match command {
        CoreCommand::IdeHostControl => {
            let action = parsed.payload["action"].as_str().unwrap_or("");
            match lithe_ide_host::execute(action, parsed.payload["arguments"].clone()) {
                Ok(data) => CoreResponse::success(id, data),
                Err(message) => {
                    CoreResponse::failure(id, CoreError::new(ErrorCode::InvalidRequest, message))
                }
            }
        }
        CoreCommand::Ping => CoreResponse::success(
            id,
            json!({
                "protocolVersion": 1,
                "coreVersion": env!("CARGO_PKG_VERSION")
            }),
        ),
        CoreCommand::Ai(command) => ai::dispatch(command, id, parsed.payload),
        CoreCommand::Plugins(command) => plugins::dispatch(command, id, parsed.payload),
        CoreCommand::Debug(command) => debug::dispatch(command, id, parsed.payload),
        CoreCommand::Keymap(command) => keymap::dispatch(command, id, parsed.payload),
        CoreCommand::Toolchains(command) => toolchains::dispatch(command, id, parsed.payload),
        CoreCommand::RunDecodeStart => plugin_response(
            id,
            parsed.payload,
            crate::build::run_decode_start_command::<Manifest>,
        ),
        CoreCommand::OutputLinksMatch => {
            plugin_response(id, parsed.payload, lithe_plugin_output_links::match_lines)
        }
        CoreCommand::OutputLinksDecodeResolution => plugin_response(
            id,
            parsed.payload,
            lithe_plugin_output_links::decode_resolution,
        ),
        CoreCommand::Build(command) => build::dispatch(command, id, parsed.payload),
        CoreCommand::VcsDetectRoots => plugin_response(
            id,
            parsed.payload,
            crate::vcs::vcs_detect_roots_command::<Manifest>,
        ),
        CoreCommand::VcsOwnerOf => {
            plugin_response(id, parsed.payload, crate::vcs::vcs_owner_of_command)
        }
        CoreCommand::VcsDecodeReply => {
            plugin_response(id, parsed.payload, crate::vcs::vcs_decode_reply_command)
        }
        CoreCommand::ProjectGeneratorCatalog => plugin_response(
            id,
            parsed.payload,
            crate::project_generators::project_generator_catalog_command::<Manifest>,
        ),
        CoreCommand::ProjectGeneratorForm => plugin_response(
            id,
            parsed.payload,
            crate::project_generators::project_generator_form_command::<Manifest>,
        ),
        CoreCommand::ProjectGenerationPlan => plugin_response(
            id,
            parsed.payload,
            crate::project_generators::project_generation_plan_command::<Manifest>,
        ),
        CoreCommand::ProjectCheckName => plugin_response(
            id,
            parsed.payload,
            crate::project_generators::project_check_name_command,
        ),
        CoreCommand::ProjectExtractArchive => plugin_response(
            id,
            parsed.payload,
            crate::project_generators::project_extract_archive_command,
        ),
        CoreCommand::Workspace(command) => workspace::dispatch(command, id, parsed.payload),
        CoreCommand::SearchActions => rules_response(id, "search.actions", parsed.payload),
        CoreCommand::FileRead => match serde_json::from_value::<FileReadRequest>(parsed.payload)
            .map_err(|error| {
                CoreError::new(ErrorCode::InvalidRequest, "Invalid file read request")
                    .with_details(error.to_string())
            })
            .and_then(project::read_file)
        {
            Ok(data) => CoreResponse::success(
                id,
                serde_json::to_value(data).expect("file response should encode"),
            ),
            Err(error) => CoreResponse::failure(id, error),
        },
        CoreCommand::FileWrite => match serde_json::from_value::<FileWriteRequest>(parsed.payload)
            .map_err(|error| {
                CoreError::new(ErrorCode::InvalidRequest, "Invalid file write request")
                    .with_details(error.to_string())
            })
            .and_then(project::write_file)
        {
            Ok(data) => CoreResponse::success(
                id,
                serde_json::to_value(data).expect("file response should encode"),
            ),
            Err(error) => CoreResponse::failure(id, error),
        },
        CoreCommand::TextDecode => {
            match serde_json::from_value::<project::TextDecodeRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid text decode request")
                        .with_details(error.to_string())
                })
                .and_then(project::decode_text_bytes)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("text response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::TextEncode => {
            match serde_json::from_value::<project::TextEncodeRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid text encode request")
                        .with_details(error.to_string())
                })
                .and_then(project::encode_text)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("text response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::DocumentClassifyText => {
            match parsed
                .payload
                .get("text")
                .and_then(serde_json::Value::as_str)
            {
                Some(text) => CoreResponse::success(
                    id,
                    serde_json::json!({
                        "isPlainText": project::is_plain_text(text)
                    }),
                ),
                None => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Decoded document text is required",
                    ),
                ),
            }
        }
        CoreCommand::DocumentPlanSaveBatch => plugin_response(
            id,
            parsed.payload,
            project::document_save_batch::plan_command,
        ),
        CoreCommand::DocumentReduceSaveBatch => plugin_response(
            id,
            parsed.payload,
            project::document_save_batch::reduce_command,
        ),
        CoreCommand::DocumentLifecycle => {
            match serde_json::from_value::<DocumentLifecycleRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid document lifecycle request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(project::decide_document_lifecycle)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("document lifecycle decision should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::History(command) => history::dispatch(command, id, parsed.payload),
        CoreCommand::DiffCompareTexts => {
            plugin_response(id, parsed.payload, lithe_diff::compare_texts_command)
        }
        CoreCommand::ExecutionResolveSearchPath => {
            match serde_json::from_value::<crate::execution::SearchPathRequest>(parsed.payload) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::json!(crate::execution::resolve_search_path(request)),
                ),
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid search path request")
                        .with_details(error.to_string()),
                ),
            }
        }
        CoreCommand::ExecutionResolveEnvironment => {
            match serde_json::from_value::<crate::execution::LaunchEnvironmentRequest>(
                parsed.payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(crate::execution::resolve_launch_environment(request))
                        .expect("environment encodes"),
                ),
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid launch environment request",
                    )
                    .with_details(error.to_string()),
                ),
            }
        }
        CoreCommand::ProcessLifecycleResolve => {
            match serde_json::from_value::<lithe_process::lifecycle::LifecycleRequest>(
                parsed.payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(lithe_process::lifecycle::resolve(&request))
                        .expect("process lifecycle plan should encode"),
                ),
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid process lifecycle request",
                    )
                    .with_details(error.to_string()),
                ),
            }
        }
        CoreCommand::ProcessLifecycleInventory => {
            match serde_json::from_value::<lithe_process::inventory::InventoryRequest>(
                parsed.payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(lithe_process::inventory::classify(&request))
                        .expect("process inventory should encode"),
                ),
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid process inventory request",
                    )
                    .with_details(error.to_string()),
                ),
            }
        }
        CoreCommand::ProcessLifecycleSettle => {
            match serde_json::from_value::<lithe_process::lifecycle::SettleRequest>(parsed.payload)
            {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(lithe_process::lifecycle::settle(&request))
                        .expect("process lifecycle settlement should encode"),
                ),
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid process lifecycle settlement request",
                    )
                    .with_details(error.to_string()),
                ),
            }
        }
        CoreCommand::Editor(command) => editor::dispatch(command, id, parsed.payload, document),
        CoreCommand::WorkbenchChromeLayoutReduce => {
            rules_response(id, "workbench.chromeLayout.reduce", parsed.payload)
        }
        CoreCommand::TerminalParseLinkTarget => {
            let result =
                serde_json::from_value::<lithe_terminal::ParseLinkTargetsRequest>(parsed.payload)
                    .map_err(|_| {
                        CoreError::new(ErrorCode::InvalidRequest, "Invalid terminal link request")
                    })
                    .and_then(lithe_terminal::parse_link_targets);
            match result {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("terminal link result should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::MarkdownRender => {
            match serde_json::from_value::<MarkdownRenderRequest>(parsed.payload).map_err(|error| {
                CoreError::new(ErrorCode::InvalidRequest, "Invalid Markdown render request")
                    .with_details(error.to_string())
            }) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(crate::project::render(request))
                        .expect("Markdown render response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::MarkdownResolveReference => {
            match serde_json::from_value::<MarkdownReferenceRequest>(parsed.payload).map_err(
                |error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Markdown reference request",
                    )
                    .with_details(error.to_string())
                },
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(crate::project::resolve_reference(request))
                        .expect("Markdown reference response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::HtmlPreviewSite => {
            let site = serde_json::from_value::<HtmlPreviewSiteRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid HTML preview site request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(|request| {
                    project::html_preview_site(request).ok_or_else(|| {
                        CoreError::new(
                            ErrorCode::InvalidRequest,
                            "HTML preview document must be an absolute file path",
                        )
                    })
                });
            match site {
                Ok(site) => CoreResponse::success(
                    id,
                    serde_json::to_value(site).expect("HTML preview site should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::HtmlPreviewResolveRequest => {
            match serde_json::from_value::<HtmlPreviewRequest>(parsed.payload).map_err(|error| {
                CoreError::new(ErrorCode::InvalidRequest, "Invalid HTML preview request")
                    .with_details(error.to_string())
            }) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(project::resolve_html_preview_request(request))
                        .expect("HTML preview resource should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::Lsp(command) => lsp::dispatch(command, id, parsed.payload),
        CoreCommand::NetworkProxyPolicy => {
            match serde_json::from_value::<lithe_network::NetworkProxyPolicyRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid network proxy policy request",
                    )
                    .with_details(error.to_string())
                })
                .map(lithe_network::proxy_policy)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Proxy policy response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::RunConfig(command) => run_config::dispatch(command, id, parsed.payload),
        CoreCommand::Git(command) => git::dispatch(command, id, parsed.payload),
        CoreCommand::Remote(command) => remote::dispatch(command, id, parsed.payload),
        CoreCommand::GitHubParseRemote => {
            match serde_json::from_value::<ParseRemoteRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid GitHub remote request")
                        .with_details(error.to_string())
                })
                .and_then(lithe_github::parse_remote)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("GitHub repository should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitHubRequestPlan => {
            match serde_json::from_value::<RequestPlanRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid GitHub request plan")
                        .with_details(error.to_string())
                })
                .and_then(lithe_github::request_plan)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("GitHub request plan should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitHubPullRequestCheckoutPlan => {
            match serde_json::from_value::<lithe_github::PullRequestCheckoutPlanRequest>(
                parsed.payload,
            )
            .map_err(|error| {
                CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Invalid pull-request checkout plan",
                )
                .with_details(error.to_string())
            })
            .and_then(lithe_github::pull_request_checkout_plan)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Pull-request checkout plan should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitHubDeviceAuthorizationTransition => {
            match serde_json::from_value::<lithe_github::DeviceAuthorizationTransitionRequest>(
                parsed.payload,
            )
            .map_err(|error| {
                CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Invalid GitHub device transition",
                )
                .with_details(error.to_string())
            })
            .and_then(lithe_github::device_authorization_transition)
            {
                Ok(data) => CoreResponse::success(
                    id,
                    serde_json::to_value(data).expect("Device transition should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::GitHubNormalizeResponse => {
            match serde_json::from_value::<NormalizeResponseRequest>(parsed.payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid GitHub response")
                        .with_details(error.to_string())
                })
                .and_then(lithe_github::normalize_response)
            {
                Ok(data) => CoreResponse::success(id, data),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        CoreCommand::Diagnostics(command) => diagnostics::dispatch(command, id, parsed.payload),
    };
    // Workspace steps already reconcile cancelled writes under a bounded cleanup
    // deadline. Preserve that continuation so a retry cannot duplicate a commit.
    if response.is_success() && !preserve_workspace_outcome {
        match crate::protocol::cancellation::check() {
            Ok(()) => response,
            Err(error) => CoreResponse::failure(response_id, error),
        }
    } else {
        response
    }
}

/// Commands whose engines read and run through the request's workspace host;
/// every other command refuses a remote workspace instead of touching the
/// local placeholder folder.
fn has_remote_workspace_adapter(command: &str) -> bool {
    command.starts_with("git.")
        || command.starts_with("build.")
        || matches!(
            command,
            "workspace.repositories"
                | "workspace.snapshot"
                | "workspace.listDirectories"
                | "workspace.search"
                | "workspace.searchEverywhere"
                | "file.read"
                | "file.write"
                | "workspace.searchIndex.warm"
                | "workspace.searchIndex.update"
                | "workspace.searchIndex.invalidate"
                | "workspace.replacePreview"
                | "lsp.startServer"
        )
}

#[cfg(test)]
mod tests {
    use super::execute_json;
    use serde_json::{json, Value};

    #[test]
    fn configure_core_events_sets_the_debug_gate_and_rejects_unknown_fields() {
        use lithe_diagnostics::core_events::{accepts, configure};
        use lithe_diagnostics::record::LogLevel;
        let send = |payload: Value| -> Value {
            serde_json::from_str(&execute_json(
                &json!({"command": "diagnostics.configureCoreEvents", "payload": payload})
                    .to_string(),
            ))
            .unwrap()
        };
        assert_eq!(send(json!({"debug": true, "roots": ["vcs"]}))["ok"], true);
        let scoped = (
            accepts(LogLevel::Debug, "vcs"),
            accepts(LogLevel::Debug, "app"),
        );
        let rejected = send(json!({"debug": true, "level": "debug"}));
        // Restore the process default before asserting so a failure cannot leak DEBUG.
        configure(false, None);
        assert_eq!(scoped, (true, false));
        assert_eq!(rejected["ok"], false);
        assert_eq!(rejected["error"]["code"], "invalid_request");
        assert!(!accepts(LogLevel::Debug, "vcs"));
        assert!(accepts(LogLevel::Warn, "vcs"));
    }

    #[test]
    fn routes_semantic_lsp_runtime_commands() {
        let unknown_session = "missing-runtime-session";
        let requests = [
            json!({
                "command": "lsp.stopServer",
                "payload": { "sessionId": unknown_session }
            }),
            json!({
                "command": "lsp.syncDocument",
                "payload": {
                    "sessionId": unknown_session,
                    "uri": "file:///tmp/main.go",
                    "languageId": "go",
                    "text": "package main\n"
                }
            }),
            json!({
                "command": "lsp.closeDocument",
                "payload": {
                    "sessionId": unknown_session,
                    "uri": "file:///tmp/main.go"
                }
            }),
            json!({
                "command": "lsp.request",
                "payload": {
                    "sessionId": unknown_session,
                    "operationId": "completion-1",
                    "operation": "completion",
                    "uri": "file:///tmp/main.go",
                    "position": { "line": 0, "utf16Column": 0 }
                }
            }),
            json!({
                "command": "lsp.buildRequest",
                "payload": {
                    "sessionId": unknown_session,
                    "operationId": "build-1",
                    "method": "lithe/build/model",
                    "params": {}
                }
            }),
            json!({
                "command": "lsp.functionRequest",
                "payload": {
                    "sessionId": unknown_session,
                    "operationId": "function-1",
                    "function": { "pluginID": "dev.lithe.fixture.provider", "functionID": "snapshot.refresh" },
                    "params": {}
                }
            }),
            json!({
                "command": "lsp.cancelOperation",
                "payload": {
                    "sessionId": unknown_session,
                    "operationId": "completion-1"
                }
            }),
            json!({
                "command": "lsp.pollEvents",
                "payload": { "sessionId": unknown_session }
            }),
            json!({
                "command": "lsp.waitEvents",
                "payload": { "sessionId": unknown_session }
            }),
            json!({
                "command": "lsp.destroyServer",
                "payload": { "sessionId": unknown_session }
            }),
        ];

        for request in requests {
            let response: Value =
                serde_json::from_str(&execute_json(&request.to_string())).unwrap();
            assert_eq!(response["ok"], false, "request should reach the runtime");
            assert_eq!(response["error"]["code"], "invalid_request");
            assert_eq!(response["error"]["details"], unknown_session);
        }

        let invalid_start: Value = serde_json::from_str(&execute_json(
            &json!({
                "command": "lsp.startServer",
                "payload": {
                    "providerId": "go",
                    "executablePath": "",
                    "rootUri": "file:///tmp/project",
                    "workingDirectory": "/tmp/project"
                }
            })
            .to_string(),
        ))
        .unwrap();
        assert_eq!(invalid_start["ok"], false);
        assert_eq!(invalid_start["error"]["code"], "invalid_request");
        assert_eq!(
            invalid_start["error"]["details"],
            "executablePath/workingDirectory"
        );
    }
}
