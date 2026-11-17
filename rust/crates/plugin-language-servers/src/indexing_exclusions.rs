//! Indexing exclusions a language server reads from its own settings.
//!
//! Some servers index every directory of the workspace unless one of their
//! own settings lists the directories to skip, and each server names that
//! setting and writes a directory differently. A server therefore declares
//! `indexingExclusions`: the array in its workspace configuration and the
//! form of one entry. The LSP engine fills that array with the workspace's
//! excluded directories, so Core never matches on a language, a server or a
//! tool name to find it.

use lithe_plugin_model::{invalid, Validation};
use serde::{Deserialize, Serialize};

/// The placeholder `entry` must contain exactly once.
const PATH_PLACEHOLDER: &str = "${path}";
/// Characters a glob reads as a pattern rather than as part of a name.
const GLOB_METACHARACTERS: [char; 6] = ['*', '?', '[', ']', '{', '}'];
/// Most keys in `section`, and most entries in `default`.
const MAX_SECTION_KEYS: usize = 8;
const MAX_DEFAULT_ENTRIES: usize = 32;
/// Longest `section`, `entry`, `rootEntry` or `default` entry, in UTF-8 bytes.
const MAX_TEXT_BYTES: usize = 256;

/// `indexingExclusions` of a language server: where its workspace
/// configuration lists excluded directories, and how it writes one.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IndexingExclusions {
    /// Dotted path of the array in the workspace configuration, such as
    /// `gopls.directoryFilters`.
    section: String,
    /// One excluded directory, where `${path}` is its workspace-relative
    /// path separated by `/`.
    entry: String,
    /// The entry for the workspace root itself. Without it the root is
    /// `entry` with the path `.`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root_entry: Option<String>,
    /// What the server uses when the configuration has no such array. The
    /// excluded directories are added after it, so the server's own default
    /// still applies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    default: Vec<String>,
    /// How the server reads a path in an entry.
    #[serde(default)]
    path_syntax: PathSyntax,
}

/// How a server reads the path in an exclusion entry.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PathSyntax {
    /// The path is a literal name.
    #[default]
    Literal,
    /// The path is a glob. A directory whose name contains a glob
    /// metacharacter cannot be written literally, so it is left out rather
    /// than excluding the other directories the pattern would match.
    Glob,
}

impl IndexingExclusions {
    /// Checks the declaration at `field`. The engine checks a session
    /// policy again with the same rules, since `lsp.startServer` is public.
    pub fn validate(&self, field: &str) -> Validation<()> {
        let keys = self.section_keys();
        if self.section.len() > MAX_TEXT_BYTES
            || keys.len() > MAX_SECTION_KEYS
            || keys.iter().any(|key| key.is_empty() || !is_plain_text(key))
            || self.section.contains(char::is_whitespace)
        {
            return Err(invalid(
                format!("{field}.section"),
                "section is a dotted path of at most 8 non-empty keys without whitespace.",
            ));
        }
        if !is_entry_text(&self.entry)
            || self.entry.matches(PATH_PLACEHOLDER).count() != 1
            || self.entry.replacen(PATH_PLACEHOLDER, "", 1).contains("${")
        {
            return Err(invalid(
                format!("{field}.entry"),
                "entry must contain ${path} exactly once and no other template.",
            ));
        }
        if self
            .root_entry
            .as_deref()
            .is_some_and(|entry| !is_literal_entry(entry))
        {
            return Err(invalid(
                format!("{field}.rootEntry"),
                "rootEntry is non-empty literal text without a template.",
            ));
        }
        if self.default.len() > MAX_DEFAULT_ENTRIES
            || !self.default.iter().all(|entry| is_literal_entry(entry))
        {
            return Err(invalid(
                format!("{field}.default"),
                "default lists at most 32 non-empty literal entries without a template.",
            ));
        }
        Ok(())
    }

    /// The keys of `section`, outermost first.
    pub fn section_keys(&self) -> Vec<&str> {
        self.section.split('.').collect()
    }

    /// What the server uses when its configuration has no such array.
    pub fn default_entries(&self) -> &[String] {
        &self.default
    }

    /// The entries for `roots`, the workspace-relative excluded directories
    /// in order, where `.` is the workspace root.
    pub fn entries(&self, roots: &[String]) -> Vec<String> {
        roots
            .iter()
            .filter(|path| {
                self.path_syntax == PathSyntax::Literal || !path.contains(GLOB_METACHARACTERS)
            })
            .map(|path| match (&self.root_entry, path.as_str()) {
                (Some(root), ".") => root.clone(),
                _ => self.entry.replacen(PATH_PLACEHOLDER, path, 1),
            })
            .collect()
    }
}

/// Text without control characters, at most `MAX_TEXT_BYTES` long.
fn is_plain_text(text: &str) -> bool {
    text.len() <= MAX_TEXT_BYTES && !text.contains(char::is_control)
}

fn is_entry_text(text: &str) -> bool {
    !text.is_empty() && is_plain_text(text)
}

/// An entry used as written: `${` would read as a template that is never
/// expanded.
fn is_literal_entry(text: &str) -> bool {
    is_entry_text(text) && !text.contains("${")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn declaration(value: serde_json::Value) -> IndexingExclusions {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn literal_paths_fill_the_entry_template_in_order() {
        let exclusions =
            declaration(json!({"section": "server.files.exclude", "entry": "${path}"}));
        assert_eq!(exclusions.section_keys(), ["server", "files", "exclude"]);
        assert_eq!(
            exclusions.entries(&[".".into(), "vendor".into(), "带 空格".into(), "a*".into()]),
            [".", "vendor", "带 空格", "a*"]
        );
        assert!(exclusions.default_entries().is_empty());
    }

    #[test]
    fn glob_paths_leave_out_names_with_metacharacters_and_use_the_root_entry() {
        let exclusions = declaration(json!({
            "section": "server.directoryFilters",
            "entry": "-${path}",
            "rootEntry": "-",
            "default": ["-**/node_modules"],
            "pathSyntax": "glob"
        }));
        let roots = [".", "vendor", "literal*", "x?", "a[b]", "c{d}", "out/gen"].map(String::from);
        assert_eq!(exclusions.entries(&roots), ["-", "-vendor", "-out/gen"]);
        assert_eq!(exclusions.default_entries(), ["-**/node_modules"]);
    }

    #[test]
    fn declarations_are_bounded_and_templates_are_exact() {
        let valid = json!({"section": "a.b", "entry": "-${path}"});
        assert!(declaration(valid.clone()).validate("f").is_ok());
        let cases = [
            ("section", json!(""), "f.section"),
            ("section", json!("a..b"), "f.section"),
            ("section", json!("a b"), "f.section"),
            ("section", json!("a.b.c.d.e.f.g.h.i"), "f.section"),
            ("entry", json!("-path"), "f.entry"),
            ("entry", json!("${path}/${path}"), "f.entry"),
            ("entry", json!("${path}${setting:x}"), "f.entry"),
            ("rootEntry", json!(""), "f.rootEntry"),
            ("rootEntry", json!("${path}"), "f.rootEntry"),
            ("default", json!([""]), "f.default"),
            ("default", json!(["${path}"]), "f.default"),
            ("default", json!(vec!["x"; 33]), "f.default"),
        ];
        for (key, value, field) in cases {
            let mut candidate = valid.clone();
            candidate[key] = value;
            let rejection = declaration(candidate).validate("f").unwrap_err();
            assert_eq!(rejection.field.as_deref(), Some(field), "{key}");
        }
        assert!(serde_json::from_value::<IndexingExclusions>(
            json!({"section": "a", "entry": "${path}", "other": true})
        )
        .is_err());
        assert!(serde_json::from_value::<IndexingExclusions>(
            json!({"section": "a", "entry": "${path}", "pathSyntax": "regex"})
        )
        .is_err());
    }
}
