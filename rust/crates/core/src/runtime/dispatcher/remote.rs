//! `remote.*` commands: probing, authentication, connections, files,
//! processes and port forwards.

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
        RemoteCommand::WorkspaceTrust => {
            match serde_json::from_value(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid remote workspace trust request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(lithe_workspace::trust::state)
            {
                Ok(value) => CoreResponse::success(id, value),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RemoteCommand::EditWorkspaceTrust => {
            match serde_json::from_value(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid remote workspace trust request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(lithe_workspace::trust::edit)
            {
                Ok(value) => CoreResponse::success(id, value),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RemoteCommand::StartForward => {
            match serde_json::from_value(payload)
                .map_err(|error| invalid_forward_request(&error))
                .and_then(lithe_workspace::forwards::start)
            {
                Ok(started) => CoreResponse::success(
                    id,
                    serde_json::to_value(started).expect("port forward result should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RemoteCommand::StartHostForward => {
            match serde_json::from_value(payload)
                .map_err(|error| invalid_forward_request(&error))
                .and_then(lithe_workspace::forwards::start_host)
            {
                Ok(started) => CoreResponse::success(
                    id,
                    serde_json::to_value(started).expect("port forward result should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RemoteCommand::StartReverseForward => {
            match serde_json::from_value(payload)
                .map_err(|error| invalid_forward_request(&error))
                .and_then(lithe_workspace::forwards::start_reverse)
            {
                Ok(started) => CoreResponse::success(
                    id,
                    serde_json::to_value(started).expect("port forward result should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RemoteCommand::StopForward => {
            match serde_json::from_value(payload)
                .map_err(|error| invalid_forward_request(&error))
                .and_then(lithe_workspace::forwards::stop)
            {
                Ok(stopped) => CoreResponse::success(
                    id,
                    serde_json::to_value(stopped).expect("port forward result should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RemoteCommand::Forwards => {
            match serde_json::from_value(payload)
                .map_err(|error| invalid_forward_request(&error))
                .and_then(lithe_workspace::forwards::list)
            {
                Ok(forwards) => CoreResponse::success(
                    id,
                    serde_json::to_value(forwards).expect("port forward list should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RemoteCommand::ParseForwardAddress => {
            match serde_json::from_value(payload)
                .map_err(|error| invalid_forward_request(&error))
                .and_then(lithe_workspace::forwards::parse_address)
            {
                Ok(address) => CoreResponse::success(
                    id,
                    serde_json::to_value(address).expect("port forward address should encode"),
                ),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RemoteCommand::Connections => {
            match serde_json::from_value(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid remote connections request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(lithe_workspace::connections::list)
            {
                Ok(value) => CoreResponse::success(id, value),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RemoteCommand::EditConnections => {
            match serde_json::from_value(payload)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Invalid remote connections request",
                    )
                    .with_details(error.to_string())
                })
                .and_then(lithe_workspace::connections::edit)
            {
                Ok(value) => CoreResponse::success(id, value),
                Err(error) => CoreResponse::failure(id, error),
            }
        }
        RemoteCommand::ParseAddress => answer(
            id,
            payload,
            "Invalid remote address request",
            lithe_workspace::connections::parse_address,
        ),
        RemoteCommand::EffectiveSettings => answer(
            id,
            payload,
            "Invalid effective SSH settings request",
            lithe_workspace::ssh_configuration::effective_settings,
        ),
        RemoteCommand::SshConfigHosts => answer(
            id,
            payload,
            "Invalid SSH configuration hosts request",
            lithe_workspace::ssh_configuration::ssh_config_hosts,
        ),
        RemoteCommand::MottyHosts => answer(
            id,
            payload,
            "Invalid Motty hosts request",
            lithe_workspace::motty::hosts,
        ),
        RemoteCommand::Preferences => answer(
            id,
            payload,
            "Invalid remote preferences request",
            lithe_workspace::preferences::read,
        ),
        RemoteCommand::EditPreferences => answer(
            id,
            payload,
            "Invalid remote preferences request",
            lithe_workspace::preferences::edit,
        ),
        RemoteCommand::RunningPrograms => answer(
            id,
            payload,
            "Invalid remote programs request",
            lithe_workspace::programs::list,
        ),
        RemoteCommand::EndPrograms => answer(
            id,
            payload,
            "Invalid remote programs request",
            lithe_workspace::programs::end,
        ),
        RemoteCommand::Disconnect => {
            match serde_json::from_value::<lithe_workspace::sessions::DisconnectRequest>(payload) {
                Ok(request) => {
                    CoreResponse::success(id, json!(lithe_workspace::sessions::disconnect(request)))
                }
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
        RemoteCommand::ListWslDistributions => match lithe_workspace::wsl::list() {
            Ok(listed) => CoreResponse::success(
                id,
                serde_json::to_value(listed).expect("WSL distributions should encode"),
            ),
            Err(error) => CoreResponse::failure(id, error),
        },
        RemoteCommand::ListContainers => answer(
            id,
            payload,
            "Invalid container listing request",
            |request| {
                lithe_workspace::containers::list(request).map(|listed| {
                    serde_json::to_value(listed).expect("container listings should encode")
                })
            },
        ),
        RemoteCommand::DevcontainerConfigs => answer(
            id,
            payload,
            "Invalid dev container configuration request",
            |request| {
                lithe_workspace::devcontainers::configs(request).map(|listed| {
                    serde_json::to_value(listed)
                        .expect("dev container configurations should encode")
                })
            },
        ),
        RemoteCommand::DevcontainerUp => answer(
            id,
            payload,
            "Invalid dev container start request",
            |request| {
                lithe_workspace::devcontainers::up(request).map(|started| {
                    serde_json::to_value(started).expect("dev container starts should encode")
                })
            },
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

/// Decodes `payload` as `run`'s request and answers with its result; a
/// payload that does not decode is refused with `message`.
fn answer<R: serde::de::DeserializeOwned>(
    id: Option<String>,
    payload: Value,
    message: &'static str,
    run: impl FnOnce(R) -> Result<Value, CoreError>,
) -> CoreResponse {
    match serde_json::from_value(payload)
        .map_err(|error| {
            CoreError::new(ErrorCode::InvalidRequest, message).with_details(error.to_string())
        })
        .and_then(run)
    {
        Ok(value) => CoreResponse::success(id, value),
        Err(error) => CoreResponse::failure(id, error),
    }
}

/// A `remote.*Forward*` payload that does not decode.
fn invalid_forward_request(error: &serde_json::Error) -> CoreError {
    CoreError::new(ErrorCode::InvalidRequest, "Invalid port forward request")
        .with_details(error.to_string())
}
