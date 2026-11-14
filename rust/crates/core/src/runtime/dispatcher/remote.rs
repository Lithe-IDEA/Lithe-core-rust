//! `remote.*` commands: probing, authentication, connections, files and processes.

use crate::protocol::{CoreError, CoreResponse, ErrorCode, RemoteCommand};
use serde_json::{json, Value};

/// Runs one `remote.*` command.
pub(super) fn dispatch(command: RemoteCommand, id: Option<String>, payload: Value) -> CoreResponse {
    match command {
        RemoteCommand::Probe => {
            match serde_json::from_value::<lithe_workspace::probe::ProbeRequest>(payload) {
                Ok(request) => match lithe_workspace::probe::probe(request) {
                    Ok(result) => CoreResponse::success(
                        id,
                        serde_json::to_value(result).expect("remote probe result should encode"),
                    ),
                    Err(error) => CoreResponse::failure(id, error),
                },
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid remote probe request")
                        .with_details(error.to_string()),
                ),
            }
        }
        RemoteCommand::Connect => {
            match serde_json::from_value::<lithe_workspace::sessions::ConnectRequest>(payload) {
                Ok(request) => match lithe_workspace::sessions::connect(request) {
                    Ok(result) => CoreResponse::success(
                        id,
                        serde_json::to_value(result).expect("remote connect result should encode"),
                    ),
                    Err(error) => CoreResponse::failure(id, error),
                },
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid remote connect request")
                        .with_details(error.to_string()),
                ),
            }
        }
        RemoteCommand::ReadDir => {
            match serde_json::from_value::<lithe_workspace::sessions::ReadDirRequest>(payload) {
                Ok(request) => match lithe_workspace::sessions::read_dir(request) {
                    Ok(result) => CoreResponse::success(id, result),
                    Err(error) => CoreResponse::failure(id, error),
                },
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid remote directory request",
                    )
                    .with_details(error.to_string()),
                ),
            }
        }
        RemoteCommand::ReadFile => {
            match serde_json::from_value::<lithe_workspace::sessions::ReadFileRequest>(payload) {
                Ok(request) => match lithe_workspace::sessions::read_file(request) {
                    Ok(result) => CoreResponse::success(id, result),
                    Err(error) => CoreResponse::failure(id, error),
                },
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid remote file request")
                        .with_details(error.to_string()),
                ),
            }
        }
        RemoteCommand::WriteFile => {
            match serde_json::from_value::<lithe_workspace::sessions::WriteFileRequest>(payload) {
                Ok(request) => match lithe_workspace::sessions::write_file(request) {
                    Ok(result) => CoreResponse::success(id, result),
                    Err(error) => CoreResponse::failure(id, error),
                },
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid remote file request")
                        .with_details(error.to_string()),
                ),
            }
        }
        RemoteCommand::CaptureProcess => {
            match serde_json::from_value(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid host capture request")
                        .with_details(error.to_string())
                })
                .and_then(lithe_workspace::host::capture_process)
            {
                Ok(value) => CoreResponse::success(id, value),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RemoteCommand::HostRequest
        | RemoteCommand::RunProcess
        | RemoteCommand::RunWatch
        | RemoteCommand::RunTerminal => {
            let operation = match command {
                RemoteCommand::HostRequest => lithe_workspace::host::request,
                RemoteCommand::RunProcess => lithe_workspace::host::run_process,
                RemoteCommand::RunWatch => lithe_workspace::host::run_watch,
                _ => lithe_workspace::host::run_terminal,
            };
            match serde_json::from_value(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid workspace host request")
                        .with_details(error.to_string())
                })
                .and_then(operation)
            {
                Ok(value) => CoreResponse::success(id, value),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RemoteCommand::UploadFile => {
            match serde_json::from_value(payload)
                .map_err(|error| {
                    CoreError::new(ErrorCode::InvalidRequest, "Invalid upload request")
                        .with_details(error.to_string())
                })
                .and_then(lithe_workspace::upload::upload)
            {
                Ok(value) => CoreResponse::success(id, value),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RemoteCommand::HostInfo => {
            match payload
                .get("sessionId")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| CoreError::new(ErrorCode::InvalidRequest, "Missing host session"))
                .and_then(lithe_workspace::upload::host_info)
            {
                Ok(value) => CoreResponse::success(id, value),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RemoteCommand::Disconnect => {
            match serde_json::from_value::<lithe_workspace::sessions::DisconnectRequest>(payload) {
                Ok(request) => CoreResponse::success(
                    id,
                    json!({ "closed": lithe_workspace::sessions::disconnect(request) }),
                ),
                Err(error) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid remote disconnect request",
                    )
                    .with_details(error.to_string()),
                ),
            }
        }
        RemoteCommand::CloseAll => CoreResponse::success(
            id,
            json!({ "closed": lithe_workspace::sessions::close_all() }),
        ),
        RemoteCommand::AuthRespond => {
            #[derive(serde::Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct Reply {
                request_id: String,
                answer: Option<String>,
            }
            match serde_json::from_value::<Reply>(payload) {
                Ok(reply) => CoreResponse::success(
                    id,
                    json!({ "accepted": lithe_workspace::probe::respond(&reply.request_id, reply.answer) }),
                ),
                Err(_) => CoreResponse::failure(
                    id,
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid remote authentication response",
                    ),
                ),
            }
        }
    }
}
