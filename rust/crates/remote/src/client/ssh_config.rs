//! What OpenSSH makes of a saved connection, and the hosts the user's SSH
//! configuration names (design §3.1).
//!
//! - [`effective_settings`] reads two `ssh -G` outputs of the same hop, one
//!   with and one without the configuration files (see
//!   [`crate::client::SshHop::configuration_command`]), and tells for every
//!   value the window shows whether the connection, the SSH configuration or
//!   a default set it.
//! - [`configuration_files`] and [`host_aliases`] find the `Host` aliases of
//!   `~/.ssh/config` and the files it includes, without running anything;
//!   [`configured_host`] reads one alias's `ssh -G` output for the import
//!   list.
//!
//! Nothing here starts a process: the caller runs `ssh -G` with the
//! restricted environment of the transport and passes the output in. Only
//! files below the user's home directory are read, at most 64 files of
//! 1 MiB each, 16 `Include` levels deep, as OpenSSH bounds its own reading.

use crate::client::connections::{Authentication, SshConnection, DEFAULT_CONNECT_TIMEOUT_SECONDS};
use crate::client::transport::SSH_KEEPALIVE_SECONDS;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// `Include` levels followed, as OpenSSH's own `READCONF_MAX_DEPTH`.
const MAX_INCLUDE_DEPTH: usize = 16;

/// Configuration files read in total.
const MAX_FILES: usize = 64;

/// Largest configuration file read, in bytes.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Aliases listed for an import.
pub const MAX_ALIASES: usize = 256;

/// Directory entries a wildcard `Include` examines per directory.
const MAX_DIRECTORY_ENTRIES: usize = 1024;

/// Where an effective value comes from.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    /// The saved connection sets it.
    Connection,
    /// The user's or the system's SSH configuration sets it.
    SshConfig,
    /// Neither does: it is OpenSSH's or Lithe's default.
    Default,
}

/// One value and where it comes from.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Effective<T> {
    /// The value OpenSSH uses.
    pub value: T,
    /// Where it comes from.
    pub source: Source,
}

/// The values a connection uses, as `ssh -G` prints them.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveSettings {
    /// Name or address OpenSSH connects to after applying `HostName`.
    pub host_name: Effective<String>,
    /// TCP port.
    pub port: Effective<u16>,
    /// Login name; the local user name by default.
    pub user: Effective<String>,
    /// Private key paths OpenSSH offers, as written in the configuration.
    /// Empty for password authentication.
    pub identity_files: Effective<Vec<String>>,
    /// Jump hosts, or `None` for a direct connection.
    pub proxy_jump: Effective<Option<String>>,
    /// Seconds to wait for the TCP connection.
    pub connect_timeout_seconds: Effective<u32>,
    /// Seconds between keep-alive probes.
    pub keep_alive_seconds: Effective<u32>,
}

/// Why `ssh -G` output could not be read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputError {
    /// The key that is missing or malformed.
    pub key: &'static str,
}

/// `ssh -G` output: lowercase keys and their values in order. Keys such as
/// `identityfile` repeat.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Output {
    values: BTreeMap<String, Vec<String>>,
}

impl Output {
    /// Reads `ssh -G` output, one `key value` pair per line.
    pub fn parse(text: &str) -> Self {
        let mut values: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for line in text.lines() {
            let Some((key, value)) = line.trim().split_once(' ') else {
                continue;
            };
            values
                .entry(key.to_ascii_lowercase())
                .or_default()
                .push(value.trim().to_owned());
        }
        Self { values }
    }

    fn first(&self, key: &str) -> Option<&str> {
        self.values
            .get(key)
            .and_then(|values| values.first())
            .map(String::as_str)
    }

    fn all(&self, key: &str) -> Vec<String> {
        self.values.get(key).cloned().unwrap_or_default()
    }

    fn required(&self, key: &'static str) -> Result<&str, OutputError> {
        self.first(key).ok_or(OutputError { key })
    }

    fn number<T: std::str::FromStr>(&self, key: &'static str) -> Result<T, OutputError> {
        self.required(key)?.parse().map_err(|_| OutputError { key })
    }

    /// `proxyjump`, absent when unset or `none`.
    fn jumps(&self) -> Option<String> {
        self.first("proxyjump")
            .filter(|jumps| !jumps.eq_ignore_ascii_case("none"))
            .map(str::to_owned)
    }
}

/// The settings `connection` uses: `configured` is the `ssh -G` output of
/// its hop, `baseline` the same command with `-F none`.
///
/// A field the connection sets comes from the connection. Otherwise a value
/// that differs from the baseline comes from the SSH configuration, and an
/// equal one is a default. The connect timeout and the keep-alive interval
/// are always passed on the command line, which wins over the configuration
/// files, so they come from the connection or are Lithe's defaults.
pub fn effective_settings(
    connection: &SshConnection,
    configured: &Output,
    baseline: &Output,
) -> Result<EffectiveSettings, OutputError> {
    let source = |set: bool, key: &str| {
        if set {
            Source::Connection
        } else if configured.values.get(key) != baseline.values.get(key) {
            Source::SshConfig
        } else {
            Source::Default
        }
    };
    let host_name = configured.required("hostname")?.to_owned();
    let host_source = match source(false, "hostname") {
        // Without a `HostName`, OpenSSH connects to the host the connection
        // names.
        Source::Default => Source::Connection,
        other => other,
    };
    let identity_files = match connection.authentication() {
        Authentication::Password => Effective {
            value: Vec::new(),
            source: Source::Connection,
        },
        Authentication::PrivateKey => Effective {
            value: configured.all("identityfile"),
            source: Source::Connection,
        },
        Authentication::Auto | Authentication::Agent => Effective {
            value: configured.all("identityfile"),
            source: source(false, "identityfile"),
        },
    };
    let lithe_default = |set: bool| {
        if set {
            Source::Connection
        } else {
            Source::Default
        }
    };
    Ok(EffectiveSettings {
        host_name: Effective {
            value: host_name,
            source: host_source,
        },
        port: Effective {
            value: configured.number("port")?,
            source: source(connection.port().is_some(), "port"),
        },
        user: Effective {
            value: configured.required("user")?.to_owned(),
            source: source(connection.user().is_some(), "user"),
        },
        identity_files,
        proxy_jump: Effective {
            value: configured.jumps(),
            source: source(connection.proxy_jump().is_some(), "proxyjump"),
        },
        connect_timeout_seconds: Effective {
            value: configured
                .number("connecttimeout")
                .unwrap_or(DEFAULT_CONNECT_TIMEOUT_SECONDS),
            source: lithe_default(connection.connect_timeout_seconds().is_some()),
        },
        keep_alive_seconds: Effective {
            value: configured
                .number("serveraliveinterval")
                .unwrap_or(SSH_KEEPALIVE_SECONDS),
            source: lithe_default(connection.keep_alive_seconds().is_some()),
        },
    })
}

/// A `Host` line's names, as the import lists them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostName {
    /// A name a connection can use as its host.
    Alias(String),
    /// A pattern with `*`, `?` or `!`, which names no single host.
    Pattern(String),
}

/// What one alias resolves to, read from its `ssh -G` output.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfiguredHost {
    /// Name or address after `HostName`.
    pub host_name: String,
    /// Login name.
    pub user: String,
    /// TCP port.
    pub port: u16,
    /// Jump hosts, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_jump: Option<String>,
}

/// Reads one alias's `ssh -G` output.
pub fn configured_host(output: &Output) -> Result<ConfiguredHost, OutputError> {
    Ok(ConfiguredHost {
        host_name: output.required("hostname")?.to_owned(),
        user: output.required("user")?.to_owned(),
        port: output.number("port")?,
        proxy_jump: output.jumps(),
    })
}

/// The hosts named by `jumps` (`[user@]host[:port]`, comma separated), for
/// telling which aliases other hosts jump through.
pub fn jump_hosts(jumps: &str) -> Vec<String> {
    jumps
        .split(',')
        .map(|jump| {
            let host = jump.rsplit_once('@').map_or(jump, |(_, host)| host);
            let host = match host.strip_prefix('[') {
                Some(bracketed) => bracketed.split(']').next().unwrap_or(bracketed),
                None => host.split(':').next().unwrap_or(host),
            };
            host.trim().to_owned()
        })
        .filter(|host| !host.is_empty())
        .collect()
}

/// The names on the `Host` lines of `text`, in order, and the arguments of
/// its `Include` lines. `Match` blocks name no host and are skipped.
pub fn host_aliases(text: &str) -> (Vec<HostName>, Vec<String>) {
    let mut names = Vec::new();
    let mut includes = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (keyword, arguments) = split_keyword(line);
        if keyword.eq_ignore_ascii_case("host") {
            for word in words(arguments) {
                if word.contains(['*', '?', '!']) {
                    names.push(HostName::Pattern(word));
                } else {
                    names.push(HostName::Alias(word));
                }
            }
        } else if keyword.eq_ignore_ascii_case("include") {
            includes.extend(words(arguments));
        }
    }
    (names, includes)
}

/// The user's configuration file and every file it includes, in the order
/// OpenSSH reads them, with their text. `home` is the user's home
/// directory. A missing `~/.ssh/config` gives no files.
pub fn configuration_files(home: &Path) -> Vec<(PathBuf, String)> {
    let ssh_directory = home.join(".ssh");
    let mut files = Vec::new();
    read_configuration(
        &ssh_directory.join("config"),
        home,
        &ssh_directory,
        0,
        &mut files,
    );
    files
}

fn read_configuration(
    path: &Path,
    home: &Path,
    ssh_directory: &Path,
    depth: usize,
    files: &mut Vec<(PathBuf, String)>,
) {
    if depth > MAX_INCLUDE_DEPTH
        || files.len() >= MAX_FILES
        || files.iter().any(|(read, _)| read == path)
    {
        return;
    }
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let (_, includes) = host_aliases(&text);
    files.push((path.to_owned(), text));
    for include in includes {
        for included in expand_include(&include, home, ssh_directory) {
            read_configuration(&included, home, ssh_directory, depth + 1, files);
        }
    }
}

/// The files an `Include` argument names: `~` is the home directory, a
/// relative path is below `~/.ssh`, and `*` and `?` match within one path
/// component, in sorted order as OpenSSH's `glob` returns them. Paths
/// outside the home directory are left out.
fn expand_include(argument: &str, home: &Path, ssh_directory: &Path) -> Vec<PathBuf> {
    let path = if let Some(rest) = argument.strip_prefix("~/") {
        home.join(rest)
    } else if Path::new(argument).is_absolute() {
        PathBuf::from(argument)
    } else {
        ssh_directory.join(argument)
    };
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
        || !path.starts_with(home)
    {
        return Vec::new();
    }
    let mut matches = vec![PathBuf::new()];
    for component in path.components() {
        let part = component.as_os_str().to_string_lossy();
        if !part.contains(['*', '?']) {
            for candidate in &mut matches {
                candidate.push(component.as_os_str());
            }
            continue;
        }
        let mut next = Vec::new();
        for directory in &matches {
            let Ok(entries) = std::fs::read_dir(directory) else {
                continue;
            };
            let mut names: Vec<String> = entries
                .take(MAX_DIRECTORY_ENTRIES)
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| !name.starts_with('.') && wildcard_match(&part, name))
                .collect();
            names.sort();
            next.extend(names.into_iter().map(|name| directory.join(name)));
        }
        matches = next;
    }
    matches.truncate(MAX_FILES);
    matches
}

/// `*` matches any run of characters and `?` one character.
fn wildcard_match(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    let (mut p, mut n) = (0, 0);
    let (mut star, mut resume) = (None, 0);
    while n < name.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == name[n]) {
            p += 1;
            n += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            resume = n;
            p += 1;
        } else if let Some(position) = star {
            p = position + 1;
            resume += 1;
            n = resume;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|character| *character == '*')
}

/// `Keyword arguments` or `Keyword=arguments`.
fn split_keyword(line: &str) -> (&str, &str) {
    let end = line
        .find(|character: char| character.is_whitespace() || character == '=')
        .unwrap_or(line.len());
    let (keyword, rest) = line.split_at(end);
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest);
    (keyword, rest.trim())
}

/// Whitespace-separated words, where double quotes keep spaces in a word.
fn words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for character in text.chars() {
        match character {
            '"' => quoted = !quoted,
            character if character.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            character => current.push(character),
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

#[cfg(test)]
mod tests;
