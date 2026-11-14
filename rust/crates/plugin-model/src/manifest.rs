//! The plugin host's validated manifest, as a domain crate sees it.

use crate::Validation;
use serde_json::Value;

/// A whole `plugin.json` that the plugin host validated. A domain crate reads
/// its own section through a narrower trait built on this one, so it can
/// validate a manifest it receives without depending on the host that
/// assembles every section.
pub trait PluginManifest: Sized {
    /// Validates a raw manifest value: every section and every rule that
    /// spans sections.
    fn validate(value: Value) -> Validation<Self>;
    /// The plugin ID.
    fn id(&self) -> &str;
}
