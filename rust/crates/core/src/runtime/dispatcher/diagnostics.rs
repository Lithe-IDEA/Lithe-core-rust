//! `diagnostics.*` commands: log events and queries, log filters and retention,
//! redaction and diagnostic bundles.

use crate::protocol::{CoreError, CoreResponse, DiagnosticsCommand, ErrorCode};
use lithe_diagnostics::{manifest::BuildManifestRequest, redaction::RedactTextRequest};
use serde_json::{json, Value};

/// Runs one `diagnostics.*` command.
pub(super) fn dispatch(
    command: DiagnosticsCommand,
    id: Option<String>,
    payload: Value,
) -> CoreResponse {
    match command {
        DiagnosticsCommand::PrepareLogEvent => {
            match serde_json::from_value::<lithe_diagnostics::logging::LogEventRequest>(payload) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(lithe_diagnostics::logging::prepare_log_event(request))
                        .expect("Log event should encode"),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid log event request"),
                ),
            }
        }
        DiagnosticsCommand::QueryLogs => {
            match serde_json::from_value::<lithe_diagnostics::logging::LogQueryRequest>(payload) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(lithe_diagnostics::logging::query_logs(request))
                        .expect("Log query should encode"),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid log query request"),
                ),
            }
        }
        DiagnosticsCommand::RecordPluginOperation => CoreResponse::success(
            id,
            log_v2_result(lithe_diagnostics::plugin_operations::record_request(
                payload,
            )),
        ),
        DiagnosticsCommand::DrainCoreEvents => CoreResponse::success(
            id,
            serde_json::to_value(lithe_diagnostics::core_events::drain())
                .expect("Core logs encode"),
        ),
        DiagnosticsCommand::ConfigureCoreEvents => {
            match serde_json::from_value::<CoreEventConfiguration>(payload) {
                Ok(configuration) => {
                    lithe_diagnostics::core_events::configure(
                        configuration.debug,
                        configuration.roots,
                    );
                    CoreResponse::success(id, json!({}))
                }
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid Core event configuration",
                    )
                    .with_details(error.to_string()),
                ),
            }
        }
        DiagnosticsCommand::EncodeLogBatch => {
            match serde_json::from_value::<lithe_diagnostics::record::EncodeLogBatchRequest>(
                payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(lithe_diagnostics::record::encode_log_batch(request))
                        .expect("Log batch should encode"),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid log batch request"),
                ),
            }
        }
        DiagnosticsCommand::ParseLogFilter => {
            match serde_json::from_value::<lithe_diagnostics::filter::ParseLogFilterRequest>(
                payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(lithe_diagnostics::filter::parse_log_filter(request))
                        .expect("Filter response serializes"),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid log parse_log_filter request",
                    ),
                ),
            }
        }
        DiagnosticsCommand::ComposeLogFilter => {
            match serde_json::from_value::<lithe_diagnostics::filter::ComposeLogFilterRequest>(
                payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    log_v2_result(lithe_diagnostics::filter::compose_log_filter(request)),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid log compose_log_filter request",
                    ),
                ),
            }
        }
        DiagnosticsCommand::FormatLogRecords => {
            match serde_json::from_value::<lithe_diagnostics::format::FormatLogRecordsRequest>(
                payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    log_v2_result(lithe_diagnostics::format::format_log_records(request)),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid log format_log_records request",
                    ),
                ),
            }
        }
        DiagnosticsCommand::PlanLogCollection => {
            match serde_json::from_value::<lithe_diagnostics::collection::PlanLogCollectionRequest>(
                payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    log_v2_result(lithe_diagnostics::collection::plan_log_collection(request)),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid log collection request"),
                ),
            }
        }
        DiagnosticsCommand::QueryLogsV2 => {
            match serde_json::from_value::<lithe_diagnostics::query_v2::LogQueryV2Request>(payload)
            {
                Ok(request) => CoreResponse::success(
                    id,
                    log_v2_result(lithe_diagnostics::query_v2::query_logs_v2(request)),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid structured log query"),
                ),
            }
        }
        DiagnosticsCommand::PlanLogRetention => {
            match serde_json::from_value::<lithe_diagnostics::retention::PlanLogRetentionRequest>(
                payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    log_v2_result(lithe_diagnostics::retention::plan_log_retention(request)),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid log retention request"),
                ),
            }
        }
        DiagnosticsCommand::ResolveEngineLogSubject => {
            match serde_json::from_value::<lithe_lsp_engine::ResolveEngineLogSubjectRequest>(
                payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(lithe_lsp_engine::resolve_engine_log_subject(request))
                        .expect("Engine log subject should encode"),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid engine log subject request",
                    ),
                ),
            }
        }
        DiagnosticsCommand::ResolveLogChannel => {
            match serde_json::from_value::<lithe_diagnostics::channel::ResolveLogChannelRequest>(
                payload,
            ) {
                Ok(request) => CoreResponse::success(
                    id,
                    log_v2_result(lithe_diagnostics::channel::resolve_log_channel(request)),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid log channel request"),
                ),
            }
        }
        DiagnosticsCommand::RedactText => {
            match serde_json::from_value::<RedactTextRequest>(payload).map_err(|error| {
                CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Invalid diagnostics redact request",
                )
                .with_details(error.to_string())
            }) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(lithe_diagnostics::redaction::redact_text(request))
                        .expect("Diagnostics redact response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        DiagnosticsCommand::BuildManifest => {
            match serde_json::from_value::<BuildManifestRequest>(payload).map_err(|error| {
                CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Invalid diagnostics manifest request",
                )
                .with_details(error.to_string())
            }) {
                Ok(request) => CoreResponse::success(
                    id,
                    serde_json::to_value(lithe_diagnostics::manifest::build_manifest(request))
                        .expect("Diagnostics manifest response should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
    }
}

/// Host detailed-diagnostics state; `roots: None` enables DEBUG for every root.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CoreEventConfiguration {
    debug: bool,
    roots: Option<Vec<String>>,
}

// Domain failures retain their unique numeric logging codes inside the existing Core envelope.
fn log_v2_result<T: serde::Serialize>(
    result: Result<T, lithe_diagnostics::channel::LogError>,
) -> serde_json::Value {
    match result {
        Ok(value) => serde_json::json!({"result": value}),
        Err(error) => serde_json::json!({"error": error}),
    }
}
