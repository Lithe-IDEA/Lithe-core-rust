//! Version and configuration validation.

use super::*;

pub(crate) fn validate_version_value(document: &Value) -> Result<(), CoreError> {
    let version = document.get("version").and_then(Value::as_u64).unwrap_or(0) as u32;
    validate_version(version)?;
    validate_tool_preferences(document)
}

pub(crate) fn validate_tool_preferences(document: &Value) -> Result<(), CoreError> {
    if let Some(preferences) = document
        .get("toolchain")
        .and_then(|value| value.get("preferences"))
    {
        lithe_toolchain::preferences::validate(preferences)
            .map_err(|error| CoreError::new(ErrorCode::InvalidRequest, error.message))?;
    }
    Ok(())
}

pub(crate) fn validate_version(version: u32) -> Result<(), CoreError> {
    if version != VERSION {
        return Err(CoreError::new(
            ErrorCode::NotSupported,
            "Unsupported run configuration version",
        )
        .with_details(format!("expected {VERSION}, found {version}")));
    }
    Ok(())
}

/// Rejects a sidecar document written for a version this Core does not read.
pub fn validate_sidecar_version(version: u32) -> Result<(), CoreError> {
    if version != SIDECAR_VERSION {
        return Err(
            CoreError::new(ErrorCode::NotSupported, "Unsupported document version")
                .with_details(format!("expected {SIDECAR_VERSION}, found {version}")),
        );
    }
    Ok(())
}

pub(crate) fn validate_sidecar_version_value(document: &Value) -> Result<(), CoreError> {
    let version = document.get("version").and_then(Value::as_u64).unwrap_or(0) as u32;
    validate_sidecar_version(version)
}

pub(crate) fn validate_configuration(configuration: &RunConfiguration) -> Result<(), CoreError> {
    if configuration.id.trim().is_empty() || configuration.name.trim().is_empty() {
        return Err(CoreError::new(
            ErrorCode::ParseFailed,
            "Run configuration id and name are required",
        ));
    }
    if !valid_provider(&configuration.provider) {
        return Err(CoreError::new(
            ErrorCode::NotSupported,
            "Unsupported run configuration provider",
        )
        .with_details(configuration.provider.clone()));
    }
    if configuration.provider == "build.provider" {
        let reference: BuildProviderRunReference = serde_json::from_value(
            configuration
                .extensions
                .get("build")
                .cloned()
                .unwrap_or(Value::Null),
        )
        .map_err(|_| {
            CoreError::new(
                ErrorCode::ParseFailed,
                "Invalid build provider run reference",
            )
        })?;
        reference.validate()?;
        crate::provider_run_options::BuildProviderRunOptions::from_extensions(
            &configuration.extensions,
        )?;
        if configuration.command.is_some()
            || !configuration.toolchains.is_empty()
            || configuration.debug.is_some()
            || configuration.execution != reference.execution.unwrap_or_default()
        {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Build provider configurations cannot store commands, tool bindings or debug adapters",
            ));
        }
    }
    if invalid_relative_path(&configuration.cwd) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Run configuration contains an invalid project-relative path",
        )
        .with_details(format!("workingDirectory: {}", configuration.cwd)));
    }
    Ok(())
}

#[allow(dead_code)]
fn valid_relative(value: &str) -> bool {
    !invalid_relative_path(value)
}

/// `namespace.name`, both segments lowercase kebab starting with a letter.
/// Shape is checked, membership is not: an unknown ecosystem must be able to
/// register a provider without touching this file.
pub(crate) fn valid_provider(value: &str) -> bool {
    let Some((namespace, name)) = value.split_once('.') else {
        return false;
    };
    [namespace, name].iter().all(|segment| {
        segment
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_lowercase())
            && segment
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    })
}
