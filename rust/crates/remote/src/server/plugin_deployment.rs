//! Private, content-addressed cache for approved plugin process inputs.
//! A shared directory lock leases the immutable tree across workspace servers.

use crate::protocol::methods::{
    self, CommitManagedRuntimeParams, CommitPluginDeploymentParams, ManagedRuntimeLayout,
    PluginDeploymentKey, SweepPluginDeploymentsParams,
};
use crate::protocol::{ErrorCode, RemoteError};
use crate::server::connection::Context;
use crate::server::fs::{host_path, io_error};
use crate::server::run_directory::{
    lock_private_directory, private_directory, require_private_directory,
};
use crate::server::{lock, managed_runtime, parse, platform};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Mutex;

const MAX_LEASES: usize = 512;
const MAX_INSTALLED: usize = 8192;

/// Directory locks held until the session's processes have been stopped.
pub(crate) struct Deployments {
    home: Option<String>,
    leases: Mutex<BTreeMap<PathBuf, File>>,
}

impl Deployments {
    /// Retains the account home resolved when the server creates the session.
    pub(crate) fn new(home: Option<String>) -> Self {
        Self {
            home,
            leases: Mutex::default(),
        }
    }

    /// Handles a finite cache operation on the connected session.
    pub(crate) fn handle(
        &self,
        context: &Context,
        method: &str,
        params: Value,
    ) -> Result<Value, RemoteError> {
        if context.session.is_ending() || context.closed.load(Ordering::Acquire) {
            return Err(RemoteError::new(ErrorCode::NotFound, "session ended"));
        }
        let runtime = matches!(
            method,
            methods::FS_MANAGED_RUNTIME_LOOKUP | methods::FS_MANAGED_RUNTIME_COMMIT
        );
        let cache = cache_root(self.home.as_deref(), runtime)?;
        match method {
            methods::FS_PLUGIN_DEPLOYMENT_LOOKUP => {
                let key: PluginDeploymentKey = parse(params)?;
                let path = deployment_path(&cache, &key)?;
                Ok(json!({"path": self.acquire(context, &path)?.then_some(path)}))
            }
            methods::FS_PLUGIN_DEPLOYMENT_COMMIT => {
                let params: CommitPluginDeploymentParams = parse(params)?;
                let path = deployment_path(&cache, &params.key)?;
                let parent = path
                    .parent()
                    .expect("deployment path has a plugin directory");
                private_directory(parent)?;
                context.session.run_directories.consume_staging(
                    context,
                    params.directory_id,
                    true,
                    |directory, cancelled| {
                        let staged = directory.join("package");
                        let metadata = fs::symlink_metadata(&staged)
                            .map_err(|error| io_error(error, "staged deployment"))?;
                        require_private_directory(&metadata)?;
                        self.publish(context, &staged, &path, cancelled)?;
                        Ok(json!({"path": path}))
                    },
                )
            }
            methods::FS_MANAGED_RUNTIME_LOOKUP => {
                let layout: ManagedRuntimeLayout = parse(params)?;
                managed_runtime::validate_layout(&layout)?;
                let path = deployment_path(&cache, &layout.key)?;
                if self.acquire(context, &path)? {
                    managed_runtime::published_paths(&path, &layout)
                } else {
                    Ok(Value::Null)
                }
            }
            methods::FS_MANAGED_RUNTIME_COMMIT => {
                let params: CommitManagedRuntimeParams = parse(params)?;
                let layout = params.layout;
                managed_runtime::validate_layout(&layout)?;
                let path = deployment_path(&cache, &layout.key)?;
                private_directory(path.parent().expect("runtime has a plugin directory"))?;
                context.session.run_directories.consume_staging(
                    context,
                    params.directory_id,
                    false,
                    |directory, cancelled| {
                        if self.acquire(context, &path)? {
                            return managed_runtime::published_paths(&path, &layout);
                        }
                        let staged = managed_runtime::prepare(directory, &layout, cancelled)?;
                        self.publish(context, &staged, &path, cancelled)?;
                        managed_runtime::published_paths(&path, &layout)
                    },
                )
            }
            methods::FS_PLUGIN_DEPLOYMENT_SWEEP => {
                let params: SweepPluginDeploymentsParams = parse(params)?;
                let installed = installed_versions(params)?;
                sweep(&cache, &installed)?;
                sweep(&cache_root(self.home.as_deref(), true)?, &installed)?;
                context.session.run_directories.sweep_staging()?;
                Ok(Value::Null)
            }
            _ => Err(RemoteError::new(
                ErrorCode::UnknownMethod,
                "unknown deployment method",
            )),
        }
    }

    /// Atomically publishes an immutable tree or leases the concurrent winner.
    fn publish(
        &self,
        context: &Context,
        staged: &Path,
        path: &Path,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), RemoteError> {
        for _ in 0..2 {
            if cancelled() {
                return Err(RemoteError::new(ErrorCode::NotFound, "staging cancelled"));
            }
            match platform::rename_no_replace(staged, path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(io_error(error, "plugin deployment")),
            }
            if self.acquire(context, path)? {
                return Ok(());
            }
        }
        Err(RemoteError::new(
            ErrorCode::Busy,
            "plugin deployment changed while being published",
        ))
    }

    /// Keeps a cache pathname live from selection through process termination.
    fn acquire(&self, context: &Context, path: &Path) -> Result<bool, RemoteError> {
        let mut leases = lock(&self.leases);
        if context.session.is_ending() {
            return Err(RemoteError::new(ErrorCode::NotFound, "session ended"));
        }
        if leases.contains_key(path) {
            return Ok(true);
        }
        if leases.len() == MAX_LEASES {
            return Err(RemoteError::new(
                ErrorCode::Busy,
                "too many active deployments",
            ));
        }
        let Some(lease) = lock_private_directory(path, libc::LOCK_SH | libc::LOCK_NB)? else {
            return Ok(false);
        };
        leases.insert(path.to_owned(), lease);
        Ok(true)
    }

    /// Releases all deployment leases after terminating the session's processes.
    pub(crate) fn clear(&self) {
        lock(&self.leases).clear();
    }
}

fn cache_root(home: Option<&str>, runtime: bool) -> Result<PathBuf, RemoteError> {
    let home =
        home.ok_or_else(|| RemoteError::new(ErrorCode::Unsupported, "host home unavailable"))?;
    let base = host_path(home)?.join(".lithe-server");
    private_directory(&base)?;
    let cache = base.join(if runtime {
        "managed-runtimes"
    } else {
        "plugin-deployments"
    });
    private_directory(&cache)?;
    Ok(cache)
}

fn plugin_id_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with('.')
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        })
}

fn version_valid(value: &str) -> bool {
    // Reserve room for the separator and the digest in a filesystem component.
    value.len() <= 255 - 1 - 64
        && value.split('.').count() == 3
        && value
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

pub(super) fn hash_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn invalid_identity() -> RemoteError {
    RemoteError::new(
        ErrorCode::InvalidRequest,
        "invalid plugin deployment identity",
    )
}

fn deployment_path(cache: &Path, key: &PluginDeploymentKey) -> Result<PathBuf, RemoteError> {
    if !plugin_id_valid(&key.plugin_id) || !version_valid(&key.version) || !hash_valid(&key.sha256)
    {
        return Err(invalid_identity());
    }
    let plugin = cache.join(&key.plugin_id);
    if let Some(metadata) = metadata_if_present(&plugin)? {
        require_private_directory(&metadata)?;
    }
    Ok(plugin.join(format!("{}-{}", key.version, key.sha256)))
}

fn metadata_if_present(path: &Path) -> Result<Option<fs::Metadata>, RemoteError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(error, "plugin deployment")),
    }
}

fn installed_versions(
    params: SweepPluginDeploymentsParams,
) -> Result<BTreeSet<(String, String)>, RemoteError> {
    if params.installed.len() > MAX_INSTALLED {
        return Err(invalid_identity());
    }
    let mut installed = BTreeSet::new();
    for item in params.installed {
        if !plugin_id_valid(&item.plugin_id) || !version_valid(&item.version) {
            return Err(invalid_identity());
        }
        installed.insert((item.plugin_id, item.version));
    }
    Ok(installed)
}

fn sweep(cache: &Path, installed: &BTreeSet<(String, String)>) -> Result<(), RemoteError> {
    for plugin in fs::read_dir(cache).map_err(|error| io_error(error, "plugin deployments"))? {
        let plugin = plugin.map_err(|error| io_error(error, "plugin deployments"))?;
        let Some(id) = plugin
            .file_name()
            .into_string()
            .ok()
            .filter(|id| plugin_id_valid(id))
        else {
            continue;
        };
        let Some(metadata) = metadata_if_present(&plugin.path())? else {
            continue;
        };
        require_private_directory(&metadata)?;
        sweep_plugin(&plugin.path(), &id, installed)?;
    }
    Ok(())
}

fn sweep_plugin(
    plugin: &Path,
    id: &str,
    installed: &BTreeSet<(String, String)>,
) -> Result<(), RemoteError> {
    for entry in fs::read_dir(plugin).map_err(|error| io_error(error, "plugin deployments"))? {
        let entry = entry.map_err(|error| io_error(error, "plugin deployments"))?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Some((version, hash)) = name.split_once('-') else {
            continue;
        };
        if !version_valid(version)
            || !hash_valid(hash)
            || installed.contains(&(id.to_owned(), version.to_owned()))
        {
            continue;
        }
        let path = entry.path();
        if let Some(_lease) = lock_private_directory(&path, libc::LOCK_EX | libc::LOCK_NB)? {
            fs::remove_dir_all(&path).map_err(|error| io_error(error, "plugin deployment"))?;
        }
    }
    // Leave the ID directory in place: a publisher may be about to add a version.
    Ok(())
}
