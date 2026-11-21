//! Addresses typed into the ports page.
//!
//! The ports page takes a service on the remote host in whichever form the
//! user has it, and turns it into the TCP target a forward reaches:
//!
//! - `3000`: a port on the host's loopback interface, written `localhost`.
//! - `host:port`, with brackets around an IPv6 address, such as `[::1]:3000`
//!   or `db.internal:5432`.
//! - A URL, `scheme://host[:port][/path][?query][#fragment]`. Without a port
//!   the scheme's own applies: 80 for `http` and `ws`, 443 for `https` and
//!   `wss`; any other scheme must name its port. The scheme and what follows
//!   the authority are kept, so the page opens the forwarded address with the
//!   same path.
//!
//! An address without a scheme may also carry a path, as in
//! `localhost:3000/app`. An authority of digits only is a port in either
//! form, so `http://3000` reaches `localhost:3000`.
//!
//! A URL with a user name or password (`user:secret@host`) is refused rather
//! than stripped: the page shows and keeps the address, and a secret must
//! never end up there (design S12). Hosts are only checked, not resolved;
//! whether the target is the host's own loopback interface, which decides
//! whether the user has to confirm it (design S28), is the forward's rule.
//!
//! Note: .agents/notes/proposed/architecture/2026-10-24-remote-development-workspace-host.md

use crate::client::forward_set::is_target_host;
use serde::Serialize;
use std::fmt;

/// Longest address accepted, in bytes, so that a pasted document is refused
/// instead of kept.
pub const MAX_ADDRESS_BYTES: usize = 2048;

/// Longest URL scheme accepted, in bytes.
const MAX_SCHEME_BYTES: usize = 32;

/// What a forward reaches, as typed on the ports page.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardAddress {
    /// Host as the remote host resolves it: an IP address, an IPv6 one
    /// without brackets, or a name in lower case; `localhost` for a port
    /// alone.
    pub target_host: String,
    /// TCP port, 1 to 65535.
    pub target_port: u16,
    /// URL scheme in lower case, when the address was a URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
    /// What followed the host and port: path, query and fragment, such as
    /// `/app?tab=1`. Absent when nothing or only `/` followed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Why an address was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressError {
    /// Nothing but white space.
    Empty,
    /// Longer than [`MAX_ADDRESS_BYTES`].
    TooLong,
    /// White space or a control character inside the address.
    Malformed,
    /// A scheme that is not `letter *(letter / digit / + / - / .)`.
    Scheme,
    /// A URL with a user name or password.
    Credentials,
    /// The host is not an IP address or a host name.
    Host,
    /// The port is missing, `0`, above 65535 or not a number.
    Port,
}

impl AddressError {
    /// The reason as the API reports it.
    pub fn reason(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::TooLong => "tooLong",
            Self::Malformed => "malformed",
            Self::Scheme => "scheme",
            Self::Credentials => "credentials",
            Self::Host => "host",
            Self::Port => "port",
        }
    }
}

impl fmt::Display for AddressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid forward address: {}", self.reason())
    }
}

impl std::error::Error for AddressError {}

/// Reads an address typed into the ports page; surrounding white space is
/// ignored.
pub fn parse(text: &str) -> Result<ForwardAddress, AddressError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(AddressError::Empty);
    }
    if text.len() > MAX_ADDRESS_BYTES {
        return Err(AddressError::TooLong);
    }
    if text
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(AddressError::Malformed);
    }
    let (scheme, rest) = match text.split_once("://") {
        Some((scheme, rest)) => (Some(parse_scheme(scheme)?), rest),
        None => (None, text),
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(end);
    if authority.contains('@') {
        return Err(AddressError::Credentials);
    }
    let (target_host, target_port) = parse_authority(authority, scheme.as_deref())?;
    let path = (!path.is_empty() && path != "/").then(|| path.to_owned());
    Ok(ForwardAddress {
        target_host,
        target_port,
        scheme,
        path,
    })
}

/// The scheme in lower case, when it is one RFC 3986 allows.
fn parse_scheme(scheme: &str) -> Result<String, AddressError> {
    let mut bytes = scheme.bytes();
    let valid = scheme.len() <= MAX_SCHEME_BYTES
        && bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || b"+-.".contains(&byte));
    if !valid {
        return Err(AddressError::Scheme);
    }
    Ok(scheme.to_ascii_lowercase())
}

/// The host and port of `authority`, with `scheme`'s port when it names none.
fn parse_authority(authority: &str, scheme: Option<&str>) -> Result<(String, u16), AddressError> {
    if !authority.is_empty() && authority.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(("localhost".into(), parse_port(authority)?));
    }
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (inner, after) = rest.split_once(']').ok_or(AddressError::Host)?;
        // Brackets only ever enclose an IPv6 address.
        if inner.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(AddressError::Host);
        }
        let port = match after {
            "" => None,
            _ => Some(after.strip_prefix(':').ok_or(AddressError::Host)?),
        };
        (inner, port)
    } else {
        match authority.split_once(':') {
            // A second colon is an IPv6 address without brackets, which
            // cannot be told apart from its port.
            Some((_, port)) if port.contains(':') => return Err(AddressError::Host),
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    if host.is_empty() || !is_target_host(host) {
        return Err(AddressError::Host);
    }
    let port = match port {
        Some(port) => parse_port(port)?,
        None => scheme.and_then(default_port).ok_or(AddressError::Port)?,
    };
    Ok((host.to_ascii_lowercase(), port))
}

/// A port from 1 to 65535, in decimal digits only.
fn parse_port(text: &str) -> Result<u16, AddressError> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(AddressError::Port);
    }
    match text.parse::<u16>() {
        Ok(port) if port != 0 => Ok(port),
        _ => Err(AddressError::Port),
    }
}

/// The port a URL of `scheme` uses when it names none.
fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(text: &str) -> ForwardAddress {
        parse(text).unwrap_or_else(|error| panic!("{text}: {error}"))
    }

    fn target(text: &str) -> (String, u16) {
        let address = address(text);
        (address.target_host, address.target_port)
    }

    #[test]
    fn a_port_alone_is_on_the_host_loopback_interface() {
        assert_eq!(target("3000"), ("localhost".into(), 3000));
        assert_eq!(target("  8080\n"), ("localhost".into(), 8080));
        assert_eq!(target("65535"), ("localhost".into(), 65535));
        assert_eq!(parse("0"), Err(AddressError::Port));
        assert_eq!(parse("65536"), Err(AddressError::Port));
        assert_eq!(parse("99999999999999999999"), Err(AddressError::Port));
    }

    #[test]
    fn host_and_port_name_addresses_and_host_names() {
        assert_eq!(target("127.0.0.1:5432"), ("127.0.0.1".into(), 5432));
        assert_eq!(target("0.0.0.0:8080"), ("0.0.0.0".into(), 8080));
        assert_eq!(target("[::1]:3000"), ("::1".into(), 3000));
        assert_eq!(target("[FE80::1]:3000"), ("fe80::1".into(), 3000));
        assert_eq!(target("DB.Internal:5432"), ("db.internal".into(), 5432));
        assert_eq!(target("api_v2.local:9000"), ("api_v2.local".into(), 9000));
        // A bare IPv6 address cannot be told apart from its port.
        assert_eq!(parse("::1:3000"), Err(AddressError::Host));
        assert_eq!(parse("fe80::1"), Err(AddressError::Host));
        assert_eq!(parse("[::1]3000"), Err(AddressError::Host));
        assert_eq!(parse("[127.0.0.1]:80"), Err(AddressError::Host));
        assert_eq!(parse("[::1"), Err(AddressError::Host));
        assert_eq!(parse(":3000"), Err(AddressError::Host));
        assert_eq!(parse("-oProxy:22"), Err(AddressError::Host));
        assert_eq!(parse("db%2Einternal:5432"), Err(AddressError::Host));
        // Without a scheme, a host needs its port.
        assert_eq!(parse("localhost"), Err(AddressError::Port));
        assert_eq!(parse("localhost:"), Err(AddressError::Port));
        assert_eq!(parse("localhost:http"), Err(AddressError::Port));
        assert_eq!(parse("localhost:+80"), Err(AddressError::Port));
    }

    #[test]
    fn urls_keep_their_scheme_and_path_and_default_the_port() {
        assert_eq!(
            address("http://localhost:3000/app?tab=1#top"),
            ForwardAddress {
                target_host: "localhost".into(),
                target_port: 3000,
                scheme: Some("http".into()),
                path: Some("/app?tab=1#top".into()),
            }
        );
        assert_eq!(
            address("HTTPS://Example.com"),
            ForwardAddress {
                target_host: "example.com".into(),
                target_port: 443,
                scheme: Some("https".into()),
                path: None,
            }
        );
        assert_eq!(target("http://10.0.0.5/"), ("10.0.0.5".into(), 80));
        assert_eq!(target("ws://[::1]"), ("::1".into(), 80));
        assert_eq!(target("wss://socket.local"), ("socket.local".into(), 443));
        assert_eq!(
            address("http://localhost:3000/").path,
            None,
            "a lone slash opens the same page as none"
        );
        assert_eq!(
            address("http://localhost:3000?debug").path,
            Some("?debug".into())
        );
        assert_eq!(
            target("postgres://db.internal:5432/app"),
            ("db.internal".into(), 5432)
        );
        // A scheme without a default port must name one.
        assert_eq!(parse("postgres://db.internal/app"), Err(AddressError::Port));
        assert_eq!(parse("http://"), Err(AddressError::Host));
        assert_eq!(parse("http://:8080"), Err(AddressError::Host));
        assert_eq!(parse("1http://host"), Err(AddressError::Scheme));
        assert_eq!(parse("ht_tp://host"), Err(AddressError::Scheme));
        assert_eq!(parse("://host:80"), Err(AddressError::Scheme));
    }

    #[test]
    fn digits_alone_are_a_port_with_or_without_a_scheme() {
        assert_eq!(address("http://3000").target_port, 3000);
        assert_eq!(address("http://3000").target_host, "localhost");
        assert_eq!(
            address("localhost:3000/app").path,
            Some("/app".into()),
            "an address without a scheme keeps its path"
        );
        assert_eq!(address("localhost:3000/app").scheme, None);
    }

    #[test]
    fn credentials_are_refused_rather_than_kept() {
        assert_eq!(
            parse("https://user:secret@example.com"),
            Err(AddressError::Credentials)
        );
        assert_eq!(
            parse("postgres://user@db:5432"),
            Err(AddressError::Credentials)
        );
        assert_eq!(parse("user@db:5432"), Err(AddressError::Credentials));
        // An @ after the authority is part of the path.
        assert_eq!(
            address("http://localhost:3000/@user").path,
            Some("/@user".into())
        );
    }

    #[test]
    fn white_space_control_characters_and_size_are_bounded() {
        assert_eq!(parse(""), Err(AddressError::Empty));
        assert_eq!(parse(" \t\n"), Err(AddressError::Empty));
        assert_eq!(parse("local host:80"), Err(AddressError::Malformed));
        assert_eq!(
            parse("http://localhost:80/a b"),
            Err(AddressError::Malformed)
        );
        assert_eq!(parse("localhost:80\u{7}"), Err(AddressError::Malformed));
        let long = format!("http://localhost:80/{}", "a".repeat(MAX_ADDRESS_BYTES));
        assert_eq!(parse(&long), Err(AddressError::TooLong));
        let scheme = format!("{}://localhost:80", "h".repeat(MAX_SCHEME_BYTES + 1));
        assert_eq!(parse(&scheme), Err(AddressError::Scheme));
    }

    #[test]
    fn reasons_are_distinct() {
        let reasons = [
            AddressError::Empty,
            AddressError::TooLong,
            AddressError::Malformed,
            AddressError::Scheme,
            AddressError::Credentials,
            AddressError::Host,
            AddressError::Port,
        ]
        .map(AddressError::reason);
        let mut unique = reasons.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), reasons.len());
    }
}
