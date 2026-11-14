//! Network proxy policy normalization shared by the macOS and Windows hosts.
//!
//! The user-facing proxy settings (mode, server, credentials, exclusions) are
//! normalized and validated here so both hosts interpret the same stored
//! values identically. Platforms only map the returned policy onto their own
//! network stack: `connectionProxyDictionary` plus an authentication delegate
//! on macOS, and the HTTP client configuration on Windows. What an exclusion
//! entry means, and which hosts it sends direct, is decided in `exclusions`.

mod exclusions;
mod proxy;

pub use exclusions::{bypasses, ProxyExclusionKind, ProxyExclusionRule};
pub use proxy::{
    proxy_policy, NetworkProxyIdentity, NetworkProxyMode, NetworkProxyPolicy,
    NetworkProxyPolicyKind, NetworkProxyPolicyRequest, NetworkProxyType,
};

#[cfg(test)]
mod tests;
