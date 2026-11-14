//! Attribution of a process table to the parts of one Lithe instance, for the
//! memory readout of the status bar.
//!
//! Hosts read every process's parent, start time, responsible process and
//! memory, and name the processes they started for a role: language servers
//! and plugin services. Which process belongs to which part, and the sums, are
//! decided here so every host shows the same numbers for the same table:
//!
//! - the host process is a part of its own;
//! - a process whose parent chain reaches a role process first belongs to that
//!   role, so a language server that a plugin service started counts once, as a
//!   language server, together with everything below it;
//! - a process whose parent chain reaches the host first is a child process,
//!   grouped under the host's direct child it descends from;
//! - a process outside the host's tree is a system helper when the operating
//!   system holds the host responsible for it, as macOS does for the WebKit
//!   processes of a web view, grouped under the topmost such process;
//! - every other process is not the instance's and is left out.
//!
//! Each process counts once. Fixtures live in
//! `rust/fixtures/process-lifecycle/memory-summary-v1.json`.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Entries returned when the request does not give a limit.
const DEFAULT_ENTRY_LIMIT: usize = 20;
/// Most entries one summary returns, whatever the request asks.
const MAX_ENTRY_LIMIT: usize = 200;
/// Longest entry name before it is shortened with an ellipsis.
const NAME_CHARACTERS: usize = 80;

/// One row of the host's process table.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryProcess {
    /// Operating-system process ID.
    pub process_id: i64,
    /// Parent process ID as the operating system reports it.
    pub parent_process_id: i64,
    /// Process the operating system holds responsible for this one, on systems
    /// that have that notion.
    #[serde(default)]
    pub responsible_process_id: Option<i64>,
    /// Start time in microseconds since 1970. When a process and its reported
    /// parent both have one and the parent started later, the parent ID was
    /// reused and the link is ignored.
    #[serde(default)]
    pub start_time: Option<u64>,
    /// Short process name.
    #[serde(default)]
    pub name: String,
    /// Memory charged to the process, in bytes. Every row uses the same measure.
    pub bytes: u64,
}

/// What the host started a process for.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MemoryRoleKind {
    /// A language server of an open project.
    LanguageServer,
    /// The backend process of a plugin.
    PluginService,
}

/// A process the host started for a role.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRole {
    /// Operating-system process ID of the role's process.
    pub process_id: i64,
    /// What the process was started for.
    pub kind: MemoryRoleKind,
    /// Name shown for the role; empty shows the process name.
    #[serde(default)]
    pub name: String,
}

/// Request of `processMemory.summarize`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRequest {
    /// Process ID of the host itself; it must be one of `processes`.
    pub host_process_id: i64,
    /// Every process the host could read, in any order.
    pub processes: Vec<MemoryProcess>,
    /// Processes the host started for a role. A role naming the host is
    /// ignored, and the first role listed for a process ID is the one used.
    #[serde(default)]
    pub roles: Vec<MemoryRole>,
    /// Most entries to return; defaults to 20 and is capped at 200.
    #[serde(default)]
    pub entry_limit: Option<usize>,
}

/// Part of the instance a process's memory counts in, in the order the
/// summary lists them.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum MemoryKind {
    /// The host process.
    Host,
    /// Language servers and the processes below them.
    LanguageServer,
    /// Plugin services and the processes below them.
    PluginService,
    /// Processes the operating system started on the host's behalf outside
    /// its process tree.
    SystemHelper,
    /// Other processes of the host's tree, such as terminal shells and tools.
    ChildProcess,
}

impl MemoryKind {
    const ALL: [MemoryKind; 5] = [
        MemoryKind::Host,
        MemoryKind::LanguageServer,
        MemoryKind::PluginService,
        MemoryKind::SystemHelper,
        MemoryKind::ChildProcess,
    ];
}

impl From<MemoryRoleKind> for MemoryKind {
    fn from(kind: MemoryRoleKind) -> Self {
        match kind {
            MemoryRoleKind::LanguageServer => MemoryKind::LanguageServer,
            MemoryRoleKind::PluginService => MemoryKind::PluginService,
        }
    }
}

/// Sum of one part.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryCategory {
    /// The part.
    pub kind: MemoryKind,
    /// Bytes of every process in the part.
    pub bytes: u64,
    /// Number of processes in the part.
    pub process_count: usize,
}

/// One process of the instance together with the processes counted under it.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntry {
    /// Part the entry counts in; never `host`.
    pub kind: MemoryKind,
    /// Role name, else the process name, shortened to one row.
    pub name: String,
    /// Process ID of the entry's top process.
    pub process_id: i64,
    /// Bytes of the top process and the processes below it.
    pub bytes: u64,
    /// Number of processes in the entry, the top one included.
    pub process_count: usize,
}

/// Entries past the limit.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OmittedEntries {
    /// Number of entries left out.
    pub count: usize,
    /// Bytes of the entries left out.
    pub bytes: u64,
}

/// Response of `processMemory.summarize`.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemorySummary {
    /// Bytes of every process of the instance.
    pub total_bytes: u64,
    /// Every part, in the order of [`MemoryKind`], empty ones included.
    pub categories: Vec<MemoryCategory>,
    /// Entries other than the host, largest first; ties go by part, then
    /// name, then process ID.
    pub entries: Vec<MemoryEntry>,
    /// Entries past the limit.
    pub omitted: OmittedEntries,
}

/// Why a process table cannot be summarized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemoryError {
    /// The host process is not in the table.
    HostMissing(i64),
    /// Two rows have the same process ID.
    DuplicateProcess(i64),
}

impl fmt::Display for MemoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MemoryError::HostMissing(process_id) => {
                write!(formatter, "host process {process_id} is not in the table")
            }
            MemoryError::DuplicateProcess(process_id) => {
                write!(formatter, "process {process_id} appears more than once")
            }
        }
    }
}

impl std::error::Error for MemoryError {}

/// Where one process counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owner {
    /// Not the instance's.
    Unowned,
    /// The host process.
    Host,
    /// Counted in `kind`, under the process at position `root`.
    Part { kind: MemoryKind, root: usize },
}

/// Sums the memory of every process of the host's instance by part and by
/// top process (`processMemory.summarize`).
pub fn summarize(request: &MemoryRequest) -> Result<MemorySummary, MemoryError> {
    let processes = &request.processes;
    let mut positions = HashMap::with_capacity(processes.len());
    for (position, process) in processes.iter().enumerate() {
        if positions.insert(process.process_id, position).is_some() {
            return Err(MemoryError::DuplicateProcess(process.process_id));
        }
    }
    let host = request.host_process_id;
    if !positions.contains_key(&host) {
        return Err(MemoryError::HostMissing(host));
    }
    let mut roles: HashMap<i64, &MemoryRole> = HashMap::new();
    for role in &request.roles {
        if role.process_id != host {
            roles.entry(role.process_id).or_insert(role);
        }
    }
    let owners = Attribution {
        processes,
        positions: &positions,
        roles: &roles,
        host,
    }
    .owners();

    let mut categories = MemoryKind::ALL.map(|kind| MemoryCategory {
        kind,
        bytes: 0,
        process_count: 0,
    });
    let mut groups: HashMap<usize, MemoryEntry> = HashMap::new();
    let mut total_bytes = 0u64;
    for (process, owner) in processes.iter().zip(&owners) {
        let kind = match *owner {
            Owner::Unowned => continue,
            Owner::Host => MemoryKind::Host,
            Owner::Part { kind, root } => {
                let entry = groups.entry(root).or_insert_with(|| MemoryEntry {
                    kind,
                    name: entry_name(&processes[root], roles.get(&processes[root].process_id)),
                    process_id: processes[root].process_id,
                    bytes: 0,
                    process_count: 0,
                });
                entry.bytes = entry.bytes.saturating_add(process.bytes);
                entry.process_count += 1;
                kind
            }
        };
        let category = &mut categories[kind as usize];
        category.bytes = category.bytes.saturating_add(process.bytes);
        category.process_count += 1;
        total_bytes = total_bytes.saturating_add(process.bytes);
    }

    let mut entries: Vec<MemoryEntry> = groups.into_values().collect();
    entries.sort_by(|a, b| {
        b.bytes
            .cmp(&a.bytes)
            .then(a.kind.cmp(&b.kind))
            .then_with(|| a.name.cmp(&b.name))
            .then(a.process_id.cmp(&b.process_id))
    });
    let limit = request
        .entry_limit
        .unwrap_or(DEFAULT_ENTRY_LIMIT)
        .min(MAX_ENTRY_LIMIT)
        .min(entries.len());
    let rest = entries.split_off(limit);
    let omitted = OmittedEntries {
        count: rest.len(),
        bytes: rest
            .iter()
            .fold(0u64, |sum, entry| sum.saturating_add(entry.bytes)),
    };
    Ok(MemorySummary {
        total_bytes,
        categories: categories.to_vec(),
        entries,
        omitted,
    })
}

/// The role name, else the process name, short enough for one row.
fn entry_name(process: &MemoryProcess, role: Option<&&MemoryRole>) -> String {
    let name = role
        .map(|role| role.name.as_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(&process.name);
    if name.chars().count() > NAME_CHARACTERS {
        let shortened: String = name.chars().take(NAME_CHARACTERS - 1).collect();
        format!("{shortened}…")
    } else {
        name.to_string()
    }
}

/// The table and the facts that decide where each process counts.
struct Attribution<'a> {
    processes: &'a [MemoryProcess],
    positions: &'a HashMap<i64, usize>,
    roles: &'a HashMap<i64, &'a MemoryRole>,
    host: i64,
}

impl Attribution<'_> {
    /// Where every process counts, by position.
    ///
    /// Each walk climbs parent links from a process until it reaches a
    /// process already decided, the host, a role, or a process without a
    /// usable parent, then decides the climbed processes from the top down.
    /// A walk that comes back to a process it climbed has found a cycle of
    /// parent links, which only reused process IDs without start times can
    /// produce; the processes of the cycle are decided as if none had a
    /// parent, so the result does not depend on where the walk started.
    fn owners(&self) -> Vec<Owner> {
        let count = self.processes.len();
        let mut owners: Vec<Option<Owner>> = vec![None; count];
        let mut climbing = vec![false; count];
        let mut path = Vec::new();
        for start in 0..count {
            if owners[start].is_some() {
                continue;
            }
            path.clear();
            let mut current = start;
            let mut above = Owner::Unowned;
            loop {
                if let Some(owner) = owners[current] {
                    above = owner;
                    break;
                }
                if climbing[current] {
                    let at = path
                        .iter()
                        .position(|&position| position == current)
                        .expect("a climbed process is on the path");
                    for &member in &path[at..] {
                        owners[member] = Some(self.owner(member, Owner::Unowned));
                        climbing[member] = false;
                    }
                    path.truncate(at);
                    above = owners[current].expect("cycle members are decided");
                    break;
                }
                climbing[current] = true;
                path.push(current);
                if self.decides_itself(current) {
                    break;
                }
                match self.parent(current) {
                    Some(parent) => current = parent,
                    None => break,
                }
            }
            for &member in path.iter().rev() {
                above = self.owner(member, above);
                owners[member] = Some(above);
                climbing[member] = false;
            }
        }
        owners
            .into_iter()
            .map(|owner| owner.expect("every process is decided"))
            .collect()
    }

    /// Whether the process at `position` is the host or a role, whose place
    /// does not depend on its parent.
    fn decides_itself(&self, position: usize) -> bool {
        let process_id = self.processes[position].process_id;
        process_id == self.host || self.roles.contains_key(&process_id)
    }

    /// Where the process at `position` counts, given where its parent counts.
    fn owner(&self, position: usize, above: Owner) -> Owner {
        let process = &self.processes[position];
        if process.process_id == self.host {
            return Owner::Host;
        }
        if let Some(role) = self.roles.get(&process.process_id) {
            return Owner::Part {
                kind: role.kind.into(),
                root: position,
            };
        }
        match above {
            Owner::Host => Owner::Part {
                kind: MemoryKind::ChildProcess,
                root: position,
            },
            Owner::Part { .. } => above,
            Owner::Unowned if process.responsible_process_id == Some(self.host) => Owner::Part {
                kind: MemoryKind::SystemHelper,
                root: position,
            },
            Owner::Unowned => Owner::Unowned,
        }
    }

    /// Position of the parent of the process at `position`: none when the
    /// parent is not in the table, is the process itself, or started after it.
    fn parent(&self, position: usize) -> Option<usize> {
        let process = &self.processes[position];
        let &parent = self.positions.get(&process.parent_process_id)?;
        if parent == position {
            return None;
        }
        if let (Some(child_start), Some(parent_start)) =
            (process.start_time, self.processes[parent].start_time)
        {
            if parent_start > child_start {
                return None;
            }
        }
        Some(parent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Fixture {
        schema_version: u32,
        cases: Vec<FixtureCase>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FixtureCase {
        name: String,
        request: serde_json::Value,
        expected: serde_json::Value,
    }

    fn process(process_id: i64, parent_process_id: i64, bytes: u64) -> MemoryProcess {
        MemoryProcess {
            process_id,
            parent_process_id,
            responsible_process_id: None,
            start_time: None,
            name: format!("p{process_id}"),
            bytes,
        }
    }

    // Every host replays this file, so a rule change must update it.
    #[test]
    fn shared_fixture_summaries_match_in_any_table_order() {
        let fixture: Fixture = serde_json::from_str(include_str!(
            "../../../fixtures/process-lifecycle/memory-summary-v1.json"
        ))
        .expect("fixture should parse");
        assert_eq!(fixture.schema_version, 1);
        for case in fixture.cases {
            let mut request: MemoryRequest =
                serde_json::from_value(case.request).expect("fixture request should decode");
            for ordering in ["original", "reversed", "rotated"] {
                match ordering {
                    "reversed" => request.processes.reverse(),
                    "rotated" if !request.processes.is_empty() => request.processes.rotate_left(1),
                    _ => {}
                }
                let summary = summarize(&request).expect("fixture case should summarize");
                assert_eq!(
                    serde_json::to_value(summary).expect("summary should encode"),
                    case.expected,
                    "fixture case {} ({ordering} table)",
                    case.name
                );
            }
        }
    }

    #[test]
    fn a_table_without_the_host_or_with_a_repeated_process_is_rejected() {
        let missing = MemoryRequest {
            host_process_id: 10,
            processes: vec![process(11, 1, 5)],
            roles: Vec::new(),
            entry_limit: None,
        };
        assert_eq!(summarize(&missing), Err(MemoryError::HostMissing(10)));
        let repeated = MemoryRequest {
            host_process_id: 10,
            processes: vec![process(10, 1, 5), process(11, 10, 5), process(11, 10, 6)],
            roles: Vec::new(),
            entry_limit: None,
        };
        assert_eq!(summarize(&repeated), Err(MemoryError::DuplicateProcess(11)));
    }

    #[test]
    fn a_deep_chain_is_decided_without_recursion() {
        let depth = 100_000;
        let mut processes = vec![process(1, 0, 1)];
        processes.extend((2..=depth).map(|process_id| process(process_id, process_id - 1, 1)));
        processes.reverse();
        let summary = summarize(&MemoryRequest {
            host_process_id: 1,
            processes,
            roles: Vec::new(),
            entry_limit: Some(1),
        })
        .expect("chain should summarize");
        assert_eq!(summary.total_bytes, depth as u64);
        assert_eq!(summary.entries.len(), 1);
        assert_eq!(summary.entries[0].process_id, 2);
        assert_eq!(summary.entries[0].process_count, depth as usize - 1);
    }

    #[test]
    fn sums_saturate_instead_of_overflowing() {
        let summary = summarize(&MemoryRequest {
            host_process_id: 1,
            processes: vec![process(1, 0, u64::MAX), process(2, 1, 1)],
            roles: Vec::new(),
            entry_limit: None,
        })
        .expect("table should summarize");
        assert_eq!(summary.total_bytes, u64::MAX);
    }

    #[test]
    fn long_names_are_shortened() {
        let mut long = process(2, 1, 1);
        long.name = "x".repeat(200);
        let summary = summarize(&MemoryRequest {
            host_process_id: 1,
            processes: vec![process(1, 0, 1), long],
            roles: Vec::new(),
            entry_limit: None,
        })
        .expect("table should summarize");
        let name = &summary.entries[0].name;
        assert_eq!(name.chars().count(), NAME_CHARACTERS);
        assert!(name.ends_with('…'));
    }

    #[test]
    fn a_larger_limit_is_capped() {
        let mut processes = vec![process(1, 0, 1)];
        processes.extend((2..=251).map(|process_id| process(process_id, 1, 1)));
        let summary = summarize(&MemoryRequest {
            host_process_id: 1,
            processes,
            roles: Vec::new(),
            entry_limit: Some(10_000),
        })
        .expect("table should summarize");
        assert_eq!(summary.entries.len(), MAX_ENTRY_LIMIT);
        assert_eq!(
            summary.omitted,
            OmittedEntries {
                count: 50,
                bytes: 50
            }
        );
    }
}
