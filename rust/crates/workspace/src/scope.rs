//! Request-scoped host identity for existing workspace semantic engines.
//!
//! A remote request never falls back to the placeholder filesystem or local
//! executables. Git continues to own Git semantics; this boundary supplies IO.

use crate::sessions::{lookup, Connection};
use lithe_platform::{
    paths::{invalid_relative_path, is_absolute_path, join_native, slash_separated},
    Platform,
};
use lithe_protocol::{CoreError, ErrorCode};
use serde::Deserialize;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Host and path identity supplied by the native workspace registry.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Workspace {
    session_id: String,
    local_root: PathBuf,
    remote_root: String,
}

/// The connected host a request's workspace lives on, and its path mapping.
#[derive(Clone)]
pub struct Host {
    /// Session that reaches the host.
    pub connection: Arc<Connection>,
    workspace: Workspace,
}

thread_local! { static HOST: RefCell<Option<Host>> = const { RefCell::new(None) }; }

/// Restores the caller's host for nested Core requests and error paths.
pub struct Scope(Option<Host>);

impl Scope {
    /// Installs the host of `workspace` for the current request, or no host
    /// when `workspace` is `None`. Fails when a root is relative or climbs
    /// with `..`, or when the session is not connected.
    pub fn begin(workspace: Option<Workspace>) -> Result<Self, CoreError> {
        let host = workspace
            .map(|mut workspace| {
                let root = normalize_remote_path(&workspace.remote_root);
                let Some(root) = root.filter(|_| {
                    valid_root(&workspace.local_root) && workspace.local_root.file_name().is_some()
                }) else {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Workspace roots must be absolute without parent components",
                    ));
                };
                workspace.remote_root = root;
                Ok(Host {
                    connection: lookup(&workspace.session_id)?,
                    workspace,
                })
            })
            .transpose()?;
        Ok(Self(HOST.with(|current| current.replace(host))))
    }
}

/// Installs an already validated session owner on a backend worker.
pub fn enter(host: Option<Host>) -> Scope {
    Scope(HOST.with(|current| current.replace(host)))
}

fn valid_root(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
}

fn normalize_remote_path(path: &str) -> Option<String> {
    // Shipped workspace servers run on Unix, even when the client is Windows.
    if !is_absolute_path(path, Platform::Unix) || path.contains('\0') {
        return None;
    }
    let mut normalized = String::with_capacity(path.len());
    for part in path
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
    {
        if part == ".." {
            return None;
        }
        normalized.push('/');
        normalized.push_str(part);
    }
    if normalized.is_empty() {
        normalized.push('/');
    }
    Some(normalized)
}

impl Drop for Scope {
    fn drop(&mut self) {
        HOST.with(|current| current.replace(self.0.take()));
    }
}

/// The host installed for the current request, if any.
pub fn current() -> Option<Host> {
    HOST.with(|host| host.borrow().clone())
}

/// Whether filesystem and tool operations belong to a connected workspace host.
pub fn is_remote() -> bool {
    current().is_some()
}

impl Host {
    /// Maps schema-owned file URIs while preserving host identity and escaping.
    pub fn map_uri(&self, value: &str, outbound: bool) -> Option<String> {
        let uri = url::Url::parse(value).ok()?;
        if uri.scheme() != "file"
            || uri
                .host_str()
                .is_some_and(|host| !host.is_empty() && host != "localhost")
        {
            return None;
        }
        let path = uri.to_file_path().ok()?;
        let mapped = if outbound {
            self.remote_path(&path)
        } else {
            self.local_path(&path)
        };
        if mapped == path {
            return None;
        }
        url::Url::from_file_path(mapped)
            .ok()
            .map(|uri| uri.to_string())
    }

    /// Translates a placeholder path to the host path it stands for; paths
    /// outside the workspace stay as they are.
    pub fn remote_path(&self, path: &Path) -> PathBuf {
        self.workspace
            .project_to_remote(path, Platform::HOST)
            .unwrap_or_else(|| path.to_owned())
    }

    pub(crate) fn local_path(&self, path: &Path) -> PathBuf {
        path.to_str()
            .and_then(|path| self.workspace.project_to_local(path, Platform::HOST))
            .unwrap_or_else(|| path.to_owned())
    }

    /// The id of the session this host belongs to, which keys process-wide
    /// state released through [`crate::sessions::on_session_end`].
    pub fn session_id(&self) -> &str {
        &self.workspace.session_id
    }
}

impl Workspace {
    fn project_to_remote(&self, path: &Path, client: Platform) -> Option<PathBuf> {
        let (root, suffix) = if let Ok(suffix) = path.strip_prefix(self.host_paths_root()) {
            ("/", suffix)
        } else {
            (
                self.remote_root.as_str(),
                path.strip_prefix(&self.local_root).ok()?,
            )
        };
        let suffix = slash_separated(suffix.to_str()?, client);
        Some(if suffix.is_empty() {
            PathBuf::from(root)
        } else {
            PathBuf::from(join_native(root, &suffix, Platform::Unix))
        })
    }

    fn project_to_local(&self, path: &str, client: Platform) -> Option<PathBuf> {
        let path = normalize_remote_path(path)?;
        if path == self.remote_root {
            return Some(self.local_root.clone());
        }
        let (root, suffix) = if let Some(suffix) = path
            .strip_prefix(self.remote_root.trim_end_matches('/'))
            .and_then(|suffix| suffix.strip_prefix('/'))
        {
            (self.local_root.clone(), suffix)
        } else {
            (self.host_paths_root(), path.trim_start_matches('/'))
        };
        if suffix.is_empty() {
            return Some(root);
        }
        // A name legal on Unix can be a separator, drive or stream on Windows.
        // Mapping it to a native path would change its identity.
        (!invalid_relative_path(suffix, client)).then(|| root.join(suffix))
    }

    fn host_paths_root(&self) -> PathBuf {
        // A sibling namespace cannot collide with files in the opened folder.
        let mut name = self.local_root.file_name().unwrap().to_os_string();
        name.push(".lithe-remote-host");
        self.local_root.with_file_name(name)
    }
}

/// Projects host paths back onto the native workspace's URL identity.
pub fn local_path(path: &Path) -> PathBuf {
    current()
        .map(|host| host.local_path(path))
        .unwrap_or_else(|| path.to_owned())
}

/// Translates a schema-owned path to its host; opaque argv and text stay untouched.
pub fn remote_path(path: &Path) -> PathBuf {
    current()
        .map(|host| host.remote_path(path))
        .unwrap_or_else(|| path.to_owned())
}

/// Includes host identity in process-local repository coordination keys.
pub fn identity_key(path: &Path) -> PathBuf {
    current()
        .map(|host| PathBuf::from(host.session_id()).join(path.strip_prefix("/").unwrap_or(path)))
        .unwrap_or_else(|| path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(remote_root: &str) -> Workspace {
        Workspace {
            session_id: "missing-test-session".into(),
            local_root: std::env::temp_dir().join("remote-workspace"),
            remote_root: remote_root.into(),
        }
    }

    #[test]
    fn remote_root_validation_precedes_connection_lookup() {
        for (path, expected) in [
            ("/srv/repo", ErrorCode::ConnectionClosed),
            ("/", ErrorCode::ConnectionClosed),
            ("/srv/./repo//", ErrorCode::ConnectionClosed),
            ("srv/repo", ErrorCode::InvalidRequest),
            (r"C:\srv\repo", ErrorCode::InvalidRequest),
            ("/srv/../repo", ErrorCode::InvalidRequest),
            ("/srv/\0repo", ErrorCode::InvalidRequest),
        ] {
            let error = match Scope::begin(Some(workspace(path))) {
                Err(error) => error,
                Ok(_) => panic!("unexpected session for {path:?}"),
            };
            assert_eq!(
                std::mem::discriminant(&error.code),
                std::mem::discriminant(&expected),
                "{path:?}: got {:?}",
                error.code
            );
        }
    }

    #[test]
    fn workspace_and_host_paths_keep_their_namespace() {
        let workspace = workspace("/srv/repo");
        for client in Platform::ALL {
            let relative = Path::new("src").join("main.rs");
            let local = workspace.local_root.join(relative);
            let remote = workspace.project_to_remote(&local, client).unwrap();
            assert_eq!(remote.to_str(), Some("/srv/repo/src/main.rs"));
            assert_eq!(
                workspace.project_to_local("/srv/repo/./src//main.rs", client),
                Some(local)
            );

            let external = workspace.host_paths_root().join("srv/repository/lib.rs");
            assert_eq!(
                workspace.project_to_local("/srv/repository/lib.rs", client),
                Some(external.clone())
            );
            assert_eq!(
                workspace
                    .project_to_remote(&external, client)
                    .unwrap()
                    .to_str(),
                Some("/srv/repository/lib.rs")
            );
            assert_eq!(
                workspace.project_to_local("/", client),
                Some(workspace.host_paths_root())
            );
        }
    }

    #[test]
    fn unix_file_names_cannot_become_windows_separators_or_drives() {
        let workspace = workspace("/");
        for name in [r"dir\leaf", "D:leaf"] {
            let remote = format!("/{name}");
            assert_eq!(
                workspace.project_to_local(&remote, Platform::Unix),
                Some(workspace.local_root.join(name))
            );
            assert!(workspace
                .project_to_local(&remote, Platform::Windows)
                .is_none());
        }
    }
}
