//! `diagnostics`: output line patterns and their severities.

use super::*;

/// How build output becomes file diagnostics.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(
    tag = "decoder",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum DiagnosticsDeclaration {
    LineMatcher {
        patterns: Vec<LinePattern>,
    },
    /// Output lines holding canonical diagnostic objects.
    LitheDiagnostics,
}

/// A regular expression and which capture group holds each field.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LinePattern {
    pub(crate) regex: String,
    pub(crate) file: usize,
    pub(crate) line: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) column: Option<usize>,
    pub(crate) message: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) severity: Option<usize>,
    /// Raw severity text to severity; without it the text is lowercased.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) severities: BTreeMap<String, Severity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) default_severity: Option<Severity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

impl Severity {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Information => "information",
            Self::Hint => "hint",
        }
    }
}

impl BuildSystemDeclaration {
    pub(crate) fn validate_diagnostics(&self, field: &str) -> Validation<()> {
        let Some(DiagnosticsDeclaration::LineMatcher { patterns }) = &self.diagnostics else {
            return Ok(());
        };
        let field = format!("{field}.diagnostics.patterns");
        if patterns.is_empty() || patterns.len() > MAX_LIST {
            return Err(invalid(field, "A line matcher needs 1-32 patterns."));
        }
        for (index, pattern) in patterns.iter().enumerate() {
            let field = format!("{field}[{index}]");
            let groups = [
                Some(pattern.file),
                Some(pattern.line),
                pattern.column,
                Some(pattern.message),
                pattern.severity,
            ];
            let highest = groups.iter().flatten().copied().max().unwrap_or(0);
            check_regex(&pattern.regex, &format!("{field}.regex"), highest)?;
            if groups.iter().flatten().any(|group| *group == 0) {
                return Err(invalid(field, "Capture groups are numbered from 1."));
            }
            if !pattern.severities.is_empty() && pattern.severity.is_none() {
                return Err(invalid(
                    format!("{field}.severities"),
                    "Severity names need a severity group.",
                ));
            }
        }
        Ok(())
    }
}
