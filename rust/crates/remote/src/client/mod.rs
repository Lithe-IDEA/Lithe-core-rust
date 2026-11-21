//! Client side of Lithe remote workspaces.
//!
//! - [`connections`] keeps the SSH connections the user saved and turns each
//!   into an SSH hop with allowlisted options only.
//! - [`transport`] turns a chain of hops (SSH, WSL, container exec) into the
//!   local command line that runs a program at the end of the chain.
//! - [`wsl`] turns a WSL distribution into the first hop and lists the
//!   installed distributions.
//! - [`bootstrap`] probes the platform at the end of the chain and describes
//!   the local server builds that can run there.
//! - [`launch`] probes, installs and starts the server over one connection,
//!   so a host that asks for a password asks only once. The host itself
//!   verifies the installed binary and its directories.
//! - [`session`] starts the server, exchanges protocol messages with it and
//!   routes each resource's events to its subscriber.
//! - [`api`] offers typed operations and resource handles on a session.
//! - [`forward`] forwards local ports through tunnels.
//! - [`forward_address`] reads the addresses typed into the ports page:
//!   ports, `host:port` and URLs.
//! - [`forward_set`] keeps the forwards of one session for the ports page,
//!   confirms targets outside the host and releases them with the session.
//! - [`connection_log`] records one connection's lifecycle in the v2 log,
//!   identified by a random id instead of the host or user name.
//! - [`trust`] keeps the machine-local trust decisions on remote workspaces,
//!   keyed by the target and the host's real path.
//! - [`ssh_config`] reads what OpenSSH makes of a connection (`ssh -G`) and
//!   the `Host` aliases of the user's SSH configuration.
//! - `motty` (feature `motty`) reads the SSH hosts of a Motty configuration
//!   database, read only, for an import.
//! - [`prompt`] tells what an OpenSSH question asks for, so the window can
//!   show a host key, password, passphrase or code form.
//! - [`preferences`] keeps the settings page's machine-local values: the
//!   OpenSSH client to start and how long a lost session is reconnected.
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
pub mod connections;
pub mod forward;
pub mod forward_address;
pub mod forward_set;
pub mod launch;
#[cfg(feature = "motty")]
pub mod motty;
pub mod preferences;
pub mod prompt;
pub mod session;
pub mod ssh_config;
pub mod transport;
pub mod trust;
pub mod wsl;

pub use api::{SpawnSpec, TerminalSpec, WriteOptions};
pub use bootstrap::{ArtifactError, ServerArtifact, ServerTarget};
pub use connection_log::{ConnectionKind, ConnectionLog};
pub use forward::PortForward;
pub use forward_address::{AddressError, ForwardAddress};
pub use forward_set::{ForwardError, ForwardInfo, ForwardSet, ForwardSpec, TargetField};
pub use session::{ClientError, ConnectOptions, Event, RemoteSession, ResumeMode};
pub use transport::{
    ContainerEngine, ContainerHop, Hop, SshHop, TransportChain, TransportError, WslHop,
};
pub use wsl::WslConnection;
