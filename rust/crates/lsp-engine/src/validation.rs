//! Start-request validation and the errors every entry point shares.

use super::*;

pub(crate) fn validate_start_request(request: &StartServerRequest) -> Result<(), CoreError> {
    if request.provider_id.trim().is_empty() {
        return Err(invalid_field("providerId"));
    }
    // A host-owned server is no plugin's backend, and a service Core does not
    // know could never be forwarded.
    if !request.host_services.is_empty()
        && (request.plugin_id.is_none()
            || request
                .host_services
                .iter()
                .any(|service| !lithe_plugin_rpc::is_host_grant(service)))
    {
        return Err(invalid_field("hostServices"));
    }
    if let Some(policy) = &request.session_policy {
        policy.validate(request.plugin_id.as_deref())?;
    }
    if !request.host_options.is_empty()
        && (request.plugin_id.is_none()
            || request
                .host_options
                .editor
                .as_ref()
                .is_some_and(|editor| editor.validate().is_err()))
    {
        return Err(invalid_field("hostOptions"));
    }
    if request.executable_path.trim().is_empty()
        || request.executable_path.contains('\0')
        || request.working_directory.trim().is_empty()
        || request.working_directory.contains('\0')
    {
        return Err(invalid_field("executablePath/workingDirectory"));
    }
    if !request.root_uri.contains("://") || request.root_uri.contains('\0') {
        return Err(invalid_field("rootUri"));
    }
    Ok(())
}

fn invalid_field(field: &str) -> CoreError {
    CoreError::new(
        ErrorCode::InvalidRequest,
        "Invalid language-server start request.",
    )
    .with_details(field)
}

pub(crate) fn core_error_detail(error: &CoreError) -> String {
    match error.details.as_deref() {
        Some(details) if !details.is_empty() => format!("{} ({details})", error.message),
        _ => error.message.clone(),
    }
}

pub(crate) fn unknown_session(session_id: &str) -> CoreError {
    CoreError::new(
        ErrorCode::InvalidRequest,
        "Unknown language-server session.",
    )
    .with_details(session_id)
}

pub(crate) fn ensure_not_terminal(lifecycle: LspLifecycleState) -> Result<(), CoreError> {
    if matches!(
        lifecycle,
        LspLifecycleState::Stopping | LspLifecycleState::Stopped | LspLifecycleState::Failed
    ) {
        Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Language-server session is not accepting document changes.",
        ))
    } else {
        Ok(())
    }
}
