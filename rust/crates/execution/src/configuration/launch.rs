//! Launch inputs and launch plans of a resolved configuration.

use super::*;

/// Extracts existing Run document values; framework goals and runtime semantics stay in providers.
pub fn provider_launch_inputs(request: ProviderLaunchInputsRequest) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    if invalid_relative_path(&request.provider_root)
        || !project_directory_exists(&root, &request.provider_root)
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "The provider instance leaves the workspace.",
        ));
    }
    let explicit_cwd = configuration_override_has_key(
        &root,
        request.local_document.as_ref(),
        &request.configuration_id,
        "cwd",
    )?;
    let explicit_arguments = configuration_override_has_key(
        &root,
        request.local_document.as_ref(),
        &request.configuration_id,
        "args",
    )?;
    let resolved = resolve(ResolveRequest {
        team_document: None,
        root: request.root,
        toolchain_candidates: Vec::new(),
        local_document: request.local_document,
    })?;
    let config = resolved["configurations"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|value| value["id"] == request.configuration_id)
        })
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::InvalidRequest,
                "Run configuration is unavailable.",
            )
        })?;
    if config["provider"] == "build.provider" {
        let reference: BuildProviderRunReference =
            serde_json::from_value(config["extensions"]["build"].clone()).map_err(|_| {
                CoreError::new(
                    ErrorCode::ParseFailed,
                    "Invalid build provider run reference",
                )
            })?;
        reference.validate()?;
        if reference.root != request.provider_root || config["disabled"] == true {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Run configuration belongs to a different provider instance or is disabled.",
            ));
        }
        let arguments = if explicit_arguments {
            config["args"].clone()
        } else {
            json!(reference.user_arguments)
        };
        let choices = crate::provider_run_options::BuildProviderRunOptions::from_extensions(
            &serde_json::from_value(config["extensions"].clone()).map_err(|_| {
                CoreError::new(ErrorCode::InvalidRequest, "Invalid provider run options")
            })?,
        )?;
        let mut options = json!({"execution":reference.execution.unwrap_or_default(),"runtimeArguments":choices.runtime_arguments,
            "arguments":arguments,"environment":config["env"]});
        if !choices.entrypoint.is_empty() {
            options["entrypoint"] = json!(choices.entrypoint);
        }
        // Saving an older selector materializes its root as cwd. Keep that
        // default opaque for v4 applications; only an actual override is v10.
        if explicit_cwd
            && (config["cwd"] != reference.root
                || request.configuration_id == reference.generated_id())
        {
            options["workingDirectory"] = config["cwd"].clone();
        }
        return Ok(
            json!({"buildSystemID":reference.build_system_id,"provider":reference.run,"projectPath":reference.root,
            "launchOptions":options,"optionOverrides":choices.options,"tools":choices.active_tools(),"reference":reference}),
        );
    }
    Err(CoreError::new(
        ErrorCode::InvalidRequest,
        "Run configuration has no provider-owned launch inputs.",
    ))
}

/// Resolves one configuration into the exact executable, arguments, and environment.
///
/// Core only assembles process-based plans: a detector or provider either named
/// the command, or transferred executable ownership to a runtime toolchain.
/// Ecosystem launch semantics stay in the plugin that declared the run.
pub fn create_launch_plan(request: LaunchPlanRequest) -> Result<Value, CoreError> {
    existing_root(&request.root)?;
    let resolved = resolve(ResolveRequest {
        team_document: None,
        root: request.root,
        toolchain_candidates: Vec::new(),
        local_document: request.local_document,
    })?;
    let config = resolved["configurations"]
        .as_array()
        .and_then(|items| items.iter().find(|v| v["id"] == request.configuration_id))
        .ok_or_else(|| {
            CoreError::new(ErrorCode::InvalidRequest, "Run configuration was not found")
        })?;
    if config["disabled"].as_bool().unwrap_or(false) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Run configuration is disabled",
        ));
    }
    if let Some(command) = config["command"].as_str().filter(|value| !value.is_empty()) {
        return process_launch_plan(config, command);
    }
    if let Some(toolchain) = config["toolchains"]["runtime"]
        .as_str()
        .filter(|value| !value.is_empty())
    {
        return toolchain_process_launch_plan(config, toolchain);
    }
    Err(CoreError::new(
        ErrorCode::InvalidRequest,
        "Run provider must declare a command or runtime toolchain",
    )
    .with_details(config["provider"].as_str().unwrap_or("")))
}

/// Returns whether a user-owned layer explicitly sets one configuration key.
///
/// Generated `cwd: "."` is the schema default and may inherit the provider
/// root. A project or local `cwd: "."`, however, is an intentional override
/// and must keep the workspace root.
fn configuration_override_has_key(
    root: &Path,
    provided_local: Option<&Value>,
    configuration_id: &str,
    key: &str,
) -> Result<bool, CoreError> {
    let team = read_document_value(root, "run/configurations.json")?
        .unwrap_or_else(|| json!({"version": VERSION, "configurations": []}));
    let local = local_layer_document(root, provided_local.cloned())?;
    for document in [&team, &local] {
        validate_version_value(document)?;
        if document["configurations"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["id"] == configuration_id))
            .is_some_and(|item| item.get(key).is_some())
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Launch plan for configurations that name their own executable.
///
/// `executable.command` is a bare program name resolved on PATH by the host, as
/// opposed to `executable.toolchain` which the host resolves from its toolchain
/// registry. The two are mutually exclusive and the host branches on which key
/// is present. No toolchain home variable is injected: the plan carries only
/// the configuration's own environment.
fn process_launch_plan(config: &Value, command: &str) -> Result<Value, CoreError> {
    if command.contains('/') || command.contains('\\') {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Run configuration command must be a bare program name",
        )
        .with_details(command));
    }
    let arguments = config["args"].as_array().cloned().unwrap_or_default();
    if arguments.iter().any(|value| !value.is_string()) {
        return Err(CoreError::new(
            ErrorCode::ParseFailed,
            "Run configuration arguments must be strings",
        ));
    }
    Ok(json!({
        "executable": { "command": command },
        "arguments": arguments,
        "workingDirectory": config["cwd"].as_str().unwrap_or("."),
        // `environment` is reserved for toolchain-derived values the host must
        // resolve. Literal key/value pairs stay in `env` so the two never
        // collide in one field.
        "environment": {},
        "env": config["env"].as_object().cloned().unwrap_or_default()
    }))
}

/// Launch plan for a language provider whose executable is supplied by the
/// host Toolchain Registry. The shared contract uses the stable `runtime` key;
/// provider-specific SDK metadata stays in other toolchain/extension entries.
fn toolchain_process_launch_plan(config: &Value, toolchain: &str) -> Result<Value, CoreError> {
    let arguments = config["args"].as_array().cloned().unwrap_or_default();
    if arguments.iter().any(|value| !value.is_string()) {
        return Err(CoreError::new(
            ErrorCode::ParseFailed,
            "Run configuration arguments must be strings",
        ));
    }
    Ok(json!({
        "executable": { "toolchain": toolchain },
        "arguments": arguments,
        "workingDirectory": config["cwd"].as_str().unwrap_or("."),
        "environment": {},
        "env": config["env"].as_object().cloned().unwrap_or_default()
    }))
}

pub(crate) fn split_arguments(input: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quote = None;
    let mut escaped = false;
    for character in input.chars() {
        // Quoting can deliberately start an empty argv item.
        if !character.is_whitespace() || quote.is_some() || escaped {
            started = true;
        }
        if escaped {
            current.push(character);
            escaped = false;
        } else if character == '\\' && quote != Some('\'') {
            escaped = true;
        } else if matches!(character, '\'' | '"') {
            if quote == Some(character) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(character);
            } else {
                current.push(character);
            }
        } else if character.is_whitespace() && quote.is_none() {
            if started {
                result.push(std::mem::take(&mut current));
                started = false;
            }
        } else {
            current.push(character);
        }
    }
    if escaped {
        current.push('\\');
    }
    if started {
        result.push(current);
    }
    result
}
