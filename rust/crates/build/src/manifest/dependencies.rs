//! `dependencies`, `sync` and `layout` declarations.

use super::*;

/// Where dependency trees come from.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(
    tag = "source",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
#[allow(clippy::large_enum_variant)] // Decoded once per manifest validation.
pub(crate) enum DependenciesSource {
    Command {
        scopes: Vec<Scope>,
        arguments: Vec<BuildArgument>,
        #[serde(default)]
        output: OutputChannel,
        decoder: DependencyDecoder,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        map: Option<DependencyRule>,
        executes_project_code: bool,
        may_use_network: bool,
    },
    /// The provider answers `lithe/build/dependencies` with
    /// `litheDependencies`.
    Provider { scopes: Vec<Scope> },
}

/// Mapping of one dependency element onto a flat list. Dependency trees
/// come from `litheDependencies`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DependencyRule {
    pub(crate) each: String,
    #[serde(default, rename = "where", skip_serializing_if = "Vec::is_empty")]
    pub(crate) filter: Vec<Predicate>,
    pub(crate) name: FieldSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) group: Option<FieldSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) version: Option<FieldSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) scope: Option<FieldSpec>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) properties: BTreeMap<String, FieldSpec>,
}

/// Files whose change makes the model stale, relative to the instance root
/// and to each project directory.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SyncDeclaration {
    pub(crate) watch: Vec<String>,
}

/// Output directories excluded from every project, relative to each
/// project directory.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LayoutDeclaration {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) excluded: Vec<String>,
}

impl BuildSystemDeclaration {
    pub(crate) fn validate_dependencies(
        &mut self,
        field: &str,
        options: &BTreeSet<String>,
    ) -> Validation<()> {
        if let Some(DependenciesSource::Provider { scopes }) = &mut self.dependencies {
            return check_scopes(
                scopes,
                &format!("{field}.dependencies.scopes"),
                &[Scope::Workspace, Scope::Project],
            );
        }
        let Some(DependenciesSource::Command {
            scopes,
            arguments,
            output,
            decoder,
            map,
            ..
        }) = &mut self.dependencies
        else {
            return Ok(());
        };
        let field = format!("{field}.dependencies");
        check_scopes(
            scopes,
            &format!("{field}.scopes"),
            &[Scope::Workspace, Scope::Project],
        )?;
        check_argument_count(arguments, &format!("{field}.arguments"))?;
        template::validate_arguments(
            arguments,
            &Allowed {
                root: true,
                project: availability(scopes, Scope::Project),
                host_directories: true,
                conditions: true,
                options: Some(options),
                ..Allowed::none()
            },
            &format!("{field}.arguments"),
        )?;
        check_output_channel(*output, arguments, &field)?;
        match (decoder, map) {
            (DependencyDecoder::LitheDependencies, None) => Ok(()),
            (DependencyDecoder::LitheDependencies, Some(_)) => Err(invalid(
                format!("{field}.map"),
                "The litheDependencies format needs no map.",
            )),
            (_, None) => Err(invalid(
                format!("{field}.map"),
                "A dependency command with a generic format needs a map.",
            )),
            (_, Some(rule)) => rule.validate(&format!("{field}.map")),
        }
    }
}

impl DependencyRule {
    fn validate(&self, field: &str) -> Validation<()> {
        select::validate(&self.each, &format!("{field}.each"), false)?;
        check_predicates(&self.filter, field)?;
        self.name.validate(&format!("{field}.name"))?;
        for spec in [&self.group, &self.version, &self.scope]
            .into_iter()
            .flatten()
        {
            spec.validate(field)?;
        }
        check_properties(&self.properties, field)
    }
}
