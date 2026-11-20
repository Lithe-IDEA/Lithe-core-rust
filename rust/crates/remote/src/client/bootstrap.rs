//! What the client learns before installing `lithe-remote-server` at the end
//! of a transport chain: the host's platform, facts about the host, and the
//! local server builds that can run there.
//!
//! The client always uploads the server itself instead of letting the remote
//! download it, because many SSH hosts and containers have no internet access.
//! [`crate::client::launch`] probes, installs and starts the server over one
//! connection and reuses this module's probe script and parser, which the
//! standalone probe also runs. Each version lives in its own install
//! directory, so clients of different releases can share a host.

use sha2::{Digest, Sha256};
use std::fs::File;
use std::io;
use std::path::PathBuf;

const PROBE_BEGIN: &str = "LITHE-PROBE-BEGIN";
const PROBE_END: &str = "LITHE-PROBE-END";

/// Reports system, machine and home directory between markers, so output
/// from the remote user's shell startup files is ignored.
const PROBE_SCRIPT: &str = concat!(
    host_details_script!(),
    "echo LITHE-PROBE-BEGIN; uname -s; uname -m; echo \"$HOME\"; echo LITHE-PROBE-END"
);

/// Reports facts shown to the user as `key=value` lines between markers.
/// Every command is optional: a missing tool leaves its line empty. Raw
/// command output is parsed here rather than on the host, because the
/// script must avoid the characters that cannot cross an SSH hop.
macro_rules! host_details_script {
    () => {
        concat!(
            "echo LITHE-DETAILS-BEGIN; ",
            "echo \"hostName=$(uname -n)\"; echo \"kernel=$(uname -r)\"; ",
            "if [ -r /etc/os-release ]; then ( . /etc/os-release; echo \"operatingSystem=$PRETTY_NAME\" ); ",
            "elif command -v sw_vers >/dev/null 2>&1; then echo \"operatingSystem=macOS $(sw_vers -productVersion)\"; fi; ",
            "echo \"cpuCount=$(getconf _NPROCESSORS_ONLN 2>/dev/null)\"; ",
            "if [ -r /proc/meminfo ]; then echo \"memInfo=$(grep MemTotal /proc/meminfo)\"; ",
            "else echo \"memoryBytes=$(sysctl -n hw.memsize 2>/dev/null)\"; fi; ",
            "echo \"disk=$(df -Pk \"$HOME\" 2>/dev/null | tail -n 1)\"; ",
            "if [ -r /proc/uptime ]; then echo \"uptime=$(cat /proc/uptime)\"; fi; ",
            "echo LITHE-DETAILS-END; "
        )
    };
}
pub(crate) use host_details_script;

/// Hex digits of the SHA-256 used in the install directory name. Sixteen
/// keep paths short while making accidental collisions between builds of the
/// same version practically impossible; the full hash is still verified.
pub(crate) const INSTALL_HASH_PREFIX: usize = 16;

/// Server build target selected from the remote's `uname` output.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ServerTarget {
    /// Linux on x86_64.
    LinuxX86_64,
    /// Linux on AArch64.
    LinuxAarch64,
    /// macOS on Intel.
    MacosX86_64,
    /// macOS on Apple silicon.
    MacosAarch64,
}

impl ServerTarget {
    /// Rust target triple the artifact is built for. Linux uses musl so one
    /// static binary runs on glibc and musl distributions alike.
    pub fn rust_target(self) -> &'static str {
        match self {
            Self::LinuxX86_64 => "x86_64-unknown-linux-musl",
            Self::LinuxAarch64 => "aarch64-unknown-linux-musl",
            Self::MacosX86_64 => "x86_64-apple-darwin",
            Self::MacosAarch64 => "aarch64-apple-darwin",
        }
    }

    /// Maps `uname -s` and `uname -m`. Architecture comes from the remote,
    /// not the local machine: an amd64 container on an Apple silicon Mac
    /// needs the x86_64 server.
    pub fn from_uname(system: &str, machine: &str) -> Option<Self> {
        let arm = matches!(machine, "aarch64" | "arm64" | "aarch64_be");
        let intel = matches!(machine, "x86_64" | "amd64");
        match system {
            "Linux" if intel => Some(Self::LinuxX86_64),
            "Linux" if arm => Some(Self::LinuxAarch64),
            "Darwin" if intel => Some(Self::MacosX86_64),
            "Darwin" if arm => Some(Self::MacosAarch64),
            _ => None,
        }
    }
}

/// A server binary available on the local machine.
#[derive(Clone, Debug)]
pub struct ServerArtifact {
    /// Platform the binary runs on.
    pub target: ServerTarget,
    /// Location of the binary on the local machine.
    pub path: PathBuf,
    /// Server version, the first part of the install directory name.
    pub version: String,
    /// Lowercase hex SHA-256 of the binary; the host must compute the same hash.
    pub sha256: String,
}

impl ServerArtifact {
    /// Describes a binary on disk, hashing it.
    pub fn from_file(
        target: ServerTarget,
        path: PathBuf,
        version: impl Into<String>,
    ) -> io::Result<Self> {
        let mut hasher = Sha256::new();
        io::copy(&mut File::open(&path)?, &mut hasher)?;
        Ok(Self {
            target,
            path,
            version: version.into(),
            sha256: crate::protocol::methods::encode_hex(&hasher.finalize()),
        })
    }
}

/// What the probe reported about the end of a chain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProbeReport {
    /// `uname -s`, for example `Linux` or `Darwin`.
    pub system: String,
    /// `uname -m`, for example `x86_64` or `arm64`.
    pub machine: String,
    /// `$HOME` when it is an absolute path.
    pub home: Option<String>,
    /// Host facts for display.
    pub details: HostDetails,
}

/// Facts about the host for display; each is `None` when it could not be read.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostDetails {
    /// `uname -n`.
    pub host_name: Option<String>,
    /// For example `Ubuntu 22.04.4 LTS` or `macOS 15.1`.
    pub operating_system: Option<String>,
    /// `uname -r`.
    pub kernel: Option<String>,
    /// Online processors, from `getconf _NPROCESSORS_ONLN`.
    pub cpu_count: Option<u32>,
    /// Physical memory in bytes.
    pub memory_bytes: Option<u64>,
    /// Size of the file system holding the home directory.
    pub disk_total_bytes: Option<u64>,
    /// Free bytes on that file system available to the user.
    pub disk_available_bytes: Option<u64>,
    /// Whole seconds since boot.
    pub uptime_seconds: Option<u64>,
}

/// Reads the details block; missing or malformed lines leave fields unset.
fn parse_details(text: &str) -> HostDetails {
    let mut details = HostDetails::default();
    let mut lines = text.lines().map(str::trim_end);
    if lines
        .by_ref()
        .find(|line| *line == "LITHE-DETAILS-BEGIN")
        .is_none()
    {
        return details;
    }
    let present = |value: &str| Some(value.trim().to_owned()).filter(|value| !value.is_empty());
    for line in lines {
        if line == "LITHE-DETAILS-END" {
            break;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "hostName" => details.host_name = present(value),
            "kernel" => details.kernel = present(value),
            "operatingSystem" => details.operating_system = present(value),
            "cpuCount" => details.cpu_count = value.trim().parse().ok(),
            "memoryBytes" => details.memory_bytes = value.trim().parse().ok(),
            // `MemTotal:       16303780 kB`
            "memInfo" => {
                details.memory_bytes = value
                    .split_whitespace()
                    .nth(1)
                    .and_then(|kilobytes| kilobytes.parse::<u64>().ok())
                    .map(|kilobytes| kilobytes * 1024)
            }
            // `df -Pk`: file system, 1024-blocks, used, available, capacity, mount.
            "disk" => {
                let fields: Vec<&str> = value.split_whitespace().collect();
                if fields.len() >= 6 {
                    let kilobytes =
                        |index: usize| fields[index].parse::<u64>().ok().map(|value| value * 1024);
                    details.disk_total_bytes = kilobytes(1);
                    details.disk_available_bytes = kilobytes(3);
                }
            }
            // `/proc/uptime`: seconds since boot, then idle seconds.
            "uptime" => {
                details.uptime_seconds = value
                    .split_whitespace()
                    .next()
                    .and_then(|seconds| seconds.parse::<f64>().ok())
                    .map(|seconds| seconds as u64)
            }
            _ => {}
        }
    }
    details
}

impl ProbeReport {
    /// Server build for this platform, or `None` when none is shipped.
    pub fn target(&self) -> Option<ServerTarget> {
        ServerTarget::from_uname(&self.system, &self.machine)
    }
}

/// Command that reports the remote platform. Callers that run it with their
/// own process handling, for example to answer SSH prompts, pass it to
/// [`crate::client::TransportChain::command`] and read the output with [`parse_probe`].
pub fn probe_command() -> Vec<String> {
    vec!["sh".into(), "-c".into(), PROBE_SCRIPT.into()]
}

/// Reads the output of [`probe_command`]; `None` when the marked block is
/// missing or incomplete, for example because the shell never ran.
pub fn parse_probe(stdout: &[u8]) -> Option<ProbeReport> {
    let lines = probe_lines(stdout)?;
    let [system, machine, home] = lines.as_slice() else {
        return None;
    };
    Some(ProbeReport {
        system: system.clone(),
        machine: machine.clone(),
        home: Some(home.clone()).filter(|home| home.starts_with('/')),
        details: parse_details(&String::from_utf8_lossy(stdout)),
    })
}

fn probe_lines(stdout: &[u8]) -> Option<Vec<String>> {
    let text = String::from_utf8_lossy(stdout);
    let mut lines = text.lines().map(str::trim_end);
    lines.by_ref().find(|line| *line == PROBE_BEGIN)?;
    let mut collected = Vec::new();
    for line in lines {
        if line == PROBE_END {
            return Some(collected);
        }
        collected.push(line.to_owned());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_details_are_read_from_raw_command_output() {
        let stdout = b"LITHE-DETAILS-BEGIN\nhostName=devbox\nkernel=6.8.0-45-generic\n\
operatingSystem=Ubuntu 22.04.4 LTS\ncpuCount=4\nmemInfo=MemTotal:       16303780 kB\n\
disk=/dev/vda1 82045336 23412820 55282748 30% /\nuptime=93784.52 351234.11\nLITHE-DETAILS-END\n\
LITHE-PROBE-BEGIN\nLinux\nx86_64\n/root\nLITHE-PROBE-END\n";
        let details = parse_probe(stdout).unwrap().details;
        assert_eq!(
            details,
            HostDetails {
                host_name: Some("devbox".into()),
                operating_system: Some("Ubuntu 22.04.4 LTS".into()),
                kernel: Some("6.8.0-45-generic".into()),
                cpu_count: Some(4),
                memory_bytes: Some(16_303_780 * 1024),
                disk_total_bytes: Some(82_045_336 * 1024),
                disk_available_bytes: Some(55_282_748 * 1024),
                uptime_seconds: Some(93_784),
            }
        );
        // macOS reports memory directly and has no /proc; empty values stay unset.
        let mac = parse_details(
            "LITHE-DETAILS-BEGIN\nmemoryBytes=17179869184\ncpuCount=\nLITHE-DETAILS-END\n",
        );
        assert_eq!(mac.memory_bytes, Some(17_179_869_184));
        assert_eq!(mac.cpu_count, None);
    }

    // The probe crosses SSH hops through the transport's quoting, which
    // rejects these characters instead of escaping them.
    #[test]
    fn the_probe_can_cross_an_ssh_hop() {
        assert!(!PROBE_SCRIPT
            .chars()
            .any(|character| matches!(character, '\'' | '\\' | '!') || character.is_control()));
    }

    #[test]
    fn uname_values_map_to_server_targets() {
        assert_eq!(
            ServerTarget::from_uname("Linux", "x86_64"),
            Some(ServerTarget::LinuxX86_64)
        );
        assert_eq!(
            ServerTarget::from_uname("Linux", "aarch64"),
            Some(ServerTarget::LinuxAarch64)
        );
        assert_eq!(
            ServerTarget::from_uname("Darwin", "arm64"),
            Some(ServerTarget::MacosAarch64)
        );
        assert_eq!(
            ServerTarget::from_uname("Darwin", "x86_64"),
            Some(ServerTarget::MacosX86_64)
        );
        assert_eq!(ServerTarget::from_uname("Linux", "armv7l"), None);
        assert_eq!(ServerTarget::from_uname("FreeBSD", "amd64"), None);
    }

    // Shell startup files may print before and after the probe; only the
    // marked block is trusted.
    #[test]
    fn probe_output_is_read_between_markers() {
        let stdout =
            b"Welcome!\nLITHE-PROBE-BEGIN\nLinux\nx86_64\n/home/dev\nLITHE-PROBE-END\nbye\n";
        assert_eq!(
            probe_lines(stdout).unwrap(),
            vec!["Linux".to_string(), "x86_64".into(), "/home/dev".into()]
        );
        assert_eq!(probe_lines(b"LITHE-PROBE-BEGIN\nLinux\n"), None);
        assert_eq!(probe_lines(b"Permission denied\n"), None);
        assert_eq!(
            parse_probe(stdout).unwrap(),
            ProbeReport {
                system: "Linux".into(),
                machine: "x86_64".into(),
                home: Some("/home/dev".into()),
                details: HostDetails::default(),
            }
        );
        // A relative or empty `$HOME` is not a usable install location.
        assert_eq!(
            parse_probe(b"LITHE-PROBE-BEGIN\nLinux\nx86_64\n\nLITHE-PROBE-END\n")
                .unwrap()
                .home,
            None
        );
    }
}
