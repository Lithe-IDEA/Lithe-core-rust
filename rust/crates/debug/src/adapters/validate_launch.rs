//! Validation of capabilities, consoles, launches and endpoints.

use super::*;

impl DebugCapabilities {
    pub(crate) fn validate(&mut self, field: &str) -> Validation<()> {
        let disabled_field = format!("{field}.disabled");
        if self.disabled.is_empty() {
            return Err(invalid(
                disabled_field,
                "Disabled capabilities list at least one name.",
            ));
        }
        if let Some(index) = self
            .disabled
            .iter()
            .position(|name| !DAP_CAPABILITIES.contains(&name.as_str()))
        {
            return Err(invalid(
                format!("{disabled_field}[{index}]"),
                "Only standard DAP capability names can be disabled.",
            ));
        }
        sort_unique(&mut self.disabled, &disabled_field)
    }
}

impl DebugConsole {
    pub(crate) fn validate(&mut self, field: &str) -> Validation<()> {
        let commands_field = format!("{field}.commands");
        if let Some(prefix) = &self.command_prefix {
            // The host concatenates the prefix and the line as written, so a
            // space or control character would shift what the adapter reads.
            if prefix.is_empty()
                || prefix.chars().count() > MAX_CONSOLE_COMMAND_PREFIX
                || prefix
                    .chars()
                    .any(|character| character.is_control() || character.is_whitespace())
            {
                return Err(invalid(
                    format!("{field}.commandPrefix"),
                    "A command prefix is 1-8 characters without spaces.",
                ));
            }
        }
        if self.commands.is_empty() && self.command_prefix.is_none() {
            return Err(invalid(
                commands_field,
                "A console declares at least one command or a command prefix.",
            ));
        }
        if self.commands.len() > MAX_CONSOLE_COMMANDS {
            return Err(invalid(
                commands_field,
                format!("An adapter declares at most {MAX_CONSOLE_COMMANDS} console commands."),
            ));
        }
        for (index, command) in self.commands.iter().enumerate() {
            let command_field = format!("{commands_field}[{index}]");
            // The name is inserted as written, so surrounding spaces would
            // change what the user's line means.
            if command.name.is_empty()
                || command.name.trim() != command.name
                || command.name.chars().count() > MAX_CONSOLE_COMMAND_NAME
                || command.name.chars().any(char::is_control)
            {
                return Err(invalid(
                    format!("{command_field}.name"),
                    "A command name is one line of 1-128 characters without surrounding spaces.",
                ));
            }
            if let Some(description) = &command.description {
                if description.trim().is_empty()
                    || description.chars().count() > MAX_CONSOLE_COMMAND_DESCRIPTION
                    || description.chars().any(char::is_control)
                {
                    return Err(invalid(
                        format!("{command_field}.description"),
                        "A description is one line of 1-240 characters.",
                    ));
                }
            }
        }
        self.commands
            .sort_by(|left, right| left.name.cmp(&right.name));
        if let Some(pair) = self
            .commands
            .windows(2)
            .find(|pair| pair[0].name == pair[1].name)
        {
            return Err(invalid(
                commands_field,
                format!("The command {} is listed more than once.", pair[0].name),
            ));
        }
        Ok(())
    }
}

impl DebugLaunch {
    pub(crate) fn validate(&self, field: &str, context: &DeclarationContext<'_>) -> Validation<()> {
        let endpoint_field = format!("{field}.endpoint");
        let hosted = self.transport == DebugTransport::TcpLanguageServer;
        match (&self.process, hosted) {
            (Some(process), false) => process.validate(&format!("{field}.process"), context)?,
            (None, true) => {}
            (None, false) => {
                return Err(invalid(
                    format!("{field}.process"),
                    "An adapter process is required unless the transport is tcpLanguageServer.",
                ));
            }
            (Some(_), true) => {
                return Err(invalid(
                    format!("{field}.process"),
                    "A tcpLanguageServer adapter runs inside the language server and has no process.",
                ));
            }
        }
        match (self.transport, &self.endpoint) {
            (DebugTransport::Stdio, None) => Ok(()),
            (DebugTransport::Stdio, Some(_)) => Err(invalid(
                endpoint_field,
                "The stdio transport has no endpoint.",
            )),
            (_, None) => Err(invalid(
                endpoint_field,
                "A TCP transport needs an endpoint.",
            )),
            (DebugTransport::TcpAnnounced, Some(endpoint)) => {
                let valid = endpoint.port.is_none()
                    && !endpoint.has_language_server_fields()
                    && matches!(endpoint.stream.as_deref(), Some("stdout" | "stderr"))
                    && matches!(endpoint.value.as_deref(), Some("port" | "hostPort"))
                    && endpoint.prefix.as_deref().is_some_and(|prefix| {
                        !prefix.is_empty()
                            && prefix.len() <= MAX_ENDPOINT_PREFIX
                            && !prefix.contains(['\r', '\n', '\0'])
                    });
                if valid {
                    Ok(())
                } else {
                    Err(invalid(
                        endpoint_field,
                        "An announced endpoint names a stream, a one-line prefix, and port or hostPort.",
                    ))
                }
            }
            (DebugTransport::TcpPort, Some(endpoint)) => {
                let valid = endpoint.stream.is_none()
                    && endpoint.prefix.is_none()
                    && endpoint.value.is_none()
                    && !endpoint.has_language_server_fields()
                    && endpoint.port.is_some_and(|port| port >= 1);
                if valid {
                    Ok(())
                } else {
                    Err(invalid(
                        endpoint_field,
                        "A fixed endpoint is a loopback port from 1 to 65535.",
                    ))
                }
            }
            (DebugTransport::TcpLanguageServer, Some(endpoint)) => {
                if endpoint.stream.is_some()
                    || endpoint.prefix.is_some()
                    || endpoint.value.is_some()
                    || endpoint.port.is_some()
                {
                    return Err(invalid(
                        endpoint_field,
                        "A language server endpoint names only a language and a command.",
                    ));
                }
                let language = endpoint.language_id.as_deref().unwrap_or_default();
                if !context.language_servers.contains(language) {
                    return Err(invalid(
                        format!("{endpoint_field}.languageID"),
                        "The language server is not declared in languageServers.",
                    ));
                }
                if !endpoint.command.as_deref().is_some_and(is_server_command) {
                    return Err(invalid(
                        format!("{endpoint_field}.command"),
                        "A command is 1-128 letters, digits, and . _ : -.",
                    ));
                }
                Ok(())
            }
        }
    }
}

impl DebugEndpoint {
    fn has_language_server_fields(&self) -> bool {
        self.language_id.is_some() || self.command.is_some()
    }
}
