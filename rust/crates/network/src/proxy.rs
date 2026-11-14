//! Normalization and validation for `network.proxyPolicy`.
//!
//! The settings layer may hold untrimmed or incomplete values; this module is
//! the single authority that turns them into the policy a network stack must
//! apply. A custom server that is missing its host or port normalizes to the
//! system policy instead of silently breaking every request, and credentials
//! are only carried for a configured custom proxy so a later mode switch can
//! never forward them to a different proxy.

use serde::{Deserialize, Serialize};

use crate::exclusions::{parse, ProxyExclusionRule};

/// How the application's own HTTP traffic reaches the internet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NetworkProxyMode {
    /// Resolve proxies through the operating system configuration.
    System,
    /// Contact every host directly, ignoring configured system proxies.
    Direct,
    /// Tunnel requests through the user-configured server.
    Custom,
}

/// The protocol a custom proxy server speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NetworkProxyType {
    /// HTTP proxy; also carries HTTPS requests through CONNECT.
    Http,
    /// SOCKS5 proxy; tunnels every protocol and port.
    Socks5,
}

/// Request for `network.proxyPolicy`: the stored proxy settings exactly as
/// the settings layer holds them. Values may be untrimmed or incomplete;
/// normalization happens here. `port` must fit in 16 bits or the dispatcher
/// rejects the request as invalid.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkProxyPolicyRequest {
    /// Mode the user selected.
    pub mode: NetworkProxyMode,
    /// Proxy protocol for the custom mode; defaults to HTTP when absent.
    #[serde(default)]
    pub proxy_type: Option<NetworkProxyType>,
    /// Custom proxy host; surrounding whitespace is ignored.
    #[serde(default)]
    pub host: String,
    /// Custom proxy port; zero means not configured.
    #[serde(default)]
    pub port: u16,
    /// Proxy account; surrounding whitespace is ignored and empty means none.
    #[serde(default)]
    pub username: String,
    /// Proxy password, forwarded only when `password_identity` matches the server.
    #[serde(default)]
    pub password: String,
    /// Identity stored with a restored password; absent for an explicit draft.
    #[serde(default)]
    pub password_identity: Option<NetworkProxyIdentity>,
    /// Hosts that bypass the proxy; blank entries are dropped.
    #[serde(default)]
    pub exclusions: Vec<String>,
}

/// The endpoint and account that own a saved proxy password. Hosts persist
/// this together with the secret; changing preferences must not rebind it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkProxyIdentity {
    /// Proxy protocol the password was saved for.
    pub proxy_type: NetworkProxyType,
    /// Proxy host the password was saved for, compared case-insensitively.
    pub host: String,
    /// Proxy port the password was saved for.
    pub port: u16,
    /// Account the password belongs to.
    pub username: String,
}

/// The policy kind a network stack must apply after normalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NetworkProxyPolicyKind {
    /// Resolve proxies through the operating system configuration.
    System,
    /// Contact every host directly; an empty proxy dictionary.
    Direct,
    /// Tunnel requests through the normalized custom server.
    Custom,
}

/// Normalized policy for `network.proxyPolicy`. Credentials are only
/// populated for `policy == custom`; system and direct never carry them, so
/// hosts can forward the plan to their network stack without a second
/// credential-scoping decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkProxyPolicy {
    /// Policy the network stack applies.
    pub policy: NetworkProxyPolicyKind,
    /// Protocol of the custom server; HTTP when the request named none.
    pub proxy_type: NetworkProxyType,
    /// Trimmed custom host; empty unless `policy` is custom.
    pub host: String,
    /// Custom port; zero unless `policy` is custom.
    pub port: u16,
    /// Trimmed account; empty unless `policy` is custom.
    pub username: String,
    /// Password; empty unless `policy` is custom and the saved identity matches the server.
    pub password: String,
    /// Trimmed, non-empty bypass hosts; empty unless `policy` is custom.
    pub exclusions: Vec<String>,
    /// `exclusions` read into the rules a network stack compares (`bypasses`).
    pub exclusion_rules: Vec<ProxyExclusionRule>,
    /// Entries of `exclusions` that have none of the supported forms and send no host direct.
    pub invalid_exclusions: Vec<String>,
    /// True when the caller-selected mode could actually be honored; false
    /// for an incomplete custom server that normalized to the system policy.
    pub configured: bool,
}

/// Normalizes stored proxy settings into the policy a network stack applies.
pub fn proxy_policy(request: NetworkProxyPolicyRequest) -> NetworkProxyPolicy {
    let exclusions: Vec<String> = request
        .exclusions
        .iter()
        .map(|entry| entry.trim().to_string())
        .filter(|entry| !entry.is_empty())
        .collect();
    let proxy_type = request.proxy_type.unwrap_or(NetworkProxyType::Http);
    let empty = NetworkProxyPolicy {
        policy: NetworkProxyPolicyKind::System,
        proxy_type,
        host: String::new(),
        port: 0,
        username: String::new(),
        password: String::new(),
        exclusions: Vec::new(),
        exclusion_rules: Vec::new(),
        invalid_exclusions: Vec::new(),
        configured: false,
    };
    match request.mode {
        NetworkProxyMode::System => empty,
        NetworkProxyMode::Direct => NetworkProxyPolicy {
            policy: NetworkProxyPolicyKind::Direct,
            ..empty
        },
        NetworkProxyMode::Custom => {
            let host = request.host.trim();
            let configured = !host.is_empty() && request.port > 0;
            if !configured {
                return empty;
            }
            let username = request.username.trim();
            let password_matches = request.password_identity.as_ref().is_none_or(|identity| {
                identity.proxy_type == proxy_type
                    && identity.host.trim().eq_ignore_ascii_case(host)
                    && identity.port == request.port
                    && identity.username.trim() == username
            });
            let parsed = parse(&exclusions);
            NetworkProxyPolicy {
                policy: NetworkProxyPolicyKind::Custom,
                proxy_type,
                host: host.to_string(),
                port: request.port,
                username: username.to_string(),
                password: if password_matches {
                    request.password
                } else {
                    String::new()
                },
                exclusions,
                exclusion_rules: parsed.rules,
                invalid_exclusions: parsed.invalid,
                configured: true,
            }
        }
    }
}
