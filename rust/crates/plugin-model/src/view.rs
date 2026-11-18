//! The plugin function that draws a plugin view: the body of a tool window or
//! a settings region. The view's tree and canvas vocabulary, and the checks a
//! result passes before a host draws it, live in `lithe-plugin-ui`.

use crate::names::is_function_id;
use crate::rejection::{invalid, Validation};
use serde::{Deserialize, Serialize};

/// Names the same-plugin function a host calls with a view event (`open`,
/// `click`, `change`, …) and draws from the tree or patches it returns. The
/// host owns layout, controls, input and Apply; the plugin only describes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ViewDeclaration {
    /// Plugin function served by the plugin's backend process.
    pub function: String,
}

impl ViewDeclaration {
    /// Checks the declaration as the manifest entry at `field`.
    pub fn validate(&self, field: &str) -> Validation<()> {
        if !is_function_id(&self.function) {
            return Err(invalid(
                format!("{field}.function"),
                "A view names a plugin function ID.",
            ));
        }
        Ok(())
    }
}
