//! Where a Core request's files and tools live: the local machine, or a host
//! reached through a remote development session.
//!
//! A request that names a remote workspace installs its [`Host`] for the
//! request's duration ([`Scope::begin`]); [`fs`] then reads and writes on that
//! host and never falls back to the local placeholder folder, and
//! [`remote_path`] / [`local_path`] translate schema-owned paths between the
//! two namespaces. Without a host every call is the local filesystem.
//!
//! The session side owns the long-lived connections: [`probe`] checks that a
//! saved SSH connection reaches a usable host, [`sessions`] opens and keeps the
//! connections, [`host`] forwards finite requests and owned streaming resources
//! (processes, terminals, watches), and [`upload`] streams a verified file to
//! the host. Domain semantics stay in their engines (Git, build, language
//! servers); this crate only supplies the IO boundary.

pub mod fs;
pub mod host;
pub mod probe;
mod scope;
pub mod sessions;
#[cfg(feature = "test-support")]
pub mod test_support;
pub mod upload;

pub use scope::{
    current, enter, identity_key, is_remote, local_path, remote_path, Host, Scope, Workspace,
};
