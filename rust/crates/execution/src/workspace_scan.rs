//! Shared run-discovery traversal and incomplete-snapshot diagnostics.
//!
//! Automatic discovery is best effort below the required workspace root. Declared
//! build-system modules remain owned by their provider and bypass these defaults.
//!
//! Heuristic discovery also skips the directories Git ignores: agent worktrees,
//! generated trees and local checkouts repeat or imitate the project's own
//! manifests, and listing them as run or build roots shows every module twice.
//! The rules come from the `.gitignore` files the scan lists and the `info/exclude`
//! of each repository whose root it lists. Nothing above the workspace root is
//! read, so a workspace opened below its repository root uses only the files
//! inside it. A global `core.excludesFile` and the index are not read either, so a
//! tracked manifest below an ignored directory is not discovered. Fingerprint
//! scans keep every directory: they decide freshness, and reading an ignored
//! input there only costs an extra regeneration.
//!
//! A heuristic scan can also be given [`IgnoredPaths`], the workspace's own
//! patterns (Build options keep them for build detection). They decide before
//! Git's rules, so `!pattern` brings back a directory Git ignores.

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::Match;
use lithe_protocol::CoreError;
use lithe_workspace::fs;
use std::collections::BTreeSet;
use std::io;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Heuristic discovery budget; full fingerprint scans retain cancellation checks.
const MAX_HEURISTIC_DIRECTORIES: usize = 4000;

/// Largest ignore file, gitfile or `commondir` file the scan reads. A larger one
/// contributes no rules, which only lets the scan see more directories.
const MAX_IGNORE_SOURCE_BYTES: u64 = 1 << 20;

const EXCLUDED_DIRECTORIES: &[&str] = &[
    ".git",
    ".worktree",
    ".worktrees",
    ".lithe",
    ".idea",
    ".vscode",
    ".gradle",
    ".venv",
    "venv",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    "__pycache__",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".terraform",
    ".cargo",
    ".stack-work",
    ".bundle",
    ".dart_tool",
    ".build",
    ".swiftpm",
    "node_modules",
    "bower_components",
    "vendor",
    "target",
    "build",
    "dist",
    "out",
    "obj",
    "coverage",
    "DerivedData",
    "Pods",
];

/// Default pruning for automatic run discovery, fingerprints and host file lists.
/// Hidden source directories are allowed unless explicitly named here.
pub fn is_run_scan_excluded_directory(name: &str) -> bool {
    EXCLUDED_DIRECTORIES
        .iter()
        .any(|excluded| excluded.eq_ignore_ascii_case(name))
}

/// One directory's files, without following directory symlinks.
pub struct ScannedDirectory {
    /// Absolute path of the directory.
    pub path: PathBuf,
    /// Names of the regular files directly inside it.
    pub files: BTreeSet<String>,
}

/// A partial scan must never be used as evidence of deletion or freshness.
pub struct WorkspaceScan {
    /// Readable directories, sorted by path.
    pub directories: Vec<ScannedDirectory>,
    /// Workspace-relative paths that could not be read; their contents are unknown.
    pub unreadable_paths: BTreeSet<String>,
}

/// Traverses automatic discovery inputs. A depth limit belongs only to heuristic
/// service detection, which also skips Git-ignored directories; fingerprints and
/// explicit host file lists use no depth limit and keep them.
pub fn scan_run_workspace(
    root: &Path,
    max_depth: Option<usize>,
) -> Result<WorkspaceScan, CoreError> {
    scan_with_reader(root, max_depth, &read_directory)
}

/// A heuristic scan that also skips the directories `ignored` names. Only a
/// heuristic scan takes them, so the depth limit is required here.
pub fn scan_run_workspace_ignoring(
    root: &Path,
    max_depth: usize,
    ignored: &IgnoredPaths,
) -> Result<WorkspaceScan, CoreError> {
    scan_with_readers(
        root,
        Some(max_depth),
        &read_directory,
        &read_ignore_source,
        ignored,
    )
}

/// Workspace-relative patterns in `.gitignore` syntax, one pattern per entry,
/// that a heuristic scan skips. They only ever name directories, since the scan
/// prunes a directory before listing it, so a file pattern such as `*.txt`
/// skips nothing. As in one ignore file, the last matching pattern decides, and
/// a pattern with a `/` before its end is anchored at the workspace root while
/// any other pattern matches a directory name at any depth (`.*` is every
/// directory whose name starts with a dot).
#[derive(Default)]
pub struct IgnoredPaths {
    matcher: Option<Gitignore>,
}

impl IgnoredPaths {
    /// Patterns relative to `root`. `Err` carries the index of the first one
    /// that is not a pattern: empty, padded with whitespace, holding a control
    /// character, a comment (`#`), a lone `!`, or a glob Git cannot parse; it is
    /// `patterns.len()` when each parses but they cannot be compiled together.
    pub fn new(root: &Path, patterns: &[String]) -> Result<Self, usize> {
        let mut builder = GitignoreBuilder::new(root);
        for (index, pattern) in patterns.iter().enumerate() {
            let bare = pattern.strip_prefix('!').unwrap_or(pattern);
            if bare.is_empty()
                || bare.trim() != bare
                || pattern.starts_with('#')
                || pattern.chars().any(char::is_control)
                || builder.add_line(None, pattern).is_err()
            {
                return Err(index);
            }
        }
        let matcher = builder.build().map_err(|_| patterns.len())?;
        Ok(Self {
            matcher: (!matcher.is_empty()).then_some(matcher),
        })
    }

    /// Whether the patterns decide about `directory`: `Some(true)` skips it,
    /// `Some(false)` keeps it whatever Git says, `None` leaves it to Git.
    fn decides(&self, directory: &Path) -> Option<bool> {
        match self.matcher.as_ref()?.matched(directory, true) {
            Match::Ignore(_) => Some(true),
            Match::Whitelist(_) => Some(false),
            Match::None => None,
        }
    }
}

fn read_directory(path: &Path) -> io::Result<Vec<(PathBuf, bool, bool)>> {
    fs::read_dir(path)?
        .map(|entry| {
            let entry = entry?;
            let kind = entry.file_type()?;
            Ok((entry.path(), kind.is_dir(), kind.is_symlink()))
        })
        .collect()
}

// Reads through the workspace adapter, like the directory listing, so a remote
// project's ignore files come from the remote host.
fn read_ignore_source(path: &Path) -> io::Result<String> {
    if fs::metadata(path)?.len() > MAX_IGNORE_SOURCE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ignore source exceeds the scan limit",
        ));
    }
    // A pattern that is not UTF-8 cannot match a name the scan keeps: names that
    // are not UTF-8 are reported unreadable instead of being traversed.
    Ok(String::from_utf8_lossy(&fs::read(path)?).into_owned())
}

/// Same traversal as [`scan_run_workspace`] with an injected directory reader,
/// so tests can fail or remove a directory between reads.
pub fn scan_with_reader(
    root: &Path,
    max_depth: Option<usize>,
    read: &impl Fn(&Path) -> io::Result<Vec<(PathBuf, bool, bool)>>,
) -> Result<WorkspaceScan, CoreError> {
    scan_with_readers(
        root,
        max_depth,
        read,
        &read_ignore_source,
        &IgnoredPaths::default(),
    )
}

// Tests also inject the reader of ignore files, gitfiles and `commondir`.
fn scan_with_readers(
    root: &Path,
    max_depth: Option<usize>,
    read: &impl Fn(&Path) -> io::Result<Vec<(PathBuf, bool, bool)>>,
    read_text: &impl Fn(&Path) -> io::Result<String>,
    ignored: &IgnoredPaths,
) -> Result<WorkspaceScan, CoreError> {
    let mut directories = Vec::new();
    let unreadable_paths =
        visit_with_readers(root, max_depth, read, read_text, ignored, |directory| {
            directories.push(directory);
            ControlFlow::Continue(())
        })?;
    directories.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(WorkspaceScan {
        directories,
        unreadable_paths,
    })
}

/// The ignore rules in force in one directory: its own `.gitignore` first, then
/// its ancestors' up to the repository's `info/exclude`. As in Git, the first file
/// with a matching pattern decides, so a lower file can re-include a directory.
struct IgnoreRules {
    matcher: Gitignore,
    parent: Option<Rc<IgnoreRules>>,
}

fn is_ignored_directory(
    ignored: &IgnoredPaths,
    rules: Option<&Rc<IgnoreRules>>,
    path: &Path,
) -> bool {
    if let Some(decision) = ignored.decides(path) {
        return decision;
    }
    let mut current = rules;
    while let Some(rules) = current {
        match rules.matcher.matched(path, true) {
            Match::Ignore(_) => return true,
            Match::Whitelist(_) => return false,
            Match::None => current = rules.parent.as_ref(),
        }
    }
    false
}

/// Rules from one ignore file, with patterns relative to `directory`. `None` when
/// the file has no pattern the matcher accepts.
fn ignore_matcher(directory: &Path, text: &str) -> Option<Gitignore> {
    let mut builder = GitignoreBuilder::new(directory);
    for (index, line) in text.lines().enumerate() {
        // UTF-8 BOM is an encoding marker in Git's ignore files, not a glob.
        let line = if index == 0 {
            line.strip_prefix('\u{feff}').unwrap_or(line)
        } else {
            line
        };
        // Git skips a pattern it cannot parse and keeps the rest of the file.
        let _ = builder.add_line(None, line);
    }
    builder.build().ok().filter(|matcher| !matcher.is_empty())
}

/// An ignore source that cannot be read adds no rules, so the scan sees more,
/// never less. A broken remote transport still fails the scan, as it does for a
/// directory, because it says nothing about the file.
fn read_rules_text(
    path: &Path,
    read_text: &impl Fn(&Path) -> io::Result<String>,
) -> Result<Option<String>, CoreError> {
    match read_text(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if fs::is_transport_failure(&error) => Err(CoreError::from(error)),
        Err(_) => Ok(None),
    }
}

/// The `info/exclude` of the repository whose worktree is `worktree`. A gitfile
/// (linked worktree or submodule) points at its gitdir; a linked worktree's gitdir
/// names, in `commondir`, the main gitdir that holds the shared `info/exclude`.
fn repository_exclude(
    worktree: &Path,
    gitdir_is_directory: bool,
    read_text: &impl Fn(&Path) -> io::Result<String>,
) -> Result<Option<String>, CoreError> {
    let marker = worktree.join(".git");
    let gitdir = if gitdir_is_directory {
        marker
    } else {
        let Some(text) = read_rules_text(&marker, read_text)? else {
            return Ok(None);
        };
        let Some(target) = text
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("gitdir:"))
            .map(str::trim)
            .filter(|target| !target.is_empty())
        else {
            return Ok(None);
        };
        // An absolute target replaces the worktree path; a relative one is
        // relative to the gitfile's directory, as Git resolves it.
        worktree.join(target)
    };
    let common = match read_rules_text(&gitdir.join("commondir"), read_text)? {
        Some(text) => match text.lines().next().map(str::trim) {
            Some(common) if !common.is_empty() => gitdir.join(common),
            _ => gitdir,
        },
        None => gitdir,
    };
    read_rules_text(&common.join("info").join("exclude"), read_text)
}

/// The rules for the directory being listed. A directory with its own `.git`
/// starts a repository that Git governs only by its own ignore files, so the
/// enclosing repository's rules stop there.
fn directory_rules(
    directory: &Path,
    inherited: Option<Rc<IgnoreRules>>,
    repository: Option<bool>,
    has_gitignore: bool,
    read_text: &impl Fn(&Path) -> io::Result<String>,
) -> Result<Option<Rc<IgnoreRules>>, CoreError> {
    let mut rules = match repository {
        Some(gitdir_is_directory) => repository_exclude(directory, gitdir_is_directory, read_text)?
            .and_then(|text| ignore_matcher(directory, &text))
            .map(|matcher| {
                Rc::new(IgnoreRules {
                    matcher,
                    parent: None,
                })
            }),
        None => inherited,
    };
    if has_gitignore {
        if let Some(matcher) = read_rules_text(&directory.join(".gitignore"), read_text)?
            .and_then(|text| ignore_matcher(directory, &text))
        {
            rules = Some(Rc::new(IgnoreRules {
                matcher,
                parent: rules,
            }));
        }
    }
    Ok(rules)
}

// Consumers decide whether to retain each directory and when enough inputs have
// been found. Early stopping must not change full fingerprint scan semantics.
fn visit_with_readers(
    root: &Path,
    max_depth: Option<usize>,
    read: &impl Fn(&Path) -> io::Result<Vec<(PathBuf, bool, bool)>>,
    read_text: &impl Fn(&Path) -> io::Result<String>,
    ignored: &IgnoredPaths,
    mut visit: impl FnMut(ScannedDirectory) -> ControlFlow<()>,
) -> Result<BTreeSet<String>, CoreError> {
    let heuristic = max_depth.is_some();
    let mut unreadable_paths = BTreeSet::new();
    let mut directory_count = 0;
    let mut pending: Vec<(PathBuf, usize, Option<Rc<IgnoreRules>>)> =
        vec![(root.to_owned(), 0, None)];
    while let Some((path, depth, inherited)) = pending.pop() {
        lithe_protocol::cancellation::check()?;
        if heuristic && directory_count >= MAX_HEURISTIC_DIRECTORIES {
            unreadable_paths.insert(relative(root, &path));
            unreadable_paths.extend(pending.iter().map(|(path, _, _)| relative(root, path)));
            break;
        }
        let entries = match read(&path) {
            Ok(entries) => entries,
            Err(error) if path == root => return Err(CoreError::from(error)),
            // A broken remote transport says nothing about this directory, and
            // continuing would publish a model built from a truncated tree.
            // Failures the host reports for one path stay explicit incomplete-scan
            // evidence, exactly as they do for a local workspace.
            Err(error) if fs::is_transport_failure(&error) => return Err(CoreError::from(error)),
            Err(_) => {
                unreadable_paths.insert(relative(root, &path));
                continue;
            }
        };
        let mut files = BTreeSet::new();
        let mut subdirectories = Vec::new();
        // Whether this directory holds a `.git` entry, and if so whether it is a
        // gitdir (true) or a gitfile pointing at one (false).
        let mut repository = None;
        for (child, directory, symlink) in entries {
            lithe_protocol::cancellation::check()?;
            let Some(name) = child.file_name().and_then(|name| name.to_str()) else {
                unreadable_paths.insert(relative(root, &child));
                continue;
            };
            if symlink {
                continue;
            }
            if name == ".git" {
                repository = Some(directory);
            }
            if directory {
                if !is_run_scan_excluded_directory(name)
                    && max_depth.is_none_or(|limit| depth < limit)
                {
                    subdirectories.push(child);
                }
            } else {
                files.insert(name.to_owned());
            }
        }
        let rules = if heuristic {
            directory_rules(
                &path,
                inherited,
                repository,
                files.contains(".gitignore"),
                read_text,
            )?
        } else {
            None
        };
        let mut children = subdirectories
            .into_iter()
            .filter(|child| !is_ignored_directory(ignored, rules.as_ref(), child))
            .map(|child| (child, depth + 1, rules.clone()))
            .collect::<Vec<_>>();
        children.sort_by(|left, right| right.0.cmp(&left.0));
        pending.extend(children);
        directory_count += 1;
        if visit(ScannedDirectory { path, files }).is_break() {
            break;
        }
    }
    Ok(unreadable_paths)
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn early_stop_retains_observed_unreadable_paths_without_affecting_full_scans() {
        let root = Path::new("fixture");
        let read = |path: &Path| {
            if path == root {
                Ok(vec![
                    (root.join("a-blocked"), true, false),
                    (root.join("b-sources"), true, false),
                    (root.join("z-later"), true, false),
                ])
            } else if path.ends_with("a-blocked") {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            } else {
                Ok(vec![(path.join("main.txt"), false, false)])
            }
        };
        let unknown = visit_with_readers(
            root,
            None,
            &read,
            &read_ignore_source,
            &IgnoredPaths::default(),
            |directory| {
                if directory.files.contains("main.txt") {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            },
        )
        .unwrap();
        assert_eq!(unknown, BTreeSet::from(["a-blocked".into()]));
        let full = scan_with_reader(root, None, &read).unwrap();
        assert_eq!(full.unreadable_paths, unknown);
        assert!(full
            .directories
            .iter()
            .any(|dir| dir.path.ends_with("z-later")));
    }

    #[test]
    fn unreadable_child_preserves_siblings_but_root_is_required() {
        let root = Path::new("fixture");
        let read = |path: &Path| {
            if path == root {
                Ok(vec![
                    (root.join("blocked"), true, false),
                    (root.join(".sources"), true, false),
                    (root.join(".pytest_cache"), true, false),
                ])
            } else if path.ends_with(".sources") {
                Ok(vec![(path.join("main.txt"), false, false)])
            } else {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            }
        };
        let result = scan_with_reader(root, None, &read).unwrap();
        assert_eq!(result.unreadable_paths, BTreeSet::from(["blocked".into()]));
        assert!(result
            .directories
            .iter()
            .any(|dir| dir.files.contains("main.txt")));
        assert!(scan_with_reader(root, None, &|_| Err(io::Error::from(
            io::ErrorKind::PermissionDenied
        )))
        .is_err());
    }
    #[test]
    fn directory_links_do_not_escape_or_reenter_the_scan() {
        let root = Path::new("fixture");
        let result = scan_with_reader(root, None, &|path| {
            assert_eq!(path, root, "links must never be traversed");
            Ok(vec![
                (root.join("cycle"), true, true),
                (root.join("main.txt"), false, false),
            ])
        })
        .unwrap();
        assert_eq!(result.directories.len(), 1);
        assert_eq!(
            result.directories[0].files,
            BTreeSet::from(["main.txt".into()])
        );
    }

    /// A tree in memory: each directory lists its entries, a trailing `/` marking
    /// a directory, and `texts` holds the files the scan reads as rules, keyed by
    /// their path with `..` resolved.
    struct Tree {
        directories: Vec<(&'static str, Vec<&'static str>)>,
        texts: Vec<(&'static str, &'static str)>,
    }

    fn resolved(path: &Path) -> String {
        let mut parts = Vec::new();
        for component in path.components() {
            match component {
                std::path::Component::ParentDir => {
                    parts.pop();
                }
                std::path::Component::Normal(part) => parts.push(part.to_string_lossy()),
                _ => {}
            }
        }
        parts.join("/")
    }

    impl Tree {
        /// The directories a scan of `root` lists, relative to it.
        fn scan(&self, root: &str, max_depth: Option<usize>) -> BTreeSet<String> {
            self.scan_with(root, max_depth, &IgnoredPaths::default())
        }

        /// The directories a heuristic scan of `root` lists when it also skips
        /// `patterns`.
        fn scan_ignoring(&self, root: &str, patterns: &[&str]) -> BTreeSet<String> {
            let patterns: Vec<String> = patterns
                .iter()
                .map(|pattern| (*pattern).to_owned())
                .collect();
            let ignored = IgnoredPaths::new(Path::new(root), &patterns).unwrap();
            self.scan_with(root, Some(6), &ignored)
        }

        fn scan_with(
            &self,
            root: &str,
            max_depth: Option<usize>,
            ignored: &IgnoredPaths,
        ) -> BTreeSet<String> {
            let read = |path: &Path| -> io::Result<Vec<(PathBuf, bool, bool)>> {
                let key = resolved(path);
                let (_, entries) = self
                    .directories
                    .iter()
                    .find(|(directory, _)| *directory == key)
                    .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
                Ok(entries
                    .iter()
                    .map(|entry| match entry.strip_suffix('/') {
                        Some(name) => (path.join(name), true, false),
                        None => (path.join(entry), false, false),
                    })
                    .collect())
            };
            let read_text = |path: &Path| -> io::Result<String> {
                let key = resolved(path);
                self.texts
                    .iter()
                    .find(|(file, _)| *file == key)
                    .map(|(_, text)| (*text).to_owned())
                    .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
            };
            let root = Path::new(root);
            let scan = scan_with_readers(root, max_depth, &read, &read_text, ignored).unwrap();
            assert!(scan.unreadable_paths.is_empty());
            scan.directories
                .iter()
                .map(|directory| relative(root, &directory.path))
                .collect()
        }
    }

    fn set<const N: usize>(paths: [&str; N]) -> BTreeSet<String> {
        paths.into_iter().map(str::to_owned).collect()
    }

    #[test]
    fn heuristic_discovery_skips_what_git_ignores_and_full_scans_keep_it() {
        let tree = Tree {
            directories: vec![
                (
                    "fixture",
                    vec![".git/", ".gitignore", ".claude/", "generated/", "src/"],
                ),
                ("fixture/.claude", vec!["settings/", "worktrees/"]),
                ("fixture/.claude/settings", vec![]),
                ("fixture/.claude/worktrees", vec!["copy/"]),
                ("fixture/.claude/worktrees/copy", vec![".git", "go.mod"]),
                ("fixture/generated", vec!["go.mod"]),
                ("fixture/src", vec![".gitignore", "cache/", "generated/"]),
                ("fixture/src/cache", vec![]),
                ("fixture/src/generated", vec!["go.mod"]),
            ],
            texts: vec![
                ("fixture/.gitignore", "\u{feff}generated/\n"),
                // Agent tools keep their worktrees out of Git here, not in
                // `.gitignore`; a pattern Git cannot parse leaves the rest in force.
                (
                    "fixture/.git/info/exclude",
                    "# local\n[z-a]\n.claude/worktrees/\n",
                ),
                // A lower file re-includes what the root ignores and adds its own.
                ("fixture/src/.gitignore", "!generated/\ncache/\n"),
            ],
        };
        assert_eq!(
            tree.scan("fixture", Some(6)),
            set(["", ".claude", ".claude/settings", "src", "src/generated"])
        );
        assert_eq!(
            tree.scan("fixture", None),
            set([
                "",
                ".claude",
                ".claude/settings",
                ".claude/worktrees",
                ".claude/worktrees/copy",
                "generated",
                "src",
                "src/cache",
                "src/generated",
            ])
        );
    }

    #[test]
    fn a_linked_worktree_uses_the_shared_exclude_and_a_nested_repository_its_own_rules() {
        let tree = Tree {
            directories: vec![
                ("work", vec![".git", "lib/", "logs/"]),
                ("work/logs", vec![]),
                ("work/lib", vec![".git/", ".gitignore", "logs/", "tmp/"]),
                ("work/lib/logs", vec![]),
                ("work/lib/tmp", vec![]),
            ],
            texts: vec![
                ("work/.git", "gitdir: ../main/.git/worktrees/work\n"),
                ("main/.git/worktrees/work/commondir", "../..\n"),
                ("main/.git/info/exclude", "logs/\n"),
                ("work/lib/.gitignore", "tmp/\n"),
            ],
        };
        // The enclosing `logs/` stops at `lib`, a repository of its own.
        assert_eq!(tree.scan("work", Some(6)), set(["", "lib", "lib/logs"]));
    }

    #[test]
    fn an_ignore_file_that_cannot_be_read_hides_nothing() {
        let tree = Tree {
            directories: vec![
                ("fixture", vec![".git", ".gitignore", "generated/"]),
                ("fixture/generated", vec![]),
            ],
            // Neither the `.gitignore` nor the gitfile can be read.
            texts: vec![],
        };
        assert_eq!(tree.scan("fixture", Some(6)), set(["", "generated"]));
    }

    #[test]
    fn ignored_paths_decide_before_git_and_the_last_matching_pattern_wins() {
        let tree = Tree {
            directories: vec![
                (
                    "fixture",
                    vec![
                        ".git/",
                        ".gitignore",
                        ".claude/",
                        ".config/",
                        "generated/",
                        "tools/",
                        "src/",
                    ],
                ),
                ("fixture/.claude", vec!["worktrees/"]),
                ("fixture/.claude/worktrees", vec![]),
                ("fixture/.config", vec![]),
                ("fixture/generated", vec![]),
                ("fixture/tools", vec!["fixtures/"]),
                ("fixture/tools/fixtures", vec![]),
                ("fixture/src", vec!["tools/", ".hidden/"]),
                ("fixture/src/tools", vec![]),
                ("fixture/src/.hidden", vec![]),
            ],
            texts: vec![("fixture/.gitignore", "generated/\n")],
        };
        // `.*` skips every dot directory at any depth; Git still skips `generated`.
        assert_eq!(
            tree.scan_ignoring("fixture", &[".*"]),
            set(["", "src", "src/tools", "tools", "tools/fixtures"])
        );
        // A later `!` keeps one dot directory and overrides Git, and a pattern
        // with a leading `/` matches only below the root.
        assert_eq!(
            tree.scan_ignoring("fixture", &[".*", "!.config", "!generated", "/tools"]),
            set(["", ".config", "generated", "src", "src/tools"])
        );
        // No pattern leaves only Git's rules.
        assert_eq!(
            tree.scan_ignoring("fixture", &[]),
            tree.scan("fixture", Some(6))
        );
    }

    #[test]
    fn an_ignored_path_list_names_the_first_entry_that_is_not_a_pattern() {
        let root = Path::new("fixture");
        let check = |list: &[&str]| {
            let patterns: Vec<String> = list.iter().map(|pattern| (*pattern).to_owned()).collect();
            IgnoredPaths::new(root, &patterns).err()
        };
        assert_eq!(
            check(&[".*", "!.config", "build/", "a/**/b", "\\#literal"]),
            None
        );
        for (list, index) in [
            (&["ok", ""][..], 1),
            (&[" padded"][..], 0),
            (&["trailing "][..], 0),
            (&["# comment"][..], 0),
            (&["!"][..], 0),
            (&["! spaced"][..], 0),
            (&["line\nbreak"][..], 0),
            (&["ok", "[z-a]"][..], 1),
        ] {
            assert_eq!(check(list), Some(index), "{list:?}");
        }
    }
}
