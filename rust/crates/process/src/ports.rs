//! Listening TCP port attribution, normalization, and formatting for one run process tree.
//!
//! Hosts supply the OS process table and raw listening sockets for candidate
//! processes; Core filters sockets to the run's live process tree (ignoring
//! reused PIDs whose parent started later and any caller-excluded debug ports),
//! normalizes bind addresses, deduplicates dual-stack bindings, and formats
//! stable display labels and browser URLs. Fixtures live in
//! `rust/fixtures/process-lifecycle/listening-ports-v1.json`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};

/// Schema version of [`ListeningPortsResponse`].
pub const SCHEMA_VERSION: u32 = 1;

/// Default maximum number of listening ports returned in one response.
const DEFAULT_LIMIT: usize = 8;

/// Hard upper bound on listening ports returned in one response.
const MAX_LIMIT: usize = 32;

/// One row of the host's process table used to reconstruct the run's process tree.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PortProcess {
    /// Operating-system process ID.
    pub process_id: i64,
    /// Parent process ID reported by the operating system.
    pub parent_process_id: i64,
    /// Start time in microseconds since 1970. When both child and parent carry
    /// a start time and the parent started later, the parent ID was reused and
    /// the parent link is ignored.
    #[serde(default)]
    pub start_time: Option<u64>,
}

/// One raw listening TCP socket observed by the host.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ListeningSocket {
    /// Process ID that owns the listening socket descriptor.
    pub process_id: i64,
    /// Bound IP address string (IPv4, IPv6, bracketed IPv6, optional `%zone`,
    /// or `""` / `"*"` for unspecified wildcard).
    pub address: String,
    /// Bound TCP port number (`1..=65535` for live sockets).
    pub port: u16,
}

/// Request of `process.listeningPorts`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ListeningPortsRequest {
    /// Root process ID of the run session. Must be positive.
    pub root_process_id: i64,
    /// Process rows used to find descendants of [`Self::root_process_id`].
    #[serde(default)]
    pub processes: Vec<PortProcess>,
    /// Candidate listening TCP sockets read by the host.
    #[serde(default)]
    pub sockets: Vec<ListeningSocket>,
    /// Ports to omit from the result, such as an attached debugger port.
    #[serde(default)]
    pub exclude_ports: Vec<u16>,
    /// Maximum number of deduplicated ports to return, defaulted to 8 and
    /// clamped to `1..=32`.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// One deduplicated, normalized listening TCP port of a run.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ListeningPort {
    /// TCP port number.
    pub port: u16,
    /// Canonical bound IP address (`"0.0.0.0"`, `"127.0.0.1"`, `"::"`, etc.).
    pub address: String,
    /// Compact display label (`":8080"`, `"192.168.1.5:8080"`, or `"[fe80::1]:8080"`).
    pub label: String,
    /// Browser-ready HTTP URL (`"http://localhost:8080"`, `"http://192.168.1.5:8080"`, etc.).
    pub url: String,
    /// Process ID selected as the owner of this port.
    pub process_id: i64,
}

/// Response of `process.listeningPorts`.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ListeningPortsResponse {
    /// Contract schema version (`1`).
    pub schema_version: u32,
    /// Deduplicated listening ports ordered ascending by [`ListeningPort::port`].
    pub ports: Vec<ListeningPort>,
    /// Whether more than the effective limit of ports matched before truncation.
    pub truncated: bool,
}

/// Resolves, normalizes, deduplicates, and formats listening TCP ports belonging
/// to the process tree rooted at `request.root_process_id`.
pub fn resolve(request: &ListeningPortsRequest) -> Result<ListeningPortsResponse, String> {
    if request.root_process_id <= 0 {
        return Err("rootProcessId must be positive".to_string());
    }

    let tree_pids = collect_tree_pids(request.root_process_id, &request.processes)?;
    let excluded: HashSet<u16> = request.exclude_ports.iter().copied().collect();
    let limit = request.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    #[derive(Clone)]
    struct Candidate {
        ip: IpAddr,
        address: String,
        process_id: i64,
    }

    let mut by_port: BTreeMap<u16, Candidate> = BTreeMap::new();
    for socket in &request.sockets {
        let ip = normalize_address(&socket.address)?;
        if socket.port == 0
            || excluded.contains(&socket.port)
            || !tree_pids.contains(&socket.process_id)
        {
            continue;
        }
        let address = ip.to_string();
        let candidate = Candidate {
            ip,
            address,
            process_id: socket.process_id,
        };
        let replace = match by_port.get(&socket.port) {
            None => true,
            Some(existing) => {
                candidate_rank(candidate.ip, candidate.process_id, &candidate.address)
                    < candidate_rank(existing.ip, existing.process_id, &existing.address)
            }
        };
        if replace {
            by_port.insert(socket.port, candidate);
        }
    }

    let total = by_port.len();
    let truncated = total > limit;
    let ports = by_port
        .into_iter()
        .take(limit)
        .map(|(port, candidate)| ListeningPort {
            port,
            address: candidate.address,
            label: format_label(candidate.ip, port),
            url: format_url(candidate.ip, port),
            process_id: candidate.process_id,
        })
        .collect();

    Ok(ListeningPortsResponse {
        schema_version: SCHEMA_VERSION,
        ports,
        truncated,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TreeMembership {
    Unknown,
    Visiting,
    InTree,
    OutOfTree,
}

/// Builds the set of PIDs in the process tree rooted at `root_pid`, rejecting
/// duplicate process rows and ignoring parent links whose parent started after
/// its reported child (indicating PID reuse).
fn collect_tree_pids(root_pid: i64, processes: &[PortProcess]) -> Result<HashSet<i64>, String> {
    let mut positions = HashMap::with_capacity(processes.len());
    for (index, process) in processes.iter().enumerate() {
        if positions.insert(process.process_id, index).is_some() {
            return Err(format!(
                "process {} appears more than once",
                process.process_id
            ));
        }
    }

    let mut states = vec![TreeMembership::Unknown; processes.len()];
    let mut path = Vec::new();

    for start in 0..processes.len() {
        if !matches!(states[start], TreeMembership::Unknown) {
            continue;
        }
        path.clear();
        let mut current = start;
        let resolved = loop {
            match states[current] {
                TreeMembership::InTree => break TreeMembership::InTree,
                TreeMembership::OutOfTree => break TreeMembership::OutOfTree,
                TreeMembership::Visiting => {
                    // Any cycle that never visited the root cannot descend from it.
                    let at = path
                        .iter()
                        .position(|&idx| idx == current)
                        .expect("visiting node must be on current path");
                    for &member in &path[at..] {
                        states[member] = TreeMembership::OutOfTree;
                    }
                    path.truncate(at);
                    break TreeMembership::OutOfTree;
                }
                TreeMembership::Unknown => {}
            }
            states[current] = TreeMembership::Visiting;
            path.push(current);

            let process = &processes[current];
            if process.process_id == root_pid {
                break TreeMembership::InTree;
            }
            if process.parent_process_id == root_pid && !positions.contains_key(&root_pid) {
                // Allow callers that pass only descendants without the root row itself.
                break TreeMembership::InTree;
            }
            let Some(&parent) = positions.get(&process.parent_process_id) else {
                break TreeMembership::OutOfTree;
            };
            if parent == current {
                break TreeMembership::OutOfTree;
            }
            if let (Some(child_start), Some(parent_start)) =
                (process.start_time, processes[parent].start_time)
            {
                if parent_start > child_start {
                    break TreeMembership::OutOfTree;
                }
            }
            current = parent;
        };

        for &member in &path {
            states[member] = resolved;
        }
    }

    let mut tree_pids = HashSet::new();
    tree_pids.insert(root_pid);
    for (process, state) in processes.iter().zip(states) {
        if state == TreeMembership::InTree {
            tree_pids.insert(process.process_id);
        }
    }
    Ok(tree_pids)
}

/// Normalizes a socket bind address into a canonical [`IpAddr`], folding
/// empty/wildcard strings to `0.0.0.0` and IPv4-mapped IPv6 (`::ffff:a.b.c.d`)
/// to IPv4.
fn normalize_address(raw: &str) -> Result<IpAddr, String> {
    let trimmed = raw.trim();
    let unwrapped = if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() >= 2 {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };
    if unwrapped.is_empty() || unwrapped == "*" {
        return Ok(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    }
    let without_zone = match unwrapped.split_once('%') {
        Some((addr, _zone)) if addr.contains(':') => addr,
        _ => unwrapped,
    };
    let parsed: IpAddr = without_zone
        .parse()
        .map_err(|_| format!("invalid listening socket address: {raw}"))?;
    Ok(unwrap_ipv4_mapped(parsed))
}

fn unwrap_ipv4_mapped(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => IpAddr::V4(v4),
        IpAddr::V6(v6) => match ipv4_mapped_octets(v6) {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
    }
}

fn ipv4_mapped_octets(v6: Ipv6Addr) -> Option<Ipv4Addr> {
    match v6.octets() {
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, a, b, c, d] => Some(Ipv4Addr::new(a, b, c, d)),
        _ => None,
    }
}

fn is_local_bind(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_unspecified(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified(),
    }
}

fn candidate_rank(ip: IpAddr, process_id: i64, address: &str) -> (bool, bool, bool, i64, &str) {
    (
        !is_local_bind(ip),
        !ip.is_ipv4(),
        !ip.is_loopback(),
        process_id,
        address,
    )
}

fn format_label(ip: IpAddr, port: u16) -> String {
    if is_local_bind(ip) {
        return format!(":{port}");
    }
    match ip {
        IpAddr::V4(v4) => format!("{v4}:{port}"),
        IpAddr::V6(v6) => format!("[{v6}]:{port}"),
    }
}

fn format_url(ip: IpAddr, port: u16) -> String {
    if is_local_bind(ip) {
        return format!("http://localhost:{port}");
    }
    match ip {
        IpAddr::V4(v4) => format!("http://{v4}:{port}"),
        IpAddr::V6(v6) => format!("http://[{v6}]:{port}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc_row(process_id: i64, parent_process_id: i64, start_time: Option<u64>) -> PortProcess {
        PortProcess {
            process_id,
            parent_process_id,
            start_time,
        }
    }

    fn sock(process_id: i64, address: &str, port: u16) -> ListeningSocket {
        ListeningSocket {
            process_id,
            address: address.to_string(),
            port,
        }
    }

    #[test]
    fn resolves_tree_descendants_even_when_root_is_absent_or_processes_empty() {
        let root_only = resolve(&ListeningPortsRequest {
            root_process_id: 500,
            processes: Vec::new(),
            sockets: vec![sock(500, "127.0.0.1", 8080), sock(999, "127.0.0.1", 9090)],
            exclude_ports: Vec::new(),
            limit: None,
        })
        .unwrap();
        assert_eq!(root_only.schema_version, 1);
        assert_eq!(root_only.ports.len(), 1);
        assert_eq!(root_only.ports[0].port, 8080);
        assert_eq!(root_only.ports[0].label, ":8080");
        assert_eq!(root_only.ports[0].url, "http://localhost:8080");

        let with_child = resolve(&ListeningPortsRequest {
            root_process_id: 500,
            processes: vec![proc_row(501, 500, Some(200)), proc_row(502, 501, Some(300))],
            sockets: vec![sock(502, "0.0.0.0", 8081), sock(600, "0.0.0.0", 8082)],
            exclude_ports: Vec::new(),
            limit: None,
        })
        .unwrap();
        assert_eq!(with_child.ports.len(), 1);
        assert_eq!(with_child.ports[0].port, 8081);
        assert_eq!(with_child.ports[0].process_id, 502);
    }

    #[test]
    fn ignores_reused_pid_parent_links_and_cycles() {
        let response = resolve(&ListeningPortsRequest {
            root_process_id: 100,
            processes: vec![
                proc_row(100, 1, Some(5000)),
                // Child started BEFORE root 100 => 100 was reused; must NOT belong to tree.
                proc_row(200, 100, Some(1000)),
                // Valid child started AFTER root 100.
                proc_row(201, 100, Some(5500)),
                // Disconnected cycle.
                proc_row(300, 301, None),
                proc_row(301, 300, None),
            ],
            sockets: vec![
                sock(200, "127.0.0.1", 7000),
                sock(201, "127.0.0.1", 8080),
                sock(300, "127.0.0.1", 7001),
            ],
            exclude_ports: Vec::new(),
            limit: None,
        })
        .unwrap();
        assert_eq!(response.ports.len(), 1);
        assert_eq!(response.ports[0].port, 8080);
        assert_eq!(response.ports[0].process_id, 201);
    }

    #[test]
    fn excludes_specified_ports_and_zero_ports() {
        let response = resolve(&ListeningPortsRequest {
            root_process_id: 100,
            processes: vec![proc_row(100, 1, None)],
            sockets: vec![
                sock(100, "127.0.0.1", 0),
                sock(100, "127.0.0.1", 5005),
                sock(100, "0.0.0.0", 8080),
            ],
            exclude_ports: vec![5005],
            limit: None,
        })
        .unwrap();
        assert_eq!(response.ports.len(), 1);
        assert_eq!(response.ports[0].port, 8080);
    }

    #[test]
    fn deduplicates_dual_stack_and_worker_sockets_deterministically() {
        let response = resolve(&ListeningPortsRequest {
            root_process_id: 100,
            processes: vec![
                proc_row(100, 1, None),
                proc_row(101, 100, None),
                proc_row(102, 100, None),
            ],
            sockets: vec![
                sock(102, "192.168.1.5", 8080),
                sock(102, "::", 8080),
                sock(101, "0.0.0.0", 8080),
                sock(102, "[::ffff:127.0.0.1]", 8081),
                sock(101, "::1", 8081),
                sock(101, "192.168.1.10", 9000),
                sock(102, "10.0.0.2", 9000),
            ],
            exclude_ports: Vec::new(),
            limit: None,
        })
        .unwrap();
        assert_eq!(response.ports.len(), 3);
        // 8080: IPv4 unspecified 0.0.0.0 beats IPv6 :: and specific interface.
        assert_eq!(response.ports[0].port, 8080);
        assert_eq!(response.ports[0].address, "0.0.0.0");
        assert_eq!(response.ports[0].label, ":8080");
        assert_eq!(response.ports[0].url, "http://localhost:8080");
        assert_eq!(response.ports[0].process_id, 101);
        // 8081: IPv4-mapped ::ffff:127.0.0.1 unwraps to IPv4 127.0.0.1 and beats ::1.
        assert_eq!(response.ports[1].port, 8081);
        assert_eq!(response.ports[1].address, "127.0.0.1");
        assert_eq!(response.ports[1].label, ":8081");
        assert_eq!(response.ports[1].url, "http://localhost:8081");
        // 9000: two specific IPv4 addresses => lowest process_id (101) wins.
        assert_eq!(response.ports[2].port, 9000);
        assert_eq!(response.ports[2].address, "192.168.1.10");
        assert_eq!(response.ports[2].label, "192.168.1.10:9000");
        assert_eq!(response.ports[2].url, "http://192.168.1.10:9000");
        assert_eq!(response.ports[2].process_id, 101);
    }

    #[test]
    fn normalizes_wildcards_brackets_zones_and_formats_ipv6_urls() {
        let response = resolve(&ListeningPortsRequest {
            root_process_id: 100,
            processes: Vec::new(),
            sockets: vec![
                sock(100, "", 3000),
                sock(100, " * ", 3001),
                sock(100, "[::1]", 3002),
                sock(100, "[fe80::1%en0]", 3003),
                sock(100, "2001:db8::5", 3004),
            ],
            exclude_ports: Vec::new(),
            limit: None,
        })
        .unwrap();
        assert_eq!(response.ports[0].address, "0.0.0.0");
        assert_eq!(response.ports[0].label, ":3000");
        assert_eq!(response.ports[0].url, "http://localhost:3000");

        assert_eq!(response.ports[1].address, "0.0.0.0");
        assert_eq!(response.ports[1].label, ":3001");

        assert_eq!(response.ports[2].address, "::1");
        assert_eq!(response.ports[2].label, ":3002");
        assert_eq!(response.ports[2].url, "http://localhost:3002");

        assert_eq!(response.ports[3].address, "fe80::1");
        assert_eq!(response.ports[3].label, "[fe80::1]:3003");
        assert_eq!(response.ports[3].url, "http://[fe80::1]:3003");

        assert_eq!(response.ports[4].address, "2001:db8::5");
        assert_eq!(response.ports[4].label, "[2001:db8::5]:3004");
        assert_eq!(response.ports[4].url, "http://[2001:db8::5]:3004");
    }

    #[test]
    fn enforces_limit_clamping_and_truncation_flag() {
        let sockets = (1..=40).map(|i| sock(100, "127.0.0.1", 8000 + i)).collect();
        let default_capped = resolve(&ListeningPortsRequest {
            root_process_id: 100,
            processes: Vec::new(),
            sockets,
            exclude_ports: Vec::new(),
            limit: None,
        })
        .unwrap();
        assert_eq!(default_capped.ports.len(), 8);
        assert!(default_capped.truncated);
        assert_eq!(default_capped.ports[0].port, 8001);
        assert_eq!(default_capped.ports[7].port, 8008);

        let min_clamped = resolve(&ListeningPortsRequest {
            root_process_id: 100,
            processes: Vec::new(),
            sockets: vec![sock(100, "127.0.0.1", 8080), sock(100, "127.0.0.1", 8081)],
            exclude_ports: Vec::new(),
            limit: Some(0),
        })
        .unwrap();
        assert_eq!(min_clamped.ports.len(), 1);
        assert!(min_clamped.truncated);

        let max_clamped = resolve(&ListeningPortsRequest {
            root_process_id: 100,
            processes: Vec::new(),
            sockets: (1..=40).map(|i| sock(100, "127.0.0.1", 9000 + i)).collect(),
            exclude_ports: Vec::new(),
            limit: Some(100),
        })
        .unwrap();
        assert_eq!(max_clamped.ports.len(), 32);
        assert!(max_clamped.truncated);
    }

    #[test]
    fn rejects_invalid_root_duplicate_processes_and_malformed_addresses() {
        assert!(resolve(&ListeningPortsRequest {
            root_process_id: 0,
            processes: Vec::new(),
            sockets: Vec::new(),
            exclude_ports: Vec::new(),
            limit: None,
        })
        .is_err());

        assert!(resolve(&ListeningPortsRequest {
            root_process_id: -5,
            processes: Vec::new(),
            sockets: Vec::new(),
            exclude_ports: Vec::new(),
            limit: None,
        })
        .is_err());

        assert!(resolve(&ListeningPortsRequest {
            root_process_id: 100,
            processes: vec![proc_row(100, 1, None), proc_row(100, 1, None)],
            sockets: Vec::new(),
            exclude_ports: Vec::new(),
            limit: None,
        })
        .is_err());

        for bad in [
            "not-an-ip",
            "localhost",
            "[::1",
            "999.999.999.999",
            "127.0.0.1%eth0",
        ] {
            assert!(
                resolve(&ListeningPortsRequest {
                    root_process_id: 100,
                    processes: Vec::new(),
                    sockets: vec![sock(100, bad, 8080)],
                    exclude_ports: Vec::new(),
                    limit: None,
                })
                .is_err(),
                "expected {bad} to be rejected"
            );
        }
    }
}
