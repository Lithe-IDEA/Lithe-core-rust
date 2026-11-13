//! Keymap rules: binding grammar and conflicts, `when` contexts, saved schemes and the modifier
//! gesture recognizer that runs on every native key event.

mod bindings;
mod context;
mod modifier_gesture;
mod schemes;

pub use bindings::{analyze_conflicts_json, ANALYZE_CONFLICTS};
pub use context::{evaluate_context_json, parse_context_json, EVALUATE_CONTEXT, PARSE_CONTEXT};
pub use modifier_gesture::{
    modifier_gesture_step, modifier_gesture_step_json, GestureEvent, GestureState, GestureStep,
    MODIFIER_GESTURE_STEP, THRESHOLD_SECONDS,
};
pub use schemes::{reduce_schemes_json, REDUCE_SCHEMES};
