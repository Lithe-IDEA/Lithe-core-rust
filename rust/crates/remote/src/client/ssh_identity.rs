//! Identity evidence from the actual OpenSSH process, never from `ssh -G`.
//!
//! The caller captures OpenSSH's own `-E` log in an owner-only directory.
//! Remote banners, command stderr and proxy stderr cannot write this channel.
//! Only source-tagged records of the locally spawned PID are considered. An
//! unfamiliar format, a proxy route or incomplete evidence has no reusable
//! identity; callers must use a connection-local trust key and send no token.

use serde::Serialize;
use sha2::{Digest, Sha256};

/// Maximum evidence read for one connection. Overflow fails closed.
pub const MAX_BYTES: usize = 64 * 1024;

/// Overrides only logging and connection multiplexing, never host-key policy.
pub const OPTIONS: &[&str] = &[
    "ControlMaster=no",
    "ControlPath=none",
    "FingerprintHash=sha256",
    "LogLevel=ERROR",
    "LogVerbose=sshconnect.c:ssh_login():*,sshconnect.c:verify_host_key():*,sshconnect.c:ssh_connect_direct():*,sshconnect.c:ssh_proxy_connect():*,sshconnect.c:ssh_proxy_fdpass_connect():*,sshconnect2.c:ssh_userauth2():*",
];

/// Validated facts from one successfully authenticated direct connection.
#[derive(Debug, Eq, PartialEq, Serialize)]
pub struct Identity {
    host: String,
    port: u16,
    user: String,
    host_key: String,
    peer: String,
}

/// Reads only OpenSSH's own records for `pid`. Success must follow both the
/// selected host key and account, on a direct socket. It proves which key
/// OpenSSH accepted under the user's existing host-key policy.
pub fn parse(bytes: &[u8], pid: u32) -> Option<Identity> {
    if bytes.len() > MAX_BYTES {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let pid_value = pid.to_string();
    let mut account = None;
    let mut key = None;
    let mut peer = None;
    let mut authenticated = false;
    for line in text.lines() {
        let Some((tag, remainder)) = line.split_once(" (") else {
            continue;
        };
        let Some((origin, message)) = remainder.split_once("): ") else {
            continue;
        };
        let origin_pid = origin.strip_prefix("pid=").or_else(|| {
            // Apple's OpenSSH also tags the binary path in its local log.
            let (binary, pid) = origin.strip_prefix("bin=")?.rsplit_once(", pid=")?;
            (binary.starts_with('/') && field(binary)).then_some(pid)
        });
        if origin_pid != Some(pid_value.as_str()) {
            continue;
        }
        let tag = tag.strip_prefix("debug1: ").unwrap_or(tag);
        let Some((function, line_number)) = tag.rsplit_once(':') else {
            continue;
        };
        if line_number.is_empty() || !line_number.bytes().all(|c| c.is_ascii_digit()) {
            continue;
        }
        match function {
            "sshconnect.c:ssh_proxy_connect()" | "sshconnect.c:ssh_proxy_fdpass_connect()" => {
                return None
            }
            "sshconnect.c:ssh_connect_direct()" if message.starts_with("Connecting to ") => {
                let address = message.strip_prefix("Connecting to ")?.strip_suffix('.')?;
                let (address, port) = address.rsplit_once(" port ")?;
                let (_, ip) = address.rsplit_once(" [")?;
                let ip = ip.strip_suffix(']')?.parse::<std::net::IpAddr>().ok()?;
                peer = Some((ip.to_string(), port.parse::<u16>().ok()?));
            }
            "sshconnect.c:ssh_login()" if message.starts_with("Authenticating to ") => {
                if account.is_some() {
                    return None;
                }
                let (address, user) = message
                    .strip_prefix("Authenticating to ")?
                    .rsplit_once(" as '")?;
                let user = user.strip_suffix('\'')?;
                let (host, port) = address.rsplit_once(':')?;
                if !field(host) || !field(user) {
                    return None;
                }
                account = Some((host.to_owned(), port.parse::<u16>().ok()?, user.to_owned()));
            }
            "sshconnect.c:verify_host_key()" if !authenticated => {
                let value = message
                    .strip_prefix("Server host key: ")
                    .or_else(|| message.strip_prefix("Server host certificate: "));
                if let Some(value) = value {
                    let mut fields = value.split_whitespace();
                    let kind = fields.next()?;
                    let fingerprint = fields.next()?.trim_end_matches(',');
                    if !field(kind)
                        || !fingerprint
                            .strip_prefix("SHA256:")?
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'='))
                        || !(50..=51).contains(&fingerprint.len())
                    {
                        return None;
                    }
                    if key.replace(format!("{kind} {fingerprint}")).is_some() {
                        return None;
                    }
                }
            }
            "sshconnect2.c:ssh_userauth2()" if message.starts_with("Authenticated to ") => {
                if authenticated
                    || key.is_none()
                    || account.is_none()
                    || message.contains("(via proxy)")
                {
                    return None;
                }
                authenticated = true;
            }
            _ => {}
        }
    }
    let (host, port, user) = account?;
    let (peer, peer_port) = peer?;
    if !authenticated || port == 0 || peer_port != port {
        return None;
    }
    Some(Identity {
        host,
        port,
        user,
        host_key: key?,
        peer,
    })
}

fn field(value: &str) -> bool {
    !value.is_empty() && value.len() <= 1024 && !value.chars().any(|c| c.is_control() || c == '\\')
}

/// Domain-separated digest of canonical structured identity facts. It is
/// opaque storage identity, never a credential or a replacement for SSH.
pub fn digest(value: &impl Serialize) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"lithe.remote.identity.v1\0");
    hasher.update(serde_json::to_vec(value).expect("identity fields serialize"));
    crate::protocol::methods::encode_hex(&hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence() -> String {
        format!("debug1: sshconnect.c:ssh_connect_direct():10 (pid=7): Connecting to box [127.0.0.1] port 22.\n\
            debug1: sshconnect.c:ssh_login():20 (pid=7): Authenticating to box:22 as 'dev'\n\
            debug1: sshconnect.c:verify_host_key():30 (pid=7): Server host key: ssh-ed25519 SHA256:{}\n\
            sshconnect2.c:ssh_userauth2():40 (pid=7): Authenticated to box ([127.0.0.1]:22) using \"publickey\".\n", "a".repeat(43))
    }

    #[test]
    fn accepts_only_complete_actual_process_evidence() {
        let text = evidence();
        let identity = parse(text.as_bytes(), 7).unwrap();
        assert_eq!(
            parse(
                text.replace("(pid=7)", "(bin=/usr/bin/ssh, pid=7)")
                    .as_bytes(),
                7
            ),
            Some(identity)
        );
        let identity = parse(text.as_bytes(), 7).unwrap();
        assert_eq!(identity.host, "box");
        assert_eq!(identity.user, "dev");
        assert!(parse(text.as_bytes(), 8).is_none());
        assert!(parse(text.split("sshconnect2.c").next().unwrap().as_bytes(), 7).is_none());
        assert!(parse(text.replace(" (pid=7)", "").as_bytes(), 7).is_none());
        assert!(parse(
            text.replace("([127.0.0.1]:22)", "(via proxy)").as_bytes(),
            7
        )
        .is_none());
        assert!(parse(format!("{text}{}", "x".repeat(MAX_BYTES)).as_bytes(), 7).is_none());
        for changed in [
            text.replace("'dev'", "'ops'"),
            text.replace(&"a".repeat(43), &"b".repeat(43)),
            text.replace("box", "other"),
        ] {
            assert_ne!(
                digest(&identity),
                digest(&parse(changed.as_bytes(), 7).unwrap())
            );
        }
    }
}
