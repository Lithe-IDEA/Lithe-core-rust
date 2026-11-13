//! `detect` and `nesting`: how a build system recognizes its projects.

use super::*;

/// How instances of the build system are recognized during the shared walk.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Detection {
    /// Exact file names; a directory containing any of them is a candidate.
    pub(crate) markers: Vec<String>,
    #[serde(default)]
    pub(crate) nesting: Nesting,
    /// Decides between plugins declaring the same build-system ID.
    #[serde(default = "default_priority")]
    pub(crate) priority: u8,
}

fn default_priority() -> u8 {
    DEFAULT_PRIORITY
}

/// What a marker below another instance's root means.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Nesting {
    /// Every candidate directory is its own instance (Cargo packages).
    #[default]
    Separate,
    /// Only candidates with no candidate above them are instances; deeper
    /// markers belong to them (multi-project builds and reactors).
    Outermost,
}

impl BuildSystemDeclaration {
    pub(crate) fn validate_detection(&mut self, field: &str) -> Validation<()> {
        let detection = &mut self.detection;
        let markers = format!("{field}.detection.markers");
        if detection.markers.is_empty() || detection.markers.len() > MAX_LIST {
            return Err(invalid(markers, "Detection needs 1-32 marker file names."));
        }
        if let Some(index) = detection
            .markers
            .iter()
            .position(|marker| !is_file_name(marker))
        {
            return Err(invalid(
                format!("{markers}[{index}]"),
                "A marker is a file name without a directory.",
            ));
        }
        detection.markers = sorted_unique(std::mem::take(&mut detection.markers));
        if detection.priority > MAX_PRIORITY {
            return Err(invalid(
                format!("{field}.detection.priority"),
                "The priority must be 0-100.",
            ));
        }
        Ok(())
    }
}
