//! Linux host adapter, used for SSH hosts and containers.

use super::{unix, ChildKind};
use crate::protocol::methods::{ListeningPort, OperatingSystem, OrphanProtection};
use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

pub(crate) const OPERATING_SYSTEM: OperatingSystem = OperatingSystem::Linux;
pub(crate) const ORPHAN_PROTECTION: OrphanProtection = OrphanProtection::ParentDeathSignal;

/// TCP socket state `LISTEN` in `/proc/net/tcp`.
const TCP_LISTEN_STATE: &str = "0A";

/// Configures a child to lead its own session and process group and to be
/// killed by the kernel if the server dies.
///
/// `PR_SET_PDEATHSIG` fires when the *thread* that forked the child exits,
/// not the process, which is why every spawn runs on the spawner thread.
pub(crate) fn prepare_child(command: &mut Command, kind: ChildKind) {
    // SAFETY: getpid has no preconditions.
    let server = unsafe { libc::getpid() };
    // SAFETY: the closure only calls async-signal-safe functions, as required
    // between fork and exec.
    unsafe {
        command.pre_exec(move || {
            unix::start_new_session()?;
            if kind == ChildKind::Terminal {
                unix::acquire_controlling_terminal()?;
            }
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                return Err(io::Error::last_os_error());
            }
            // The signal is not delivered retroactively: if the server died
            // before prctl, the child has already been re-parented.
            if libc::getppid() != server {
                libc::_exit(1);
            }
            Ok(())
        });
    }
}

pub(crate) fn case_sensitive(path: &Path) -> Option<bool> {
    unix::probe_case_sensitivity(path)
}

/// Where the kernel lists `binfmt_misc` entries.
const BINFMT_MISC: &str = "/proc/sys/fs/binfmt_misc";

/// Entries WSL registers to start Windows programs. Some WSL releases name it
/// `WSLInterop-late` on distributions that run systemd; either or both can
/// exist.
const WSL_INTEROP_ENTRIES: [&str; 2] = ["WSLInterop", "WSLInterop-late"];

/// See [`crate::protocol::methods::HostCapabilities::windows_interop`].
pub(crate) fn windows_interop() -> Option<bool> {
    windows_interop_in(Path::new(BINFMT_MISC))
}

/// Reads the WSL entries below `directory`. The first line of a
/// `binfmt_misc` entry is `enabled` or `disabled`; an entry that cannot be
/// read is treated as absent.
fn windows_interop_in(directory: &Path) -> Option<bool> {
    let mut registered = false;
    for entry in WSL_INTEROP_ENTRIES {
        let Ok(text) = std::fs::read_to_string(directory.join(entry)) else {
            continue;
        };
        registered = true;
        if text.lines().next().map(str::trim) == Some("enabled") {
            return Some(true);
        }
    }
    registered.then_some(false)
}

/// Processes whose session id is `session`, read from `/proc/<pid>/stat`.
pub(crate) fn session_members(session: u32) -> Vec<u32> {
    process_sessions()
        .filter(|(_, member_session)| *member_session == session)
        .map(|(pid, _)| pid)
        .collect()
}

/// The sessions among `sessions` that have a member, from one scan.
pub(crate) fn sessions_with_members(sessions: &HashSet<u32>) -> HashSet<u32> {
    process_sessions()
        .map(|(_, session)| session)
        .filter(|session| sessions.contains(session))
        .collect()
}

/// `(pid, session id)` of every process.
fn process_sessions() -> impl Iterator<Item = (u32, u32)> {
    process_ids().filter_map(|pid| {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        Some((pid, stat_session(&stat)?))
    })
}

/// Every process id in `/proc`, lowest first as the kernel lists them.
fn process_ids() -> impl Iterator<Item = u32> {
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
}

/// The session id is the fourth field after the command name, which is
/// parenthesized and may itself contain spaces and parentheses.
fn stat_session(stat: &str) -> Option<u32> {
    let (_, after_name) = stat.rsplit_once(')')?;
    after_name.split_whitespace().nth(3)?.parse().ok()
}

/// Listening TCP sockets in the server's network namespace, which inside a
/// container is the container's own. Sockets of other users are included:
/// Docker's port proxies, for example, run as root. Their owner stays
/// unknown, as only the user's own processes can be inspected.
pub(crate) fn listening_ports() -> io::Result<Vec<ListeningPort>> {
    let mut sockets = Vec::new();
    for (table, ipv6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
        match std::fs::read_to_string(table) {
            Ok(contents) => sockets.extend(parse_proc_net_tcp(&contents, ipv6)),
            // Kernels without IPv6 have no tcp6 table.
            Err(error) if error.kind() == io::ErrorKind::NotFound && ipv6 => {}
            Err(error) => return Err(error),
        }
    }
    let owners = socket_owners(sockets.iter().filter_map(|socket| socket.inode).collect());
    Ok(sockets
        .into_iter()
        .map(|socket| {
            let owner = socket.inode.and_then(|inode| owners.get(&inode));
            ListeningPort {
                address: socket.address,
                port: socket.port,
                pid: owner.map(|owner| owner.pid),
                process: owner.and_then(|owner| owner.name.clone()),
            }
        })
        .collect())
}

/// A listening socket from `/proc/net/tcp`, before its owner is known.
#[derive(Debug, Eq, PartialEq)]
struct Socket {
    address: String,
    port: u16,
    /// Absent when the line has no inode or the kernel printed 0.
    inode: Option<u64>,
}

/// The process that holds a socket.
struct Owner {
    pid: u32,
    name: Option<String>,
}

/// Finds the processes holding the sockets with the inodes in `wanted`, by
/// reading their descriptor tables. `/proc/<pid>/fd` is readable only for
/// the server user's own processes, so other users' sockets keep no owner.
/// A socket that several processes inherited goes to the lowest process id,
/// usually the parent that opened it. The scan ends once every socket has an
/// owner.
fn socket_owners(mut wanted: HashSet<u64>) -> HashMap<u64, Owner> {
    let mut owners = HashMap::new();
    for pid in process_ids() {
        if wanted.is_empty() {
            break;
        }
        // Unreadable for other users' processes and gone for ones that just
        // exited; neither owns a socket we can name.
        let Ok(descriptors) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
            continue;
        };
        let mut name = None;
        for descriptor in descriptors.flatten() {
            let Some(inode) = std::fs::read_link(descriptor.path())
                .ok()
                .and_then(|target| socket_inode(&target))
            else {
                continue;
            };
            if wanted.remove(&inode) {
                let name = name.get_or_insert_with(|| process_name(pid)).clone();
                owners.insert(inode, Owner { pid, name });
            }
        }
    }
    owners
}

/// The inode of a descriptor link such as `socket:[12345]`.
fn socket_inode(target: &Path) -> Option<u64> {
    target
        .to_str()?
        .strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

/// The command name in `/proc/<pid>/comm`, at most 15 bytes.
fn process_name(pid: u32) -> Option<String> {
    let name = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let name = name.trim_end_matches('\n');
    (!name.is_empty()).then(|| name.to_owned())
}

/// Listening sockets of one `/proc/net/tcp` or `tcp6` table. The columns are
/// `sl local_address rem_address st tx_queue:rx_queue tr:tm->when retrnsmt
/// uid timeout inode`.
fn parse_proc_net_tcp(contents: &str, ipv6: bool) -> Vec<Socket> {
    contents
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let local = fields.nth(1)?;
            let state = fields.nth(1)?;
            if state != TCP_LISTEN_STATE {
                return None;
            }
            let inode = fields
                .nth(5)
                .and_then(|inode| inode.parse().ok())
                .filter(|inode| *inode != 0);
            let (address, port) = local.split_once(':')?;
            Some(Socket {
                address: if ipv6 {
                    parse_ipv6(address)?.to_string()
                } else {
                    parse_ipv4(address)?.to_string()
                },
                port: u16::from_str_radix(port, 16).ok()?,
                inode,
            })
        })
        .collect()
}

/// The kernel prints each 32-bit word of an address in host byte order.
fn parse_ipv4(hex: &str) -> Option<Ipv4Addr> {
    let word = u32::from_str_radix(hex, 16).ok()?;
    Some(Ipv4Addr::from(word.to_ne_bytes()))
}

fn parse_ipv6(hex: &str) -> Option<Ipv6Addr> {
    if hex.len() != 32 {
        return None;
    }
    let mut bytes = [0_u8; 16];
    for (index, chunk) in bytes.chunks_mut(4).enumerate() {
        let word = u32::from_str_radix(&hex[index * 8..index * 8 + 8], 16).ok()?;
        chunk.copy_from_slice(&word.to_ne_bytes());
    }
    Some(Ipv6Addr::from(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixture lines copied from a little-endian host: 127.0.0.1:8080 and
    // 0.0.0.0:22 listening, plus an established connection that is ignored.
    #[test]
    fn session_ids_are_read_after_the_parenthesized_name() {
        let stat = "4242 (my (odd) name) S 1 4242 4242 34816 4242 4194560 0";
        assert_eq!(stat_session(stat), Some(4242));
    }

    #[test]
    fn proc_net_tables_yield_listening_sockets_only() {
        let ipv4 = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1 1 0 100 0 0 10 0\n   1: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 2 1 0 100 0 0 10 0\n   2: 0100007F:1F90 0100007F:D431 01 00000000:00000000 00:00000000 00000000  1000        0 3 1 0 20 4 30 10 -1\n";
        let ports = parse_proc_net_tcp(ipv4, false);
        assert_eq!(
            ports,
            vec![
                Socket {
                    address: "127.0.0.1".into(),
                    port: 8080,
                    inode: Some(1),
                },
                Socket {
                    address: "0.0.0.0".into(),
                    port: 22,
                    inode: Some(2),
                },
            ]
        );
        // A table without the inode column, and one with inode 0, still
        // list the port, just without an owner.
        let ipv6 = "  sl  local_address remote_address st\n   0: 00000000000000000000000001000000:1538 00000000000000000000000000000000:0000 0A\n   1: 00000000000000000000000000000000:0050 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 0 1\n";
        assert_eq!(
            parse_proc_net_tcp(ipv6, true),
            vec![
                Socket {
                    address: "::1".into(),
                    port: 5432,
                    inode: None,
                },
                Socket {
                    address: "::".into(),
                    port: 80,
                    inode: None,
                },
            ]
        );
    }

    #[test]
    fn only_socket_descriptors_name_an_inode() {
        assert_eq!(socket_inode(Path::new("socket:[12345]")), Some(12345));
        for target in ["pipe:[12345]", "/dev/null", "socket:[]", "socket:[12345"] {
            assert_eq!(socket_inode(Path::new(target)), None, "{target}");
        }
    }

    // The test's own listener is in a process this user can inspect.
    #[test]
    fn listening_sockets_of_the_users_own_processes_name_their_owner() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let ports = listening_ports().unwrap();
        let own = ports
            .iter()
            .find(|listening| listening.port == port && listening.address == "127.0.0.1")
            .unwrap_or_else(|| panic!("{port} not in {ports:?}"));
        assert_eq!(own.pid, Some(std::process::id()));
        assert!(own.process.is_some(), "{own:?}");
    }

    /// A fresh directory standing in for `/proc/sys/fs/binfmt_misc`, holding
    /// `entries` as `(name, contents)`.
    fn binfmt_misc_with(entries: &[(&str, &str)]) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "lithe-remote-binfmt-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        for (name, contents) in entries {
            std::fs::write(directory.join(name), contents).unwrap();
        }
        directory
    }

    // Contents as WSL leaves them: the state line, then the interpreter.
    const ENABLED: &str = "enabled\ninterpreter /init\nflags: PF\noffset 0\nmagic 4d5a\n";
    const DISABLED: &str = "disabled\ninterpreter /init\nflags: PF\noffset 0\nmagic 4d5a\n";

    #[test]
    fn windows_interop_follows_whichever_wsl_entry_is_enabled() {
        let check = |entries: &[(&str, &str)], expected: Option<bool>| {
            let directory = binfmt_misc_with(entries);
            assert_eq!(windows_interop_in(&directory), expected, "{entries:?}");
            std::fs::remove_dir_all(&directory).unwrap();
        };
        check(&[("WSLInterop", ENABLED)], Some(true));
        check(&[("WSLInterop", DISABLED)], Some(false));
        check(&[("WSLInterop-late", ENABLED)], Some(true));
        check(
            &[("WSLInterop", DISABLED), ("WSLInterop-late", ENABLED)],
            Some(true),
        );
        check(
            &[("WSLInterop", DISABLED), ("WSLInterop-late", DISABLED)],
            Some(false),
        );
        // Not a state the kernel writes; a registered entry still counts.
        check(&[("WSLInterop", "unexpected\n")], Some(false));
        // Other interpreters do not make a host WSL.
        check(&[("python3.12", ENABLED), ("status", "enabled\n")], None);
    }

    #[test]
    fn windows_interop_is_unknown_without_binfmt_misc() {
        let directory = binfmt_misc_with(&[]);
        assert_eq!(windows_interop_in(&directory), None);
        std::fs::remove_dir_all(&directory).unwrap();
        assert_eq!(windows_interop_in(&directory), None);
    }
}
