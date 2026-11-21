//! Client side of Lithe remote workspaces.
//!
//! - [`transport`] turns a chain of hops (SSH, container exec) into the local
//!   command line that runs a program at the end of the chain.
//! - [`bootstrap`] probes the platform at the end of the chain and describes
//!   the local server builds that can run there.
//! - [`launch`] probes, installs and starts the server over one connection,
//!   so a host that asks for a password asks only once. The host itself
//!   verifies the installed binary and its directories.
//! - [`session`] starts the server, exchanges protocol messages with it and
//!   routes each resource's events to its subscriber.
//! - [`api`] offers typed operations and resource handles on a session.
//! - [`forward`] forwards local ports through tunnels.
//! - [`connection_log`] records one connection's lifecycle in the v2 log,
//!   identified by a random id instead of the host or user name.
//! - [`trust`] keeps the machine-local trust decisions on remote workspaces,
//!   keyed by the target and the host's real path.
//!
//! The transport layer deliberately knows nothing about files or processes.
//! Supporting a new kind of remote means adding a hop, not new capabilities.
//!
//! Note: .agents/notes/proposed/architecture/2026-10-24-remote-development-workspace-host.md

pub mod api;
pub mod bootstrap;
#[cfg(all(test, unix))]
mod command;
pub mod connection_log;
pub mod forward;
pub mod launch;
pub mod session;
pub mod transport;
pub mod trust;

pub use api::{SpawnSpec, TerminalSpec, WriteOptions};
pub use bootstrap::{ArtifactError, ServerArtifact, ServerTarget};
pub use connection_log::{ConnectionKind, ConnectionLog};
pub use forward::PortForward;
pub use session::{ClientError, ConnectOptions, Event, RemoteSession, ResumeMode};
pub use transport::{ContainerEngine, ContainerHop, Hop, SshHop, TransportChain, TransportError};
