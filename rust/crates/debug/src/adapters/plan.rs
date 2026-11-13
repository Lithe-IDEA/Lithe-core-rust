//! The launch plan of a debug session.

use super::*;

impl DebugAdapterDeclaration {
    /// What the host needs besides the process: how to reach the adapter and
    /// what to show. The process itself is the launch plan's own part.
    pub fn plan_descriptor(&self, plugin_id: &str) -> Value {
        let mut descriptor = json!({
            "id": self.id,
            "providerID": format!("{plugin_id}/{}", self.id),
            "displayName": self.display_name,
            "adapterID": self.adapter_id.as_deref().unwrap_or(&self.id),
            "requests": self.requests,
            "selectors": self.selectors,
            "configurationTypes": self.configuration_types,
            "transport": self.launch.transport,
            "endpoint": self.launch.endpoint,
            "disabledCapabilities": self.capabilities.as_ref().map(|c| c.disabled.clone()).unwrap_or_default(),
            // Present exactly when the adapter attaches to a remote host and
            // port; says whether the host may offer path mappings and how many
            // the adapter can take (0 when it takes none).
            "remoteAttach": self.connection_binding().map(|binding| json!({
                "pathMappings": binding.path_mappings.is_some(),
                "maxPathMappings": binding.path_mappings.as_ref().map_or(0, DebugPathMappingBinding::limit),
            })),
        });
        // The host hands these to the debug engine with the session, which then follows the
        // declaration instead of recognizing the adapter by its ID.
        if let Some(features) = &self.features {
            descriptor["features"] = json!(features);
        }
        if let Some(defaults) = &self.defaults {
            descriptor["defaults"] = json!(defaults);
        }
        descriptor
    }
}

/// Checks one debug start against the registration, the selected runtime and
/// the target facts the request's bindings read.
pub fn plan(
    adapter: &DebugAdapterDeclaration,
    request: DebugRequest,
    facts: &StartFacts,
) -> Validation<()> {
    if facts.registration.is_stale() {
        return Err(PluginRejection::at(
            "debug.provider.stale",
            "registration",
            "The adapter registration changed after this start was prepared.",
        ));
    }
    if !adapter.requests.contains(&request) {
        return Err(PluginRejection::at(
            "debug.request.unsupported",
            "request",
            "The adapter does not serve this request.",
        ));
    }
    if uses_selected_runtime(std::slice::from_ref(adapter))
        && facts.runtime_selection.is_none_or(Generation::is_stale)
    {
        return Err(PluginRejection::at(
            "debug.runtime.stale",
            "runtimeSelection",
            "The selected runtime changed after this start was prepared.",
        ));
    }
    // A remote target is only ever attached to, through an adapter that says
    // where its host and port go.
    if facts.target.kind == TARGET_CONNECTION
        && (request != DebugRequest::Attach || adapter.connection_binding().is_none())
    {
        return Err(PluginRejection::at(
            "debug.target.invalid",
            "target.kind",
            "The adapter does not attach to a remote host and port.",
        ));
    }
    let bindings = adapter
        .configurations
        .as_ref()
        .and_then(|configurations| match request {
            DebugRequest::Launch => configurations.launch.as_ref(),
            DebugRequest::Attach => configurations.attach.as_ref(),
        })
        .and_then(|configuration| configuration.target_bindings.as_ref());
    let Some(bindings) = bindings else {
        return Ok(());
    };
    let missing = |field: &'static str, fact: &str| {
        PluginRejection::at(
            "debug.target.invalid",
            field,
            format!("The target has no {fact}, which the adapter's configuration reads."),
        )
    };
    let target = &facts.target;
    match bindings.program.as_deref() {
        Some(TARGET_EXECUTABLE)
            if !target
                .executable
                .as_deref()
                .is_some_and(|path| std::path::Path::new(path).is_absolute()) =>
        {
            return Err(missing("target.executable", "executable"));
        }
        Some(source)
            if source != TARGET_EXECUTABLE && target.path.as_deref().is_none_or(str::is_empty) =>
        {
            return Err(missing("target.path", "path"));
        }
        _ => {}
    }
    if bindings.args.is_some()
        && target.arguments.as_ref().is_none_or(|args| {
            args.len() > 1024
                || args
                    .iter()
                    .any(|arg| arg.len() > 65536 || arg.contains('\0'))
        })
    {
        return Err(missing("target.arguments", "bounded arguments"));
    }
    if bindings.env.is_some()
        && target.environment.as_ref().is_none_or(|env| {
            env.len() > 64
                || env.iter().any(|(key, value)| {
                    key.is_empty()
                        || key.len() > 128
                        || !key.bytes().enumerate().all(|(index, byte)| {
                            byte == b'_'
                                || byte.is_ascii_alphabetic()
                                || index > 0 && byte.is_ascii_digit()
                        })
                        || value.len() > 65536
                        || value.contains('\0')
                })
        })
    {
        return Err(missing("target.environment", "bounded environment"));
    }
    if bindings.process_id.is_some() && target.process_id.is_none() {
        return Err(missing("target.processID", "process ID"));
    }
    if bindings.cwd.is_some()
        && target
            .working_directory
            .as_deref()
            .is_none_or(str::is_empty)
    {
        return Err(missing("target.workingDirectory", "working directory"));
    }
    if let Some(connection) = &bindings.connection {
        connection_endpoint(target)?;
        if let Some(shape) = &connection.path_mappings {
            check_path_mappings(&target.path_mappings, shape)?;
        } else if !target.path_mappings.is_empty() {
            // Dropping a mapping the user typed would let the session start
            // with source lines that silently do not match.
            return Err(PluginRejection::at(
                "debug.target.invalid",
                "target.pathMappings",
                "The adapter takes no path mappings.",
            ));
        }
    }
    Ok(())
}
