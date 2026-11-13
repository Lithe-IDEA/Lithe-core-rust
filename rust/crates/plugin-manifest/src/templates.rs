//! Command templates: their variables, the contexts that allow each variable, and
//! parsing.

use super::*;

/// Template variables. `${userArguments}` expands to zero or more elements;
/// the others substitute into a single element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TemplateVariable {
    File,
    FileDirectory,
    TestName,
    TestNameRegex,
    UserArguments,
}

impl TemplateVariable {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "file" => Some(Self::File),
            "fileDirectory" => Some(Self::FileDirectory),
            "testName" => Some(Self::TestName),
            "testName:regex" => Some(Self::TestNameRegex),
            "userArguments" => Some(Self::UserArguments),
            _ => None,
        }
    }

    pub(crate) fn needs_file(self) -> bool {
        matches!(self, Self::File | Self::FileDirectory)
    }
}

/// Which command template an argument list belongs to; each allows a
/// different set of variables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TemplateContext {
    Run,
    TestWorkspace,
    TestFile,
    TestCase,
}

impl TemplateContext {
    pub(crate) fn allows(self, variable: TemplateVariable) -> bool {
        use TemplateVariable::*;
        match self {
            Self::Run => matches!(variable, File | FileDirectory | UserArguments),
            Self::TestWorkspace => false,
            Self::TestFile => matches!(variable, File | FileDirectory),
            Self::TestCase => matches!(variable, File | FileDirectory | TestName | TestNameRegex),
        }
    }
}

pub(crate) fn parse_template(value: &str) -> Option<Vec<TemplatePart<TemplateVariable>>> {
    parse_template_with(value, TemplateVariable::parse)
}

impl ArgumentTemplate {
    pub(crate) fn value(&self) -> &str {
        match self {
            Self::Literal(value) | Self::Conditional { value, .. } => value,
        }
    }

    pub(crate) fn condition(&self) -> Option<ArgumentCondition> {
        match self {
            Self::Literal(_) => None,
            Self::Conditional { when, .. } => Some(*when),
        }
    }
}
