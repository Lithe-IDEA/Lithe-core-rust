//! Request-scoped host identity for existing workspace semantic engines.
//!
//! A remote request never falls back to the placeholder filesystem or local
//! executables. Git continues to own Git semantics; this boundary supplies IO.

use crate::sessions::{lookup, Connection};
use lithe_document_uri::DocumentPathPlatform;
use lithe_platform::{
    paths::{invalid_relative_path, is_absolute_path, join_native, slash_separated},
    Platform,
};
use lithe_protocol::{CoreError, ErrorCode};
use percent_encoding::percent_decode_str;
use serde::Deserialize;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Host and path identity supplied by the native workspace registry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
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
    /// Captures the connection's validated path identities without retaining its transport.
    pub fn path_mapping(&self) -> Workspace {
        self.workspace.clone()
    }

    /// Workspace root as an absolute POSIX path on this host.
    pub fn remote_root(&self) -> &str {
        &self.workspace.remote_root
    }

    /// Maps a scan ID relative to the opened root, even if the server resolved
    /// that root through a symlink before walking its children.
    pub fn local_workspace_path(&self, relative: &str) -> Option<PathBuf> {
        if relative.is_empty() {
            return Some(self.workspace.local_root.clone());
        }
        if invalid_relative_path(relative, Platform::HOST) {
            return None;
        }
        let remote = join_native(&self.workspace.remote_root, relative, Platform::Unix);
        self.workspace.project_to_local(&remote, Platform::HOST)
    }

    /// Maps an absolute host path only if the native client can represent it.
    pub fn project_host_path(&self, path: &Path) -> Option<PathBuf> {
        self.workspace
            .project_to_local(path.to_str()?, Platform::HOST)
    }

    /// Maps schema-owned file URIs while preserving host identity and escaping.
    pub fn map_uri(&self, value: &str, outbound: bool) -> Option<String> {
        self.workspace.map_uri(value, outbound, Platform::HOST)
    }

    /// Translates a placeholder path to the host path it stands for; paths
    /// outside the workspace stay as they are.
    pub fn remote_path(&self, path: &Path) -> PathBuf {
        self.workspace
            .project_to_remote(path, Platform::HOST)
            .unwrap_or_else(|| path.to_owned())
    }

    pub(crate) fn local_path(&self, path: &Path) -> PathBuf {
        self.project_host_path(path)
            .unwrap_or_else(|| path.to_owned())
    }

    /// The id of the session this host belongs to, which keys process-wide
    /// state released through [`crate::sessions::on_session_end`].
    pub fn session_id(&self) -> &str {
        &self.workspace.session_id
    }
}

impl Workspace {
    /// Native absolute root used to identify documents in this workspace.
    pub fn local_root(&self) -> &Path {
        &self.local_root
    }

    /// Projects a native document path into the host's filesystem namespace.
    pub fn to_host_path(&self, path: &str) -> Option<String> {
        self.project_to_remote(Path::new(path), Platform::HOST)?
            .into_os_string()
            .into_string()
            .ok()
    }

    /// Projects an absolute host path into the client's document namespace.
    pub fn to_document_path(&self, path: &str) -> Option<String> {
        self.project_to_local(path, Platform::HOST)?
            .into_os_string()
            .into_string()
            .ok()
    }

    /// Returns the POSIX path relative to the workspace root for a scoped host operation.
    pub fn remote_relative_path(&self, path: &str) -> Option<String> {
        let path = normalize_remote_path(path)?;
        self.remote_suffix(&path).map(str::to_owned)
    }

    fn remote_suffix<'a>(&self, path: &'a str) -> Option<&'a str> {
        if path == self.remote_root {
            return Some("");
        }
        path.strip_prefix(self.remote_root.trim_end_matches('/'))?
            .strip_prefix('/')
    }

    fn map_uri(&self, value: &str, outbound: bool, client: Platform) -> Option<String> {
        let uri = url::Url::parse(value).ok()?;
        if uri.scheme() != "file" || uri.query().is_some() || uri.fragment().is_some() {
            return None;
        }
        if outbound {
            // The client identity is native even when the target always uses POSIX.
            let local = uri.to_file_path().ok()?;
            let remote = self.project_to_remote(&local, client)?;
            DocumentPathPlatform::Posix
                .document_uri(remote.to_str()?)
                .ok()
        } else {
            // URL::to_file_path interprets paths using the compiled client OS. A
            // server URI always names a POSIX path, even on a Windows client.
            if uri.host_str().is_some_and(|host| host != "localhost") {
                return None;
            }
            let remote = percent_decode_str(uri.path()).decode_utf8().ok()?;
            let local = self.project_to_local(&remote, client)?;
            url::Url::from_file_path(local).ok().map(|uri| uri.into())
        }
    }

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
        let (root, suffix) = if let Some(suffix) = self.remote_suffix(&path) {
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
    fn remote_file_uris_use_posix_syntax_and_keep_escaped_names() {
        let workspace = workspace("/srv/repo");
        for client in Platform::ALL {
            let local = workspace.local_root.join("src/a #明%.rs");
            let local_uri = url::Url::from_file_path(local).unwrap().to_string();
            let remote_uri = "file:///srv/repo/src/a%20%23%E6%98%8E%25.rs";
            assert_eq!(
                workspace.map_uri(&local_uri, true, client).as_deref(),
                Some(remote_uri)
            );
            assert_eq!(
                workspace.map_uri(remote_uri, false, client),
                Some(local_uri)
            );
            for invalid in [
                "file://foreign/srv/repo/a.rs",
                "file:///srv/repo/a%00.rs",
                "file:///srv/repo/a%FF.rs",
                "file:///srv/repo/a.rs?x",
                "file:///srv/repo/a.rs#x",
            ] {
                assert_eq!(workspace.map_uri(invalid, false, client), None, "{invalid}");
            }
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
