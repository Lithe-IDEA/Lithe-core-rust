//! What each proxy exclusion entry means, and which hosts it sends direct.
//!
//! The settings page promises five forms: an exact host, a domain suffix written
//! `.example.com` or `*.example.com`, an IP address, a CIDR block, and `<local>` for names
//! without a dot. This module is the only place that reads those forms. `parse` turns the
//! entries into rules a network stack compares with plain equality, suffix and address-block
//! checks; `bypasses` is that comparison, which hosts repeat exactly as the contract states.
//! Fixtures live in `rust/fixtures/network/proxy-bypass-v1.json`.

use std::net::{IpAddr, Ipv6Addr};

use serde::Serialize;

/// The form of one exclusion rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProxyExclusionKind {
    /// The host equals `value`.
    Host,
    /// The host is `value` or ends with `.` followed by `value`.
    Suffix,
    /// The host is an IP address equal to `value`.
    Ip,
    /// The host is an IP address inside the block `value`.
    Cidr,
    /// The host is a name without a dot; `value` is empty.
    Local,
}

/// One exclusion entry in the form a network stack compares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyExclusionRule {
    /// How the host is compared with `value`.
    pub kind: ProxyExclusionKind,
    /// Lowercase host or domain without a trailing dot, canonical IP address, or canonical
    /// network address with its prefix length; empty for `local`.
    pub value: String,
}

/// The exclusion entries of one policy, read.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParsedExclusions {
    /// Rules of the entries that have one of the five forms, in entry order.
    pub rules: Vec<ProxyExclusionRule>,
    /// Trimmed entries that have none of the forms; they send no host direct.
    pub invalid: Vec<String>,
}

/// Reads `entries`, which are trimmed and not empty. A repeated rule is kept once.
pub fn parse(entries: &[String]) -> ParsedExclusions {
    let mut parsed = ParsedExclusions::default();
    for entry in entries {
        match rule(entry) {
            Some(rule) => {
                if !parsed.rules.contains(&rule) {
                    parsed.rules.push(rule);
                }
            }
            None => parsed.invalid.push(entry.clone()),
        }
    }
    parsed
}

/// Whether `host`, the host name or address of a request URL without brackets, goes direct
/// under `rules`. The host is compared lowercase and without a trailing dot; an empty host
/// never goes direct.
pub fn bypasses(host: &str, rules: &[ProxyExclusionRule]) -> bool {
    let host = normalized_name(host);
    if host.is_empty() {
        return false;
    }
    let address = host_address(&host);
    rules.iter().any(|rule| match rule.kind {
        ProxyExclusionKind::Host => host == rule.value,
        ProxyExclusionKind::Suffix => {
            host == rule.value
                || host
                    .strip_suffix(rule.value.as_str())
                    .is_some_and(|rest| rest.ends_with('.'))
        }
        ProxyExclusionKind::Ip => address.is_some_and(|address| {
            rule.value
                .parse::<IpAddr>()
                .is_ok_and(|value| value == address)
        }),
        ProxyExclusionKind::Cidr => {
            address.is_some_and(|address| block(&rule.value).is_some_and(|b| b.contains(address)))
        }
        ProxyExclusionKind::Local => address.is_none() && !host.contains('.'),
    })
}

/// The address `host` writes, an IPv6 address being allowed a non-empty `%zone`, which is not
/// compared; `None` for a name.
fn host_address(host: &str) -> Option<IpAddr> {
    if let Ok(address) = host.parse::<IpAddr>() {
        return Some(address);
    }
    let (address, zone) = host.split_once('%')?;
    if zone.is_empty() {
        return None;
    }
    address.parse::<Ipv6Addr>().ok().map(IpAddr::V6)
}

/// The rule of one entry, or `None` when it has none of the five forms.
fn rule(entry: &str) -> Option<ProxyExclusionRule> {
    let lowered = normalized_name(entry);
    if lowered == "<local>" {
        return Some(ProxyExclusionRule {
            kind: ProxyExclusionKind::Local,
            value: String::new(),
        });
    }
    if lowered.contains('/') {
        let block = block(&lowered)?;
        return Some(ProxyExclusionRule {
            kind: ProxyExclusionKind::Cidr,
            value: block.to_string(),
        });
    }
    let unbracketed = lowered
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(&lowered);
    if let Ok(address) = unbracketed.parse::<IpAddr>() {
        return Some(ProxyExclusionRule {
            kind: ProxyExclusionKind::Ip,
            value: address.to_string(),
        });
    }
    if let Some(domain) = lowered
        .strip_prefix("*.")
        .or_else(|| lowered.strip_prefix('.'))
    {
        return is_name(domain).then(|| ProxyExclusionRule {
            kind: ProxyExclusionKind::Suffix,
            value: domain.to_string(),
        });
    }
    is_name(&lowered).then_some(ProxyExclusionRule {
        kind: ProxyExclusionKind::Host,
        value: lowered,
    })
}

/// `value` trimmed, lowercase, without one trailing dot.
fn normalized_name(value: &str) -> String {
    let lowered = value.trim().to_lowercase();
    match lowered.strip_suffix('.') {
        Some(stripped) => stripped.to_string(),
        None => lowered,
    }
}

/// Whether `value` can be a host name: not empty, and without wildcards, separators, ports
/// or white space. Labels are not checked further, so internal names stay usable.
fn is_name(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('.')
        && !value.contains("..")
        && !value
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '*' | '/' | ':' | '[' | ']' | '<' | '>'))
}

/// An address block written `address/prefix`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Block {
    network: IpAddr,
    prefix: u8,
}

impl Block {
    fn contains(self, address: IpAddr) -> bool {
        match (self.network, address) {
            (IpAddr::V4(network), IpAddr::V4(address)) => {
                masked(u128::from(u32::from(address)), self.prefix, 32)
                    == u128::from(u32::from(network))
            }
            (IpAddr::V6(network), IpAddr::V6(address)) => {
                masked(u128::from(address), self.prefix, 128) == u128::from(network)
            }
            _ => false,
        }
    }
}

impl std::fmt::Display for Block {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}/{}", self.network, self.prefix)
    }
}

/// The block `value` writes, with the host bits of its address cleared; `None` when the
/// address or the prefix length is not valid for the address family.
fn block(value: &str) -> Option<Block> {
    let (address, prefix) = value.split_once('/')?;
    let address = address.parse::<IpAddr>().ok()?;
    if prefix.is_empty() || !prefix.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let prefix: u8 = prefix.parse().ok()?;
    let network = match address {
        IpAddr::V4(address) => {
            if prefix > 32 {
                return None;
            }
            let bits = masked(u128::from(u32::from(address)), prefix, 32);
            IpAddr::V4(u32::try_from(bits).ok()?.into())
        }
        IpAddr::V6(address) => {
            if prefix > 128 {
                return None;
            }
            IpAddr::V6(masked(u128::from(address), prefix, 128).into())
        }
    };
    Some(Block { network, prefix })
}

/// `bits`, an address of `width` bits, with every bit after the first `prefix` cleared.
fn masked(bits: u128, prefix: u8, width: u8) -> u128 {
    if prefix == 0 {
        return 0;
    }
    let keep = u128::MAX << (u32::from(width) - u32::from(prefix));
    let width_mask = if width == 128 {
        u128::MAX
    } else {
        (1u128 << width) - 1
    };
    bits & keep & width_mask
}
