//! Declarative syntax highlighting of plugin languages (`languages`).
//!
//! A language may name a grammar file in its package and a small editing
//! configuration (comments and brackets). `kind` names the engine that runs
//! the grammar, so a later version can add another engine without changing
//! what an existing grammar means. Today the only engine is `textmate`: a
//! TextMate grammar in the JSON form VS Code reads (`.tmLanguage.json`), so a
//! plugin can ship the grammar a VS Code extension already maintains.
//!
//! The host reads the file from the read-only package and passes its text to
//! `plugins.declarative.decodeGrammar`; Core never opens package files. Core
//! checks the structure the editor's TextMate engine reads and bounds its
//! size, so every grammar Core accepts loads in the editor and none can make
//! it allocate without limit. An include of another grammar's scope resolves
//! against the grammars the editor has; an include nothing defines matches
//! nothing, as in VS Code. `injections` and `injectionSelector` are not run,
//! so they are not checked. Regex cost cannot be proven from the source: the
//! editor tokenizes plugin grammars off its UI thread with a time budget per
//! line, and a pattern its engine cannot compile never matches.
//!
//! Note: the decision is recorded in .agents/notes/implemented/architecture/2026-10-31-plugin-editor-extension-apis.md

use lithe_plugin_model::{invalid, is_safe_relative_path, PluginRejection, Validation};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Largest grammar file, in UTF-8 bytes. VS Code's C++ grammar is 662 KiB.
pub(crate) const MAX_GRAMMAR_BYTES: usize = 2 * 1024 * 1024;
/// Most rules in one grammar: every pattern, repository entry and capture.
const MAX_RULES: usize = 16384;
/// Deepest nesting of rules inside patterns, repositories and captures.
const MAX_RULE_DEPTH: usize = 32;
/// Longest regular expression of `match`, `begin`, `end` or `while`.
const MAX_REGEX_BYTES: usize = 32 * 1024;
/// Highest capture group a rule styles. The editor keeps one slot per group
/// up to the highest, so the number bounds what one rule allocates.
const MAX_CAPTURE_GROUP: u64 = 4096;
/// Longest `scopeName`.
const MAX_SCOPE_NAME_BYTES: usize = 128;
/// Longest `name` or `contentName`, a space-separated list of scopes.
const MAX_SCOPES_BYTES: usize = 1024;
/// Longest `include` or repository key.
const MAX_REFERENCE_BYTES: usize = 256;
/// Most bracket pairs in a language configuration.
const MAX_BRACKETS: usize = 32;
/// Longest comment or bracket delimiter.
const MAX_DELIMITER_BYTES: usize = 16;

/// Engine that runs a grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum GrammarKind {
    /// A TextMate grammar as JSON, the `.tmLanguage.json` files of VS Code.
    Textmate,
}

/// `languages[].grammar`: the engine and the package file it reads.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GrammarDeclaration {
    pub(crate) kind: GrammarKind,
    /// Package-relative grammar file, e.g. `syntaxes/http.tmLanguage.json`.
    pub(crate) path: String,
}

/// `languages[].configuration`: comment and bracket behavior of the editor.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LanguageConfiguration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    line_comment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    block_comment: Option<[String; 2]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    brackets: Vec<[String; 2]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    auto_closing_pairs: Vec<[String; 2]>,
}

impl GrammarDeclaration {
    pub(crate) fn validate(&self, field: &str) -> Validation<()> {
        if !is_safe_relative_path(&self.path) || self.path.contains(':') {
            return Err(invalid(
                format!("{field}.path"),
                "The grammar must be a file inside the plugin package.",
            ));
        }
        Ok(())
    }
}

impl LanguageConfiguration {
    pub(crate) fn validate(&self, field: &str) -> Validation<()> {
        let delimiter = |value: &str| {
            !value.is_empty()
                && value.len() <= MAX_DELIMITER_BYTES
                && !value.chars().any(char::is_control)
        };
        if self
            .line_comment
            .as_deref()
            .is_some_and(|value| !delimiter(value))
        {
            return Err(invalid(
                format!("{field}.lineComment"),
                "A comment delimiter is 1 to 16 bytes without control characters.",
            ));
        }
        if self
            .block_comment
            .as_ref()
            .is_some_and(|[open, close]| !delimiter(open) || !delimiter(close))
        {
            return Err(invalid(
                format!("{field}.blockComment"),
                "A comment delimiter is 1 to 16 bytes without control characters.",
            ));
        }
        for (key, pairs) in [
            ("brackets", &self.brackets),
            ("autoClosingPairs", &self.auto_closing_pairs),
        ] {
            if pairs.len() > MAX_BRACKETS
                || pairs
                    .iter()
                    .any(|[open, close]| !delimiter(open) || !delimiter(close))
            {
                return Err(invalid(
                    format!("{field}.{key}"),
                    "At most 32 pairs, each delimiter 1 to 16 bytes without control characters.",
                ));
            }
        }
        Ok(())
    }
}

fn grammar_error(field: impl Into<String>, message: impl Into<String>) -> PluginRejection {
    PluginRejection::at("plugin.grammar.invalid", field, message)
}

/// Validates the text of a TextMate grammar file and returns its `scopeName`.
/// Hosts register the original text, which the editor's engine parses itself.
pub(crate) fn decode_textmate(text: &str) -> Validation<String> {
    if text.len() > MAX_GRAMMAR_BYTES {
        return Err(grammar_error(
            "grammar",
            "The grammar file is larger than 2 MiB.",
        ));
    }
    let value: Value = serde_json::from_str(text)
        .map_err(|error| grammar_error("grammar", format!("The grammar is not JSON: {error}.")))?;
    let Value::Object(grammar) = &value else {
        return Err(grammar_error(
            "grammar",
            "A TextMate grammar is a JSON object.",
        ));
    };
    let scope_name = match grammar.get("scopeName") {
        Some(Value::String(scope)) if is_scope_name(scope) => scope.clone(),
        _ => {
            return Err(grammar_error(
                "grammar.scopeName",
                "scopeName is 1 to 128 bytes of letters, digits, dots, hyphens, underscores and plus signs.",
            ))
        }
    };
    if grammar.get("name").is_some_and(|name| !name.is_string()) {
        return Err(grammar_error("grammar.name", "name is a string."));
    }
    let mut rules = RuleBudget { count: 0 };
    match grammar.get("patterns") {
        Some(Value::Array(patterns)) => rules.patterns(patterns, "grammar.patterns", 1)?,
        _ => {
            return Err(grammar_error(
                "grammar.patterns",
                "A grammar has a patterns array.",
            ))
        }
    }
    if let Some(repository) = grammar.get("repository") {
        rules.repository(repository, "grammar.repository", 1)?;
    }
    Ok(scope_name)
}

/// Walks the rules of a grammar, counting them against [`MAX_RULES`].
struct RuleBudget {
    count: usize,
}

impl RuleBudget {
    fn patterns(&mut self, patterns: &[Value], field: &str, depth: usize) -> Validation<()> {
        for (index, rule) in patterns.iter().enumerate() {
            self.rule(rule, &format!("{field}[{index}]"), depth)?;
        }
        Ok(())
    }

    fn repository(&mut self, repository: &Value, field: &str, depth: usize) -> Validation<()> {
        let Value::Object(entries) = repository else {
            return Err(grammar_error(field, "repository is an object of rules."));
        };
        for (key, rule) in entries {
            if key.is_empty() || key.len() > MAX_REFERENCE_BYTES {
                return Err(grammar_error(field, "A repository key is 1 to 256 bytes."));
            }
            self.rule(rule, &format!("{field}.{key}"), depth)?;
        }
        Ok(())
    }

    fn rule(&mut self, rule: &Value, field: &str, depth: usize) -> Validation<()> {
        self.count += 1;
        if self.count > MAX_RULES {
            return Err(grammar_error(
                "grammar",
                "A grammar has at most 16384 rules.",
            ));
        }
        if depth > MAX_RULE_DEPTH {
            return Err(grammar_error(field, "Rules nest at most 32 levels deep."));
        }
        let Value::Object(rule) = rule else {
            return Err(grammar_error(field, "A rule is an object."));
        };
        for (key, entry) in rule {
            let entry_field = format!("{field}.{key}");
            match key.as_str() {
                "include" => {
                    if !entry.as_str().is_some_and(|include| {
                        !include.is_empty() && include.len() <= MAX_REFERENCE_BYTES
                    }) {
                        return Err(grammar_error(
                            entry_field,
                            "include is a string of 1 to 256 bytes.",
                        ));
                    }
                }
                "name" | "contentName" => {
                    if entry
                        .as_str()
                        .is_none_or(|scopes| scopes.len() > MAX_SCOPES_BYTES)
                    {
                        return Err(grammar_error(
                            entry_field,
                            format!("{key} is a string of at most 1024 bytes."),
                        ));
                    }
                }
                "match" | "begin" | "end" | "while" => {
                    if entry
                        .as_str()
                        .is_none_or(|regex| regex.len() > MAX_REGEX_BYTES)
                    {
                        return Err(grammar_error(
                            entry_field,
                            format!("{key} is a regular expression of at most 32 KiB."),
                        ));
                    }
                }
                "captures" | "beginCaptures" | "endCaptures" | "whileCaptures" => {
                    self.captures(entry, &entry_field, depth + 1)?;
                }
                "patterns" => {
                    let Value::Array(patterns) = entry else {
                        return Err(grammar_error(entry_field, "patterns is an array of rules."));
                    };
                    self.patterns(patterns, &entry_field, depth + 1)?;
                }
                "repository" => self.repository(entry, &entry_field, depth + 1)?,
                "applyEndPatternLast" => {
                    let flag = entry.is_boolean() || entry.as_u64().is_some_and(|flag| flag <= 1);
                    if !flag {
                        return Err(grammar_error(
                            entry_field,
                            "applyEndPatternLast is a boolean, 0 or 1.",
                        ));
                    }
                }
                // `comment` and other notes the engine does not read.
                _ => {}
            }
        }
        Ok(())
    }

    /// Captures are an object keyed by group number, or an array by position.
    /// Keys that are not group numbers are notes, as in VS Code.
    fn captures(&mut self, captures: &Value, field: &str, depth: usize) -> Validation<()> {
        match captures {
            Value::Object(groups) => {
                for (key, rule) in groups {
                    let Some(group) = capture_group(key) else {
                        continue;
                    };
                    if group > MAX_CAPTURE_GROUP {
                        return Err(grammar_error(
                            format!("{field}.{key}"),
                            "A capture group number is at most 4096.",
                        ));
                    }
                    self.rule(rule, &format!("{field}.{key}"), depth)?;
                }
                Ok(())
            }
            Value::Array(groups) => {
                if groups.len() as u64 > MAX_CAPTURE_GROUP + 1 {
                    return Err(grammar_error(
                        field,
                        "A capture group number is at most 4096.",
                    ));
                }
                for (index, rule) in groups.iter().enumerate() {
                    if !rule.is_null() {
                        self.rule(rule, &format!("{field}[{index}]"), depth)?;
                    }
                }
                Ok(())
            }
            _ => Err(grammar_error(
                field,
                "Captures are an object keyed by group number or an array.",
            )),
        }
    }
}

/// The group number a captures key names, read as the editor reads it: an
/// optionally signed decimal integer, where a negative number names no group.
fn capture_group(key: &str) -> Option<u64> {
    let (negative, digits) = match key.as_bytes().first() {
        Some(b'+') => (false, &key[1..]),
        Some(b'-') => (true, &key[1..]),
        _ => (false, key),
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    // A number too large for a u64 is past every bound.
    let group = digits.parse().unwrap_or(u64::MAX);
    (!negative || group == 0).then_some(group)
}

fn is_scope_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_SCOPE_NAME_BYTES
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-+".contains(character))
}
