//! Recursive file watching.
//!
//! The platform backend (inotify or FSEvents) reports raw changes to a
//! coalescer, which merges bursts, such as a build writing thousands of files
//! or an editor's save sequence, into sorted batches. A batch is sent after
//! [`QUIET_PERIOD`] without new changes, or at the latest [`MAX_BATCH_DELAY`]
//! after its first change. When changes are lost, the batch carries
//! `overflow` and the client rescans instead of trusting partial data.

use crate::protocol::methods::{
    self, ChangeKind, FileChange, WatchChangedEvent, WatchErrorCode, WatchErrorEvent, WatchParams,
    WatchStartParams, WatchStartResult, WatchState,
};
use crate::protocol::{ErrorCode, RemoteError};
use crate::server::connection::Context;
use crate::server::fs::host_path;
use crate::server::platform::watcher;
use crate::server::resources::Registry;
use crate::server::session::Session;
use crate::server::threads::Parked;
use crate::server::{parse, to_value};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{RecvTimeoutError, SendError};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const QUIET_PERIOD: Duration = Duration::from_millis(50);
const MAX_BATCH_DELAY: Duration = Duration::from_millis(250);

/// Changes per batch before the batch degrades to `overflow`. Beyond this
/// a rescan is cheaper for the client than applying individual changes.
const MAX_BATCH_CHANGES: usize = 5000;

/// Path bytes before JSON escaping (at most sixfold). Leaves frame headroom
/// for change kinds and the envelope even for control characters in names.
const MAX_BATCH_PATH_BYTES: usize = 1024 * 1024;

/// Backend pressure is bounded separately from the coalesced event frame.
const MAX_RAW_CHANGES: usize = 2048;
const MAX_RAW_PATH_BYTES: usize = 1024 * 1024;
const MAX_LIMIT_MESSAGE_CHARS: usize = 1024;

/// A change as reported by a backend, before coalescing.
pub(crate) enum RawEvent {
    Change(PathBuf, ChangeKind),
    /// The backend dropped events.
    Overflow,
    /// Only per-directory backends (inotify) have a limit.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    LimitReached(String),
    RootRemoved,
}

/// Directory names below the root that are neither watched nor reported.
pub(crate) struct Excludes {
    root: PathBuf,
    names: BTreeSet<String>,
}

impl Excludes {
    pub(crate) fn new(root: PathBuf, names: Vec<String>) -> Self {
        Self {
            root,
            names: names.into_iter().collect(),
        }
    }

    pub(crate) fn contains(&self, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(&self.root) else {
            return false;
        };
        relative.components().any(|component| {
            component
                .as_os_str()
                .to_str()
                .is_some_and(|name| self.names.contains(name))
        })
    }

    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub(crate) fn is_excluded_name(&self, name: &std::ffi::OsStr) -> bool {
        name.to_str().is_some_and(|name| self.names.contains(name))
    }
}

/// Nonblocking backend queue. Control events survive a full change queue.
pub(crate) struct Sink(Arc<RawQueue>);

pub(crate) struct RawReceiver(Arc<RawQueue>);

struct RawQueue {
    state: Mutex<RawState>,
    ready: Condvar,
}

/// Change paths are bounded by both count and bytes; overflow is sticky until read.
struct RawState {
    changes: VecDeque<(PathBuf, ChangeKind)>,
    bytes: usize,
    overflow: bool,
    limit: Option<String>,
    root_removed: bool,
    closed: bool,
    receiving: bool,
}

fn raw_channel() -> (Sink, RawReceiver) {
    let queue = Arc::new(RawQueue {
        state: Mutex::new(RawState {
            changes: VecDeque::new(),
            bytes: 0,
            overflow: false,
            limit: None,
            root_removed: false,
            closed: false,
            receiving: true,
        }),
        ready: Condvar::new(),
    });
    (Sink(Arc::clone(&queue)), RawReceiver(queue))
}

impl Sink {
    pub(crate) fn send(&self, event: RawEvent) -> Result<(), SendError<RawEvent>> {
        let mut state = crate::server::lock(&self.0.state);
        if !state.receiving {
            return Err(SendError(event));
        }
        match event {
            RawEvent::Change(path, kind) if !state.overflow => {
                let bytes = path.as_os_str().len();
                if state.changes.len() >= MAX_RAW_CHANGES
                    || state.bytes.saturating_add(bytes) > MAX_RAW_PATH_BYTES
                {
                    state.changes.clear();
                    state.bytes = 0;
                    state.overflow = true;
                } else {
                    state.bytes += bytes;
                    state.changes.push_back((path, kind));
                }
            }
            RawEvent::Change(..) => {}
            RawEvent::Overflow => {
                state.changes.clear();
                state.bytes = 0;
                state.overflow = true;
            }
            RawEvent::LimitReached(message) => {
                state.limit = Some(message.chars().take(MAX_LIMIT_MESSAGE_CHARS).collect());
            }
            RawEvent::RootRemoved => state.root_removed = true,
        }
        self.0.ready.notify_one();
        Ok(())
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        crate::server::lock(&self.0.state).closed = true;
        self.0.ready.notify_one();
    }
}

impl RawReceiver {
    fn recv(&self) -> Result<RawEvent, RecvTimeoutError> {
        self.receive(None)
    }

    fn recv_timeout(&self, timeout: Duration) -> Result<RawEvent, RecvTimeoutError> {
        self.receive(Some(Instant::now() + timeout))
    }

    fn receive(&self, deadline: Option<Instant>) -> Result<RawEvent, RecvTimeoutError> {
        let mut state = crate::server::lock(&self.0.state);
        loop {
            if std::mem::take(&mut state.overflow) {
                return Ok(RawEvent::Overflow);
            }
            if let Some((path, kind)) = state.changes.pop_front() {
                state.bytes -= path.as_os_str().len();
                return Ok(RawEvent::Change(path, kind));
            }
            if let Some(message) = state.limit.take() {
                return Ok(RawEvent::LimitReached(message));
            }
            if std::mem::take(&mut state.root_removed) {
                return Ok(RawEvent::RootRemoved);
            }
            if state.closed {
                return Err(RecvTimeoutError::Disconnected);
            }
            state = if let Some(deadline) = deadline {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(RecvTimeoutError::Timeout);
                }
                self.0
                    .ready
                    .wait_timeout(state, remaining)
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .0
            } else {
                self.0
                    .ready
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
            };
        }
    }
}

impl Drop for RawReceiver {
    fn drop(&mut self) {
        let mut state = crate::server::lock(&self.0.state);
        state.receiving = false;
        state.changes.clear();
        state.bytes = 0;
    }
}

pub(crate) struct WatchTable {
    registry: Registry<RunningWatch>,
}

struct RunningWatch {
    root: String,
    /// Dropping the backend handle stops the backend, which closes the
    /// coalescer's channel and ends its thread.
    _backend: watcher::Watcher,
}

impl WatchTable {
    pub(crate) fn new() -> Self {
        Self {
            registry: Registry::new("watch"),
        }
    }

    pub(crate) fn handle(
        &self,
        context: &Context,
        method: &str,
        params: Value,
    ) -> Result<Value, RemoteError> {
        match method {
            methods::WATCH_START => to_value(self.start(&context.session, parse(params)?)?),
            methods::WATCH_STOP => {
                let params: WatchParams = parse(params)?;
                self.registry
                    .remove(params.watch_id)
                    .map(|_| Value::Null)
                    .ok_or_else(|| self.registry.unknown(params.watch_id))
            }
            _ => Err(RemoteError::new(
                ErrorCode::UnknownMethod,
                format!("unknown method {method}"),
            )),
        }
    }

    pub(crate) fn states(&self) -> Vec<WatchState> {
        self.registry
            .lock()
            .iter()
            .map(|(watch_id, watch)| WatchState {
                watch_id: *watch_id,
                root: watch.root.clone(),
            })
            .collect()
    }

    pub(crate) fn stop_all(&self) {
        let watches = std::mem::take(&mut *self.registry.lock());
        drop(watches);
    }

    fn start(
        &self,
        session: &Arc<Session>,
        params: WatchStartParams,
    ) -> Result<WatchStartResult, RemoteError> {
        session.ids.observe(params.watch_id);
        self.registry.ensure_free(params.watch_id)?;
        let root = host_path(&params.root)?;
        if !std::fs::metadata(&root)
            .map_err(|error| crate::server::fs::io_error(error, &params.root))?
            .is_dir()
        {
            return Err(RemoteError::new(
                ErrorCode::NotADirectory,
                "only directories can be watched",
            ));
        }
        // The coalescer starts before the backend, so a host out of threads
        // refuses the watch without a backend that nobody would stop.
        let watch_id = params.watch_id;
        let coalescer = {
            let session = Arc::clone(session);
            Parked::start("remote-watch", move |raw: RawReceiver| {
                coalesce(raw, |batch| deliver(&session, watch_id, batch));
            })?
        };
        let excludes = Arc::new(Excludes::new(root.clone(), params.exclude_names));
        let (sink, raw) = raw_channel();
        let (backend, watched_directories) = watcher::start(root, &params.root, excludes, sink)?;
        self.registry.insert(
            watch_id,
            RunningWatch {
                root: params.root,
                _backend: backend,
            },
        )?;
        coalescer.run(raw);
        Ok(WatchStartResult {
            watched_directories,
        })
    }
}

/// What the coalescer hands to delivery.
pub(crate) enum Delivery {
    Changes {
        changes: Vec<FileChange>,
        overflow: bool,
    },
    Error(WatchErrorCode, String),
}

fn deliver(session: &Session, watch_id: u64, delivery: Delivery) {
    match delivery {
        Delivery::Changes { changes, overflow } => session.link.emit(
            methods::EVENT_WATCH_CHANGED,
            WatchChangedEvent {
                watch_id,
                changes,
                overflow,
            },
        ),
        Delivery::Error(code, message) => {
            if code == WatchErrorCode::RootRemoved {
                session.watches.registry.remove(watch_id);
            }
            session.link.emit(
                methods::EVENT_WATCH_ERROR,
                WatchErrorEvent {
                    watch_id,
                    code,
                    message,
                },
            );
        }
    }
}

#[derive(Default)]
struct Batch {
    changes: BTreeMap<PathBuf, ChangeKind>,
    bytes: usize,
    overflow: bool,
}

impl Batch {
    fn add(&mut self, path: PathBuf, kind: ChangeKind) {
        if self.overflow {
            return;
        }
        let merged = match (self.changes.get(&path), kind) {
            // A file created and then written in one burst is still new.
            (Some(ChangeKind::Created), ChangeKind::Modified) => ChangeKind::Created,
            // Deleted and recreated, as atomic saves do: the client keeps
            // its entry and reloads the content.
            (Some(ChangeKind::Deleted), ChangeKind::Created) => ChangeKind::Modified,
            (_, kind) => kind,
        };
        if !self.changes.contains_key(&path) {
            self.bytes = self.bytes.saturating_add(path.as_os_str().len());
        }
        self.changes.insert(path, merged);
        if self.changes.len() > MAX_BATCH_CHANGES || self.bytes > MAX_BATCH_PATH_BYTES {
            self.changes.clear();
            self.bytes = 0;
            self.overflow = true;
        }
    }

    fn take(&mut self) -> Option<Delivery> {
        let batch = std::mem::take(self);
        if batch.changes.is_empty() && !batch.overflow {
            return None;
        }
        let mut overflow = batch.overflow;
        let mut changes = Vec::with_capacity(batch.changes.len());
        for (path, kind) in batch.changes {
            match path.into_os_string().into_string() {
                Ok(path) => changes.push(FileChange { path, kind }),
                // Not addressable in this protocol version; a rescan shows it.
                Err(_) => overflow = true,
            }
        }
        Some(Delivery::Changes { changes, overflow })
    }
}

/// Merges raw events into batches until the backend closes the channel.
pub(crate) fn coalesce(raw: RawReceiver, mut deliver: impl FnMut(Delivery)) {
    let mut batch = Batch::default();
    loop {
        // Wait for the first event of a batch without a deadline.
        let Ok(first) = raw.recv() else {
            return;
        };
        let started = Instant::now();
        let mut next = Some(first);
        loop {
            match next.take() {
                Some(RawEvent::Change(path, kind)) => batch.add(path, kind),
                Some(RawEvent::Overflow) => {
                    batch.changes.clear();
                    batch.bytes = 0;
                    batch.overflow = true;
                }
                Some(RawEvent::LimitReached(message)) => {
                    if let Some(changes) = batch.take() {
                        deliver(changes);
                    }
                    deliver(Delivery::Error(WatchErrorCode::LimitReached, message));
                }
                Some(RawEvent::RootRemoved) => {
                    if let Some(changes) = batch.take() {
                        deliver(changes);
                    }
                    deliver(Delivery::Error(
                        WatchErrorCode::RootRemoved,
                        "the watched directory was removed".into(),
                    ));
                    return;
                }
                None => {}
            }
            let waited = started.elapsed();
            if waited >= MAX_BATCH_DELAY {
                break;
            }
            match raw.recv_timeout(QUIET_PERIOD.min(MAX_BATCH_DELAY - waited)) {
                Ok(event) => next = Some(event),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    if let Some(changes) = batch.take() {
                        deliver(changes);
                    }
                    return;
                }
            }
        }
        if let Some(changes) = batch.take() {
            deliver(changes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_backpressure_preserves_overflow_and_root_removal() {
        let (sender, receiver) = raw_channel();
        for index in 0..=MAX_RAW_CHANGES {
            sender
                .send(RawEvent::Change(
                    format!("/w/{index}").into(),
                    ChangeKind::Modified,
                ))
                .unwrap();
        }
        sender
            .send(RawEvent::LimitReached("watch limit".into()))
            .unwrap();
        sender.send(RawEvent::RootRemoved).unwrap();
        drop(sender);
        let mut deliveries = Vec::new();
        coalesce(receiver, |delivery| deliveries.push(delivery));
        assert!(deliveries
            .iter()
            .any(|delivery| matches!(delivery, Delivery::Changes { overflow: true, .. })));
        assert!(deliveries
            .iter()
            .any(|delivery| matches!(delivery, Delivery::Error(WatchErrorCode::LimitReached, _))));
        assert!(matches!(
            deliveries.last(),
            Some(Delivery::Error(WatchErrorCode::RootRemoved, _))
        ));
    }

    #[test]
    fn a_batch_with_long_paths_requires_a_rescan_before_it_exceeds_the_frame_budget() {
        let mut batch = Batch::default();
        for index in 0..300 {
            batch.add(
                format!("/w/{index}/{}", "x".repeat(4096)).into(),
                ChangeKind::Modified,
            );
        }
        assert!(
            matches!(batch.take(), Some(Delivery::Changes { changes, overflow: true }) if changes.is_empty())
        );
    }

    fn changes_of(deliveries: &[Delivery]) -> Vec<(String, ChangeKind)> {
        deliveries
            .iter()
            .flat_map(|delivery| match delivery {
                Delivery::Changes { changes, .. } => changes
                    .iter()
                    .map(|change| (change.path.clone(), change.kind))
                    .collect(),
                Delivery::Error(..) => Vec::new(),
            })
            .collect()
    }

    // An editor's atomic save deletes and recreates the file, and a new file
    // is usually written right after creation. Clients must see one
    // `modified` and one `created`, not the raw sequence.
    #[test]
    fn bursts_merge_into_one_sorted_batch() {
        let (sender, receiver) = raw_channel();
        for (path, kind) in [
            ("/w/b.txt", ChangeKind::Deleted),
            ("/w/a.txt", ChangeKind::Created),
            ("/w/b.txt", ChangeKind::Created),
            ("/w/a.txt", ChangeKind::Modified),
        ] {
            sender.send(RawEvent::Change(path.into(), kind)).unwrap();
        }
        drop(sender);
        let mut deliveries = Vec::new();
        coalesce(receiver, |delivery| deliveries.push(delivery));
        assert_eq!(
            changes_of(&deliveries),
            vec![
                ("/w/a.txt".into(), ChangeKind::Created),
                ("/w/b.txt".into(), ChangeKind::Modified),
            ]
        );
    }

    #[test]
    fn lost_events_turn_the_batch_into_an_overflow() {
        let (sender, receiver) = raw_channel();
        sender
            .send(RawEvent::Change("/w/a".into(), ChangeKind::Created))
            .unwrap();
        sender.send(RawEvent::Overflow).unwrap();
        sender
            .send(RawEvent::Change("/w/b".into(), ChangeKind::Created))
            .unwrap();
        drop(sender);
        let mut deliveries = Vec::new();
        coalesce(receiver, |delivery| deliveries.push(delivery));
        assert!(matches!(
            deliveries.as_slice(),
            [Delivery::Changes { changes, overflow: true }] if changes.is_empty()
        ));
    }

    #[test]
    fn excluded_names_match_any_component_below_the_root() {
        let excludes = Excludes::new("/w".into(), vec!["node_modules".into(), "target".into()]);
        assert!(excludes.contains(Path::new("/w/node_modules/x/index.js")));
        assert!(excludes.contains(Path::new("/w/app/target/classes")));
        assert!(!excludes.contains(Path::new("/w/src/target.rs")));
        // The root's own ancestors never count, even if one is named target.
        let nested = Excludes::new("/srv/target/w".into(), vec!["target".into()]);
        assert!(!nested.contains(Path::new("/srv/target/w/src/main.rs")));
    }
}
