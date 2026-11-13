//! Plugin-owned toolchain policy; hosts supply trust, filesystem and probe facts.
//!
//! No manager configuration, native scanning or process lifecycle belongs here.

pub mod definitions;
pub mod fixed;
pub mod paths;
mod pin_sources;
pub mod pins;
pub mod preferences;
pub mod query;
mod resolution;

use lithe_plugin_model::{PluginRejection, Validation};
use lithe_protocol::CoreError;
use serde_json::{json, Value};

/// Revalidates one complete plugin manifest and returns its normalized JSON form.
///
/// The plugin host owns the manifest schema; it supplies this function so toolchain
/// policy reads only manifests that passed the same validation as installation.
pub type ManifestValidator = fn(Value) -> Validation<Value>;

fn command(
    input: Value,
    operation: impl FnOnce(&Value) -> Validation<Value>,
    key: &str,
) -> Result<Value, CoreError> {
    let result = if input.to_string().len() > 4 * 1024 * 1024 {
        Err(PluginRejection::new(
            "plugin.toolchain.limitExceeded",
            "Toolchain input exceeds its byte budget.",
        ))
    } else {
        operation(&input)
    };
    Ok(match result {
        Ok(value) => json!({"status":"ok",key:value}),
        Err(error) => json!({"status":"rejected","error":error}),
    })
}

/// Registers the definitions of enabled plugins; a tool ID declared by several plugins
/// belongs to none of them and is reported in `conflicts` instead. `validate_manifest`
/// revalidates each manifest; see [`definitions::registry`].
pub fn registry_command(
    input: Value,
    validate_manifest: ManifestValidator,
) -> Result<Value, CoreError> {
    command(
        input,
        |input| definitions::registry(input, validate_manifest),
        "registry",
    )
}

/// Validates one provider reply against its request and host filesystem facts.
/// `validate_manifest` revalidates the registry it carries.
pub fn decode_reply_command(
    input: Value,
    validate_manifest: ManifestValidator,
) -> Result<Value, CoreError> {
    command(
        input,
        |input| query::decode(input, validate_manifest),
        "snapshot",
    )
}

/// Builds the error reply for a query that timed out or found no backend.
pub fn failure_reply_command(input: Value) -> Result<Value, CoreError> {
    command(input, query::failure_reply, "reply")
}

/// Reads the nearest directory pin without opening or writing native files.
pub fn resolve_pins_command(input: Value) -> Result<Value, CoreError> {
    command(input, pins::resolve, "resolution")
}

/// Applies one atomic batch of inert directory-pin edits without native writes.
pub fn edit_pins_command(input: Value) -> Result<Value, CoreError> {
    command(input, pins::edit, "edit")
}

/// Plans bounded reads of native workspace pin sources without opening them.
pub fn pin_storage_plan_command(input: Value) -> Result<Value, CoreError> {
    command(input, pin_sources::storage_plan, "storage")
}

/// Decodes a complete pin source without host-side schema parsing.
pub fn decode_pin_document_command(input: Value) -> Result<Value, CoreError> {
    command(input, pin_sources::decode, "document")
}

/// Plans conditional publication of the pin document and its ignore prerequisite.
pub fn edit_pin_sources_command(input: Value) -> Result<Value, CoreError> {
    command(input, pin_sources::edit, "edit")
}

/// Resolves inert consumer choices without materializing symbolic inheritance.
pub fn resolve_consumer_preferences_command(input: Value) -> Result<Value, CoreError> {
    command(input, preferences::resolve, "preferences")
}

/// Changes one inert consumer preference without native writes or side effects.
pub fn edit_consumer_preferences_command(input: Value) -> Result<Value, CoreError> {
    command(input, preferences::edit, "edit")
}

/// Resolves one tool using the same validated registry and candidate snapshot.
pub fn resolve_command(
    input: Value,
    validate_manifest: ManifestValidator,
) -> Result<Value, CoreError> {
    command(
        input,
        |input| resolution::resolve(input, validate_manifest),
        "resolution",
    )
}

/// Extracts and compares a version using only its enabled plugin definition.
pub fn check_version_command(
    input: Value,
    validate_manifest: ManifestValidator,
) -> Result<Value, CoreError> {
    command(
        input,
        |input| resolution::check_version(input, validate_manifest),
        "version",
    )
}

/// Expands a tool definition's launch template for a selected executable.
pub fn instantiate_launch_command(
    input: Value,
    validate_manifest: ManifestValidator,
) -> Result<Value, CoreError> {
    command(
        input,
        |input| resolution::instantiate_launch(input, validate_manifest),
        "launch",
    )
}

/// Produces bounded scan declarations, not a native scan or tool-name table.
pub fn discovery_plan_command(
    input: Value,
    validate_manifest: ManifestValidator,
) -> Result<Value, CoreError> {
    command(
        input,
        |input| resolution::discovery_plan(input, validate_manifest),
        "plan",
    )
}

/// Lists checked installations, versions and the automatic choice for a settings page.
pub fn candidates_command(
    input: Value,
    validate_manifest: ManifestValidator,
) -> Result<Value, CoreError> {
    command(
        input,
        |input| resolution::candidates(input, validate_manifest),
        "candidates",
    )
}
