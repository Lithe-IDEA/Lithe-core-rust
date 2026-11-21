//! Saved remote connections (design §3.1, S08 to S10, S12 and S15).
//!
//! The connection window lists the SSH connections the user saved, and every
//! request that reaches a host takes the [`SshConnection`] of one of them. This
//! module owns both: which fields a connection has, how they become an
//! [`SshHop`], and the machine-local document that keeps the list.
//!
//! - A connection carries only what selects the target and the
//!   authentication. Passwords, passphrases and one-time codes never appear in
//!   it (S12); a private key is a path OpenSSH reads, never its contents (S15).
//! - Every field becomes an allowlisted SSH option (S08). Options that run a
//!   local command, such as `ProxyCommand`, stay in the user's own
//!   `~/.ssh/config`.
//! - The document lives at [`STORAGE_PATH`] below the platform's
//!   user-preference directory, never on a host or in a workspace. This module
//!   validates and edits it; the platform adapter reads it and publishes edits
//!   atomically.
//!
//! - An entry may carry a [`Label`]: an accent color and a group. Neither
//!   reaches SSH; [`Connections::sections`] turns the groups into the
//!   headings the window lists the entries under.
//!
//! Every refusal names the field and the reason, so a client can explain each
//! one in the user's language next to the field it concerns.

use crate::client::transport::{is_jumps, is_ssh_user, Hop, SshHop, TransportChain};
use crate::client::trust::host_path;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Document location below the platform's user-preference directory.
pub const STORAGE_PATH: &str = "remote/connections.json";

/// Largest accepted document, in bytes.
pub const MAX_BYTES: usize = 256 * 1024;

/// Seconds OpenSSH waits for the TCP connection when the connection sets none.
pub const DEFAULT_CONNECT_TIMEOUT_SECONDS: u32 = 15;

/// Saved connections.
const MAX_CONNECTIONS: usize = 128;

/// Longest connection name, in characters.
const MAX_NAME_CHARS: usize = 128;

/// Longest group name, in characters.
const MAX_GROUP_CHARS: usize = 64;

/// Longest host, user, private key path or jump host list, in bytes.
const MAX_FIELD_BYTES: usize = 1024;

/// Longest connection id, in bytes.
const MAX_ID_BYTES: usize = 64;

/// Ids drawn for a new entry before giving up. A random 128-bit id never
/// needs a second draw; the bound only stops a broken generator.
const ID_ATTEMPTS: usize = 16;

/// Folders remembered per connection, most recent first.
const MAX_RECENT_FOLDERS: usize = 10;

/// Longest connect timeout. OpenSSH accepts more, but a host that has not
/// answered in ten minutes is unreachable for a user waiting on a window.
const MAX_CONNECT_TIMEOUT_SECONDS: u32 = 600;

/// Longest keep-alive interval.
const MAX_KEEP_ALIVE_SECONDS: u32 = 3600;

/// How the connection proves the user's identity. Every method still lets
/// OpenSSH ask follow-up questions, such as a one-time code.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Authentication {
    /// OpenSSH's own order: keys and agent first, then interactive methods.
    #[default]
    Auto,
    /// Keys offered by the running SSH agent or the SSH configuration.
    Agent,
    /// Only the private key named by `identityFile`.
    PrivateKey,
    /// Password or keyboard-interactive prompts, without trying keys.
    Password,
}

impl Authentication {
    fn is_auto(&self) -> bool {
        *self == Self::Auto
    }
}

/// Accent color the window marks an entry with. Clients pick the shade for
/// their light and dark themes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Color {
    /// Red.
    Red,
    /// Orange.
    Orange,
    /// Yellow.
    Yellow,
    /// Green.
    Green,
    /// Blue.
    Blue,
    /// Purple.
    Purple,
}

/// How the window marks and groups an entry. `None` leaves the entry
/// without a color or outside every group.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Label {
    /// Accent color of the entry.
    pub color: Option<Color>,
    /// Group the entry is listed under. Trimmed before it is saved; a blank
    /// group is no group.
    pub group: Option<String>,
}

/// The entries listed under one heading, in the user's order.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Section<'a> {
    /// The group's name; `None` for the entries without a group.
    pub group: Option<&'a str>,
    /// Ids of the entries in the section.
    pub ids: Vec<&'a str>,
}

/// One SSH connection: the host, the account and how to authenticate.
/// Names stay in [`SavedConnection`]; secrets are never part of it.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SshConnection {
    /// Host name, address, or a `Host` alias from the user's SSH configuration.
    host: String,
    /// `None` keeps the port from the SSH configuration (22 by default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    /// `None` keeps the user from the SSH configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    user: Option<String>,
    #[serde(default, skip_serializing_if = "Authentication::is_auto")]
    authentication: Authentication,
    /// Private key path; required for, and only kept with,
    /// [`Authentication::PrivateKey`]. OpenSSH expands a leading `~` itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    identity_file: Option<String>,
    /// `ProxyJump`: `none`, or comma-separated `[user@]host[:port]` jump
    /// hosts. `None` keeps the jump hosts of the SSH configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    proxy_jump: Option<String>,
    /// `None` uses [`DEFAULT_CONNECT_TIMEOUT_SECONDS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    connect_timeout_seconds: Option<u32>,
    /// `None` keeps the transport's keep-alive default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    keep_alive_seconds: Option<u32>,
}

/// Why a connection, a document or an edit was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionError {
    /// The offending field, for example `host`, `name` or `document`.
    pub field: &'static str,
    /// Why the field was refused.
    pub reason: Reason,
}

/// The kind of refusal, stable for clients to translate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    /// The field is empty but required.
    Required,
    /// The value has characters or a form the field does not accept, or is
    /// out of range.
    Invalid,
    /// The value is longer than the field allows.
    TooLong,
    /// Another saved connection already uses the value.
    Duplicate,
    /// The list already holds as many entries as it may.
    Full,
    /// No saved connection has the id.
    Unknown,
}

impl ConnectionError {
    fn at(field: &'static str, reason: Reason) -> Self {
        Self { field, reason }
    }
}

impl fmt::Display for ConnectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid remote connection at {} ({:?})",
            self.field, self.reason
        )
    }
}

impl std::error::Error for ConnectionError {}

impl SshConnection {
    /// The connection as it is saved and used: text fields trimmed, empty
    /// optional fields removed, and the private key path dropped unless the
    /// connection authenticates with it.
    pub fn normalized(mut self) -> Self {
        fn trimmed(value: Option<String>) -> Option<String> {
            value
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty())
        }
        self.host = self.host.trim().to_owned();
        self.user = trimmed(self.user);
        self.identity_file = match self.authentication {
            Authentication::PrivateKey => trimmed(self.identity_file),
            _ => None,
        };
        self.proxy_jump = trimmed(self.proxy_jump);
        self
    }

    /// Checks every field of a normalized connection.
    pub fn check(&self) -> Result<(), ConnectionError> {
        use Reason::*;
        let host = &self.host;
        if host.is_empty() {
            return Err(ConnectionError::at("host", Required));
        }
        if host.len() > MAX_FIELD_BYTES {
            return Err(ConnectionError::at("host", TooLong));
        }
        // Host names, addresses (IPv6 with an optional zone) and SSH aliases.
        // No `@`, since the user has its own field, no `/`, so an `ssh://`
        // URI cannot smuggle in a user or port, and no leading `-`, which ssh
        // would read as an option.
        if host.starts_with('-')
            || !host
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "._-:%".contains(character))
        {
            return Err(ConnectionError::at("host", Invalid));
        }
        if self.port == Some(0) {
            return Err(ConnectionError::at("port", Invalid));
        }
        if let Some(user) = &self.user {
            if user.len() > MAX_FIELD_BYTES {
                return Err(ConnectionError::at("user", TooLong));
            }
            if !is_ssh_user(user) {
                return Err(ConnectionError::at("user", Invalid));
            }
        }
        match (&self.authentication, &self.identity_file) {
            (Authentication::PrivateKey, None) => {
                return Err(ConnectionError::at("identityFile", Required))
            }
            (Authentication::PrivateKey, Some(path)) => {
                if path.len() > MAX_FIELD_BYTES {
                    return Err(ConnectionError::at("identityFile", TooLong));
                }
                if path.trim() != path || path.chars().any(char::is_control) {
                    return Err(ConnectionError::at("identityFile", Invalid));
                }
            }
            (_, Some(_)) => return Err(ConnectionError::at("identityFile", Invalid)),
            (_, None) => {}
        }
        if let Some(jumps) = &self.proxy_jump {
            if jumps.len() > MAX_FIELD_BYTES {
                return Err(ConnectionError::at("proxyJump", TooLong));
            }
            if !is_jumps(jumps) {
                return Err(ConnectionError::at("proxyJump", Invalid));
            }
        }
        if self
            .connect_timeout_seconds
            .is_some_and(|seconds| !(1..=MAX_CONNECT_TIMEOUT_SECONDS).contains(&seconds))
        {
            return Err(ConnectionError::at("connectTimeoutSeconds", Invalid));
        }
        if self
            .keep_alive_seconds
            .is_some_and(|seconds| !(1..=MAX_KEEP_ALIVE_SECONDS).contains(&seconds))
        {
            return Err(ConnectionError::at("keepAliveSeconds", Invalid));
        }
        Ok(())
    }

    /// The SSH hop of a normalized connection, refused before any process
    /// starts when a field is malformed.
    pub fn hop(&self) -> Result<SshHop, ConnectionError> {
        self.check()?;
        let destination = match &self.user {
            Some(user) => format!("{user}@{}", self.host),
            None => self.host.clone(),
        };
        let mut hop = SshHop::new(destination);
        hop.port = self.port;
        let mut option = |key: &str, value: String| hop.options.push((key.into(), value));
        option(
            "ConnectTimeout",
            self.connect_timeout_seconds
                .unwrap_or(DEFAULT_CONNECT_TIMEOUT_SECONDS)
                .to_string(),
        );
        // OpenSSH keeps the first value, so the connection's interval comes
        // before the transport's default.
        if let Some(seconds) = self.keep_alive_seconds {
            option("ServerAliveInterval", seconds.to_string());
        }
        // Prompts must reach AskPass even when the user's configuration sets
        // BatchMode for scripts.
        option("BatchMode", "no".into());
        if let Some(jumps) = &self.proxy_jump {
            option("ProxyJump", jumps.clone());
        }
        match self.authentication {
            Authentication::Auto => {}
            Authentication::Agent => option("PreferredAuthentications", "publickey".into()),
            Authentication::PrivateKey => {
                let Some(identity) = self.identity_file.as_deref() else {
                    return Err(ConnectionError::at("identityFile", Reason::Required));
                };
                // ssh splits an option value on whitespace, so a path such as
                // "~/My Keys/id_rsa" must be quoted (ssh_config quoting rules).
                let identity = if identity
                    .chars()
                    .any(|c| c.is_whitespace() || c == '"' || c == '\\')
                {
                    format!(
                        "\"{}\"",
                        identity.replace('\\', "\\\\").replace('"', "\\\"")
                    )
                } else {
                    identity.to_owned()
                };
                option("IdentityFile", identity);
                // Without this, keys from the agent are tried first and can
                // use up the server's attempt limit before the chosen key.
                option("IdentitiesOnly", "yes".into());
                option("PreferredAuthentications", "publickey".into());
            }
            Authentication::Password => {
                option(
                    "PreferredAuthentications",
                    "keyboard-interactive,password".into(),
                );
                option("PubkeyAuthentication", "no".into());
            }
        }
        // The transport applies its own allowlist when it builds a command;
        // the fields above must never produce a hop it refuses.
        TransportChain::new(vec![Hop::Ssh(hop.clone())])
            .command(&["true".to_owned()])
            .map_err(|_| ConnectionError::at("host", Reason::Invalid))?;
        Ok(hop)
    }

    /// Whether `other` reaches another host or account: remembered folders
    /// belong to the target, not to the name.
    fn same_target(&self, other: &Self) -> bool {
        self.host == other.host
            && self.port == other.port
            && self.user == other.user
            && self.proxy_jump == other.proxy_jump
    }
}

/// One saved connection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedConnection {
    /// Stable id Core chose when the connection was first saved.
    id: String,
    /// Name the user gave, unique among the saved connections.
    name: String,
    connection: SshConnection,
    /// Accent color the window marks the entry with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    color: Option<Color>,
    /// Group the entry is listed under. Groups compare regardless of case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    group: Option<String>,
    /// Host folders opened over this connection, most recent first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    recent_folders: Vec<String>,
}

impl SavedConnection {
    fn check(&self) -> Result<(), ConnectionError> {
        check_id(&self.id)?;
        check_name(&self.name)?;
        if let Some(group) = &self.group {
            check_group(group)?;
        }
        self.connection.check()?;
        if self.recent_folders.len() > MAX_RECENT_FOLDERS {
            return Err(ConnectionError::at("recentFolders", Reason::Full));
        }
        for (index, folder) in self.recent_folders.iter().enumerate() {
            if host_path(folder).is_none() {
                return Err(ConnectionError::at("recentFolders", Reason::Invalid));
            }
            if self.recent_folders[..index].contains(folder) {
                return Err(ConnectionError::at("recentFolders", Reason::Duplicate));
            }
        }
        Ok(())
    }
}

/// The validated document: saved connections in the user's order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Connections {
    version: u32,
    #[serde(default)]
    connections: Vec<SavedConnection>,
}

impl Default for Connections {
    fn default() -> Self {
        Self {
            version: 1,
            connections: Vec::new(),
        }
    }
}

impl Connections {
    /// Reads `document`, or the empty document when there is none yet.
    ///
    /// Refuses documents over [`MAX_BYTES`], malformed JSON, duplicate keys,
    /// unknown fields, other versions, more than 128 connections, malformed
    /// entries, and ids or names used twice.
    pub fn parse(document: Option<&str>) -> Result<Self, ConnectionError> {
        let Some(text) = document else {
            return Ok(Self::default());
        };
        if text.len() > MAX_BYTES {
            return Err(ConnectionError::at("document", Reason::TooLong));
        }
        let connections: Self = serde_json::from_str(text)
            .map_err(|_| ConnectionError::at("document", Reason::Invalid))?;
        if connections.version != 1 {
            return Err(ConnectionError::at("version", Reason::Invalid));
        }
        if connections.connections.len() > MAX_CONNECTIONS {
            return Err(ConnectionError::at("connections", Reason::Full));
        }
        for (index, entry) in connections.connections.iter().enumerate() {
            entry.check()?;
            let earlier = &connections.connections[..index];
            if earlier.iter().any(|other| other.id == entry.id) {
                return Err(ConnectionError::at("id", Reason::Duplicate));
            }
            if earlier
                .iter()
                .any(|other| same_name(&other.name, &entry.name))
            {
                return Err(ConnectionError::at("name", Reason::Duplicate));
            }
        }
        Ok(connections)
    }

    /// The document text to publish, refused when it would exceed
    /// [`MAX_BYTES`].
    pub fn text(&self) -> Result<String, ConnectionError> {
        let mut text = serde_json::to_string_pretty(self)
            .map_err(|_| ConnectionError::at("document", Reason::Invalid))?;
        text.push('\n');
        if text.len() > MAX_BYTES {
            return Err(ConnectionError::at("document", Reason::TooLong));
        }
        Ok(text)
    }

    /// Saved connections in the user's order.
    pub fn entries(&self) -> &[SavedConnection] {
        &self.connections
    }

    /// The saved connection with `id`.
    fn get(&self, id: &str) -> Option<&SavedConnection> {
        self.connections.iter().find(|entry| entry.id == id)
    }

    /// The entries by group: one section per group, in the order the groups
    /// first appear, then the entries without a group. An empty list has no
    /// sections. Groups that differ only in case share the section, which
    /// shows the first entry's spelling.
    pub fn sections(&self) -> Vec<Section<'_>> {
        let mut sections: Vec<Section<'_>> = Vec::new();
        let mut ungrouped = Vec::new();
        for entry in &self.connections {
            let Some(group) = entry.group.as_deref() else {
                ungrouped.push(entry.id.as_str());
                continue;
            };
            match sections
                .iter_mut()
                .find(|section| section.group.is_some_and(|name| same_name(name, group)))
            {
                Some(section) => section.ids.push(&entry.id),
                None => sections.push(Section {
                    group: Some(group),
                    ids: vec![&entry.id],
                }),
            }
        }
        if !ungrouped.is_empty() {
            sections.push(Section {
                group: None,
                ids: ungrouped,
            });
        }
        sections
    }

    /// Saves `connection` under `name` with `label`, replacing the entry
    /// `id` or, without one, appending a new entry with an id from `new_id`.
    /// Returns the id.
    ///
    /// Renaming keeps the remembered folders; pointing the entry at another
    /// host, port, user or jump host forgets them. A group that another entry
    /// already uses with other letter case takes that entry's spelling, so
    /// the window shows one heading. `new_id` is called until it returns an
    /// unused valid id, at most 16 times.
    pub fn save(
        &mut self,
        id: Option<&str>,
        name: &str,
        connection: SshConnection,
        label: Label,
        mut new_id: impl FnMut() -> String,
    ) -> Result<String, ConnectionError> {
        let name = name.trim();
        check_name(name)?;
        let group = match label.group.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(group) => {
                check_group(group)?;
                let spelling = self
                    .connections
                    .iter()
                    .filter(|other| Some(other.id.as_str()) != id)
                    .filter_map(|other| other.group.as_deref())
                    .find(|other| same_name(other, group))
                    .unwrap_or(group);
                Some(spelling.to_owned())
            }
        };
        let connection = connection.normalized();
        connection.check()?;
        if self
            .connections
            .iter()
            .any(|other| Some(other.id.as_str()) != id && same_name(&other.name, name))
        {
            return Err(ConnectionError::at("name", Reason::Duplicate));
        }
        match id {
            Some(id) => {
                let entry = self
                    .connections
                    .iter_mut()
                    .find(|entry| entry.id == id)
                    .ok_or(ConnectionError::at("id", Reason::Unknown))?;
                if !entry.connection.same_target(&connection) {
                    entry.recent_folders.clear();
                }
                entry.name = name.to_owned();
                entry.connection = connection;
                entry.color = label.color;
                entry.group = group;
                Ok(entry.id.clone())
            }
            None => {
                if self.connections.len() >= MAX_CONNECTIONS {
                    return Err(ConnectionError::at("connections", Reason::Full));
                }
                let id = (0..ID_ATTEMPTS)
                    .map(|_| new_id())
                    .find(|candidate| check_id(candidate).is_ok() && self.get(candidate).is_none())
                    .ok_or(ConnectionError::at("id", Reason::Invalid))?;
                self.connections.push(SavedConnection {
                    id: id.clone(),
                    name: name.to_owned(),
                    connection,
                    color: label.color,
                    group,
                    recent_folders: Vec::new(),
                });
                Ok(id)
            }
        }
    }

    /// Removes the entry `id`; an absent id changes nothing.
    pub fn remove(&mut self, id: &str) {
        self.connections.retain(|entry| entry.id != id);
    }

    /// Puts the host folder `path` first among the entry's recent folders,
    /// keeping at most ten.
    pub fn remember_folder(&mut self, id: &str, path: &str) -> Result<(), ConnectionError> {
        let path = host_path(path).ok_or(ConnectionError::at("path", Reason::Invalid))?;
        let entry = self
            .connections
            .iter_mut()
            .find(|entry| entry.id == id)
            .ok_or(ConnectionError::at("id", Reason::Unknown))?;
        entry.recent_folders.retain(|folder| folder != path);
        entry.recent_folders.insert(0, path.to_owned());
        entry.recent_folders.truncate(MAX_RECENT_FOLDERS);
        Ok(())
    }
}

/// Letters, digits and `-`, at most 64 bytes.
fn check_id(id: &str) -> Result<(), ConnectionError> {
    let valid = !id.is_empty()
        && id.len() <= MAX_ID_BYTES
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-');
    if valid {
        Ok(())
    } else {
        Err(ConnectionError::at("id", Reason::Invalid))
    }
}

/// A trimmed name of at most 128 characters without control characters.
fn check_name(name: &str) -> Result<(), ConnectionError> {
    if name.is_empty() {
        return Err(ConnectionError::at("name", Reason::Required));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(ConnectionError::at("name", Reason::TooLong));
    }
    if name.trim() != name || name.chars().any(char::is_control) {
        return Err(ConnectionError::at("name", Reason::Invalid));
    }
    Ok(())
}

/// A trimmed group of at most 64 characters without control characters.
fn check_group(group: &str) -> Result<(), ConnectionError> {
    if group.is_empty() {
        return Err(ConnectionError::at("group", Reason::Required));
    }
    if group.chars().count() > MAX_GROUP_CHARS {
        return Err(ConnectionError::at("group", Reason::TooLong));
    }
    if group.trim() != group || group.chars().any(char::is_control) {
        return Err(ConnectionError::at("group", Reason::Invalid));
    }
    Ok(())
}

/// Names that differ only in letter case would look the same in a title.
fn same_name(left: &str, right: &str) -> bool {
    left.to_lowercase() == right.to_lowercase()
}

#[cfg(test)]
mod tests;
