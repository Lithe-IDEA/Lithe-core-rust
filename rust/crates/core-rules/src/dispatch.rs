//! Command routing for the rules `lithe-core` forwards to this crate.

use crate::{editor, keymap, visibility, workbench, RuleError, RuleErrorCode};
use serde_json::Value;

/// Every command name this crate owns. `lithe-core` routes exactly these names here.
pub const COMMANDS: &[&str] = &[
    keymap::MODIFIER_GESTURE_STEP,
    editor::REDUCE_WORKBENCH_STATE,
    visibility::CLASSIFY_VISIBILITY,
    visibility::VISIBILITY_DEFAULTS,
    workbench::REDUCE_CHROME_LAYOUT,
];

/// Runs one rule on an already decoded payload.
pub fn execute(command: &str, payload: Value) -> Result<Value, RuleError> {
    match command {
        keymap::MODIFIER_GESTURE_STEP => keymap::modifier_gesture_step_json(payload),
        editor::REDUCE_WORKBENCH_STATE => editor::reduce_json(payload),
        visibility::CLASSIFY_VISIBILITY => visibility::classify_visibility_json(payload),
        visibility::VISIBILITY_DEFAULTS => visibility::visibility_defaults_json(payload),
        workbench::REDUCE_CHROME_LAYOUT => workbench::reduce_chrome_layout_json(payload),
        _ => Err(RuleError::new(
            RuleErrorCode::NotSupported,
            "Unknown rule command",
        )),
    }
}
