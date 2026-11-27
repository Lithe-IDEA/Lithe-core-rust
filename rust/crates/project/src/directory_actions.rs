//! Directory roles, custom folder types, color overrides, exclusion and rule migration
//! for the Project view.
//!
//! A workspace stores persisted directory metadata keyed by workspace-relative path:
//! the role or custom type of a directory (`marks`), the directories that opt back in to
//! indexing although Git ignores them (`includedDirectories`), the role a directory had
//! before it was excluded (`previousRoles`), optional user-defined folder types
//! (`customTypes`), and optional per-path color overrides (`colors`). Hosts persist this
//! state verbatim and delegate all transitions and presentation resolution to Core:
//!
//! - `workspace.reduceDirectoryAction` applies role assignment, exclusion, cancel
//!   exclusion, clear mark, color override, and custom type mutations to a copy of that
//!   state.
//! - `workspace.relocateDirectoryRules` moves the keys of a subtree after a rename or
//!   move, drops them after a delete, or hands them to the state of another workspace
//!   root.
//! - `workspace.resolveDirectoryPresentation` merges user rules, optional shared project
//!   rules, glob-matched custom types and Git-ignored paths into deterministic per-path
//!   tree presentation metadata and manual indexing exclusions.
//!
//! Directory keys are compared as paths: with `caseInsensitive` set, keys that differ only
//! by case name the same directory. Stored keys keep the spelling they were written with.
//! The sequence contract is `rust/fixtures/workspace/directory-actions-v1.json`; the
//! request and response shapes are pinned by `directory-rules-v1.json`.

use lithe_platform::paths::invalid_relative_path;
use lithe_platform::Platform;
use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Largest number of path-keyed entries one request may carry.
const MAX_ENTRIES: usize = 100_000;
/// Largest number of user-defined directory types one rule set may carry.
const MAX_CUSTOM_TYPES: usize = 64;
/// Largest number of user-imported custom directory icons one rule set may carry.
const MAX_CUSTOM_ICONS: usize = 128;
/// Maximum byte length of a custom type or icon identifier.
const MAX_TYPE_ID_LEN: usize = 64;
/// Maximum byte length of a custom type display name.
const MAX_TYPE_NAME_LEN: usize = 64;
/// Maximum byte length of a custom type badge tag.
const MAX_TYPE_TAG_LEN: usize = 24;
/// Maximum byte length of a custom type match glob pattern.
const MAX_GLOB_LEN: usize = 256;

/// Built-in directory roles recognized by Core.
const BUILTIN_ROLES: [&str; 9] = [
    "plain",
    "sources",
    "tests",
    "resources",
    "testResources",
    "generated",
    "excluded",
    "module",
    "package",
];

/// Icon names allowed for custom directory types.
const ALLOWED_ICONS: [&str; 11] = [
    "folder",
    "sources",
    "tests",
    "resources",
    "generated",
    "excluded",
    "star",
    "book",
    "archive",
    "database",
    "gear",
];

/// User-defined directory type combining visual styling with indexing and inheritance traits.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CustomDirectoryType {
    /// Stable identifier starting with `custom.`.
    id: String,
    /// Human-readable name displayed in menus and settings.
    name: String,
    /// Optional short label shown beside directory names in the project tree.
    #[serde(default)]
    tag: String,
    /// Built-in icon identifier from [`ALLOWED_ICONS`].
    icon: String,
    /// Lowercase `#rrggbb` accent color.
    color: String,
    /// Whether rows of this type receive a subtle background tint.
    #[serde(default)]
    tint_bg: bool,
    /// Whether the directory name text is tinted with `color`.
    #[serde(default)]
    tint_text: bool,
    /// Whether descendant rows inherit the background tint.
    #[serde(default)]
    inherit_bg: bool,
    /// Whether directories of this type are excluded from workspace indexing and search.
    #[serde(default)]
    exclude_indexing: bool,
    /// Whether directories of this type carry a generated/read-only hint.
    #[serde(default)]
    read_only_hint: bool,
    /// Optional comma-separated workspace-relative glob patterns for automatic matching.
    #[serde(default)]
    match_glob: String,
}

/// User-imported custom SVG icon entry referenced by custom directory types.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CustomDirectoryIcon {
    /// Stable identifier starting with `icon.`.
    id: String,
    /// Display name shown in the searchable icon picker.
    name: String,
    /// Relative or managed file name of the imported SVG resource.
    path: String,
}

#[derive(Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// The persisted rule state of one workspace root.
struct Rules {
    /// Directory role or custom type ID by workspace-relative path; a missing key is plain.
    #[serde(default)]
    marks: BTreeMap<String, String>,
    /// Directories that stay indexed although Git ignores them.
    #[serde(default)]
    included_directories: Vec<String>,
    /// The role to restore when a manual exclusion is canceled.
    #[serde(default)]
    previous_roles: BTreeMap<String, String>,
    /// User-defined directory types; omitted when empty for v1 wire compatibility.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    custom_types: Vec<CustomDirectoryType>,
    /// User-imported custom SVG icons; omitted when empty for v1 wire compatibility.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    custom_icons: Vec<CustomDirectoryIcon>,
    /// Per-path `#rrggbb` quick color overrides; omitted when empty for v1 wire compatibility.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    colors: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum Action {
    /// Assign a built-in role (other than `excluded`) or a defined `custom.*` type ID.
    SetRole { role: String },
    /// Exclude the directory from indexing while remembering its previous role.
    Exclude,
    /// Undo manual or Git exclusion for the directory.
    CancelExclusion,
    /// Remove the directory's explicit mark, remembered role and color override.
    ClearMark,
    /// Set or clear a per-path `#rrggbb` quick color override.
    SetColor { color: Option<String> },
    /// Insert or update a custom directory type in `rules.customTypes`.
    UpsertCustomType {
        #[serde(rename = "directoryType")]
        directory_type: CustomDirectoryType,
    },
    /// Remove a custom directory type and drop any marks referencing it.
    DeleteCustomType { id: String },
    /// Insert or update a user-imported custom icon in `rules.customIcons`.
    UpsertCustomIcon {
        #[serde(rename = "directoryIcon")]
        directory_icon: CustomDirectoryIcon,
    },
    /// Remove a user-imported custom icon and fall back referencing custom types to `folder`.
    DeleteCustomIcon { id: String },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReduceRequest {
    /// Workspace-relative directory; `.` is the workspace root.
    path: String,
    action: Action,
    #[serde(default)]
    case_insensitive: bool,
    rules: Rules,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RelocateRequest {
    /// Directory that was moved or deleted, relative to the root that owns `rules`.
    source: String,
    /// Where it went, relative to the root that owns `target` (or `rules` without a
    /// target); absent when the directory was deleted.
    #[serde(default)]
    destination: Option<String>,
    #[serde(default)]
    case_insensitive: bool,
    rules: Rules,
    /// Rules of the destination root when the move crosses workspace roots.
    #[serde(default)]
    target: Option<Rules>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResolvePresentationRequest {
    /// Local user rules (highest priority).
    rules: Rules,
    /// Optional project-shared rules (e.g. `.lithe/directories.json`, lower than user rules).
    #[serde(default)]
    shared_rules: Option<Rules>,
    /// Workspace-relative directories currently excluded by `.gitignore`.
    #[serde(default)]
    git_excluded: Vec<String>,
    #[serde(default)]
    case_insensitive: bool,
    /// Workspace-relative paths (directories or files) to resolve presentation for.
    #[serde(default)]
    paths: Vec<String>,
}

/// Resolved presentation and indexing metadata for one workspace-relative path.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ResolvedDirectoryEntry {
    /// Effective role or custom type ID (`plain` when unmarked).
    role: String,
    /// Winning layer: `user`, `shared`, `glob`, `vcs`, `inherited`, or `plain`.
    source: String,
    /// Resolved icon identifier for directories.
    icon: String,
    /// Optional `#rrggbb` accent color for the directory icon or text.
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    /// Optional `#rrggbb` row background tint color (direct or inherited from an ancestor).
    #[serde(skip_serializing_if = "Option::is_none")]
    bg_color: Option<String>,
    /// Whether the row name text should use `color`.
    tint_text: bool,
    /// Optional short label displayed beside the directory name.
    #[serde(skip_serializing_if = "Option::is_none")]
    tag: Option<String>,
    /// Whether this path is excluded from indexing (directly or via an ancestor).
    excluded: bool,
    /// Whether this path carries a generated/read-only hint.
    read_only_hint: bool,
    /// Role that would apply if the user's local override were cleared.
    #[serde(skip_serializing_if = "Option::is_none")]
    auto_fallback_role: Option<String>,
}

/// Applies one directory action (`workspace.reduceDirectoryAction`).
///
/// Returns `{ changed, rules }`, where `rules` is the complete state to persist. `changed`
/// is false when the action has no effect, including canceling an exclusion that a manually
/// excluded ancestor still enforces; the state is then the input. Excluding remembers the
/// replaced role and canceling restores it; changing a role keeps include exceptions;
/// excluding deletes only the directory's own exception.
pub fn reduce_command(payload: Value) -> Result<Value, CoreError> {
    let request: ReduceRequest = parse(payload)?;
    validate_path(&request.path)?;
    validate_rules(&request.rules)?;
    let ident = Identity(request.case_insensitive);
    let before = request.rules.clone();
    let mut rules = request.rules;
    let path = request.path;
    let same = |key: &String| ident.key(key) == ident.key(&path);
    match request.action {
        Action::CancelExclusion => {
            let path_ident = ident.key(&path);
            let blocked = rules.marks.iter().any(|(key, role)| {
                role == "excluded"
                    && ident.key(key) != path_ident
                    && (key == "." || path_ident.starts_with(&format!("{}/", ident.key(key))))
            });
            if blocked {
                return Ok(response(false, before));
            }
            let current = take_same(&mut rules.marks, &same);
            let previous = take_same(&mut rules.previous_roles, &same);
            let restored = if current.as_deref() == Some("excluded") {
                previous
            } else {
                current
            };
            if let Some(role) = restored {
                rules.marks.insert(path.clone(), role);
            }
            if !rules.included_directories.iter().any(same) {
                rules.included_directories.push(path);
            }
        }
        Action::Exclude => {
            let current = take_same(&mut rules.marks, &same);
            if let Some(role) = current.filter(|role| role != "excluded") {
                rules.previous_roles.retain(|key, _| !same(key));
                rules.previous_roles.insert(path.clone(), role);
            }
            rules.marks.insert(path.clone(), "excluded".into());
            rules.included_directories.retain(|key| !same(key));
        }
        Action::SetRole { role } => {
            if role == "excluded" || !is_known_role(&role, &rules.custom_types) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Directory role is not assignable",
                ));
            }
            take_same(&mut rules.marks, &same);
            rules.marks.insert(path, role);
        }
        Action::ClearMark => {
            take_same(&mut rules.marks, &same);
            take_same(&mut rules.previous_roles, &same);
            take_same(&mut rules.colors, &same);
        }
        Action::SetColor { color } => {
            take_same(&mut rules.colors, &same);
            if let Some(raw) = color {
                let normalized = normalize_hex_color(&raw)?;
                rules.colors.insert(path, normalized);
            }
        }
        Action::UpsertCustomType { directory_type } => {
            let normalized = normalize_custom_type(directory_type, &rules.custom_icons)?;
            if let Some(existing) = rules
                .custom_types
                .iter_mut()
                .find(|item| item.id == normalized.id)
            {
                *existing = normalized;
            } else {
                if rules.custom_types.len() >= MAX_CUSTOM_TYPES {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Too many custom directory types",
                    ));
                }
                rules.custom_types.push(normalized);
            }
            rules.custom_types.sort_by(|a, b| a.id.cmp(&b.id));
        }
        Action::DeleteCustomType { id } => {
            validate_custom_type_id(&id)?;
            rules.custom_types.retain(|item| item.id != id);
            rules.marks.retain(|_, role| role != &id);
            rules.previous_roles.retain(|_, role| role != &id);
        }
        Action::UpsertCustomIcon { directory_icon } => {
            let normalized = normalize_custom_icon(directory_icon)?;
            if let Some(existing) = rules
                .custom_icons
                .iter_mut()
                .find(|item| item.id == normalized.id)
            {
                *existing = normalized;
            } else {
                if rules.custom_icons.len() >= MAX_CUSTOM_ICONS {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Too many custom directory icons",
                    ));
                }
                rules.custom_icons.push(normalized);
            }
            rules.custom_icons.sort_by(|a, b| a.id.cmp(&b.id));
        }
        Action::DeleteCustomIcon { id } => {
            validate_custom_icon_id(&id)?;
            rules.custom_icons.retain(|item| item.id != id);
            for ct in &mut rules.custom_types {
                if ct.icon == id {
                    ct.icon = "folder".into();
                }
            }
        }
    }
    let changed = rules != before;
    Ok(response(changed, rules))
}

/// Migrates directory rules after a move or delete (`workspace.relocateDirectoryRules`).
///
/// Returns `{ rules, target }`. Every key at or below `source` is removed from `rules`.
/// With a `destination` the key is rewritten under it, in `target` for a move across
/// workspace roots and in `rules` otherwise; without one the keys are dropped. `target` is
/// absent from the response unless the request had one.
pub fn relocate_command(payload: Value) -> Result<Value, CoreError> {
    let request: RelocateRequest = parse(payload)?;
    validate_path(&request.source)?;
    if let Some(destination) = &request.destination {
        validate_path(destination)?;
    }
    validate_rules(&request.rules)?;
    if let Some(target) = &request.target {
        validate_rules(target)?;
        if request.destination.is_none() {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "A target root needs a destination",
            ));
        }
    }
    let ident = Identity(request.case_insensitive);
    let source_key = ident.key(&request.source);
    let source_len = request.source.chars().count();
    let under = |key: &str| {
        let key = ident.key(key);
        request.source == "." || key == source_key || key.starts_with(&format!("{source_key}/"))
    };
    let moved_key = |key: &str| -> Option<String> {
        let destination = request.destination.as_deref()?;
        let tail: String = if request.source == "." {
            if key == "." {
                String::new()
            } else {
                format!("/{key}")
            }
        } else {
            key.chars().skip(source_len).collect()
        };
        Some(if destination == "." {
            let trimmed = tail.trim_start_matches('/');
            if trimmed.is_empty() {
                ".".into()
            } else {
                trimmed.into()
            }
        } else {
            format!("{destination}{tail}")
        })
    };
    let mut remaining = request.rules.clone();
    let mut destination_rules = request.target.clone();
    let moved = |remaining: &mut Rules, into: &mut Rules| {
        let marks = std::mem::take(&mut remaining.marks);
        for (key, role) in marks {
            if under(&key) {
                if let Some(moved) = moved_key(&key) {
                    into.marks.insert(moved, role);
                }
            } else {
                remaining.marks.insert(key, role);
            }
        }
        let previous = std::mem::take(&mut remaining.previous_roles);
        for (key, role) in previous {
            if under(&key) {
                if let Some(moved) = moved_key(&key) {
                    into.previous_roles.insert(moved, role);
                }
            } else {
                remaining.previous_roles.insert(key, role);
            }
        }
        let colors = std::mem::take(&mut remaining.colors);
        for (key, color) in colors {
            if under(&key) {
                if let Some(moved) = moved_key(&key) {
                    into.colors.insert(moved, color);
                }
            } else {
                remaining.colors.insert(key, color);
            }
        }
        let included = std::mem::take(&mut remaining.included_directories);
        for key in included {
            if under(&key) {
                if let Some(moved) = moved_key(&key) {
                    push_unique(&mut into.included_directories, moved, &ident);
                }
            } else {
                remaining.included_directories.push(key);
            }
        }
    };
    match destination_rules.as_mut() {
        Some(target) => moved(&mut remaining, target),
        None => {
            // Same root: moved entries are collected first so a key that lands on
            // another moved key cannot be processed twice.
            let mut collected = Rules::default();
            moved(&mut remaining, &mut collected);
            remaining.marks.extend(collected.marks);
            remaining.previous_roles.extend(collected.previous_roles);
            remaining.colors.extend(collected.colors);
            for key in collected.included_directories {
                push_unique(&mut remaining.included_directories, key, &ident);
            }
        }
    }
    Ok(json!({ "rules": remaining, "target": destination_rules }))
}

/// Resolves deterministic presentation metadata and effective manual indexing exclusions
/// across local user rules, optional shared project rules, glob-matched custom types and
/// `.gitignore` exclusions (`workspace.resolveDirectoryPresentation`).
pub fn resolve_presentation_command(payload: Value) -> Result<Value, CoreError> {
    let request: ResolvePresentationRequest = parse(payload)?;
    validate_rules(&request.rules)?;
    if let Some(shared) = &request.shared_rules {
        validate_rules(shared)?;
    }
    if request.paths.len() + request.git_excluded.len() > MAX_ENTRIES {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Too many paths in directory presentation request",
        ));
    }
    for path in request.paths.iter().chain(&request.git_excluded) {
        validate_path(path)?;
    }

    let ident = Identity(request.case_insensitive);

    // Merge custom types deterministically: local user definitions override shared ones.
    let mut custom_types: BTreeMap<String, CustomDirectoryType> = BTreeMap::new();
    if let Some(shared) = &request.shared_rules {
        for item in &shared.custom_types {
            custom_types.insert(item.id.clone(), item.clone());
        }
    }
    for item in &request.rules.custom_types {
        custom_types.insert(item.id.clone(), item.clone());
    }

    let user_marks = index_map(&request.rules.marks, &ident);
    let user_colors = index_map(&request.rules.colors, &ident);
    let user_included: BTreeSet<String> = request
        .rules
        .included_directories
        .iter()
        .map(|p| ident.key(p))
        .collect();

    let shared_marks = request
        .shared_rules
        .as_ref()
        .map(|r| index_map(&r.marks, &ident))
        .unwrap_or_default();
    let shared_colors = request
        .shared_rules
        .as_ref()
        .map(|r| index_map(&r.colors, &ident))
        .unwrap_or_default();
    let git_excluded: BTreeSet<String> =
        request.git_excluded.iter().map(|p| ident.key(p)).collect();

    // Compute manual exclusions from user + shared marks (including custom types with
    // `excludeIndexing: true`) so indexing stays aligned with presentation.
    let mut manual_excluded = BTreeSet::new();
    for (raw_path, role) in request
        .shared_rules
        .iter()
        .flat_map(|r| r.marks.iter())
        .chain(request.rules.marks.iter())
    {
        let key = ident.key(raw_path);
        let effective_role = user_marks.get(&key).or_else(|| shared_marks.get(&key));
        if let Some(effective) = effective_role {
            if role_excludes_indexing(effective, &custom_types) && !user_included.contains(&key) {
                manual_excluded.insert(raw_path.clone());
            }
        }
        let _ = role;
    }

    let mut entries = BTreeMap::new();
    for raw_path in &request.paths {
        let resolved = resolve_one(
            raw_path,
            &ident,
            &custom_types,
            &user_marks,
            &user_colors,
            &user_included,
            &shared_marks,
            &shared_colors,
            &git_excluded,
        );
        if resolved.excluded
            && matches!(resolved.source.as_str(), "user" | "shared" | "glob")
            && !user_included.contains(&ident.key(raw_path))
        {
            manual_excluded.insert(raw_path.clone());
        }
        entries.insert(raw_path.clone(), resolved);
    }

    Ok(json!({
        "entries": entries,
        "manualExcludedDirectories": manual_excluded.into_iter().collect::<Vec<_>>(),
    }))
}

#[allow(clippy::too_many_arguments)]
fn resolve_one(
    raw_path: &str,
    ident: &Identity,
    custom_types: &BTreeMap<String, CustomDirectoryType>,
    user_marks: &BTreeMap<String, String>,
    user_colors: &BTreeMap<String, String>,
    user_included: &BTreeSet<String>,
    shared_marks: &BTreeMap<String, String>,
    shared_colors: &BTreeMap<String, String>,
    git_excluded: &BTreeSet<String>,
) -> ResolvedDirectoryEntry {
    let key = ident.key(raw_path);
    let user_role = user_marks.get(&key);
    let user_color = user_colors.get(&key);
    let shared_role = shared_marks.get(&key);
    let shared_color = shared_colors.get(&key);
    let glob_type = find_glob_match(raw_path, custom_types, ident.0);
    let is_vcs_excluded = git_excluded.contains(&key) && !user_included.contains(&key);

    // Determine fallback role (what would apply if the user clears their local override).
    let auto_fallback_role = if let Some(role) = shared_role {
        Some(role.clone())
    } else if let Some(ct) = glob_type {
        Some(ct.id.clone())
    } else if is_vcs_excluded {
        Some("excluded".to_owned())
    } else {
        None
    };

    // Determine direct role and winning source.
    let (role, mut source) = if let Some(role) = user_role {
        (role.clone(), "user".to_owned())
    } else if user_color.is_some() {
        ("plain".to_owned(), "user".to_owned())
    } else if let Some(role) = shared_role {
        (role.clone(), "shared".to_owned())
    } else if shared_color.is_some() {
        ("plain".to_owned(), "shared".to_owned())
    } else if let Some(ct) = glob_type {
        (ct.id.clone(), "glob".to_owned())
    } else if is_vcs_excluded {
        ("excluded".to_owned(), "vcs".to_owned())
    } else {
        ("plain".to_owned(), "plain".to_owned())
    };

    let (icon, base_color, base_tint_bg, tint_text, tag, mut excluded, read_only_hint) =
        style_for_role(&role, custom_types);

    let effective_color = user_color
        .cloned()
        .or_else(|| shared_color.cloned())
        .or(base_color);

    let direct_bg = if user_color.is_some() || shared_color.is_some() || base_tint_bg {
        effective_color.clone()
    } else {
        None
    };

    // Walk ancestors (nearest first) for inherited background tint and exclusion state.
    let mut bg_color = direct_bg;
    for ancestor in ancestors(raw_path) {
        let anc_key = ident.key(&ancestor);
        let anc_role = user_marks
            .get(&anc_key)
            .or_else(|| shared_marks.get(&anc_key))
            .cloned()
            .or_else(|| find_glob_match(&ancestor, custom_types, ident.0).map(|ct| ct.id.clone()))
            .or_else(|| {
                if git_excluded.contains(&anc_key) && !user_included.contains(&anc_key) {
                    Some("excluded".to_owned())
                } else {
                    None
                }
            });
        let anc_color_override = user_colors
            .get(&anc_key)
            .or_else(|| shared_colors.get(&anc_key));

        if let Some(ref ar) = anc_role {
            if role_excludes_indexing(ar, custom_types) {
                excluded = true;
            }
        }
        if bg_color.is_none() {
            if let Some(override_hex) = anc_color_override {
                bg_color = Some(override_hex.clone());
            } else if let Some(ref ar) = anc_role {
                if role_inherits_bg(ar, custom_types) {
                    let (_, anc_base_color, anc_tint_bg, _, _, _, _) =
                        style_for_role(ar, custom_types);
                    if anc_tint_bg {
                        bg_color = anc_base_color;
                    }
                }
            }
        }
    }

    if source == "plain" && (bg_color.is_some() || excluded) {
        source = "inherited".to_owned();
    }

    ResolvedDirectoryEntry {
        role,
        source,
        icon,
        color: effective_color,
        bg_color,
        tint_text,
        tag,
        excluded,
        read_only_hint,
        auto_fallback_role: if user_role.is_some() || user_color.is_some() {
            auto_fallback_role
        } else {
            None
        },
    }
}

/// Returns `(icon, color, tint_bg, tint_text, tag, excluded, read_only_hint)` for a role.
#[allow(clippy::type_complexity)]
fn style_for_role(
    role: &str,
    custom_types: &BTreeMap<String, CustomDirectoryType>,
) -> (
    String,
    Option<String>,
    bool,
    bool,
    Option<String>,
    bool,
    bool,
) {
    if let Some(ct) = custom_types.get(role) {
        return (
            ct.icon.clone(),
            Some(ct.color.clone()),
            ct.tint_bg,
            ct.tint_text,
            if ct.tag.is_empty() {
                None
            } else {
                Some(ct.tag.clone())
            },
            ct.exclude_indexing,
            ct.read_only_hint,
        );
    }
    match role {
        "sources" => (
            "sources".into(),
            Some("#4e8df5".into()),
            true,
            false,
            Some("sources".into()),
            false,
            false,
        ),
        "tests" => (
            "tests".into(),
            Some("#57ab5a".into()),
            true,
            false,
            Some("tests".into()),
            false,
            false,
        ),
        "resources" => (
            "resources".into(),
            Some("#e09b3d".into()),
            false,
            false,
            Some("resources".into()),
            false,
            false,
        ),
        "testResources" => (
            "resources".into(),
            Some("#57ab5a".into()),
            false,
            false,
            Some("test resources".into()),
            false,
            false,
        ),
        "generated" => (
            "generated".into(),
            Some("#9f7aea".into()),
            true,
            false,
            Some("generated".into()),
            false,
            true,
        ),
        "excluded" => (
            "excluded".into(),
            Some("#e09b3d".into()),
            true,
            false,
            Some("excluded".into()),
            true,
            false,
        ),
        "module" => ("folder".into(), None, false, false, None, false, false),
        "package" => ("folder".into(), None, false, false, None, false, false),
        _ => ("folder".into(), None, false, false, None, false, false),
    }
}

fn role_excludes_indexing(
    role: &str,
    custom_types: &BTreeMap<String, CustomDirectoryType>,
) -> bool {
    if role == "excluded" {
        return true;
    }
    custom_types.get(role).is_some_and(|ct| ct.exclude_indexing)
}

fn role_inherits_bg(role: &str, custom_types: &BTreeMap<String, CustomDirectoryType>) -> bool {
    if let Some(ct) = custom_types.get(role) {
        return ct.inherit_bg;
    }
    matches!(role, "sources" | "tests" | "generated" | "excluded")
}

/// Returns workspace-relative ancestor paths from nearest parent up to `.`.
fn ancestors(path: &str) -> Vec<String> {
    if path == "." {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut current = path;
    while let Some((parent, _)) = current.rsplit_once('/') {
        if parent.is_empty() {
            break;
        }
        out.push(parent.to_owned());
        current = parent;
    }
    out.push(".".to_owned());
    out
}

fn index_map(map: &BTreeMap<String, String>, ident: &Identity) -> BTreeMap<String, String> {
    map.iter().map(|(k, v)| (ident.key(k), v.clone())).collect()
}

fn find_glob_match<'a>(
    path: &str,
    custom_types: &'a BTreeMap<String, CustomDirectoryType>,
    case_insensitive: bool,
) -> Option<&'a CustomDirectoryType> {
    let candidate = if case_insensitive {
        path.to_lowercase()
    } else {
        path.to_owned()
    };
    let basename = candidate.rsplit('/').next().unwrap_or(&candidate);
    for ct in custom_types.values() {
        if ct.match_glob.is_empty() {
            continue;
        }
        for raw_pat in ct.match_glob.split(',') {
            let pat = raw_pat.trim();
            if pat.is_empty() {
                continue;
            }
            let pat = if case_insensitive {
                pat.to_lowercase()
            } else {
                pat.to_owned()
            };
            let target = if pat.contains('/') {
                candidate.as_str()
            } else {
                basename
            };
            if simple_glob_match(&pat, target) {
                return Some(ct);
            }
        }
    }
    None
}

/// Matches a glob supporting `*`, `?` and `**` against a workspace-relative path.
fn simple_glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let mut dp = vec![vec![false; t.len() + 1]; p.len() + 1];
    dp[0][0] = true;
    for i in 1..=p.len() {
        if p[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        }
    }
    for i in 1..=p.len() {
        let is_double_star = p[i - 1] == '*' && i >= 2 && p[i - 2] == '*';
        for j in 1..=t.len() {
            if p[i - 1] == '*' {
                if is_double_star {
                    dp[i][j] = dp[i - 1][j] || dp[i][j - 1];
                } else {
                    dp[i][j] = dp[i - 1][j] || (t[j - 1] != '/' && dp[i][j - 1]);
                }
            } else if p[i - 1] == '?' {
                dp[i][j] = t[j - 1] != '/' && dp[i - 1][j - 1];
            } else {
                dp[i][j] = p[i - 1] == t[j - 1] && dp[i - 1][j - 1];
            }
        }
    }
    dp[p.len()][t.len()]
}

struct Identity(bool);

impl Identity {
    fn key(&self, path: &str) -> String {
        if self.0 {
            path.to_lowercase()
        } else {
            path.to_owned()
        }
    }
}

fn push_unique(list: &mut Vec<String>, key: String, ident: &Identity) {
    if !list
        .iter()
        .any(|existing| ident.key(existing) == ident.key(&key))
    {
        list.push(key);
    }
}

/// Removes every entry the predicate matches and returns the value of the exact key
/// when one matched, otherwise of the first match.
fn take_same(
    map: &mut BTreeMap<String, String>,
    same: &impl Fn(&String) -> bool,
) -> Option<String> {
    let keys = map
        .keys()
        .filter(|key| same(key))
        .cloned()
        .collect::<Vec<_>>();
    let mut first = None;
    for key in keys {
        let value = map.remove(&key);
        first = first.or(value);
    }
    first
}

fn response(changed: bool, rules: Rules) -> Value {
    json!({ "changed": changed, "rules": rules })
}

fn parse<T: serde::de::DeserializeOwned>(payload: Value) -> Result<T, CoreError> {
    serde_json::from_value(payload).map_err(|error| {
        CoreError::new(ErrorCode::InvalidRequest, "Invalid directory rules request")
            .with_details(error.to_string())
    })
}

fn validate_path(path: &str) -> Result<(), CoreError> {
    if invalid_relative_path(path, Platform::HOST) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Directory rule paths must be workspace-relative",
        ));
    }
    Ok(())
}

fn is_known_role(role: &str, custom_types: &[CustomDirectoryType]) -> bool {
    BUILTIN_ROLES.contains(&role) || custom_types.iter().any(|ct| ct.id == role)
}

fn validate_custom_type_id(id: &str) -> Result<(), CoreError> {
    let Some(rest) = id.strip_prefix("custom.") else {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Custom directory type ID must start with 'custom.'",
        ));
    };
    if rest.is_empty()
        || id.len() > MAX_TYPE_ID_LEN
        || !rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid custom directory type ID",
        ));
    }
    Ok(())
}

fn normalize_hex_color(raw: &str) -> Result<String, CoreError> {
    let trimmed = raw.trim();
    let Some(hex) = trimmed.strip_prefix('#') else {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Color must be a #rrggbb hex string",
        ));
    };
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Color must be a #rrggbb hex string",
        ));
    }
    Ok(format!("#{}", hex.to_ascii_lowercase()))
}

fn validate_custom_icon_id(id: &str) -> Result<(), CoreError> {
    let Some(rest) = id.strip_prefix("icon.") else {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Custom directory icon ID must start with 'icon.'",
        ));
    };
    if rest.is_empty()
        || id.len() > MAX_TYPE_ID_LEN
        || !rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid custom directory icon ID",
        ));
    }
    Ok(())
}

fn normalize_custom_icon(mut icon: CustomDirectoryIcon) -> Result<CustomDirectoryIcon, CoreError> {
    validate_custom_icon_id(&icon.id)?;
    icon.name = icon.name.trim().to_owned();
    if icon.name.is_empty() || icon.name.len() > MAX_TYPE_NAME_LEN {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Custom directory icon name must be 1-64 bytes",
        ));
    }
    icon.path = icon.path.trim().to_owned();
    if icon.path.is_empty()
        || icon.path.len() > MAX_GLOB_LEN
        || icon.path.contains("..")
        || icon.path.starts_with('/')
        || icon.path.contains('\\')
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid custom directory icon path",
        ));
    }
    Ok(icon)
}

fn normalize_custom_type(
    mut ct: CustomDirectoryType,
    custom_icons: &[CustomDirectoryIcon],
) -> Result<CustomDirectoryType, CoreError> {
    validate_custom_type_id(&ct.id)?;
    ct.name = ct.name.trim().to_owned();
    if ct.name.is_empty() || ct.name.len() > MAX_TYPE_NAME_LEN {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Custom directory type name must be 1-64 bytes",
        ));
    }
    ct.tag = ct.tag.trim().to_owned();
    if ct.tag.len() > MAX_TYPE_TAG_LEN {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Custom directory type tag exceeds 24 bytes",
        ));
    }
    let icon_known = ALLOWED_ICONS.contains(&ct.icon.as_str())
        || custom_icons.iter().any(|item| item.id == ct.icon);
    if !icon_known {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Unsupported directory type icon",
        ));
    }
    ct.color = normalize_hex_color(&ct.color)?;
    ct.match_glob = ct.match_glob.trim().to_owned();
    if ct.match_glob.len() > MAX_GLOB_LEN {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Custom directory type glob exceeds 256 bytes",
        ));
    }
    Ok(ct)
}

fn validate_rules(rules: &Rules) -> Result<(), CoreError> {
    if rules.marks.len()
        + rules.included_directories.len()
        + rules.previous_roles.len()
        + rules.colors.len()
        > MAX_ENTRIES
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Too many directory rules",
        ));
    }
    if rules.custom_icons.len() > MAX_CUSTOM_ICONS {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Too many custom directory icons",
        ));
    }
    let mut seen_icon_ids = BTreeSet::new();
    for icon in &rules.custom_icons {
        normalize_custom_icon(icon.clone())?;
        if !seen_icon_ids.insert(icon.id.as_str()) {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Duplicate custom directory icon ID",
            ));
        }
    }
    if rules.custom_types.len() > MAX_CUSTOM_TYPES {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Too many custom directory types",
        ));
    }
    let mut seen_ids = BTreeSet::new();
    for ct in &rules.custom_types {
        normalize_custom_type(ct.clone(), &rules.custom_icons)?;
        if !seen_ids.insert(ct.id.as_str()) {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Duplicate custom directory type ID",
            ));
        }
    }
    let paths = rules
        .marks
        .keys()
        .chain(&rules.included_directories)
        .chain(rules.previous_roles.keys())
        .chain(rules.colors.keys());
    for path in paths {
        validate_path(path)?;
    }
    for color in rules.colors.values() {
        normalize_hex_color(color)?;
    }
    for role in rules.marks.values().chain(rules.previous_roles.values()) {
        if !is_known_role(role, &rules.custom_types) {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Unknown directory role",
            ));
        }
    }
    Ok(())
}
