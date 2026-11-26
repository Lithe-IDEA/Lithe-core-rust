//! Machine-local trust of remote workspaces (design S01 to S04).
//!
//! A remote workspace is trusted, kept in safe mode, or not decided yet, as a
//! local one is, but a decision never carries over to another place:
//!
//! - It is keyed by the [`Target`], the hops that reach the host and the
//!   numeric account the server runs as, together with the workspace's real
//!   path as the host resolves it (`fs.realPath`). Local workspaces, other
//!   hosts, other accounts and other ports never inherit it, and a symbolic
//!   link opens the decision of the folder it points to.
//! - It is stored only on the local machine, in the document at
//!   [`STORAGE_PATH`] below the platform's user-preference directory. The host
//!   keeps no marker, because project files and other users can change it.
//! - A trusted location covers the workspaces below it on its own target only,
//!   and a host's root folder is never offered as one.
//! - Nothing trusts every remote location at once.
//!
//! This module validates, decides and merges. The caller resolves the target
//! and the real path over an open session, so a key is never a path the host
//! did not resolve. The platform adapter reads the document and publishes
//! edits atomically.

use crate::client::transport::{is_wsl_distribution, is_wsl_user, Hop, TransportChain};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Document location below the platform's user-preference directory, next to
/// the trust of local workspaces in `toolchains/global.json`.
pub const STORAGE_PATH: &str = "toolchains/remote-trust.json";

/// Largest accepted document, in bytes.
pub const MAX_BYTES: usize = 256 * 1024;

/// Targets with decisions.
const MAX_TARGETS: usize = 64;

/// Entries of each list of one target, as for local workspaces.
const MAX_ENTRIES: usize = 256;

/// Hops of one target.
const MAX_HOPS: usize = 8;

/// Longest hop field, in bytes.
const MAX_FIELD_BYTES: usize = 1024;

/// Longest connection name kept for display, in characters.
const MAX_LABEL_CHARS: usize = 128;

/// Longest host path, in bytes: Linux `PATH_MAX`.
const MAX_PATH_BYTES: usize = 4096;

/// Where a remote workspace lives, apart from its path.
///
/// Two workspaces share decisions only when every field is equal, compared
/// exactly. Spellings that may reach the same account differently, such as
/// `dev@box` and `box` with `User=dev`, ask again instead of guessing.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Target {
    /// The hops that choose the host, outermost first.
    pub hops: Vec<TargetHop>,
    /// Numeric account the server runs as, from `server.hello`. It tells
    /// apart accounts that the SSH configuration chooses without the hop
    /// naming them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<u32>,
    /// Actual authenticated target digest, or a connection-local digest when
    /// reusable evidence is unavailable. Absent only in legacy records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
}

/// The fields of one hop that choose where it ends. Keys, timeouts,
/// keep-alive, working directories and environment do not, so changing them
/// keeps the decisions.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum TargetHop {
    /// An SSH hop.
    Ssh(SshTarget),
    /// An `exec` into a container.
    Container(ContainerTarget),
    /// A distribution of the Windows Subsystem for Linux.
    Wsl(WslTarget),
}

/// SSH hop fields that choose the host, the account and the route.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SshTarget {
    /// Destination as the hop gives it: `host`, `user@host` or an alias.
    pub destination: String,
    /// Port passed as `-p`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Value of the first `Port` option; ssh uses the first value of an option.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port_option: Option<String>,
    /// Value of the first `User` option.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Value of the first `ProxyJump` option.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_jump: Option<String>,
}

/// Container hop fields that choose the engine, the container and the account.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContainerTarget {
    /// `docker` or `podman`.
    pub engine: String,
    /// Engine CLI executable. Docker-compatible CLIs reach different daemons.
    pub program: String,
    /// Docker context choosing the daemon; absent in legacy targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Container name or id.
    pub container: String,
    /// `user`, `uid` or `user:group` given to `exec`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

/// WSL hop fields that choose the distribution and the account. The program
/// is always the system's `wsl.exe`, so it is not part of the key.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WslTarget {
    /// Distribution name as `wsl.exe --list` prints it.
    pub distribution: String,
    /// Linux account passed as `--user`; absent for the distribution's
    /// default user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

impl Target {
    /// The target of a session opened through `chain` to a server running as
    /// the numeric account `user_id`.
    pub fn new(chain: &TransportChain, user_id: Option<u32>) -> Self {
        let hops = chain
            .hops
            .iter()
            .map(|hop| match hop {
                Hop::Ssh(hop) => {
                    let option = |name: &str| {
                        hop.options
                            .iter()
                            .find(|(key, _)| key.eq_ignore_ascii_case(name))
                            .map(|(_, value)| value.clone())
                    };
                    TargetHop::Ssh(SshTarget {
                        destination: hop.destination.clone(),
                        port: hop.port,
                        port_option: option("Port"),
                        user: option("User"),
                        proxy_jump: option("ProxyJump"),
                    })
                }
                Hop::Container(hop) => TargetHop::Container(ContainerTarget {
                    engine: hop.engine.name().into(),
                    program: hop.program.clone(),
                    context: hop.context.clone(),
                    container: hop.container.clone(),
                    user: hop.user.clone(),
                }),
                Hop::Wsl(hop) => TargetHop::Wsl(WslTarget {
                    distribution: hop.distribution.clone(),
                    user: hop.user.clone(),
                }),
            })
            .collect();
        Self {
            hops,
            user_id,
            identity: None,
        }
    }

    /// Refuses targets no transport chain produces: too many hops, empty,
    /// oversized or control characters in a field, a destination ssh would
    /// read as an option, an unknown engine, or a WSL hop that is not first
    /// or names a distribution or user `wsl.exe` would not be given. Port
    /// records refuse the same targets.
    pub(crate) fn validate(&self) -> Result<(), TrustError> {
        let field = |value: &str| {
            !value.is_empty()
                && value.len() <= MAX_FIELD_BYTES
                && !value.chars().any(char::is_control)
        };
        let optional = |value: &Option<String>| value.as_deref().is_none_or(field);
        let valid = self
            .identity
            .as_deref()
            .is_none_or(crate::protocol::methods::is_sha256_hex)
            && self.hops.len() <= MAX_HOPS
            && self.hops.iter().enumerate().all(|(index, hop)| match hop {
                TargetHop::Ssh(hop) => {
                    field(&hop.destination)
                        && !hop.destination.starts_with('-')
                        && optional(&hop.port_option)
                        && optional(&hop.user)
                        && optional(&hop.proxy_jump)
                }
                TargetHop::Container(hop) => {
                    matches!(hop.engine.as_str(), "docker" | "podman")
                        && field(&hop.program)
                        && hop.context.as_deref().is_none_or(|name| {
                            hop.engine == "docker" && super::container::is_context_name(name)
                        })
                        && field(&hop.container)
                        && optional(&hop.user)
                }
                TargetHop::Wsl(hop) => {
                    index == 0
                        && is_wsl_distribution(&hop.distribution)
                        && hop.user.as_deref().is_none_or(is_wsl_user)
                }
            });
        if valid {
            Ok(())
        } else {
            Err(TrustError::at("target"))
        }
    }
}

/// A workspace's trust: `trusted` lets project code run, `distrusted` is a
/// recorded choice of safe mode, and `unknown` means the user was not asked.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TrustState {
    /// Project code may run.
    Trusted,
    /// The user chose safe mode for this workspace.
    Distrusted,
    /// No decision applies; the workspace stays in safe mode.
    Unknown,
}

/// The trust of one remote workspace.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTrust {
    /// The decision that applies.
    pub state: TrustState,
    /// The real path the decision is keyed by.
    pub workspace: String,
    /// The trusted location that trusts the workspace, when the state comes
    /// from one.
    pub location: Option<String>,
    /// The folder a trust prompt may offer to trust with the workspace: its
    /// parent, unless that is the host's root.
    pub offered_location: Option<String>,
}

/// One explicit decision on a workspace.
#[derive(Clone, Copy, Debug)]
pub struct Decision<'a> {
    /// `true` lets project code run; `false` records safe mode.
    pub trusted: bool,
    /// Also trusts the offered location. Refused with `trusted: false` and for
    /// a workspace whose parent is the host's root.
    pub location: bool,
    /// Connection name shown with the target's decisions; the previous name
    /// stays when absent. Never part of the key.
    pub label: Option<&'a str>,
}

/// One list of a target's decisions.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum List {
    /// Workspaces whose project code may run.
    TrustedWorkspaces,
    /// Workspaces kept in safe mode.
    DistrustedWorkspaces,
    /// Folders whose workspaces, at any depth, are trusted unless distrusted
    /// themselves.
    TrustedLocations,
}

/// Why a document, a target, a path or a decision was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrustError {
    /// The offending field, for example `document`, `workspace` or `location`.
    pub field: &'static str,
}

impl TrustError {
    fn at(field: &'static str) -> Self {
        Self { field }
    }
}

impl fmt::Display for TrustError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid remote workspace trust at {}",
            self.field
        )
    }
}

impl std::error::Error for TrustError {}

/// The validated document: the decisions of every target, sorted by target.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Decisions {
    version: u32,
    #[serde(default)]
    targets: Vec<TargetDecisions>,
}

/// Decisions on the workspaces of one target. Each list is sorted and holds
/// distinct real paths.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TargetDecisions {
    target: Target,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    trusted_workspaces: Vec<String>,
    /// An explicit decision on a workspace wins over a location containing it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    distrusted_workspaces: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    trusted_locations: Vec<String>,
}

impl TargetDecisions {
    fn new(target: Target) -> Self {
        Self {
            target,
            label: None,
            trusted_workspaces: Vec::new(),
            distrusted_workspaces: Vec::new(),
            trusted_locations: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.trusted_workspaces.is_empty()
            && self.distrusted_workspaces.is_empty()
            && self.trusted_locations.is_empty()
    }

    fn list_mut(&mut self, list: List) -> &mut Vec<String> {
        match list {
            List::TrustedWorkspaces => &mut self.trusted_workspaces,
            List::DistrustedWorkspaces => &mut self.distrusted_workspaces,
            List::TrustedLocations => &mut self.trusted_locations,
        }
    }

    /// Sorts the lists and refuses what a decision never records: malformed
    /// or duplicate paths, oversized lists, the root as a location, and a
    /// workspace that is both trusted and distrusted.
    fn normalize(&mut self) -> Result<(), TrustError> {
        self.target.validate()?;
        if let Some(label) = &self.label {
            validate_label(label)?;
        }
        for (list, field) in [
            (&mut self.trusted_workspaces, "trustedWorkspaces"),
            (&mut self.distrusted_workspaces, "distrustedWorkspaces"),
            (&mut self.trusted_locations, "trustedLocations"),
        ] {
            list.sort();
            let malformed = list.iter().any(|path| host_path(path).is_none());
            let duplicate = list.windows(2).any(|pair| pair[0] == pair[1]);
            if list.len() > MAX_ENTRIES || malformed || duplicate {
                return Err(TrustError::at(field));
            }
        }
        if self
            .trusted_locations
            .iter()
            .any(|location| location == "/")
        {
            return Err(TrustError::at("trustedLocations"));
        }
        if self
            .trusted_workspaces
            .iter()
            .any(|workspace| contains(&self.distrusted_workspaces, workspace))
        {
            return Err(TrustError::at("distrustedWorkspaces"));
        }
        Ok(())
    }

    fn within_bounds(&self) -> Result<(), TrustError> {
        for (list, field) in [
            (&self.trusted_workspaces, "trustedWorkspaces"),
            (&self.distrusted_workspaces, "distrustedWorkspaces"),
            (&self.trusted_locations, "trustedLocations"),
        ] {
            if list.len() > MAX_ENTRIES {
                return Err(TrustError::at(field));
            }
        }
        Ok(())
    }
}

impl Decisions {
    /// Reads `document`, or the empty document when there is none yet.
    ///
    /// Refuses documents over [`MAX_BYTES`], malformed JSON, duplicate keys,
    /// unknown fields, other versions, more than 64 targets, a target listed
    /// twice, and lists a decision never records. Targets without decisions
    /// are dropped.
    pub fn parse(document: Option<&str>) -> Result<Self, TrustError> {
        let Some(text) = document else {
            return Ok(Self {
                version: 1,
                targets: Vec::new(),
            });
        };
        if text.len() > MAX_BYTES {
            return Err(TrustError::at("document"));
        }
        let mut decisions: Self =
            serde_json::from_str(text).map_err(|_| TrustError::at("document"))?;
        if decisions.version != 1 {
            return Err(TrustError::at("version"));
        }
        if decisions.targets.len() > MAX_TARGETS {
            return Err(TrustError::at("targets"));
        }
        for entry in &mut decisions.targets {
            entry.normalize()?;
        }
        decisions.targets.retain(|entry| !entry.is_empty());
        decisions
            .targets
            .sort_by(|left, right| left.target.cmp(&right.target));
        if decisions
            .targets
            .windows(2)
            .any(|pair| pair[0].target == pair[1].target)
        {
            return Err(TrustError::at("targets"));
        }
        Ok(decisions)
    }

    /// The document text to publish, refused when it would exceed
    /// [`MAX_BYTES`].
    pub fn text(&self) -> Result<String, TrustError> {
        let mut text =
            serde_json::to_string_pretty(self).map_err(|_| TrustError::at("document"))?;
        text.push('\n');
        if text.len() > MAX_BYTES {
            return Err(TrustError::at("document"));
        }
        Ok(text)
    }

    /// The trust of the workspace at `real_path` on `target`, as IntelliJ IDEA
    /// orders it: an explicit decision on the workspace first, then a trusted
    /// location of the same target containing it.
    pub fn workspace(
        &self,
        target: &Target,
        real_path: &str,
    ) -> Result<WorkspaceTrust, TrustError> {
        let workspace = host_path(real_path).ok_or(TrustError::at("workspace"))?;
        let (state, location) = match self.entry(target) {
            Some(entry) if contains(&entry.trusted_workspaces, workspace) => {
                (TrustState::Trusted, None)
            }
            Some(entry) if contains(&entry.distrusted_workspaces, workspace) => {
                (TrustState::Distrusted, None)
            }
            Some(entry) => match entry
                .trusted_locations
                .iter()
                .find(|location| within(workspace, location))
            {
                Some(location) => (TrustState::Trusted, Some(location.clone())),
                None => (TrustState::Unknown, None),
            },
            None => (TrustState::Unknown, None),
        };
        Ok(WorkspaceTrust {
            state,
            workspace: workspace.into(),
            location,
            offered_location: offered_location(workspace),
        })
    }

    /// Records `decision` on the workspace at `real_path` on `target`,
    /// keeping every other decision. Trusting moves the workspace out of the
    /// distrusted list and safe mode moves it out of the trusted one. On an
    /// error nothing changes.
    pub fn decide(
        &mut self,
        target: &Target,
        real_path: &str,
        decision: Decision<'_>,
    ) -> Result<(), TrustError> {
        target.validate()?;
        let workspace = host_path(real_path)
            .ok_or(TrustError::at("workspace"))?
            .to_owned();
        if let Some(label) = decision.label {
            validate_label(label)?;
        }
        let offered = if decision.location {
            if !decision.trusted {
                return Err(TrustError::at("location"));
            }
            Some(offered_location(&workspace).ok_or(TrustError::at("location"))?)
        } else {
            None
        };
        let position = self
            .targets
            .binary_search_by(|entry| entry.target.cmp(target));
        let mut entry = match position {
            Ok(index) => self.targets[index].clone(),
            Err(_) if self.targets.len() >= MAX_TARGETS => {
                return Err(TrustError::at("targets"));
            }
            Err(_) => TargetDecisions::new(target.clone()),
        };
        if decision.trusted {
            remove(&mut entry.distrusted_workspaces, &workspace);
            insert(&mut entry.trusted_workspaces, workspace);
        } else {
            remove(&mut entry.trusted_workspaces, &workspace);
            insert(&mut entry.distrusted_workspaces, workspace);
        }
        if let Some(offered) = offered {
            insert(&mut entry.trusted_locations, offered);
        }
        if let Some(label) = decision.label {
            entry.label = Some(label.into());
        }
        entry.within_bounds()?;
        match position {
            Ok(index) => self.targets[index] = entry,
            Err(index) => self.targets.insert(index, entry),
        }
        Ok(())
    }

    /// Removes `path` from one list of `target`'s decisions, and the target
    /// once it has none left. Removing an absent entry changes nothing.
    pub fn revoke(&mut self, target: &Target, list: List, path: &str) -> Result<(), TrustError> {
        target.validate()?;
        let path = host_path(path).ok_or(TrustError::at("path"))?;
        if let Ok(index) = self
            .targets
            .binary_search_by(|entry| entry.target.cmp(target))
        {
            let entry = &mut self.targets[index];
            remove(entry.list_mut(list), path);
            if entry.is_empty() {
                self.targets.remove(index);
            }
        }
        Ok(())
    }

    fn entry(&self, target: &Target) -> Option<&TargetDecisions> {
        self.targets
            .binary_search_by(|entry| entry.target.cmp(target))
            .ok()
            .map(|index| &self.targets[index])
    }
}

/// `path` when it is an absolute, normalized host path, the form of an
/// `fs.realPath` result: it starts with `/`, has no empty, `.` or `..`
/// component, ends without `/` unless it is `/`, contains no NUL and is at
/// most 4096 bytes long.
pub fn host_path(path: &str) -> Option<&str> {
    let normalized = path.len() <= MAX_PATH_BYTES
        && !path.contains('\0')
        && (path == "/"
            || path.strip_prefix('/').is_some_and(|rest| {
                rest.split('/')
                    .all(|component| !component.is_empty() && component != "." && component != "..")
            }));
    normalized.then_some(path)
}

/// Whether `workspace` is `location` or below it.
fn within(workspace: &str, location: &str) -> bool {
    workspace
        .strip_prefix(location)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// The parent of `workspace`, unless that is the host's root, which would
/// trust every project on the host.
fn offered_location(workspace: &str) -> Option<String> {
    let (parent, _) = workspace.rsplit_once('/')?;
    (!parent.is_empty()).then(|| parent.to_owned())
}

fn validate_label(label: &str) -> Result<(), TrustError> {
    let valid = !label.is_empty()
        && label.chars().count() <= MAX_LABEL_CHARS
        && !label.chars().any(char::is_control);
    if valid {
        Ok(())
    } else {
        Err(TrustError::at("label"))
    }
}

fn contains(list: &[String], path: &str) -> bool {
    list.binary_search_by(|entry| entry.as_str().cmp(path))
        .is_ok()
}

fn insert(list: &mut Vec<String>, path: String) {
    if let Err(index) = list.binary_search(&path) {
        list.insert(index, path);
    }
}

fn remove(list: &mut Vec<String>, path: &str) {
    if let Ok(index) = list.binary_search_by(|entry| entry.as_str().cmp(path)) {
        list.remove(index);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::transport::{ContainerEngine, ContainerHop, SshHop, WslHop};
    use serde_json::json;

    fn ssh(destination: &str) -> SshHop {
        SshHop::new(destination)
    }

    fn target(hops: Vec<Hop>, user_id: Option<u32>) -> Target {
        Target::new(&TransportChain::new(hops), user_id)
    }

    fn devbox() -> Target {
        target(vec![Hop::Ssh(ssh("dev@box"))], Some(1000))
    }

    #[test]
    fn authenticated_targets_do_not_inherit_alias_only_decisions() {
        let legacy = devbox();
        let mut decisions = Decisions::parse(None).unwrap();
        trust(&mut decisions, &legacy, "/srv/app", false);
        let bound = |identity: &str| {
            let mut value = serde_json::to_value(&legacy).unwrap();
            value["identity"] = json!(identity);
            serde_json::from_value::<Target>(value).unwrap()
        };
        let first = bound(&"a".repeat(64));
        let rebound = bound(&"b".repeat(64));
        assert_eq!(state(&decisions, &first, "/srv/app"), TrustState::Unknown);
        trust(&mut decisions, &first, "/srv/app", false);
        assert_eq!(state(&decisions, &first, "/srv/app"), TrustState::Trusted);
        assert_eq!(state(&decisions, &rebound, "/srv/app"), TrustState::Unknown);
        let reloaded = Decisions::parse(Some(&decisions.text().unwrap())).unwrap();
        assert_eq!(state(&reloaded, &first, "/srv/app"), TrustState::Trusted);
        assert_eq!(state(&reloaded, &rebound, "/srv/app"), TrustState::Unknown);
    }

    fn trust(decisions: &mut Decisions, target: &Target, path: &str, location: bool) {
        decisions
            .decide(
                target,
                path,
                Decision {
                    trusted: true,
                    location,
                    label: Some("Dev box"),
                },
            )
            .unwrap();
    }

    fn state(decisions: &Decisions, target: &Target, path: &str) -> TrustState {
        decisions.workspace(target, path).unwrap().state
    }

    #[test]
    fn decisions_never_carry_over_between_hosts_accounts_ports_or_containers() {
        let mut decisions = Decisions::parse(None).unwrap();
        trust(&mut decisions, &devbox(), "/home/dev/app", false);
        assert_eq!(
            state(&decisions, &devbox(), "/home/dev/app"),
            TrustState::Trusted
        );

        let with_port = SshHop {
            port: Some(2222),
            ..ssh("dev@box")
        };
        let option = |name: &str, value: &str| SshHop {
            options: vec![(name.into(), value.into())],
            ..ssh("dev@box")
        };
        let others = [
            target(vec![Hop::Ssh(ssh("dev@other"))], Some(1000)),
            target(vec![Hop::Ssh(ssh("ops@box"))], Some(1000)),
            target(vec![Hop::Ssh(ssh("box"))], Some(1000)),
            target(vec![Hop::Ssh(with_port)], Some(1000)),
            target(vec![Hop::Ssh(option("Port", "2222"))], Some(1000)),
            target(vec![Hop::Ssh(option("User", "root"))], Some(1000)),
            target(vec![Hop::Ssh(option("ProxyJump", "bastion"))], Some(1000)),
            target(vec![Hop::Ssh(ssh("dev@box"))], Some(0)),
            target(vec![Hop::Ssh(ssh("dev@box"))], None),
            target(
                vec![
                    Hop::Ssh(ssh("dev@box")),
                    Hop::Container(ContainerHop::new(ContainerEngine::Docker, "api")),
                ],
                Some(1000),
            ),
            target(Vec::new(), Some(1000)),
        ];
        for other in &others {
            assert_ne!(other, &devbox());
            assert_eq!(
                state(&decisions, other, "/home/dev/app"),
                TrustState::Unknown,
                "{other:?}"
            );
        }
        assert_eq!(
            state(&decisions, &devbox(), "/home/dev/other"),
            TrustState::Unknown
        );
    }

    #[test]
    fn targets_keep_only_the_fields_that_choose_where_a_hop_ends() {
        let hop = SshHop {
            program: "/opt/ssh".into(),
            port: Some(22),
            options: vec![
                ("ConnectTimeout".into(), "5".into()),
                ("user".into(), "dev".into()),
                ("User".into(), "root".into()),
                ("IdentityFile".into(), "~/.ssh/id".into()),
            ],
            ..ssh("box")
        };
        let mut container = ContainerHop::new(ContainerEngine::Podman, "api");
        container.user = Some("1000:1000".into());
        container.working_directory = Some("/srv".into());
        container
            .environment
            .insert("TOKEN".into(), "secret".into());
        let built = target(vec![Hop::Ssh(hop), Hop::Container(container)], Some(7));
        assert_eq!(
            serde_json::to_value(&built).unwrap(),
            json!({
                "hops": [
                    {"ssh": {"destination": "box", "port": 22, "user": "dev"}},
                    {"container": {"engine": "podman", "program": "podman", "container": "api", "user": "1000:1000"}}
                ],
                "userId": 7
            })
        );

        let keep_alive = SshHop {
            options: vec![("ServerAliveInterval".into(), "30".into())],
            ..ssh("dev@box")
        };
        assert_eq!(target(vec![Hop::Ssh(keep_alive)], Some(1000)), devbox());
    }

    #[test]
    fn wsl_targets_name_the_distribution_and_account_and_come_first_only() {
        let wsl = |distribution: &str, user: Option<&str>| {
            let mut hop = WslHop::new(distribution);
            hop.program = r"C:\Windows\System32\wsl.exe".into();
            hop.user = user.map(Into::into);
            Hop::Wsl(hop)
        };
        let ubuntu = target(vec![wsl("Ubuntu", Some("dev"))], Some(1000));
        assert_eq!(
            serde_json::to_value(&ubuntu).unwrap(),
            json!({"hops": [{"wsl": {"distribution": "Ubuntu", "user": "dev"}}], "userId": 1000})
        );
        let mut decisions = Decisions::parse(None).unwrap();
        trust(&mut decisions, &ubuntu, "/home/dev/app", false);
        let reparsed = Decisions::parse(Some(&decisions.text().unwrap())).unwrap();
        assert_eq!(
            state(&reparsed, &ubuntu, "/home/dev/app"),
            TrustState::Trusted
        );
        // The program path is where wsl.exe lives, not where the hop ends.
        let mut moved = WslHop::new("Ubuntu");
        moved.user = Some("dev".into());
        assert_eq!(target(vec![Hop::Wsl(moved)], Some(1000)), ubuntu);
        for other in [
            target(vec![wsl("Ubuntu", None)], Some(1000)),
            target(vec![wsl("Ubuntu", Some("root"))], Some(1000)),
            target(vec![wsl("Debian", Some("dev"))], Some(1000)),
            target(vec![Hop::Ssh(ssh("dev@Ubuntu"))], Some(1000)),
            target(
                vec![
                    wsl("Ubuntu", Some("dev")),
                    Hop::Container(ContainerHop::new(ContainerEngine::Docker, "api")),
                ],
                Some(1000),
            ),
        ] {
            assert_ne!(other, ubuntu);
            assert_eq!(
                state(&reparsed, &other, "/home/dev/app"),
                TrustState::Unknown,
                "{other:?}"
            );
        }

        let entry = |hops: serde_json::Value| {
            json!({"version": 1, "targets": [{"target": {"hops": hops}, "trustedWorkspaces": ["/a"]}]})
                .to_string()
        };
        for hops in [
            json!([{"ssh": {"destination": "box"}}, {"wsl": {"distribution": "Ubuntu"}}]),
            json!([{"wsl": {"distribution": "Ubuntu"}}, {"wsl": {"distribution": "Debian"}}]),
            json!([{"wsl": {"distribution": "-d"}}]),
            json!([{"wsl": {"distribution": "Ubuntu", "user": "root;id"}}]),
        ] {
            assert_eq!(
                Decisions::parse(Some(&entry(hops.clone()))),
                Err(TrustError::at("target")),
                "{hops}"
            );
        }
        // Unknown fields are refused while reading, like any other.
        assert_eq!(
            Decisions::parse(Some(&entry(json!([
                {"wsl": {"distribution": "Ubuntu", "program": "wsl.exe"}}
            ])))),
            Err(TrustError::at("document"))
        );
    }

    #[test]
    fn a_trusted_location_covers_its_own_target_only_and_never_the_host_root() {
        let mut decisions = Decisions::parse(None).unwrap();
        trust(&mut decisions, &devbox(), "/srv/projects/app", true);
        let deep = decisions
            .workspace(&devbox(), "/srv/projects/other/service")
            .unwrap();
        assert_eq!(deep.state, TrustState::Trusted);
        assert_eq!(deep.location.as_deref(), Some("/srv/projects"));
        assert_eq!(
            deep.offered_location.as_deref(),
            Some("/srv/projects/other")
        );
        assert_eq!(
            state(&decisions, &devbox(), "/srv/projects-old/app"),
            TrustState::Unknown
        );
        assert_eq!(state(&decisions, &devbox(), "/srv"), TrustState::Unknown);
        let elsewhere = target(vec![Hop::Ssh(ssh("dev@other"))], Some(1000));
        assert_eq!(
            state(&decisions, &elsewhere, "/srv/projects/app"),
            TrustState::Unknown
        );

        let top = decisions.workspace(&devbox(), "/srv").unwrap();
        assert_eq!(top.offered_location, None);
        assert_eq!(
            decisions
                .workspace(&devbox(), "/")
                .unwrap()
                .offered_location,
            None
        );
        let before = decisions.clone();
        for (path, trusted) in [("/srv", true), ("/srv/projects/app", false)] {
            let refused = decisions.decide(
                &devbox(),
                path,
                Decision {
                    trusted,
                    location: true,
                    label: None,
                },
            );
            assert_eq!(refused, Err(TrustError::at("location")), "{path}");
        }
        assert_eq!(decisions, before);
    }

    #[test]
    fn an_explicit_decision_wins_over_a_location_and_moves_between_lists() {
        let mut decisions = Decisions::parse(None).unwrap();
        trust(&mut decisions, &devbox(), "/srv/projects/app", true);
        let keep_safe = Decision {
            trusted: false,
            location: false,
            label: None,
        };
        decisions
            .decide(&devbox(), "/srv/projects/untrusted", keep_safe)
            .unwrap();
        assert_eq!(
            state(&decisions, &devbox(), "/srv/projects/untrusted"),
            TrustState::Distrusted
        );
        decisions
            .decide(&devbox(), "/srv/projects/app", keep_safe)
            .unwrap();
        assert_eq!(
            state(&decisions, &devbox(), "/srv/projects/app"),
            TrustState::Distrusted
        );
        trust(&mut decisions, &devbox(), "/srv/projects/untrusted", false);
        let text = decisions.text().unwrap();
        let document: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            document["targets"][0]["trustedWorkspaces"],
            json!(["/srv/projects/untrusted"])
        );
        assert_eq!(
            document["targets"][0]["distrustedWorkspaces"],
            json!(["/srv/projects/app"])
        );
        assert_eq!(document["targets"][0]["label"], "Dev box");
        assert_eq!(Decisions::parse(Some(&text)).unwrap(), decisions);
    }

    #[test]
    fn revoking_removes_entries_and_empty_targets_and_ignores_absent_ones() {
        let mut decisions = Decisions::parse(None).unwrap();
        trust(&mut decisions, &devbox(), "/srv/projects/app", true);
        decisions
            .revoke(&devbox(), List::TrustedLocations, "/srv/projects")
            .unwrap();
        assert_eq!(
            state(&decisions, &devbox(), "/srv/projects/other"),
            TrustState::Unknown
        );
        let unchanged = decisions.clone();
        decisions
            .revoke(&devbox(), List::DistrustedWorkspaces, "/srv/projects/app")
            .unwrap();
        let elsewhere = target(vec![Hop::Ssh(ssh("dev@other"))], Some(1000));
        decisions
            .revoke(&elsewhere, List::TrustedWorkspaces, "/srv/projects/app")
            .unwrap();
        assert_eq!(decisions, unchanged);
        decisions
            .revoke(&devbox(), List::TrustedWorkspaces, "/srv/projects/app")
            .unwrap();
        assert_eq!(decisions, Decisions::parse(None).unwrap());
        assert_eq!(
            decisions.revoke(&devbox(), List::TrustedWorkspaces, "srv"),
            Err(TrustError::at("path"))
        );
    }

    #[test]
    fn documents_refuse_ambiguous_malformed_and_oversized_content() {
        let entry = |extra: serde_json::Value| {
            let mut entry =
                json!({"target": {"hops": [{"ssh": {"destination": "dev@box"}}], "userId": 1000}});
            for (key, value) in extra.as_object().unwrap() {
                entry[key] = value.clone();
            }
            entry
        };
        let document =
            |targets: Vec<serde_json::Value>| json!({"version": 1, "targets": targets}).to_string();
        let refused = [
            ("{\"version\":1,\"version\":1}".to_owned(), "document"),
            (
                json!({"version": 1, "trustAll": true}).to_string(),
                "document",
            ),
            (json!({"version": 2}).to_string(), "version"),
            (
                document(vec![entry(json!({"trustedWorkspaces": ["/a/b", "/a/b"]}))]),
                "trustedWorkspaces",
            ),
            (
                document(vec![entry(json!({"trustedWorkspaces": ["a/b"]}))]),
                "trustedWorkspaces",
            ),
            (
                document(vec![entry(json!({"trustedWorkspaces": ["/a/../b"]}))]),
                "trustedWorkspaces",
            ),
            (
                document(vec![entry(json!({"trustedWorkspaces": ["/a/"]}))]),
                "trustedWorkspaces",
            ),
            (
                document(vec![entry(json!({"trustedLocations": ["/"]}))]),
                "trustedLocations",
            ),
            (
                document(vec![entry(
                    json!({"trustedWorkspaces": ["/a"], "distrustedWorkspaces": ["/a"]}),
                )]),
                "distrustedWorkspaces",
            ),
            (
                document(vec![entry(
                    json!({"label": "Dev\nbox", "trustedWorkspaces": ["/a"]}),
                )]),
                "label",
            ),
            (
                document(vec![
                    entry(json!({"trustedWorkspaces": ["/a"]})),
                    entry(json!({"trustedWorkspaces": ["/b"]})),
                ]),
                "targets",
            ),
            (
                document(vec![
                    json!({"target": {"hops": [{"ssh": {"destination": "-oProxyCommand=x"}}]}, "trustedWorkspaces": ["/a"]}),
                ]),
                "target",
            ),
            (
                document(vec![
                    json!({"target": {"hops": [{"container": {"engine": "lxc", "program": "lxc", "container": "a"}}]}, "trustedWorkspaces": ["/a"]}),
                ]),
                "target",
            ),
            (
                format!(
                    "{{\"version\":1,\"targets\":[],\"x\":\"{}\"}}",
                    "a".repeat(MAX_BYTES)
                ),
                "document",
            ),
        ];
        for (text, field) in refused {
            assert_eq!(
                Decisions::parse(Some(&text)),
                Err(TrustError::at(field)),
                "{}",
                &text[..text.len().min(200)]
            );
        }

        let unsorted = document(vec![
            entry(json!({"trustedWorkspaces": ["/b", "/a"]})),
            json!({"target": {"hops": []}, "label": "Unused"}),
        ]);
        let decisions = Decisions::parse(Some(&unsorted)).unwrap();
        let normalized: serde_json::Value =
            serde_json::from_str(&decisions.text().unwrap()).unwrap();
        assert_eq!(normalized["targets"].as_array().unwrap().len(), 1);
        assert_eq!(
            normalized["targets"][0]["trustedWorkspaces"],
            json!(["/a", "/b"])
        );
    }

    #[test]
    fn lists_and_targets_stay_within_their_bounds() {
        let mut decisions = Decisions::parse(None).unwrap();
        for index in 0..MAX_ENTRIES {
            trust(&mut decisions, &devbox(), &format!("/w/{index}"), false);
        }
        let full = decisions.clone();
        let decision = Decision {
            trusted: true,
            location: false,
            label: None,
        };
        assert_eq!(
            decisions.decide(&devbox(), "/w/extra", decision),
            Err(TrustError::at("trustedWorkspaces"))
        );
        assert_eq!(decisions, full);

        let mut decisions = Decisions::parse(None).unwrap();
        for index in 0..MAX_TARGETS {
            let host = target(vec![Hop::Ssh(ssh(&format!("dev@host{index}")))], Some(1000));
            decisions.decide(&host, "/w", decision).unwrap();
        }
        let extra = target(vec![Hop::Ssh(ssh("dev@extra"))], Some(1000));
        assert_eq!(
            decisions.decide(&extra, "/w", decision),
            Err(TrustError::at("targets"))
        );
        assert_eq!(
            decisions.decide(
                &devbox(),
                "/w",
                Decision {
                    label: Some(&"x".repeat(MAX_LABEL_CHARS + 1)),
                    ..decision
                }
            ),
            Err(TrustError::at("label"))
        );
    }

    #[test]
    fn host_paths_must_be_absolute_and_normalized() {
        for path in ["/", "/home/dev", "/srv/my project", "/a/.hidden"] {
            assert_eq!(host_path(path), Some(path));
        }
        let long = format!("/{}", "a".repeat(MAX_PATH_BYTES));
        for path in [
            "",
            "home",
            "//srv",
            "/srv/",
            "/srv//a",
            "/srv/./a",
            "/srv/..",
            "/a\0b",
            long.as_str(),
        ] {
            assert_eq!(host_path(path), None, "{path:?}");
        }
    }
}
