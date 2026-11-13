//! `test`: test declarations, filters and result sources.

use super::*;

fn is_test_selector_kind(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 120
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

/// How tests run and how their results are read.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TestDeclaration {
    pub(crate) scopes: Vec<Scope>,
    /// Tags a selected target must carry, as for tasks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) target_tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) filters: Vec<TestFilter>,
    /// Public opaque selector namespaces interpreted only by the owning backend.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) selector_kinds: Vec<String>,
    /// Omitted for argv templates; provider sends a typed test request.
    #[serde(default, skip_serializing_if = "TaskSource::is_command")]
    pub(crate) source: TaskSource,
    /// The provider can enumerate exact selectors for the native test tree.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) discovery: bool,
    pub(crate) arguments: Vec<BuildArgument>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) results: Option<TestResultsSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum TestFilter {
    Suite,
    Case,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(
    tag = "decoder",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum TestResultsSource {
    /// A canonical document returned by the same provider test operation.
    Provider,
    /// JUnit XML report files, globbed relative to the project directory.
    JunitXml { reports: Vec<String> },
    /// Canonical result files, globbed relative to the project directory.
    LitheTestResults { reports: Vec<String> },
}

impl BuildSystemDeclaration {
    pub(crate) fn validate_test(
        &mut self,
        field: &str,
        options: &BTreeSet<String>,
    ) -> Validation<()> {
        let Some(test) = &mut self.test else {
            return Ok(());
        };
        let field = format!("{field}.test");
        check_scopes(
            &mut test.scopes,
            &format!("{field}.scopes"),
            &[Scope::Workspace, Scope::Project, Scope::Target],
        )?;
        check_target_tags(&mut test.target_tags, &test.scopes, &field)?;
        let distinct_selector_kinds: BTreeSet<_> = test.selector_kinds.iter().collect();
        if !test.selector_kinds.is_empty()
            && (test.source != TaskSource::Provider
                || test.selector_kinds.len() > 16
                || distinct_selector_kinds.len() != test.selector_kinds.len()
                || test
                    .selector_kinds
                    .iter()
                    .any(|kind| !is_test_selector_kind(kind)))
        {
            return Err(invalid(
                &field,
                "Test selectorKinds require a provider and bounded identifiers.",
            ));
        }
        test.selector_kinds.sort();
        test.filters.sort();
        test.filters.dedup();
        if test.discovery
            && (test.source != TaskSource::Provider
                || !test.filters.contains(&TestFilter::Suite)
                || !test.filters.contains(&TestFilter::Case))
        {
            return Err(invalid(
                &field,
                "Test discovery requires a provider and suite/case filters.",
            ));
        }
        if test.source == TaskSource::Provider {
            if !test.arguments.is_empty() {
                return Err(invalid(&field, "Provider tests require empty arguments."));
            }
        } else {
            if matches!(test.results, Some(TestResultsSource::Provider)) {
                return Err(invalid(
                    &field,
                    "Provider-owned reports require provider test execution.",
                ));
            }
            check_argument_count(&test.arguments, &format!("{field}.arguments"))?;
            template::validate_arguments(
                &test.arguments,
                &Allowed {
                    root: true,
                    project: availability(&test.scopes, Scope::Project),
                    target: availability(&test.scopes, Scope::Target),
                    test: true,
                    user_arguments: true,
                    conditions: true,
                    options: Some(options),
                    ..Allowed::none()
                },
                &format!("{field}.arguments"),
            )?;
        }
        match &mut test.results {
            Some(TestResultsSource::JunitXml { reports })
            | Some(TestResultsSource::LitheTestResults { reports }) => {
                if reports.is_empty() {
                    return Err(invalid(
                        format!("{field}.results.reports"),
                        "Declare at least one report pattern.",
                    ));
                }
                check_paths(reports, &format!("{field}.results.reports"), true)
            }
            _ => Ok(()),
        }
    }
}
