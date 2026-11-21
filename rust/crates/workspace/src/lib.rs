//! Where a Core request's files and tools live: the local machine, or a host
//! reached through a remote development session.
//!
//! A request that names a remote workspace installs its [`Host`] for the
//! request's duration ([`Scope::begin`]); [`fs`] then reads and writes on that
//! host and never falls back to the local placeholder folder, and
//! [`remote_path`] / [`local_path`] translate schema-owned paths between the
//! two namespaces. Without a host every call is the local filesystem.
//!
//! The session side owns the long-lived connections: [`connections`] keeps
//! the SSH connections the user saved, [`ssh_configuration`] tells what
//! OpenSSH makes of them and lists the hosts of the user's SSH configuration,
//! [`motty`] lists the hosts saved in Motty for an import, [`wsl`] lists the
//! WSL distributions of this Windows machine, [`containers`] lists the
//! running containers on this machine or a connected host, [`probe`] checks
//! that a saved SSH connection reaches a usable host, [`sessions`] opens and
//! keeps the connections (over OpenSSH, `wsl.exe` or a container engine's
//! `exec`), [`host`] forwards finite requests
//! and owned streaming resources (processes, terminals, watches),
//! [`forwards`] keeps the port forwards of the ports page, [`upload`] streams
//! a verified file to the host, [`trust`] keys remote workspace trust by
//! what the connection resolves, and [`preferences`] keeps the settings
//! page's values, including the SSH program every OpenSSH start uses.
//! Domain semantics stay in their engines (Git, build, language servers);
//! this crate only supplies the IO boundary.

pub mod connections;
pub mod containers;
pub mod forwards;
pub mod fs;
pub mod host;
pub mod motty;
pub mod preferences;
pub mod probe;
mod scope;
pub mod sessions;
pub mod ssh_configuration;
#[cfg(feature = "test-support")]
pub mod test_support;
pub mod trust;
pub mod upload;
pub mod wsl;

pub use scope::{
    current, enter, identity_key, is_remote, local_path, remote_path, Host, Scope, Workspace,
};
