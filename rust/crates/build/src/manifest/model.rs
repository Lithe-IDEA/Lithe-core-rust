//! `model`: the project model source and the rules that map it.

use super::*;

/// Where the project model comes from.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(
    tag = "source",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum ModelSource {
    /// A file in the instance root, read during detection without starting a
    /// process.
    Static {
        file: String,
        decoder: DocumentDecoder,
        map: ModelMap,
    },
    /// A command the host runs on request; its output is decoded by Core.
    Command {
        arguments: Vec<BuildArgument>,
        #[serde(default)]
        output: OutputChannel,
        decoder: ModelDecoder,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        map: Option<ModelMap>,
        executes_project_code: bool,
        may_use_network: bool,
    },
    /// The provider answers `lithe/build/model` with a `litheBuildModel`.
    Provider,
}

/// Structured formats a static model file may use. A tool whose manifest
/// uses another format gets a plugin command that prints `litheBuildModel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum DocumentDecoder {
    Toml,
}

/// Formats a model command may print.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ModelDecoder {
    /// Whitespace-separated JSON values: NDJSON, concatenated objects such
    /// as `go list -json` prints, or a single document.
    JsonStream,
    /// The canonical model format; needs no map.
    LitheBuildModel,
}

/// Formats a dependency command may print.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum DependencyDecoder {
    /// Whitespace-separated JSON values: NDJSON, concatenated objects such
    /// as `go list -json` prints, or a single document.
    JsonStream,
    /// The canonical dependency format; needs no map.
    LitheDependencies,
}

/// Where a command leaves its result.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum OutputChannel {
    #[default]
    Stdout,
    /// A host-allocated file passed as `${outputFile}`.
    OutputFile,
}

/// Mapping of a decoded document onto the model: projects with their
/// targets and content roots, and option values. The map only selects
/// values; anything that needs more, such as tasks or parent links read from
/// the tool, comes from a plugin command that prints `litheBuildModel`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ModelMap {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) projects: Vec<ProjectRule>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) option_values: BTreeMap<String, OptionValueRule>,
}

/// Fields every rule shares: which elements it reads and when it applies.
/// Written out in each rule because `deny_unknown_fields` and `flatten`
/// cannot be combined.
pub(crate) trait RuleMatch {
    fn each(&self) -> Option<&str>;
    fn filter(&self) -> &[Predicate];
    fn fallback(&self) -> bool;
    fn when_file(&self) -> Option<&str>;
}

macro_rules! rule_match {
    ($rule:ty) => {
        impl RuleMatch for $rule {
            fn each(&self) -> Option<&str> {
                self.each.as_deref()
            }
            fn filter(&self) -> &[Predicate] {
                &self.filter
            }
            fn fallback(&self) -> bool {
                self.fallback
            }
            fn when_file(&self) -> Option<&str> {
                self.when_file.as_deref()
            }
        }
    };
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProjectRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) each: Option<String>,
    #[serde(default, rename = "where", skip_serializing_if = "Vec::is_empty")]
    pub(crate) filter: Vec<Predicate>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) fallback: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) when_file: Option<String>,
    pub(crate) name: FieldSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) display_name: Option<FieldSpec>,
    /// Project directory; the instance root when absent. The parent is the
    /// deepest project containing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) path: Option<FieldSpec>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) properties: BTreeMap<String, FieldSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) targets: Vec<TargetRule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) content_roots: Vec<ContentRootRule>,
}
rule_match!(ProjectRule);

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TargetRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) each: Option<String>,
    #[serde(default, rename = "where", skip_serializing_if = "Vec::is_empty")]
    pub(crate) filter: Vec<Predicate>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) fallback: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) when_file: Option<String>,
    pub(crate) name: FieldSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) path: Option<FieldSpec>,
    pub(crate) tags: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) properties: BTreeMap<String, FieldSpec>,
}
rule_match!(TargetRule);

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ContentRootRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) each: Option<String>,
    #[serde(default, rename = "where", skip_serializing_if = "Vec::is_empty")]
    pub(crate) filter: Vec<Predicate>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) fallback: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) when_file: Option<String>,
    /// Directory relative to the project, or absolute inside the workspace.
    pub(crate) path: FieldSpec,
    pub(crate) kind: ContentRootKind,
}
rule_match!(ContentRootRule);

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OptionValueRule {
    pub(crate) each: String,
    #[serde(default, rename = "where", skip_serializing_if = "Vec::is_empty")]
    pub(crate) filter: Vec<Predicate>,
    pub(crate) value: FieldSpec,
}

/// Role of a source directory in a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ContentRootKind {
    Source,
    Resource,
    TestSource,
    TestResource,
    GeneratedSource,
    GeneratedTestSource,
    Excluded,
}

impl BuildSystemDeclaration {
    pub(crate) fn validate_model(&self, field: &str, options: &BTreeSet<String>) -> Validation<()> {
        let field = format!("{field}.model");
        let model_options: BTreeSet<String> = self
            .options
            .iter()
            .filter(|option| option.choices_from_model)
            .map(|option| option.id.clone())
            .collect();
        match &self.model {
            None => {
                if !model_options.is_empty() {
                    return Err(invalid(field, "choicesFromModel needs a model."));
                }
                Ok(())
            }
            Some(ModelSource::Static { file, map, .. }) => {
                if !is_safe_relative_path(file) {
                    return Err(PluginRejection::at(
                        "plugin.path.invalid",
                        format!("{field}.file"),
                        "The model file must be relative to the build-system root.",
                    ));
                }
                map.validate(&format!("{field}.map"), &model_options)
            }
            Some(ModelSource::Command {
                arguments,
                output,
                decoder,
                map,
                ..
            }) => {
                check_argument_count(arguments, &format!("{field}.arguments"))?;
                template::validate_arguments(
                    arguments,
                    &Allowed {
                        root: true,
                        host_directories: true,
                        options: Some(options),
                        ..Allowed::none()
                    },
                    &format!("{field}.arguments"),
                )?;
                check_output_channel(*output, arguments, &field)?;
                match (decoder, map) {
                    (ModelDecoder::LitheBuildModel, None) => Ok(()),
                    (ModelDecoder::LitheBuildModel, Some(_)) => Err(invalid(
                        format!("{field}.map"),
                        "The litheBuildModel format needs no map.",
                    )),
                    (_, None) => Err(invalid(
                        format!("{field}.map"),
                        "A model command with a generic format needs a map.",
                    )),
                    (_, Some(map)) => map.validate(&format!("{field}.map"), &model_options),
                }
            }
            Some(ModelSource::Provider) => Ok(()),
        }
    }
}

pub(crate) fn check_output_channel(
    output: OutputChannel,
    arguments: &[BuildArgument],
    field: &str,
) -> Validation<()> {
    let uses_file = template::uses(arguments, &Variable::OutputFile);
    match output {
        OutputChannel::OutputFile if !uses_file => Err(invalid(
            format!("{field}.output"),
            "An outputFile command must pass ${outputFile}.",
        )),
        OutputChannel::Stdout if uses_file => Err(invalid(
            format!("{field}.output"),
            "${outputFile} needs \"output\": \"outputFile\".",
        )),
        _ => Ok(()),
    }
}

impl ModelMap {
    fn validate(&self, field: &str, model_options: &BTreeSet<String>) -> Validation<()> {
        if self.projects.is_empty() {
            return Err(invalid(
                format!("{field}.projects"),
                "A model map needs at least one project rule.",
            ));
        }
        check_rule_count(self.projects.len(), &format!("{field}.projects"))?;
        for (index, rule) in self.projects.iter().enumerate() {
            let field = format!("{field}.projects[{index}]");
            check_rule(rule, &field)?;
            rule.name.validate(&format!("{field}.name"))?;
            for spec in [&rule.display_name, &rule.path].into_iter().flatten() {
                spec.validate(&field)?;
            }
            check_properties(&rule.properties, &field)?;
            check_rule_count(rule.targets.len(), &format!("{field}.targets"))?;
            for (target_index, target) in rule.targets.iter().enumerate() {
                target.validate(&format!("{field}.targets[{target_index}]"))?;
            }
            check_rule_count(rule.content_roots.len(), &format!("{field}.contentRoots"))?;
            for (root_index, content_root) in rule.content_roots.iter().enumerate() {
                let field = format!("{field}.contentRoots[{root_index}]");
                check_rule(content_root, &field)?;
                content_root.path.validate(&format!("{field}.path"))?;
            }
        }
        for (id, rule) in &self.option_values {
            let field = format!("{field}.optionValues.{id}");
            if !model_options.contains(id) {
                return Err(invalid(
                    field,
                    "optionValues may only fill options declared with choicesFromModel.",
                ));
            }
            select::validate(&rule.each, &format!("{field}.each"), false)?;
            check_predicates(&rule.filter, &field)?;
            rule.value.validate(&format!("{field}.value"))?;
        }
        Ok(())
    }
}

fn check_rule_count(count: usize, field: &str) -> Validation<()> {
    if count > MAX_LIST {
        return Err(invalid(field, "Too many rules."));
    }
    Ok(())
}

fn check_rule(rule: &impl RuleMatch, field: &str) -> Validation<()> {
    if let Some(each) = rule.each() {
        select::validate(each, &format!("{field}.each"), false)?;
    }
    check_predicates(rule.filter(), field)?;
    if let Some(file) = rule.when_file() {
        if !is_safe_relative_path(file) {
            return Err(PluginRejection::at(
                "plugin.path.invalid",
                format!("{field}.whenFile"),
                "whenFile is relative to the project directory.",
            ));
        }
    }
    Ok(())
}

pub(crate) fn check_predicates(predicates: &[Predicate], field: &str) -> Validation<()> {
    if predicates.len() > 8 {
        return Err(invalid(format!("{field}.where"), "Too many predicates."));
    }
    for (index, predicate) in predicates.iter().enumerate() {
        predicate.validate(&format!("{field}.where[{index}]"))?;
    }
    Ok(())
}

pub(crate) fn check_properties(
    properties: &BTreeMap<String, FieldSpec>,
    field: &str,
) -> Validation<()> {
    if properties.len() > MAX_LIST {
        return Err(invalid(
            format!("{field}.properties"),
            "Too many properties.",
        ));
    }
    for (key, spec) in properties {
        if !template::is_property_key(key) {
            return Err(invalid(
                format!("{field}.properties"),
                "Property keys use letters, digits, ., _, and -.",
            ));
        }
        spec.validate(&format!("{field}.properties.{key}"))?;
    }
    Ok(())
}

impl TargetRule {
    fn validate(&self, field: &str) -> Validation<()> {
        check_rule(self, field)?;
        self.name.validate(&format!("{field}.name"))?;
        if let Some(path) = &self.path {
            path.validate(&format!("{field}.path"))?;
        }
        check_properties(&self.properties, field)?;
        if self.tags.iter().any(|tag| !is_tag(tag)) {
            return Err(invalid(format!("{field}.tags"), "Tags are identifiers."));
        }
        Ok(())
    }
}
