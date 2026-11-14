//! Validation of one adapter declaration.

use super::*;

/// Whether any adapter starts on the runtime Run selected.
pub(crate) fn uses_selected_runtime(adapters: &[DebugAdapterDeclaration]) -> bool {
    adapters.iter().any(|adapter| {
        adapter
            .launch
            .process
            .as_ref()
            .and_then(DebugProcess::selected_runtime_tool)
            .is_some()
    })
}

/// Whether any adapter serves `attach`, which needs its own permission.
pub fn serves_attach(adapters: &[DebugAdapterDeclaration]) -> bool {
    adapters
        .iter()
        .any(|adapter| adapter.requests.contains(&DebugRequest::Attach))
}

impl DebugAdapterDeclaration {
    pub(crate) fn validate(
        &mut self,
        field: &str,
        context: &DeclarationContext<'_>,
    ) -> Validation<()> {
        if !is_local_id(&self.id) {
            return Err(invalid(
                format!("{field}.id"),
                "A debug adapter ID is a lowercase letter followed by up to 63 lowercase letters, digits, dots, underscores, or hyphens.",
            ));
        }
        validate_name(&self.display_name, &format!("{field}.displayName"))?;
        if let Some(adapter_id) = &self.adapter_id {
            validate_name(adapter_id, &format!("{field}.adapterID"))?;
        }

        // `launch` is the default; an explicit empty list would serve nothing.
        if self.requests.is_empty() {
            self.requests = vec![DebugRequest::Launch];
        }
        self.requests.sort();
        let before = self.requests.len();
        self.requests.dedup();
        if self.requests.len() != before {
            return Err(invalid(
                format!("{field}.requests"),
                "A request is listed more than once.",
            ));
        }

        self.validate_selectors(field)?;
        self.validate_configuration_types(field)?;
        if let Some(configurations) = &self.configurations {
            // A configuration for a request the adapter does not serve could
            // never apply.
            for (name, request, configuration) in [
                ("launch", DebugRequest::Launch, &configurations.launch),
                ("attach", DebugRequest::Attach, &configurations.attach),
            ] {
                let Some(configuration) = configuration else {
                    continue;
                };
                let configuration_field = format!("{field}.configurations.{name}");
                if !self.requests.contains(&request) {
                    return Err(invalid(
                        configuration_field,
                        "The configuration is for a request the adapter does not declare.",
                    ));
                }
                configuration.validate(&configuration_field, request, context)?;
            }
        }
        // A connection target needs somewhere to put `host:port`; without it
        // the selector would offer the adapter for a target it cannot start.
        if self.selectors.iter().any(|selector| {
            selector
                .target_kinds
                .iter()
                .any(|kind| kind == TARGET_CONNECTION)
        }) && self.connection_binding().is_none()
        {
            return Err(invalid(
                format!("{field}.selectors"),
                "A connection selector needs an attach configuration with a connection binding.",
            ));
        }
        if let Some(capabilities) = &mut self.capabilities {
            capabilities.validate(&format!("{field}.capabilities"))?;
        }
        if let Some(console) = &mut self.console {
            console.validate(&format!("{field}.console"))?;
        }
        let declared = DeclaredAdapterBehavior {
            features: self.features.clone(),
            defaults: self.defaults.clone(),
        };
        declared.check().map_err(|(declared_field, message)| {
            invalid(format!("{field}.{declared_field}"), message)
        })?;
        self.launch.validate(&format!("{field}.launch"), context)
    }

    /// The binding that receives a remote target, when the adapter attaches
    /// to one.
    pub(crate) fn connection_binding(&self) -> Option<&DebugConnectionBinding> {
        self.configurations
            .as_ref()
            .and_then(|configurations| configurations.attach.as_ref())
            .and_then(|configuration| configuration.target_bindings.as_ref())
            .and_then(|bindings| bindings.connection.as_ref())
    }

    fn validate_selectors(&mut self, field: &str) -> Result<(), PluginRejection> {
        if self.selectors.len() > MAX_SELECTORS {
            return Err(invalid(
                format!("{field}.selectors"),
                format!("An adapter declares at most {MAX_SELECTORS} selectors."),
            ));
        }
        for (index, selector) in self.selectors.iter_mut().enumerate() {
            let selector_field = format!("{field}.selectors[{index}]");
            if selector.target_kinds.is_empty() && selector.language_ids.is_empty() {
                return Err(invalid(
                    selector_field,
                    "A selector matches on at least one field.",
                ));
            }
            if let Some(kind) = selector
                .target_kinds
                .iter()
                .find(|kind| !TARGET_KINDS.contains(&kind.as_str()))
            {
                return Err(invalid(
                    format!("{selector_field}.targetKinds"),
                    format!("{kind} is not a target kind."),
                ));
            }
            if selector.language_ids.iter().any(|id| !is_language_id(id)) {
                return Err(invalid(
                    format!("{selector_field}.languageIDs"),
                    "A language ID is lowercase letters, digits, and + _ -.",
                ));
            }
            sort_unique(
                &mut selector.target_kinds,
                &format!("{selector_field}.targetKinds"),
            )?;
            sort_unique(
                &mut selector.language_ids,
                &format!("{selector_field}.languageIDs"),
            )?;
        }
        Ok(())
    }

    fn validate_configuration_types(&mut self, field: &str) -> Validation<()> {
        let types_field = format!("{field}.configurationTypes");
        if self.configuration_types.len() > MAX_CONFIGURATION_TYPES
            || self
                .configuration_types
                .iter()
                .any(|name| name.trim().is_empty() || name.chars().any(char::is_control))
        {
            return Err(invalid(
                types_field,
                format!("Configuration types are up to {MAX_CONFIGURATION_TYPES} non-empty names."),
            ));
        }
        sort_unique(&mut self.configuration_types, &types_field)
    }
}

/// Event that activates the plugin when a debug session needs the adapter.
pub fn activation_event(adapter: &DebugAdapterDeclaration) -> String {
    format!("onDebug:{}", adapter.id)
}

/// Install-review entries: the adapter process with the runtimes it needs.
/// Pointing an adapter at another executable or transport is a new program
/// the user must approve. A selected-runtime adapter adds no runtime entry:
/// the runtime is the one the user already chose for Run.
pub fn programs(adapter: &DebugAdapterDeclaration) -> Vec<Value> {
    let mut program = match &adapter.launch.process {
        Some(DebugProcess::Server(process)) => process.program(ServerRole::DebugAdapter),
        Some(DebugProcess::SelectedRuntime(process)) => json!({
            "kind": "selectedRuntimeDebugAdapter",
            "tool": process.launch.tool,
            "arguments": process.arguments,
            "validationArguments": process.validation_arguments,
        }),
        // The server is reviewed as the plugin's language server; what is new
        // here is the command that makes it serve debugging.
        None => json!({ "kind": "languageServerDebugAdapter" }),
    };
    program["debugAdapter"] = json!(adapter.id);
    program["transport"] = json!(adapter.launch.transport);
    program["endpoint"] = json!(adapter.launch.endpoint);
    let mut programs = vec![program];
    if let Some(DebugProcess::Server(process)) = &adapter.launch.process {
        programs.extend(process.runtime_programs());
    }
    programs
}

// The install check has no caller until the plugin installer reads it; tests
// run it now.
