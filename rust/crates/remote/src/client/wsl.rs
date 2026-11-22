//! Distributions of the Windows Subsystem for Linux as remote workspaces.
//!
//! A distribution on the local Windows machine is reached through
//! `wsl.exe` instead of OpenSSH. [`WslConnection`] names the distribution
//! and, optionally, the Linux account, and becomes the first hop of a
//! transport chain; the server is then probed, installed and started inside
//! the distribution exactly as on an SSH host. Nothing is saved: the
//! connection window lists the installed distributions each time it opens,
//! with [`list_command`] and [`parse_distributions`].
//!
//! `wsl.exe --list` prints UTF-16LE unless `WSL_UTF8=1` is set, and releases
//! before WSL 0.64 ignore that variable, so the parser reads both encodings.
//!
//! Note: .agents/notes/proposed/architecture/2026-10-24-remote-development-workspace-host.md

use crate::client::connections::{ConnectionError, Reason};
use crate::client::transport::{
    is_wsl_distribution, is_wsl_user, wsl_program, WslHop, MAX_WSL_DISTRIBUTION_BYTES,
    MAX_WSL_USER_BYTES,
};
use serde::{Deserialize, Serialize};

/// Distributions one listing returns at most; later ones are dropped.
pub const MAX_DISTRIBUTIONS: usize = 64;

/// Bytes of `wsl.exe --list` output read at most; the rest is ignored.
pub const MAX_LIST_BYTES: usize = 64 * 1024;

/// Distributions Docker Desktop registers for its own engine. They hold no
/// user workspace, and `docker-desktop-data` has no shell at all, so
/// listings leave them out, as Visual Studio Code does.
const ENGINE_DISTRIBUTIONS: [&str; 2] = ["docker-desktop", "docker-desktop-data"];

/// `remote.connect`'s `wsl` field: the distribution to open and the account.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WslConnection {
    /// Distribution name as `wsl.exe --list` prints it, for example `Ubuntu-24.04`.
    pub distribution: String,
    /// Linux account passed as `--user`; the distribution's default user when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

impl WslConnection {
    /// The hop into the distribution through the system's `wsl.exe`. Both
    /// fields are trimmed and an empty `user` means the default user.
    ///
    /// Refused, with the field and the reason, when the distribution is
    /// empty, longer than 64 bytes or not a name `wsl.exe --import` accepts,
    /// or the user is longer than 32 bytes or not a Linux login name.
    pub fn hop(&self) -> Result<WslHop, ConnectionError> {
        let distribution = self.distribution.trim();
        if distribution.is_empty() {
            return Err(ConnectionError::at("distribution", Reason::Required));
        }
        if distribution.len() > MAX_WSL_DISTRIBUTION_BYTES {
            return Err(ConnectionError::at("distribution", Reason::TooLong));
        }
        if !is_wsl_distribution(distribution) {
            return Err(ConnectionError::at("distribution", Reason::Invalid));
        }
        let user = self
            .user
            .as_deref()
            .map(str::trim)
            .filter(|user| !user.is_empty());
        if let Some(user) = user {
            if user.len() > MAX_WSL_USER_BYTES {
                return Err(ConnectionError::at("user", Reason::TooLong));
            }
            if !is_wsl_user(user) {
                return Err(ConnectionError::at("user", Reason::Invalid));
            }
        }
        let mut hop = WslHop::new(distribution);
        hop.user = user.map(Into::into);
        Ok(hop)
    }
}

/// Local command line that prints the installed distributions, one name per
/// line (`wsl.exe --list --quiet`).
pub fn list_command() -> Vec<String> {
    vec![wsl_program(), "--list".into(), "--quiet".into()]
}

/// The distribution names in the output of [`list_command`], in the order
/// `wsl.exe` printed them.
///
/// Reads UTF-16LE, with or without a byte order mark, and UTF-8. Leaves out
/// Docker Desktop's engine distributions, repeated names (WSL compares names
/// without regard to case), and lines that are not distribution names, such
/// as the message `wsl.exe` prints when none is installed. At most
/// [`MAX_LIST_BYTES`] of output and [`MAX_DISTRIBUTIONS`] names are read.
pub fn parse_distributions(output: &[u8]) -> Vec<String> {
    let output = &output[..output.len().min(MAX_LIST_BYTES)];
    let text = output_text(output);
    let mut names: Vec<String> = Vec::new();
    for line in text.lines() {
        let name = line.trim_matches(|character: char| {
            character.is_whitespace() || character == '\0' || character == '\u{feff}'
        });
        let known = |known: &str| known.eq_ignore_ascii_case(name);
        if !is_wsl_distribution(name)
            || ENGINE_DISTRIBUTIONS.iter().any(|engine| known(engine))
            || names.iter().any(|seen| known(seen))
        {
            continue;
        }
        names.push(name.into());
        if names.len() == MAX_DISTRIBUTIONS {
            break;
        }
    }
    names
}

/// What `wsl.exe` itself printed, as text: UTF-16LE when it starts with that
/// byte order mark or its second byte is NUL, which no UTF-8 text has, or
/// when it is not UTF-8 and has a NUL at an odd offset, as UTF-16LE text
/// that opens with a character outside ASCII, such as a message in Chinese,
/// has at its line ends; UTF-8 otherwise. Malformed sequences become
/// replacement characters, which no distribution name has.
pub fn output_text(output: &[u8]) -> String {
    let utf16 = match output {
        [0xff, 0xfe, rest @ ..] => Some(rest),
        [_, 0, ..] => Some(output),
        _ if std::str::from_utf8(output).is_err()
            && output.iter().skip(1).step_by(2).any(|byte| *byte == 0) =>
        {
            Some(output)
        }
        _ => None,
    };
    match utf16 {
        Some(bytes) => char::decode_utf16(
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair)),
        )
        .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect(),
        None => String::from_utf8_lossy(output).into_owned(),
    }
}

/// What `wsl.exe` printed when it gave up before its command started, as
/// text. It reports its own failures, with a stable error code such as
/// `Wsl/Service/WSL_E_DISTRO_NOT_FOUND`, on standard output; a distribution
/// that started may add a line on standard error. Standard output comes
/// first, each part read as [`output_text`] reads it and blank parts left
/// out.
pub fn failure_text(stdout: &[u8], stderr: &[u8]) -> String {
    [stdout, stderr]
        .iter()
        .map(|part| output_text(part).trim().to_owned())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(text: &str, bom: bool) -> Vec<u8> {
        let mut bytes = if bom { vec![0xff, 0xfe] } else { Vec::new() };
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        bytes
    }

    #[test]
    fn listings_read_utf16_with_or_without_a_byte_order_mark_and_utf8() {
        let listing = "Ubuntu-24.04\r\nDebian\r\n";
        let expected = vec!["Ubuntu-24.04".to_owned(), "Debian".to_owned()];
        assert_eq!(parse_distributions(&utf16(listing, false)), expected);
        assert_eq!(parse_distributions(&utf16(listing, true)), expected);
        assert_eq!(parse_distributions(listing.as_bytes()), expected);
        // WSL_UTF8 output may still end each line with a NUL.
        assert_eq!(
            parse_distributions(b"Ubuntu-24.04\0\r\n\r\nDebian\n"),
            expected
        );
        assert!(parse_distributions(b"").is_empty());
        assert!(parse_distributions(&[0xff, 0xfe]).is_empty());
        // Messages decode the same way, so diagnostics stay readable.
        let message = "There is no distribution with the supplied name.\r\n";
        assert_eq!(output_text(&utf16(message, false)), message);
        assert_eq!(output_text(message.as_bytes()), message);
        assert_eq!(output_text(b"x"), "x");
        // A message in Chinese opens with a character whose second byte is
        // not NUL; its line ends and error code still are.
        let chinese =
            "不存在具有所提供名称的分发。\r\n错误代码： Wsl/Service/WSL_E_DISTRO_NOT_FOUND\r\n";
        assert_eq!(output_text(&utf16(chinese, false)), chinese);
        assert_eq!(output_text(chinese.as_bytes()), chinese);
        // UTF-8 with a stray NUL stays UTF-8.
        assert_eq!(output_text(b"Alpine1\0\r\n"), "Alpine1\0\r\n");
    }

    // Real `wsl.exe` prints its failure on standard output; the line on
    // standard error comes from the distribution it started.
    #[test]
    fn failures_read_standard_output_then_standard_error() {
        let message = "User not found.\r\nError code: Wsl/WSL_E_USER_NOT_FOUND\r\n";
        let relay =
            b"<3>WSL (342 - Relay) ERROR: CreateProcessParseCommon:1067: getpwnam(dev) failed 17\n";
        assert_eq!(
            failure_text(&utf16(message, false), relay),
            "User not found.\r\nError code: Wsl/WSL_E_USER_NOT_FOUND\n\
             <3>WSL (342 - Relay) ERROR: CreateProcessParseCommon:1067: getpwnam(dev) failed 17"
        );
        assert_eq!(failure_text(message.as_bytes(), b""), message.trim());
        assert_eq!(
            failure_text(b"\r\n", relay),
            String::from_utf8_lossy(relay).trim()
        );
        assert_eq!(failure_text(b"", b""), "");
    }

    #[test]
    fn listings_leave_out_engine_distributions_repeats_and_messages() {
        let listing = "docker-desktop\r\nUbuntu\r\nDOCKER-DESKTOP-DATA\r\nubuntu\r\n\
            Windows Subsystem for Linux has no installed distributions.\r\n\
            https://aka.ms/wslstore\r\n-d\r\nAlpine\r\n";
        assert_eq!(
            parse_distributions(&utf16(listing, false)),
            vec!["Ubuntu".to_owned(), "Alpine".to_owned()]
        );
        let many: String = (0..MAX_DISTRIBUTIONS + 5)
            .map(|index| format!("Distro{index}\n"))
            .collect();
        let names = parse_distributions(many.as_bytes());
        assert_eq!(names.len(), MAX_DISTRIBUTIONS);
        assert_eq!(names[0], "Distro0");
        // Output past the bound is not read.
        let mut long = "a".repeat(MAX_LIST_BYTES).into_bytes();
        long.extend(b"\nLate\n");
        assert!(parse_distributions(&long).is_empty());
    }

    #[test]
    fn connections_become_a_hop_after_trimming_and_refuse_malformed_fields() {
        let connection = |distribution: &str, user: Option<&str>| WslConnection {
            distribution: distribution.into(),
            user: user.map(Into::into),
        };
        let hop = connection(" Ubuntu-24.04 ", Some(" dev ")).hop().unwrap();
        assert_eq!(hop.distribution, "Ubuntu-24.04");
        assert_eq!(hop.user.as_deref(), Some("dev"));
        assert_eq!(hop.program, wsl_program());
        assert_eq!(connection("Debian", Some("  ")).hop().unwrap().user, None);

        let long_distribution = "d".repeat(MAX_WSL_DISTRIBUTION_BYTES + 1);
        let long_user = "u".repeat(MAX_WSL_USER_BYTES + 1);
        for (refused, field, reason) in [
            (connection(" ", None), "distribution", Reason::Required),
            (
                connection(&long_distribution, None),
                "distribution",
                Reason::TooLong,
            ),
            (connection("-d", None), "distribution", Reason::Invalid),
            (
                connection("Ubuntu 24", None),
                "distribution",
                Reason::Invalid,
            ),
            (
                connection("Ubuntu", Some(&long_user)),
                "user",
                Reason::TooLong,
            ),
            (connection("Ubuntu", Some("-u")), "user", Reason::Invalid),
            (connection("Ubuntu", Some("a b")), "user", Reason::Invalid),
        ] {
            assert_eq!(
                refused.hop(),
                Err(ConnectionError::at(field, reason)),
                "{refused:?}"
            );
        }
    }

    #[test]
    fn requests_name_only_the_distribution_and_the_user() {
        let parsed: WslConnection =
            serde_json::from_value(serde_json::json!({"distribution": "Ubuntu"})).unwrap();
        assert_eq!(parsed.user, None);
        assert!(serde_json::from_value::<WslConnection>(
            serde_json::json!({"distribution": "Ubuntu", "program": "C:\\evil.exe"})
        )
        .is_err());
    }

    #[test]
    fn the_listing_runs_the_system_wsl_program_quietly() {
        assert_eq!(
            list_command(),
            vec![wsl_program(), "--list".into(), "--quiet".into()]
        );
    }
}
