//! Command routing for the rules `lithe-core` forwards to this crate.

use crate::{action_search, editor, keymap, visibility, workbench, RuleError, RuleErrorCode};
use serde_json::Value;

/// Every command name this crate owns. `lithe-core` routes exactly these names here.
pub const COMMANDS: &[&str] = &[
    action_search::SEARCH_ACTIONS,
    keymap::ANALYZE_CONFLICTS,
    keymap::EVALUATE_CONTEXT,
    keymap::MODIFIER_GESTURE_STEP,
    keymap::PARSE_CONTEXT,
    keymap::REDUCE_SCHEMES,
    editor::REDUCE_WORKBENCH_STATE,
    visibility::CLASSIFY_VISIBILITY,
    visibility::VISIBILITY_DEFAULTS,
    workbench::REDUCE_CHROME_LAYOUT,
];

/// Runs one rule on an already decoded payload.
pub fn execute(command: &str, payload: Value) -> Result<Value, RuleError> {
    match command {
        action_search::SEARCH_ACTIONS => action_search::search_actions_json(payload),
        keymap::ANALYZE_CONFLICTS => keymap::analyze_conflicts_json(payload),
        keymap::EVALUATE_CONTEXT => keymap::evaluate_context_json(payload),
        keymap::MODIFIER_GESTURE_STEP => keymap::modifier_gesture_step_json(payload),
        keymap::PARSE_CONTEXT => keymap::parse_context_json(payload),
        keymap::REDUCE_SCHEMES => keymap::reduce_schemes_json(payload),
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
