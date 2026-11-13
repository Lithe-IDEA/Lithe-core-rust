//! Icons a manifest may name: a host icon ID or package SVG/PNG resources.

use crate::names::is_safe_relative_path;
use crate::rejection::{invalid, Validation};
use serde::{Deserialize, Serialize};

/// Icon identity is portable data; hosts own asset resolution, sizing and states.
/// Custom assets are package-relative and are decoded only by the platform adapter.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UIIcon {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dark_path: Option<String>,
    #[serde(default)]
    mode: UIIconMode,
}

/// How the host paints an icon resource.
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UIIconMode {
    /// Tinted with the current theme foreground, like built-in icons.
    #[default]
    Template,
    /// Drawn with its own colors, untinted.
    Original,
}

impl UIIcon {
    /// Native actions reuse the host vocabulary, without package resource loading.
    pub fn is_valid_host_icon(&self) -> bool {
        self.host.is_some() && self.validate().is_ok()
    }

    /// Checks the icon as the manifest's `ui.icon`.
    pub fn validate(&self) -> Validation<()> {
        self.validate_at("ui.icon")
    }

    /// Validates the icon and reports a failure at `field`; build systems
    /// reuse the UI icon vocabulary for their tool windows and toolbar toggles.
    pub fn validate_at(&self, field: &str) -> Validation<()> {
        const HOST_IDS: &[&str] = &[
            "tool-window",
            "plugin",
            "database",
            "services",
            "terminal",
            "search",
            "refresh",
            "reset",
            "run",
            "debug",
            "settings",
            "add",
            "remove",
            "git",
            "problems",
        ];
        let valid_path = |path: &str| {
            is_safe_relative_path(path)
                && path.len() <= 256
                && !path.contains(':')
                && !path.chars().any(char::is_control)
                && path.split('/').all(|part| !part.is_empty() && part != ".")
                && (path.ends_with(".svg") || path.ends_with(".png"))
        };
        let valid = match (&self.host, &self.path) {
            (Some(host), None) => {
                HOST_IDS.contains(&host.as_str())
                    && self.dark_path.is_none()
                    && self.mode == UIIconMode::Template
            }
            (None, Some(path)) => {
                valid_path(path) && self.dark_path.as_deref().is_none_or(valid_path)
            }
            _ => false,
        };
        if !valid {
            return Err(invalid(
                field,
                "An icon must name a supported host icon or safe package SVG/PNG resources.",
            ));
        }
        Ok(())
    }
}
