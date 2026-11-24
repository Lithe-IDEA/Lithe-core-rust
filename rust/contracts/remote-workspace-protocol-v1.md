# Remote workspace protocol v1

The protocol between a Lithe client and `lithe-remote-server`, which runs at
the end of a transport chain (SSH host, container, or a chain of both). The
Rust types in `rust/crates/remote/src/protocol/` are the reference
implementation; this document is the compatibility surface for other
clients. Design rationale lives in
`.agents/notes/proposed/architecture/2026-10-24-remote-development-workspace-host.md`.

## Starting the server

| Command | Session |
| --- | --- |
| `lithe-remote-server --stdio` | Ends with the connection, terminating everything it started. |
| `lithe-remote-server --connect <key> [--state-dir <dir>]` | Persistent: relays to the daemon of session `<key>`, starting it if needed. |
| `lithe-remote-server --target-identity --state-dir <dir>` | Prints the non-secret identity of the final account's server state before a client chooses a session key. |

`<key>` is 1–64 characters from `[A-Za-z0-9._-]`. The daemon keeps its
socket, lock and log under `<dir>` (default `$HOME/.lithe-server`) in
directories only the user can access, and rejects peers with another user
id. A session has one client at a time: a new connection that presents the
session token (see below) replaces the current one. The idle timeout runs
while the session has no client, unless its last client sent `server.keep`
and the session still runs processes or open terminals; when it passes, the
session ends with everything it runs. It is 10 minutes unless `server.hello` requests another with
`idleTimeoutSeconds`, at most 24 hours, and the latest request applies. A
connection the daemon cannot set up is closed without ending the session.
The daemon log is created `0600`, never through a symbolic link, and moves to
`<name>.log.1` once it reaches 1 MiB. Launchers serialize socket rechecking and daemon creation with a per-identity
`.start` advisory lock in the private `run/` directory, bounded by the existing
startup deadline. The lock is released when its owning descriptor closes; its
inode is retained so concurrent waiters cannot lock different files. This keeps
a redundant launcher from restarting an explicitly closed session.

When a chain starts with an SSH hop, the reference client starts `ssh` with
only the local variables OpenSSH itself needs (home, user, shell, search
path, temporary directory, agent socket, Kerberos and the Windows system
variables), followed by its own AskPass variables. ssh sends the remote host
every variable a `SendEnv` pattern names, many systems ship
`SendEnv LANG LC_*`, and no command-line option can withdraw a pattern, so
this is the only way to keep local locale, proxy and credential variables
off the host; the server runs with the remote user's login environment.
Other clients should do the same.

## Stream

1. The server writes the preamble `\nLITHE-REMOTE-PROTOCOL/1\n` once per
   connection. Clients discard up to 64 KiB of bytes before it (shell
   banners) and report them as diagnostics when the preamble never arrives.
2. Every following unit is a frame: a 4-byte big-endian length and that many
   bytes of UTF-8 JSON. Frames larger than 16 MiB are rejected.
3. Closing the client-to-server stream is a disconnect.

The reference server bounds response serialization to the frame limit. A result
that exceeds it becomes a `TOO_LARGE` error for that request, and subsequent
responses remain usable. Event producers bound their own batches; events cannot
be replaced by a request error and are never silently discarded by the writer.

All traffic, including tunnels, uses this stream. The server listens on a
network port only for a forwarding rule the user added (`listener.open`, see
"Listeners"); such a port belongs to the connection that opened it and
closes with it. Local port forwards are the client's: the reference client
listens on loopback addresses only, because a remote development server
usually has no authentication of its own, and bridges at most 64 connections
per forward, leaving further ones in the system's listen queue until one
ends. Other clients should keep both rules.

## Messages

```json
{"type": "request", "id": 1, "method": "fs.stat", "params": {"path": "/srv"}}
{"type": "response", "id": 1, "result": {"kind": "directory", "size": 4096, "modifiedMs": 1761300000000, "readonly": false, "mode": 493}}
{"type": "response", "id": 2, "error": {"code": "NOT_FOUND", "message": "…", "details": {"path": "/x", "osError": 2}}}
{"type": "event", "event": "process.exit", "params": {"processId": 1, "exitCode": 0, "signal": null}, "seq": 42}
```

- Request `id`s are chosen by the client and unique per connection.
  Responses may arrive in any order. A client that stops waiting discards a
  late response.
- The first request must be `server.hello`; anything earlier fails with
  `INVALID_REQUEST`. Events reach a connection only after it calls
  `server.resume`, once.
- Every event carries `seq`, increasing by one per event in the session.
- Field names are camelCase. Byte payloads (`data`) are standard Base64.
- Paths are absolute paths on the remote host. Relative paths fail with
  `INVALID_PATH`.
- Queued input and resize requests — `process.writeStdin`, `process.closeStdin`,
  `terminal.write`, `terminal.resize`, `tunnel.write`, `tunnel.close`,
  `walk.cancel` — are applied in the order sent. Potentially blocking creation
  and teardown run on workers; other requests may complete in any order.
- At most 64 requests run on workers at once; a further one fails with
  `BUSY` and changes nothing. When the host cannot start a thread that a
  request or a new process, terminal, tunnel, watch or walk needs, the
  request fails with `BUSY` the same way, before it has any effect.
  `environment.resolve` instead falls back to the server's environment and
  names the failure in `diagnostics`.
- A `process.kill`, `terminal.close`, or `watch.stop` received while the same
  resource is starting records teardown before worker scheduling. The creating
  worker applies it after publication, even if the close worker is rejected as
  `BUSY`, its caller times out, or the connection drops. A close waiter has a
  60-second deadline; expiry does not abandon the recorded teardown. Malformed
  controls never cancel creation. Forced kills cannot be downgraded by a later
  non-forced kill. Unrelated resources remain independent and no registry lock
  covers spawning, filesystem I/O, or process-tree termination.
- A nonpersistent disconnect and every session shutdown also reclaim late
  publications. Persistent disconnects alone retain resources for reconnection;
  explicit shutdown or idle expiry must not leave a late-created resource alive.

## Resources and events

Processes, terminals, watches, tunnels and walks are identified by ids the
**client** chooses, unique within the session across reconnections (start
from `session.nextResourceId` in the hello result). Reusing the id of a live
resource fails with `RESOURCE_ID_IN_USE`; an unknown id fails with
`UNKNOWN_RESOURCE`.

Every event is named `<family>.<name>` and carries `<family>Id`, so a client
can route `terminal.output` with `terminalId: 7` to terminal 7, and can
subscribe before creating the resource.

## Methods

| Method | Params | Result |
| --- | --- | --- |
| `server.hello` | `protocolVersion`, `clientName`, `idleTimeoutSeconds?`, `sessionToken?` | `protocolVersion`, `serverVersion`, `host`, `session`; `INVALID_SESSION_TOKEN` closes the connection |
| `server.resume` | `afterSeq?` | `truncated`; then every kept event after `afterSeq`, followed by live events |
| `server.ack` | `seq` | `null`; events up to `seq` may be forgotten |
| `server.shutdown` | — | `null`; the session ends |
| `server.keep` | — | `null`; the running processes and terminals outlast this client; `UNSUPPORTED` on a `--stdio` session |
| `server.ping` | — | `{}` |
| `server.state` | — | running `processes` (`processId`, `pid`, `argv`, `label?`, `runningMs`, `leaderExited`), `terminals` (`terminalId`, `pid`, `argv`, `label?`, `runningMs`, `columns`, `rows`), `watches` |
| `fs.stat` | `path` | metadata of the path itself (links not followed) |
| `fs.readDir` | `path` | `entries` sorted by name byte order, `skippedNonUtf8` |
| `fs.readFile` | `path`, `offset`, `length` (≤ 8 MiB) | `data`, `eof`, `metadata`; only a regular file is read |
| `fs.writeFile` | `path`, `data` (≤ 8 MiB), `expected?`, `overwrite`, `createParents`, `root?` | metadata after the write |
| `fs.createDir` | `path`, `recursive`, `mode?` (at most `0o7777`) | `null` |
| `fs.remove` | `path`, `recursive` | `null`; links are removed, never followed |
| `fs.rename` | `from`, `to`, `overwrite` | `null` |
| `fs.copy` | `from`, `to`, `recursive`, `overwrite` | `skippedSpecialFiles`; links are copied as links |
| `fs.trash` | `path` | `null`; moves the entry to the host account's recoverable system trash |
| `fs.realPath` | `path` | `path` with links resolved |
| `fs.setPermissions` | `path`, `mode` (≤ `0o7777`) | `null` |
| `fs.volumeInfo` | `path` | `caseSensitive` (`null` when unknown) |
| `fs.uploadStart` | `path`, `size`, `sha256`, `mode` | `uploadId` |
| `fs.uploadChunk` | `uploadId`, `offset`, `data` | next `offset` |
| `fs.uploadCommit` | `uploadId` | verified `size`, `sha256` |
| `fs.uploadAbort` | `uploadId` | `null` |
| `fs.createRunDirectory` | `directoryId` | private session-owned `path` |
| `fs.releaseRunDirectory` | `directoryId` | `null`; idempotent release of this session's allocation |
| `walk.start` | `walkId`, `root`, `excludeNames`, `maxEntries?` | `null`; entries follow as events |
| `walk.cancel` | `walkId` | `null` |
| `watch.start` | `watchId`, `root`, `excludeNames` | `watchedDirectories` (per-directory backends) |
| `watch.stop` | `watchId` | `null` |
| `process.spawn` | `processId`, `argv`, `cwd?`, `environment` (`server` / `login`), `env`, `pipeStdin`, `label?` | `pid` |
| `process.writeStdin` | `processId`, `data` | `null`; `BUSY` when the process stops reading |
| `process.closeStdin` | `processId` | `null` |
| `process.kill` | `processId`, `force` | `null`; signals every process of its session, also after `process.exit` while members run; `UNKNOWN_RESOURCE` once the session is empty |
| `terminal.open` | `terminalId`, `argv?`, `cwd?`, `env`, `columns`, `rows`, `label?` | `pid`; default program is the login shell |
| `terminal.write` | `terminalId`, `data` | `null`; `BUSY` when not read |
| `terminal.resize` | `terminalId`, `columns`, `rows` | `null` |
| `terminal.close` | `terminalId` | `null`; `SIGHUP` to its session, `SIGKILL` after 2 s |
| `environment.resolve` | `refresh` | `environment`, `source`, `diagnostics?` |
| `tunnel.open` | `tunnelId`, `target` (`{kind: tcp, host, port}` / `{kind: unix, path}`) | `null`; `CONNECT_FAILED` on failure; `BUSY` while the session has 256 open tunnels |
| `tunnel.write` | `tunnelId`, `data` | `null`; `BUSY` when the peer stops reading |
| `tunnel.shutdownWrite` | `tunnelId` | `null`; queued data is sent, then the write side is shut down; replies keep arriving |
| `tunnel.close` | `tunnelId` | `null`; aborts both directions, queued data is discarded |
| `listener.open` | `listenerId`, `host` (an IP address), `port` (`0`: the host chooses), `relay?` (a `tunnel.open` target) | `port`; `ADDRESS_IN_USE` when another socket holds the port, `PERMISSION_DENIED` when the host refuses it, `BUSY` while the session has 64 listeners |
| `listener.adopt` | `listenerId`, `acceptId`, `tunnelId` | `null`; the waiting connection becomes that tunnel; `UNKNOWN_RESOURCE` when it no longer waits |
| `listener.close` | `listenerId` | `listenerId`, `port`, `connections`, `failedConnections`; closes the port and every connection it relays or still holds |
| `listener.list` | — | `listeners` by `listenerId`: `listenerId`, `port`, `connections`, `failedConnections` |
| `ports.listening` | — | `ports` sorted by `address` and `port`, one entry for each: `address`, `port`, and `pid` and `process` (the command name) when the server's user can inspect the owning process |

`fs.readFile` opens the path without blocking and checks the type of the
opened file, so a FIFO without a writer never holds the request and a path
replaced between `fs.stat` and the read is still checked. A directory fails
with `IS_A_DIRECTORY`; sockets, FIFOs and devices fail with `UNSUPPORTED`.
Metadata includes optional `revision`, an opaque identity incorporating the
host's file identity, size and precise modification/change timestamps. Clients
compare it exactly and return it in `expected`; they must not interpret its
encoding. Older servers may omit it.

`fs.readDir` examines at most 20,000 directory entries, including names that are
not UTF-8, and bounds serialized entries below the frame limit with space for
the response envelope. Exceeding either budget returns `TOO_LARGE` rather than
a successful partial listing. Recursive indexes should use `walk.start`.

`fs.copy` refuses, with `INVALID_REQUEST`, to copy a file onto itself —
directly, through a symbolic link in either direction or onto a hard link —
and to copy a directory into itself after resolving links. Sockets, FIFOs and
devices are never copied: at the top level the request fails with
`UNSUPPORTED`, inside a tree they are skipped and counted.
With `overwrite: false`, a private sibling staging directory holds the copy
until an atomic no-replace rename publishes it. A concurrent destination
creation fails with `ALREADY_EXISTS` and leaves that destination unchanged.

`fs.trash` uses the host's native trash implementation (Freedesktop Trash on
Linux and the system Trash on macOS). Recovery content and metadata belong to
the remote account, never to the client placeholder or installation. A host
without usable trash storage returns an error and retains the original entry;
it does not fall back to permanent deletion.

`fs.writeFile` replaces the file atomically through a temporary sibling. With
`expected` (`size`, `modifiedMs`, optional `revision`) it fails with `CONFLICT`
unless the file still matches; without it, an existing file is only replaced
when `overwrite` is set. The revision catches an inode replacement or precise
timestamp change even when size and millisecond modification time are unchanged.
A write with neither `expected` nor `overwrite` publishes by atomic no-replace
linking: a concurrently created destination is retained and the write fails
with `ALREADY_EXISTS`. A replacement's temporary file is created with permissions
no broader than the original, before any content is written.

The optional `root` requires the `guardedWrites` feature. Editor clients send
their workspace root and refuse an older server instead of retrying without the
boundary. The requested path must be lexically below that root with no `..`
components. The server resolves the root, opens it through directory handles,
and opens each descendant directory without following symbolic links. The leaf
must not be a symbolic link either. Missing descendants are created only with
`createParents` and are reopened without following links. Staging and publication
use the same parent handle; refused boundary or symlink crossings return
`PERMISSION_DENIED`. Without `root`, explicit general host writes retain their
ability to write outside a workspace and through an existing leaf link.

Cooperating Lithe daemons serialize writes to the same directory with a
five-second advisory-lock deadline. The server rechecks the version after
staging and syncing, immediately before publication. POSIX rename provides no
version-based compare-and-swap: an external writer that ignores this lock can
still change a file between that final check and replacement. Directory handles
also do not sandbox a hostile same-user process that moves an already-open
directory outside the root. These are limits of conflict and path protection,
not guarantees of isolation from the remote account itself. Failed saves reclaim
their temporary files; a crash can leave one behind.

Hosts advertising `features: ["upload", ...]` accept staged uploads without
increasing the frame limit. Upload IDs are server-assigned and owned by the
connection that created them, including in a persistent session. At most four
uploads are staged per server; each file is at most 2 GiB. Chunks contain
1–256 KiB of decoded bytes and must name the exact acknowledged offset.
`sha256` is a 64-character hexadecimal digest; `mode` permits only `0o000`–`0o777`.
Staging uses a private `0600` sibling file. Commit verifies size and digest,
syncs the file and atomically publishes without replacing an existing target.
Failed commits, aborts, disconnects, session shutdown and an absolute five-minute
expiry remove staging files. Clients serialize chunks and commit only after
the last acknowledgement. Interrupted clients may not know whether a completed
commit published; owners must remove their allocated run directory separately.

Upload registry locks do not cover file creation, writes, `fsync`, publication
or deletion. Operations on one upload use a separate nonblocking lock (`BUSY`
for concurrent operations on that upload). Abort, expiry and disconnect mark
an in-flight upload cancelled without waiting for that lock; its worker retains
staging cleanup responsibility. Independent uploads remain usable.

The optional `runDirectories` feature owns complete runtime input snapshots,
not just unpublished upload files. Clients choose a unique `directoryId` before
allocation (1–64 ASCII letters, digits, `_` or `-`), so a cancelled or lost reply
can still be released by identity. The host creates a fresh `0700` directory
below its private `~/.lithe-server/plugin-runs/`; it never adopts an existing
directory or accepts an arbitrary cleanup path. At most 256 allocations belong
to a session. A later release is ordered after a pending allocation even when
its worker has not started. Release is idempotent and affects only that
session's allocation. A removal failure is returned to the waiting release
request, and ownership remains registered so release can be retried. Existing
parent directories must be real, owned by the host user and inaccessible to
group/other users; allocation never changes their permissions.
Clients stop the consuming process before release.

Nonpersistent disconnect and session shutdown remove these directories after
process teardown and worker drain. Persistent disconnect retains them for
reconnection; an abandoned desktop's directories are removed at idle expiry.
This is not crash recovery of a forcibly killed server: such directories remain
until a future process-liveness-aware recovery mechanism or manual cleanup.
No age-based scan deletes other sessions' snapshots. Installed resources and
persistent provider state are never cleanup targets.

`process.exit` and `terminal.exit` report the leader. The server keeps owning
its session until no member is left, so members that outlive the leader can
still be killed and are cleaned up when the session ends; `server.state`
marks such processes with `leaderExited`.

`server.state` lets a client list what still runs before it closes a window,
and lets a later client list what an earlier one left running. `label`
(`kind` ≤ 64 characters, `name` ≤ 256 characters, no control characters)
is what the client shows for the resource; the server stores it unchanged,
never interprets it, and refuses a label that breaks a rule with
`INVALID_REQUEST` before starting anything. `runningMs` counts from the
start on the server's monotonic clock, so it is right after a client
restart and whatever the clients' clocks say. A terminal's `argv` is the
program actually started: the requested `argv`, or the login shell's path.

When a session ends, the server terminates everything it still owns
(`SIGTERM` or `SIGHUP`, then `SIGKILL` after 2 seconds to sessions that still
have members), lets running requests finish for up to 5 seconds, and exits.

`environment.resolve` asks the user's login shell, interactive first, for its
environment and caches it for the session. `process.spawn` with
`environment: login` uses that environment as the base.

## Events

| Event | Params |
| --- | --- |
| `process.output` | `processId`, `stream` (`stdout` / `stderr`), `data` |
| `process.exit` | `processId`, `exitCode`, `signal` — always after that process's output |
| `terminal.output` | `terminalId`, `data` |
| `terminal.exit` | `terminalId`, `exitCode`, `signal` — always after that terminal's output |
| `watch.changed` | `watchId`, `changes` (`path`, `kind`: `created` / `modified` / `deleted`), `overflow` |
| `watch.error` | `watchId`, `code` (`LIMIT_REACHED` / `ROOT_REMOVED`), `message` |
| `walk.entries` | `walkId`, `entries` (`path` relative with `/`, `kind`) |
| `walk.done` | `walkId`, `total`, `truncated`, `cancelled`, `skippedNonUtf8`, `unreadableDirectories` |
| `tunnel.data` | `tunnelId`, `data` |
| `tunnel.closed` | `tunnelId`, `error?` — the last event of the tunnel |
| `listener.accepted` | `listenerId`, `acceptId` — a reverse listener holds a connection for `listener.adopt` |

`watch.changed` batches are coalesced and sorted by path. With `overflow`
changes were lost and the client must rescan the watched tree.
The reference backend queue retains at most 2,048 changes and 1 MiB of raw
path bytes without blocking the native watcher. Pressure clears those changes
and preserves an overflow marker, watch-limit notice and root-removal notice.
Coalesced batches hold at most 5,000 distinct changes and 1 MiB of path bytes
before JSON escaping; a larger batch becomes an empty overflow batch.

`walk.entries` batches hold at most 2,000 entries and 1 MiB of relative-path
bytes before JSON escaping. A single path beyond that budget ends the walk with
`truncated: true`; it is not sent as an oversized event.

## Listeners

The `listener` feature lets a client make the host listen on a TCP port for
a forwarding rule the user added:

- With `relay`, a **host rule**: the host connects every accepted connection
  to that target itself and copies the bytes on the host, so nothing
  crosses the stream. `host` is a loopback address, or `0.0.0.0` or `::`
  when the user confirmed listening on every interface. At most 64
  connections of one listener and 256 of the session are relayed at once;
  further ones wait in the kernel's backlog, or are closed and counted as
  failed when the session is at its limit.
- Without `relay`, a **reverse rule**: `host` must be a loopback address.
  Every accepted connection waits, at most 16 at once per listener, for the
  client, which receives `listener.accepted`, subscribes to a new tunnel and
  calls `listener.adopt`; the connection then behaves as a tunnel the client
  opened. A connection not adopted within 10 seconds is closed and counted
  as failed.

Host names are refused, so the host never resolves what the client sends as
an address to bind. `failedConnections` counts connections that never
reached their target: a relay target that refused, a connection not adopted
in time, or one the host had no thread or tunnel left for. A listener
belongs to the connection that opened it, also in a persistent session, and
closes when that connection ends; clients do not reopen one after a
reconnect on their own, so a host port is open only while the user's rule
is. `listener.close` waits up to one second for the port to be released, so
a rule edited to the same port can bind it again at once.

## Event delivery and persistent sessions

`server.hello` returns `session` with `persistent`, `resumed`,
`nextResourceId`, `lastEventSeq` and, for a persistent session,
`sessionToken`: 128 random bits, hex encoded, fixed for the whole session.
The first client of a persistent session is admitted whatever it presents;
every later `server.hello` must present that token as `sessionToken`. A
missing or different token fails with `INVALID_SESSION_TOKEN`, the server
closes that connection, and the attached client is not disturbed. The socket
already admits only the host account, so the token keeps other programs of
that account, or a second window reusing the key, from taking over the
session and its buffered output. Clients keep it in secret storage and never
log it; the server compares it in constant time.

The session keeps every event until the
client acknowledges it with `server.ack` (clients acknowledge periodically,
for example every 256 events), up to 8 MiB; beyond that the oldest kept
events are dropped. Written to a socket is not received: when a connection
dies, events may still sit in its outbox or in kernel buffers, so they stay
kept until acknowledged.

`server.resume` with `afterSeq` — the last `seq` the client received on an
earlier connection — sends every kept event after it, then live events, so
each event arrives exactly once. A reconnecting client first calls
`server.state` and subscribes to the resources it reattaches to, then
resumes, so their missed events reach those subscriptions. `truncated` means
events after `afterSeq` were dropped; the client re-reads state and rescans
watches.

While no client is attached, a persistent session keeps processes, terminals
and watches running until the idle timeout. A client whose user chose to
leave them running sends `server.keep` before it goes, without
`server.shutdown`; the session then outlasts the idle timeout while any of
its processes or terminals runs, and ends once none does. Each client that
attaches decides again: attaching clears an earlier `server.keep`. So the
programs of a client that crashed or lost its network end with the session
after the idle timeout, unless the user reconnects before it passes. Tunnels, walks and listeners belong to the connection
that opened them and end with it. After `server.shutdown` a persistent session is no
longer reachable, even while it is still terminating its processes; the next
connection with the same key starts a fresh session.

## Host capabilities

`server.hello` returns `host` with `os` (`linux` / `macos`), `arch`
(`x86_64` / `aarch64`), `hostName`, `userName`, `userId`, `homeDirectory`,
`defaultShell`, `pathSeparator`, `windowsInterop`, `process.orphanProtection`
(`parentDeathSignal` / `serverShutdown`), `watch.backend`
(`inotify` / `fsevents`), `watch.directoryLimit` and the sorted `features`
list. Clients branch on these facts, never on `os` alone, and check
`features` before calling optional methods.

`userName` comes from the server's environment when it is set, so it is for
display only. `userId` is the numeric account from the kernel; clients key
per-account state, such as workspace trust, by it. Servers that predate it omit
the field.

`windowsInterop` is `true` when the host can start Windows programs through
WSL interoperability, that is when a `binfmt_misc` entry WSL registers
(`WSLInterop`, or `WSLInterop-late` on some WSL releases with systemd) is
enabled, and `false` when such an entry exists but none is enabled. The field
is omitted when there is no such entry: outside WSL, on macOS, in a container
that does not see it, and from servers that predate it. WSL is not a security
boundary, so clients show the value and never relax trust for it.

## Error codes

`PROTOCOL_MISMATCH`, `INVALID_SESSION_TOKEN`, `INVALID_REQUEST`,
`UNKNOWN_METHOD`, `INVALID_PATH`,
`NOT_FOUND`, `PERMISSION_DENIED`, `ALREADY_EXISTS`, `NOT_A_DIRECTORY`,
`IS_A_DIRECTORY`, `DIRECTORY_NOT_EMPTY`, `CONFLICT`, `TOO_LARGE`,
`UNKNOWN_RESOURCE`, `RESOURCE_ID_IN_USE`, `SPAWN_FAILED`, `CONNECT_FAILED`,
`ADDRESS_IN_USE`, `BUSY`, `UNSUPPORTED`, `IO`. Clients additionally produce `TIMEOUT` and
`DISCONNECTED` locally. Messages are for people; OS error numbers stay in
`details`. `BUSY` always means a capacity limit that may clear: retrying
later is safe because the failed request changed nothing.

## Compatibility

Incompatible changes increment the protocol version, which `server.hello`
rejects with `PROTOCOL_MISMATCH`. Additive fields and new methods keep the
version and are announced through `features`. The daemon's socket name
includes the server version, so a client upgrade starts a new daemon rather
than reaching an older one.

## Server identity

`--target-identity --state-dir <dir>` prints 64 hexadecimal characters derived
from a random marker, the current numeric user ID and the canonical state root.
The marker is `target-identity-v1` in the private state directory, created as
an owner-only regular file without following symbolic links and published
without replacing another launcher's marker. Later launches validate ownership,
permissions and length before reading it. This is persistent account-local
state, not a secret or hardware identity: copying the marker to a clone with
the same account and path also copies that identity.

The reference bootstrap prints `LITHE-TARGET <identity>` after installation
and waits for a session key on the same authenticated stream before executing
`--connect`. The client combines this marker with the actual OpenSSH identity
and explicit route before selecting any previously stored key or token. The
marker does not replace SSH host-key authentication.

`lithe-remote-server --identity` prints one JSON line with `serverVersion`,
`protocolVersion` and `sha256` of the executable itself. A binary is never
trusted to report its own hash: bootstrappers reuse an installed server only
when the host's own `sha256sum` or `shasum` computes the hash of the artifact
they ship, and only from install directories that are not symbolic links,
belong to the remote user and are not writable by the group or other users.
On a host without a hash tool, a fresh upload is accepted when its size
matches and `--identity` reports the expected hash, and it is uploaded again
on every connection instead of being reused.

The expected hash is never computed from the file about to be sent: the
application ships it in a manifest signed with the builds, and the
bootstrapper reads the build once, compares it with that hash and sends the
same bytes. A build that cannot be read or differs is refused before anything
reaches the host (`remote_server_invalid` from `remote.connect`).
