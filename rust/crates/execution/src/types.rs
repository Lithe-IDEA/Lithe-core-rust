//! Shared classification types of run configurations.

use serde::{Deserialize, Serialize};

/// How a configuration behaves once started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum Execution {
    /// A user-facing process that may be interactive but is not a background service.
    #[default]
    Application,
    /// A long-running process expected to keep serving until explicitly stopped.
    Service,
    /// A finite command expected to exit after producing its result.
    Task,
    /// A non-process entry that groups other configurations for coordinated launch.
    Group,
}

/// How the configuration came to exist. Higher values win deduplication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum Confidence {
    /// Inferred from naming or source-layout conventions rather than a declaration.
    Heuristic,
    /// Supported by a manifest, build plugin, dependency, or other project declaration.
    #[default]
    Declared,
    /// Authored by Lithe itself and therefore stronger than any declaration.
    Native,
}

/// Whether a configuration runs the project itself or the external services it
/// depends on.
///
/// An application repository often declares the databases and caches it runs
/// against beside the application. Both are runnable, but listing every
/// dependency beside the one application service buries it, so hosts group
/// infrastructure separately instead of dropping it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum RunCategory {
    /// Code in this repository: a service, application, or build task.
    #[default]
    Project,
    /// External dependencies the project runs against, such as a database.
    Infrastructure,
}

impl RunCategory {
    /// Lets serialization omit the common case so generated documents stay small.
    pub fn is_project(&self) -> bool {
        matches!(self, Self::Project)
    }
}
