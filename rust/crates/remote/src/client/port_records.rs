//! Machine-local port records of remote workspaces (design §8, S27, S28).
//!
//! The ports page keeps what the user set by hand when the workspace closes
//! or the connection drops. For each connection and workspace it remembers:
//!
//! - the ports the user added, with the address typed. A URL keeps its scheme
//!   and path, never its query or fragment, which can carry an access token;
//! - a label and the local port the user chose for a row;
//! - the local forwards the user started and did not stop. Only these start
//!   again after a reconnect ([`WorkspaceRecords::restorable`]). One to a
//!   machine other than the host keeps the confirmation the user gave for it
//!   (S28), and stopping the forward forgets that confirmation;
//! - the forwarding rules that listen on the host, host and reverse forwards.
//!   They never start on their own: each start asks again, and listening on
//!   every host interface is never part of a rule (S27).
//!
//! Records are keyed like workspace trust, by the [`Target`] and the
//! workspace's real path as the host resolves it, so another host, account,
//! container or folder never sees them. They are stored only on the local
//! machine, in the document at [`STORAGE_PATH`] below the platform's
//! user-preference directory, and hold no secret. Unlike trust decisions they
//! are a convenience: beyond [`MAX_WORKSPACES`] workspaces, or once the
//! document would outgrow [`MAX_BYTES`], the workspace edited longest ago is
//! dropped instead of refusing the edit.
//!
//! This module validates and merges. The caller resolves the target and the
//! real path over an open session; the platform adapter reads the document
//! and publishes edits atomically.

use crate::client::forward_address;
use crate::client::forward_set::{canonical_target, MAX_FORWARDS};
use crate::client::trust::{host_path, Target};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Document location below the platform's user-preference directory, next to
/// the remote preferences and the saved connections.
pub const STORAGE_PATH: &str = "remote/ports.json";

/// Largest accepted document, in bytes.
pub const MAX_BYTES: usize = 256 * 1024;

/// Workspaces with records; the one edited longest ago makes room.
pub const MAX_WORKSPACES: usize = 64;

/// Port rows of one workspace.
pub const MAX_PORTS: usize = 128;

/// Forwarding rules of one workspace: as many as one connection runs.
pub const MAX_RULES: usize = MAX_FORWARDS;

/// Longest label, in characters.
const MAX_LABEL_CHARS: usize = 64;

/// A port of the host that the ports page lists as one row, and what the
/// user set for it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortRecord {
    /// The address the port is on, as the ports page keys its rows. Absent
    /// for the host's own loopback address, which `localhost`, `127.0.0.1`,
    /// `::1` and the every-interface addresses all name; otherwise an IP
    /// address in canonical form or a name in lower case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// TCP port, 1 to 65535.
    pub port: u16,
    /// The address the user added the row with; absent for a listening port
    /// the user did not add.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<AddedAddress>,
    /// Name the user gave the row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Local port the user chose; absent forwards on the row's own number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_port: Option<u16>,
    /// Target host of the local forward the user started for the row and did
    /// not stop, as `remote.startForward` lists it. It starts again after a
    /// reconnect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward_target: Option<String>,
    /// The user confirmed that forward although its target is outside the
    /// host's loopback interface (S28). Kept only with `forward_target`, and
    /// only for such a target.
    #[serde(default, skip_serializing_if = "is_false")]
    pub confirmed_target: bool,
}

/// An address typed into the ports page, as `remote.parseForwardAddress`
/// read it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddedAddress {
    /// The host read: an IP address, an IPv6 one without brackets, or a name
    /// in lower case; `localhost` for a port alone.
    pub host: String,
    /// URL scheme in lower case, when the address was a URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
    /// The URL's path without its query and fragment; absent when nothing
    /// but `/` remains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// A forwarding rule that listens on the host. Two rules are the same rule
/// when they differ only in their labels.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Rule {
    /// A host port on the host's loopback interface that connects to another
    /// server from the host.
    Host {
        /// Host port; absent lets the host choose one at each start.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        listen_port: Option<u16>,
        /// The server, as `remote.forwards` lists the target.
        target_host: String,
        /// Its TCP port.
        target_port: u16,
        /// Name the user gave the rule.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    /// A host loopback port that reaches a port on this machine's loopback
    /// interface.
    Reverse {
        /// Host port; absent lets the host choose one at each start.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        listen_port: Option<u16>,
        /// The port on this machine's loopback interface.
        local_port: u16,
        /// Name the user gave the rule.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
}

impl Rule {
    /// Whether `other` is the same rule, whatever the labels.
    fn same(&self, other: &Rule) -> bool {
        self.unlabelled() == other.unlabelled()
    }

    fn unlabelled(&self) -> Rule {
        let mut rule = self.clone();
        match &mut rule {
            Rule::Host { label, .. } | Rule::Reverse { label, .. } => *label = None,
        }
        rule
    }
}

/// What the ports page remembers of one workspace.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRecords {
    /// Rows by port, then by host, the loopback row first.
    pub ports: Vec<PortRecord>,
    /// Rules in the order the user added them.
    pub rules: Vec<Rule>,
}

impl WorkspaceRecords {
    /// The local forwards to start again after a reconnect, with the target
    /// host of each.
    pub fn restorable(&self) -> impl Iterator<Item = (&PortRecord, &str)> {
        self.ports.iter().filter_map(|record| {
            record
                .forward_target
                .as_deref()
                .map(|target| (record, target))
        })
    }
}

/// Why a document, a target, a path or a record was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortRecordError {
    /// The offending field, for example `document`, `host` or `added`.
    pub field: &'static str,
}

fn at(field: &'static str) -> PortRecordError {
    PortRecordError { field }
}

impl fmt::Display for PortRecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid remote port record at {}", self.field)
    }
}

impl std::error::Error for PortRecordError {}

/// The validated document: the records of every workspace, the one edited
/// last first.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortRecords {
    version: u32,
    #[serde(default)]
    workspaces: Vec<WorkspaceEntry>,
}

/// The records of one workspace on one target.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspaceEntry {
    target: Target,
    /// Real path of the workspace root on the host.
    workspace: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    ports: Vec<PortRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    rules: Vec<Rule>,
}

impl WorkspaceEntry {
    fn is(&self, target: &Target, workspace: &str) -> bool {
        self.target == *target && self.workspace == workspace
    }

    fn is_empty(&self) -> bool {
        self.ports.is_empty() && self.rules.is_empty()
    }

    /// Sorts the rows and refuses what an edit never records: a malformed
    /// target or path, oversized lists, values an edit would have changed,
    /// and a row or rule listed twice. Rows with nothing to remember are
    /// dropped.
    fn normalize(&mut self) -> Result<(), PortRecordError> {
        self.target.validate().map_err(|_| at("target"))?;
        if host_path(&self.workspace).is_none() {
            return Err(at("workspace"));
        }
        if self.ports.len() > MAX_PORTS
            || self
                .ports
                .iter()
                .any(|record| normalize_port(record.clone()).as_ref() != Ok(record))
        {
            return Err(at("ports"));
        }
        self.ports.retain(|record| !is_blank(record));
        self.ports
            .sort_by(|left, right| row_order(left).cmp(&row_order(right)));
        if self
            .ports
            .windows(2)
            .any(|pair| row_order(&pair[0]) == row_order(&pair[1]))
        {
            return Err(at("ports"));
        }
        if self.rules.len() > MAX_RULES
            || self
                .rules
                .iter()
                .any(|rule| normalize_rule(rule.clone()).as_ref() != Ok(rule))
            || self
                .rules
                .iter()
                .enumerate()
                .any(|(index, rule)| self.rules[..index].iter().any(|other| other.same(rule)))
        {
            return Err(at("rules"));
        }
        Ok(())
    }
}

impl PortRecords {
    /// Reads `document`, or the empty document when there is none yet.
    ///
    /// Refuses documents over [`MAX_BYTES`], malformed JSON, duplicate keys,
    /// unknown fields (`allInterfaces` included), other versions, more than
    /// [`MAX_WORKSPACES`] workspaces, a workspace listed twice, and records an
    /// edit never writes. Workspaces with nothing to remember are dropped.
    pub fn parse(document: Option<&str>) -> Result<Self, PortRecordError> {
        let Some(text) = document else {
            return Ok(Self {
                version: 1,
                workspaces: Vec::new(),
            });
        };
        if text.len() > MAX_BYTES {
            return Err(at("document"));
        }
        let mut records: Self = serde_json::from_str(text).map_err(|_| at("document"))?;
        if records.version != 1 {
            return Err(at("version"));
        }
        if records.workspaces.len() > MAX_WORKSPACES {
            return Err(at("workspaces"));
        }
        for entry in &mut records.workspaces {
            entry.normalize()?;
        }
        records.workspaces.retain(|entry| !entry.is_empty());
        let workspaces = &records.workspaces;
        if workspaces.iter().enumerate().any(|(index, entry)| {
            workspaces[..index]
                .iter()
                .any(|other| other.is(&entry.target, &entry.workspace))
        }) {
            return Err(at("workspaces"));
        }
        Ok(records)
    }

    /// The document text to publish, refused when it would exceed
    /// [`MAX_BYTES`].
    pub fn text(&self) -> Result<String, PortRecordError> {
        let mut text = serde_json::to_string_pretty(self).map_err(|_| at("document"))?;
        text.push('\n');
        if text.len() > MAX_BYTES {
            return Err(at("document"));
        }
        Ok(text)
    }

    /// What the ports page remembers of the workspace at `real_path` on
    /// `target`; nothing when it has no records.
    pub fn records(
        &self,
        target: &Target,
        real_path: &str,
    ) -> Result<WorkspaceRecords, PortRecordError> {
        target.validate().map_err(|_| at("target"))?;
        let workspace = host_path(real_path).ok_or(at("workspace"))?;
        Ok(self
            .workspaces
            .iter()
            .find(|entry| entry.is(target, workspace))
            .map(|entry| WorkspaceRecords {
                ports: entry.ports.clone(),
                rules: entry.rules.clone(),
            })
            .unwrap_or_default())
    }

    /// Records `record` for its row, replacing what the row had. A record
    /// with nothing to remember removes the row.
    pub fn set_port(
        &mut self,
        target: &Target,
        real_path: &str,
        record: PortRecord,
    ) -> Result<(), PortRecordError> {
        let record = normalize_port(record)?;
        self.update(target, real_path, |entry| {
            let position = entry
                .ports
                .binary_search_by(|row| row_order(row).cmp(&row_order(&record)));
            match position {
                Ok(index) if is_blank(&record) => {
                    entry.ports.remove(index);
                }
                Ok(index) => entry.ports[index] = record,
                Err(_) if is_blank(&record) => {}
                Err(_) if entry.ports.len() >= MAX_PORTS => return Err(at("ports")),
                Err(index) => entry.ports.insert(index, record),
            }
            Ok(())
        })
    }

    /// Forgets the row of `port` on `host`, any spelling of the row's host
    /// accepted. Removing an absent row changes nothing.
    pub fn remove_port(
        &mut self,
        target: &Target,
        real_path: &str,
        host: Option<&str>,
        port: u16,
    ) -> Result<(), PortRecordError> {
        let host = match host {
            Some(host) => row_host(host, "host")?,
            None => None,
        };
        self.update(target, real_path, |entry| {
            entry
                .ports
                .retain(|record| record.port != port || record.host != host);
            Ok(())
        })
    }

    /// Records `rule`, replacing the label of the same rule or adding it last.
    pub fn set_rule(
        &mut self,
        target: &Target,
        real_path: &str,
        rule: Rule,
    ) -> Result<(), PortRecordError> {
        let rule = normalize_rule(rule)?;
        self.update(target, real_path, |entry| {
            match entry.rules.iter().position(|other| other.same(&rule)) {
                Some(index) => entry.rules[index] = rule,
                None if entry.rules.len() >= MAX_RULES => return Err(at("rules")),
                None => entry.rules.push(rule),
            }
            Ok(())
        })
    }

    /// Forgets the rule that is the same as `rule`. Removing an absent rule
    /// changes nothing.
    pub fn remove_rule(
        &mut self,
        target: &Target,
        real_path: &str,
        rule: Rule,
    ) -> Result<(), PortRecordError> {
        let rule = normalize_rule(rule)?;
        self.update(target, real_path, |entry| {
            entry.rules.retain(|other| !other.same(&rule));
            Ok(())
        })
    }

    /// Applies `change` to the records of the workspace at `real_path` on
    /// `target`. A change moves the workspace first and removes it once it
    /// has nothing left; the workspaces edited longest ago make room. On an
    /// error, and when nothing changes, the document stays as it was.
    fn update(
        &mut self,
        target: &Target,
        real_path: &str,
        change: impl FnOnce(&mut WorkspaceEntry) -> Result<(), PortRecordError>,
    ) -> Result<(), PortRecordError> {
        target.validate().map_err(|_| at("target"))?;
        let workspace = host_path(real_path).ok_or(at("workspace"))?;
        let position = self
            .workspaces
            .iter()
            .position(|entry| entry.is(target, workspace));
        let before = match position {
            Some(index) => self.workspaces[index].clone(),
            None => WorkspaceEntry {
                target: target.clone(),
                workspace: workspace.into(),
                ports: Vec::new(),
                rules: Vec::new(),
            },
        };
        let mut after = before.clone();
        change(&mut after)?;
        if after == before {
            return Ok(());
        }
        if let Some(index) = position {
            self.workspaces.remove(index);
        }
        if !after.is_empty() {
            self.workspaces.insert(0, after);
            self.workspaces.truncate(MAX_WORKSPACES);
            while self.workspaces.len() > 1 && self.text().is_err() {
                self.workspaces.pop();
            }
        }
        Ok(())
    }
}

/// The row key of a port: `None` for the host's own loopback address, which
/// `localhost`, `127.0.0.1`, `::1` and the every-interface addresses name;
/// otherwise the host as a forward reaches it. Refused, at `field`, for a
/// host no forward can target.
fn row_host(host: &str, field: &'static str) -> Result<Option<String>, PortRecordError> {
    let (canonical, _) = canonical_target(host).ok_or(at(field))?;
    Ok((!matches!(canonical.as_str(), "127.0.0.1" | "::1" | "localhost")).then_some(canonical))
}

/// The row order: by port, then by host, the loopback row first.
fn row_order(record: &PortRecord) -> (u16, Option<&str>) {
    (record.port, record.host.as_deref())
}

fn is_blank(record: &PortRecord) -> bool {
    record.added.is_none()
        && record.label.is_none()
        && record.local_port.is_none()
        && record.forward_target.is_none()
}

/// `record` as it is stored: the row's host as its key, the added address
/// without query or fragment, the forward's target as `remote.forwards`
/// lists it, and a confirmation only where one applies.
fn normalize_port(mut record: PortRecord) -> Result<PortRecord, PortRecordError> {
    if record.port == 0 {
        return Err(at("port"));
    }
    record.host = match record.host.as_deref() {
        Some(host) => row_host(host, "host")?,
        None => None,
    };
    if let Some(added) = record.added.take() {
        record.added = Some(normalize_added(added, record.port, record.host.as_deref())?);
    }
    if let Some(label) = &record.label {
        validate_label(label)?;
    }
    if record.local_port == Some(0) {
        return Err(at("localPort"));
    }
    if let Some(target) = record.forward_target.take() {
        let (canonical, _) = canonical_target(&target).ok_or(at("forwardTarget"))?;
        if row_host(&canonical, "forwardTarget")? != record.host {
            return Err(at("forwardTarget"));
        }
        record.forward_target = Some(canonical);
    }
    let loopback = record
        .host
        .as_deref()
        .is_none_or(|host| canonical_target(host).is_some_and(|(_, loopback)| loopback));
    record.confirmed_target &= record.forward_target.is_some() && !loopback;
    Ok(record)
}

/// `added` as `remote.parseForwardAddress` reads it back for `port`, on the
/// row `row`, without the query and the fragment of its path: those can
/// carry an access token, such as Jupyter's `?token=`, and stay in the page's
/// memory only.
fn normalize_added(
    mut added: AddedAddress,
    port: u16,
    row: Option<&str>,
) -> Result<AddedAddress, PortRecordError> {
    added.path = added.path.and_then(|path| {
        let path = path.split(['?', '#']).next().unwrap_or_default();
        (!path.is_empty() && path != "/").then(|| path.to_owned())
    });
    let host = if added.host.contains(':') {
        format!("[{}]", added.host)
    } else {
        added.host.clone()
    };
    let path = added.path.as_deref().unwrap_or_default();
    let text = match &added.scheme {
        Some(scheme) => format!("{scheme}://{host}:{port}{path}"),
        None => format!("{host}:{port}{path}"),
    };
    let read = forward_address::parse(&text).map_err(|_| at("added"))?;
    let same = read.target_host == added.host
        && read.target_port == port
        && read.scheme == added.scheme
        && read.path == added.path;
    if !same || row_host(&added.host, "added")?.as_deref() != row {
        return Err(at("added"));
    }
    Ok(added)
}

/// `rule` as it is stored: its target as `remote.forwards` lists it, and no
/// listening port for one the host chooses.
fn normalize_rule(rule: Rule) -> Result<Rule, PortRecordError> {
    let listen = |port: Option<u16>| port.filter(|port| *port != 0);
    let label = |label: Option<String>| match label {
        Some(label) => validate_label(&label).map(|()| Some(label)),
        None => Ok(None),
    };
    match rule {
        Rule::Host {
            listen_port,
            target_host,
            target_port,
            label: name,
        } => {
            let (target_host, _) = canonical_target(&target_host).ok_or(at("targetHost"))?;
            if target_port == 0 {
                return Err(at("targetPort"));
            }
            Ok(Rule::Host {
                listen_port: listen(listen_port),
                target_host,
                target_port,
                label: label(name)?,
            })
        }
        Rule::Reverse {
            listen_port,
            local_port,
            label: name,
        } => {
            if local_port == 0 {
                return Err(at("localPort"));
            }
            Ok(Rule::Reverse {
                listen_port: listen(listen_port),
                local_port,
                label: label(name)?,
            })
        }
    }
}

fn validate_label(label: &str) -> Result<(), PortRecordError> {
    let valid = !label.is_empty()
        && label.chars().count() <= MAX_LABEL_CHARS
        && !label.chars().any(char::is_control);
    if valid {
        Ok(())
    } else {
        Err(at("label"))
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests;
