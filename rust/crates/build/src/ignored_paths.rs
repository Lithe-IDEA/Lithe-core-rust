//! The directories build detection does not look into, a Build option of the
//! workspace. Patterns use `.gitignore` syntax relative to the workspace root
//! (see [`IgnoredPaths`]) and are stored as `ignoredPaths` in the team and
//! machine-local Build documents, the local list replacing the team's. With
//! neither, detection skips every directory whose name starts with a dot:
//! those hold tool state, caches and nested worktrees, whose copies of a
//! project would otherwise show up as more build targets.

use lithe_execution::IgnoredPaths;
use lithe_plugin_model::{PluginRejection, Validation};
use std::path::Path;

/// The patterns used when no Build document sets `ignoredPaths`.
pub(super) const DEFAULT: [&str; 1] = [".*"];

const MAX_PATTERNS: usize = 256;
const MAX_PATTERN_BYTES: usize = 1024;

/// The rejection for a list that cannot be used, naming the first entry that is
/// not accepted (`ignoredPaths[<index>]`), or the whole list when every entry
/// is a pattern but they cannot be compiled together.
fn invalid(index: usize, count: usize) -> PluginRejection {
    let field = if index < count {
        format!("ignoredPaths[{index}]")
    } else {
        "ignoredPaths".to_owned()
    };
    PluginRejection::at(
        "plugin.build.invalidIgnoredPath",
        field,
        "The ignored path is not a .gitignore pattern",
    )
}

/// Compiles the patterns for the workspace at `root`, or the default when
/// `patterns` is `None`. A list holds at most 256 patterns of at most 1024
/// bytes each.
pub(super) fn compile(root: &Path, patterns: Option<&[String]>) -> Validation<IgnoredPaths> {
    let Some(patterns) = patterns else {
        return compile(root, Some(&DEFAULT.map(str::to_owned)));
    };
    let accepted = patterns
        .iter()
        .take(MAX_PATTERNS)
        .take_while(|pattern| pattern.len() <= MAX_PATTERN_BYTES)
        .count();
    let ignored = IgnoredPaths::new(root, &patterns[..accepted])
        .map_err(|index| invalid(index, patterns.len()))?;
    if accepted < patterns.len() {
        return Err(invalid(accepted, patterns.len()));
    }
    Ok(ignored)
}

/// Checks a list before it is stored; storage is never relative to one root.
pub(super) fn check(patterns: &[String]) -> Validation<()> {
    compile(Path::new("/"), Some(patterns)).map(|_| ())
}
