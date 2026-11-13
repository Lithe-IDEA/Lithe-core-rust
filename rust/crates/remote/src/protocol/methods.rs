//! Method names, event names and their typed parameters.
//!
//! Paths in this protocol are absolute paths on the remote host, using the
//! host's own spelling. This is deliberate: the protocol is a filesystem and
//! process adapter, and toolchains, terminals and debuggers need real host
//! paths. Workspace-relative identifiers are built above this layer.
//!
//! Long-lived resources (processes, terminals, watches, tunnels, walks) are
//! identified by ids the *client* chooses, unique per session. A client can
//! therefore subscribe to a resource's events before creating it, and keeps
//! the same ids after reconnecting. Every event is named `<family>.<name>` and
//! carries `<family>Id`, for example `terminal.output` with `terminalId`.

use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Opens the connection: negotiates the protocol version and returns the host
/// and session facts. It must be the first request of every connection.
pub const SERVER_HELLO: &str = "server.hello";
/// Ends the session and everything it started; the next connection with the
/// same key starts a fresh session.
pub const SERVER_SHUTDOWN: &str = "server.shutdown";
/// Round-trip check that returns an empty object.
pub const SERVER_PING: &str = "server.ping";
/// Lists the running processes, terminals and watches as a [`ServerState`].
pub const SERVER_STATE: &str = "server.state";
/// Starts event delivery to this connection, see [`ResumeParams`].
pub const SERVER_RESUME: &str = "server.resume";
/// Acknowledges received events, see [`AckParams`].
pub const SERVER_ACK: &str = "server.ack";

/// Returns the [`FileMetadata`] of a path without following a final link.
pub const FS_STAT: &str = "fs.stat";
/// Lists one directory as a [`ReadDirResult`].
pub const FS_READ_DIR: &str = "fs.readDir";
/// Reads a byte range of a file, see [`ReadFileParams`].
pub const FS_READ_FILE: &str = "fs.readFile";
/// Replaces a whole file atomically, see [`WriteFileParams`].
pub const FS_WRITE_FILE: &str = "fs.writeFile";
/// Creates a directory, see [`CreateDirParams`].
pub const FS_CREATE_DIR: &str = "fs.createDir";
/// Removes a path; links are removed, never followed.
pub const FS_REMOVE: &str = "fs.remove";
/// Renames or moves a path, see [`RenameParams`].
pub const FS_RENAME: &str = "fs.rename";
/// Copies a file or tree, see [`CopyParams`].
pub const FS_COPY: &str = "fs.copy";
/// Moves an entry to the host account's recoverable system trash.
pub const FS_TRASH: &str = "fs.trash";
/// Resolves every link of a path and returns it as a [`RealPathResult`].
pub const FS_REAL_PATH: &str = "fs.realPath";
/// Sets the permission bits of a path, see [`SetPermissionsParams`].
pub const FS_SET_PERMISSIONS: &str = "fs.setPermissions";
/// Reports properties of the volume holding a path as a [`VolumeInfo`].
pub const FS_VOLUME_INFO: &str = "fs.volumeInfo";
/// Creates a private disposable directory, see [`TempDirectoryParams`].
pub const FS_TEMP_DIRECTORY: &str = "fs.tempDirectory";
/// Connection-owned staging for large files, with verified no-replace publication.
pub const FS_UPLOAD_START: &str = "fs.uploadStart";
/// Appends one chunk to a staged upload and returns the next expected offset.
pub const FS_UPLOAD_CHUNK: &str = "fs.uploadChunk";
/// Verifies a staged upload's size and SHA-256 and publishes it without
/// replacing an existing target.
pub const FS_UPLOAD_COMMIT: &str = "fs.uploadCommit";
/// Discards a staged upload and its staging file.
pub const FS_UPLOAD_ABORT: &str = "fs.uploadAbort";
/// Private runtime snapshots owned by the session, not by a live connection.
pub const FS_CREATE_RUN_DIRECTORY: &str = "fs.createRunDirectory";
/// Removes a run directory this session allocated; releasing it again
/// succeeds without effect.
pub const FS_RELEASE_RUN_DIRECTORY: &str = "fs.releaseRunDirectory";

/// Caller-generated opaque identity, known before allocation can be cancelled.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunDirectoryParams {
    /// 1–64 ASCII letters, digits, `_` or `-`, unique within the session.
    pub directory_id: String,
}

/// Maximum decoded chunk, independent of the full file size.
pub const MAX_UPLOAD_CHUNK_BYTES: usize = 256 * 1024;
/// Maximum staged file, matching the expanded plugin importer budget.
pub const MAX_UPLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// A new upload's destination, expected bytes, integrity and final permissions.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UploadStartParams {
    /// Absolute host path the file is published at; it must not exist yet.
    pub path: String,
    /// Exact byte length of the whole file, at most [`MAX_UPLOAD_BYTES`].
    pub size: u64,
    /// Lower- or upper-case hex SHA-256 of the whole file, 64 characters.
    pub sha256: String,
    /// Permission bits of the published file, `0o000`–`0o777`.
    pub mode: u32,
}

/// One strictly ordered chunk of an existing upload.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UploadChunkParams {
    /// Id the server returned from `fs.uploadStart`.
    pub upload_id: u64,
    /// Byte offset of this chunk; it must equal the offset the previous chunk
    /// acknowledged.
    pub offset: u64,
    /// Base64 bytes, 1 to [`MAX_UPLOAD_CHUNK_BYTES`] once decoded.
    pub data: String,
}

/// Identifies the upload being committed or aborted.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UploadParams {
    /// Id the server returned from `fs.uploadStart`.
    pub upload_id: u64,
}

/// Starts streaming the entries below a root, see [`WalkStartParams`].
pub const WALK_START: &str = "walk.start";
/// Stops a walk; its `walk.done` event reports `cancelled`.
pub const WALK_CANCEL: &str = "walk.cancel";

/// Starts watching a directory tree, see [`WatchStartParams`].
pub const WATCH_START: &str = "watch.start";
/// Stops a watch.
pub const WATCH_STOP: &str = "watch.stop";

/// Starts a process with piped output, see [`SpawnParams`].
pub const PROCESS_SPAWN: &str = "process.spawn";
/// Writes Base64 bytes to a process's standard input; fails with `BUSY` when
/// the process stops reading.
pub const PROCESS_WRITE_STDIN: &str = "process.writeStdin";
/// Closes a process's standard input, so it reads end-of-file.
pub const PROCESS_CLOSE_STDIN: &str = "process.closeStdin";
/// Signals every process of a process's session, see [`KillParams`].
pub const PROCESS_KILL: &str = "process.kill";

/// Opens a pseudo-terminal running a program, see [`TerminalOpenParams`].
pub const TERMINAL_OPEN: &str = "terminal.open";
/// Writes Base64 input to a terminal; fails with `BUSY` when it is not read.
pub const TERMINAL_WRITE: &str = "terminal.write";
/// Changes a terminal's size in character cells.
pub const TERMINAL_RESIZE: &str = "terminal.resize";
/// Sends `SIGHUP` to the terminal's session and `SIGKILL` after 2 seconds.
pub const TERMINAL_CLOSE: &str = "terminal.close";

/// Returns the user's login environment, see [`EnvironmentResolveParams`].
pub const ENVIRONMENT_RESOLVE: &str = "environment.resolve";

/// Connects a tunnel to a remote TCP or Unix socket target; fails with
/// `CONNECT_FAILED` when the target refuses.
pub const TUNNEL_OPEN: &str = "tunnel.open";
/// Sends Base64 bytes through a tunnel; fails with `BUSY` when the peer stops
/// reading.
pub const TUNNEL_WRITE: &str = "tunnel.write";
/// Sends queued data, then shuts down the tunnel's write side; replies keep
/// arriving.
pub const TUNNEL_SHUTDOWN_WRITE: &str = "tunnel.shutdownWrite";
/// Aborts both directions of a tunnel and discards queued data.
pub const TUNNEL_CLOSE: &str = "tunnel.close";
/// Lists the TCP ports listening on the host as a [`ListeningPortsResult`].
pub const PORTS_LISTENING: &str = "ports.listening";

/// Output of a spawned process, carried by [`ProcessOutputEvent`].
pub const EVENT_PROCESS_OUTPUT: &str = "process.output";
/// End of a spawned process, carried by [`ProcessExitEvent`].
pub const EVENT_PROCESS_EXIT: &str = "process.exit";
/// Output of a terminal, carried by [`TerminalOutputEvent`].
pub const EVENT_TERMINAL_OUTPUT: &str = "terminal.output";
/// End of a terminal, carried by [`TerminalExitEvent`].
pub const EVENT_TERMINAL_EXIT: &str = "terminal.exit";
/// Changes below a watched root, carried by [`WatchChangedEvent`].
pub const EVENT_WATCH_CHANGED: &str = "watch.changed";
/// A degraded or ended watch, carried by [`WatchErrorEvent`].
pub const EVENT_WATCH_ERROR: &str = "watch.error";
/// A batch of walked entries, carried by [`WalkEntriesEvent`].
pub const EVENT_WALK_ENTRIES: &str = "walk.entries";
/// End of a walk, carried by [`WalkDoneEvent`].
pub const EVENT_WALK_DONE: &str = "walk.done";
/// Bytes received through a tunnel, carried by [`TunnelDataEvent`].
pub const EVENT_TUNNEL_DATA: &str = "tunnel.data";
/// End of a tunnel, carried by [`TunnelClosedEvent`].
pub const EVENT_TUNNEL_CLOSED: &str = "tunnel.closed";

/// Largest byte count one `fs.readFile` or `fs.writeFile` call may move.
/// Base64 inflates it by a third, which keeps the frame under
/// [`crate::protocol::MAX_FRAME_BYTES`].
pub const MAX_TRANSFER_BYTES: u64 = 8 * 1024 * 1024;

/// Encodes bytes for a JSON `data` field.
pub fn encode_bytes(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Decodes a JSON `data` field.
pub fn decode_bytes(data: &str) -> Result<Vec<u8>, base64::DecodeError> {
    base64::engine::general_purpose::STANDARD.decode(data)
}

// ---------------------------------------------------------------------------
// Server and session
// ---------------------------------------------------------------------------

/// Parameters of `server.hello`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloParams {
    /// Protocol version the client speaks; a different server version fails
    /// with `PROTOCOL_MISMATCH`.
    pub protocol_version: u32,
    /// Free-form client identification for server diagnostics.
    pub client_name: String,
    /// Seconds a persistent session keeps its processes and terminals while
    /// no client is attached. Ignored by `--stdio` servers, whose session ends
    /// with the connection. The server applies the latest value it received.
    #[serde(default)]
    pub idle_timeout_seconds: Option<u64>,
}

/// Result of `server.hello`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloResult {
    /// Protocol version the server speaks, equal to the client's.
    pub protocol_version: u32,
    /// Version of the `lithe-remote-server` executable.
    pub server_version: String,
    /// Facts about the host the client branches on.
    pub host: HostCapabilities,
    /// State of the session this connection attached to.
    pub session: SessionInfo,
}

/// The session a connection attached to.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    /// Whether the session survives disconnection (`--connect` servers).
    pub persistent: bool,
    /// Whether a client was attached to this session before. Its unseen
    /// events are delivered after `server.resume`.
    pub resumed: bool,
    /// Smallest resource id not used yet in this session.
    pub next_resource_id: u64,
    /// Sequence number of the latest event the session produced, `0` before
    /// the first one.
    pub last_event_seq: u64,
}

/// Starts event delivery to this connection. Until it is called, events are
/// kept for the connection, so a client can subscribe to the resources it
/// knows before their missed events arrive.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeParams {
    /// Last event sequence number this client received, from an earlier
    /// connection to the same session. Kept events after it are sent first.
    /// `None` sends every event still kept.
    #[serde(default)]
    pub after_seq: Option<u64>,
}

/// Result of `server.resume`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeResult {
    /// Events after `afterSeq` were dropped because the session's retention
    /// limit was reached. The client must re-read state, for example call
    /// `server.state` and rescan watched trees.
    pub truncated: bool,
}

/// Confirms that the client received every event up to `seq`, which the
/// session may then forget. Clients send it periodically; it has no result.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AckParams {
    /// Sequence number of the latest event the client received.
    pub seq: u64,
}

/// Facts about the remote host that clients must use instead of assuming a
/// platform. A client never branches on "is this macOS"; it reads the
/// capability that the behavior actually depends on.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostCapabilities {
    /// Operating system of the host, for display and diagnostics.
    pub os: OperatingSystem,
    /// Normalized CPU architecture of the running server: `x86_64` or
    /// `aarch64`.
    pub arch: String,
    /// Host name as the host reports it.
    pub host_name: String,
    /// Account the server runs as; `None` when the host cannot name it.
    pub user_name: Option<String>,
    /// Home directory of that account; `None` when it is not set.
    pub home_directory: Option<String>,
    /// Login shell of the remote user, used for terminals and for resolving
    /// the user's environment.
    pub default_shell: Option<String>,
    /// Separator used by host paths. Always `/` for current hosts; carried so
    /// that clients do not hard-code it.
    pub path_separator: String,
    /// How spawned processes are kept from outliving the server.
    pub process: ProcessCapabilities,
    /// How file changes are watched.
    pub watch: WatchCapabilities,
    /// Optional method groups this server implements, sorted.
    pub features: Vec<Feature>,
}

/// Operating system of a remote host.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OperatingSystem {
    /// Linux, including containers and WSL.
    Linux,
    /// macOS.
    Macos,
}

/// Process facts of a remote host.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessCapabilities {
    /// What stops spawned processes from outliving the server.
    pub orphan_protection: OrphanProtection,
}

/// What stops spawned processes from outliving the server.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OrphanProtection {
    /// The kernel kills each process group leader when the server dies, even
    /// if the server is killed without a chance to clean up (Linux).
    ParentDeathSignal,
    /// Process groups are terminated when the server shuts down or its
    /// session expires. A server killed with `SIGKILL` can leave them running
    /// (macOS).
    ServerShutdown,
}

/// File watching facts of a remote host.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchCapabilities {
    /// Kernel facility behind `watch.*`.
    pub backend: WatchBackend,
    /// Upper bound of watched directories imposed by the host, when the
    /// backend watches directories individually (inotify). Large trees beyond
    /// it degrade to partial watching and report `watch.error`.
    pub directory_limit: Option<u64>,
}

/// Kernel facility a host uses to watch files.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WatchBackend {
    /// Linux inotify, one watch per directory.
    Inotify,
    /// macOS FSEvents, one stream per watched tree.
    Fsevents,
}

/// Optional method group a server implements, listed in
/// [`HostCapabilities::features`]. Clients check it before calling the group.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Feature {
    /// `fs.*` methods.
    Filesystem,
    /// Bounded staged uploads with SHA-256 and atomic no-replace publication.
    Upload,
    /// Session-owned private run directories reclaimed when the session ends.
    RunDirectories,
    /// `walk.*` methods and events.
    Walk,
    /// `watch.*` methods and events.
    Watch,
    /// `process.*` methods and events.
    Process,
    /// `terminal.*` methods and events.
    Terminal,
    /// `environment.resolve`.
    Environment,
    /// `tunnel.*` and `ports.listening`.
    Tunnel,
    /// The session survives disconnection when started with `--connect`.
    Persistence,
}

/// Result of `server.state`: the resources a reconnecting client can
/// reattach to.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerState {
    /// Processes the session still owns.
    pub processes: Vec<ProcessState>,
    /// Open terminals.
    pub terminals: Vec<TerminalState>,
    /// Active watches.
    pub watches: Vec<WatchState>,
}

/// A process the session still owns.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessState {
    /// Client-chosen id from `process.spawn`.
    pub process_id: u64,
    /// Operating-system pid of the session leader, for diagnostics only.
    pub pid: u32,
    /// Program and arguments the process was spawned with.
    pub argv: Vec<String>,
    /// The process itself exited, but members of its session still run;
    /// `process.kill` still reaches them.
    pub leader_exited: bool,
}

/// An open terminal.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalState {
    /// Client-chosen id from `terminal.open`.
    pub terminal_id: u64,
    /// Operating-system pid of the terminal's program, for diagnostics only.
    pub pid: u32,
    /// Current width in character cells.
    pub columns: u16,
    /// Current height in character cells.
    pub rows: u16,
}

/// An active watch.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchState {
    /// Client-chosen id from `watch.start`.
    pub watch_id: u64,
    /// Absolute host path of the watched directory.
    pub root: String,
}

// ---------------------------------------------------------------------------
// Filesystem
// ---------------------------------------------------------------------------

/// Parameters of the `fs.*` methods that take only a path.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathParams {
    /// Absolute host path.
    pub path: String,
}

/// Kind of a filesystem entry, never following links.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FileKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link, whatever it points to.
    Symlink,
    /// Sockets, devices and FIFOs.
    Other,
}

/// Metadata of the path itself; symbolic links are not followed.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMetadata {
    /// Kind of the entry itself.
    pub kind: FileKind,
    /// Size in bytes as the host reports it; for a link, the length of its
    /// target text.
    pub size: u64,
    /// Modification time in milliseconds since the Unix epoch. Milliseconds
    /// stay exact in JavaScript numbers, which a future TypeScript client
    /// relies on.
    pub modified_ms: Option<i64>,
    /// No write permission bit is set.
    pub readonly: bool,
    /// Permission bits (`0o755` style), for executable markers and
    /// `fs.setPermissions`.
    pub mode: u32,
    /// Link target as stored in the link, when it is valid UTF-8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symlink_target: Option<String>,
    /// Kind of the resolved target; `None` for a dangling link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symlink_target_kind: Option<FileKind>,
    /// Permissions of the resolved target, never the link's own mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symlink_target_mode: Option<u32>,
}

/// One entry of a `fs.readDir` listing.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryEntry {
    /// File name within the listed directory.
    pub name: String,
    /// Metadata of the entry itself.
    pub metadata: FileMetadata,
}

/// Result of `fs.readDir`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadDirResult {
    /// Sorted by the byte order of `name`.
    pub entries: Vec<DirectoryEntry>,
    /// Entries whose names are not valid UTF-8. They cannot be addressed
    /// through this protocol version, so they are counted rather than
    /// silently dropped.
    pub skipped_non_utf8: u32,
}

/// Parameters of `fs.readFile`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadFileParams {
    /// Absolute host path of the file.
    pub path: String,
    /// Byte offset the read starts at; `0` by default.
    #[serde(default)]
    pub offset: u64,
    /// Requested byte count; at most [`MAX_TRANSFER_BYTES`].
    pub length: u64,
}

/// Result of `fs.readFile`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadFileResult {
    /// Base64 bytes starting at the requested offset.
    pub data: String,
    /// Whether the read reached the end of the file.
    pub eof: bool,
    /// Metadata of the file at the time of the read, for a later
    /// [`FileVersion`] check.
    pub metadata: FileMetadata,
}

/// Identity of the content a client last read, used to reject a write that
/// would overwrite someone else's change.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileVersion {
    /// Size in bytes the client last saw.
    pub size: u64,
    /// Modification time in milliseconds the client last saw.
    pub modified_ms: Option<i64>,
}

/// Parameters of `fs.writeFile`. The file is replaced atomically through a
/// temporary sibling, and writes go through symbolic links.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteFileParams {
    /// Absolute host path of the file.
    pub path: String,
    /// Base64 content that replaces the whole file.
    pub data: String,
    /// When present, the write fails with `CONFLICT` unless the current file
    /// matches it. When absent, the file must not exist yet unless
    /// `overwrite` is set.
    #[serde(default)]
    pub expected: Option<FileVersion>,
    /// Replace an existing file without an `expected` check.
    #[serde(default)]
    pub overwrite: bool,
    /// Create missing parent directories first.
    #[serde(default)]
    pub create_parents: bool,
}

/// Parameters of `fs.createDir`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDirParams {
    /// Absolute host path of the new directory.
    pub path: String,
    /// Create missing parents too, and succeed when the directory exists.
    #[serde(default)]
    pub recursive: bool,
    /// POSIX permissions applied during creation, before any content is written.
    #[serde(default)]
    pub mode: Option<u32>,
}

/// Creates a private disposable directory below the host's temporary root.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TempDirectoryParams {
    /// Diagnostic name, restricted to ASCII letters, digits, `_` and `-`.
    pub prefix: String,
}

/// Parameters of `fs.remove`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveParams {
    /// Absolute host path to remove.
    pub path: String,
    /// Remove a non-empty directory with its contents.
    #[serde(default)]
    pub recursive: bool,
}

/// Parameters of `fs.rename`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameParams {
    /// Absolute host path of the existing entry.
    pub from: String,
    /// Absolute host path it moves to.
    pub to: String,
    /// Replace an existing destination.
    #[serde(default)]
    pub overwrite: bool,
}

/// Copies a file, or a directory tree when `recursive` is set. Symbolic
/// links are recreated as links, never followed, so a copy cannot escape the
/// copied tree.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyParams {
    /// Absolute host path of the source.
    pub from: String,
    /// Absolute host path of the copy.
    pub to: String,
    /// Copy a directory with its contents.
    #[serde(default)]
    pub recursive: bool,
    /// Replace an existing destination. Without it the copy is published by
    /// an atomic no-replace rename, and a concurrently created destination
    /// fails with `ALREADY_EXISTS`.
    #[serde(default)]
    pub overwrite: bool,
}

/// Result of `fs.copy`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyResult {
    /// Sockets, FIFOs and device files inside a copied tree. They are not
    /// copied: reading a FIFO can block forever and devices are not content.
    pub skipped_special_files: u64,
}

/// Parameters of `fs.setPermissions`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPermissionsParams {
    /// Absolute host path.
    pub path: String,
    /// Permission bits, for example `0o755`.
    pub mode: u32,
}

/// Result of `fs.realPath`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RealPathResult {
    /// Absolute host path with every link resolved.
    pub path: String,
}

/// Result of `fs.volumeInfo`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeInfo {
    /// Whether names on the volume containing the path are case-sensitive.
    /// APFS is usually not; most Linux filesystems are. `None` when the
    /// server could not determine it.
    pub case_sensitive: Option<bool>,
}

// ---------------------------------------------------------------------------
// Walk
// ---------------------------------------------------------------------------

/// Streams every entry below `root` in depth-first, name-sorted order. Used
/// for file indexes and "go to file"; symbolic links are reported but not
/// followed.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalkStartParams {
    /// Client-chosen id that the walk's events carry.
    pub walk_id: u64,
    /// Absolute host path of the directory to walk.
    pub root: String,
    /// Entry names that are reported but not descended into, such as
    /// `node_modules` or `target`.
    #[serde(default)]
    pub exclude_names: Vec<String>,
    /// Stops the walk after this many entries and reports `truncated`.
    #[serde(default)]
    pub max_entries: Option<u64>,
}

/// Parameters of `walk.cancel`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalkParams {
    /// Id the walk was started with.
    pub walk_id: u64,
}

/// One walked entry.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalkEntry {
    /// Path relative to the walk root with `/` separators.
    pub path: String,
    /// Kind of the entry itself.
    pub kind: FileKind,
}

/// Parameters of the `walk.entries` event.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalkEntriesEvent {
    /// Id the walk was started with.
    pub walk_id: u64,
    /// Next entries in walk order.
    pub entries: Vec<WalkEntry>,
}

/// Last event of a walk.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalkDoneEvent {
    /// Id the walk was started with.
    pub walk_id: u64,
    /// Entries reported in `walk.entries` events.
    pub total: u64,
    /// The walk stopped at `maxEntries`.
    pub truncated: bool,
    /// The walk stopped because of `walk.cancel`.
    pub cancelled: bool,
    /// Entries skipped because their names are not valid UTF-8.
    pub skipped_non_utf8: u64,
    /// Directories that could not be read, usually for lack of permission.
    pub unreadable_directories: u64,
}

// ---------------------------------------------------------------------------
// Watch
// ---------------------------------------------------------------------------

/// Parameters of `watch.start`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchStartParams {
    /// Client-chosen id that the watch's events carry.
    pub watch_id: u64,
    /// Directory watched recursively.
    pub root: String,
    /// Directory names whose contents are not watched, such as `.git/objects`
    /// style noise or build output. Matched against each path component.
    #[serde(default)]
    pub exclude_names: Vec<String>,
}

/// Result of `watch.start`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchStartResult {
    /// Directories registered with a per-directory backend; `None` for
    /// backends that watch whole trees.
    pub watched_directories: Option<u64>,
}

/// Parameters of `watch.stop`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchParams {
    /// Id the watch was started with.
    pub watch_id: u64,
}

/// How a watched path changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    /// The path appeared.
    Created,
    /// The content or metadata of the path changed.
    Modified,
    /// The path disappeared.
    Deleted,
}

/// One change below a watched root.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    /// Absolute host path.
    pub path: String,
    /// How the path changed.
    pub kind: ChangeKind,
}

/// Coalesced changes, sorted by path. With `overflow` some changes were lost
/// and the client must rescan the watched tree.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchChangedEvent {
    /// Id the watch was started with.
    pub watch_id: u64,
    /// Changes since the previous event, one per path.
    pub changes: Vec<FileChange>,
    /// Changes were lost; the client must rescan the watched tree.
    pub overflow: bool,
}

/// Parameters of the `watch.error` event.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchErrorEvent {
    /// Id the watch was started with.
    pub watch_id: u64,
    /// What went wrong.
    pub code: WatchErrorCode,
    /// Explanation for people.
    pub message: String,
}

/// Why a watch reported `watch.error`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WatchErrorCode {
    /// The host's watch limit was reached; part of the tree is unwatched.
    LimitReached,
    /// The watched root was deleted or moved; the watch has ended.
    RootRemoved,
}

// ---------------------------------------------------------------------------
// Processes
// ---------------------------------------------------------------------------

/// Base environment of a spawned process or terminal.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EnvironmentSource {
    /// The server's own environment, as the transport started it. Over SSH
    /// this usually lacks what login profiles add to `PATH`.
    #[default]
    Server,
    /// The user's login environment from `environment.resolve`, so tools
    /// installed through nvm, SDKMAN or Homebrew are found.
    Login,
}

/// Parameters of `process.spawn`. The process leads a new session, so
/// `process.kill` reaches everything it starts.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnParams {
    /// Client-chosen id that the process's events carry.
    pub process_id: u64,
    /// Program and arguments. The program is resolved through the base
    /// environment's `PATH` when it contains no `/`.
    pub argv: Vec<String>,
    /// Absolute host working directory; the server's own when absent.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Base environment the `env` variables are applied to.
    #[serde(default)]
    pub environment: EnvironmentSource,
    /// Variables added to, or replacing, the base environment.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Keep standard input open for `process.writeStdin`. Otherwise the
    /// process reads end-of-file immediately.
    #[serde(default)]
    pub pipe_stdin: bool,
}

/// Result of `process.spawn`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnResult {
    /// Operating-system pid, for diagnostics only.
    pub pid: u32,
}

/// Parameters of `process.writeStdin`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteStdinParams {
    /// Id the process was spawned with.
    pub process_id: u64,
    /// Base64 bytes for standard input.
    pub data: String,
}

/// Parameters of the `process.*` methods that take only the process id.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessParams {
    /// Id the process was spawned with.
    pub process_id: u64,
}

/// Parameters of `process.kill`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KillParams {
    /// Id the process was spawned with.
    pub process_id: u64,
    /// `SIGKILL` instead of `SIGTERM`. Either signal goes to every process
    /// of the session the process leads, including job-control groups it
    /// created, so build tools do not leave workers behind.
    #[serde(default)]
    pub force: bool,
}

/// Output stream of a spawned process.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OutputStream {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

/// Parameters of the `process.output` event.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessOutputEvent {
    /// Id the process was spawned with.
    pub process_id: u64,
    /// Stream the bytes were read from.
    pub stream: OutputStream,
    /// Base64 bytes in the order the process wrote them to this stream.
    pub data: String,
}

/// Sent once per process, after all of its output events.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessExitEvent {
    /// Id the process was spawned with.
    pub process_id: u64,
    /// Exit status when the process exited normally.
    pub exit_code: Option<i32>,
    /// Terminating signal number when the process did not exit normally.
    pub signal: Option<i32>,
}

// ---------------------------------------------------------------------------
// Terminals
// ---------------------------------------------------------------------------

/// Parameters of `terminal.open`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOpenParams {
    /// Client-chosen id that the terminal's events carry.
    pub terminal_id: u64,
    /// Program and arguments. Defaults to the user's login shell started as
    /// a login shell, which loads the same profile files as an SSH login.
    #[serde(default)]
    pub argv: Option<Vec<String>>,
    /// Absolute host working directory; when absent, the user's home, or `/`
    /// without one.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Variables added to the server environment. `TERM` defaults to
    /// `xterm-256color`.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Initial width in character cells.
    pub columns: u16,
    /// Initial height in character cells.
    pub rows: u16,
}

/// Result of `terminal.open`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOpenResult {
    /// Operating-system pid of the terminal's program, for diagnostics only.
    pub pid: u32,
}

/// Parameters of `terminal.write`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalWriteParams {
    /// Id the terminal was opened with.
    pub terminal_id: u64,
    /// Base64 input bytes, as typed or pasted.
    pub data: String,
}

/// Parameters of `terminal.resize`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalResizeParams {
    /// Id the terminal was opened with.
    pub terminal_id: u64,
    /// New width in character cells.
    pub columns: u16,
    /// New height in character cells.
    pub rows: u16,
}

/// Parameters of `terminal.close`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalParams {
    /// Id the terminal was opened with.
    pub terminal_id: u64,
}

/// Parameters of the `terminal.output` event.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOutputEvent {
    /// Id the terminal was opened with.
    pub terminal_id: u64,
    /// Base64 bytes the terminal's program wrote.
    pub data: String,
}

/// Sent once per terminal, after all of its output.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalExitEvent {
    /// Id the terminal was opened with.
    pub terminal_id: u64,
    /// Exit status when the program exited normally.
    pub exit_code: Option<i32>,
    /// Terminating signal number when the program did not exit normally.
    pub signal: Option<i32>,
}

// ---------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------

/// Parameters of `environment.resolve`. The result is cached for the session.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentResolveParams {
    /// Re-run the login shell instead of returning the cached result, for
    /// example after the user edited a profile file.
    #[serde(default)]
    pub refresh: bool,
}

/// Result of `environment.resolve`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentResolveResult {
    /// Every variable of the resolved environment.
    pub environment: BTreeMap<String, String>,
    /// Where the environment came from.
    pub source: ResolvedEnvironmentSource,
    /// Why a better source was not used, for example a profile that timed out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<String>,
}

/// Where a resolved environment came from, best first.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ResolvedEnvironmentSource {
    /// An interactive login shell, which also reads files such as `.zshrc`
    /// and `.bashrc` where version managers usually add themselves.
    InteractiveLoginShell,
    /// A non-interactive login shell; interactive start-up failed.
    LoginShell,
    /// The server environment; the shell could not be used.
    Server,
}

// ---------------------------------------------------------------------------
// Tunnels and ports
// ---------------------------------------------------------------------------

/// Where a tunnel connects on the remote side.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TunnelTarget {
    /// A TCP endpoint as seen from the host.
    Tcp {
        /// Host name or address, usually a loopback address.
        host: String,
        /// TCP port.
        port: u16,
    },
    /// A Unix domain socket, such as a Docker or database socket.
    Unix {
        /// Absolute host path of the socket.
        path: String,
    },
}

/// Parameters of `tunnel.open`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelOpenParams {
    /// Client-chosen id that the tunnel's events carry.
    pub tunnel_id: u64,
    /// Endpoint the host connects to.
    pub target: TunnelTarget,
}

/// Parameters of `tunnel.write`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelWriteParams {
    /// Id the tunnel was opened with.
    pub tunnel_id: u64,
    /// Base64 bytes for the target.
    pub data: String,
}

/// Parameters of `tunnel.shutdownWrite` and `tunnel.close`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelParams {
    /// Id the tunnel was opened with.
    pub tunnel_id: u64,
}

/// Parameters of the `tunnel.data` event.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelDataEvent {
    /// Id the tunnel was opened with.
    pub tunnel_id: u64,
    /// Base64 bytes the target sent.
    pub data: String,
}

/// Last event of a tunnel. `error` explains an abnormal end.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelClosedEvent {
    /// Id the tunnel was opened with.
    pub tunnel_id: u64,
    /// Why the tunnel failed; absent when both sides closed normally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A TCP port listening on the host.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListeningPort {
    /// Bound address as reported by the host, such as `127.0.0.1`, `::` or
    /// `0.0.0.0`.
    pub address: String,
    /// TCP port.
    pub port: u16,
}

/// Result of `ports.listening`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListeningPortsResult {
    /// Sorted and without duplicates.
    pub ports: Vec<ListeningPort>,
}

/// Output of `lithe-remote-server --identity`, used by the bootstrapper to
/// decide whether an installed binary can be reused.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerIdentity {
    /// Version of the executable.
    pub server_version: String,
    /// Protocol version the executable speaks.
    pub protocol_version: u32,
    /// Lower-case hex SHA-256 of the executable file itself.
    pub sha256: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tunnel_targets_are_tagged_by_kind() {
        assert_eq!(
            serde_json::to_value(TunnelTarget::Tcp {
                host: "127.0.0.1".into(),
                port: 8080
            })
            .unwrap(),
            json!({"kind": "tcp", "host": "127.0.0.1", "port": 8080})
        );
        let unix: TunnelTarget =
            serde_json::from_value(json!({"kind": "unix", "path": "/var/run/docker.sock"}))
                .unwrap();
        assert_eq!(
            unix,
            TunnelTarget::Unix {
                path: "/var/run/docker.sock".into()
            }
        );
    }
}
