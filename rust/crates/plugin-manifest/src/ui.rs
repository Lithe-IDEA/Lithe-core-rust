//! Validation of UI contributions: tool windows, web content, plugin views,
//! docks and commands.

use super::*;

impl DeclarativePluginManifest {
    pub(crate) fn validate_ui(&mut self) -> Validation<()> {
        let Some(ui) = &mut self.ui else {
            return Ok(());
        };
        if (ui.tool_windows.is_empty() && ui.commands.is_empty())
            || ui.tool_windows.len() > 64
            || ui.commands.len() > 128
        {
            return Err(invalid(
                "ui",
                "UI declares a tool window or a command, with at most 64 tool windows and 128 commands.",
            ));
        }
        let prefix = format!("{}.", self.id);
        let mut ids = BTreeSet::new();
        let mut windows = BTreeSet::new();
        let mut action_ids = BTreeSet::new();
        for (index, window) in ui.tool_windows.iter().enumerate() {
            if let Some(icon) = &window.icon {
                icon.validate()?;
            }
            validate_window_body(window, index, self.backend.is_some())?;
            if !is_plugin_id(&window.id)
                || !window.id.starts_with(&prefix)
                || window.title.trim().is_empty()
                || window.title.len() > 256
                || !ids.insert(window.id.clone())
            {
                return Err(invalid(
                    "ui.toolWindows",
                    "Invalid, duplicate or unowned tool-window ID, title or text.",
                ));
            }
            windows.insert(window.id.clone());
            action_ids.insert(format!("{}.toggle", window.id));
        }
        for command in &mut ui.commands {
            if let Some(icon) = &command.icon {
                icon.validate()?;
            }
            if !is_plugin_id(&command.id)
                || !command.id.starts_with(&prefix)
                || command.title.trim().is_empty()
                || command.title.len() > 256
                || !ids.insert(command.id.clone())
                || !action_ids.insert(command.id.clone())
                || match &command.tool_window_id {
                    Some(id) => !windows.contains(id),
                    // A command without a window runs on the plugin backend.
                    None => self.backend.is_none(),
                }
                || command.placements.is_empty()
            {
                return Err(invalid(
                    "ui.commands",
                    "Invalid or duplicate command, placement or tool-window reference.",
                ));
            }
            command.placements.sort();
            command.placements.dedup();
        }
        // Placement entries share one command action, but their contribution
        // IDs must not shadow an explicitly declared window or another action.
        for command in &ui.commands {
            for placement in &command.placements {
                let suffix = match placement {
                    UICommandPlacement::CommandPalette => "commandPalette",
                    UICommandPlacement::Toolbar => "toolbar",
                    UICommandPlacement::Menu => "menu",
                };
                if !ids.insert(format!("{}.{suffix}", command.id)) {
                    return Err(invalid(
                        "ui.commands",
                        "A command placement shadows another UI identity.",
                    ));
                }
            }
        }
        ui.tool_windows
            .sort_by(|a, b| (a.order, &a.id).cmp(&(b.order, &b.id)));
        ui.commands
            .sort_by(|a, b| (a.order, &a.id).cmp(&(b.order, &b.id)));
        Ok(())
    }
}

/// A window body is literal text, one package HTML page, or a plugin view
/// that the plugin's backend draws.
fn validate_window_body(window: &UIToolWindow, index: usize, has_backend: bool) -> Validation<()> {
    let field = format!("ui.toolWindows[{index}]");
    match (&window.text, &window.web, &window.view) {
        (Some(text), None, None) if text.len() <= 65536 => Ok(()),
        (Some(_), None, None) => Err(invalid(
            format!("{field}.text"),
            "The window text is too long.",
        )),
        (None, None, Some(view)) => {
            view.validate(&format!("{field}.view"))?;
            if !has_backend {
                return Err(invalid(
                    format!("{field}.view"),
                    "A view is drawn by the plugin backend; declare one.",
                ));
            }
            Ok(())
        }
        (None, Some(web), None) => {
            let entry = &web.entry;
            let valid = is_safe_relative_path(entry)
                && entry.len() <= 256
                && !entry.contains(':')
                && !entry.chars().any(char::is_control)
                && entry.split('/').all(|part| !part.is_empty() && part != ".")
                && entry.ends_with(".html");
            if valid {
                Ok(())
            } else {
                Err(invalid(
                    format!("{field}.web.entry"),
                    "The web entry must be a package-relative .html file.",
                ))
            }
        }
        _ => Err(invalid(
            field,
            "A tool window shows exactly one of text, web and view.",
        )),
    }
}
