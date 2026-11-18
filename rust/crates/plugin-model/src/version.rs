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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
/// Strict three-component semantic version used for compatibility comparisons.
pub struct PluginVersion {
    /// Breaking-change component.
    pub major: u32,
    /// Backward-compatible feature component.
    pub minor: u32,
    /// Backward-compatible fix component.
    pub patch: u32,
}

impl PluginVersion {
    /// Parses exactly `major.minor.patch`; prerelease tags and missing parts are
    /// rejected because the manifest contract does not define their ordering.
    pub fn parse(value: &str) -> Option<Self> {
        let mut parts = value.split('.');
        let version = Self {
            major: parts.next()?.parse().ok()?,
            minor: parts.next()?.parse().ok()?,
            patch: parts.next()?.parse().ok()?,
        };
        parts.next().is_none().then_some(version)
    }
}

/// One comparison of a [`PluginVersionRequirement`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Comparison {
    Exact,
    Greater,
    GreaterOrEqual,
    Less,
    LessOrEqual,
}

/// The plugin versions a stored document accepts, such as `>=1.2.0` or
/// `^0.3.1, <0.3.9`.
///
/// Comma-separated clauses must all hold. A clause is an operator and a
/// `major.minor.patch` version: `=` or no operator is that version only, `>`,
/// `>=`, `<` and `<=` compare, `~1.2.3` keeps major and minor (`>=1.2.3,
/// <1.3.0`) and `^1.2.3` keeps the leftmost non-zero component (`>=1.2.3,
/// <2.0.0`; `^0.2.3` is `<0.3.0`, `^0.0.3` is `<0.0.4`). A bare version is
/// exact rather than caret so a hand-written requirement means what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginVersionRequirement {
    clauses: Vec<(Comparison, PluginVersion)>,
}

impl PluginVersionRequirement {
    /// Parses a requirement; `None` for an empty list, an unknown operator or
    /// a version that is not `major.minor.patch`.
    pub fn parse(value: &str) -> Option<Self> {
        let mut clauses = Vec::new();
        for clause in value.split(',') {
            let clause = clause.trim();
            let (operator, version) = ["^", "~", ">=", "<=", ">", "<", "="]
                .iter()
                .find_map(|operator| {
                    clause
                        .strip_prefix(operator)
                        .map(|rest| (*operator, rest.trim()))
                })
                .unwrap_or(("=", clause));
            let version = PluginVersion::parse(version)?;
            match operator {
                "^" => {
                    clauses.push((Comparison::GreaterOrEqual, version));
                    clauses.push((Comparison::Less, caret_bound(version)));
                }
                "~" => {
                    clauses.push((Comparison::GreaterOrEqual, version));
                    clauses.push((
                        Comparison::Less,
                        PluginVersion {
                            major: version.major,
                            minor: version.minor.checked_add(1)?,
                            patch: 0,
                        },
                    ));
                }
                ">=" => clauses.push((Comparison::GreaterOrEqual, version)),
                "<=" => clauses.push((Comparison::LessOrEqual, version)),
                ">" => clauses.push((Comparison::Greater, version)),
                "<" => clauses.push((Comparison::Less, version)),
                _ => clauses.push((Comparison::Exact, version)),
            }
        }
        Some(Self { clauses })
    }

    /// Whether `version` satisfies every clause.
    pub fn matches(&self, version: PluginVersion) -> bool {
        self.clauses
            .iter()
            .all(|(comparison, bound)| match comparison {
                Comparison::Exact => version == *bound,
                Comparison::Greater => version > *bound,
                Comparison::GreaterOrEqual => version >= *bound,
                Comparison::Less => version < *bound,
                Comparison::LessOrEqual => version <= *bound,
            })
    }
}

/// The exclusive upper bound of `^version`: the next value of its leftmost
/// non-zero component. A component at its maximum has no next value, so the
/// bound stays at the maximum and only versions below it match.
fn caret_bound(version: PluginVersion) -> PluginVersion {
    let next = |component: u32| component.saturating_add(1);
    if version.major > 0 {
        PluginVersion {
            major: next(version.major),
            minor: 0,
            patch: 0,
        }
    } else if version.minor > 0 {
        PluginVersion {
            major: 0,
            minor: next(version.minor),
            patch: 0,
        }
    } else {
        PluginVersion {
            major: 0,
            minor: 0,
            patch: next(version.patch),
        }
    }
}

#[cfg(test)]
mod requirement_tests {
    use super::*;

    fn accepts(requirement: &str, version: &str) -> bool {
        PluginVersionRequirement::parse(requirement)
            .unwrap()
            .matches(PluginVersion::parse(version).unwrap())
    }

    #[test]
    fn operators_compare_versions_numerically() {
        assert!(accepts(">=0.3.1", "0.10.0"));
        assert!(!accepts(">=0.3.1", "0.3.0"));
        assert!(accepts("1.2.3", "1.2.3"));
        assert!(!accepts("=1.2.3", "1.2.4"));
        assert!(accepts(">1.0.0, <2.0.0", "1.9.9"));
        assert!(!accepts(">1.0.0, <2.0.0", "2.0.0"));
        assert!(accepts("<=1.0.0", "1.0.0"));
    }

    // Caret keeps the leftmost non-zero component, tilde keeps major and minor.
    #[test]
    fn caret_and_tilde_bound_the_compatible_range() {
        assert!(accepts("^1.2.3", "1.9.0"));
        assert!(!accepts("^1.2.3", "2.0.0"));
        assert!(accepts("^0.2.3", "0.2.9"));
        assert!(!accepts("^0.2.3", "0.3.0"));
        assert!(!accepts("^0.0.3", "0.0.4"));
        assert!(accepts("~1.2.3", "1.2.9"));
        assert!(!accepts("~1.2.3", "1.3.0"));
    }

    #[test]
    fn malformed_requirements_are_rejected() {
        for requirement in ["", ">=1.2", "~>1.0.0", "1.2.3-beta", ">=1.0.0,"] {
            assert!(
                PluginVersionRequirement::parse(requirement).is_none(),
                "{requirement}"
            );
        }
    }
}
