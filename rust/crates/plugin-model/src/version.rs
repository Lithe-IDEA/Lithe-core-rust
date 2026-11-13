//! Numeric versions in manifests and in a runtime's version output.

use crate::rejection::{invalid, Validation};

/// A minimum version is one to three dot-separated numbers; hosts compare it
/// numerically against the leading components of the runtime's own version.
pub fn validate_minimum_version(value: &str, field: &str) -> Validation<()> {
    let parts: Vec<&str> = value.split('.').collect();
    let numeric = parts.iter().all(|part| {
        !part.is_empty()
            && part.len() <= 9
            && part.chars().all(|character| character.is_ascii_digit())
    });
    if numeric && parts.len() <= 3 {
        Ok(())
    } else {
        Err(invalid(
            field.to_string(),
            "A minimum version must be one to three dot-separated numbers.",
        ))
    }
}

/// The numeric components of a dotted version; non-numeric components are skipped.
pub fn version_components(value: &str) -> Vec<u64> {
    value
        .split('.')
        .filter_map(|part| part.parse().ok())
        .collect()
}

/// First dotted number in a version probe's output, at most three
/// components: `20.11.0` from `v20.11.0`, `1.22.4` from
/// `go version go1.22.4 darwin/arm64`. A tool whose output needs more than
/// this is checked through its tool definition instead.
pub fn reported_version(output: &str) -> Option<Vec<u64>> {
    let line = output
        .lines()
        .find(|line| line.contains(|character: char| character.is_ascii_digit()))?;
    let start = line.find(|character: char| character.is_ascii_digit())?;
    let text: String = line[start..]
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    // A sentence-ending period is not a component.
    text.split('.')
        .take_while(|part| !part.is_empty())
        .take(3)
        .map(|part| part.parse().ok())
        .collect()
}
