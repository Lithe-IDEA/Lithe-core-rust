# Application Boundary Contract

The application boundary describes product behavior that a SwiftUI/AppKit or
React/Tauri Windows UI can consume. It does not describe widgets, threads, processes,
or operating-system APIs. It defines the cross-platform contract; current
product scope and setup are documented in [`README.md`](https://github.com/1lck/Lithe-IDEA/blob/preview/README.md); the
verification scripts are the executable source of boundary checks.

## Data Rules

- All payloads are UTF-8 JSON when exchanged across a process or language boundary.
- Workspace paths are relative to the opened workspace and use `/` separators.
- Absolute paths may appear at native editor/process boundaries and as LSP/ACP
  `file://` resource URIs, but are not persisted as cross-platform identifiers.
- Product-facing line numbers are one-based. Editor/LSP positions explicitly use
  zero-based lines and UTF-16 columns. Missing locations are `null`.
- Lists have deterministic ordering so contract fixtures can be compared directly.
- Every asynchronous operation exposes `idle`, `loading`, `ready`, and `failed` outcomes.
- Failures contain a stable `code` and user-facing `message`; platform details belong in `details`.

## Shared built-in translations

[Shared i18n](https://github.com/1lck/Lithe-IDEA/blob/preview/shared/i18n/README.md) owns migrated semantic keys, typed named parameters,
locale registration, platform scopes and documented platform wording overrides.
Build-time generation supplies native macOS string tables and the Windows catalog;
no runtime JSON loading, package download or write to installed resources occurs.
The locale registry preserves each platform's persisted identifiers and default
language. Explicit fallback chains resolve shared entries during generation;
all existing platform catalogs and presentation-owned bilingual copy use this source. Translation coverage is not a
capability claim. [Format fixtures](https://github.com/1lck/Lithe-IDEA/blob/preview/shared/i18n/fixtures/format-v1.json) bind generation
to native positional formatting, literal percent signs and unmodified argument data.

## Feature Contracts

Action search matches the current interface title, description and group as well
as the original action metadata. Localized host copy and literal plugin titles
remain distinct; changing the interface language takes effect on the next query.
Hidden tool-window rail entries remain recoverable by their available command.
Core's stateless `search.actions` returns ordered candidate offsets; the host keeps
ownership of action availability, execution and module activation. See
[the action search fixture](../fixtures/search/actions-v1.json).

Shared Monaco highlighting is owned by the current view/model pair. A queued
callback for a closed or replaced model must finish without running against the
new model or locking surviving documents. Primary, split and diff views share
this lifecycle protection; actual symbol computation remains upstream-owned.

| Feature | Shared input/output | Platform-owned implementation |
| --- | --- | --- |
| Workspace | visible snapshot, relative paths, file metadata, deterministic ordering | workspace root selection, native dialogs, and watchers |
| Documents | relative-path validation, decoded text, selected encoding, dirty/save state, and conflict outcome | native byte conversion, atomic write, and external-change notifications |
| Search | query matching, deterministic result ordering, symbols, and replacement preview | workspace lifecycle and optional index persistence |
| Git | workspace commit plans with optional immutable ChangeList path scopes, dependency ordering, guarded steps and partial-success retry, changes, commits, branches, diffs, reviewed history actions and recovery, worktree listing and safe management, worktree-aware PR publication context, validation, and mutation results | Git executable discovery, credentials, process environment, opening checkout paths, local ChangeList selection and preference persistence |
| GitHub | remote parsing, trusted request plans, normalized branch comparisons and pull requests/reviews/comments, deterministic ordering, and stable errors | OAuth configuration, HTTPS, browser opening, and operating-system credential storage |
| [AI commit messages](ai-commit.md) | provider configuration parsing, commit rules, bounded diff evidence, request plans, and response text | local configuration discovery, credentials, HTTP, cancellation, and draft UI |
| [IDE capability API / MCP](ide-api/v1.md) | canonical tool schemas, narrowing host selection, permission/argument validation, stable execution IDs, bounded output cursors and errors | implemented workflow inventory, explicit project authorization, live environment/Build/Run application actions, helper discovery and writable connection storage |
| Runtime | tool requirements, normalized candidates, and effective toolchain references | tool probing and executable paths |
| Language tooling | provider catalog, local fallback results, complete LSP process/session runtime, capabilities, diagnostics, UTF-16 edits, and normalized feature results | executable/environment discovery and UI provider routing |
| Run/Debug | versioned configuration documents, three-layer resolution, diagnostics, platform-neutral launch plans, DAP framing/state, reverse terminal requests, breakpoint relocation, stepping filters, threads, stacks, variables, and events | project and preference persistence, native edit reporting, adapter discovery, PTY/ConPTY debuggee launch, child processes, sockets, native termination, and UI |
| Terminal | input bytes, output bytes, lifecycle; [shell selection semantics](terminal-profiles.md) | PTY/ConPTY, shell discovery, native profile preferences, and environment |
| Workbench background | versioned source (`none`, bundled slot `01`–`10`, or `custom`) and opacity | UI, image rendering, bundled-resource packaging, local-image access permission and persistence |
| Local History | revision metadata, text content, restore result | persistence location and file operations |
| Application logging | fixed module categories, versioned event context, recursive privacy rules, bounded timeline filters and frozen diagnostic preview manifests | background queues, native file rotation/retention, effective directory fallback, crash capture, session selection, viewing and user-selected export destinations |
| Modules | stable IDs, manifests, enabled state, lifecycle snapshots, dependencies, capabilities, and contributions | native factories, processes, timers, PTY/ConPTY, watchers, connections, and UI rendering |
| Community integrations | Discourse authorization sessions, RSA-OAEP callback verification, user API protocol models, and normalized community data | opening the system browser, receiving URL callbacks, and credential-vault persistence |
| Updates | normalized release metadata, state names, validated `stable|autobuild` preference, explicit rolling consent, and stable error codes | update feeds, package verification, download, installation, restart, and native UI |

### Application update channels

A missing saved channel selects `stable` for Stable and legacy shared-identity builds;
separately installed `Lithe-Dev` defaults to `autobuild`. Invalid preferences select
Stable, and an explicit saved choice takes precedence. Choosing `autobuild` from Stable requires explicit
acknowledgement that rolling builds may contain unfinished features and serious bugs;
both settings UIs continuously recommend Stable for ordinary users. The AutoBuild
release is `https://github.com/1lck/Lithe-IDEA/releases/tag/AutoBuild` and is built
from one pinned `preview` commit per nightly run.

Installed identity and selected channel are separate. A cross-channel switch requires
an explicit full installation; stable semantic versions must never be compared to
rolling workflow build numbers. Channel changes cannot race active checks, downloads,
installation, or an outstanding switch. Existing offers must be dismissed before the
new preference takes effect. Legacy macOS builds with the Stable bundle ID keep their
signed full Stable rollback path. Dedicated development builds open Stable downloads
for a separate installation; the installer must not accept another bundle identity.

AutoBuild installs as `Lithe-Dev`, with macOS bundle ID `app.lithe.desktop.dev` and
Windows Tauri identifier `app.lithe.windows.dev`. Stable retains its existing names,
identifiers, preferences domains, Keychain services and historical storage paths.
Preferences, credentials, default logs and app-owned runtime data/cache/temp roots
are isolated by installed identity, never by a mutable channel preference. The
macOS adapter inserts the development bundle ID below system writable roots;
Windows uses Tauri's identifier-scoped app directories and credential service.
Explicit user-selected project/tool/log paths can still be shared. On the first
Dev launch, if no Dev preferences or durable state exist, copy Stable settings and
workspace records into independent Dev storage and persist a versioned completion
marker. Existing Dev data is never merged or overwritten; subsequent launches do
not fall back to Stable. Changing the channel alone does not switch profiles.
Updater/channel suppression state, custom log destinations, browser caches,
indexes, locks and runtime connections are excluded. macOS also copies its local
AI secret file and marketplace source configuration, along with
persisted database configuration/history/schedules and custom background image access
records. These preference keys also count as existing Dev data. OS-vault
credentials remain separate and may require signing in again. Windows snapshots
only native settings JSON and closed WebView localStorage, before creating
any WebView; it never copies a live browser profile. A busy source or failed copy
stops initialization with a retry message and no success marker. Published files
and preferences resume from a staged snapshot after an interrupted commit. Stable
source roots are checked only when creating a new snapshot; existing Dev data,
completed markers and frozen snapshot recovery must work without access to Stable.
Signed bundles remain read-only.
DMGs/ZIPs contain `Lithe-Dev.app` and Windows installers use the development product
name, while public release asset filenames remain compatible. Pre-isolation
AutoBuild needs a separate full installation; only matching bundle identities and
app names can be Sparkle differential baselines.

Within AutoBuild, one whole-second UTC build time is captured per build and encoded
as `YYYYMMDD.HHMMSS` for Sparkle's numeric version comparison. Its corresponding
ISO timestamp is displayed as `AutoBuild — Built at YYYY-MM-DD HH:mm:ss UTC`
(localized as `AutoBuild — 构建自 … UTC`). Equal or older build times are rejected;
workflow run/attempt counters do not determine version identity. The fixed tag stays
`AutoBuild`; its release title and notes show the refreshed build time. Commit SHA
identifies source and SHA-256 checks bytes, neither orders releases. Date formatting
is checked on both clients against
[`build-time-v1.json`](../fixtures/updates/build-time-v1.json).
Windows AutoBuild currently uses an acknowledged browser installer route because
its rolling publisher has no signed Tauri manifest or monotonic updater version.
That route has no target update metadata, download/install progress, or claim that
the installed app is current; scheduled checks do not open a browser or dialog.
Normal Stable-to-Stable checks retain each platform's existing signed updater.
The shared scenarios live in
[`channel-selection-v1.json`](../fixtures/updates/channel-selection-v1.json).

### Document encoding

Local text documents expose the selected encoding in the editor status bar. The
supported labels are `UTF-8`, `UTF-8 with BOM`, `UTF-16 LE with BOM`,
`UTF-16 BE with BOM`, `UTF-16 LE`, `UTF-16 BE`, `GBK`, `GB18030`, `Big5`,
`Shift JIS`, `EUC-JP`, `EUC-KR`, `Windows-1250`, `Windows-1251`,
`Windows-1252`, and `KOI8-R`. Native adapters auto-detect UTF-8/BOM, UTF-16 by
its BOM, and the GBK family when opening a file; every other encoding is
selected explicitly because its bytes cannot be told apart reliably. An
explicit reopen request decodes with the selected codec
and exposes replacement characters for invalid byte sequences. “Save
with Encoding” converts the current Unicode buffer only after the target codec
accepts every character. Both products preserve the raw-byte identity of the
last acknowledged disk snapshot and reject a save when another process changed
those bytes.

### macOS registered workbench tool windows

The macOS host resolves tool-window actions and content from module contributions.
Project (`workspace.project`), Changes (`git.changes`) and Search (`search.workspace`)
use the same registry as right/bottom tools and declarative plugin windows. Left,
right and bottom each have primary and secondary dock groups, with at most one
visible window per group. Menu moves and pointer drops update the same dock state. Moving or hiding a tool window
changes presentation only; module ownership and resource lifetime stay in ModuleRuntime.
A title-bar minimize action closes its own contribution ID in its current region.

Workspace layout stores entry order, visibility and placement by contribution ID.
Left content selection and expansion have optional fields for compatibility with
older geometry-only layouts. Legacy workspace session names (`project`, `changes`,
`search`, `pullRequests`) map to stable IDs; other registered IDs survive as strings.
Legacy docking enum names also map to contribution IDs without losing group or
order. Geometry writes merge existing UI preferences. Module release and shutdown do not
replace saved user visibility with teardown state. Restoring a declarative window
activates its owner even if its left slot is already selected, without toggling it closed.
These are macOS host behaviors; Windows adoption remains independent.

Workbench actions share host-owned visibility, enablement, checked and pending state.
Declarative commands cannot start a duplicate window activation while pending; the
window entry can cancel its own display request. Invocation revalidates the complete
current contribution and state, including old menu/palette callbacks. Built-in command
enablement reuses the existing shortcut policy. One module's concurrent activation
requests share construction; closing/disable cancels and joins that construction before
resources are released. A cancelled UI waiter cannot cancel another window's waiter.

### External HTML preview

Both products open an existing local `.html` / `.htm` document in the system's
configured web browser, independently of the file type's default editor. The
browser reads the saved file; this action does not save buffers or start a web
server. File URLs preserve Unicode and reserved characters. Browser discovery
and launch belong to platform adapters, and launch failures are shown to the user.

## Module Lifecycle Contract

The macOS reference product implements the built-in manifest in
`rust/fixtures/modules/built-in-v1.json`. Module IDs and manifest fields are
platform-neutral compatibility surfaces. A future Windows implementation may
adopt the contract independently without sharing Swift implementation code or
being coupled to the macOS migration schedule. An implementation of this
contract must preserve these invariants:

- A disabled module is not instantiated and owns no task, timer, watcher,
  session, connection, or child process.
- An on-demand module is instantiated only after its capability is requested.
- Sleeping stops every owned resource and releases the module instance.
- Active non-interruptible work holds a lease that blocks sleep with a reason.
- Wake reconstructs the module, activates declared dependencies first, and
  republishes capabilities and contributions.
- Required modules cannot be disabled. A provider cannot be disabled while an
  enabled module depends on it.
- Module state is one of `disabled`, `inactive`, `activating`, `active`, `idle`,
  `preparingToSleep`, `sleeping`, `sleepBlocked`, or `failed`.
- Native plugin manifests, compatibility, ownership, and signatures are
  validated before Bundle loading. A failed optional package is reported to
  plugin management and does not prevent required modules from starting.
- Successfully loaded native plugin module IDs remain durably marked for the
  process lifetime. An unclean exit leaves the mark behind, so the next launch
  quarantines those modules before constructing any plugin Bundle. A clean
  application termination clears the mark.
- Disabling a loaded in-process native plugin stops its module-owned resources
  immediately. Its code remains mapped until restart, and the next launch
  skips the Bundle before invoking its principal class or factories.
- Plugin update, rollback, and uninstall operations that affect mapped code
  are finalized before plugin scanning on the next launch.
- Native plugin factories receive a read-only host context. Host services use
  stable IDs and shared protocols; plugin code cannot import a platform
  composition root or the application executable.
- AI Assistance, Terminal, Git, Search, Local History, and Debug are built-in
  lifecycle modules. They are not marketplace plugins.
- Every language provider, build tool and ecosystem runner is represented by an
  independently configurable language-support plugin; Core names none of them.
  On macOS, Go and PHP are community declarative
  plugins from Lithe Store (`io.github.lithe-idea.go-support`,
  `io.github.lithe-idea.php-support`); the retired official IDs
  `dev.lithe.plugin.go-support` and `dev.lithe.plugin.php-support` keep their
  pinned publisher signature requirement but have no official source. The
  remaining providers share the host's generic language-server module.
- A downloadable language support package may declare language-server,
  execution, testing, and debug module IDs under one language ID. All referenced
  modules must be owned by the same package. Execution and testing may share a
  module when they share one toolchain lifecycle; language-server and debug
  lifecycles remain independently addressable.
- Plugin-owned Run and Test operations may reuse deterministic shared launch
  plans, but the actual child process must use a session owned by the plugin
  module. A disabled plugin language must not fall back to a built-in process
  provider.
- Process-backed language ownership comes from verified installed manifests,
  including packages that are disabled, quarantined, or failed to load. Those
  states make the capability unavailable; they never restore a host process
  fallback.
- Extension execution shutdown completes only after its operating-system
  process exits. A bounded force-stop failure remains visible as an active
  module resource. Active Run and Test sessions hold leases; successful LSP
  document synchronization refreshes the owning module's idle timer.
- Language package manifests include inert file-extension, file-name, and
  project-file recognition metadata. The host may use this metadata to suggest
  an uninstalled plugin, but it must not load the Bundle or probe a toolchain
  during recognition.

Platform products do not share module-runtime implementation code. The stable
manifest, lifecycle semantics, and deterministic JSON representation are the
portable boundary.

## Document Lifecycle Contract

An open document, rather than an editor widget, owns live text for the lifetime
of its tab. Recreating Monaco, `NSTextView`, syntax services, or an LSP binding
must reattach to that document and must not read an older memoized snapshot.

Persistence state is one of `clean`, `dirty`, `saving`, or `conflict`.
`saving` retains `operationId` and the immutable revision being written. A save
completion only clears dirty state when it still owns that operation and no
newer revision exists. A watcher may reload a clean document, but an external
change to a dirty document enters `conflict` and preserves live text until the
user chooses Keep Editor or Load Disk Version. Notifications during an in-flight
save are reconciled after its result. A missing file uses `diskConflict`, including
when the buffer was clean; it never silently clears the buffer or recreates the file.
Platforms own text and native I/O; Rust Core owns the deterministic decisions.

Local UTF-8 document saves carry the exact last-observed disk content (or an
explicitly acknowledged missing-file state) into a native guarded write. The
adapter compares bytes, stages a sibling file, checks again, and replaces the
target. A conflict does not write. Keep Editor acknowledges only the observed
version; another external edit must conflict again. Unsupported targets and I/O
errors fail closed. This narrows, but cannot eliminate, the race between the last
comparison and a write by an unrelated process.

Open document owners lease parent-directory observations independently of project
indexing. Windows routes events by window, document ID, and registration generation;
focus renews registrations and reconciles disk content. macOS provides document
observation through the native workspace file port. Closed or relocated documents
reject stale reads. The Windows frontend bounds concurrent reconciliation to four
per window; macOS runs native document I/O on a serial worker queue. Native local
filesystem calls do not promise a hard cancellation deadline. Remote/WSL and virtual
documents retain their separate adapters.

Windows native document failures expose a stable `code`, actionable `message`, and
platform `details`: `DOCUMENT_PERMISSION_DENIED`, `DOCUMENT_UNSUPPORTED`,
`DOCUMENT_INVALID_TEXT`, `DOCUMENT_IO_FAILED`, or `DOCUMENT_WORKER_FAILED`.
Watcher setup failures are logged and retried on focus; persistence remains guarded.

Workspace visibility and project detection exclude nested checkout containers
named `.worktree` or `.worktrees` by default, so a copied project is not treated
as a second set of sources or runnable services.

Process-backed features use the shared request fields `operationID` and
optional `timeoutMilliseconds`. Adapters emit lifecycle states `starting`,
`running`, `stopping`, `finished`, and `failed`; `operationID` lets the UI
ignore stale termination events after a restart. `stop()` is the cancellation
operation and must terminate the platform process without changing feature
state owned by another operation. The macOS native and language-extension lifecycle
adapters may additionally report an optional `processID` on `running`: this is the
exact launched root, never a process selected by name. Consumers must validate
`operationID` before retaining it and invalidate it on stop, completion, reset, or
replacement. Hosts without a local PID leave it absent. Test performance uses
native CPU/RSS of that root and its descendants (including forked workers), with
RSS explicitly labeled as resident memory rather than a runtime's managed heap.

Editor changes use versioned incremental document synchronization and do not
restart the language-server session or clear its index.

Language feature clients route through a provider interface rather than
depending directly on an LSP session. Process-free providers remain available
when an executable is missing. LSP-backed features are enabled only after the
server advertises them during initialize or dynamic registration. The shared
core owns JSON-RPC state, stdio, process lifecycle, and normalized results;
platform adapters own executable and provider-resource discovery. Detailed
invariants are documented in
[`.agents/notes/implemented/architecture/2026-09-13-language-tooling-and-lsp-runtime-ownership.md`](https://github.com/1lck/Lithe-IDEA/blob/preview/.agents/notes/implemented/architecture/2026-09-13-language-tooling-and-lsp-runtime-ownership.md).

Session lifecycle is a single discriminated state, never a set of booleans.
Capability negotiation is separately `unknown` or `known`; a known capability
is then supported or unsupported. Failures retain stable `code`, `stage`, exit
code, and diagnostic detail across the Rust, Swift, and TypeScript boundaries.
Domain and adapter layers return stable reasons rather than user-facing prose;
each product's presentation layer owns localized notification text.

Debugger stepping policy is portable. Rust Core owns adapter defaults,
normalization, validation, adapter launch projection, and the `isFiltered`
classification on normalized stack frames. Platform products own preference
persistence and decide whether matching consecutive frames are collapsed or
expanded in their native call-stack UI. No Debug session, adapter process, or
background task is created merely because stepping preferences exist.

Exception pause metadata is portable when the adapter advertises the standard
exception-information request. Rust Core normalizes the exception type,
description, break mode, stack trace, evaluation name, and nested details;
native products decide how that data is presented beside the current frame's
ordinary scopes and variables. An adapter that supplies no object reference
does not make the exception itself expandable through this contract.

Debugger variable paging is portable. Rust Core owns the standard DAP
`filter`, zero-based `start`, and positive `count` request projection and
normalizes adapter-reported `namedVariables` and `indexedVariables` counts to
non-negative values. Native products own tree expansion and page-size policy;
the macOS reference product loads at most 100 children per request, appends
named children before indexed children, exposes an in-tree load-more action,
and discards stale pages after the selected frame changes. A native client must
also stop offering more pages when an adapter returns more children than were
requested or repeats an already loaded page.

Debugger terminal launch ownership is split at the native boundary. Rust Core
advertises terminal support, validates and normalizes DAP `runInTerminal`
reverse requests, correlates the platform response, and rejects stale or
duplicate completions. The platform Terminal module owns PTY/ConPTY creation,
direct executable-and-argument startup, environment application, process IDs,
terminal presentation, and native termination. A Debug session is still lazy:
neither a terminal nor a debuggee process exists until an adapter requests one.

Debugger disconnect ownership is portable. A session started with `launch`
owns its local debuggee and sends `terminateDebuggee: true` when stopping. A
session started with `attach` does not own the remote process and sends
`terminateDebuggee: false`; closing the native transport must therefore detach
without killing the remote process. A session stopped before launch or attach
also uses the non-terminating policy.

### Explicit debug adapter selection

`DebugLaunchConfiguration.adapterProviderID` is optional host routing metadata.
Plugin registrations use the existing Registry identity `pluginID/adapterID`.
The field is not a DAP argument, a process
command, or a plugin-private function call. An omitted identity uses the existing
file selection. An explicit identity, including an empty or withdrawn one, must
never fall back to another adapter matching the file.

Qualified registrations include every declared adapter, including attach-only
and executable/process adapters. They do not become automatic file defaults.
Before creating a transport, the session manager resolves the current registration
and verifies its declared launch/attach request. An explicit target need not be
an open source file. Retry and configuration transformations retain the identity;
failed additional starts clean up their own session and preserve the previous one.

Source breakpoints are shared by matching registered adapters, including replay
on a later launch. Function, exception and data breakpoint changes use the selected
adapter identity, even when the target has no source extension. Plugin ownership
continues to control withdrawal and process cleanup. The platform resolver still
owns manifest validation, permissions, executable resolution and supported transports;
this routing field grants no new permission and carries no adapter process or socket.

The host fixture is [adapter-selection-v1.json](../fixtures/debug/adapter-selection-v1.json).
macOS consumes this contract. Windows has no plugin adapter launch consumer yet.
Build Provider version 5 adds a separate native debug preparation request;
version 4 run plans are never reinterpreted as debug plans. The selected run
names a same-plugin launch adapter. Core validates an existing instance-contained
program plus bounded arguments/environment/directory, and the platform rechecks
executable permissions before binding target facts through `debug.adapters.startArguments`.
Build-tree Debug and saved-provider Run Debug save documents first and revalidate
workspace, source, configuration, model and plugin identity before handoff. Debug
owns the resulting session; stopping preparation or accepting late replies cannot
start a session after invalidation. Cross-plugin preparation is not covered by
this native increment.

## Error Codes

Use stable categories rather than platform error strings:

- `invalid_request`
- `workspace_not_found`
- `permission_denied`
- `not_supported`
- `runtime_missing`
- `process_start_failed`
- `process_failed`
- `parse_failed`
- `cancelled`
- `timed_out`
- `unknown`

GitHub authorization is independent of a Lithe account. Device Flow is the
preferred path, and its token is stored only in the platform credential store.
The application boundary never exposes that token to a view or persistence
fixture. See [`github.md`](github.md).

## UI Boundary

The UI sends commands to an application feature model and renders state from
that model. It must not construct `Process`, file watchers, terminals, runtime
locators, Git command runners, or persistence stores. Platform-specific actions
such as directory picking, file-browser reveal, clipboard access, and native
shortcut monitoring are capability ports, not application logic.

## Workbench Background Contract

[`workbench-background-v1.schema.json`](workbench-background-v1.schema.json)
defines the portable background preference. It stores only a stable bundled
slot ID, `custom`, or `none`, plus opacity. A bundled slot is the same product
identifier on every platform; each product packages and renders its own copy of
that slot's image. `custom` deliberately contains no path, bookmark, token, or
image bytes. Native file access authorization and local-image metadata are
platform-private, so a preference can be understood on another platform
without leaking an unusable absolute path or macOS security-scoped bookmark.

Search and Git examples are kept in `rust/fixtures/`. New behavior should
add a fixture before adding a second platform implementation.

Git history clients load reference metadata independently from bounded commit
pages. The first page may load concurrently with references, but later pages
append through Core's opaque `nextCursor` while continuing the same bounded Git
log stream. Changing repositories or references and closing the history view
cancels the owning `operationID` and closes any retained cursor; a late result
cannot replace the active selection and its returned cursor is also closed.

Launch environment precedence and automatic run scanning are Rust Core policy.
Platform adapters discover executable/home paths and provide the native parent
environment; `execution.resolveEnvironment` applies inherited < explicit values.
The selected toolchain installation of each step (a runtime home, a build
tool's own home) is applied by the host from `toolchains.*` before that merge,
so Core names no tool and a plugin decides which installation a step uses.
Automatic scan exclusions,
partial-snapshot retention and diagnostics share one implementation; unreadable
optional subtrees must not erase previously discovered configurations.

Ordinary workspace events use Core's `workspace.filterChanges` policy before a
host applies event-count recovery thresholds. Native lost-event/error flags always
request recovery. Hosts retain native watch registration, Git/document channels,
explicit watches and platform-specific lifecycle; the shared filter must not
replace those channels or swallow real source changes mixed with build output.

Run configuration behavior is exposed through the `runConfig.*` commands.
Platform clients coordinate inspection, generation, resolution, typed document
edits, and launch planning, but must not implement a second JSON merger,
toolchain matcher, ID generator, or argument parser. Ecosystem-specific
arguments are built by the plugin that declared the run. Opening a project inspects existing files without writing; generation
is an explicit user action. Shared project overrides stay in
`.lithe/run/configurations.json`. Machine-local overrides may live in
`.lithe/run/local.json` or in a host-owned document supplied as
`localDocument`; absolute toolchain paths belong only in that local layer and
are excluded from project visibility and Git by default. Generic runtime
executables such as Node are selected in `.lithe/toolchains/local.json`; this
machine-local document is also excluded from Git by default. Missing or
incompatible toolchains block only configurations that consume the affected
toolchain, while diagnostics without a configuration ID apply to the project.
Runtime consumption is declared by the detector from the actual command rather
than inferred from the provider namespace. An automatically discovered runtime
path is session-effective: validation and launch share it, but persistence
still requires an explicit user selection.

On Windows, Debug cleanup owns a Run execution ID in addition to its reusable
output slot. The host checks that ID atomically when stopping the process so a
late adapter shutdown cannot terminate a replacement Run in the same slot.

## SVG document preview

SVG extensions are matched case-insensitively and open as editable text in both
workspace and standalone file flows. The default presentation is editor plus
preview, with editor-only and preview-only modes available. All modes use the
same document buffer and preserve normal dirty, save, undo, and read-only rules.
Preview rendering uses the current unsaved source. Malformed source shows a
rendering failure while the editor remains accessible; correcting the source
restores the preview. SVG is rendered as image data, never inserted into the
application DOM as executable markup. Rendering and resizable layout are owned
by the platform. The behavior fixture is
[`svg-preview-v1.json`](../fixtures/editor/svg-preview-v1.json).

### Optional PHP support

PHP plugin installation is explicit. The base application ships neither a PHP
package nor Node/Bun/Intelephense. macOS installs the Lithe Store community package
`io.github.lithe-idea.php-support` through plugin management; the package does not
bundle Intelephense, which the user installs, and its tools are user-owned and
never deleted by Lithe.
Windows installs language tools only on an explicit install/repair action; its
managed tool cache is removed on uninstall without touching global tools. PHP
run/test discovery and launch require enabled support. Disable cancels installation,
stops in-flight and active owned processes, and unregisters providers; closing a
workspace stops that workspace's PHP Run sessions. Shared lexical PHP recognition
may remain available without spawning processes or downloading dependencies.

Windows optional language implementations are single-module worker packages
(`lithe-worker-plugin`, format version 1). PHP configuration and Composer/PHPUnit
plan generation live in `Plugins/win/Official/PhpSupport`; application builds must
not import that implementation, even through a dynamic import. The inert optional
language catalog may identify package ownership without carrying executable code.

A user imports a `.lithe-extension` file through extension management. The package
contains a validated manifest and ESM source (maximum 256 KiB), stored atomically in
the WebView user profile under `lithe.worker-package:<id>`. Import installs language
tools but leaves the plugin disabled; enabling starts the existing worker host.
Incomplete installation is not restored on restart. Uninstall removes the source,
parser cache and owned tools; the application installation directory stays read-only.
Local packages are user-selected code, not authenticated official downloads.

The v1 language package accepts `languages`, `lsp`, and `runActions` declarations;
other host permissions and contributions are not granted. `lsp.requiredExecutables`
is passed to the native tool adapter for PATH validation. `runActions.manifestFiles`
contains at most 16 root-level file names; `executables` names allowed PATH commands.
The worker's `api.runActions.register` receives file contents and returns bounded
plans (`id`, `name`, `sourceLabel`, optional `description`, `executable`, `arguments`).
Only a user click launches a validated plan through the host Run service after saving
the workspace. Workers never own the native process handles; the host tracks both
extension ID and workspace ID, stops pending and active runs on disable/close, and
rejects stale discovery results. Remote/WSL projects do not use these local plans.

## Text content and language selection

Native adapters decode document bytes using the existing encoding catalog.
Decoded content is classified by Core's `document.classifyText` policy, also
available through its borrowed UTF-8 C ABI. File extensions and installed
language contributions must not exempt text from control-character validation
or cause Unicode text to be classified as binary. Windows opening and session
restoration share one content loader; read failures remain actionable errors.
Explicit image, database, PDF, and binary-format viewers retain their host routing.

Both Monaco hosts use the bundled INI tokenizer for `.properties`. Tokenization
is presentation only: no grammar or language server is required to open plain
text. Register bundled contributions in `frontend/editor`, not in a separate
platform-specific tokenizer. Text fixtures are shared under
`rust/fixtures/editor/text-content-v1.json`.

Build-provider application preparation and execution have separate owners. The
workspace Build session validates a current target and prepares it; the native
Run host owns the application process, output and stop. Explicit persistence
uses the existing local Run v2 layer and stores a typed selector rather than
launch facts. Reopening or restarting re-resolves that selector in the enabled
workspace Provider and prepares afresh. Save-before-run completes before taking
the preparation snapshot; source/configuration changes revoke pending results.
The public Run operation ID spans preparation and process handoff, so UI and Agent
queries keep one execution identity. Stop targets the owned preparation ID before
launch and the owned native process ID afterward. Missing or stale references fail without a CLI or old domain fallback.
A Provider application's native output remains visible even without generated
Run configurations. See `../fixtures/run-configuration/build-provider.json`.

Windows native preparation returns `resolvedExecutables`, binding directly
launchable definition IDs to this operation's checked native executable paths. Provider Run accepts a
toolchain reference only when that ID is present; it hands the bound path and
selected environment to the native process resolver. Saved legacy runtime paths
and language-import fields cannot replace a missing binding. Conflicting roots
for one ID fail instead of selecting by iteration order. The host-only JSON shape
is exercised by Rust serialization and the TypeScript Run workflow using
`../fixtures/build/native-tool-context.json`; it is not a persisted launch permit.

Automatic Provider run discovery uses the current public declaration/target pairs,
independently of language imports. The host projects generic selectors and upstream
labels; Core owns stable Run IDs, layering and snapshot freshness. Neither layer
interprets framework IDs or reconstructs commands/toolchains from those selectors.

Portable Provider user choices live in Run v2 `extensions.build-run`, separately
from `extensions.build` selector identity. Hosts preserve exact runtime argument
arrays, opaque entrypoints and declared build-option values; the current Provider
model/declaration validates them during preparation. This reuses protocol 10 rather
than adding tool-specific persistence fields. Migrating old domain formats remains
an explicit temporary adapter responsibility, with conflict-protected storage.

Complete persisted Build references retain opaque Run IDs across format migrations;
the prefix does not confer authority. Core deduplicates only the generated counterpart
of the effective saved selector and validates it before use. Prepared team/local Run
layers can be resolved without filesystem publication; native adapters own cancellation,
source checks, metadata-preserving conditional writes, rollback and readback.
