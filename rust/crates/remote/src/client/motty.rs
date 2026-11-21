//! SSH hosts of a Motty configuration database, for an import (#2231).
//!
//! Motty keeps its hosts, groups and jump routes in a SQLite database
//! (`config.db`): table `records(kind, id, stamp, deleted, payload)` with a
//! JSON payload `{"o": order, "d": data}` per record, and `meta.format`
//! naming the layout. Passwords and keys live in a separate vault, which
//! this module never opens.
//!
//! [`read`] opens the database read only and refuses layouts other than
//! [`SUPPORTED_FORMAT`]. [`candidates`] turns each host into a connection
//! the import can save and lists, field by field, what is imported and why
//! the rest is not, so the window shows the user exactly what changes.
//! Nothing is written to Motty's files.

use crate::client::connections::{Authentication, Color, Connections, SshConnection};
use serde::Serialize;
use serde_json::{Map, Value};
use std::path::Path;
use std::time::Duration;

/// The only database layout read.
pub const SUPPORTED_FORMAT: &str = "28";

/// Largest database read, in bytes.
const MAX_DATABASE_BYTES: u64 = 64 * 1024 * 1024;

/// Records read in total.
const MAX_RECORDS: usize = 4096;

/// Largest record payload read, in bytes.
const MAX_PAYLOAD_BYTES: usize = 256 * 1024;

/// Jump hosts followed for one host.
const MAX_JUMPS: usize = 8;

/// How long a read waits while Motty writes the database.
const BUSY_TIMEOUT: Duration = Duration::from_millis(500);

/// Why the database could not be read.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ReadError {
    /// There is no database at the path.
    Missing,
    /// The database could not be opened or queried.
    Unreadable,
    /// The database is larger than 64 MiB or holds more than 4096 records.
    TooLarge,
    /// The layout is not [`SUPPORTED_FORMAT`].
    UnsupportedFormat,
}

/// One record that is not deleted.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    /// `host`, `group` or `proxy`.
    pub kind: String,
    /// Motty's id of the record.
    pub id: String,
    /// Motty's order of the record among its kind.
    pub order: i64,
    /// The record's `d` object.
    pub data: Map<String, Value>,
}

/// The records of a database.
#[derive(Clone, Debug, PartialEq)]
pub struct Catalog {
    /// `meta.format` of the database.
    pub format: String,
    /// Hosts, groups and jump routes, in Motty's order.
    pub records: Vec<Record>,
}

/// Reads the hosts, groups and jump routes of the database at `path`.
pub fn read(path: &Path) -> Result<Catalog, ReadError> {
    let metadata = std::fs::metadata(path).map_err(|_| ReadError::Missing)?;
    if !metadata.is_file() {
        return Err(ReadError::Missing);
    }
    if metadata.len() > MAX_DATABASE_BYTES {
        return Err(ReadError::TooLarge);
    }
    let database = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| ReadError::Unreadable)?;
    database
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(|_| ReadError::Unreadable)?;
    let format: String = database
        .query_row("SELECT value FROM meta WHERE key = 'format'", [], |row| {
            row.get(0)
        })
        .map_err(|_| ReadError::Unreadable)?;
    if format != SUPPORTED_FORMAT {
        return Err(ReadError::UnsupportedFormat);
    }
    let mut statement = database
        .prepare(
            "SELECT kind, id, payload FROM records \
             WHERE deleted = 0 AND kind IN ('host', 'group', 'proxy') \
             ORDER BY kind, id LIMIT ?1",
        )
        .map_err(|_| ReadError::Unreadable)?;
    let mut rows = statement
        .query([MAX_RECORDS as i64 + 1])
        .map_err(|_| ReadError::Unreadable)?;
    let mut records = Vec::new();
    while let Some(row) = rows.next().map_err(|_| ReadError::Unreadable)? {
        if records.len() == MAX_RECORDS {
            return Err(ReadError::TooLarge);
        }
        let kind: String = row.get(0).map_err(|_| ReadError::Unreadable)?;
        let id: String = row.get(1).map_err(|_| ReadError::Unreadable)?;
        let payload = match row.get_ref(2).map_err(|_| ReadError::Unreadable)? {
            rusqlite::types::ValueRef::Blob(bytes) | rusqlite::types::ValueRef::Text(bytes) => {
                bytes
            }
            _ => continue,
        };
        if payload.len() > MAX_PAYLOAD_BYTES {
            continue;
        }
        // A record Lithe cannot read is left out, as if Motty had none.
        let Ok(Value::Object(mut payload)) = serde_json::from_slice::<Value>(payload) else {
            continue;
        };
        let Some(Value::Object(data)) = payload.remove("d") else {
            continue;
        };
        let order = payload.get("o").and_then(Value::as_i64).unwrap_or(i64::MAX);
        records.push(Record {
            kind,
            id,
            order,
            data,
        });
    }
    records.sort_by(|left, right| {
        (left.kind.as_str(), left.order, left.id.as_str()).cmp(&(
            right.kind.as_str(),
            right.order,
            right.id.as_str(),
        ))
    });
    Ok(Catalog { format, records })
}

/// What an import does with a host.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    /// Every setting is imported.
    Importable,
    /// The connection is imported without some settings.
    Partial,
    /// A saved connection probably reaches the same host already.
    Exists,
    /// The host is not an SSH host, or Lithe refuses its address.
    Unsupported,
}

/// Why a Motty setting is not imported.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Skip {
    /// A password or key stored in Motty's vault, which Lithe never reads.
    Secret,
    /// Lithe has no such setting.
    NoSetting,
    /// The host uses another protocol than SSH.
    Protocol,
    /// The jump route runs a command, which connections may not.
    Command,
    /// Kerberos is left to the SSH configuration.
    Kerberos,
    /// Only the first key file is imported.
    ExtraKey,
    /// Lithe refuses the value.
    Invalid,
}

/// How one Motty setting maps to the connection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    /// Motty's name of the setting.
    pub motty: String,
    /// The value as the window shows it.
    pub value: String,
    /// The connection field it becomes; `None` when it is not imported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lithe: Option<&'static str>,
    /// Why it is not imported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip: Option<Skip>,
}

/// One Motty host and what an import makes of it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    /// Motty's id of the host, for choosing hosts.
    pub key: String,
    /// The host's label, the name the import saves it under.
    pub name: String,
    /// `[user@]host[:port]`, as the window shows it.
    pub address: String,
    /// Motty's operating system tag, for the icon.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// Color, mapped to the nearest of Lithe's colors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
    /// Group name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// What the import does with it.
    pub status: Status,
    /// The saved connection that probably reaches the same host.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub existing_id: Option<String>,
    /// The connection the import saves; `None` when it is unsupported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<SshConnection>,
    /// Every setting the host has, imported or not.
    pub fields: Vec<Field>,
}

/// The hosts of a database and what an import makes of them.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// `meta.format` of the database.
    pub format: String,
    /// Groups that have at least one host, in Motty's order.
    pub groups: Vec<String>,
    /// Hosts in Motty's order.
    pub candidates: Vec<Candidate>,
}

/// What an import makes of every host of `catalog`, compared with the
/// saved `connections`.
pub fn candidates(catalog: &Catalog, connections: &Connections) -> Report {
    let of_kind = |kind: &'static str| {
        catalog
            .records
            .iter()
            .filter(move |record| record.kind == kind)
    };
    let hosts: Vec<&Record> = of_kind("host").collect();
    let group_records: Vec<&Record> = of_kind("group").collect();
    let proxies: Vec<&Record> = of_kind("proxy")
        .filter(|proxy| !boolean(&proxy.data, "stopped"))
        .collect();
    let group_name = |value: &str| -> Option<String> {
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        let named = group_records
            .iter()
            .find(|group| group.id == value)
            .and_then(|group| text(&group.data, "name"));
        Some(named.unwrap_or(value).trim().to_owned()).filter(|name| !name.is_empty())
    };
    let mut groups: Vec<String> = Vec::new();
    let candidates = hosts
        .iter()
        .map(|host| {
            let candidate = candidate(host, &hosts, &proxies, &group_name, connections);
            if let Some(group) = &candidate.group {
                if !groups.iter().any(|known| known.eq_ignore_ascii_case(group)) {
                    groups.push(group.clone());
                }
            }
            candidate
        })
        .collect();
    Report {
        format: catalog.format.clone(),
        groups,
        candidates,
    }
}

fn candidate(
    host: &Record,
    hosts: &[&Record],
    proxies: &[&Record],
    group_name: &dyn Fn(&str) -> Option<String>,
    connections: &Connections,
) -> Candidate {
    let data = &host.data;
    let mut fields = Vec::new();
    let address = text(data, "address").unwrap_or("").trim().to_owned();
    let user = text(data, "user")
        .map(str::trim)
        .filter(|user| !user.is_empty());
    let port = data
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port != 0);
    let label = text(data, "label")
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .unwrap_or(&address)
        .to_owned();
    fields.push(imported("label", label.clone(), "name"));
    fields.push(imported("address", address.clone(), "host"));
    if let Some(user) = user {
        fields.push(imported("user", user.to_owned(), "user"));
    }
    if let Some(port) = port {
        fields.push(imported("port", port.to_string(), "port"));
    }
    let color = text(data, "color").and_then(nearest_color);
    if let Some(hex) = text(data, "color").filter(|_| color.is_some()) {
        fields.push(imported("color", hex.trim().to_owned(), "color"));
    }
    let group = text(data, "group").and_then(group_name);
    if let Some(group) = &group {
        fields.push(imported("group", group.clone(), "group"));
    }

    let mut connection = SshConnection::to_host(address.clone());
    connection.user = user.map(str::to_owned);
    connection.port = port;

    let protocol = text(data, "protocol")
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let ssh = protocol.is_empty() || protocol == "ssh";
    if !ssh {
        fields.push(skipped("protocol", protocol.clone(), Skip::Protocol));
    }

    let options = data.get("ssh_options").and_then(Value::as_object);
    let key_files: Vec<String> = options
        .and_then(|options| options.get("identity_files"))
        .and_then(Value::as_array)
        .map(|files| {
            files
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|file| !file.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let method = text(data, "authentication")
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match method.as_str() {
        "password" => connection.authentication = Authentication::Password,
        "agent" => connection.authentication = Authentication::Agent,
        // Keys, or Motty's automatic choice: the first key file decides.
        "" | "key" | "publickey" => {
            if let Some(first) = key_files.first() {
                connection.authentication = Authentication::PrivateKey;
                connection.identity_file = Some(first.clone());
            }
        }
        _ => {}
    }
    match method.as_str() {
        "" => {}
        "password" | "agent" | "key" | "publickey" => {
            fields.push(imported("authentication", method.clone(), "authentication"));
        }
        "kerberos" => fields.push(skipped("authentication", method.clone(), Skip::Kerberos)),
        _ => fields.push(skipped("authentication", method.clone(), Skip::NoSetting)),
    }
    let mut keys = key_files.iter();
    if let Some(chosen) = &connection.identity_file {
        keys.next();
        fields.push(imported(
            "ssh_options.identity_files",
            chosen.clone(),
            "identityFile",
        ));
    }
    for extra in keys {
        fields.push(skipped(
            "ssh_options.identity_files",
            extra.clone(),
            Skip::ExtraKey,
        ));
    }
    for secret in ["identity", "credential"] {
        if data.get(secret).is_some_and(present) {
            fields.push(skipped(secret, String::new(), Skip::Secret));
        }
    }
    if let Some(interval) = options
        .and_then(|options| options.get("server_alive_interval"))
        .and_then(Value::as_u64)
        .and_then(|seconds| u32::try_from(seconds).ok())
        .filter(|seconds| *seconds != 0)
    {
        connection.keep_alive_seconds = Some(interval);
        fields.push(imported(
            "ssh_options.server_alive_interval",
            interval.to_string(),
            "keepAliveSeconds",
        ));
    }
    for (key, value) in options.into_iter().flatten() {
        let handled = matches!(key.as_str(), "identity_files" | "server_alive_interval")
            // Lithe offers only the chosen key, with `IdentitiesOnly`.
            || (key == "identities_only" && connection.identity_file.is_some());
        if !handled && present(value) {
            fields.push(skipped(
                &format!("ssh_options.{key}"),
                display(value),
                Skip::NoSetting,
            ));
        }
    }
    match jump_route(host, hosts, proxies) {
        Jump::None => {}
        Jump::Route(route) => {
            connection.proxy_jump = Some(route.clone());
            fields.push(imported("proxy", route, "proxyJump"));
        }
        Jump::Command => fields.push(skipped("proxy", String::new(), Skip::Command)),
    }
    for (key, value) in data {
        if !HANDLED.contains(&key.as_str()) && present(value) {
            fields.push(skipped(key, display(value), Skip::NoSetting));
        }
    }

    let connection = connection.normalized();
    let valid = ssh && connection.check().is_ok();
    if ssh && !valid {
        if let Some(field) = fields.iter_mut().find(|field| field.motty == "address") {
            field.lithe = None;
            field.skip = Some(Skip::Invalid);
        }
    }
    let existing_id = valid
        .then(|| connections.existing(&connection))
        .flatten()
        .map(|entry| entry.id().to_owned());
    let status = if !valid {
        Status::Unsupported
    } else if existing_id.is_some() {
        Status::Exists
    } else if fields.iter().any(|field| field.skip.is_some()) {
        Status::Partial
    } else {
        Status::Importable
    };
    Candidate {
        key: host.id.clone(),
        name: label,
        address: display_address(&address, user, port),
        system: text(data, "system")
            .map(str::trim)
            .filter(|system| !system.is_empty())
            .map(str::to_owned),
        color,
        group,
        status,
        existing_id,
        connection: valid.then_some(connection),
        fields,
    }
}

/// Host settings [`candidate`] maps itself, and those that only change how
/// Motty shows a host.
const HANDLED: &[&str] = &[
    "label",
    "address",
    "user",
    "port",
    "color",
    "group",
    "protocol",
    "authentication",
    "ssh_options",
    "identity",
    "credential",
    "proxy",
    "id",
    "icon",
    "system",
    "order",
    "pinned",
];

fn imported(motty: &str, value: String, lithe: &'static str) -> Field {
    Field {
        motty: motty.to_owned(),
        value,
        lithe: Some(lithe),
        skip: None,
    }
}

fn skipped(motty: &str, value: String, skip: Skip) -> Field {
    Field {
        motty: motty.to_owned(),
        value,
        lithe: None,
        skip: Some(skip),
    }
}

enum Jump {
    None,
    Route(String),
    Command,
}

/// The jump route of `host`: the jump host of its proxy record, after the
/// jump host's own route, as OpenSSH's comma-separated `ProxyJump`.
fn jump_route(host: &Record, hosts: &[&Record], proxies: &[&Record]) -> Jump {
    let mut route: Vec<String> = Vec::new();
    let mut target = host.id.clone();
    for _ in 0..MAX_JUMPS {
        let Some(proxy) = proxies
            .iter()
            .find(|proxy| text(&proxy.data, "target_id") == Some(target.as_str()))
        else {
            break;
        };
        if boolean(&proxy.data, "via_shell")
            || text(&proxy.data, "relay_command").is_some_and(|command| !command.trim().is_empty())
        {
            return Jump::Command;
        }
        let jump_host =
            text(&proxy.data, "jump_id").and_then(|id| hosts.iter().find(|host| host.id == id));
        let (address, user, port) = match jump_host {
            Some(jump) => (
                text(&jump.data, "address").unwrap_or("").trim().to_owned(),
                text(&jump.data, "user").map(str::to_owned),
                jump.data.get("port").and_then(Value::as_u64),
            ),
            None => (
                text(&proxy.data, "jump_address")
                    .unwrap_or("")
                    .trim()
                    .to_owned(),
                text(&proxy.data, "jump_user").map(str::to_owned),
                proxy.data.get("jump_port").and_then(Value::as_u64),
            ),
        };
        if address.is_empty() {
            break;
        }
        let port = port
            .and_then(|port| u16::try_from(port).ok())
            .filter(|port| *port != 0);
        let user = user
            .as_deref()
            .map(str::trim)
            .filter(|user| !user.is_empty());
        route.insert(0, display_address(&address, user, port));
        match jump_host {
            Some(jump) if jump.id != target => target = jump.id.clone(),
            _ => break,
        }
    }
    if route.is_empty() {
        Jump::None
    } else {
        Jump::Route(route.join(","))
    }
}

/// `[user@]host[:port]`, with an IPv6 host in brackets when a port follows.
fn display_address(host: &str, user: Option<&str>, port: Option<u16>) -> String {
    let mut text = String::new();
    if let Some(user) = user {
        text.push_str(user);
        text.push('@');
    }
    match port {
        Some(port) if host.contains(':') => text.push_str(&format!("[{host}]:{port}")),
        Some(port) => text.push_str(&format!("{host}:{port}")),
        None => text.push_str(host),
    }
    text
}

/// The Lithe color closest to Motty's `#rrggbb` color.
fn nearest_color(hex: &str) -> Option<Color> {
    let hex = hex.trim().strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let channel =
        |index: usize| i32::from(u8::from_str_radix(&hex[index..index + 2], 16).unwrap_or(0));
    let (red, green, blue) = (channel(0), channel(2), channel(4));
    // Motty's palette (Tailwind 500 shades), which maps exactly.
    [
        (Color::Red, 0xef, 0x44, 0x44),
        (Color::Orange, 0xf9, 0x73, 0x16),
        (Color::Yellow, 0xea, 0xb3, 0x08),
        (Color::Green, 0x22, 0xc5, 0x5e),
        (Color::Blue, 0x3b, 0x82, 0xf6),
        (Color::Purple, 0xa8, 0x55, 0xf7),
    ]
    .into_iter()
    .min_by_key(|(_, r, g, b)| (red - r).pow(2) + (green - g).pow(2) + (blue - b).pow(2))
    .map(|(color, ..)| color)
}

fn text<'a>(data: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    data.get(key).and_then(Value::as_str)
}

fn boolean(data: &Map<String, Value>, key: &str) -> bool {
    data.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Whether a setting has a value: not null, `false`, `0`, empty text, an
/// empty list or an object of such values.
fn present(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(set) => *set,
        Value::Number(number) => number.as_f64() != Some(0.0),
        Value::String(text) => !text.trim().is_empty(),
        Value::Array(items) => items.iter().any(present),
        Value::Object(fields) => fields.values().any(present),
    }
}

/// A short form of a setting's value for the window.
fn display(value: &Value) -> String {
    match value {
        Value::String(text) => text.trim().to_owned(),
        Value::Bool(_) => String::new(),
        Value::Array(items) => items
            .iter()
            .filter(|item| present(item))
            .map(display)
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(_) => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests;
