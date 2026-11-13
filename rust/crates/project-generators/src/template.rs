//! Variables of project generator templates.
//!
//! Strings in the manifest use the `${...}` syntax every plugin template
//! uses. Template files in the package use `{{ ... }}` instead, because build
//! scripts and property files of many tools already contain `${...}` that
//! must reach the project unchanged. In files, only `{{` followed by
//! `project.` or `field.` is a variable; any other `{{` is copied as text.

use lithe_plugin_model::{parse_template_with, TemplatePart};
use std::collections::BTreeMap;

/// What a variable reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Source {
    /// The project name the user typed.
    ProjectName,
    /// A field's value: text, the chosen option, `true`/`false`, a
    /// comma-separated list, or the home directory of a tool.
    Field(String),
    /// The full version of a tool field's selection.
    ToolVersion(String),
    /// The major version of a tool field's selection, such as `21`.
    ToolMajor(String),
    /// The executable of a tool field's selection, the file the tool
    /// definition resolved inside the home.
    ToolExecutable(String),
}

/// A transformation applied after the value is read, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Modifier {
    /// Dots become `/`: a package name as a source directory.
    Path,
    /// A dotted package identifier: lowercase, only letters, digits,
    /// underscores, and single dots, each segment starting with a letter or
    /// underscore. `com.example.my-app` becomes `com.example.myapp`.
    Package,
    /// Lowercase.
    Lower,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Variable {
    pub(crate) source: Source,
    pub(crate) modifiers: Vec<Modifier>,
}

impl Variable {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split('|').map(str::trim);
        let name = parts.next()?;
        let source = if name == "project.name" {
            Source::ProjectName
        } else {
            let rest = name.strip_prefix("field.")?;
            match rest.split_once('.') {
                None => Source::Field(rest.to_string()),
                Some((id, "version")) => Source::ToolVersion(id.to_string()),
                Some((id, "major")) => Source::ToolMajor(id.to_string()),
                Some((id, "executable")) => Source::ToolExecutable(id.to_string()),
                Some(_) => return None,
            }
        };
        let modifiers = parts
            .map(|modifier| match modifier {
                "path" => Some(Modifier::Path),
                "package" => Some(Modifier::Package),
                "lower" => Some(Modifier::Lower),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self { source, modifiers })
    }

    /// The field the variable reads, if any.
    #[cfg(test)]
    pub(crate) fn field(&self) -> Option<&str> {
        match &self.source {
            Source::ProjectName => None,
            Source::Field(id)
            | Source::ToolVersion(id)
            | Source::ToolMajor(id)
            | Source::ToolExecutable(id) => Some(id),
        }
    }
}

/// Parses a manifest string. `None` for an unknown or unterminated variable.
pub(crate) fn parse(value: &str) -> Option<Vec<TemplatePart<Variable>>> {
    parse_template_with(value, Variable::parse)
}

/// A filled-in field, as templates read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FieldValue {
    Text(String),
    Flag(bool),
    List(Vec<String>),
    /// The installation a tool field selected.
    Tool {
        home: String,
        executable: String,
        version: String,
    },
}

impl FieldValue {
    pub(crate) fn text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Flag(flag) => flag.to_string(),
            Self::List(items) => items.join(","),
            Self::Tool { home, .. } => home.clone(),
        }
    }
}

/// Everything a template may read.
pub(crate) struct Values<'a> {
    pub(crate) project_name: &'a str,
    pub(crate) fields: &'a BTreeMap<String, FieldValue>,
}

impl Values<'_> {
    pub(crate) fn read(&self, variable: &Variable) -> String {
        let raw = match &variable.source {
            Source::ProjectName => self.project_name.to_string(),
            Source::Field(id) => self
                .fields
                .get(id)
                .map(FieldValue::text)
                .unwrap_or_default(),
            Source::ToolVersion(id) => match self.fields.get(id) {
                Some(FieldValue::Tool { version, .. }) => version.clone(),
                _ => String::new(),
            },
            Source::ToolMajor(id) => match self.fields.get(id) {
                Some(FieldValue::Tool { version, .. }) => major_version(version),
                _ => String::new(),
            },
            Source::ToolExecutable(id) => match self.fields.get(id) {
                Some(FieldValue::Tool { executable, .. }) => executable.clone(),
                _ => String::new(),
            },
        };
        variable
            .modifiers
            .iter()
            .fold(raw, |value, modifier| apply(*modifier, &value))
    }

    pub(crate) fn expand(&self, parts: &[TemplatePart<Variable>]) -> String {
        parts
            .iter()
            .map(|part| match part {
                TemplatePart::Text(text) => text.clone(),
                TemplatePart::Variable(variable) => self.read(variable),
            })
            .collect()
    }

    /// Expands a template file. Fails with the offending text when a
    /// `{{ project.… }}` or `{{ field.… }}` variable cannot be read.
    pub(crate) fn expand_file(
        &self,
        contents: &str,
        known_field: impl Fn(&Variable) -> bool,
    ) -> Result<String, String> {
        let mut output = String::with_capacity(contents.len());
        let mut rest = contents;
        while let Some(start) = rest.find("{{") {
            output.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let inner_start = after.trim_start();
            if !(inner_start.starts_with("project.") || inner_start.starts_with("field.")) {
                output.push_str("{{");
                rest = after;
                continue;
            }
            let Some(end) = after.find("}}") else {
                return Err(format!("{{{{{}", first_line(after)));
            };
            let inner = &after[..end];
            match Variable::parse(inner).filter(|variable| known_field(variable)) {
                Some(variable) => output.push_str(&self.read(&variable)),
                None => return Err(format!("{{{{{inner}}}}}")),
            }
            rest = &after[end + 2..];
        }
        output.push_str(rest);
        Ok(output)
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

fn apply(modifier: Modifier, value: &str) -> String {
    match modifier {
        Modifier::Path => value.replace('.', "/"),
        Modifier::Lower => value.to_lowercase(),
        Modifier::Package => package_name(value),
    }
}

/// The first numeric component: `21.0.2` → `21`, `25` → `25`.
///
/// Tool-specific numbering (such as a legacy `1.` prefix) is normalized by the
/// tool definition's `version.pattern` capture before the version reaches a
/// template, so this reader never interprets a tool's scheme.
pub(crate) fn major_version(version: &str) -> String {
    version
        .split(|character: char| !character.is_ascii_digit())
        .find(|part| !part.is_empty())
        .unwrap_or_default()
        .to_string()
}

fn package_name(value: &str) -> String {
    value
        .to_lowercase()
        .split('.')
        .map(|segment| {
            segment
                .chars()
                .filter(|character| character.is_ascii_alphanumeric() || *character == '_')
                .collect::<String>()
        })
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            if segment.starts_with(|character: char| character.is_ascii_digit()) {
                format!("_{segment}")
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(fields: &[(&str, FieldValue)]) -> BTreeMap<String, FieldValue> {
        fields
            .iter()
            .map(|(id, value)| (id.to_string(), value.clone()))
            .collect()
    }

    #[test]
    fn parses_sources_and_modifiers() {
        assert_eq!(
            Variable::parse("field.packageName | path"),
            Some(Variable {
                source: Source::Field("packageName".into()),
                modifiers: vec![Modifier::Path],
            })
        );
        assert_eq!(
            Variable::parse("field.compiler.major").map(|variable| variable.source),
            Some(Source::ToolMajor("compiler".into()))
        );
        assert_eq!(
            Variable::parse("field.compiler.version").map(|variable| variable.source),
            Some(Source::ToolVersion("compiler".into()))
        );
        assert_eq!(
            Variable::parse("field.python.executable").map(|variable| variable.source),
            Some(Source::ToolExecutable("python".into()))
        );
        assert!(Variable::parse("field.compiler.home").is_none());
        assert!(Variable::parse("project.path").is_none());
        assert!(Variable::parse("field.name|upper").is_none());
    }

    #[test]
    fn tool_fields_read_as_home_executable_and_version() {
        let fields = values(&[(
            "python",
            FieldValue::Tool {
                home: "/opt/python".into(),
                executable: "/opt/python/bin/python3".into(),
                version: "3.12.4".into(),
            },
        )]);
        let values = Values {
            project_name: "demo",
            fields: &fields,
        };
        let expand = |text: &str| values.expand(&parse(text).unwrap());
        assert_eq!(expand("${field.python}"), "/opt/python");
        assert_eq!(
            expand("${field.python.executable} -m venv ."),
            "/opt/python/bin/python3 -m venv ."
        );
        assert_eq!(expand("${field.python.version}"), "3.12.4");
        assert_eq!(expand("${field.python.major}"), "3");
        // A field that holds no tool reads as empty, never as another value.
        assert_eq!(expand("${field.missing.executable}"), "");
        assert_eq!(expand("${field.missing.version}"), "");
        assert_eq!(expand("${field.missing.major}"), "");
    }

    #[test]
    fn package_modifier_makes_a_valid_package() {
        assert_eq!(package_name("com.Example.my-app"), "com.example.myapp");
        assert_eq!(package_name("org..2fa.demo"), "org._2fa.demo");
    }

    #[test]
    fn major_versions_take_the_first_numeric_component() {
        assert_eq!(major_version("8.0_412"), "8");
        assert_eq!(major_version("1.8.0"), "1");
        assert_eq!(major_version("25.0.3"), "25");
        assert_eq!(major_version(""), "");
    }

    #[test]
    fn files_only_expand_project_and_field_variables() {
        let fields = values(&[("groupId", FieldValue::Text("com.example".into()))]);
        let values = Values {
            project_name: "demo",
            fields: &fields,
        };
        let expanded = values
            .expand_file(
                "<groupId>{{ field.groupId }}</groupId>${project.version} {{other}} {{project.name}}",
                |_| true,
            )
            .unwrap();
        assert_eq!(
            expanded,
            "<groupId>com.example</groupId>${project.version} {{other}} demo"
        );
        assert_eq!(
            values.expand_file("{{ field.missing }}", |variable| variable.field()
                != Some("missing")),
            Err("{{ field.missing }}".into())
        );
    }
}
