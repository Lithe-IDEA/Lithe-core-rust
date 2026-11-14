//! The `${...}` template syntax shared by every template family: launch,
//! run and test, build-system and project-generator templates each parse it
//! with their own variable names.

/// One piece of a parsed `${...}` template: literal text or a variable of the
/// template family that parsed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplatePart<Variable> {
    /// Literal text.
    Text(String),
    /// A variable of the template family.
    Variable(Variable),
}

/// Splits a template into text and variables, using `parse` for the variable
/// names of one template family. `None` for an unknown or unterminated
/// variable. Build-system templates share this parser with plugin templates.
pub fn parse_template_with<Variable>(
    value: &str,
    parse: fn(&str) -> Option<Variable>,
) -> Option<Vec<TemplatePart<Variable>>> {
    let mut parts = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        if start > 0 {
            parts.push(TemplatePart::Text(rest[..start].to_string()));
        }
        let after = &rest[start + 2..];
        let end = after.find('}')?;
        parts.push(TemplatePart::Variable(parse(&after[..end])?));
        rest = &after[end + 1..];
    }
    if !rest.is_empty() {
        parts.push(TemplatePart::Text(rest.to_string()));
    }
    Some(parts)
}

/// Escapes regular-expression metacharacters so a test name matches only
/// literally, in both PCRE (PHPUnit) and RE2 (Go) syntax.
pub fn regex_literal(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if "\\*?+[](){}^$|.".contains(character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}
