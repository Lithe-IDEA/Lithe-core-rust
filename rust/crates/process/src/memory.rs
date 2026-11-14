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
//! Each process counts once. Every host reports memory the same way: `bytes`
//! is what is in memory, and `compressedBytes` is what the system compressed
//! or wrote out to disk, which `bytes` does not include. Graphics memory that
//! `bytes` does not hold, such as dedicated video memory on Windows, is summed
//! apart. Processor and graphics percentages come from the use counters of two
//! tables: each summary returns a baseline the host sends back with the next
//! table, so the percentages are computed here too. Fixtures live in
//! `rust/fixtures/process-lifecycle/memory-summary-v1.json`.

use std::collections::{BTreeMap, HashMap};
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
    /// Memory of the process that is in memory, in bytes. Every row uses the
    /// same measure.
    pub bytes: u64,
    /// Memory of the process that the system compressed or wrote out to disk,
    /// in bytes, which `bytes` does not include. Absent when it was not read.
    #[serde(default)]
    pub compressed_bytes: Option<u64>,
    /// Graphics memory charged to the process that `bytes` does not include,
    /// such as dedicated video memory on Windows. Absent where it was not read
    /// or `bytes` already holds it.
    #[serde(default)]
    pub gpu_memory_bytes: Option<u64>,
    /// Processor time the process used since it started, user and system
    /// together, in microseconds. Absent when it was not read.
    #[serde(default)]
    pub cpu_time: Option<u64>,
    /// Busy time of every graphics engine the process used. Empty when the
    /// process used none or they were not read.
    #[serde(default)]
    pub gpu_engines: Vec<GpuEngineTime>,
}

/// Time one graphics engine spent on a process's work since it started.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GpuEngineTime {
    /// Name of the engine, the same for every process and every table of one
    /// host, such as an adapter and its node. Times of one name add up.
    pub engine: String,
    /// Microseconds the engine was busy with the process's work.
    pub busy_time: u64,
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
    /// When the table was read, in microseconds of a clock that only moves
    /// forward. Without it the summary has no percentages and no baseline.
    #[serde(default)]
    pub sampled_at: Option<u64>,
    /// The baseline of the host's previous summary.
    #[serde(default)]
    pub previous: Option<UsageBaseline>,
    /// When given, a processor percentage is a share of this many logical
    /// processors, as the Windows Task Manager shows it; otherwise 100 is one
    /// processor busy, as Activity Monitor and top show it. Must be at least 1.
    #[serde(default)]
    pub logical_processors: Option<u32>,
}

/// Use counters of the instance's processes at one moment, which the next
/// request sends back to measure the use in between.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UsageBaseline {
    /// `sampledAt` of the table the baseline was taken from.
    pub sampled_at: u64,
    /// The instance's processes that had a use counter, by process ID.
    pub processes: Vec<BaselineProcess>,
}

/// Use counters of one process in a baseline.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BaselineProcess {
    /// Operating-system process ID.
    pub process_id: i64,
    /// Start time of the process; a row of the next table with another start
    /// time is a reused process ID and is not compared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_time: Option<u64>,
    /// Processor time used since the process started, in microseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_time: Option<u64>,
    /// Busy time of each graphics engine, one row per engine, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gpu_engines: Vec<GpuEngineTime>,
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

/// Figures of a group of processes beyond its memory in memory. Each one is
/// present on the summary, every category and every entry, or on none: the
/// byte sums when a process of the instance reported them, the percentages
/// when a process of the instance can be compared with the previous baseline.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageFigures {
    /// Bytes the system compressed or wrote out to disk.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compressed_bytes: Option<u64>,
    /// Graphics memory that the in-memory bytes do not hold.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_memory_bytes: Option<u64>,
    /// Processor use since the previous table, rounded to a tenth.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_percent: Option<f64>,
    /// Use of the busiest graphics engine since the previous table, rounded
    /// to a tenth and at most 100.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_percent: Option<f64>,
}

/// Sum of one part.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryCategory {
    /// The part.
    pub kind: MemoryKind,
    /// Bytes in memory of every process in the part.
    pub bytes: u64,
    /// Number of processes in the part.
    pub process_count: usize,
    /// The part's other figures.
    #[serde(flatten)]
    pub usage: UsageFigures,
}

/// One process of the instance together with the processes counted under it.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntry {
    /// Part the entry counts in; never `host`.
    pub kind: MemoryKind,
    /// Role name, else the process name, shortened to one row.
    pub name: String,
    /// Process ID of the entry's top process.
    pub process_id: i64,
    /// Bytes in memory of the top process and the processes below it.
    pub bytes: u64,
    /// Number of processes in the entry, the top one included.
    pub process_count: usize,
    /// The entry's other figures.
    #[serde(flatten)]
    pub usage: UsageFigures,
}

/// Entries past the limit.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OmittedEntries {
    /// Number of entries left out.
    pub count: usize,
    /// Bytes in memory of the entries left out.
    pub bytes: u64,
}

/// Response of `processMemory.summarize`.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemorySummary {
    /// Bytes in memory of every process of the instance.
    pub total_bytes: u64,
    /// The instance's other figures.
    #[serde(flatten)]
    pub usage: UsageFigures,
    /// Every part, in the order of [`MemoryKind`], empty ones included.
    pub categories: Vec<MemoryCategory>,
    /// Entries other than the host, largest first; ties go by part, then
    /// name, then process ID.
    pub entries: Vec<MemoryEntry>,
    /// Entries past the limit.
    pub omitted: OmittedEntries,
    /// Use counters to send with the next table; present when the request
    /// has `sampledAt`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<UsageBaseline>,
}

/// Why a process table cannot be summarized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemoryError {
    /// The host process is not in the table.
    HostMissing(i64),
    /// Two rows have the same process ID.
    DuplicateProcess(i64),
    /// The request counts processor shares of no processor.
    NoLogicalProcessors,
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
            MemoryError::NoLogicalProcessors => {
                write!(formatter, "logicalProcessors must be at least 1")
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
    if request.logical_processors == Some(0) {
        return Err(MemoryError::NoLogicalProcessors);
    }
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

    let interval = match (request.sampled_at, &request.previous) {
        (Some(now), Some(previous)) => now
            .checked_sub(previous.sampled_at)
            .filter(|&span| span > 0),
        _ => None,
    };
    let mut previous: HashMap<i64, &BaselineProcess> = HashMap::new();
    if let (Some(_), Some(baseline)) = (interval, &request.previous) {
        for process in &baseline.processes {
            previous.entry(process.process_id).or_insert(process);
        }
    }

    let mut shown = Shown::default();
    let mut categories: [Tally; 5] = Default::default();
    let mut groups: HashMap<usize, (MemoryKind, Tally)> = HashMap::new();
    let mut total = Tally::default();
    let mut baseline = Vec::new();
    for (process, owner) in processes.iter().zip(&owners) {
        let (kind, root) = match *owner {
            Owner::Unowned => continue,
            Owner::Host => (MemoryKind::Host, None),
            Owner::Part { kind, root } => (kind, Some(root)),
        };
        let engines = engine_times(&process.gpu_engines);
        let usage = Usage::since(
            process,
            &engines,
            previous.get(&process.process_id).copied(),
        );
        shown.note(process, &usage);
        categories[kind as usize].add(process, &usage);
        total.add(process, &usage);
        if let Some(root) = root {
            groups
                .entry(root)
                .or_insert_with(|| (kind, Tally::default()))
                .1
                .add(process, &usage);
        }
        if request.sampled_at.is_some() && (process.cpu_time.is_some() || !engines.is_empty()) {
            baseline.push(BaselineProcess {
                process_id: process.process_id,
                start_time: process.start_time,
                cpu_time: process.cpu_time,
                gpu_engines: engines
                    .iter()
                    .map(|(&engine, &busy_time)| GpuEngineTime {
                        engine: engine.to_string(),
                        busy_time,
                    })
                    .collect(),
            });
        }
    }

    let rates = Rates {
        interval: interval.unwrap_or(0),
        logical_processors: request.logical_processors,
    };
    let mut entries: Vec<MemoryEntry> = groups
        .into_iter()
        .map(|(root, (kind, tally))| MemoryEntry {
            kind,
            name: entry_name(&processes[root], roles.get(&processes[root].process_id)),
            process_id: processes[root].process_id,
            bytes: tally.bytes,
            process_count: tally.process_count,
            usage: shown.figures(&tally, &rates),
        })
        .collect();
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
    baseline.sort_by_key(|process| process.process_id);
    Ok(MemorySummary {
        total_bytes: total.bytes,
        usage: shown.figures(&total, &rates),
        categories: MemoryKind::ALL
            .iter()
            .zip(&categories)
            .map(|(&kind, tally)| MemoryCategory {
                kind,
                bytes: tally.bytes,
                process_count: tally.process_count,
                usage: shown.figures(tally, &rates),
            })
            .collect(),
        entries,
        omitted,
        baseline: request.sampled_at.map(|sampled_at| UsageBaseline {
            sampled_at,
            processes: baseline,
        }),
    })
}

/// Busy time of each engine of a list, times of one name added up.
fn engine_times(engines: &[GpuEngineTime]) -> BTreeMap<&str, u64> {
    let mut times = BTreeMap::new();
    for engine in engines {
        let time: &mut u64 = times.entry(engine.engine.as_str()).or_default();
        *time = time.saturating_add(engine.busy_time);
    }
    times
}

/// What one process used between the previous baseline and this table.
#[derive(Default)]
struct Usage<'a> {
    /// Processor microseconds, when both tables have the process's counter.
    cpu_time: Option<u64>,
    /// Busy microseconds of each engine, when both tables have engines for
    /// the process. An engine the baseline does not list counts from zero.
    gpu_engines: Option<BTreeMap<&'a str, u64>>,
}

impl<'a> Usage<'a> {
    /// The use of `process`, whose engines are `engines`, since `previous`.
    /// A baseline row with another start time belongs to an earlier process
    /// with the same ID, so nothing is compared.
    fn since(
        process: &MemoryProcess,
        engines: &BTreeMap<&'a str, u64>,
        previous: Option<&BaselineProcess>,
    ) -> Self {
        let Some(previous) = previous.filter(|previous| previous.start_time == process.start_time)
        else {
            return Usage::default();
        };
        let cpu_time = process
            .cpu_time
            .zip(previous.cpu_time)
            .map(|(now, before)| now.saturating_sub(before));
        let gpu_engines = (!engines.is_empty() && !previous.gpu_engines.is_empty()).then(|| {
            let before = engine_times(&previous.gpu_engines);
            engines
                .iter()
                .map(|(&engine, &now)| {
                    let earlier = before.get(engine).copied().unwrap_or(0);
                    (engine, now.saturating_sub(earlier))
                })
                .collect()
        });
        Usage {
            cpu_time,
            gpu_engines,
        }
    }
}

/// Which figures the summary carries.
#[derive(Default)]
struct Shown {
    compressed_bytes: bool,
    gpu_memory_bytes: bool,
    cpu_percent: bool,
    gpu_percent: bool,
}

impl Shown {
    /// Takes note of the figures one process of the instance has.
    fn note(&mut self, process: &MemoryProcess, usage: &Usage) {
        self.compressed_bytes |= process.compressed_bytes.is_some();
        self.gpu_memory_bytes |= process.gpu_memory_bytes.is_some();
        self.cpu_percent |= usage.cpu_time.is_some();
        self.gpu_percent |= usage.gpu_engines.is_some();
    }

    /// The figures of one group.
    fn figures(&self, tally: &Tally, rates: &Rates) -> UsageFigures {
        UsageFigures {
            compressed_bytes: self.compressed_bytes.then_some(tally.compressed_bytes),
            gpu_memory_bytes: self.gpu_memory_bytes.then_some(tally.gpu_memory_bytes),
            cpu_percent: self.cpu_percent.then(|| rates.cpu(tally.cpu_time)),
            gpu_percent: self.gpu_percent.then(|| rates.gpu(&tally.gpu_engines)),
        }
    }
}

/// How use over the interval becomes a percentage.
struct Rates {
    /// Microseconds between the previous baseline and this table; never zero
    /// when a percentage is shown.
    interval: u64,
    /// Processors a processor percentage is a share of, if any.
    logical_processors: Option<u32>,
}

impl Rates {
    /// Processor percentage of `time` microseconds.
    fn cpu(&self, time: u64) -> f64 {
        match self.logical_processors {
            Some(processors) => tenths(self.share(time, f64::from(processors))).min(100.0),
            None => tenths(self.share(time, 1.0)),
        }
    }

    /// Percentage of the busiest engine.
    fn gpu(&self, engines: &BTreeMap<String, u64>) -> f64 {
        let busiest = engines.values().copied().max().unwrap_or(0);
        tenths(self.share(busiest, 1.0)).min(100.0)
    }

    /// `time` as a percentage of the interval on `width` processors.
    fn share(&self, time: u64, width: f64) -> f64 {
        time as f64 * 100.0 / (self.interval as f64 * width)
    }
}

/// `value` rounded to a tenth.
fn tenths(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// Sums of one group of processes.
#[derive(Default)]
struct Tally {
    bytes: u64,
    process_count: usize,
    compressed_bytes: u64,
    gpu_memory_bytes: u64,
    /// Processor microseconds used over the interval.
    cpu_time: u64,
    /// Busy microseconds of each engine over the interval.
    gpu_engines: BTreeMap<String, u64>,
}

impl Tally {
    /// Adds one process and its use.
    fn add(&mut self, process: &MemoryProcess, usage: &Usage) {
        self.bytes = self.bytes.saturating_add(process.bytes);
        self.process_count += 1;
        self.compressed_bytes = self
            .compressed_bytes
            .saturating_add(process.compressed_bytes.unwrap_or(0));
        self.gpu_memory_bytes = self
            .gpu_memory_bytes
            .saturating_add(process.gpu_memory_bytes.unwrap_or(0));
        self.cpu_time = self.cpu_time.saturating_add(usage.cpu_time.unwrap_or(0));
        for (&engine, &time) in usage.gpu_engines.iter().flatten() {
            let busy = self.gpu_engines.entry(engine.to_string()).or_default();
            *busy = busy.saturating_add(time);
        }
    }
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
            compressed_bytes: None,
            gpu_memory_bytes: None,
            cpu_time: None,
            gpu_engines: Vec::new(),
        }
    }

    fn request(host_process_id: i64, processes: Vec<MemoryProcess>) -> MemoryRequest {
        MemoryRequest {
            host_process_id,
            processes,
            roles: Vec::new(),
            entry_limit: None,
            sampled_at: None,
            previous: None,
            logical_processors: None,
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
        let missing = request(10, vec![process(11, 1, 5)]);
        assert_eq!(summarize(&missing), Err(MemoryError::HostMissing(10)));
        let repeated = request(
            10,
            vec![process(10, 1, 5), process(11, 10, 5), process(11, 10, 6)],
        );
        assert_eq!(summarize(&repeated), Err(MemoryError::DuplicateProcess(11)));
    }

    #[test]
    fn a_deep_chain_is_decided_without_recursion() {
        let depth = 100_000;
        let mut processes = vec![process(1, 0, 1)];
        processes.extend((2..=depth).map(|process_id| process(process_id, process_id - 1, 1)));
        processes.reverse();
        let summary = summarize(&MemoryRequest {
            entry_limit: Some(1),
            ..request(1, processes)
        })
        .expect("chain should summarize");
        assert_eq!(summary.total_bytes, depth as u64);
        assert_eq!(summary.entries.len(), 1);
        assert_eq!(summary.entries[0].process_id, 2);
        assert_eq!(summary.entries[0].process_count, depth as usize - 1);
    }

    #[test]
    fn sums_saturate_instead_of_overflowing() {
        let summary = summarize(&request(1, vec![process(1, 0, u64::MAX), process(2, 1, 1)]))
            .expect("table should summarize");
        assert_eq!(summary.total_bytes, u64::MAX);
    }

    #[test]
    fn long_names_are_shortened() {
        let mut long = process(2, 1, 1);
        long.name = "x".repeat(200);
        let summary =
            summarize(&request(1, vec![process(1, 0, 1), long])).expect("table should summarize");
        let name = &summary.entries[0].name;
        assert_eq!(name.chars().count(), NAME_CHARACTERS);
        assert!(name.ends_with('…'));
    }

    #[test]
    fn a_larger_limit_is_capped() {
        let mut processes = vec![process(1, 0, 1)];
        processes.extend((2..=251).map(|process_id| process(process_id, 1, 1)));
        let summary = summarize(&MemoryRequest {
            entry_limit: Some(10_000),
            ..request(1, processes)
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

    fn engine(engine: &str, busy_time: u64) -> GpuEngineTime {
        GpuEngineTime {
            engine: engine.to_string(),
            busy_time,
        }
    }

    fn baseline(sampled_at: u64, processes: Vec<BaselineProcess>) -> Option<UsageBaseline> {
        Some(UsageBaseline {
            sampled_at,
            processes,
        })
    }

    #[test]
    fn times_of_one_engine_add_up_and_counters_that_went_back_count_nothing() {
        let mut host = process(1, 0, 1);
        host.cpu_time = Some(100);
        host.gpu_engines = vec![engine("gpu", 300_000), engine("gpu", 200_000)];
        let summary = summarize(&MemoryRequest {
            sampled_at: Some(2_000_000),
            previous: baseline(
                1_000_000,
                vec![BaselineProcess {
                    process_id: 1,
                    start_time: None,
                    cpu_time: Some(500),
                    gpu_engines: vec![engine("gpu", 100_000)],
                }],
            ),
            ..request(1, vec![host])
        })
        .expect("table should summarize");
        assert_eq!(summary.usage.cpu_percent, Some(0.0));
        assert_eq!(summary.usage.gpu_percent, Some(40.0));
        let baseline = summary.baseline.expect("a sampled table has a baseline");
        assert_eq!(
            baseline.processes[0].gpu_engines,
            vec![engine("gpu", 500_000)]
        );
    }

    #[test]
    fn a_table_read_no_later_than_the_baseline_has_no_percentages() {
        let mut host = process(1, 0, 1);
        host.cpu_time = Some(2_000);
        let previous = BaselineProcess {
            process_id: 1,
            start_time: None,
            cpu_time: Some(1_000),
            gpu_engines: Vec::new(),
        };
        for sampled_at in [5_000, 4_000] {
            let summary = summarize(&MemoryRequest {
                sampled_at: Some(sampled_at),
                previous: baseline(5_000, vec![previous.clone()]),
                ..request(1, vec![host.clone()])
            })
            .expect("table should summarize");
            assert_eq!(
                summary.usage,
                UsageFigures::default(),
                "sampled at {sampled_at}"
            );
            assert_eq!(summary.baseline.map(|b| b.sampled_at), Some(sampled_at));
        }
    }

    #[test]
    fn a_share_of_no_processor_is_rejected() {
        let rejected = summarize(&MemoryRequest {
            logical_processors: Some(0),
            ..request(1, vec![process(1, 0, 1)])
        });
        assert_eq!(rejected, Err(MemoryError::NoLogicalProcessors));
    }
}
