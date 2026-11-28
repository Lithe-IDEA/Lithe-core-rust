//! Bounded file-name and glob-pattern to language-ID mappings (`editor.fileTypeMappings.*`).
//!
//! Built-in mappings cover unambiguous community file names such as `uv.lock`, `Cargo.lock`,
//! and `bun.lock`. User-configured `global` and `project` rules are validated, normalized,
//! and resolved here so all desktop targets share identical matching and precedence semantics.

use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Maximum number of custom mapping entries allowed in one scope (`global` or `project`).
pub const MAX_MAPPINGS_PER_SCOPE: usize = 256;
/// Maximum UTF-8 byte length of a normalized pattern.
pub const MAX_PATTERN_BYTES: usize = 256;
/// Maximum UTF-8 byte length of a language identifier.
pub const MAX_LANGUAGE_ID_BYTES: usize = 64;
/// Maximum wildcard tokens (`*`, `**`, or `?`) permitted in a single pattern.
pub const MAX_WILDCARDS_PER_PATTERN: usize = 4;

/// Built-in unambiguous file name to language ID mappings.
pub const BUILTIN_FILE_TYPE_MAPPINGS: &[(&str, &str)] = &[
    // TOML lockfiles and manifests
    ("uv.lock", "toml"),
    ("Cargo.lock", "toml"),
    ("poetry.lock", "toml"),
    ("pdm.lock", "toml"),
    ("Pipfile", "toml"),
    ("Gopkg.lock", "toml"),
    // JSON and JSONC lockfiles and config files
    ("flake.lock", "json"),
    ("composer.lock", "json"),
    ("Pipfile.lock", "json"),
    (".prettierrc", "json"),
    (".watchmanconfig", "json"),
    ("bun.lock", "jsonc"),
    (".swcrc", "jsonc"),
    // YAML config files
    (".clang-format", "yaml"),
    (".clang-tidy", "yaml"),
    (".yamllint", "yaml"),
    // INI / properties config files
    (".npmrc", "ini"),
    (".yarnrc", "ini"),
    (".gitmodules", "ini"),
    (".pypirc", "ini"),
    (".flake8", "ini"),
    (".pylintrc", "ini"),
    // Shell startup and environment files
    (".bash_logout", "shell"),
    (".zshenv", "shell"),
    (".zlogin", "shell"),
    (".zlogout", "shell"),
    (".envrc", "shell"),
    // Ruby DSL manifests
    ("Brewfile", "ruby"),
    ("Podfile", "ruby"),
    ("Fastfile", "ruby"),
    ("Vagrantfile", "ruby"),
    // XML IDE descriptors
    (".classpath", "xml"),
    (".project", "xml"),
];

/// One file pattern to language identifier rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileTypeMapping {
    /// File name, relative path, or glob pattern (for example `uv.lock`, `*.lock`, `deploy/*.toml`).
    pub pattern: String,
    /// Target Lithe language identifier (for example `toml`, `json`, `yaml`).
    #[serde(rename = "languageID")]
    pub language_id: String,
}

/// One validation issue detected while normalizing a mapping scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTypeMappingIssue {
    /// Scope where the issue occurred (`global` or `project`).
    pub scope: String,
    /// Zero-based index of the entry in the input array.
    pub index: usize,
    /// Stable issue code (`invalid_pattern`, `invalid_language`, `unknown_language`, `too_many_entries`).
    pub code: String,
    /// Raw pattern value from the rejected entry.
    pub pattern: String,
    /// Raw language identifier from the rejected entry.
    #[serde(rename = "languageID")]
    pub language_id: String,
}

/// Output of `editor.fileTypeMappings.normalize`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizeResponse {
    /// Built-in unambiguous file type mappings provided by Core.
    pub builtins: Vec<FileTypeMapping>,
    /// Normalized and deduplicated global mappings.
    pub global: Vec<FileTypeMapping>,
    /// Normalized and deduplicated project mappings.
    pub project: Vec<FileTypeMapping>,
    /// Validation issues collected across `global` and `project`.
    pub issues: Vec<FileTypeMappingIssue>,
}

/// Output of `editor.fileTypeMappings.resolve`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveResponse {
    /// Matched Lithe language identifier, or `None` when no mapping matches.
    #[serde(rename = "languageID")]
    pub language_id: Option<String>,
    /// Scope that supplied the match (`project`, `global`, or `builtin`).
    pub source: Option<String>,
    /// Normalized pattern of the winning rule.
    pub pattern: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NormalizeRequest {
    #[serde(default)]
    global: Vec<FileTypeMapping>,
    #[serde(default)]
    project: Vec<FileTypeMapping>,
    #[serde(default, rename = "knownLanguageIDs")]
    known_language_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResolveRequest {
    path: String,
    #[serde(default)]
    global: Vec<FileTypeMapping>,
    #[serde(default)]
    project: Vec<FileTypeMapping>,
    #[serde(default = "default_true")]
    include_builtins: bool,
}

fn default_true() -> bool {
    true
}

/// Validates and normalizes `global` and `project` mappings alongside built-in mappings
/// (`editor.fileTypeMappings.normalize`).
pub fn normalize_command(payload: Value) -> Result<Value, CoreError> {
    let request: NormalizeRequest = serde_json::from_value(payload).map_err(|error| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid file type mappings normalize request",
        )
        .with_details(error.to_string())
    })?;

    let known: HashSet<String> = request
        .known_language_ids
        .iter()
        .map(|id| id.trim().to_ascii_lowercase())
        .filter(|id| !id.is_empty())
        .collect();

    let mut issues = Vec::new();
    let global = normalize_scope("global", &request.global, &known, &mut issues);
    let project = normalize_scope("project", &request.project, &known, &mut issues);
    let builtins = builtin_mappings();

    let response = NormalizeResponse {
        builtins,
        global,
        project,
        issues,
    };
    Ok(
        serde_json::to_value(response)
            .expect("file type mappings normalize response should encode"),
    )
}

/// Resolves the winning language ID for one file path across `project`, `global`, and
/// built-in mappings (`editor.fileTypeMappings.resolve`).
pub fn resolve_command(payload: Value) -> Result<Value, CoreError> {
    let request: ResolveRequest = serde_json::from_value(payload).map_err(|error| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid file type mappings resolve request",
        )
        .with_details(error.to_string())
    })?;

    let response = resolve_path(
        &request.path,
        &request.global,
        &request.project,
        request.include_builtins,
    );
    Ok(serde_json::to_value(response).expect("file type mappings resolve response should encode"))
}

/// Returns the built-in mapping list in deterministic declaration order.
pub fn builtin_mappings() -> Vec<FileTypeMapping> {
    BUILTIN_FILE_TYPE_MAPPINGS
        .iter()
        .map(|(pattern, language_id)| FileTypeMapping {
            pattern: (*pattern).to_owned(),
            language_id: (*language_id).to_owned(),
        })
        .collect()
}

fn normalize_scope(
    scope: &str,
    entries: &[FileTypeMapping],
    known: &HashSet<String>,
    issues: &mut Vec<FileTypeMappingIssue>,
) -> Vec<FileTypeMapping> {
    let mut valid = Vec::with_capacity(entries.len().min(MAX_MAPPINGS_PER_SCOPE));
    let mut last_index_by_key: HashMap<String, usize> = HashMap::new();

    for (index, entry) in entries.iter().enumerate() {
        if index >= MAX_MAPPINGS_PER_SCOPE {
            issues.push(FileTypeMappingIssue {
                scope: scope.to_owned(),
                index,
                code: "too_many_entries".to_owned(),
                pattern: entry.pattern.clone(),
                language_id: entry.language_id.clone(),
            });
            continue;
        }
        let Some(pattern) = normalize_pattern(&entry.pattern) else {
            issues.push(FileTypeMappingIssue {
                scope: scope.to_owned(),
                index,
                code: "invalid_pattern".to_owned(),
                pattern: entry.pattern.clone(),
                language_id: entry.language_id.clone(),
            });
            continue;
        };
        let Some(language_id) = normalize_language_id(&entry.language_id) else {
            issues.push(FileTypeMappingIssue {
                scope: scope.to_owned(),
                index,
                code: "invalid_language".to_owned(),
                pattern: entry.pattern.clone(),
                language_id: entry.language_id.clone(),
            });
            continue;
        };
        if !known.is_empty() && !known.contains(&language_id) {
            issues.push(FileTypeMappingIssue {
                scope: scope.to_owned(),
                index,
                code: "unknown_language".to_owned(),
                pattern: entry.pattern.clone(),
                language_id: entry.language_id.clone(),
            });
            continue;
        }

        let key = pattern.to_ascii_lowercase();
        let slot = valid.len();
        valid.push(Some(FileTypeMapping {
            pattern,
            language_id,
        }));
        // Last valid duplicate wins; mark earlier duplicate slot as superseded.
        if let Some(previous) = last_index_by_key.insert(key, slot) {
            valid[previous] = None;
        }
    }

    valid.into_iter().flatten().collect()
}

fn normalize_pattern(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_PATTERN_BYTES {
        return None;
    }
    if trimmed.contains(['\0', '\r', '\n']) {
        return None;
    }
    let normalized = trimmed.replace('\\', "/");
    if normalized.starts_with('/') || normalized.ends_with('/') {
        return None;
    }
    let mut wildcard_count = 0usize;
    for segment in normalized.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return None;
        }
        if segment == "**" {
            wildcard_count += 1;
        } else {
            if segment.contains("**") {
                return None;
            }
            for byte in segment.bytes() {
                if byte == b'*' || byte == b'?' {
                    wildcard_count += 1;
                }
            }
        }
        if wildcard_count > MAX_WILDCARDS_PER_PATTERN {
            return None;
        }
    }
    Some(normalized)
}

fn normalize_language_id(raw: &str) -> Option<String> {
    let normalized = raw.trim().to_ascii_lowercase();
    if normalized.is_empty() || normalized.len() > MAX_LANGUAGE_ID_BYTES {
        return None;
    }
    let mut bytes = normalized.bytes();
    let first = bytes.next()?;
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return None;
    }
    if !bytes.all(|b| {
        b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'+' | b'-')
    }) {
        return None;
    }
    Some(normalized)
}

fn resolve_path(
    raw_path: &str,
    global: &[FileTypeMapping],
    project: &[FileTypeMapping],
    include_builtins: bool,
) -> ResolveResponse {
    let Some(target) = normalize_target_path(raw_path) else {
        return ResolveResponse {
            language_id: None,
            source: None,
            pattern: None,
        };
    };

    if let Some((pattern, language_id)) = best_match_in_scope(&target, project) {
        return ResolveResponse {
            language_id: Some(language_id),
            source: Some("project".to_owned()),
            pattern: Some(pattern),
        };
    }
    if let Some((pattern, language_id)) = best_match_in_scope(&target, global) {
        return ResolveResponse {
            language_id: Some(language_id),
            source: Some("global".to_owned()),
            pattern: Some(pattern),
        };
    }
    if include_builtins {
        if let Some((pattern, language_id)) = best_match_in_builtins(&target) {
            return ResolveResponse {
                language_id: Some(language_id),
                source: Some("builtin".to_owned()),
                pattern: Some(pattern),
            };
        }
    }
    ResolveResponse {
        language_id: None,
        source: None,
        pattern: None,
    }
}

struct NormalizedTarget {
    segments: Vec<String>,
}

impl NormalizedTarget {
    fn basename(&self) -> &str {
        self.segments.last().map(String::as_str).unwrap_or("")
    }
}

fn normalize_target_path(raw: &str) -> Option<NormalizedTarget> {
    let mut trimmed = raw.trim().replace('\\', "/");
    if trimmed.is_empty() || trimmed.contains('\0') {
        return None;
    }
    // Strip optional Windows drive prefix (e.g. `C:/repo/uv.lock`) when a full path is supplied.
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/' {
        trimmed = trimmed[2..].to_owned();
    }
    while let Some(rest) = trimmed.strip_prefix("./") {
        trimmed = rest.to_owned();
    }
    let trimmed = trimmed.trim_start_matches('/');
    if trimmed.is_empty() || trimmed.ends_with('/') {
        return None;
    }
    let segments: Vec<String> = trimmed
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .map(|s| s.to_ascii_lowercase())
        .collect();
    if segments.is_empty() {
        return None;
    }
    Some(NormalizedTarget { segments })
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Specificity {
    tier: u8,
    literal_bytes: usize,
    pattern_bytes: usize,
}

fn rule_specificity(normalized_pattern: &str) -> Specificity {
    let has_slash = normalized_pattern.contains('/');
    let has_glob = normalized_pattern.contains(['*', '?']);
    let tier = match (has_slash, has_glob) {
        (true, false) => 4,
        (false, false) => 3,
        (true, true) => 2,
        (false, true) => 1,
    };
    let literal_bytes = normalized_pattern
        .bytes()
        .filter(|b| !matches!(b, b'*' | b'?' | b'/'))
        .count();
    Specificity {
        tier,
        literal_bytes,
        pattern_bytes: normalized_pattern.len(),
    }
}

fn best_match_in_scope(
    target: &NormalizedTarget,
    entries: &[FileTypeMapping],
) -> Option<(String, String)> {
    let mut best: Option<(Specificity, String, String)> = None;
    for entry in entries.iter().take(MAX_MAPPINGS_PER_SCOPE) {
        let Some(pattern) = normalize_pattern(&entry.pattern) else {
            continue;
        };
        let Some(language_id) = normalize_language_id(&entry.language_id) else {
            continue;
        };
        if !matches_pattern(target, &pattern) {
            continue;
        }
        let spec = rule_specificity(&pattern);
        if best
            .as_ref()
            .is_none_or(|(best_spec, _, _)| spec > *best_spec)
        {
            best = Some((spec, pattern, language_id));
        }
    }
    best.map(|(_, pattern, language_id)| (pattern, language_id))
}

fn best_match_in_builtins(target: &NormalizedTarget) -> Option<(String, String)> {
    let basename = target.basename();
    for &(pattern, language_id) in BUILTIN_FILE_TYPE_MAPPINGS {
        if pattern.eq_ignore_ascii_case(basename) {
            return Some((pattern.to_owned(), language_id.to_owned()));
        }
    }
    None
}

fn matches_pattern(target: &NormalizedTarget, normalized_pattern: &str) -> bool {
    let lower_pattern = normalized_pattern.to_ascii_lowercase();
    let pat_segments: Vec<&str> = lower_pattern.split('/').collect();
    if pat_segments.len() == 1 && pat_segments[0] != "**" {
        return match_glob_segment(pat_segments[0].as_bytes(), target.basename().as_bytes());
    }
    match_segments(&pat_segments, &target.segments)
}

fn match_segments(pat: &[&str], path: &[String]) -> bool {
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star_pi: Option<usize> = None;
    let mut star_ti = 0usize;

    while ti < path.len() {
        if pi < pat.len() && pat[pi] == "**" {
            star_pi = Some(pi);
            star_ti = ti;
            pi += 1;
        } else if pi < pat.len() && match_glob_segment(pat[pi].as_bytes(), path[ti].as_bytes()) {
            pi += 1;
            ti += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    while pi < pat.len() && pat[pi] == "**" {
        pi += 1;
    }
    pi == pat.len()
}

fn match_glob_segment(pat: &[u8], text: &[u8]) -> bool {
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star_pi: Option<usize> = None;
    let mut star_ti = 0usize;

    while ti < text.len() {
        if pi < pat.len() && (pat[pi] == b'?' || pat[pi] == text[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pat.len() && pat[pi] == b'*' {
            star_pi = Some(pi);
            star_ti = ti;
            pi += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    while pi < pat.len() && pat[pi] == b'*' {
        pi += 1;
    }
    pi == pat.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolves_builtin_lockfiles_and_config_files_case_insensitively() {
        for (path, want_lang, want_pat) in [
            ("uv.lock", "toml", "uv.lock"),
            ("UV.LOCK", "toml", "uv.lock"),
            ("services/api/uv.lock", "toml", "uv.lock"),
            (r"C:\repo\uv.lock", "toml", "uv.lock"),
            ("Cargo.lock", "toml", "Cargo.lock"),
            ("poetry.lock", "toml", "poetry.lock"),
            ("pdm.lock", "toml", "pdm.lock"),
            ("Pipfile", "toml", "Pipfile"),
            ("bun.lock", "jsonc", "bun.lock"),
            ("flake.lock", "json", "flake.lock"),
            (".prettierrc", "json", ".prettierrc"),
            (".clang-format", "yaml", ".clang-format"),
            (".npmrc", "ini", ".npmrc"),
            ("Brewfile", "ruby", "Brewfile"),
            (".classpath", "xml", ".classpath"),
        ] {
            let res = resolve_command(json!({"path": path})).unwrap();
            assert_eq!(res["languageID"], want_lang, "path={path}");
            assert_eq!(res["source"], "builtin", "path={path}");
            assert_eq!(res["pattern"], want_pat, "path={path}");
        }
    }

    #[test]
    fn scope_precedence_project_beats_global_beats_builtin() {
        let global = json!([{"pattern": "uv.lock", "languageID": "ini"}]);
        let project = json!([{"pattern": "uv.lock", "languageID": "yaml"}]);

        let with_global = resolve_command(json!({
            "path": "uv.lock",
            "global": global,
        }))
        .unwrap();
        assert_eq!(with_global["languageID"], "ini");
        assert_eq!(with_global["source"], "global");

        let with_both = resolve_command(json!({
            "path": "uv.lock",
            "global": global,
            "project": project,
        }))
        .unwrap();
        assert_eq!(with_both["languageID"], "yaml");
        assert_eq!(with_both["source"], "project");
    }

    #[test]
    fn specificity_prefers_exact_path_then_exact_name_then_longest_glob() {
        let global = json!([
            {"pattern": "*.lock", "languageID": "plaintext"},
            {"pattern": "*.config.lock", "languageID": "json"},
            {"pattern": "deploy/*.lock", "languageID": "yaml"},
            {"pattern": "deploy/app.lock", "languageID": "toml"},
            {"pattern": "special.lock", "languageID": "ini"},
        ]);

        let exact_rel =
            resolve_command(json!({"path": "deploy/app.lock", "global": global})).unwrap();
        assert_eq!(exact_rel["languageID"], "toml");
        assert_eq!(exact_rel["pattern"], "deploy/app.lock");

        let glob_rel =
            resolve_command(json!({"path": "deploy/other.lock", "global": global})).unwrap();
        assert_eq!(glob_rel["languageID"], "yaml");
        assert_eq!(glob_rel["pattern"], "deploy/*.lock");

        let exact_name =
            resolve_command(json!({"path": "sub/special.lock", "global": global})).unwrap();
        assert_eq!(exact_name["languageID"], "ini");
        assert_eq!(exact_name["pattern"], "special.lock");

        let longer_glob =
            resolve_command(json!({"path": "app.config.lock", "global": global})).unwrap();
        assert_eq!(longer_glob["languageID"], "json");
        assert_eq!(longer_glob["pattern"], "*.config.lock");
    }

    #[test]
    fn supports_globstar_and_single_char_wildcards() {
        let global = json!([
            {"pattern": "config/**/settings?.cfg", "languageID": "ini"},
            {"pattern": "**/*.conf", "languageID": "toml"},
        ]);

        let direct =
            resolve_command(json!({"path": "config/settings1.cfg", "global": global})).unwrap();
        assert_eq!(direct["languageID"], "ini");

        let nested =
            resolve_command(json!({"path": "config/a/b/settingsA.cfg", "global": global})).unwrap();
        assert_eq!(nested["languageID"], "ini");

        let wrong_len =
            resolve_command(json!({"path": "config/settings12.cfg", "global": global})).unwrap();
        assert_eq!(wrong_len["languageID"], Value::Null);

        let root_conf = resolve_command(json!({"path": "app.conf", "global": global})).unwrap();
        assert_eq!(root_conf["languageID"], "toml");
    }

    #[test]
    fn normalize_deduplicates_and_reports_issues() {
        let res = normalize_command(json!({
            "knownLanguageIDs": ["toml", "json", "yaml"],
            "global": [
                {"pattern": " uv.lock ", "languageID": "JSON"},
                {"pattern": r"deploy\*.toml", "languageID": "toml"},
                {"pattern": "UV.LOCK", "languageID": "TOML"},
                {"pattern": "/abs/path", "languageID": "toml"},
                {"pattern": "../escape", "languageID": "toml"},
                {"pattern": "*/*/*/*/*", "languageID": "toml"},
                {"pattern": "foo.bar", "languageID": "bad id!"},
                {"pattern": "baz.qux", "languageID": "rust"}
            ]
        }))
        .unwrap();

        assert_eq!(
            res["global"],
            json!([
                {"pattern": "deploy/*.toml", "languageID": "toml"},
                {"pattern": "UV.LOCK", "languageID": "toml"}
            ])
        );
        assert_eq!(
            res["issues"],
            json!([
                {"scope": "global", "index": 3, "code": "invalid_pattern", "pattern": "/abs/path", "languageID": "toml"},
                {"scope": "global", "index": 4, "code": "invalid_pattern", "pattern": "../escape", "languageID": "toml"},
                {"scope": "global", "index": 5, "code": "invalid_pattern", "pattern": "*/*/*/*/*", "languageID": "toml"},
                {"scope": "global", "index": 6, "code": "invalid_language", "pattern": "foo.bar", "languageID": "bad id!"},
                {"scope": "global", "index": 7, "code": "unknown_language", "pattern": "baz.qux", "languageID": "rust"}
            ])
        );
    }

    #[test]
    fn normalize_reports_entries_past_scope_limit() {
        let mut entries = Vec::with_capacity(MAX_MAPPINGS_PER_SCOPE + 1);
        for i in 0..=MAX_MAPPINGS_PER_SCOPE {
            entries.push(json!({"pattern": format!("file-{i}.lock"), "languageID": "toml"}));
        }
        let res = normalize_command(json!({"project": entries})).unwrap();
        assert_eq!(
            res["project"].as_array().unwrap().len(),
            MAX_MAPPINGS_PER_SCOPE
        );
        let issues = res["issues"].as_array().unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0]["scope"], "project");
        assert_eq!(issues[0]["index"], MAX_MAPPINGS_PER_SCOPE);
        assert_eq!(issues[0]["code"], "too_many_entries");
    }
}
