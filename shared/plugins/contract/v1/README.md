# Plugin contract v1: declarative plugins

A declarative plugin describes language support, build and run integration, version control,
debugging, project generation and host-rendered UI as data. The host reads
`plugin.json`, validates it, checks the applicable signature or community approval, and launches the
declared external processes itself. The host never loads plugin code into its
own process, so a package does not depend on the host's binary layout and does
not need to be rebuilt for each Lithe release.

This directory is the cross-platform source of truth:

| Path | Purpose |
| --- | --- |
| `plugin-manifest.schema.json` | JSON shape of `plugin.json` |
| `plugin-ui.schema.json` | JSON shape of what a [plugin view](#plugin-views) function returns: whole views and patches the host draws |
| `plugin-index.schema.json` | JSON shape of a marketplace source index |
| `plugin-recommendations.schema.json` | JSON shape of a marketplace companion recommendations file (`lithe-plugin-recommendations.json`) |
| `plugin-l10n.schema.json` | JSON shape of a plugin's translation file `l10n/<language>.json` |
| `fixtures/` | Valid example manifests and golden cases, one folder per contract area (below). Manifests are named after the capability they exercise rather than a published plugin. Case files name their base manifest by file name, so file names are unique across the folders |
| `toolchains/README.md` | Implemented Core foundation: plugin definitions/provider declarations, registry (a short tool ID defined by several enabled plugins belongs to none of them), reply validation, directory pins, snapshot resolution, version matching and discovery plans; hosts that declare `toolDefinitions` use it for read-only settings pages and for Build and run tools |

| Fixture folder | Contents |
| --- | --- |
| `fixtures/manifest/` | `hello-lithe.plugin.json`, the smallest UI plugin, and `invalid-manifests.json`: edits to a valid manifest and the rejection code every host must report |
| `fixtures/languages/` | Language support: packaged, system-command, and host-integration language servers, `node-runtime` for a runtime launch, and `language-grammar` with `grammars/`; `command-plans.json` holds golden run/test command expansions and language-server launch plans; `language-server-backend-launch.plugin.json` is a language server whose backend resolves the launch and a session policy, and `language-server-launch-resolution.json` holds its golden first-step plans and decoded answers |
| `fixtures/backend/` | Plugin backends (`backend`, `backend-workspace`, `command-backend`, `terminal-backend`); cross-plugin calls (`plugin-functions-exporter`, `plugin-functions-caller`); `function-protocol.json` holds golden plugin-function negotiation, registration, invocation, outcome, progress, and refusal cases, `plugin-logging.json` the golden `litheLogging` negotiation and notifications |
| `fixtures/editor/` | Editor actions served by a backend: `editor-actions.plugin.json`, icons, alternatives and click results in `editor-actions.json`; editor providers in `editor-providers.plugin.json` with registration and result cases in `editor-providers.json` |
| `fixtures/ui/` | `view-panel.plugin.json`: a [plugin view](#plugin-views) in a tool window and in a settings region, with golden `plugins.ui.applyResult` cases in `view-results.json`; `web-panel.plugin.json`: a web panel |
| `fixtures/settings/` | Plugin settings; `l10n/zh-Hans.json` translates `settings.plugin.json`, and `localization-cases.json` holds golden translation choices and rejections |
| `fixtures/build-systems/` | Build systems (`build-*`: static file, command output, instance options, model producer and consumer, tool path, toolchains); `build-plans.json` holds golden build-system plans and decoded outputs |
| `fixtures/run-configuration-types/` | `run-configuration-types.plugin.json`: Go Build and Go Test kinds of run configuration with choice, text, file, directory, arguments and checkbox fields, and `l10n/zh-Hans.json` translating them |
| `fixtures/build-providers/` | Build providers (`build-provider*`) and the golden cases of their configuration, run options, run and debug preparation, execution, and test protocols |
| `fixtures/debug-adapters/` | Debug adapters (standalone, served by a language server, remote attach, and `debug-adapter-features` with declared features and defaults), `debug-adapter-rejections.json`, and `debug-start.json`: what `debug.decodeBackendConfiguration` returns for each `debug/start` case |
| `fixtures/run/` | Run sessions and console links: `tool-reference-launch` names its runtimes by tool-definition ID and declares output link matchers; `run-start.json` holds what `run.decodeStart` returns for each case, including the rejections, and `output-link-matchers.json` the golden matches and resolutions |
| `fixtures/vcs/` | A version control provider |
| `fixtures/keymap-import/` | Two independent shortcut import providers sharing one packaged backend |
| `fixtures/project-generators/` | Project generators: template files (`templates/`), a scaffolding command, and an archive download; `project-generator-tool-field.plugin.json` with `templates/tool-field/` picks tool installations with `tool` fields; `project-plans.json` holds golden `project.*` form evaluations, generation plans, and name checks, and a case's optional `manifest` names the fixture it runs against (`project-generator.plugin.json` by default) |
| `fixtures/toolchains/` | Production-shape fixtures for tool definitions and a packaged toolchain provider; they require `toolDefinitions` and `toolchainProviders`, which a host advertises only when it implements them |
| `fixtures/package-trust/` | `publisher-bundled.plugin.json` and `signature-vectors.json`: public cross-platform vectors for the package publisher signature: the RFC 8032 test key, a fixed package with non-ASCII file names, its canonical payload, RFC 8032 deterministic signature, and content digest. Both platforms must build the same canonical payload and digest and accept the recorded signature; signers that add randomness, such as CryptoKit, check the signature by verification instead of byte comparison |
| `fixtures/marketplace/` | Marketplace index, availability, source URL, workspace recommendation, and install review golden cases |

Validation and template expansion are implemented once in Rust Core
(`rust/crates/plugin-manifest/` and the `rust/crates/plugin-*` crates beside
it, with build systems in `rust/crates/build/src/`) and exposed through the
`plugins.declarative.*` commands in [`rust/contracts/rust-core-api.md`](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/contracts/rust-core-api.md). Hosts must not
reimplement them.

This directory is the only copy of the contract. It lives in `shared/plugins/`
of the Rust Core repository together with the
[developer guide](../../README.md) and the official SDKs, and changes in the
same pull request as the Core code that enforces it. Plugin authors read it
here and never need the Lithe application repository. Each schema's `$id` is
its published URL,
`https://raw.githubusercontent.com/Lithe-IDEA/Lithe-core-rust/main/shared/plugins/contract/v1/<file>`;
`node shared/plugins/scripts/verify-docs.mjs` checks the IDs and the relative
links. `plugin.json` may not carry a `$schema` key, because unknown fields are
rejected; editors associate the schema by file name instead.

## Plugin-owned toolchains (Core foundation)

All tool definitions belong to enabled plugins; the host supplies only generic
query, validation, resolution, persistence, and presentation mechanisms. It
does not add a built-in tool-definition table or interpret version-manager
configuration formats.

The [implemented Core contract](toolchains/README.md) accepts `toolDefinitions: 0`
and `toolchainProviders: 0` or `1` declarations and provides the `toolchains.*` commands
listed there. Tool-definition candidates follow
[Executable resolution](#executable-resolution) and do not admit new sources into
existing Build candidate sets. Definitions are entirely
plugin-owned. Any plugin may define a short ID such as `compiler` or a qualified
`<pluginID>:<id>`; the host supplies no provenance evidence. A short ID defined
by more than one enabled plugin belongs to none of them: the registry omits
those definitions and reports the contest as a conflict.

A host that advertises these capabilities uses them for its read-only toolchain settings pages.
The native adapter scans declarations, probes checked executables, queries plugin
backends and watches their inputs; Core consumes captured facts and owns validation
and resolution. A separate fresh resolution serves Build tools and run plans
whose `toolchain` executable names a plugin-defined tool rather than a host
toolchain; settings detections never grant launch authority. Saving pins,
`toolchainConsumers`, generic runtime launches and live backend activation remain
unsupported, and language-server consumers keep their previous adapters. Version-manager
implementations such as mise belong to their own plugin packages. See
[the implemented slice](toolchains/README.md).

## Shortcut import providers (Core foundation)

`keymapImportProviders` contains one to eight `{id, displayName, recognition, inputs}` entries.
IDs are unique within the plugin, start with a lowercase letter, contain only
lowercase ASCII letters, digits and hyphens, and are at most 64 bytes. The
plugin requires `keymapImportProviders: 0`, `backend: 0`,
`backend.launch`, `keymaps.provide` and `keymaps.readSelectedSources`. Its backend starts from a packaged
executable or packaged runtime without discovered runtime dependencies. The
host starts it for an explicit import; it is not a startup or workspace-open
activation event. The permission permits offering mappings; it grants no file
access on its own. `keymaps.readSelectedSources` authorizes only the provider's
declared inputs under the source directory the user explicitly selected for this
import, and only for the lifetime of that import.

`recognition` lists one to sixteen distinct metadata-only probes. A
`{kind: "file" | "directory", path}` probe matches an entry of that kind
beneath the selected root, using the input path restrictions and refusing
symlinks. A `{kind: "root", base: "userConfig", components}` probe matches
the selected directory's exact path beneath the platform's user configuration
directory (macOS Application Support, Windows roaming AppData). Components
contain one to eight safe literal names and may end in one `*`, matching
exactly one directory component; the first component must be literal. The
source matches any probe. Recognition never authorizes reading file contents
or discloses the absolute path to the provider. A host may use probes without
launching backends, and must recheck them on import.

`inputs` lists one to sixteen unique selectors. `{kind: "file", path}` selects
one relative file; `{kind: "directory", path, extension}` selects immediate
children of that directory with the exact lowercase alphanumeric extension,
never recursive descendants. Paths have at most 256 characters, contain no
empty, `.` or `..` segments, backslashes, colons, wildcards, control characters,
or segments ending in a dot or space. Directory selectors have at most 64
matches after scanning at most 4096 directory entries. The native reader must reject symlinks and paths escaping the
selected root, bound each file to 1 MiB and the snapshot to 8 MiB, and expose
only scoped opaque handles in chunks of at most 128 KiB. A manifest cannot
authorize arbitrary paths or access an unselected source.

Core offers `experimental.litheKeymapImport: {versions: [0], providers}` only
to a plugin-owned session with declared providers. The backend selects a
nonempty duplicate-free subset as `{version: 0, providers}`. A request to an
unselected provider is refused. `lithe/keymap/import` carries only
`{providerID, snapshotID}`. The snapshot is an opaque host-owned reference;
the provider never interprets it as a path. Core bounds the operation and
validates normalized results against the host's action catalog. Providers own
source formats and must not silently promote conditional or multi-stroke rules
to global shortcuts. The exact Core commands, limits and fixtures are in the
[Rust Core API](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/contracts/rust-core-api.md#normalized-shortcut-import-validation).

When granted individually, methods `lithe/host/keymapImport/listInputs`
and `lithe/host/keymapImport/readChunk` take `snapshotID` and `providerID`.
Listing returns `{files: [{id, path, byteCount}]}`; a read additionally takes
`fileID`, a nonnegative `offset` and `maximumBytes` from 1 through 131072 and
returns `{data}` containing base64 bytes. Paths are relative to the selected
root and file IDs are opaque. The host binds snapshots to their plugin, session
and provider, revokes them when the import or session ends, and rejects files
replaced or changed after capture. A new snapshot for that provider revokes the
previous one. Errors never disclose absolute paths.

A host that advertises `keymapImportProviders` 0 implements the scoped reader.
A host advertises the capability only with the complete import workflow. The official Node,
Java, Rust and Go SDKs implement `litheKeymapImport` 0.

## Package layout

```text
<plugin-id>/
├── plugin.json                    # this contract
├── lithe-plugin-signature.json    # publisher signature over every other file
├── l10n/<language>.json           # optional translations (see Translations)
└── …                              # resources referenced by plugin.json
```

Every path in `plugin.json` is relative to the package root, uses `/`, and may
not contain `..`, a leading `/`, a backslash, or NUL. Packages may not contain
symbolic links. A host treats an installed package as read-only.

## Manifest

| Field | Meaning |
| --- | --- |
| `schemaVersion` | Always `0`. Any other value is refused with `plugin.schema.unsupported`. |
| `id` | Stable plugin ID: lowercase letters, digits, `.` and `-`. |
| `displayName` | Name shown in plugin management. |
| `version` | Plugin version, `major.minor.patch`. Independent of the Lithe version. |
| `pluginAPIVersion` | Always `0`. Any other value is refused with `plugin.api.unsupported` and `field: "pluginAPIVersion"`. Lithe's own version is never used for compatibility. See [Versioning](#versioning). |
| `requires` | Required: every capability the manifest uses, each with version `0`; see [Capabilities](#capabilities). |
| `requiresTools` | Every tool-definition ID the manifest's launches and runtime dependencies name; see [Tool references](#tool-references). |
| `vendor` | `id`, `displayName`, and `signatureRequirement` (see Trust). |
| `repository` | Optional `https://github.com/<owner>/<name>` source repository, at most one trailing `/`; owner and name use ASCII letters, digits, `.`, `_` and `-` and are neither `.` nor `..`. Any other value is refused with `field: "repository"`. Run configuration documents under `.lithe/run/plugin/` record it with the ID, and a plugin installed from another repository does not satisfy them. |
| `permissions` | Capabilities the plugin may ask the host to perform. |
| `activationEvents` | When the host may start each provider. |
| `languages` | Languages the plugin recognizes. |
| `languageServers` | At most one language server per declared language. |
| `executionProviders` | At most one run/test provider per declared language. |
| `settings` | User-adjustable plugin options. |
| `ui` | Host-rendered tool windows and commands. |
| `buildSystems` | Project-level build tools. |
| `commandInstallers` | How the host installs a bare command a `systemCommand` launch needs; requires `commandInstallers: 0` and `commands.install`; see [Command installers](#command-installers). |
| `managedRuntimes` | Pinned dependencies qualified by a packaged backend and installed in host-owned storage independent of plugin version; requires `managedRuntimes: 0` and `runtimes.install`; see [Managed runtimes](#managed-runtimes). |
| `toolDefinitions` | Plugin-owned tool policy ([Core foundation](toolchains/README.md)); requires `toolDefinitions: 0`, or `1` when a definition sets the settings-list `category` or `audience`; a host may offer read-only discovery and resolve Build and run tools. |
| `toolchainProviders` | Backend fact-provider declarations ([Core foundation](toolchains/README.md)); requires `toolchainProviders: 0` or `1`, `backend: 0` and `toolchains.provide`. Version 1 uses distinct permission and authority error numbers. |
| `keymapImportProviders` | Independent shortcut import providers; requires `keymapImportProviders: 0`, `backend: 0` and `keymaps.provide`. A host provides access to explicitly selected sources. |
| `pluginFunctions` | [Functions the plugin exports to, and calls from, other plugins](#cross-plugin-calls); requires `pluginFunctions: 0` and `backend: 0`, and grants no permission. A host that advertises this capability relays the calls. |

Unknown fields are refused (`plugin.manifest.invalid`) so a host rejects a
package instead of ignoring part of it. A new optional field in an existing
section needs no new version; a change to what a section means gives the one
capability that owns it version `1`, and only plugins that use that change
declare it (see [Versioning](#versioning)).

### Languages

`id` is lowercase (`[a-z0-9+_-]`). `fileExtensions`, `fileNames`, and
`projectFileNames` are matched case-insensitively; at least one must be
present. A leading dot is optional only in `fileExtensions` (`.php` equals
`php`). In complete file names, project markers, and management file names it
is part of the name, so `.toolrc` and `.env.example` stay hidden-file names. Optional `dependencies` lists `managementFileNames`,
`projectDependencyPaths`, and `projectBinaryPaths` relative to the project.

#### Grammars and editor configuration

A language has two optional fields, `grammar` and `configuration`. Both are data: the host
reads them from the package and the editor applies them without starting any
process, so a language with a grammar needs no language server, backend, or
activation event, and a plugin may consist of grammars alone.

```json
{
  "id": "http",
  "displayName": "HTTP Request",
  "fileExtensions": ["http", "rest"],
  "grammar": { "kind": "textmate", "path": "grammars/http.tmLanguage.json" },
  "configuration": {
    "lineComment": "#",
    "blockComment": ["/*", "*/"],
    "brackets": [["{", "}"], ["[", "]"]],
    "autoClosingPairs": [["{", "}"], ["\"", "\""]]
  }
}
```

- `grammar.kind` names the highlighting engine. The contract defines only
  `textmate`; another engine is added as a new kind, so a plugin can move
  between engines without changing anything else.
- This is a static package contribution: `languages[]` belongs to the manifest
  and `grammar.path` references a package JSON file. No public host method accepts
  inline grammar JSON to dynamically register, replace, or dispose a highlighting
  provider. Live installation/update/withdrawal of eligible static packages does
  not imply such an API or automatic reload after arbitrary file changes;
  installed packages remain read-only.
- Adding another language or grammar using the supported structure needs only
  plugin data; changing its rules needs no contract change. A new
  engine or capability kind still needs an implemented host adapter and an
  explicit contract; an arbitrary new `kind` string cannot supply one.
- This contribution does not register completion, hover, definition, symbol,
  formatting, rename, or Code Action providers backed by ordinary plugin
  functions. A backend binds the provider types a host routes with
  [`editor/registerProviders`](#editor-language-feature-providers);
  existing LSP support and `editor/registerActions` are separate capabilities.
- `grammar.path` is a package-relative file that stays inside the package.
  Hosts read it read-only from the installed package, pass its text to
  `plugins.declarative.decodeGrammar`, and register the returned `source`, the
  text Core validated byte for byte, under the returned `scopeName`. A grammar
  Core refuses is `plugin.grammar.invalid` with the failing field (such as
  `grammar.repository.string.patterns[2].begin`); the language then opens as
  plain text and the rest of the plugin keeps working.
- A TextMate grammar is the JSON form VS Code reads, a `.tmLanguage.json`
  file; plist and YAML grammars are converted to JSON first. The fixture
  [`fixtures/languages/grammars/http.tmLanguage.json`](fixtures/languages/grammars/http.tmLanguage.json)
  is a complete example. A grammar is an object with `scopeName`, 1 to 128
  bytes of ASCII letters, digits, `.`, `-`, `_`, and `+`, and a `patterns`
  array, and optionally `repository`, `injections`, and `name`. Every rule is
  an object. The editor reads `include`, `match`, `begin`, `end`, `while`, `name`,
  `contentName`, `captures`, `beginCaptures`, `endCaptures`, `whileCaptures`,
  `patterns`, `repository`, and `applyEndPatternLast` (`true`, `false`, `0`,
  or `1`), and Core refuses a grammar where one of them has another type.
  Captures are an object keyed by group number or an array by position;
  object keys that are not group numbers are ignored. Every other key, such as
  `comment`, `fileTypes`, or `uuid`, is ignored. Root-level `firstLineMatch`
  is optional [file association metadata](#first-line-matching).
- Core accepts at most 2 MiB, 16384 rules counting every pattern, repository
  entry, and capture, rules nested 32 deep, regexes of 32 KiB, capture group
  4096, `name` and `contentName` of 1024 bytes, `include` targets and
  repository keys of 256 bytes, and 64 injections with selectors of 1024
  bytes. VS Code's own grammars, the largest of which is C++ at 662 KiB and
  3165 rules, are well within these bounds.
- Highlighting rules use the Oniguruma syntax TextMate grammars are written
  in. The editor compiles them. An unsupported rule never matches, and the
  rest of the grammar still applies.
- `include` is `$self`, `$base`, `#name` for a repository entry, or `scope` or
  `scope#name` for another grammar. Another grammar resolves against those the
  editor has, its own and those of enabled plugins, which is how a grammar
  embeds a language such as CSS inside HTML; an include that resolves to
  nothing matches nothing, as in VS Code.
- `injections` is an object of rules keyed by scope selector, run as VS Code
  runs them: where the scopes at a position match a selector, the rule's
  patterns are tried alongside the current rule's. The match that starts
  earlier wins; at the same start the current rule wins, unless the selector
  has the `L:` prefix. Only the grammar a document is tokenized with applies
  its injections; those of a grammar it includes do not, which is how
  `text.html.php` marks `<?php` inside the tags, scripts, and styles of the
  HTML it includes. Each injection is a rule checked and counted like any
  other. `injectionSelector` is not supported, since no grammar is injected
  into another, so it is ignored and not checked.
- A plugin grammar replaces the editor's own grammar of the same `scopeName`
  while the plugin is enabled. Two plugin languages cannot register the same
  scope: the first keeps it, and the other is reported and opens as plain
  text.
- `configuration` sets `lineComment`, `blockComment` (open and close),
  `brackets`, and `autoClosingPairs`. Each delimiter is 1 to 16 bytes without
  control characters; each list has at most 32 pairs.
- Plugin grammars never run on the editor UI thread. The host bounds the
  backtracking of each regex search and the time one line may take, so a
  grammar cannot stall editing; a line not yet tokenized, or that the grammar
  could not tokenize within that budget, is drawn as plain text.

#### First-line matching

A packaged TextMate grammar can add an optional root-level `firstLineMatch`
pattern for extensionless scripts or formats with a signature on the first
line. The package uses its existing `languages: 0` declaration and points
`grammar.path` at the grammar file. Earlier hosts ignore the hint and can
still install the package and apply its existing file associations and
highlighting. See the [fixture manifest](fixtures/languages/first-line.plugin.json)
and [grammar](fixtures/languages/grammars/sample.tmLanguage.json).

To keep the editor's existing highlighting for a contributed language, omit
`languages[].grammar` and package the hint in
[`grammars/<language-id>.first-line.json`](fixtures/languages/grammars/sample.first-line.json).
The standalone file is a JSON object of at most 4096 UTF-8 bytes with the same
`firstLineMatch` field. Earlier hosts ignore this resource. An explicit grammar
declaration takes precedence over the standalone file.

Core interprets this hint as a Rust regular expression of 1–512 UTF-8 bytes.
For example, `"firstLineMatch": "^#! */opt/tools/sample(?: |$)"` matches the
interpreter followed by a space or the end of the line. The pattern cannot
match empty input. The regex compiler and its DFA cache each have a 256 KiB
size limit. Unsupported or invalid hints are treated as absent. The original
grammar source continues to supply highlighting rules.

The editor tries this hint for text whose basename contains no dot, after
existing user and path associations. It checks only the complete decoded
first line without the terminator. Empty lines, lines longer than 1024 UTF-8
bytes and lines containing CR, LF or NUL are ineligible. A match must consume
text at the start of the line. The first match in the enabled language
registry supplies the language ID for editing and for language services.
The matcher reads text without starting a process.

### Language servers

| Field | Meaning |
| --- | --- |
| `languageID` | A declared language. |
| `languageIdentifier` | LSP `languageId` for documents; defaults to `languageID`. |
| `launch` | `{"kind": "packaged", "path": …}` runs an executable inside the package. `{"kind": "systemCommand", "commands": [...]}` lists bare command names the host resolves with its tool discovery (PATH, project toolchains, user overrides). `{"kind": "runtime", …}` starts a package file with a packaged or discovered runtime; see [Runtime launch](#runtime-launch). `{"kind": "backend", …}` has the plugin backend answer the command line at each start; see [Backend-resolved launch](#backend-resolved-launch). The launch fields are described in [Process start](#process-start). |
| `arguments` | Arguments passed to the server, written as [launch templates](#launch-templates). |
| `validationArguments` | Probe that must exit with status 0 before a candidate is used; empty means the host trusts the candidate and lets the LSP handshake decide. |
| `runtimeDependencies` | Tools the host must discover and pass to the server, each a tool-definition ID that `requiresTools` lists (see [Tool references](#tool-references)). The host exports each path as `LITHE_<TOOL>_PATH` (`LITHE_NODE_PATH`, `LITHE_GO_PATH`; a user-set value takes precedence) and prepends its directory to the server's `PATH`, because servers such as gopls run the tool themselves and an app opened from the Dock does not inherit the shell `PATH`. A project-local copy is never chosen, since servers start automatically when a file opens. |
| `initializationOptions` | A JSON object sent as the LSP `initializationOptions`. Every string value inside it is a launch template. |
| `indexingExclusions` | The array in the server's workspace configuration that the host fills with the workspace's excluded directories; see [Indexing exclusions](#indexing-exclusions). |
| `hostIntegration` | Reserved. Core implements no host integration, so any ID is refused; see [Language server host integrations](#language-server-host-integrations). |
| `executesProjectCode` | `languageServers` 1. Whether the server runs code or configuration the workspace controls, such as build scripts it imports or project-local plugins it loads. Absent means `true`: the server starts only in a trusted workspace. Declare `false` only for a server that reads the project's files as text and runs nothing from them; see [Workspace trust](#workspace-trust). |
| `requestMethods`, `commands`, `serializedCommands`, `forwardNotifications`, `state`, `remote` | What the plugin backend may ask of the running server and how the host keeps its state; see [Plugin-owned sessions](#plugin-owned-sessions). |

One enabled plugin serves a language. Enabling a plugin, or confirming the
installation of an enabled one, turns off every other enabled plugin whose newest
installed version declares a language server for one of the same languages; the
host tells the user which plugin it turned off and why. Two plugins can serve the
same language as alternatives, for example a lightweight navigation server and a
complete one, but never at the same time, so neither has to know the other.

The host owns the language-server process lifecycle, the LSP session, and
stopping the process when the provider is disabled. A build provider with its own
process is the same kind of process: it declares `launch`, `arguments`,
`validationArguments`, `runtimeDependencies`, and `initializationOptions` with
the same meaning and validation; see [Build providers](#build-providers).

The host keeps a language server's hover answers for the life of its LSP
session, so resting on the same symbol again does not wait for the server. A
kept answer is shown for the document version and position it was asked for,
and for any other position inside the answer's `range` when that range lies on
the requested line and contains the requested position. A session keeps at most
32 answers and drops the least recently used one first. The host never keeps an
empty answer, an error, an answer with more than 64 KiB of text, or an answer
that arrives after the kept answers were forgotten; each of these is delivered
and asked for again next time. Every kept answer is forgotten when a document
opens, changes, is saved or closes; a workspace file changes on disk; settings or
indexing exclusions change; a command runs; a plugin backend sends
`languageServer/request` or `languageServer/notify` to the server, or the host
sends a notification to a plugin backend; the server sends the client any
request or ends a `$/progress` report; and when the session stops. A server
whose hover changes for any other reason announces it with a request, such as a
refresh, or by ending a progress report.


#### Language server host integrations

`hostIntegration` is reserved. Core implements no host integration and never
matches on a language ID, a server ID or a tool name: everything a language
needs beyond its launch comes from the plugin's own backend, providers and
declarations. Any `hostIntegration` ID is refused with
`plugin.integration.unsupported`, at `languageServers[<n>].hostIntegration.id`,
so a manifest naming one fails validation instead of loading without it. A build
system's `hostIntegration` is a separate declaration with the same rule; see
[Host integrations](#host-integrations) under [Build systems](#build-systems).

A plugin that declares a language owns its process-backed features: the host
stops running its built-in tools for that language, so a missing or disabled
plugin never falls back to them. A feature that none of the plugin's
`executionProviders` supplies is unavailable for the language.


#### Workspace configuration

A language server may declare `workspaceConfiguration`, an object whose
string leaves may be `${setting:<id>}` templates.
Core expands it with the merged settings
(`plugins.declarative.workspaceConfiguration`, and the launch plan's
`workspaceConfiguration`), answers the server's `workspace/configuration`
requests by section path from it, and sends
`workspace/didChangeConfiguration` after `initialized`. Hosts send a new
value with `lsp.updateWorkspaceConfiguration` when an `immediate` setting
changes; the running server receives it without a restart. The value
replaces the whole configuration. For a server whose
[backend-resolved launch](#backend-resolved-launch) answer carried its own
`workspaceConfiguration`, the new value comes from the launch function
again, not from the declaration.

#### Indexing exclusions

The user can exclude workspace directories from indexing, and directories Git
ignores can be excluded too. Many servers index every directory unless one of
their own settings lists the ones to skip. A server whose setting does so
declares `indexingExclusions`, and the host keeps that setting filled; Core
never guesses the setting from the language, the server or the tool.

| Field | Meaning |
| --- | --- |
| `section` | Dotted path of the array in the workspace configuration, such as `gopls.directoryFilters`; at most 8 non-empty keys without whitespace. |
| `entry` | One excluded directory. `${path}` appears exactly once and is the directory's workspace-relative path separated by `/`; no other template is allowed. |
| `rootEntry` | The entry for the workspace root itself. Without it the root is `entry` with the path `.`. |
| `default` | What the server uses when the configuration has no such array, at most 32 entries. |
| `pathSyntax` | `literal` (default) or `glob`: how the server reads the path. |

- The host answers `workspace/configuration` and sends
  `workspace/didChangeConfiguration` with the excluded directories appended,
  in order, to the array at `section`. It never changes the stored
  configuration, so removing an exclusion restores the user's array exactly.
- An array the configuration already has is kept and the entries are added
  after it; an entry already present moves to the end, because filters such as
  gopls's are read in order and the last match wins. Without an array the
  host starts from `default`, so the server's own default still applies.
- A configuration written with dotted keys (`"gopls.directoryFilters"`,
  `"gopls": {"directoryFilters": …}`) is normalized to nested objects along
  `section` before the entries are added; unrelated settings are kept.
- With `glob`, a directory whose name contains `*`, `?`, `[`, `]`, `{` or `}`
  is left out, since writing it would exclude every directory the pattern
  matches.
- When the excluded directories change, a running server receives
  `workspace/didChangeConfiguration` again. A server without
  `indexingExclusions` sees its configuration unchanged; the host still stops
  sending it the documents, diagnostics and file changes of excluded
  directories.
- A text field is at most 256 bytes without control characters, and
  `rootEntry` and `default` entries are literal. A violation is
  `plugin.manifest.invalid` at `languageServers[<n>].indexingExclusions.<field>`.

```json
"indexingExclusions": {
  "section": "gopls.directoryFilters",
  "entry": "-${path}",
  "rootEntry": "-",
  "default": ["-**/node_modules"],
  "pathSyntax": "glob"
}
```

#### Plugin-owned sessions

A plugin backend drives its own server only through what the server
declares. A server may declare:

| Field | Meaning |
| --- | --- |
| `requestMethods` | LSP methods the backend may send with `languageServer/request` or `languageServer/notify`; at most 64. |
| `commands` | The `workspace/executeCommand` commands the backend may run; at most 128. Declared exactly when `requestMethods` lists `workspace/executeCommand`. |
| `serializedCommands` | A subset of `commands` that runs one at a time: a request for a command already running joins it and receives the same answer. |
| `forwardNotifications` | Server notifications the host forwards to the backend; at most 32. |
| `state` | `{scope: "workspace", retention: {maxEntries, maxAgeDays}}`. The host prunes the server's per-workspace state directories (see below). `maxEntries` is 1 to 64, `maxAgeDays` 1 to 365. |
| `remote` | `supported` (default) or `unsupported`. |

- A name is non-empty text of at most 256 bytes without whitespace or control
  characters, unique within its list. Names the host sends itself
  (`initialize`, `initialized`, `shutdown`, `exit`, `textDocument/didOpen`,
  `didChange`, `didClose`, `didSave`, `willSave`, `willSaveWaitUntil`,
  `workspace/didChangeWorkspaceFolders`, and every `$/` method) cannot appear
  in `requestMethods` or `forwardNotifications`.
- A server that lists `requestMethods` or `forwardNotifications` requires the
  `languageServer.request` permission; declaring the permission without such a
  server is `plugin.manifest.invalid`.
- The launch plan's `sessionPolicy` is these fields as sorted lists plus
  `serverID` (the `languageID`), `gates` and, when declared, the server's
  `indexingExclusions`. The host sends it unchanged as
  `lsp.startServer.sessionPolicy`; Core's LSP engine enforces it, so a host
  cannot widen it and a backend cannot step outside it.
- A server with `remote: "unsupported"` is not started for a remote
  workspace: `lsp.startServer` answers `not_supported`.

What the engine enforces on `lsp.pluginServerRequest` and
`lsp.pluginServerNotify`, the two commands behind the host methods:

- A request must be in `requestMethods`, and a `workspace/executeCommand`
  must carry a `params.command` in `commands`; anything else is
  `plugin.host.methodNotDeclared`. A notification may be
  `workspace/didChangeConfiguration` (the backend always knows the server's
  settings; the value also becomes the settings the server reads through
  `workspace/configuration`) or a `requestMethods` entry other than
  `workspace/executeCommand`. The host's own methods stay refused.
- The session must belong to the calling plugin
  (`plugin.host.foreignContribution` otherwise) and be running
  (`plugin.host.languageServerUnavailable` otherwise, including every request
  still pending when the process stops, crashes, restarts, or the plugin is
  disabled).
- A request waits at most `timeoutMilliseconds` (default 60000, at most
  300000) and ends with `plugin.host.timeout`; cancelling it sends the server
  `$/cancelRequest` and ends it with `plugin.host.cancelled`. A server's JSON-RPC
  error is `plugin.host.languageServerError` with `details: {code, message}`;
  the server's `data` is not forwarded. A result over the host's result bound
  is `plugin.host.limitExceeded`.
- A **gate** is `{id, until: "notification", method, match}`, declared by the
  launch answer. It opens when the server sends a notification with that
  `method` whose params contain every member of `match` (nested objects match
  the same way). Until every gate has opened, `request` calls wait in order,
  at most 128; the 129th is `plugin.host.busy`. A waiting request still obeys
  its timeout and cancellation. Notifications are never held.
- The first request for a `serializedCommands` command runs; a later request
  for the same command, while the first is running or waiting, joins it and is
  answered with the first one's outcome. Cancelling the first request ends the
  joined requests with it.
- `forwardNotifications` reach the backend as
  `lithe/host/languageServer/didReceiveNotification`.
  `workspace/didChangeConfiguration` is the only notification Core consumes
  itself.

**State and retention.** The host keeps one state directory per workspace for
each server, under the plugin's storage (never under the package or application
bundle), and gives the server its own as `workspaceStateDirectory`. The
directories survive disabling and updating the plugin. The plugin chooses the
sub-directory names inside its `workspaceStateDirectory`; the host chooses the
names of the per-workspace directories themselves and never looks inside them.

The host refreshes a workspace directory's modification time on every start of
the server and sweeps in the background after a start. It asks
`plugins.languageServers.stateRetention` with `{retention, nowUnixSeconds,
entries: [{name, lastModifiedUnixSeconds}], activeNames}`, where `entries` are
the per-workspace directories it found and `activeNames` are those of open
workspaces, and deletes the `expiredNames` it returns: a directory older than
`maxAgeDays` and, of the rest, all but the newest `maxEntries` that open
workspaces leave free. A workspace that is open is never pruned. Names that are
not a single path component are ignored, and the host deletes only names it
listed. A plugin that declares no `state.retention` keeps everything.

The host can also discard the state of one workspace; the server rebuilds it
the next time it starts. That is an engine path only, with no user-facing
action.

**Preparation status.** `languageServer/setStatus` reports how ready the
server is. It exists because `lithe/function/progress` belongs to a call that
is running, while preparation (importing a build, indexing) outlives any call.

Example package: `fixtures/languages/language-server-backend-launch.plugin.json`.
### Structured plugin diagnostics

Every plugin-owned process may negotiate `capabilities.experimental.litheLogging`:
the host offers `{ "versions": [0] }` in `initialize`, and the SDK answers
`{ "version": 0 }`. The official SDKs require that offer. The protocol has no
version of its own: its version is `pluginAPIVersion`, and it is negotiated
independently of the other runtime protocols.

After successful initialization, the backend sends a JSON-RPC notification
`lithe/log`. Its params contain only these fields; unknown fields are rejected:

| Field | Meaning |
| --- | --- |
| `kind` | `operationFinished`, `operationFailed`, `degraded`, or `recovered`. These select INFO, ERROR, WARN, and WARN catalog events respectively. |
| `operation` | A registered function ID (up to 64 bytes), `function-call` for a longer registered ID, a supported `lithe/build/*` or `lithe/vcs/*` request method, `lithe/keymap/import`, or the SDK-owned stages `initialize`, `shutdown`, `notification`, `result-encoding`. Arbitrary display labels and user input are refused. |
| `durationMs` | Required for finished/failed, absent for degraded/recovered. Monotonic elapsed integer milliseconds, 0 through 9007199254740991. |
| `code` | Required except for finished. Stable category, 1–64 ASCII bytes from letters, digits, `._:/-$`; never an exception message, command line, query, or response body. |
| `count` | Optional affected item count, a nonnegative JSON safe integer. |
| `operationId` | Optional originating wire operation ID, 1–128 ASCII token bytes. The SDK obtains it from the call context. Core resolves a still-active wire request to its host operation; unknown or retired IDs provide no correlation. |

The notification must precede the corresponding RPC response so its wire ID
still belongs to the active host operation. Cancellation is an expected result
and does not produce an operation failure. The SDK owns automatic function and
domain request terminal facts; plugin handlers must not duplicate them. A
handler records a warning only when it actually recovers or returns a degraded
result. Frequently repeated read/preview/poll functions suppress successful
terminal facts; their failures remain observable.

Core supplies the installed plugin ID and routes these facts to
`plugins/<id>/host`. Plugins cannot provide an event ID, channel, severity,
message, arbitrary fields, error body, or writer path. The fixed catalog events
are `plugins.operation.finished`, `plugins.operation.failed`,
`plugins.operation.fallback_selected`, and `plugins.operation.recovery_finished`.
Malformed or unnegotiated input produces one category-only
`plugins.diagnostics.rejected` warning per process session, without retaining
the rejected payload. Ingress admits at most 600 facts per minute per session,
then uses the existing bounded Core handoff to the host's single writer;
overload increments its dropped counter. No polling timer or extra writer is
created. Fixture: `fixtures/backend/plugin-logging.json`.

Declarative contributions have no plugin process to send `lithe/log`, so whoever
executes the declared work records the same `plugins.operation.finished` and
`plugins.operation.failed` events on the same `plugins/<id>/host` channel. A
declarative plugin writes no logging code. Each start of a `languageServers[]` or
`debugAdapters[]` entry is one `languageServer/start` or `debugAdapter/start`
operation, from launch resolution until `initialize` succeeds. Each run of a
`buildSystems[]` command source for a model, dependencies, or test discovery
is one `lithe/build/model`, `lithe/build/dependencies`, or
`lithe/build/discoverTests` operation, using the SDK request names. Each
installation of a [command installer](#command-installers) the user confirmed is
one `commandInstaller/install` operation, from planning the chosen target until
every provided command is in its directory. A failure carries one `code`:

| Code | Meaning |
| --- | --- |
| `host.planFailed` | The declared command could not be planned |
| `host.executableUnavailable` | The declared executable or launch input could not be resolved |
| `host.launchFailed` | The resolved process could not be started |
| `host.initializeFailed` | A started server or adapter failed its `initialize` handshake |
| `host.timedOut` | The attempt exceeded its deadline |
| `host.exitedEarly` | A server or adapter exited before `initialize` succeeded |
| `host.commandFailed` | A build command or an installation exited unsuccessfully |
| `host.outputInvalid` | A build command's output is not a valid document, or an installation that exited successfully did not provide its commands |

User cancellation and superseded attempts record nothing. Plugin output, paths,
and error text never appear in these facts. The executing owner and the
host-facing command are specified as `diagnostics.recordPluginOperation` in
[`rust-core-api.md`](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/contracts/rust-core-api.md).

Severity follows the actual result. An operation that failed for the user is
ERROR, a recovery or fallback is WARN, and a useful successful user operation
is INFO. The source stream is not a severity: unclassified stderr is DEBUG
evidence, while standard `window/logMessage`/`window/showMessage` types retain
their explicit error/warning/info/detail semantics through separate catalog
events. Raw stdout is reserved for protocol frames. Source code, credentials,
SQL, HTTP headers/bodies, terminal history, protocol payloads, and command
output must never be included in diagnostic fields.

Native upstream files may use `${logDir}` only when the host supplies the
current local writable session directory. They remain separate files, not
structured catalog events. Keep metadata and caches in `${stateDir}` or
`${workspaceStateDir}`, and never infer a writable log path from `${pluginDir}`.
These runtime files do not modify installed resources or participate in build
resource reuse. Hosts without a local `logDirectory` reject launches requiring
`${logDir}`.

### Plugin settings

A plugin may declare options the user can change, one set per plugin and
shared by every workspace. The host shows them on the plugin's page, stores
the values, and passes the stored values as `settings` (setting ID to JSON
value) with each launch plan request. A change restarts the plugin's processes
unless the setting's [`effect`](#placement-scope-effect-and-page-buttons) says otherwise.

```json
"settings": [
  { "id": "heapMB", "type": "integer", "title": "Maximum memory",
    "default": 2048, "minimum": 512, "maximum": 8192, "unit": "MB" },
  { "id": "logLevel", "type": "enum", "title": "Log level", "default": "info",
    "options": [{ "value": "info", "title": "Information" }, { "value": "debug", "title": "Debug" }] },
  { "id": "telemetry", "type": "boolean", "title": "Send usage data", "default": false }
]
```

| Field | Meaning |
| --- | --- |
| `id` | Starts with a lowercase letter, then letters and digits. Unique within the plugin. |
| `type` | `integer` (requires `minimum` and `maximum`, optional `unit`), `enum` (requires one to sixteen `options`, each `{value, title}`; values contain only letters, digits, `.`, `_`, `-`), `boolean`, `multiSelect` (one to 32 `options`; the value is an array of distinct option values), `text` (requires `pattern`, matched against the whole value; optional `maxLength`, default 256 and at most 1024 Unicode scalars; optional `placeholder`), or `file` and `directory` (an absolute path of at most 1024 Unicode scalars, or empty for "not set"; optional `placeholder`). Text and paths never start with `-` and contain no control characters. |
| `title`, `description` | Text the host shows. Write it in English; the package may [translate](#translations) it for other interface languages. |
| `default` | Value used until the user changes it. Its JSON type follows `type`, and it must be accepted by the declaration. |

Because every value is a number, a boolean, or one of the declared option
values, a setting can never inject extra arguments or change which package
file runs. A stored value the declaration no longer accepts (for example after
an update narrowed a range) is replaced by the default; a launch never fails
for it. Values for undeclared IDs are ignored. A template that is exactly one
`${setting:<id>}` keeps the setting's JSON type, so an `initializationOptions`
value can be a number or boolean; inside longer text the value is substituted
as text. Declaring settings, or referring to an undeclared one, is refused
with `plugin.manifest.invalid` or `plugin.template.invalid`. The settings
declaration is not part of an install review's program list, because no value
can change what runs beyond what the templates already show.

#### Placement, scope, effect, and page buttons

A setting may also say where it is shown and when a change takes effect, so
a plugin can place its options anywhere in the host's settings without the host
knowing the plugin. Hosts keep only generic registration points: a page per
language and per build system, sections, one control per setting type, a
project/global layer switch, and buttons. What appears there comes from the
manifest. Fixture: `fixtures/settings/settings.plugin.json`.

```json
"settings": [
  { "id": "dialyzer", "type": "boolean", "title": "Dialyzer", "default": true,
    "scope": "project", "effect": "immediate", "page": { "languageID": "elixir" } },
  { "id": "mixEnv", "type": "text", "title": "MIX_ENV", "default": "dev", "pattern": "[a-z]+",
    "scope": "project", "page": { "languageID": "elixir" }, "section": "Server" }
],
"settingActions": [
  { "id": "rebuildPlt", "title": "Rebuild PLT", "function": "dialyzer.rebuild",
    "page": { "languageID": "elixir" }, "confirm": true }
]
```

| Field | Meaning |
| --- | --- |
| `scope` | `global` (default): one value for every project. `project`: the open project may override it. |
| `effect` | When a change takes effect. `restart` (default): the next start of the plugin's processes. `immediate`: pushed to the running language server through `workspaceConfiguration`. `reload`: the next build-model reload; hosts reload the plugin's build models. `nextRun`: the next run or build command. |
| `page` | `{ "languageID": … }` or `{ "buildSystemID": … }`: the settings page that shows the setting besides the plugin's own page. Any plugin may add to a page; the host shows the page while an enabled plugin contributes to it. |
| `section` | Heading the setting is grouped under on its page. |

`settingActions` (at most 16, sorted by unique `id`) are buttons the host
draws on the plugin's page and on their `page`. Each invokes the plugin
function `function` of the same plugin with empty params, served by the
plugin's own provider process for the open project (see
[plugin functions](#plugin-functions)). `confirm: true` makes the host ask
first. The host owns the button and knows nothing about what the function
does. Declaring actions counts as using `settings`.

Placement and use rules, checked by Core at validation:

- An `immediate` setting may appear only in a language server's
  `workspaceConfiguration`; it never reaches a command line or
  `initializationOptions`, so a change never needs a new process.
- A `text`, `file`, or `directory` setting is a whole template value or the
  value of one `name=` argument, never part of a longer argument.

**Layers.** Hosts store a global value per plugin and setting, and a project
value in the project's `.lithe/plugin-settings.json`
(`{pluginID: {settingID: value}}`, meant to be shared through version
control). They send both as `settings` and `projectSettings`; Core merges
them as project (for `scope: "project"` only), then global, then default,
skipping a stored value the declaration no longer accepts.
`plugins.declarative.resolveSettings` returns each effective value with
its `source` (`project`, `global`, or `default`).

#### Settings regions

`settingRegions` (at most 16, unique `id`s, at most one per page) gives a
plugin a part of a settings page that the host cannot describe with declared
rows: search keywords, a [plugin view](#plugin-views), or both. A region sits
on its `page` (as for settings, plus `{ "compilerID": … }` for a compiler
page), or on the plugin's own Settings tab when it names no page. Fixture:
`fixtures/settings/settings.plugin.json`.

```json
"settingRegions": [
  { "id": "plt", "page": { "languageID": "elixir" },
    "keywords": ["PLT", "dialyzer cache"], "view": { "function": "settings.plt" } }
]
```

| Field | Meaning |
| --- | --- |
| `id` | Starts with a lowercase letter, then letters and digits. View events name it as `settingRegionID`. |
| `page` | The page the region appears on; the plugin's own Settings tab when absent |
| `keywords` | 1–32 distinct words (ignoring case) of 1–64 characters without line breaks or surrounding spaces. Settings search finds the page by them in addition to the page title and the declared rows, and opens the page at this plugin's part. |
| `view` | `{function}`: the plugin view drawn between the page header and the plugin's declared rows on that page |

A region declares `keywords`, `view`, or both. Keywords alone need no
backend; a view needs what [plugin views](#plugin-views) need. Translations
may add keywords for other languages, and the page projection lists the
declared keywords followed by the translated ones that differ, so the page is
found in both languages. Declaring regions counts as using `settings`. A
region never stores values: its bound controls edit the same draft as the
declared rows.

#### Source-backed settings editors

An action may declare `sourceEditor: {"editFunction":"compiler.prepareSettings"}` and require `sourceSettings: 0` (or `1`, below)
alongside `settings: 0`. The host invokes the read function only after an
explicit user action; opening its page must not run project wrappers or tools.
The function must **only read** and return the
non-sensitive `litheSettingsSources` snapshot described below. It never returns
source documents. Source-editor actions may use `page: {"compilerID":"<id>"}`
to appear under Build, Execution, Deployment; other placements
retain their existing behavior. The editor, tables, pickers, theme, cancellation
and explicit Save/Cancel actions belong to the host. The plugin owns formats,
upstream discovery, module identity, supported choices and effective values.

A source editor requires the plugin's own backend. Its optional `languageID`
names a declared language server: the host makes it ready before querying.
Its optional `buildSystemID` names a declared build system: each detected instance
contributes `{root,options,tool,model}` from the same resolved native executable,
stored options and upstream model used for Build. Reads receive
`{context:{language,builds:[...]}}`; each `model` contains only the upstream
`projects` projection, without task/run lists or document contents. `language`
is the host interface language. No build selection is inferred from a tool's name.
The read function receives `operation:"readAfterSave"` after publication and
must refresh the upstream tool's native configuration before projecting it.

Snapshot fields (unknown fields are rejected):

| Field | Contract |
| --- | --- |
| `format`, `version`, `title` | `litheSettingsSources`, `0`, and a localized nonblank title of at most 256 UTF-8 bytes. The complete snapshot is at most 2 MiB. |
| `groups` | 1–32 ordered `{id,title,layout,rowTitle?}` sections. `layout` is `form`, `table`, or (version 1) `sidebar`; table rows use source titles in the first column and their fields as value columns. A `sidebar` group lists its sources for navigation, like a module list, and shows the fields of the one the user selects. |
| `sources` | At most 1024 `{id,group,title,path,revision,writable,fields,parent?}` bindings. `parent` (version 1, `sidebar` groups only) is the ID of an earlier source of the same group under which the host nests this one, at most 8 levels deep. `path` is normalized, workspace-relative, slash-separated, at most 4096 bytes and contains no empty/dot/parent components, drive letters or backslashes. Duplicate IDs or paths are rejected. Native adapters must additionally reject native file aliases and any installation resource. |
| `revision` | `missing`, or `sha256:` followed by 64 lowercase hexadecimal digits covering the exact source bytes. Reading a missing source does not create it. |
| `fields` | 1–32 `{id,title,description?,section?,kind,value,effectiveValue,inheritedValue?,effectiveSource,options?,entryKinds?,attributes?}` records per source. `section` (version 1) is a localized label of at most 64 bytes: in a `sidebar` group each distinct section becomes a tab, in order of first appearance, and fields without one come before the tabs; in a `form` group it is a subheading. Table fields have no section. IDs are opaque identifiers using only lowercase ASCII letters, digits and hyphens, at most 128 bytes; labels are nonblank and at most 256 bytes, descriptions at most 2048 bytes. Only explicitly non-sensitive fields may be projected. |
| Field values | `value:null` means absent/inherited. `effectiveValue` comes from the upstream tool and may also be null if there is no value. `inheritedValue` is the upstream value after removing a local override; omit it or use null when unknown. `effectiveSource` is a localized origin label of at most 1024 bytes. |
| Field kinds | `enum` with 1–64 distinct `{value,title}` options; `boolean`; `text` up to 8192 bytes; `stringList` or `fileList` with at most 256 nonblank strings, each at most 4096 bytes; (version 1) `entryList`, described below. Non-enumeration fields omit options. `fileList` adds the host's file picker; list editing uses one item per line, preserving spaces and platform path spelling. Only `entryList` fields have `entryKinds` and `attributes`, and tables cannot hold them. |

**Entry lists (version 1).** An `entryList` is an ordered list of typed
entries, such as a module's dependencies or source folders. The host shows it
as a list with an add menu, remove, move up and move down, and one column per
attribute; a read-only source shows it without those controls.

| Member | Contract |
| --- | --- |
| `entryKinds` | 1–16 `{id,title,input,extensions?,choices?}` in add-menu order. `id` is an opaque identifier, `title` a localized menu item. `input` is `file` or `directory`, which open the host's native picker, or `choice`, which offers `choices`. A `file` kind may list up to 16 `extensions` (ASCII letters and digits, no dot) to filter the picker. A `choice` kind has 0–1024 distinct `{value,title,detail?}` choices: `value` up to 4096 bytes, `title` up to 256, `detail` up to 1024; with none, nothing of that kind can be added now. |
| `attributes` | 0–4 per-entry columns `{id,title,kind,options?}`. `kind` is `boolean` or `enum`, which has 1–64 distinct `{value,title}` options. |
| Value | Up to 256 entries `{kind,value,title?,detail?,attributes?}`. `kind` names a declared entry kind; `value` is nonblank, at most 4096 bytes, and no two entries share both `kind` and `value`. `title` (at most 256 bytes) and `detail` (at most 1024) are display text; without a title the host shows `value`. `attributes` maps declared attribute IDs to a boolean or one of the attribute's option values; an absent key means the plugin's default, which the host shows as off or as the first option. |
| Edits | `value` is the native list in its order. Display text never makes two lists differ and never reaches the edit function, which receives `[{kind,value,attributes?}]`. A `file` or `directory` entry the user adds carries the picker's absolute native path; the plugin decides how to store it, for example relative to the source. An added `choice` entry must be one of the kind's current choices or an entry the field already holds, so the host cannot introduce a value the plugin did not offer. An empty list is an explicit value, not inheritance. |

Version 1 adds `sidebar`, `parent`, `section` and `entryList` without changing
the snapshot `version` (still `0`) or anything a version-0 snapshot means. A
plugin whose snapshots use them requires `sourceSettings: 1`; a host that
supports only version 0 rejects such a snapshot and does not install the plugin.

No secret/password field, arbitrary script, CSS or host action is accepted.
All plugin-provided labels and choices use the current interface language.
Raw source content, credentials and unrelated configuration stay private to
the plugin and native adapter; they must not appear in RPC, normal controls,
model inputs or logs. The host presents inherited values without creating an
override, maintains drafts and displays each confirmed target path. Core
`plugins.settings.sourceState` supplies `inputs`, `inheritedInputs`, ordered
`layout`, native I/O `limits`, and `effectiveDisplays`. The latter describes
inherited non-enum values and actual values differing from explicit raw values as `unknown`, `text`,
`boolean`, or list `count`, with an `inherited` flag; the host localizes these presentation hints. An
`entryList` draft is `entries`, its entries in order, and its effective display
is a `count`.

On explicit Save the host uses `plugins.settings.sourceEdit` to derive only
changed raw fields. It captures each target with a bounded native reader,
verifies the snapshot revision and current workspace/package ownership, and
creates a private **already existing empty** staging file. It invokes the
distinct `sourceEditor.editFunction` with
`{operation:"editSource",sourceID,revision,values,outputFile}`. `values` maps
opaque field IDs to raw JSON values; null removes just that override.
The function rechecks the source revision, uses its upstream/mature reader
and writer, preserves unrelated fields/comments/secrets, and writes only to
the supplied existing staging file without CREATE or following symlinks.
It does not write the original source. Cancellation cannot recreate staging.

The reply is exactly
`{format:"litheSettingsSourceEdit",version:0,sourceID,revision,changed}`.
`plugins.settings.sourceReceipt` binds it to the confirmed source. The native
adapter checks the staged bytes' SHA-256 and uses conditional atomic
publication preserving permissions and native metadata. A no-op never creates
a source. Multiple source changes are all prepared and verified before the
first publication; failures retain drafts and report any partial publication.
Publication must still own the workspace, enabled package version and editor
read baseline. After saving, invalidate affected tool facts and invoke the
read function again; report success only after each published digest appears
in the new snapshot. Missing sources and cancel are normal outcomes; all
other failures use the existing action/SDK logging path. Staging is private,
bounded, never reusable across worktrees, and removed on every terminal path.

Invalid forms, edits or receipts return `plugin.settings.invalidSourceEditor`;
unknown or revoked actions retain their existing errors. These editors are
local-workspace operations; remote workspaces are not supported in version 0.
Fixtures: [`settings-sources.json`](../../../../rust/fixtures/plugins/settings-sources.json)
(version 0) and [`settings-sources-modules.json`](../../../../rust/fixtures/plugins/settings-sources-modules.json)
(version 1: a `sidebar` of nested modules with sections, entry lists, attributes and a read-only module).

#### Translations

A package may translate its setting text with `l10n/<language>.json` files
([schema](plugin-l10n.schema.json)), where `<language>` is a language tag such
as `zh-Hans` or `ja`. `plugin.json` remains the text for every language without
a translation. Fixture: `fixtures/settings/l10n/zh-Hans.json`, with golden cases
in `fixtures/settings/localization-cases.json`.

```json
{
  "settings": {
    "dialyzer": { "title": "Dialyzer 静态分析" },
    "checks": { "title": "检查项", "options": { "format": "格式检查" } }
  },
  "sections": { "Server": "服务器" },
  "settingActions": { "rebuildPlt": { "title": "重建 PLT" } },
  "settingRegions": { "plt": { "keywords": ["PLT 缓存"] } }
}
```

| Member | Translates |
| --- | --- |
| `settings.<id>` | The setting's `title`, `description`, `placeholder`, and `unit`; `options.<value>` is the title of an `enum` or `multiSelect` option |
| `sections.<heading>` | A section heading, named exactly as `section` writes it |
| `settingActions.<id>` | The action's `title` and `description` |
| `settingRegions.<id>` | `keywords`: 1–32 search words added to the region's declared ones. The view's own text is not translated here: each view event carries the interface language. |
| `runConfigurationTypes.<buildSystemID>.<typeID>` | A [run configuration type](#run-configuration-types)'s `displayName` and `description`, and `fields.<id>` with a field's `title`, `description`, `placeholder`, and `options.<value>`; refused as `plugin.build.invalidLocalization` |
| `beforeLaunchTasks.<buildSystemID>.<taskID>` | A [before-launch task](#before-launch-tasks)'s text, shaped like `runConfigurationTypes`; refused as `plugin.build.invalidLocalization` |
| `buildOptions.<buildSystemID>.<optionID>` | A [build option](#build-systems)'s `displayName` and `description`, and `choices.<value>` with a declared choice's `title`; Core applies it to the manifest before detection (`build.localizeOptions`), so the option dialogs, toolbar toggles and tree nodes show it; refused as `plugin.build.invalidLocalization` |

Every member is optional, and text a file leaves out stays as `plugin.json`
writes it. IDs, option values and their order, stored values, and the grouping
and order of sections are the same in every language. For the interface
language `zh-Hans-CN` the host reads `l10n/zh-Hans-CN.json`, `l10n/zh-Hans.json`,
and `l10n/zh.json`, and shows the first one Core accepts, so one `zh-Hans` file
serves every region.

Core refuses a whole file, never part of it, with
`plugin.settings.invalidLocalization` when:

- it is not a JSON object, is larger than 1 MiB, repeats a key, or holds `null`;
- an entry under `settings`, `sections`, `settingActions`, or `settingRegions`
  has a field this format does not define;
- it names a setting, option value, heading, action, or region the manifest
  does not declare, gives `options` to a setting without options, or translates a field
  the manifest leaves out;
- a title, placeholder, unit, or heading is empty or longer than 120
  characters, or a description is empty or longer than 480;
- a region's `keywords` break the rules of
  [declared keywords](#settings-regions).

The host then tries the next candidate and finally shows the manifest text, so
a broken translation never hides a setting; it logs the refusal for the
plugin's author. Core ignores top-level members it does not know, so a later
host can read translations of more plugin text from the same files.
Translations need no capability or `requires` entry: a host that does not read
them shows the `plugin.json` text.

### Execution providers

`run` and `test` each name an `executable`: exactly one of a bare `command`
resolved on the host's tool path, or a host-owned project `toolchain` ID such
as `project-go`. The working directory is always the workspace root.

`test` also declares:

| Field | Meaning |
| --- | --- |
| `frameworkID` | Stable framework label such as `phpunit` or `go`. |
| `workspaceLabel` | Label of the "all tests" item and plan. |
| `projectMarkers` | File names, any one of which makes the workspace a test project. |
| `testFilePatterns` | Case-insensitive file-name patterns using only `*`. |
| `workspaceArguments`, `fileArguments`, `testCaseArguments` | Argument templates per scope. |

Argument templates are argv elements, never shell text. Each element is a
string or `{"value": …, "when": "fileSelected" | "noFileSelected"}`;
conditional elements are only allowed in `testCaseArguments`, where a file is
optional.

| Variable | Available in | Value |
| --- | --- | --- |
| `${file}` | run, file, test case | Workspace-relative file. A name starting with `-` becomes `./<name>`. |
| `${fileDirectory}` | run, file, test case | `./<directory>` of the file, or `.` at the workspace root. |
| `${testName}` | test case | Test identifier with surrounding whitespace removed. |
| `${testName:regex}` | test case | The identifier with regular-expression metacharacters escaped. |
| `${userArguments}` | run | The user's program arguments; must be a whole element and may expand to zero elements. |

In `testCaseArguments`, an element that uses `${file}` or `${fileDirectory}`
must be guarded with `"when": "fileSelected"`, and some element must use the
test name.

Discovery returns `{id: "<language>:workspace", kind: "workspace"}` followed by
`{id: "<language>:file:<path>", kind: "file", relativeFilePath}` for matching
files sorted by path, or an empty list when no project marker is present.

## Versioning

Every version in this contract is `0`. A change adds a version to exactly one
place, so a plugin states what it needs and a host states what it runs, each
per capability.

1. **Capabilities start at `0`.** A manifest lists the capabilities it uses in
   `requires`, each with version `0`. A capability version names one manifest
   section and, when the capability owns one, the runtime protocol that goes
   with it.
2. **A runtime protocol has no version of its own.** Its version is the owning
   capability's, and a host offers exactly that version
   (`{"versions": [<that version>]}`; the process answers `{"version": <that
   version>}`). The protocol versions today are `litheBuild` from
   `buildProviders`, `litheFunctions` from `backend`, `litheVcs` from
   `vcsProviders`, the toolchain provider protocol from `toolchainProviders`,
   `litheKeymapImport` from `keymapImportProviders`, and `litheLogging` from
   `pluginAPIVersion`. `litheBuild` and `litheToolchains` support `0` and `1`;
   all the others are `0`.
3. **A host declares a capability version when it can serve every form of
   it.** See [Host capabilities](#host-capabilities).
4. **A future change gives one capability version `1`.** The manifest section
   and the protocol move together, a plugin that uses the change lists
   `"<capability>": 1`, and every other plugin keeps `0` and stays installable
   wherever it was. An additive optional field in an existing section or
   message needs no new version.

`pluginAPIVersion` versions only the base fields: `schemaVersion`, `id`,
`displayName`, `version`, `vendor`, `permissions`, `activationEvents`,
`requires`, and `requiresTools`. It is always `0`; any other value is refused
with `plugin.api.unsupported` and `field: "pluginAPIVersion"`.

The other versioned documents start at `0` too:

| Document | Version field | Value |
| --- | --- | --- |
| Plugin manifest ([schema](plugin-manifest.schema.json)); it names the file format, and any other value is `plugin.schema.unsupported` | `schemaVersion` | `0` |
| Marketplace index ([schema](plugin-index.schema.json)) | `schemaVersion` | `0` |
| Canonical build documents (`litheBuildModel`, `litheDependencies`, `litheTestResults`, `litheTestDiscovery`, `litheRunEntrypoints`, `litheBuildConfiguration`, `litheConfigurationEdit`) and the `modelInputs` snapshot | `version` | `0` |
| `plugins.editorActions.decode` request | `version` | `0` (required; Core accepts only `0`, otherwise `Unsupported editor action validation version.`) |
| Plugin web bridge | `window.lithe.version` (`bridgeVersion`) | `0` |
| `backend` member of a launch plan | `version` | the manifest's `requires.backend` |

The `version` of an exported or called plugin function (1 to 1000) is not a
contract version: the plugin that exports the function owns it.

### Capabilities

```json
"pluginAPIVersion": 0,
"requires": { "buildProviders": 0, "buildSystems": 0, "languageServers": 0, "languages": 0 }
```

- `requires` is required and lists at least one capability. IDs are
  lowerCamelCase identifiers of at most 64 characters; versions are
  non-negative integers.
- Core reads `requires` before it decodes any section. A capability this
  contract does not define, or a version other than `0`, is
  `plugin.api.unsupported` with `field: "requires.<capability>"`, even when the
  manifest also holds a section Core does not know, so the user learns which
  capability needs a newer Lithe instead of seeing an unknown field.
  [Process start](#process-start) and
  [Executable resolution](#executable-resolution) belong to the base contract
  and are not capabilities: `requires.serverProcess` is refused like any
  unknown capability.
- `requires` lists exactly the capabilities the manifest uses. Leaving out a
  used one, or listing an unused one, is `plugin.manifest.invalid`. Any
  `buildSystems[].provider` requires `buildProviders`, whether it starts its
  own process or shares a language server. A template-only build plugin must
  omit `buildProviders`. Every VCS provider starts its own process and speaks
  `litheVcs`, so `vcsProviders` is its capability; `backend` is the capability
  of the plugin backend.
- The base fields change only with a new `pluginAPIVersion`.

Every capability has exactly one manifest section, and each runtime protocol
belongs to one capability:

| Capability | Manifest section | Runtime protocol | A host that declares it |
| --- | --- | --- | --- |
| `languages` | [`languages[]`](#languages), with TextMate [`grammar` and `configuration`](#grammars-and-editor-configuration) | — | Reads grammars and editor configuration from the package and applies them in the editor without starting a process. |
| `packageTrust` | `vendor.bundledSignatureRequirement`: origin-scoped trust for a host-sealed bundled copy of a publisher-signed plugin | — | Applies that trust rule to the bundled copy; see [Trust](#trust). |
| `languageServers` | [`languageServers[]`](#language-servers) apart from their launch fields, including [`workspaceConfiguration`](#workspace-configuration) and the [session fields](#plugin-owned-sessions) | — | Starts the server as [Process start](#process-start) says, delivers its workspace configuration, and serves its plugin-owned sessions. |
| `executionProviders` | [`executionProviders[]`](#execution-providers) | — | Runs and discovers tests through the declared commands. |
| `settings` | [`settings[]` and `settingActions[]`](#plugin-settings) with [placement, scope, effect and page buttons](#placement-scope-effect-and-page-buttons) | — | Shows the declared settings pages and applies their effects. |
| `ui` | [`ui`](#host-owned-workbench-ui): text, [web](#web-content) and [plugin view](#plugin-views) tool windows, views in [settings regions](#settings-regions), commands, and [backend-only commands](#backend-only-commands) | — | Renders every kind of window and command, serves the web bridge, and draws plugin views through `plugins.ui.applyResult`. |
| `buildSystems` | [`buildSystems[]`](#build-systems) without `provider`: detection, options, project model, tasks, run entries, tests, diagnostics, dependencies, sync, and [declarative preparation](#declarative-project-preparation) | — | Detects projects, loads the model, and runs the declared commands. |
| `buildProviders` | `buildSystems[].provider`, including own-process and shared language-server providers | `litheBuild` | Starts the provider and speaks `litheBuild` with workspace-owned sessions. |
| `runConfigurationTypes` | [`buildSystems[].runConfigurationTypes`](#run-configuration-types) | — | Lists the types in the Run/Debug Configurations dialog, draws their editors, and runs their saved configurations through `build.runConfigurationPlan`. |
| `beforeLaunchTasks` | [`buildSystems[].beforeLaunchTasks`](#before-launch-tasks) | — | Offers the tasks in the Add New Task menu of a configuration's Before Launch list, draws their forms, and runs their steps through `build.beforeLaunchTaskPlan`. |
| `buildModelInputs` | `buildSystems[].modelSharing` and `modelInputs`: [optional read-only model inputs](#optional-read-only-model-inputs) | — | Supplies the model snapshot between enabled workspace plugins. |
| `vcsProviders` | [`vcsProviders[]`](#vcs-providers) | `litheVcs` | Starts the provider and speaks `litheVcs`. |
| `debugAdapters` | [`debugAdapters[]`](#debug-adapters) | — | Starts the adapter and applies its [declared features and defaults](#adapter-features-and-defaults). |
| `outputLinkMatchers` | [`outputLinkMatchers[]`](#output-link-matchers) | — | Turns Run and Test console text into links through the declared patterns. |
| `projectGenerators` | [`projectGenerators[]`](#project-generators), including [`tool` fields](#tool-fields) | — | Lists the entries in the New Project window and generates the project. |
| `commandInstallers` | [`commandInstallers[]`](#command-installers) | — | Reports missing commands of `systemCommand` launches and installs them after the user confirms the command line, the network use and the target. |
| `managedRuntimes` | [`managedRuntimes`](#managed-runtimes) | Existing backend functions | Qualifies external and cached dependencies before committing an approved install/update, downloads only missing pinned artifacts, and repairs missing state before a server starts. |
| `backend` | [`backend`](#plugin-backend): the plugin's own long-lived process | `litheFunctions` | Starts the backend with or without a page, serves the host services its permissions grant, and speaks `litheFunctions`. |
| `toolDefinitions` | `toolDefinitions[]`: [tool identities, layouts, discovery and four-layer candidates](toolchains/README.md) | — | Resolves the definitions through the four layers. |
| `toolchainProviders` | `toolchainProviders[]`: [provider declarations and permission-bound answer validation](toolchains/README.md) | toolchain provider protocol (`litheToolchains`) | Starts the provider and validates its answers. |
| `pluginFunctions` | [`pluginFunctions`](#cross-plugin-calls): exported functions and declared calls between plugins | — (the relay methods are part of `litheFunctions`) | Relays calls between plugins. |
| `keymapImportProviders` | `keymapImportProviders[]`: [independent shortcut import providers](#shortcut-import-providers-core-foundation) | `litheKeymapImport` | Starts the provider and imports the shortcuts it reads. |

### Host capabilities

Validating a capability is not running it. Core validates every capability it
defines on every platform, but each host implements only some of them, so a
host states what it runs as `hostCapabilities: { "<capability>": [versions] }`
in `plugins.declarative.validate`, `plugins.marketplace.validateIndex`, and
`plugins.marketplace.installReview`:

- A host declares a capability version when it accepts every manifest form of
  that version and speaks its runtime protocol. Features a provider offers on
  top of that (tests, run preparation, commands) may go unpresented, and a
  plugin must not depend on them being called. Host services a plugin needs
  are not declared here: it discovers them at runtime with `getCapabilities`
  and its permissions.
- `validate` and `installReview` refuse a manifest whose `requires` names a
  version missing from `hostCapabilities` with `plugin.api.unsupported` and
  `field: "requires.<capability>"`. Without `hostCapabilities` they check only
  what Core supports.
- `validateIndex` offers a version only when the host and this Core build both
  support every entry of its `requires`; without `hostCapabilities` it offers
  none. A plugin that uses no build systems is therefore offered on a host
  that cannot run them.

Core validates the versions it supports without a host table, but admission
must use the actual host table; Core's protocol support alone does not enable a
capability on a host.

### Publishing

- A released capability version never changes meaning, and hosts keep
  supporting it.
- A plugin declares exactly the capabilities it uses, each at the lowest
  version that covers what it uses. Raising one without using what it adds only
  cuts off older hosts.
- A new capability version is added only for a change a host must not ignore.
  Plugins that do not use the change keep their version and stay installable
  wherever they were.
- Moving a plugin to a new capability version requires a new plugin version,
  and its index entry keeps the last version with the previous requirements so
  older hosts still have a version to install. Published packages are signed
  and are never rewritten.
- A host release that supports a capability version ships before any plugin
  version that declares it.
- The manifest holds only what a host must know before it starts a process:
  what to start, with which permissions, for which files and projects. What a
  running process can answer is negotiated over its own protocol (see
  [Build providers](#build-providers)), never declared in the manifest.

## Process start

The host starts every process a plugin declares: a language server, a build
provider with its own process, a VCS provider, a debug adapter and the plugin
backend. They share one set of launch fields, one launch-template language and
one launch plan. All of it belongs to the base contract (`pluginAPIVersion` 0):
no capability is declared to use it.

| `launch.kind` | Starts |
| --- | --- |
| `packaged` | An executable inside the package: `{path}`. |
| `systemCommand` | A bare command name the host resolves with its tool discovery: `{commands}`. |
| `runtime` | A package file run by a tool-definition's executable, shipped or discovered; see [Runtime launch](#runtime-launch). |
| `backend` | A language server whose command line the plugin backend answers at each start; see [Backend-resolved launch](#backend-resolved-launch). |

`arguments`, `validationArguments`, `runtimeDependencies`, and
`initializationOptions` mean what the [language server fields](#language-servers)
say. A tool is always named by [tool-definition ID](#tool-references): the host
has no list of runtimes it knows by name.

### Runtime launch

`{"kind": "runtime", "tool", "packagedRuntime"?, "minimumVersion"?,
"runtimeArguments"?, "entry"}` starts a file inside the package with a
runtime, so the package needs no launcher script. The runtime is either
shipped by the plugin (`packagedRuntime`) or discovered by the host.

| Field | Meaning |
| --- | --- |
| `tool` | A tool-definition ID that `requiresTools` lists; the definition decides how the entry is passed (see [Tool references](#tool-references)). |
| `packagedRuntime` | Package-relative path of the runtime executable the plugin ships, for example `runtime/${arch}/bin/node`; `${arch}` is the only variable it may use. The host starts exactly this file: it never looks the tool up on the machine, and no user override or other plugin can replace it. A plugin that needs a specific runtime owns it this way, so the host carries no knowledge of where that runtime lives. It excludes `minimumVersion`. |
| `minimumVersion` | Lowest acceptable version of a runtime the host discovers: one to three dot-separated numbers such as `21` or `18.17`. Only the components it names are compared, so `21` accepts every 21.x. A runtime whose version cannot be read does not qualify. |
| `runtimeArguments` | Launch templates placed before the entry, such as runtime options. |
| `entry` | Package-relative entry file. |

The launch tool is already a requirement and must not be repeated in
`runtimeDependencies`. A runtime launch declares no `validationArguments`: the
tool definition checks the version of the executable it resolves, and a
candidate that does not satisfy `minimumVersion` is not used, the same as a
candidate that fails a validation probe.

### Tool references

A launch names every runtime by [tool-definition ID](toolchains/README.md), so a
host never learns a tool from a name. The manifest lists every tool it starts a
process with in the top-level `requiresTools`, and the definitions come from
`toolDefinitions` of the plugin itself or of another enabled plugin.

| Field | Meaning |
| --- | --- |
| `requiresTools` | At most 32 tool-definition IDs, each short (`interpreter`) or plugin-qualified (`org.example.tools:interpreter`), without duplicates; stored sorted. |
| `launch.tool` of a `runtime` launch, `runtimeDependencies[].tool`, `runSelectedRuntime.tool` of a debug adapter | A tool-definition ID that `requiresTools` lists. An ID it does not list is `plugin.runtime.undeclared` with the `field` that names it. |
| `process.executable.tool` of a `lithe/host/run/start` request | A tool-definition ID that `requiresTools` lists. An ID it does not list is refused with `plugin.run.invalidStart` and `field: "process.executable.tool"`. |

The definition, not the host, decides how its executable starts an entry: its
`launch` template says where `runtimeArguments`, the entry and `arguments` go
(`["${executable}", "${runtimeArguments}", "${entry}", "${arguments}"]` runs
`<tool> <runtimeArguments> <entry> <arguments>`, a template without `${entry}`
runs no entry). The launch plan of a process therefore returns the parts, not a
command line:

| Field | Meaning |
| --- | --- |
| `launch` | `{kind: "tool", tool, entry, runtimeArguments, minimumVersion?, packagedRuntime?}`. `entry` is the absolute path of the package file, `runtimeArguments` are expanded, and `packagedRuntime` is the expanded path of a runtime the plugin ships (the host then uses that file as the tool's executable and never looks the tool up). |
| `arguments` | Only the plugin's own arguments; the host places them where the definition's `${arguments}` is. |
| `runtimeRequirements` | `{kind: "tool", tool, minimumVersion?}` for every tool the host must resolve. |

The host resolves each tool through the four layers of
[toolchains/README.md](toolchains/README.md), checks `minimumVersion` with
`toolchains.checkVersion`, which checks the version of the executable the
definition resolves, and expands the start with `toolchains.instantiateLaunch`.
A tool that resolves to no executable is `plugin.host.toolUnresolved`
(`details.reason` says why). See `fixtures/run/tool-reference-launch.plugin.json`.

### Launch templates

`arguments`, `validationArguments`, `runtimeArguments`, and the string values
of `initializationOptions` may use these variables. A package therefore never
contains a machine path, and the host decides where state lives.

| Variable | Value |
| --- | --- |
| `${pluginDir}` | Root of the installed, read-only package. It must be used at most once per value and be followed by a `/`-rooted path inside the package; that path may vary only by `${arch}`. |
| `${stateDir}` | Writable directory the host keeps for this plugin's server and removes when the plugin is uninstalled. Not available in `validationArguments`. |
| `${workspaceStateDir}` | Writable directory for the server's state of the current workspace. Not available in `validationArguments`, nor in a [plugin backend](#plugin-backend), which belongs to no workspace. |
| `${logDir}` | Absolute current-session `plugins/<id>/files/` directory, explicitly supplied by the host as `logDirectory`. Available for language servers and plugin backends, never in `validationArguments`. The host creates it in writable log storage, outside the installed package; Core does not derive it from package resources. |
| `${arch}` | Host CPU architecture: `arm64` or `x86_64`. |
| `${setting:<id>}` | Current value of a declared [plugin setting](#plugin-settings). Not available in `validationArguments`, and never inside a package path. |

`plugins.declarative.languageServerLaunchPlan` expands a server's launch. The
host selects the process with exactly one of `languageID` (a language server),
`buildSystemID` (a build provider), `vcsProviderID` (a VCS provider), and
`backend: true` (the [plugin backend](#plugin-backend)), passes `{pluginDirectory,
stateDirectory, workspaceStateDirectory?, logDirectory?, temporaryDirectory?, architecture,
settings?, projectCodeAllowed?}`, and receives. `workspaceStateDirectory` may be omitted only when the selected
process does not use `${workspaceStateDir}`; otherwise the request is
`plugin.launch.invalidRequest`. The same error applies when a template uses `${logDir}` but `logDirectory` is absent, empty, or not absolute.
`projectCodeAllowed` is the workspace's trust as Core reports it; absent means
untrusted, and a process that would run project code is then
`plugin.launch.untrustedWorkspace` (see [Workspace trust](#workspace-trust)).

| Field | Meaning |
| --- | --- |
| `owner` | `{pluginID, version}` of the installed package Core validated for this plan. The host starts the server for that exact installation and uses this identity, not one it read elsewhere, when it later relays requests or reports the session. |
| `launch` | `{kind: "packaged", path}`, `{kind: "systemCommand", commands}`, or `{kind: "tool", tool, entry, runtimeArguments, packagedRuntime?, minimumVersion?}`: what the host resolves to an executable. A `runtime` launch returns the tool shape of [Tool references](#tool-references). A [backend-resolved launch](#backend-resolved-launch) returns `{kind: "backend", function, timeoutMilliseconds}` here, and `{kind: "tool", tool}` appears in the plan `plugins.languageServers.decodeLaunch` returns. |
| `arguments` | Argument list after the executable. A tool-shaped launch returns only the plugin's own arguments. |
| `validationArguments` | Expanded probe arguments. |
| `runtimeRequirements` | `{kind: "tool", tool, minimumVersion?}` for every tool the host must resolve through [toolchains](toolchains/README.md), a launch tool included, sorted by tool. A packaged runtime is not listed. Only the launch tool can carry a minimum version. |
| `initializationOptions` | Expanded options; absent when the server declares none. |
| `requiredDirectories` | `state`, `workspaceState`, `log`, and/or `temporary`: the directories the host must create before starting the server. |
| `requiredPackagePaths` | Sorted package-relative paths the launch points at. The host verifies each is inside the package and not a symbolic link before starting the server. |
| `settings` | Effective value of every declared setting that produced this plan; absent when the plugin declares none. |
| `resolveParams` | Present only for a backend-resolved launch: the params of the launch function, without the `context`, optional `tools`, and optional read-only `runtimes` map the host adds. |
| `environment`, `writablePaths` | Present only in the plan `decodeLaunch` returns: the variables of the process, and the paths it writes. |
| `sessionPolicy` | Present for a language server that declares session fields: the [session policy](#plugin-owned-sessions) the host passes to `lsp.startServer` unchanged. |
| `buildProvider` | Present only for a `buildSystemID` selection: `{buildSystemID, executesProjectCode, mayUseNetwork, languageServer?}`. `languageServer` names the language whose server is the provider; the rest of the plan is then that server's plan, and the host reuses its running session instead of starting a second process. |
| `vcsProvider` | Present only for a `vcsProviderID` selection: the [provider descriptor](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/contracts/vcs-provider-v1.md#provider-descriptor) the host keeps for the session. |
| `backend` | Present only for a `backend` selection: `{hostServices, version}`, the sorted [host services](#host-services) the plugin's permissions grant, plus `version`, the manifest's `requires.backend` (`0`). The list holds service names (`workspace`, `credentials`) and the full names of methods a permission grants one by one, such as `lithe/host/workspace/getHostPaths`. The host passes it to `lsp.startServer.hostServices` without the method names it registered no handler for; a backend calling one of those gets `plugin.host.permissionDenied`. |

A language without a server, an undeclared build system, a build system
without a provider, an undeclared VCS provider, and a plugin without a
backend are `plugin.launch.unavailable`. Core never creates or
reads any of these paths.

In a workspace the user has not trusted, a language server, a build provider
with a process of its own, and a VCS provider are
`plugin.launch.untrustedWorkspace` unless the declaration says the process runs
no project code; [Workspace trust](#workspace-trust) lists which. The plugin
backend and a debug adapter with a process of its own are not refused: they
run plugin code, and the requests that would run the project are refused on
their own.

### Backend-resolved launch

A server whose command line depends on what the plugin finds when it starts
(a version it resolves, a tool it bootstraps, options it derives from the
project) lets the plugin backend answer the command line each time instead of
declaring one:

```json
"launch": {"kind": "backend", "function": "launch.resolve", "timeoutMilliseconds": 30000}
```

- Only a language server can use it. The plugin declares a `backend`.
  `function` is a plugin function ID. `timeoutMilliseconds` is 1000 to 120000
  and defaults to 30000.
- The server declares none of `arguments`, `validationArguments`,
  `runtimeDependencies`, `initializationOptions`, or `hostIntegration`: the
  answer carries the first four, and a host integration is host code, which
  this launch exists to avoid. `workspaceConfiguration` stays declarative
  unless the answer carries its own.
- `plugins.declarative.languageServerLaunchPlan` for such a server returns the
  first step, not a command line: `launch: {kind: "backend", function,
  timeoutMilliseconds}`, `resolveParams`, `requiredDirectories` (always
  `state`, `workspaceState`, `log`, and `temporary`), `settings`, and
  `sessionPolicy`. Its request adds `temporaryDirectory`, an absolute scratch
  directory of this start; the launch needs all four directories, so a request
  without one is `plugin.launch.invalidRequest`.
- The host creates the directories, starts the plugin backend if it is not
  running, and calls `function` through `lithe/function/invoke` with
  `resolveParams` and the workspace `context`. A host may also send `tools`,
  `{<toolID>: {executable, home, version}}`, the resolution of each tool the
  manifest lists in `requiresTools` that it could find; Core does not
  validate it. The host resolves each required tool as an installation
  with the user's choice and discovery, and leaves out a tool that is
  missing, unusable, or inside the project; `home` and `version` are present
  when the installation has them. A function must not require `tools` or any
  entry in it: it answers `{kind: "tool"}` and lets the host resolve the
  tool, and treats a missing entry as "configure nothing". A backend uses
  `tools` to point its server at the same installation the host runs, for
  example as the default runtime of a language server whose Run goes through
  the tool. A failed or timed-out call, or an answer for a context whose
  generation has changed, ends this start without a process.
- The host passes `{manifest, languageID, request, result}` to
  `plugins.languageServers.decodeLaunch`, which returns `{status: "ok", plan}`
  or a rejection. The plan is an ordinary launch plan (`launch` is `packaged`
  or `tool`) whose `sessionPolicy` also carries the answer's `gates`; the host
  starts it like any other. A server that is not launched by the backend is
  `plugin.launch.notBackend`, so a host never runs an answer it did not ask
  for.
- When an `immediate` setting changes while the server runs and its answer
  carried `workspaceConfiguration`, the host calls `function` again with the
  running start's directories, the workspace `context` and the new
  `settings`, decodes the answer the same way, and sends only the plan's
  `workspaceConfiguration` with `lsp.updateWorkspaceConfiguration`. The rest
  of the answer waits for the next start. A function whose answer carries
  `workspaceConfiguration` is therefore called more than once per start; one
  that installs or downloads does so only when something is missing. When the
  answer has no `workspaceConfiguration`, the host sends the declaration's
  expansion and does not call the function again.

`resolveParams`:

| Field | Meaning |
| --- | --- |
| `serverID` | The server's `languageID`; also its ID in the `languageServer` host methods. |
| `platform` | `{os, arch}`: the host operating system and CPU architecture (`arm64` or `x86_64`). |
| `stateDirectory`, `workspaceStateDirectory`, `logDirectory`, `temporaryDirectory` | The writable directories of this start, absolute and outside the package and the application bundle. |
| `settings` | The effective value of every declared plugin setting. |

The answer is an object with these fields; an unknown field is refused.

| Field | Meaning |
| --- | --- |
| `executable` | `{kind: "packaged", path}` is a file inside the plugin package, read-only, listed in the plan's `requiredPackagePaths`. `{kind: "tool", tool}` names a tool definition of the plugin; the host resolves it like any tool launch. |
| `arguments` | At most 1024 strings of at most 32 KiB, without NUL. |
| `environment` | At most 256 variables; a name is non-empty without `=`, a value is at most 32 KiB. |
| `initializationOptions`, `workspaceConfiguration` | JSON objects of at most 1 MiB each. |
| `gates` | At most 8; see [Plugin-owned sessions](#plugin-owned-sessions). |
| `writablePaths` | At most 32 paths the server writes. Each lies inside one of the four directories; `.` and `..` components are refused. The plan carries them sorted, and the host creates their parent directories before the start. |

Rejections: `plugin.launch.invalidResult` for a malformed or oversized answer
or gate, `plugin.launch.outsidePackage` for a packaged path that leaves the
package, `plugin.launch.undeclaredTool` for a tool the plugin does not
define, `plugin.launch.unwritablePath` for a writable path outside the host
directories, `plugin.launch.invalidRequest` for a request without a usable
directory, `plugin.launch.unavailable` for a language without a server, and
`plugin.launch.notBackend`. See `fixtures/languages/language-server-backend-launch.plugin.json`
and `fixtures/languages/language-server-launch-resolution.json`.

Limits the Lithe desktop host applies today: a `{kind: "tool"}` answer is resolved
with the user's tool choice and discovery, and a program that lies inside the
project is refused, because opening a project must not run it; a server whose
launch the backend resolves does not start in a remote workspace; and the
scratch `temporaryDirectory` is removed when the server process ends.

## Executable resolution

Declare executable sources in `invocation.executables`, in preference order.
Array order is the recommended form; no custom path setting is required for a
plugin that only needs automatic discovery. The source kinds are
`projectFile`, `environment`, `command`, `runtime`, `setting` and `toolchain`.
Executable resolution belongs to the base contract: no capability is declared
to use it. `setting` and the ordering metadata are described below:

```json
"executables": [
  { "id": "selected", "kind": "setting", "settingID": "toolPath" },
  { "id": "wrapper", "kind": "projectFile", "path": "mvnw" },
  { "id": "system", "kind": "command", "command": "mvn" }
]
```

A `setting` references a declared **global file** setting with `effect: "reload"`
or `"nextRun"`. Core receives raw `request.settings`. An absent
value uses the declared default; an explicit empty string emits a setting candidate
with `path: null`, which skips. A nonempty value must satisfy the setting rules,
be absolute, and have no surrounding whitespace or controls. Invalid stored
values reject the invocation instead of invoking the normal settings display
resolver's fallback-to-default behavior. Project overrides never select tools.
The host checks file permissions and versions: a configured failure is terminal,
including a failure to probe, and must not fall back to subsequent candidates or
a Provider's own lookup. Paths with spaces remain single paths. The selected
setting's directory precedes PATH. Runtime candidates construct their environment
before version probing and reuse it for execution; a rejected candidate's
environment does not leak into the next choice. Provider hints and task/test
execution use this same plan/selection mechanism. If resolution fails before a
configured candidate is reached (for example, an earlier Runtime is missing),
any nonempty setting in the tool or Runtime candidate sets still forbids the
Provider's independent lookup. All-empty settings retain automatic-discovery
fallback. This check applies after failure, so an earlier successful candidate
is unaffected by a later unused setting.

Optional metadata applies within **one candidate set owned by one plugin**:

- `id` is a unique stable identifier; anonymous candidates cannot be anchors.
- `priority` is a signed 32-bit integer; higher wins, absent means 0. Ties retain
  original array order.
- `insertBefore` or `insertAfter` names an ID in this set. One candidate cannot
  combine either with `priority`, or declare both insertion fields. Constraints
  mean relative order, not immediate adjacency.
- Normalize the complete set into a directed graph before platform filtering.
  Repeatedly select a zero-indegree node by descending priority, then original
  index; emit it and remove its edges. Explicit edges take precedence over
  priority. Duplicate/invalid IDs, unknown anchors, cycles/self-edges and
  conflicting metadata are manifest errors. Filter platforms only after sorting,
  so anchors remain valid even if they do not apply to the current platform.

`lithe_plugin_model::executables` owns the reusable source types and deterministic order;
`ExecutableCandidateResolver` receives file-system, Setting (already validated by
Core), Environment, Runtime and command-discovery capabilities from the host.
Results distinguish `skip`, `resolved` and `terminalFailure`; plugins declare data
and cannot inject host native code. Each operation uses a value snapshot for its
settings/environment and ordered plan. Passive import does not resolve/probe
runtimes or execute project wrappers. A project candidate remains project code
and is only executed through the consumer's existing trust/user-action workflow.

Executable candidates are used by build systems and their custom runtime
dependencies. Own build providers adapt command/runtime launches through
`BuildToolResolver` and receive the selected build tool as `tool.executable`;
their process protocol is unchanged. Language servers, execution providers,
and other launch forms name a [tool definition](#tool-references) instead of
declaring a candidate list. Any launch form adapted to this resolver must
preserve the package-realpath/executable checks, the
[workspace trust](#workspace-trust) of the operation, auto-start restrictions,
runtime ownership and cancellation.

See [`build-tool-path.plugin.json`](fixtures/build-systems/build-tool-path.plugin.json).

A candidate may use
`{ "kind": "toolchain", "toolchainID": "org.example.compiler:compiler" }`.
Core accepts the same short/qualified ID grammar as tool definitions and adds the
normalized instance `root`. The host adapter resolves installed definitions
against fresh native evidence and directory pins. Unavailable selections are terminal,
including when a prerequisite runtime fails first; they cannot silently fall
back to Provider discovery.
A runtime dependency that declares no `executables` is already this candidate
for the definition with its own ID, and that definition checks
`minimumVersion`. Core knows no runtime by name: `node` and `go` are tool
definitions like any other. A tool that needs another tool as a
prerequisite names that tool's definition; Core adds no registry entry for any
particular tool.
Passive import performs no version or runtime probes; a definition that requires
verification is not reported as an already verified import executable.
A toolchain that resolves to a project file remains project code,
including through a symlink, and native configuration requires explicit Read
before running it. See [`build-toolchains.plugin.json`](fixtures/build-systems/build-toolchains.plugin.json).

## Command installers

A [`systemCommand` launch](#process-start) names a bare command that the host
finds with its tool discovery. When the machine lacks it, `commandInstallers[]`
tells the host how to install it, so the user installs it from plugin
management instead of a terminal. An installer either runs a tool the plugin
already requires (a **tool installer**), or, with `commandInstallers: 1`,
downloads a pinned archive (a **download installer**). The section requires
`commandInstallers: 0`, or `1` when any installer declares `download`, and the
`commands.install` permission.

```json
"requiresTools": ["go"],
"commandInstallers": [
  {
    "id": "dlv",
    "commands": ["dlv"],
    "tool": "go",
    "minimumVersion": "1.22",
    "arguments": ["install", "github.com/go-delve/delve/cmd/dlv@latest"],
    "mayUseNetwork": true,
    "targets": {
      "managed": { "environment": { "GOBIN": "${installDirectory}" } },
      "toolDefault": {
        "directory": [
          { "arguments": ["env", "GOBIN"] },
          { "arguments": ["env", "GOPATH"], "pathList": true, "append": "bin" }
        ]
      }
    }
  }
]
```

| Field | Meaning |
| --- | --- |
| `id` | Lowercase letters, digits and `-`, at most 64 characters; unique within the plugin. |
| `commands` | 1 to 8 bare command names the installation provides. Each one is named by a `systemCommand` launch of this manifest (a language server, build provider, VCS provider, debug adapter or the backend) and by no other installer. |
| `tool` | Tool installer: the tool-definition ID, listed in `requiresTools`, whose executable runs the installation and the directory probes. |
| `minimumVersion` | Tool installer, optional: lowest version of `tool`, one to three dotted numbers. |
| `arguments` | Tool installer: arguments after the tool's executable, used by a target that does not replace them. Literal: no template variables. |
| `download` | Download installer: the pinned archives, see [Download installers](#download-installers). Excludes `tool`, `minimumVersion` and `arguments`. |
| `mayUseNetwork` | Required: whether the installation may download. The host shows it before the user confirms. Always `true` for a download installer. |
| `timeoutSeconds` | How long the host lets the installation run, 1 to 3600; 600 when absent. A download installation counts the download and the extraction. |
| `targets` | Where the commands may land: `managed`, `toolDefault`, or both. At least one. A download installer has only `managed`. |

The user chooses the target when installing:

- **`managed`** installs into a directory the host owns for the plugin, in its
  application data and never in the app bundle, the installation directory or
  the plugin package; the host removes it with the plugin. For a tool
  installer, `arguments` (optional, replaces the installer's) and
  `environment` (variables set over the tool's environment) may use
  `${installDirectory}`, the only template variable of this section, and
  together must use it at least once. `commandDirectory` is a package-style
  relative path, below the install directory, of the directory the commands
  land in; the install directory itself when absent.
- **`toolDefault`** installs wherever the tool installs when it is told
  nothing. `arguments` and `environment` are literal. `directory` lists up to
  4 probes, tried in order: runs of the same tool with non-empty literal
  `arguments` whose standard output names the directory the commands land in.
  With `pathList: true` the first non-empty entry of a list in the host's path
  separator counts; `append` is a package-style relative path added to the
  printed directory. Without probes the host can say only that the tool
  decides.

At most 16 installers, 64 arguments per list, 16 environment variables per
target, and 4096 bytes per argument or value. An environment name is uppercase
letters, digits and `_`, not starting with a digit. Only `systemCommand`
launches are served: a build system's or execution provider's executable
resolved through `invocation.executables` is not.

### Download installers

A download installer serves a command its upstream publishes as a prebuilt
archive, such as a debug adapter, so that the plugin package does not carry
it and the user downloads it only when it is missing:

```json
"commandInstallers": [
  {
    "id": "codelldb",
    "commands": ["codelldb"],
    "download": {
      "version": "1.12.3",
      "artifacts": [
        {
          "os": "macos",
          "arch": "arm64",
          "url": "https://github.com/vadimcn/codelldb/releases/download/v1.12.3/codelldb-darwin-arm64.vsix",
          "sha256": "2f114a990e1b368dd1dbd33c80c0e719767af2d228391ec0df0571c957f9ac91",
          "archive": "zip"
        }
      ]
    },
    "mayUseNetwork": true,
    "timeoutSeconds": 900,
    "targets": { "managed": { "commandDirectory": "extension/adapter" } }
  }
]
```

| Field | Meaning |
| --- | --- |
| `download.version` | The upstream version of the archives, 1 to 128 bytes without surrounding whitespace or control characters. The host shows it before the user confirms. |
| `download.artifacts` | 1 to 6 archives, at most one per `os` (`macos`, `windows`, `linux`) and `arch` (`arm64`, `x86_64`). Sorted by `os`, then `arch`, when normalized. |
| `artifacts[].url` | HTTPS URL with a host, without credentials or a fragment, at most 4096 bytes. |
| `artifacts[].mirrors` | Optional: up to 8 fallback HTTPS URLs serving the same bytes, each different from `url` and from each other. |
| `artifacts[].sha256` | SHA-256 of the whole archive, 64 hex digits; lowercased when normalized. |
| `artifacts[].archive` | `zip` or `tar.gz`. |
| `artifacts[].stripPrefix` | Optional directory every entry of the archive is below, removed when extracting; empty when absent. |

The archive and layout rules are those of [managed runtimes](#managed-runtimes):
`stripPrefix` and `targets.managed.commandDirectory` are empty or
`/`-separated relative paths without an empty, `.` or `..` part, backslash or
colon, at most 512 bytes. `commandDirectory` is below the extracted root, after
`stripPrefix` is removed. `targets.managed` passes no `arguments` and sets no
`environment`, because nothing runs. A platform without an archive is not
offered the installer: its commands have no installer there.

The host downloads the archive for its platform, trying `url` and then the
mirrors, checks the SHA-256 before extracting, and extracts into a directory
of its own below the plugin's managed directory, never into the plugin package
or the app bundle. It keeps the entries' modes, so an executable in the
archive stays executable, and refuses an archive whose entries leave the
extraction root, whose links point outside it, or that is not below
`stripPrefix`. One extraction is kept per `download.cacheKey` of the plan, so
updating the plugin without changing the archive reuses it.

### Installation steps

The host never installs on its own:

1. `plugins.commandInstallers.requirements` accepts `{manifest, platform?}`
   and lists each `systemCommand` launch with its commands and their
   installer, `null` when none provides it, and the installers offered. Each
   installer entry has `id`, `commands`, `tool`, `minimumVersion`,
   `download` (`null` for a tool installer, `{version}` for a download
   installer), `mayUseNetwork` and `targets`. With `platform: {os, arch}` a
   download installer without an archive for that platform is left out; a
   host that installs downloads always sends it. The host looks each command
   up as the launch does and offers the installer of a missing one.
2. For `toolDefault`, the host runs each probe with the resolved `tool` and
   sends their outputs to `plugins.commandInstallers.defaultDirectory`, which
   answers the directory, or `null` when no probe named an absolute one.
3. The host shows the tool and the expanded command line, or the version and
   the archive URL, whether it uses the network and the directory of each
   target, and lets the user choose one.
4. `plugins.commandInstallers.plan` accepts
   `{manifest, installerID, target, installDirectory?, platform?}` and expands
   the chosen target. Every plan has `installer`, `target`, `commands`,
   `tool`, `minimumVersion`, `mayUseNetwork`, `timeoutMilliseconds`,
   `arguments`, `environment`, `commandDirectory`, `directoryProbes` and
   `download`.
   - For a tool installer `download` is `null`. A `managed` plan needs the
     absolute `installDirectory` the host made; a `toolDefault` plan takes
     none. The host runs `tool` with `arguments` and `environment` under
     `timeoutMilliseconds`, shows the output, lets the user cancel, and then
     checks that every command is in `commandDirectory` (`managed`) or in the
     default directory (`toolDefault`).
   - For a download installer the plan needs `platform` and takes no
     `installDirectory`; `arguments` and `directoryProbes` are empty,
     `environment` is empty and `commandDirectory` is `null`. `download` is
     the archive for the platform: `installer`, `version`, `os`, `arch`,
     `url`, `sha256`, `archive`, `stripPrefix`, `cacheKey`,
     `commandDirectory` (relative to the extracted root, empty for the root)
     and `mirrors` when the archive has any. `cacheKey` is an opaque
     64-character lowercase SHA-256 of the installer ID, version, platform,
     URL, digest, format and `stripPrefix`; mirrors are left out of it.
     The host downloads, checks and extracts under `timeoutMilliseconds`,
     lets the user cancel, and then checks that every command is in
     `download.commandDirectory` below the extracted root.
5. Later launches of the plugin find the commands there: the host adds the
   plugin's managed command directories and the default directories it
   recorded to the lookup of that plugin's `systemCommand` launches.

Every installer is a program of the [install review](#trust-and-install-review)
(`kind: "commandInstaller"` with its fields), so a change to what it runs is
approved again. A download installer's program lists `download` with every
archive instead of `tool`, `minimumVersion` and `arguments`, so a new URL,
mirror, digest or version is approved again too. A host that declares only
`commandInstallers: 0` refuses a plugin with a download installer before
installing it. See
[`command-installer.plugin.json`](fixtures/command-installers/command-installer.plugin.json),
[`command-installer-download.plugin.json`](fixtures/command-installers/command-installer-download.plugin.json)
and the golden cases in
[`command-installer-plans.json`](fixtures/command-installers/command-installer-plans.json).

## Managed runtimes

`managedRuntimes: 0` lets a plugin qualify existing native installations and ask
the host to install only its missing, pinned dependencies. Dependencies live
outside the immutable plugin package and its versioned state directory. Their
identity is independent of `manifest.version`, so an update that changes only
plugin code reuses an already qualified runtime without another download.
No dependency path changes the user's project compiler or global tool selection.

```json
"requires": {"backend": 0, "managedRuntimes": 0},
"permissions": ["backend.launch", "runtimes.install"],
"managedRuntimes": {
  "resolve": {"function": "runtime.resolve", "timeoutMilliseconds": 30000},
  "dependencies": [{
    "id": "engine", "version": "4.2.0",
    "artifacts": [{
      "os": "linux", "arch": "x86_64",
      "url": "https://example.invalid/engine-4.2.0.tar.gz",
      "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "archive": "tar.gz", "stripPrefix": "engine-4.2.0", "home": ""
    }]
  }]
}
```

The illustration's digest and domain are inert examples, not downloadable assets.
The backend and any runtime needed to execute that backend must be packaged
(`packaged`, or `runtime` with `packagedRuntime`, without external runtime
dependencies). This prevents a bootstrap cycle: the resolver runs before the
dependency set or the new package version is committed. Declaring the resolver
is itself a backend activation trigger; it needs no workspace or UI contribution.

| Field | Contract |
| --- | --- |
| `resolve` | Required `{function, timeoutMilliseconds?}`. A private backend function ID; timeout 100–120000 milliseconds, default 30000. It uses the existing `litheFunctions` protocol and requires no export. |
| `dependencies` | 1–16 entries, sorted by `id` on normalization. IDs are unique lowercase letters, digits and hyphens, at most 64 bytes. |
| `version` | Required nonempty opaque upstream identity, at most 128 bytes, without controls or surrounding whitespace. It is not the plugin version and is not compared as semver by Core. |
| `tool` | Optional unique tool binding. The plugin must own that `toolDefinitions` ID and list it in `requiresTools`. It is available only in this plugin's language-server session. |
| `artifacts` | 1–6 entries, one per distinct exact `os`/`arch` pair. OS is `macos`, `windows` or `linux`; architecture is `arm64` or `x86_64`. No target fallback or wildcard. |
| `url`, `sha256`, `archive` | HTTPS URL without credentials or fragments, at most 4096 bytes; complete SHA-256 normalized to lowercase; explicit `zip` or `tar.gz`. The host verifies bytes before extracting. |
| `stripPrefix` | Exact archive root removed by extraction, default empty. |
| `home` | Relative to the stripped extraction root, default empty for the root itself. |
| `executable` | Relative to `home`; required and nonempty exactly when `tool` is present. The host derives the executable from the qualified home and this path. |

The three layout paths are empty or portable `/`-separated relative paths of at
most 512 bytes, with no empty, `.` or `..` component, backslash, colon, control
character or surrounding whitespace. They contain no template variables. Native
extraction also rejects escaping links, duplicate/aliased entries, oversized
archives and nonregular payload types; Core does not inspect a filesystem.
The desktop extraction budgets are 512 MiB compressed, 4 GiB expanded, 50,000
entries and a 32 MiB completion inventory. Permission bits are preserved where
the native filesystem supports them, and safe archive links are created last.

### Runtime qualification

The host calls the resolver with this typed input:

```json
{
  "platform": {"os": "linux", "arch": "x86_64"},
  "dependencies": [{"id": "engine", "version": "4.2.0", "os": "linux", "arch": "x86_64", "url": "https://example.invalid/engine.tar.gz", "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "archive": "tar.gz", "stripPrefix": "engine", "home": "", "executable": "", "cacheKey": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}],
  "candidates": {"engine": "/host/managed/engine"},
  "tools": {},
  "settings": {}
}
```

`dependencies` is the selected plan from Core, including an optional `tool`.
`candidates` maps dependency IDs to existing or newly staged managed homes;
the hook can be called repeatedly during preparation. `tools`, when available,
maps tool IDs to `{executable, home, version}` discovered by the host, with
optional `provider` and `layer` facts copied unchanged from Core's toolchain
resolution. The plugin can use those facts to distinguish an explicit selection
from automatic discovery without duplicating selection policy in the host. Settings
are the effective declared plugin settings. Optional `language` is the host UI's
BCP 47 language tag (for example `en-US` or `zh-Hans`) for localized qualification
errors; plugins fall back to English when it is absent or unsupported.
No workspace is required for install
preflight; no candidate or external home is a writable state directory.

The plugin owns compatibility: it inspects required files, probes the actual
runtime, checks extension compatibility and rejects broken explicit settings.
Explicit user paths take precedence. Otherwise it tries qualified candidates,
then eligible external installations, and reports only unmet dependencies as
missing. A configured but unusable explicit path is an error, never a request
to silently download a replacement. The host must not implement language names,
version rules, layout probes or discovery heuristics for a particular plugin.

The response has exactly `runtimes` and `missing`:

```json
{"runtimes": [{"id": "engine", "home": "/opt/engine", "version": "4.2.1"}], "missing": []}
```

Every declared dependency occurs exactly once across the two arrays. Unknown or
duplicate IDs are rejected. A returned home is an absolute path for the requested
native OS, at most 4096 bytes, without controls or dot/parent components; version
is a bounded nonempty identity under the same rules as the declaration. Core
validates structure and identity; the plugin qualifies compatibility and the host
checks native evidence. The host never writes into an external home.

### Runtime preparation, approval and reuse

Installation review includes `runtimes.install`, explaining network downloads
and installation of executable dependencies, plus one program of kind
`managedRuntime` per dependency. That program contains the complete normalized
dependency, all platform URLs and hashes, and the resolver declaration. Changing
any of them is a changed reviewed program. Package content integrity binds this
manifest as it does other program declarations. Preparation starts only after
the normal installation/update review succeeds.

The native preparation backend starts with no host-service grants and
`lsp.startServer.functionRelayEnabled: false`. This optional native flag defaults
to true for existing sessions. False excludes the temporary backend from both
incoming and outgoing cross-plugin routing, even if an installed backend shares
its plugin ID. Direct host-to-session resolver calls, capability discovery and
structured logging still work. The preparation process is stopped and disposed
before its private state is removed; it never replaces the normal backend in
the host's installed-session registry.

The host qualifies external/cached homes before downloading. It downloads only
missing dependencies into private staging, verifies pinned content and safe
extraction, and calls the resolver again with staged homes. It serializes cache
publication across windows and processes. The complete newly qualified set is
published before the approved package installation is committed; failures or
cancellation retain the previously usable package and set. Existing sessions
keep their selected homes. A failed dependency cannot produce a partially active
set or launch the new language server. The host can retry this preparation when
a declared managed dependency is absent or damaged before launch.

Storage is under a host-managed plugin directory independent of plugin version.
`cacheKey` is an opaque 64-character lowercase SHA-256 of Core's normalized
selected dependency identity (including platform, upstream version, URL, digest
and layout), excluding plugin version. Hosts use that key without reconstructing
it. Cached content must pass native integrity checks and plugin qualification;
existence alone is insufficient. A verified unchanged set works offline. A new
or damaged dependency that cannot be downloaded produces an actionable failure;
it does not destroy the prior set. Updating a plugin never clears all runtime
caches. Removal/reclamation must not remove a home used by a live session.

After preparation the host adds optional `runtimes: {<id>: {home, version}}`
to a backend-resolved language server's `resolveParams`. It is read-only launch
information, not a writable directory grant. A tool binding supplies
`tools[tool]` and the corresponding `{kind: "tool", tool}` launch from the
qualified home in that session only. Global toolchain catalogs, project pins and
project runtime selection remain independent. There is no absolute executable
answer kind and no runtime directory template variable.

### Managed runtime Core commands

`plugins.managedRuntimes.plan` accepts `{manifest, platform:{os,arch}}` and
returns `{status:"ok", plan:{resolve, dependencies:[...]}}`. Each selected
dependency contains `id`, `version`, optional `tool`, `os`, `arch`, `url`,
`sha256`, `archive`, `stripPrefix`, `home`, `executable` and `cacheKey`.
Without a declaration it returns `plan:{dependencies:[]}` without `resolve`.

`plugins.managedRuntimes.decodeResolution` accepts
`{manifest, platform:{os,arch}, result}` and returns
`{status:"ok", resolution:{runtimes,missing}}`, sorted by dependency ID.
Both commands revalidate the manifest and perform no native I/O. A target without
all required artifacts is `plugin.managedRuntime.unavailable`; invalid resolver
output is `plugin.managedRuntime.invalidResolution`; malformed declarations use
`plugin.manifest.invalid`. Malformed command envelopes use `invalid_request`.
The shared inert fixture is `rust/fixtures/plugins/managed-runtimes.json`.
Older hosts that do not advertise this capability reject
`requires.managedRuntimes` precisely, while existing manifests are unchanged.

## Permissions and activation

| Permission | Required when |
| --- | --- |
| `languageServer.launch` | `languageServers` or `vcsProviders` is not empty, or a build provider starts its own process |
| `process.run` | any provider declares `run` |
| `process.test` | any provider declares `test` |
| `process.build` | `buildSystems` is not empty |
| `process.vcs` | `vcsProviders` is not empty |
| `debug.launch` | `debugAdapters` is not empty |
| `debug.attach` | an adapter lists `attach` in `requests` (and only then); it also covers attaching to a remote host and port |
| `project.generate` | `projectGenerators` is not empty |
| `commands.install` | `commandInstallers` is not empty (and only then); permits running the declared installations after the user confirms each, nothing else |
| `runtimes.install` | `managedRuntimes` is present (and only then); permits the host to download and install its pinned archives after normal plugin install/update review, and repair their managed storage |
| `ui.contribute` | `ui` is declared |
| `ui.web` | a tool window shows [web content](#web-content) |
| `backend.launch` | `backend` is declared |
| `keymaps.provide` | `keymapImportProviders` is nonempty; permits offering mappings, not reading files by itself |
| `keymaps.readSelectedSources` | `keymapImportProviders` is nonempty; permits reading only declared files in an explicitly selected import source |
| `credentials.store` | the backend uses the [credentials host service](#host-services); refused without a `backend` |
| `workspace.metadata` | the backend uses the [workspace host service](#workspace-contexts-and-capability-discovery); refused without `backend` |
| `editor.actions` | the backend registers independent editor actions; requires `backend` and `documents.read`, grants only `editor/registerActions`, not document edits or domain execution |
| `editor.providers` | the backend supplies [editor language features](#editor-language-feature-providers) for matching files; requires `backend` and `documents.read`, grants only `editor/registerProviders`, not document edits |
| `terminal.create` | the backend creates and manages its own [native terminals](#plugin-owned-native-terminals), including bounded display history for reopen; requires `backend` |
| `run.start` | the backend starts and stops host-owned [Run sessions](#run-and-debug-sessions); grants `lithe/host/run/start`, `lithe/host/run/stop` and the `lithe/host/run/didExit` notification; requires `backend` |
| `debug.start` | the backend starts a [debug session](#run-and-debug-sessions) on one of its own adapters; grants only `lithe/host/debug/start`; requires `backend` and a declared `debugAdapters` entry |
| `languageServer.request` | a language server lists `requestMethods` or `forwardNotifications`, and only then; requires `backend`. Grants the [`languageServer` host methods](#language-server-host-methods) for the plugin's own servers and their two notifications, nothing else |
| `documents.contentProvider` | the backend registers a [content provider](#content-providers); requires `backend`. Grants `documents/registerContentProvider` only, not reading or editing documents |

| Activation event | Meaning |
| --- | --- |
| `onLanguage:<id>` | Start the language server when a document of the language opens in a project. |
| `onRun:<id>` | Activate the run provider when the user runs a file of the language. |
| `onTest:<id>` | Activate the test provider when the user lists or runs tests. |
| `onBuildSystem:<id>` | Activate the build system before its first plan, model, or decode. |
| `onVcsRoot:<id>` | Start the VCS provider once a root of its VCS is detected in the workspace. |
| `onDebug:<id>` | Start the debug adapter when a debug session needs it. |
| `onWorkspaceOpen` | Start the [`backend`](#backend-lifecycle-and-workspace-contexts) process once a workspace is open. Optional, names no ID, refused without `backend`. |

Each provider needs its event and each event needs a provider; the only
optional event is `onWorkspaceOpen`. There is no event that starts a plugin
on every launch (`onStartupFinished`, `*`): it is
`plugin.activation.invalid`. Enabling a
plugin only permits activation; nothing starts until its event occurs.

## Trust

- `publisherPackage`: the package must carry a valid publisher signature
  (`lithe-plugin-signature.json`, Ed25519 over every file) from a key the host
  trusts. Development builds may install an unsigned package from disk as a
  local development package; release builds refuse it.
- `sameTeamAsHost`: only honored for a package shipped inside the host's own
  signed installation, whose code signature already seals it.
- `none`: a community package without a required Lithe signature. A host may
  install it from a third-party marketplace source after matching the archive
  against the index checksum, or from a user-selected local ZIP or unpacked
  directory after taking a private snapshot. Both paths require the user to
  confirm the install review. IDs in the official namespace (`dev.lithe`,
  `dev.lithe.*`) may not use it (`plugin.manifest.invalid`).
  A signature document, when present, must still be verified; community trust
  cannot silently ignore an invalid signature.

A package can declare `requires.packageTrust: 0` and
`vendor.bundledSignatureRequirement: "sameTeamAsHost"`, together with
`vendor.signatureRequirement: "publisherPackage"`. Only official-namespace IDs
may use this declaration. Both the primary and bundled policies must match the
host's pinned official catalog. Only a package whose actual resolved path is
inside the host's sealed installation and whose origin is `bundled` may omit
the publisher signature. Every independently installed, imported or rolled-back
copy still needs the publisher signature. A present signature is always checked,
even inside the host seal. The optional declaration alone grants no trust.
A host that does not declare `packageTrust: 0` in `hostCapabilities` filters this
version before download and rejects its manifest. A manifest without the
declaration gets no bundled exemption. The valid example is
`fixtures/package-trust/publisher-bundled.plugin.json`.

A host pins the signature requirement of its official plugin IDs so a package
cannot downgrade itself. Selecting a file from disk never overrides that pin,
namespace restrictions, publisher-key checks, or the installation seal required
by `sameTeamAsHost`. A development build may permit an unsigned publisher package
as `localDevelopment`; release builds refuse that origin. Community `none`
packages use their own approval path in both build configurations and must not
be reclassified as development packages.

## Marketplace

A marketplace source is a JSON index (`plugin-index.schema.json`) that lists
plugins and their release archives. Hosts fetch, cache, and download; Rust
Core decides validity and availability through `plugins.marketplace.*` so
every product shows the same catalog and the same install review.

### Delta archives

A marketplace can publish [`lithe-plugin-deltas.json`](plugin-delta-index.schema.json)
beside its version-0 index. The main index remains authoritative for version,
host compatibility, permissions review and the full archive digest. GitHub
companions use the commit that supplied the main index. A source containing a
query has no delta companion, so private link credentials need not travel to it.

A `zstd-prefix` entry transforms the exact base ZIP into the target ZIP. The
publisher creates it with Zstandard `--patch-from=OLD.zip`. The asset must end
in `.zip.zst` and belong to the same GitHub Release as the target archive. The
base and target must refer to the same plugin and platform, with a strictly newer
target. An entry binds both archive SHA-256 digests, target byte size, patch URL,
patch SHA-256 and patch byte size. ZIPs up to 64 MiB and patches up to 75% of the
target byte count are eligible. The decoder allows a 128 MiB maximum window.

The host uses a cached base only from the installed version's approved source.
Missing, unsupported or invalid deltas lead to ordinary full downloads.
Cancellation ends the operation. The host verifies the target ZIP through the
same installation path regardless of how it obtained the archive. A delta
grants no extra permission or right to change marketplace source.

### Sources

- The host ships one **official** source: the index at
  `Plugins/marketplace/index.json` in the Lithe repository. A copy is bundled
  read-only with the application so the catalog works offline; refreshing
  fetches the same file from
  `https://raw.githubusercontent.com/Lithe-IDEA/Lithe-IDEA/HEAD/Plugins/marketplace/index.json`.
- Hosts also preset one **community** source, Lithe Store
  (`https://github.com/Lithe-IDEA/Lithe-store`, index
  `https://raw.githubusercontent.com/Lithe-IDEA/Lithe-store/HEAD/lithe-plugin-index.json`).
  It is a third-party source in every respect (community trust, `dev.lithe`
  IDs refused), enabled by default, and removable; a host remembers the
  removal instead of adding it back.
- Users may add **third-party** sources. `plugins.marketplace.resolveSource`
  turns what the user pasted into the index URL to fetch: an https index URL
  is used as is (fragment removed); `https://github.com/<owner>/<repo>`
  resolves to `lithe-plugin-index.json` at the repository root on its default
  branch (`HEAD`); a `…/blob/<ref>/<path>` page resolves to its raw file.
  It also returns `recommendationsURL`, the sibling `lithe-plugin-recommendations.json`
  beside `indexURL` (query and fragment cleared). Plain http, embedded
  credentials, and other GitHub pages are refused with `plugin.source.invalid`.
- raw.githubusercontent.com serves a branch URL from a cache for minutes, so a
  just-merged index can look unchanged. For a GitHub-hosted index without a
  query, `resolveSource` also returns `revision: { repository: { owner, name },
  ref }`, the ref the URL follows (`HEAD`, the first path segment, or
  `refs/heads/<name>` / `refs/tags/<name>`); it is null for other sources and
  for a URL already at a full commit. A host may look the ref's newest commit
  up with the GitHub `latestCommit` operation and call `resolveSource` again
  with `commit`, a full SHA-1 or SHA-256 object name (otherwise
  `plugin.source.commitInvalid`); the answer then adds `pinned: { commit,
  indexURL, recommendationsURL }`, the same files at that commit, or null when
  nothing can be pinned. The host fetches the pinned URLs and falls back to the
  unpinned ones when the lookup or the pinned fetch fails. `indexURL` stays the
  source's identity: a host records and compares that one, never the pinned
  URL. A branch whose name contains `/` is only pinned when written as
  `refs/heads/<name>`.
- Hosts cache fetched indexes in platform cache or application-support
  storage, never inside the installation.

### Workspace recommendations

A marketplace source may publish a companion recommendations file
(`plugin-recommendations.schema.json`, `lithe-plugin-recommendations.json`)
beside `lithe-plugin-index.json`, keeping `lithe-plugin-index.json` (`schemaVersion: 0`)
unchanged for existing hosts. A source without `lithe-plugin-recommendations.json`
simply offers no automatic workspace recommendations.

| Field | Meaning |
| --- | --- |
| `pluginID` | Plugin ID listed in the same source's `lithe-plugin-index.json`. |
| `markers` | Optional exact workspace file basenames (for example `pom.xml`, `go.mod`, `build.gradle.kts`). Matched case-sensitively on Unix (`darwin-*`, `linux-*`) and ASCII case-insensitively on Windows (`windows-*`). No path separators, `.`, or `..`. |
| `extensions` | Optional lowercase file extensions without a leading dot (for example `java`, `go`). Matched ASCII case-insensitively against sampled workspace files (ignoring leading-dot names without a stem such as `.gitignore`). |
| `coveredBy` | Optional other plugin IDs whose installation already covers this workflow; when any listed ID is in `installedPluginIDs`, this plugin is not recommended. |

Each recommendation entry must declare at least one `markers` or `extensions`
value; duplicate `pluginID` values within one catalog are rejected with
`plugin.recommendations.duplicate` while keeping the first entry.
`plugins.marketplace.recommendForWorkspace` validates each catalog pair
(`index` + `recommendations`), filters to entries whose plugin is `available` on
the host platform and `hostCapabilities`, excludes installed, `coveredBy`-covered,
and dismissed IDs, deduplicates across catalogs (first catalog wins), and sorts
marker matches before extension-only matches, then by plugin name and ID.

### Index entries

| Field | Meaning |
| --- | --- |
| `id` | Plugin ID; must equal the package's `plugin.json` `id`. |
| `name`, `author`, `description`, `license` | Display metadata; must not be empty. |
| `localizedDescriptions` | Optional translations of `description`, keyed by language tag such as `zh-Hans`. Hosts show the entry matching the interface language and fall back to `description`. |
| `repository` | `https://github.com/<owner>/<repo>` that owns the plugin's releases and issue tracker. |
| `homepage` | Optional https URL. |
| `tags` | Optional single words, lowercased and sorted by Core. |
| `icon` | Optional `raw.githubusercontent.com` file of the same repository. |
| `versions[]` | `version`, required `pluginAPIVersion`, required `requires` (the package manifest's `requires`; a capability or capability version this contract does not define is allowed and makes the version unavailable), `releaseDate` (`YYYY-MM-DD`), and `downloads[]`. |
| `downloads[]` | `platforms` (for example `darwin-arm64`, `darwin-x64`, `windows-x64`, `windows-arm64`), `downloadURL`, and the archive's lowercase hex `sha256`. A platform may appear in at most one download of a version. |

A `downloadURL` must be a `.zip` GitHub Release asset of the entry's own
repository (`https://github.com/<owner>/<repo>/releases/download/<tag>/<asset>.zip`,
owner and repository compared case-insensitively), so an index cannot point a
plugin at another project's release.

### Validation and availability

`plugins.marketplace.validateIndex` accepts
`{ "index": object, "sourceKind": "official" | "thirdParty", "platform": string, "hostCapabilities"?: object }`.
A malformed index root or a `schemaVersion` other than `0` rejects the whole
index. Otherwise it returns `{ "status": "ok", "index": { "plugins", "rejected" } }`:

- `rejected` lists refused entries in index order with their `error`.
  A third-party source listing an official-namespace ID is refused with
  `plugin.index.officialNamespace`; a repeated ID keeps the first listing and
  refuses later ones with `plugin.index.duplicate`.
- `plugins` is sorted by ID. Each accepted entry is normalized, lists
  `versions` newest first (each with its `requires`), reports
  `platforms`, `latestVersion`, and `latestReleaseDate` of the newest
  version, and an `availability`:
  - `{status: "available", version, pluginAPIVersion, releaseDate, downloadURL, sha256}`
    for the newest version with this platform's archive that the host can
    run: `pluginAPIVersion` `0` and every capability of its `requires` hosted
    at the version it names (see [Versioning](#versioning));
  - `{status: "unsupported", code: "plugin.index.apiUnsupported", platform, message}`
    when the platform is listed only by versions that need a capability, a
    capability version or a `pluginAPIVersion` the host does not run;
  - `{status: "unsupported", code: "plugin.index.platformUnsupported", platform, message}`
    when no version lists the platform. Hosts show "not supported on
    <platform>" instead of an install action.

### Trust and install review

- **Official**: an entry is official only when it comes from the official
  source and its package passes the Lithe publisher signature.
- **Community marketplace**: entries from third-party sources. Hosts install
  in this order: verify the archive SHA-256 against the index
  (`plugin.archive.checksumMismatch`), validate the declaration and package,
  run `plugins.marketplace.installReview`, show the review, and install only
  after the user confirms. The index checksum proves a match to that index's
  bytes, not an authenticated publisher identity.
- **Community local disk**: a user-selected ZIP or unpacked directory has no
  marketplace index or authenticated repository. The host first copies a
  private snapshot, then validates and reviews that snapshot with the same
  Core command. A local checksum must never be shown as a repository match.

`plugins.marketplace.installReview` accepts
`{ "manifest": object, "expectedID"?, "expectedVersion"?, "approvedPrograms"?: [object], "hostCapabilities"?: object }`
and returns `{ "status": "ok", "review": { id, version, permissions, programs, newPrograms } }`.
A package whose ID or version differs from the index entry is refused with
`plugin.package.mismatch`, and a package that requires a capability version
missing from `hostCapabilities` with `plugin.api.unsupported`. `programs` lists everything the host may start for
the plugin, sorted by `kind`, language, and tool:

| `kind` | Fields | Meaning |
| --- | --- | --- |
| `packagedLanguageServer` | `languageID`, `path`, `arguments`, `validationArguments` | Executable inside the package, with its launch arguments and validation probe |
| `systemLanguageServer` | `languageID`, `commands`, `arguments`, `validationArguments` | Command resolved by host tool discovery |
| `runtimeLanguageServer` | `languageID`, `tool`, `packagedRuntime`, `entry`, `runtimeArguments`, `arguments`, `validationArguments` | Package entry started by a runtime; `packagedRuntime` is `null` for a discovered runtime |
| `buildSystem` | `buildSystemID`, `invocation`, `options`, `tasks`, `run`, `test`, `model`, `dependencies`, `provider`? | Every command line the build system can plan; `provider` is the shared form, which sends build requests to a language server |
| `packagedBuildProvider`, `systemBuildProvider`, `runtimeBuildProvider` | the matching language-server fields with `buildSystemID`, `executesProjectCode`, `mayUseNetwork` instead of `languageID` | A build provider with a process of its own |
| `packagedVcsProvider`, `systemVcsProvider`, `runtimeVcsProvider` | the matching language-server fields with `vcs`, `vcsTool`, `mayUseNetwork` instead of `languageID` | A VCS provider process; `vcsTool` is the declared VCS client or `null` |
| `runtime` | `tool`, `minimumVersion`? | Runtime the host discovers and passes to a language server, build provider, or VCS provider |
| `run` | `languageID`, `executable`, `arguments` | Run command and its argument template |
| `test` | `languageID`, `executable`, `workspaceArguments`, `fileArguments`, `testCaseArguments` | Test command and its argument templates |
| `projectGenerator` | `generatorID`, `command`, `arguments`, `environment`, `collect`, `mayUseNetwork`, `timeoutSeconds`, `when` | A `command` step of a [project generator](#project-generators); `files` and `archive` steps start no program |

Each entry carries everything that decides what actually runs, so an update
that keeps the same executable but changes its arguments, argument templates,
or validation probe is a new program; language-server and build-provider
entries also carry `initializationOptions` when declared. Hosts show all
`permissions` and the complete `programs`, and persist each approved program
as canonical JSON without dropping fields. A display may use readable labels
but must retain all canonical content, including a full JSON fallback for
program kinds it cannot describe completely. Recognized kinds may keep
their readable summary and offer a “Full Program Declaration” disclosure
with the complete canonical JSON; unknown kinds already render that JSON and
do not need a duplicate disclosure.

Local disk community installation requires explicit confirmation on every first
install, update, reinstall, and same-version replacement, including unchanged
programs and permissions. An empty `newPrograms` never waives disk confirmation.
Marketplace installation retains its existing policy: first install, missing
previous approval, or nonempty `newPrograms` requires confirmation; a marketplace
update with no new programs may reuse its recorded approval. A host may pass
previous `approvedPrograms` to highlight differences, but a confirmation always
shows the complete current review. Core's review remains a pure description and
comparison, not an approval grant.
When several marketplace sources list the same ID, the user chooses the source
at install time and later updates come only from that source. Local disk
provenance is never a marketplace source or an automatic update subscription.

### Local disk snapshots and installation provenance

A host's disk entry accepts a ZIP or an unpacked package directory. It does
not run code from the selection. The host's importer copies into a
fresh private directory (mode `0700`) before declaration validation, trust
checks, review, or installation. It refuses aliases, symbolic links, special
files, ambiguous manifests, and detected source changes during copying, and
applies archive/extracted-size and entry-count limits to the snapshot. Directory
copy depth is limited to 64, and its temporary root must stay outside the
selected source. Before extraction, ZIP preflight checks central and local
header paths/types and rejects duplicate or colliding paths.

The host injects `fileStorage.temporaryDirectory()`. Within that
platform-owned writable root the importer creates
`lithe-plugin-disk-<UUID>/archive.zip` for a ZIP and `package/` for copied or
extracted files. `archiveSHA256` is computed from the copied ZIP, not by reading
the original after review; an unpacked directory has no archive and this field
is absent/`nil`. The content digest covers the package files. Neither digest
authenticates a publisher or remote repository.

Disk digesting, Rust review, and candidate validation run on a native worker,
with a worker-owned Store decoder and immutable validation policy. The UI actor
remains available to close the page or cancel. Cancellation is checked between
review stages and before publishing the prepared candidate; snapshot cleanup
waits for the worker to finish reading it.

`PluginDiskProvenance` is host-owned local installation data, separate from
`PluginMarketplaceProvenance`; it is not a new field in `plugin.json` or the
marketplace index schema. A disk record contains the selected `fileName`,
optional `archiveSHA256`, `contentDigest`, and complete canonical
`approvedPrograms`. Marketplace provenance instead retains its source ID/name,
repository, index archive checksum, content digest, and approved programs.
Each community version must have exactly one of these provenance records;
missing or conflicting provenance is rejected.

The store rechecks the content digest of the installation staging copy before
publishing it, on every installed-package scan, and before rollback. Changed,
added, or removed package files reject that candidate or version. Active and
previous versions retain their own origin and provenance (`disk` /
`previousDisk`, `marketplace` / `previousMarketplace`); rollback verifies the
restored version against its own approval. Replacing the active version with
the same version retains the existing previous-version history; it does not
create a second rollback slot for the overwritten same-version bytes. A
marketplace entry with the same ID must not automatically offer updates or
reinstall a disk-managed installation.

The importer owns partial-copy cleanup on failure or cancellation. After
preparation the service owns the snapshot until review cancellation or install
success/failure and discards it in all these cases. Closing the plugin page
cancels its disk import and ends the installation session: pending snapshots
are discarded immediately and late prepared results are discarded rather than
installed. The package store owns its separate Application Support staging,
version directories, and installation records. None of these snapshots, staging files, installed versions, or approval
records are reusable worktree build resources: a content hash does not transfer
user consent or provide a version/platform/architecture/toolchain identity stamp.
They are excluded as `plugin-disk-install-state`. No runtime write targets the
signed app bundle or Windows installation resources, so these paths do not
change code signatures or Sparkle differential-update baselines.

Static checks alone do not verify the UI confirmation, complete program rendering,
cleanup, or bundle hashes in a real workflow.

## Build systems

`buildSystems` describes a project-level build tool (Cargo, Go, Gradle,
Maven, …) as data. Every build tool gets the same model and the same
commands: Core validates the declaration, detects instances, decodes the
model, and expands every command line; the host only runs the processes Core
planned and passes their output back. A plugin therefore needs no
host change to add a build tool, and no host code branches on a build-system
ID. One complete fixture covers each plugin-described model source:
`build-static-file` reads a project file, `build-command-stdout` decodes a
command's standard output, and `build-command-output-file` decodes a file the
command writes. They use Cargo, Go, and Gradle as realistic inputs;
they are not those plugins. `build-provider` and `build-provider-language-server` show the two
forms of a build provider, and `fixtures/build-systems/build-plans.json` is the golden list
of plans and decoded outputs every host gets from them.

A build system needs `buildSystems: 0` in `requires` (`1` with [command test discovery](#test-discovery)),
the `process.build` permission,
and the activation event `onBuildSystem:<id>`. IDs of build systems, tasks,
and run entries start with a lowercase letter and use lowercase letters,
digits, and hyphens. Option IDs are camelCase like setting IDs. Paths in a
declaration are relative, use `/`, and stay inside their base directory
(`plugin.path.invalid` otherwise).

| Field | Meaning |
| --- | --- |
| `detection` | `markers` are file names; a directory holding one is a candidate root. Core searches at most 6 levels below the workspace root and skips the folders hidden by default (Core's own and the names enabled plugins declare, see `layout`), plus directories Git ignores: the rules come from the `.gitignore` files inside the workspace and the `info/exclude` of each repository whose root lies in it, not from directories above the workspace root, `core.excludesFile` or the index, so a tracked manifest below an ignored directory is not a candidate. Detection also skips the workspace's ignored paths, a Build option the user edits (`.gitignore` patterns stored in the team or machine-local Build options): by default every directory whose name starts with a dot, such as `.claude/worktrees`, and the user can list more or bring one back with `!`. `nesting: "outermost"` folds deeper candidates into the outer instance (Gradle builds, Maven reactors); `"separate"` (default) keeps each (Cargo packages). `priority` (0–100, default 50) decides between plugins declaring the same ID; ties go to the smaller plugin ID. |
| `invocation` | `executables` in preference order: `projectFile` (a wrapper inside the root, with `requires` files; always runs project code), `environment` (a path below a variable listed in `environmentInputs`), `command` (a bare name found on `PATH`), `runtime` (a declared runtime dependency itself). `platforms` limits a candidate. `globalArguments` precede every command. `version` probes the executable with literal `arguments` and a `pattern` whose first group is the version; `minimumVersion` needs a probe. |
| `runtimeDependencies` | Core knows no runtime by name; `node` and `go` follow the same rules as any other ID. A runtime without `executables` names the [tool definition](toolchains/README.md) with the same ID, so its `id` must be a valid tool-definition ID and it declares only `minimumVersion`: `build.invocationPlan` returns it with `version: null` and the single candidate `{ "kind": "toolchain", "toolchainID": <id>, "root": <root> }`, whose resolution checks `minimumVersion`, and `build.checkToolVersion` for it is `plugin.build.unavailable` because Core reads no version. Any other runtime describes its own `executables` and `version` probe, so the runtime list is open; a runtime whose only candidate is a `toolchain` candidate may omit the probe, since the definition checks the version. |
| `options` | Build dimensions the user chooses: `boolean`, `singleSelect`, `multiSelect`, `text`, `file`, `directory`. Each contributes `arguments` and `environment` (with `${value}`) when it has a value. Choices are declared, come from the model (`choicesFromModel`, with optional `optionDefaults` for choices the tool activates when unset), or are free (`allowCustom`). A `text`, `file` or `directory` option may also get the value the tool uses while it is unset from the model's `optionDefaults` (Maven's user settings file, its local repository); `build.resolveOptions` returns it as `implicitValue` for display only, and it contributes no argument. `description` (1-512 characters) says what the option does, for the host's info icon; `displayName`, `description` and choice titles are translated through the `buildOptions` part of a [translation file](#translations); a `multiSelect` option may set `excludePrefix` (or report `optionExcludePrefixes` in `litheBuildModel`) so unchecking a default choice passes `<prefix><choice>`. |
| `provider` | The process that answers model and dependency requests when no command prints them; see [Build providers](#build-providers). |
| `model` | `static` (a `toml` file read during detection, no process), `command` (a command whose output Core decodes; it states `executesProjectCode` and `mayUseNetwork`), or `provider`. |
| `tasks` | The task catalog. `scopes` say what a task runs on (`workspace`, `project`, `target`); `targetTags` restrict targets; `requiresUserArguments` makes the host ask for arguments. A `fromModel` task is the template for tasks the model lists. |
| `run` | Run entries from model targets (`fromTargets.tags`, with `arguments`) or model tasks (`fromTasks`, reusing the template task's arguments). |
| `runConfigurationTypes` | Kinds of run configuration the user creates in the Run/Debug Configurations dialog, from declared fields and a command line; see [Run configuration types](#run-configuration-types). |
| `beforeLaunchTasks` | Kinds of step a configuration's Before Launch list runs first, declared like run configuration types; see [Before-launch tasks](#before-launch-tasks). |
| `test` | How tests run: `scopes`, `targetTags`, the `filters` (`suite`, `case`) it can select, and `results` (`junitXml` or `litheTestResults` report globs relative to the project). [Test discovery](#test-discovery) is opt-in. A workspace run reads the globs under the root and every model project. A requested filter must reach the command line through a template whose conditions hold; a `case` guarded by `suiteSelected` therefore needs its suite. |
| `diagnostics` | `lineMatcher` patterns with capture-group numbers, or `litheDiagnostics` lines. |
| `dependencies` | A command (with `scopes`) whose output Core decodes, as a flat list through a `map` or as a tree from `litheDependencies`; or `provider` (with `scopes`, `workspace` and `project` only). |
| `sync` | Files whose change makes the model stale, relative to the root and to each project. |
| `layout` | `excluded` directories relative to each project, added to every project as `excluded` content roots. `hiddenDirectoryNames` (at most 32) are folder names, such as dependency, cache and build output folders, that every workspace hides at any depth while the plugin is enabled: the project tree, search, file events, Local History, run discovery, build detection and VCS and Git repository discovery skip them. Each name is one path component compared ignoring ASCII case, never a glob: it is not `.` or `..`, has no surrounding whitespace, `/`, `\`, `*`, `?`, `[`, `]` or control character, and does not start with `!` (`plugin.path.invalid` otherwise). Search Everywhere still lists their file names when the host asks for generated file names. Users turn a name off with a `!name` hidden-file pattern, as for Core's own folders. |

An option may declare `scope: "instance"`.
Omitted scope (or `"workspace"`) keeps workspace/build-system storage. Instance
values are independently persisted by normalized detected root: changing or
resetting `a` cannot change `b`, even when both accept the same choices. Explicit
`false`, `""`, and `[]` remain stored user values. The host merges only the
matching scope before detection, static run generation, model/dependency/task
requests, synchronization fingerprints, and language import. Instance data cannot
override workspace options, and workspace values cannot silently override an
instance option. When an existing option moves to instance scope, the host copies its shared value
to each detected or previously stored root, preserving explicit instance values
and empty markers. It removes the shared representation only after readback, then
redetects static models. Failure retains the source and blocks publishing defaults
until retry; scope is not a fallback chain. See `fixtures/build-systems/build-instance-options.plugin.json`.

### Presentation

Each enabled build system gets a tool window of its own, titled with its
`displayName`; `icon` (a [UI icon](#ui-icons)) is its activity-bar
icon. As for every tool window, the host owns the shell: dock, order,
visibility, layout, and lifecycle. The host draws the window body from the
model, and three optional fields tell it where an entry goes. None of them
changes a plan.

- `options[].presentation: "toolbar"` puts a boolean option on the window's
  toolbar as a toggle, for offline mode or skipping tests; it needs an
  `icon`. `"tree"` shows a multiSelect option as a checklist node at the top
  of the tree (Maven profiles). Without `presentation` the option appears only
  in the option list. An icon on an option that is not `toolbar` is
  `plugin.manifest.invalid`.
- `tasks[].category` lists 1–4 headings, outermost first, that file the task
  in its project (`["Lifecycle"]`). Without it the host files the task by
  `group`. On a `fromModel` template it is the default for the tasks the
  model lists.
- A model task in a `litheBuildModel` document can carry its own `category`,
  an array of strings. Core trims the headings. A category that is deeper
  than four headings or has an empty heading falls back to the template's, so
  a tool that reports a bad heading still has its task filed.

Core lays out the window's tree from the model, the same way for every build
tool (`build.modelTree` in the
[Rust Core API](../../../../rust/contracts/rust-core-api.md#build-systems)):

- Each `options[]` entry with `presentation: "tree"` that currently offers at
  least one choice appears at the top of `nodes` as an `option` checklist node
  with `optionChoice` children (`checked`, `defaultActive`).
- Projects nest by `parentID`. A project with no parent in the model, or one
  whose parent chain loops back to itself, becomes a root.
- Each project lists, in this order, its tasks in their folders, a
  dependencies node when `dependencies.scopes` contains `project`, a targets
  node whose targets carry their target-scoped tasks (filtered by
  `targetTags`), and its child projects.
- A declared task with a `project` scope appears in every project. A task with
  only a `workspace` scope, or one that `requiresUserArguments`, appears once,
  beside the root projects; so do workspace dependencies and discovered
  tests. When the model has no projects, every `workspace` task appears there.
- When the model has a single root project, that project's contents become
  the build system's own, and its tasks still run on the root project. Guard
  project-only arguments with `nestedProjectSelected` so running them on the
  root builds everything.
- A filter keeps each matching node whole, together with the projects and
  folders on the way to it.

A host that draws build-system windows gives each detected build system its own tool window and uses
these presentation fields. The host-drawn windows share Build Output, docking,
layout restoration, activation, and lifecycle with other workbench contributions.

### Templates

Arguments are template strings, or `{ "value": …, "when": condition }` with
one condition or a list that must all hold: `projectSelected`,
`nestedProjectSelected` (a project other than the root one), `targetSelected`,
`suiteSelected`, `caseSelected`, and their `no…` opposites.

| Variable | Available in |
| --- | --- |
| `${root}` | every list |
| `${project.name}`, `${project.path}` (relative to the root), `${project.property:<key>}` | lists with a project scope; guard with `projectSelected` when the project may be absent |
| `${target.name}`, `${target.property:<key>}` | target-scoped tasks, tests, and target run entries; guard with `targetSelected` when the target may be absent |
| `${task.name}`, `${task.property:<key>}` | `fromModel` tasks |
| `${test.suite}`, `${test.case}`, `${test.case:regex}` (regex-escaped) | `test.arguments`, guarded by `suiteSelected` / `caseSelected` |
| `${optionArguments}`, `${optionArguments:<id>}` | every command list; a whole argument that expands to all options, or one option |
| `${userArguments}` | tasks, tests and run configuration types; a whole argument. A task with `requiresUserArguments` must pass it |
| `${pluginDir}`, `${workspaceStateDir}`, `${outputFile}` | model and dependency commands; `${outputFile}` exactly when `output` is `outputFile` |
| `${value}` | option `arguments` and `environment`, and the `arguments` of a run configuration field |
| `${field:<id>}` | the `arguments` of a [run configuration type](#run-configuration-types); a whole argument that expands to one field's arguments |

Each option's arguments are emitted once per command line: reference them
either in `globalArguments` or in the command's list, and only once within a
list, except under opposite conditions. An option that no list references
contributes nothing. Unknown, misplaced, unguarded, or repeated variables are
`plugin.template.invalid`.

### Workspace trust

Loading, building, testing or running a project can run code from it: build
scripts, build-tool extensions and plugins, and the program itself. The user
decides once per workspace, as in IntelliJ IDEA: the host asks when a workspace
opens whose trust is unknown, and the user trusts it (optionally with every
project in its parent folder) or keeps it in safe mode. Core stores the
decision in the machine-local toolchain catalog and reports it as
`projectCodeAllowed`; the host passes that value with every build plan request.

In a workspace the user has not trusted, Core refuses with
`plugin.build.untrustedWorkspace` every plan that would run project code:

- every task, test, run configuration, before-launch task, run preparation and
  debug preparation, since they run the project;
- a `command` model or dependency source that declares
  `executesProjectCode: true`;
- a request to a build provider that declares `executesProjectCode: true`.

A `command` model or dependency source that declares
`executesProjectCode: false` loads in safe mode, so declare `false` only for a
command that never runs build scripts or other project code. A `static` model
needs no process. In safe mode the host leaves automatic loads undone and asks
the user to trust the workspace before an action the user starts.

Processes that start without a user action follow the same decision. The host
passes `projectCodeAllowed` with every
[launch plan request](#process-start), and in an untrusted workspace Core
refuses with `plugin.launch.untrustedWorkspace`:

- a language server, unless it declares `executesProjectCode: false`
  (`languageServers` 1); a server that imports the build or loads project-local
  plugins keeps the default;
- a build provider with a process of its own that declares
  `executesProjectCode: true`; a provider that shares a language server
  follows that server's declaration;
- every VCS provider, since a VCS client reads the repository's own
  configuration, which can name programs to run (hooks, filters, extensions).

`plugins.languageServers.decodeLaunch` checks the same rule again, so an answer
that arrives after the user revoked trust starts nothing. The host does not
start these processes in safe mode, shows that the project is in safe mode
where they would have appeared, and starts them once the user trusts the
workspace.

### Model and outputs

A model or dependency command prints to `stdout` (default) or to the
host-allocated `${outputFile}`. Its decoder is `jsonStream`
(whitespace-separated JSON values such as NDJSON or `go list -json`, decoded
as an array; a single document becomes a one-element array) or a canonical
format that needs no `map`. A static model file is `toml`.

A `map` only selects values. A model map turns a decoded document into
projects, each with its targets and content roots, and into option values; a
dependency map turns it into a flat list. Rules select elements with `each`,
filter them with `where` predicates (`{ "pointer", "equals" | "exists" }`),
apply only as a `fallback` or `whenFile` a file exists, and read fields with
selectors: `""` (the element), `@key` (its key), `/a/b` (relative to the
element), `$/a/b` (from the document root), `*` (each item). A field is a
selector, `{ "from", "transform": "lastSegment" }`, or a literal
`{ "value": "src" }`. Target `tags` are a literal list, and a project's
parent is the deepest project containing it. Paths are relative to the root
or absolute inside the workspace; anything else is dropped. A target `fallback` is
skipped only when a non-fallback target rule with an overlapping tag already
produced a target, so an explicit binary suppresses an implicit `src/main.rs`
target while explicit test or example targets do not.

Whatever a map cannot express is the plugin's own work: other file formats,
tasks read from the tool, explicit parent links, tags computed from values,
and dependency trees. The plugin's command (a script under `${pluginDir}`,
or the tool itself with a plugin-supplied init script, as Gradle does) prints
`litheBuildModel` or `litheDependencies`, or a
[build provider](#build-providers) answers. Unlike a static file, a command
runs only when the host starts it on request. The map does not grow into a
transformation language: a rule is added only when
real plugins need it and it cannot reasonably live in the plugin.

Canonical formats let a plugin's own script print the shape Core expects:

| Format | Shape |
| --- | --- |
| `litheBuildModel` | `{ "format": "litheBuildModel", "version": 0, "projects", "targets", "tasks", "contentRoots", "optionValues", "optionDefaults"?, "optionExcludePrefixes"?, "optionActiveChoices"? }` with the model's field names; `tasks[].category` is an array of headings |
| `litheTestResults` | `{ "format": "litheTestResults", "version": 0, "tests": [{ "suite", "case", "status", "message"?, "durationMillis"?, "path"?, "line"? }] }` |
| `litheDependencies` | `{ "format": "litheDependencies", "version": 0, "dependencies": [{ "name", "group"?, "version"?, "scope"?, "properties"?, "children" }] }` |
| `litheDiagnostics` | output lines `{ "lithe": "diagnostic", "path", "line", "column"?, "severity"?, "message" }` |

A canonical model can report `optionActiveChoices: { <optionID>: [...] }` for
model-backed multi-select options. List the values the tool actually activated
for this invocation, including values activated outside the IDE's saved
options. Use an empty array if the tool reports that none are active. Omit the
entry when the tool cannot observe the active set. Report baseline defaults in
`optionDefaults`; Core restricts both sets to the available `optionValues` and
ties the observed set to the option fingerprint of the model load. For a
`text`, `file` or `directory` option, `optionDefaults: { <optionID>: [value] }`
is the value the tool uses while the option is unset; Core keeps the first
value and passes nothing for it.

Unknown fields in canonical documents are ignored, so a script may add fields
without a new version. Output that does not fit its decoder is `plugin.build.invalidOutput`.

### Provider task execution

A task with `source: "provider"` is answered by the plugin's [build provider](#build-providers).
It keeps the ordinary task ID, scopes, target tags and model selection, but
`arguments` must be `[]` and `execution` must be `task`. Application/service
launch and test discovery are separate capabilities. Provider model-task
templates cannot be referenced by `run[].fromTasks`; they need a typed Run
capability rather than an empty command plan. A provider task never
falls back to a host command or a plugin-private function. Command tasks
retain their current shape and omit `source`.

`build.taskPlan` returns a `provider` request instead of argv for this source.
`lithe/build/execute` receives the effective `buildSystemID`, workspace-relative
`root`, `options`, `optionArguments`, option `environment`, `task` declaration ID,
bounded `userArguments`, and optional `project`, `target`, `modelTask` selections
(`name`, workspace-relative `path`, `tags`, `properties`). Scope/path validation
is identical to command tasks. The provider owns build semantics; options
are not a second host-generated command. As with model/dependency requests,
the host may add `tool: { executable: <resolved absolute path> }` from the
same invocation resolver; explicit invalid tool/runtime selection is a failure,
never permission for independent discovery. It must reject stale project facts
rather than guess a target.

The host offers the `litheBuild` versions it implements, `[0, 1]` today: the
protocol's version is a version of `buildProviders`, and the provider answers
the newest one it speaks, such as `{ "version": 1 }`. Version 1 adds
`lithe/build/runEntrypoints` (see [Run entrypoints](#run-entrypoints-buildproviders-1))
and changes nothing of version 0. Unsupported negotiation fails explicitly. Task execution and
its progress notification belong to this protocol with `lithe/build/model` and
`lithe/build/dependencies`.

Core sets `params.operationID` to the unique JSON-RPC request ID (as a string);
callers cannot choose it. `lithe/build/progress` notifications echo that ID and
may include `message` (up to 4 KiB UTF-8), integer `percentage` (0–100), and
`output` (up to 64 KiB UTF-8 per notification). Malformed reports and reports
for ended, cancelled, timed-out or other requests are dropped. Core maps the
wire ID back to the host operation ID; reused host IDs cannot accept old
progress. No plugin-function negotiation is needed.

The reply is `{ "exitCode": <signed 32-bit integer>, "output": <string> }`.
`output` is the complete bounded diagnostic text (at most 32 MiB UTF-8),
including text streamed in progress; the host displays progress output and
uses final output for the existing diagnostic decoder without duplicating it
in the console. Nonzero exit codes are completed tool failures, not protocol
errors. Malformed replies end with `plugin.build.invalidResult`; JSON-RPC
errors end with `plugin.build.failed`. Timeout and cancellation use the
existing request codes and `$/cancelRequest`. Request completion occurs once;
late replies/progress do not revive it.

The host binds each call to the plugin package, build-system/workspace
session, current run, effective option fingerprint and watched configuration revision.
Settings, package replacement, disable, workspace close/reopen, cancellation
and backend failure invalidate or stop the owning operation. The existing
Provider session owns children and writable workspace state; no page,
additional runtime, directory or cross-plugin private cache is required.
Only public typed results may cross domain boundaries.

Canonical examples: `fixtures/build-providers/build-provider-execution.plugin.json` and
`fixtures/build-providers/build-execution-protocol.json`.

### Provider test execution

A build system's `test` may declare `source: "provider"`. It keeps the existing scopes, target tags and
supported filters, but `arguments` must be `[]`. It requires a plugin-owned
Provider process; `results` may be omitted or use `junitXml`/`litheTestResults`
reports: the plugin owns test execution and report production, and the host
reuses the existing report decoder.

`build.testPlan` returns `provider: { method: "lithe/build/test", params }`
and the usual `results` metadata instead of argv. The effective
`buildSystemID`, `root`, `options`, `optionArguments`, `environment`, bounded
`userArguments`, optional `project`/`target`, and optional `suite`/`case` are
typed fields. A case filter does not require a suite when the provider
supports it; providers must reject selectors they cannot honor rather than
widening the test run. Paths, scopes and declared filters are validated before
the request is sent. The host's report project list only resolves report globs;
it is not a second backend project model.

A provider receives `lithe/build/test` only for a `test` that declares `source: "provider"`. Tests use the same Core-assigned operation identity,
`lithe/build/progress`, tool hint, bounded `{ exitCode, output }` result,
timeout, cancellation and late-result rules as task execution. Reports are
read after execution using the existing freshness, path and size checks.
Workspace-wide reports still require a current project model when the build
system has one. Configuration validity is retained through diagnostic and
report decoding; late results cannot update a changed workspace/configuration.

No new resource/cache location is introduced.
Canonical examples: `fixtures/build-providers/build-provider-tests.plugin.json` and
`fixtures/build-providers/build-test-protocol.json`.

#### Test discovery

A build system can discover exact test selectors for the native test tree. Both
sources require the `suite` and `case` filters and discover the whole instance.

- **Command.** A plugin that declares `requires.buildSystems: 1` may set
  `test.discoveryCommand: { arguments, executesProjectCode, mayUseNetwork }`.
  The command uses the existing `process.build` permission, build tool selection,
  global arguments, effective options and build-instance working directory.
  Use `${outputFile}` in the discovery arguments to tell the tool where to write
  the document. The same template family as a command model also allows
  `${pluginDir}`, `${workspaceStateDir}`, root and option arguments. The host
  supplies a fresh absolute file path in its writable workspace state and
  deletes it when the attempt ends. The command must exit successfully before
  the host decodes its file. The two effect flags are required and become part
  of install review and the plan. Discovery runs on request, with a deadline
  and process-tree cancellation. The test run itself uses the separately
  declared `test.source` and existing argument or provider contract.
- **Provider.** Provider tests may declare `test.discovery: true`, using the
  existing `buildProviders` protocol. `build.testDiscoveryPlan` returns
  `lithe/build/discoverTests` with the effective options and optional
  `refresh` and `tool` hints. The provider sends the document as an inline
  result under the existing bounded request and cancellation lifecycle,
  without execution progress.

Enable one source. Project selection is refused for both. `outputFile` is
required for command plans and refused for provider plans. The command writes
and the provider returns `{ "format": "litheTestDiscovery", "version": 0,
"tests": [{ "suite", "case", "project"?, "target"? }] }`. Project and target
are the usual typed selections with workspace-relative paths. There may be at
most 10,000 items and 32 MiB UTF-8 JSON. Unknown document/item fields are
ignored; malformed selectors, invalid paths/scopes, unsupported versions and
oversized lists reject the entire result with `plugin.build.invalidOutput`.
An empty array is a successful discovery with no tests.

The host passes the original plan configuration and either the same
`outputFile` (command) or inline `result` (provider) to
`build.decodeTestDiscovery`. Core validates every selector through the same
test-plan rules, deterministically orders the list, collapses exact duplicates,
and derives an opaque `id` including project/target identity. Equal suite/case
names in sibling modules therefore remain distinct. The returned `discovery`
contains `tests: [{ id, suite, case, project?, target? }]`. No source parsing,
framework detection, or second project graph occurs in the host/Core.

Native discovery starts only on a user action and is independent of plugin
pages. A provider request reuses the workspace Provider. Source edits,
configuration changes, package changes and workspace teardown revoke cached
selectors and in-flight discovery. Before running a selected item, the host
revalidates its ID against the current list and plans the exact selectors with
`build.testPlan`. The selected command or provider must reject a selector
whose facts have become stale externally. The host records a terminal
`lithe/build/discoverTests` operation for each command attempt, following the
existing [declarative logging rules](#structured-plugin-diagnostics).

Canonical examples are
[`build-command-test-discovery.plugin.json`](fixtures/build-systems/build-command-test-discovery.plugin.json),
[command output](fixtures/build-systems/build-command-test-discovery.json)
and [the provider result](fixtures/build-providers/build-test-discovery.json).

### Provider run preparation

A run declaration may use `source: "provider"`. It uses `fromTargets.tags`,
which is required; `fromTasks`, nonempty `arguments`, and extension templates
are refused. The optional `name` is a literal display name, and `execution` is
`application` (default), `service` or `task`. `workingDirectory` is supplied by
the prepared result; the declaration's command-source default has no effect.
These declarations never generate empty command-based persisted Run entries.

`build.prepareRunPlan` validates a selected `run` declaration ID, required
`project` and `target` selections, effective options, and bounded
`userArguments`. It returns a provider request for `lithe/build/prepareRun`,
with the common instance fields, options/option arguments/environment,
`run`, `project`, `target`, and `userArguments`. An output file is refused.
The host may add the resolved build tool hint. The provider owns upstream
synchronization, build, classpath and launch facts; Core does not reconstruct
those facts from source files or build configuration.

Preparation shares the Core-assigned wire `operationID`, progress,
timeout, cancellation and stale-result handling of execution. It ends when
preparation finishes; the application is a separate host-owned process.
A JSON-RPC error aborts preparation and cannot launch an application. It retains
the common LSP `serverError` category; it is not an execution exit-code result
and does not forward a private-function `pluginErrorCode`.

The successful result is a portable launch plan:

```json
{
  "executable": { "kind": "toolchain", "value": "project-jdk" },
  "arguments": ["-cp", "classes", "example.Main"],
  "workingDirectory": "app",
  "environment": {}
}
```

The executable is either a host toolchain identifier (`toolchain`) or a bare
command resolved by the host (`command`); absolute paths and shell command
strings are refused. Arguments are separate strings, at most 1,024 entries
and 64 KiB UTF-8 each, without NUL. The environment has at most 64 entries;
keys match `[A-Za-z_][A-Za-z0-9_]*` with at most 128 bytes, values are strings
of at most 64 KiB without NUL. The complete JSON is bounded at 32 MiB.
`workingDirectory` must be an existing workspace-relative directory inside
the selected instance, including after resolving filesystem aliases.

`outputEncoding` is optional: the character encoding the process writes to
stdout and stderr, for a provider that knows it, for example because it starts
the tool with a fixed output encoding. It is one of `utf-8`, `gbk`, `gb18030`,
`big5`, `shift-jis`, `euc-jp`, `euc-kr`, `windows-874`, `windows-1250` to
`windows-1258` and `koi8-r`, matched case-insensitively; the aliases `utf8`,
`cp936`, `cp950`, `shift_jis`, `sjis`, `cp932`, `eucjp`, `euckr`, `cp949`,
`cp874`, `cp1250` to `cp1258`, `iso-8859-1`, `latin1` (both `windows-1252`)
and `koi8r` are accepted, and the decoded plan carries the id. Any other
value, UTF-16 included, fails as `plugin.build.invalidOutput`. The host still
decodes each output line on its own: with the automatic console encoding
setting it tries the declared encoding first, then UTF-8, then GBK on Windows, and a fixed encoding the user chose overrides the
declaration. Leave it out when unsure; automatic decoding uses UTF-8 everywhere and falls back to GBK on Windows.

Unknown fields are ignored. The host calls `build.decodeRunPreparation`
with its workspace root, instance root and raw result; any malformed plan
fails as `plugin.build.invalidOutput` before executable resolution.

#### Typed run options and service/task preparation

The same `lithe/build/prepareRun` method carries typed launch options. A
provider run may declare `execution: "service"` or `"task"`; it uses
`fromTargets.tags`, never host command or extension templates. Preparation ends
before the host starts the process; the host owns stdout/stderr, process
identity, Stop and cleanup. A provider does not own background services.

The optional `launchOptions` object contains required `execution`
(`application`, `service` or `task`), `runtimeArguments` and `arguments`
(string arrays), and `environment` (a string map). Optional `entrypoint` is an
opaque nonempty string of at most 4,096 UTF-8 bytes without NUL; the owning
provider validates its meaning. Optional `workingDirectory` is a nonempty
workspace-relative directory, `.` allowed, without backslashes, absolute
paths, empty components or `..`. Unknown option fields are refused. Argument
and environment bounds match the launch result above, including empty argv
values. Options describe resolved user choices; providers must preserve
argument boundaries and cannot silently ignore unsupported choices.

`execution` must match the run declaration (default `application`). Service
and task preparation require `launchOptions`; application preparation without
it uses the declaration's defaults. Both SDKs reject malformed options before
invoking a handler.
Build options, resolved tool hints and native settings remain their existing
sources; launch options do not create a second profile or tool configuration.

The host supplies `workspaceRoot` to `build.prepareRunPlan` when options contain
an explicit working directory. Core validates it against the canonical workspace,
including aliases, before preparation. The host passes the same options to
`build.decodeRunPreparation`; the result must use exactly that directory.
Only this explicit override may lie outside the selected instance. Without
an override the original instance containment rule still applies. Other
launch result validation and cancellation/stale-result behavior are unchanged.
Canonical fixtures: `fixtures/build-providers/build-provider-run-options.plugin.json` and
`fixtures/build-providers/build-run-options.json`.

#### Run entrypoints (`buildProviders` 1)

A provider run may declare `entrypoints: true` instead of `fromTargets`. It is
an application that starts an entrypoint the provider names, such as a class
with a `main` method, in one of its projects. It requires `source: "provider"`
and `buildProviders` 1 in `requires`; `fromTargets`, `fromTasks`, `arguments`,
`extension`, a native `debug.adapter`, and a `service` or `task` execution
are refused. A registry `debug.configurationType` requires `buildSystems` 2.
It applies to no target, so the host never lists it per target: the user starts
it from a source file.

`build.runEntrypointsPlan` takes the common plan fields, optional `refresh`
and `tool`, and `file`, a workspace-relative file inside the instance root.
`project` and an output file are refused. It returns a request for
`lithe/build/runEntrypoints` whose params carry `file` beside the effective
options. The method uses the bounded request lifecycle of
`lithe/build/discoverTests`, without execution progress. The provider reads the
file and answers:

```json
{"format":"litheRunEntrypoints","version":0,"entrypoints":[
  {"run":"main","name":"Tool","entrypoint":"example.Tool",
   "project":{"name":"app","path":"app"}}]}
```

`run` names an entrypoint run, `name` is the display name (1–256 bytes, no
control characters), `entrypoint` is the opaque `launchOptions.entrypoint`
(1–4,096 bytes, no NUL), and `project` is a typed selection with a
workspace-relative path inside the instance. There are at most 64 entries,
in the provider's order, most relevant first. An empty list means the file
holds no entrypoint. The host passes the answer to `build.decodeRunEntrypoints`
with the plan's configuration; Core returns `entrypoints` without exact
duplicates and refuses a malformed answer whole with
`plugin.build.invalidOutput`.

The host saves an answer as an ordinary provider run reference with an empty
`targetID` and `extensions.build-run.entrypoint`, then prepares it with
`build.prepareRunPlan`: an entrypoint run takes `project`, no `target`, and
requires `launchOptions.entrypoint`; a target run still requires its target.
The provider receives `lithe/build/prepareRun` without `target`. Canonical
fixtures: `fixtures/build-providers/build-provider-run-entrypoints.plugin.json`
and `fixtures/build-providers/build-run-entrypoints.json`.

#### Registry connection debugging

A provider service run may declare `debug: {"configurationType":"example"}`.
With `buildSystems` 2, a provider application (including an entrypoint run)
may make the same declaration. Saved application configurations keep
`execution: "application"` for Run and Debug. Debug sends the existing
connection fields in `launchOptions.debug`, with no change to the
negotiated `litheBuild` version. The declaration names a public DebugAdapter Registry
alias, not another plugin's private language server or a local adapter ID.
The host selects one currently enabled adapter advertising the type and `attach`,
respecting an explicit qualified adapter selection. Missing or ambiguous entries
fail before starting the provider process. It sends:

```json
{"debug":{"adapterID":"dev.example/debug","configurationType":"example",
  "target":{"kind":"connection","host":"127.0.0.1","port":51235}}}
```

These are the exact three debug fields and three target fields. The host owns
the port (integer 1–65535), process, readiness and cleanup. Explicit null, unknown
fields, remote hosts and task execution are rejected. The type must
match the selected run declaration. The provider uses its
upstream engine to suspend the debuggee until attachment; it owns runtime-specific
flags and fails if it cannot honor the request. It returns the ordinary portable
Run process plan, without selecting an endpoint or returning DAP arguments.

The host retains the adapter's qualified identity, package and registry generation
through preparation, readiness and attach. Disable, replacement or stale input
revokes the operation and cleans its owned process. Adapter configuration is
produced by existing `debug.adapters.startArguments` with a `connection` target;
the selected registered adapter owns its transport. In particular,
`tcpLanguageServer` may refer only to a language server declared by that adapter's
own plugin. No built-in language debugger fallback is permitted.
This does not change same-package native `prepareDebug`.
Canonical fixture: `fixtures/build-providers/build-connection-debug-options.json`.

### Native debug preparation

A provider run may additionally declare `debug: {"adapter":"native"}`. `adapter` is a
local ID declared by this plugin; qualified cross-package references are refused.
The adapter must serve launch of executable targets, with bindings `program` ←
`target.path`, `cwd` ← `target.workingDirectory`, `args` ← `target.arguments`,
and `env` ← `target.environment`. These fields cannot be overridden by adapter
default arguments. It covers native executable launch only;
it does not accept attach endpoints, backend-selected adapters or arbitrary DAP JSON.

`build.prepareDebugPlan` validates the same selected run/project/target and
arguments as run preparation and returns `lithe/build/prepareDebug`. Progress, cancellation, timeouts, tool hints, workspace
ownership and JSON-RPC failure handling follow run preparation. A successful result
contains exactly these four fields:

```json
{"program":"app/bin/sample","arguments":["a b","x#y"],"workingDirectory":"app","environment":{}}
```

The provider owns synchronization/build before returning. `program` is an existing
workspace-relative regular file contained in the selected instance, including
canonical symlink resolution. Arguments, environment and directory retain run
preparation's size and containment limits. Unknown fields are refused here; there
is no adapter process, socket, shell, private function or DAP argument channel.
`build.decodeDebugPreparation` takes the selected `run` separately and returns
`preparation` with its validated `root`, declaration-owned `adapter`, and these
four fields. Missing, malformed and escaping targets fail before adapter startup.

The native host saves documents before preparation, retains model/configuration/
source/plugin identity through decode, and drops stale results. It checks executable
permissions and canonical containment again at handoff. `debug.adapters.startArguments`
binds the validated target facts; the existing Registry launches the adapter using
its own permissions and package process declaration. The launch configuration retains
`pluginID/adapterID` for routing and retry. Stopping preparation cancels only its
operation; the Debug host owns launched sessions and plugin withdrawal cleanup.
Saved Run configurations keep their existing selectors and prepare again for Debug.

Fixtures: [native declaration](fixtures/build-providers/build-provider-debug-preparation.plugin.json)
and [native result](fixtures/build-providers/build-debug-preparation.json). This does not provide
preparation through a language server, long-running services or cross-plugin collaboration.


The native Build tree re-resolves model IDs before preparation. Source/model,
option, settings, package and workspace changes revoke a pending preparation.
After validation the existing Run host resolves the executable and owns output,
stop, exit status, and argument-resource cleanup. Workspace teardown and plugin
disable stop the application by its exact host operation ID; an old owner
cannot stop a replacement process. The Run host can explicitly save a selector to the existing local Run v2 layer
as `provider: "build.provider"` with `extensions.build`. The reference contains
`pluginID`, `buildSystemID`, instance `root`, `run`, `projectID`, `targetID`, and
raw `userArguments`, never an executable, environment, or cached classpath.
Every restart re-resolves the enabled instance/model and sends a fresh preparation.
Missing plugins/targets/models fail explicitly; there is no command fallback.
Saving and running work before Run configurations are generated.
See `fixtures/build-providers/build-provider-run-preparation.plugin.json`
and `fixtures/build-providers/build-run-preparation.json`.

### Optional native configuration

An own-process provider may opt into `buildSystems[].configuration: {"source":"provider"}`.
Shared language-server providers cannot opt in.
`build.configurationPlan` uses the same selected instance/options/environment/tool
snapshot as model loading and emits `lithe/build/configuration`. It has no project,
output file, or mutation parameters. A provider that implements the method
advertises the server feature `litheBuildConfiguration: true`. Configuration reads are bounded requests
with the existing cancellation, timeout, workspace and package ownership.

`build.decodeConfiguration` validates `result` and returns `configuration`:
`{"format":"litheBuildConfiguration","version":0,"sources":[...]}`. There are at most
8 sources, each with a unique provider-owned `id`, `scope` (`installation`, `user`,
`workspace`), file `uri`, exact byte `revision` (`missing` or `sha256:` plus 64 lower-case
hex digits), `writable`, and up to 64 `fields`.
Installation sources are always read-only. Two scopes may alias one URI only when
both are read-only and have identical revision and fields. File URIs
cannot carry credentials, queries or fragments.

Each field contains `id`, `kind` (`text`, `file`, `directory`, `boolean`, `stringList`),
`value` (raw native value, or null for absent), `effectiveValue`, `effectiveSource`
(an upstream technical origin ID). IDs are opaque bindings interpreted by the
provider; there is no document-format, XPath, XML root or element binding in the
host contract. Fields cannot duplicate or expose credential/secret fields;
values and identifiers are bounded and kind-checked. Full native documents, secret
values, credential-bearing URLs and private keys must never enter this response,
logs or normal controls. Source discovery, interpolation, precedence and effective
values belong to the selected upstream engine; Core does not infer them.

The native host populates raw/inherited values, displays scope and target URI,
maintains drafts, and requires explicit Save. Opening, cancelling, losing focus
and leaving a page do not write native sources. Passive page reads refuse a
project-owned executable (such as a cold Wrapper), and cannot silently delegate
failed automatic tool discovery to the Provider. An explicit Read using project
tool action authorizes that tooling; explicit Save also owns its required read-back. The confirmed edit is
`{sourceID, revision, values:{<fieldID>:<new raw value or null>}}`; null removes only
that native override. Unchanged inherited values must not create overrides.

`build.configurationEditPlan` takes the same instance/tool/options context,
`snapshot`, confirmed `edits`, and a fresh, empty, **already existing** `outputFile`
inside the host's isolated workspace state. It validates the bounded field IDs,
types and revision, then plans `lithe/build/editConfiguration`.
The provider owns its native format and uses an upstream/mature reader/writer;
Maven XML, JSON and TOML do not add host format cases. The provider verifies the
source revision and writes replacement bytes only to that private staging file.
Open without CREATE or following a symlink, validate that it is the empty staging
file before writing, and never truncate another file. A cancelled operation cannot
recreate removed staging.
Preserve unrelated content, format-supported comments and secrets privately.

The reply is only `{format:"litheConfigurationEdit", version:0, sourceID,
revision, changed}`. `build.decodeConfigurationEdit` binds this receipt to the
same snapshot/draft and rejects extra fields. Full documents and private bytes
must never appear in RPC, UI, logs or model inputs. The native adapter validates
the staged bytes against the receipt SHA-256 before publication. A no-op creates
no source or native override. Providers never write the final source through
this workflow; publication remains the host's guarded atomic transaction.
The output uses the existing private outputs lifecycle, is never reusable across
worktrees, and is deleted on success, failure, cancellation, workspace/window
close and plugin revocation. A late backend may finish its already-open file
handle, but cannot resurrect a removed file. Installation resources stay read-only.

The platform adapter denies app/plugin installation roots, validates the selected
file and canonical target, preserves metadata and performs guarded atomic publication
against the original bytes. Before commit the operation must still own its workspace,
package, settings and monotonic option revision. Conflicts retain the draft. After
save the host invalidates build/test/language import facts and re-queries the owning
provider; read-back must identify the saved source and exact published revision.
A failed read-back is not a successful effective-value publication. Native probes,
output files and mutable provider caches live only in workspace/platform state and
are not cross-worktree reusable. Installed packages remain byte-for-byte unchanged.

Fixtures: [declaration](fixtures/build-providers/build-provider-configuration.plugin.json) and
[native facts](fixtures/build-providers/build-configuration.json).

### Optional read-only model inputs

A producer opts into `buildSystems[].modelSharing: { options: [<optionID>] }`.
Only normalized `projects`, `targets` and `contentRoots`, including their
ordinary public metadata, are published. Tasks, run entries, settings,
`optionValues`, process handles and private functions are excluded. The
options list names existing ordinary build options whose **effective values**
may be shared; omitted options stay private. A configuration fingerprint
identifies all effective options without publishing their values.

A consumer declares `modelInputs: [{ pluginID, buildSystemID }]`, requires
`buildModelInputs: 0` and `buildProviders: 0`, and owns a provider and model.
Slots are exact optional identities; Core sorts them by plugin ID and build
system ID and rejects duplicates. A producer cannot also consume
inputs, so consumers cannot recursively re-export a model.

The native host calls `build.resolveModelInputs` with the consumer manifest
and `{revision, sources: [{manifest, instances: [{buildSystemID, root, state,
model?, options?}]}]}`. Sources are enabled installed packages in the current
workspace activation, never identities supplied by another backend.
Input states are `ready`, `loading`, `stale`, or `failed`; only ready
instances include a complete normalized model. A truncated model becomes stale.
Core revalidates source manifests, ownership, shape and path containment and
returns `modelInputs: {version: 0, revision, inputs}`, where each input is:

```json
{
  "pluginID": "dev.example.source",
  "buildSystemID": "source",
  "state": "available",
  "instances": [{
    "root": "app",
    "state": "ready",
    "model": {"projects": [], "targets": [], "contentRoots": []},
    "configurationFingerprint": "<64 lowercase hex characters>",
    "options": {}
  }]
}
```

A missing, disabled, non-public or mismatched producer gives
`state: "unavailable", instances: []`. An installed public producer with no
detected instance is available with an empty list. A non-ready instance has
only `root` and `state`; no old model or options travel with it. Roots and
model paths remain workspace-relative, and instances are deterministically
ordered. There are at most 8 slots, 64 instances per source, 2,000 projects and
10,000 targets/content roots per instance, and 8 MiB of resolved JSON. The
revision is a nonnegative integer no larger than 9,007,199,254,740,991.

The host supplies this snapshot as the common plan request's optional
`modelInputs`. It is required exactly for a consumer with declared slots.
Every Provider plan copies it unchanged into `params.modelInputs`; Core
revalidates the snapshot and rejects extra fields or mismatched slots.
The running provider must have negotiated `litheBuild`; a session that has not
rejects an input-bearing request even for an existing method such as
`lithe/build/model`.

A revision belongs to one workspace activation. The host revokes consumer
model/dependency loads, discovery and in-flight execution/preparation when a
producer's model, options, watched configuration or package availability
changes. Revisions also detect changing an option away and back before a reply
arrives. Guard the result after decoding as well as after the RPC; do not
publish an old reply merely because cancellation was ignored. Already launched
applications retain their existing native ownership.

This facility never installs or activates a producer, invokes its private
functions, grants permissions, leases a process or resolves a package version.
There is no general plugin-dependency/export API. Backend authors must handle
unavailable and non-ready slots explicitly. Language integration remains
separate work. Shared fixtures:
`fixtures/build-systems/build-model-producer.plugin.json` and
`fixtures/build-systems/build-model-consumer.plugin.json`.

### Inline provider test reports

A provider test may declare `results: { "decoder": "provider" }`. Core adds
`testResults: true` to the test request. The reply includes
`testResults: { "format": "litheTestResults", "version": 0, "tests": [...] }`
next to `exitCode` and `output`. Ordinary execute replies may include the same
report when the task also ran tests. Missing required or malformed reports fail
explicitly; there is no host report-file fallback.

Each case has `suite`, `case`, `status` (`passed`, `failed`, `error`, `skipped`),
optional workspace-relative `project` and source `path`, one-based `line`,
`durationMillis`, `message`, and `invocations` (default 1). A report contains at
most 10,000 cases and 8 MiB UTF-8 JSON. Paths remain inside the selected build
root, durations are nonnegative safe integers, and messages are bounded. Core
validates and aggregates by project/suite/case, so identical names in distinct
projects remain distinct; it computes summary/success from the cases. Repeated
case durations are summed only if every invocation has a duration and the total
is a safe integer; otherwise the aggregate duration is omitted.

An optional `coverage: [{ "project": "module", "suite"?: "Suite", "case"?: "method" }]`
records completed scopes. A missing case covers the entire suite; a missing suite
covers the project (and forbids case). Scopes use canonical workspace-relative
projects inside the instance root, at most 10,000 entries, and bounded nonempty
suite/case names. Core sorts and deduplicates them. Hosts may replace old outcomes
within those scopes; without coverage they replace only the returned exact cases.
Providers must not claim a complete suite when upstream filters or interruption
could have limited execution. Opaque selectors are never parsed by hosts to infer
coverage.

The provider owns report discovery, freshness and upstream format semantics.
It must preserve reports from failing executions and reject unreadable/oversized
reports instead of returning empty success. The host passes the reply to
`build.decodeTestResults` as `request.result` with the same run/configuration
identity, checks ownership again after decode, and only presents the result.
Cancellation or configuration changes revoke late reports just like late output.
The report is operation state in a platform-provided writable directory, never
installed package state or a cross-worktree cache. Fixture:
`fixtures/build-providers/build-provider-reports.plugin.json`.

### Opaque semantic test selectors

Provider test declarations may advertise `selectorKinds`, up to 16 distinct public
namespace identifiers (120 bytes each). A test
request may then carry `selectorKind` and `selectors` instead of `suite`/`case`.
The namespace must be declared; at most 64 nonempty, control-free UTF-8 values of
4096 bytes each are allowed. An empty list explicitly selects all tests within the
request's normal project/workspace scope. Values remain opaque to the host and
Core: only the owning language engine supplies semantic identities and the build
backend translates them into its upstream runner's selection syntax. Providers
must reject identities they cannot represent exactly, rather than widen execution.

Ignoring a filter is unsafe, so Core rejects these fields before dispatch
unless the test declares the namespace. The operation identity, source/model
and configuration validity, cancellation, progress, and inline report rules
still apply. The shared example is `fixtures/build-providers/build-provider-selectors.plugin.json`.

### Declarative project preparation

A project task may declare `preparation: { "purpose": "run", "sourceRoles":
["source", "generatedSource"] }`. Purpose is
an opaque workflow identifier; `run` identifies preparation before resolving a
language launch. Roles are the canonical model content-root kinds. A provider
can declare a different task for `testSource`/`generatedTestSource`, with any
private task IDs. Hosts never prescribe task names or infer a role from a file
extension, build descriptor, class name or missing model entry.

Bindings are unique by purpose/role within each build system. Preparation
requires a project-scoped task, task execution, no argument prompt, and no
`fromModel`. Empty, overlapping, duplicate or unknown roles are rejected.
Hosts select the deepest owning project and its deepest content root from the
current model, then require exactly one declared matching task. Missing or
ambiguous declarations fail; they do not fall back to a conventional task ID.
The selected ID travels through the existing execute plan and backend method.
Shared fixture:
`fixtures/build-providers/build-provider-preparation.plugin.json`.

The build backend owns filtering, custom resource directories, profiles and
reactor semantics. Hosts await successful completion, retain one operation
identity through cancellation/configuration changes, and only then resolve and
launch the program. Per-run option/tool overrides are ephemeral host inputs to the
existing validated plan; they never mutate saved build defaults or claim that
the persistent model reflects a different configuration.

### Run configuration types

`runConfigurationTypes` adds kinds of run configuration to the Run/Debug
Configurations dialog, as IntelliJ IDEA's Go plugin adds Go Build and Go
Test. A type is data only: the fields of its editor and the command line
their values make. Core draws the editor, checks and saves the values, and
expands the command line, so a type needs no backend and the host has no
code for any type. A build system that declares types needs
`runConfigurationTypes: 0` in `requires` alongside the required `buildSystems` capability. Fixture:
`fixtures/run-configuration-types/`, translated by its `l10n/zh-Hans.json`.

```json
"runConfigurationTypes": [{
  "id": "build",
  "displayName": "Go Build",
  "icon": { "host": "run" },
  "fields": [
    { "id": "runKind", "title": "Run kind", "kind": "choice", "default": "package",
      "choices": [{ "value": "package", "title": "Package" }, { "value": "file", "title": "File" }] },
    { "id": "package", "title": "Package path", "kind": "text", "required": true,
      "visibleWhen": { "field": "runKind", "equals": "package" } },
    { "id": "file", "title": "Files", "kind": "file", "required": true,
      "visibleWhen": { "field": "runKind", "equals": "file" } },
    { "id": "race", "title": "Race detector", "kind": "checkbox", "arguments": ["-race"] },
    { "id": "output", "title": "Output name", "kind": "text", "arguments": ["-o", "${value}"] }
  ],
  "arguments": ["run", "${optionArguments}", "${field:race}", "${field:output}",
                "${field:package}", "${field:file}", "${userArguments}"]
}]
```

| Field | Meaning |
| --- | --- |
| `id` | Unique within the build system, like a task ID; saved in every configuration of the type, so renaming it orphans them |
| `displayName`, `description` | Shown in the dialog's `+` menu and as the editor's automatic name; at most 120 and 512 bytes |
| `icon` | A [UI icon](#ui-icons) for the menu and the list; display only |
| `execution` | `application` (default) or `task`, as for a task; `service` is refused |
| `fields` | At most 16, in display order between the editor's header and the program arguments |
| `arguments` | The command line after `globalArguments`. Besides `${root}` and `${optionArguments}` it takes `${field:<id>}`, a whole argument that expands to the arguments of one field, and `${userArguments}`, the program arguments. The editor shows its program arguments field only when `${userArguments}` appears, and it appears at most once. Conditions (`when`) are refused |

A field has an `id` (camelCase, unique in the type), a `title`, a `kind`, and
optional `description`, `placeholder`, `default`, `required`, `visibleWhen`
and `arguments`:

| `kind` | Value | Passed as |
| --- | --- | --- |
| `text` | One line | The value as one argument, or its `arguments` with `${value}` |
| `choice` | One of `choices` (`{value, title?}`, at most 64, required) | As `text` |
| `checkbox` | On or off | Its `arguments` (required, no `${value}`) while it is on, nothing while off |
| `file`, `directory` | A path the user picks; saved project-relative inside the project, passed absolute | As `text` |
| `arguments` | Text the user types, split like program arguments | The arguments themselves; `arguments` is refused |

- `default` is the value of a new configuration: a string for `text` and
  `choice`, a boolean for `checkbox`, a list of strings for `arguments`.
  `file` and `directory` have none, since a plugin cannot know a path on the
  user's machine. A `choice` starts with its first choice and a `checkbox`
  off when they have none.
- `required` refuses an empty visible `text`, `file`, `directory` or
  `arguments` field; a `choice` or `checkbox` always holds a value, so
  `required` on one is refused.
- `visibleWhen: {field, equals}` names an earlier `choice` or `checkbox`
  field and one of its values. A hidden field is not shown, saved, checked or
  passed, so the Go Build fields above pass exactly one of the package and
  the file.
- Every field is named exactly once as `${field:<id>}` in the type's
  `arguments`, except a field that declares no `arguments` and only controls
  the visibility of others, such as `runKind`.

An unknown, misplaced or repeated variable, or a field the command line
never names, is `plugin.template.invalid`; any other mistake is
`plugin.manifest.invalid`, with `field` naming it.

Every editor also has the fields every configuration has: the name, *Store
as project file*, *Allow multiple instances*, the working directory, the
environment variables and the steps before launch. *Allow multiple
instances* is off by default: starting a running configuration again stops
the running process first; with it on, the host starts another process
beside it. A configuration is saved with provider `build.run-type` in
`.lithe/run/plugin/<pluginID>.toml` (project) or
`<pluginID>.local.toml` (this machine) and keeps the type and the visible
fields' values in `extensions.build`, so removing the plugin keeps the
configuration, which needs the plugin again to edit or run.

To run one, the host calls `build.runConfigurationPlan` with the manifest,
the build system, `workspaceRoot`, the configuration ID and the detected
instances of the build system with their stored options. Core picks the
deepest instance whose root contains the configuration's working directory.
When that directory is the workspace root and no instance contains it, Core
can instead use the deepest instance containing the parent of the sole active
file field's resolved target, provided the target is a file inside the
workspace. Equivalent spellings of the workspace root, such as `./`, qualify
for this fallback. If neither path selects an instance, a single detected
instance still works. This choice does not change
the working directory. Core returns a task plan: the global arguments, then the
type's `arguments` with the field values, the instance's options and the
program arguments, at the configuration's working directory, with its
environment over the options'. The host starts it like any build command. A
saved value that no longer fits the type, such as a choice the plugin
removed, is `plugin.build.invalidRunConfiguration` with `field:
"values.<fieldID>"`; a type the plugin no longer declares is
`plugin.build.unavailable`.

The type's text is translated in the package's `l10n/<language>.json` under
`runConfigurationTypes.<buildSystemID>.<typeID>`: `displayName`,
`description`, and `fields.<fieldID>` with `title`, `description`,
`placeholder` and `options.<value>`, chosen and checked as in
[Translations](#translations). A file that does not fit is refused whole
with `plugin.build.invalidLocalization`, and the manifest's text is shown.

### Before-launch tasks

`beforeLaunchTasks` adds kinds of step to the *Before launch* list every run
configuration has, as IntelliJ IDEA's Go plugin adds Run Go Command to its
*Add New Task* menu. The host lists them in that menu after Run Another
Configuration; picking one opens a form of the task's fields, and the step
keeps their values. Before the configuration starts, each step runs and must
exit with 0. A build system that declares tasks needs `beforeLaunchTasks: 0`
in `requires` alongside the required `buildSystems` capability. Fixture:
`fixtures/before-launch-tasks/`, translated by its `l10n/zh-Hans.json`.

```json
"beforeLaunchTasks": [{
  "id": "command",
  "displayName": "Run Go Command",
  "description": "Runs one go command, such as generate ./..., in the configuration's working directory.",
  "icon": { "host": "run" },
  "fields": [
    { "id": "command", "title": "Command", "kind": "arguments", "required": true,
      "placeholder": "generate ./..." }
  ],
  "arguments": ["${field:command}"]
}]
```

A task is declared like a [run configuration type](#run-configuration-types),
with the same `id`, `displayName`, `description`, `icon`, `fields` and
`arguments` rules, at most 16 per build system, with two differences:

- A task always runs to completion, so it has no `execution`; declaring one
  is `plugin.manifest.invalid`.
- A task has no program arguments, so its `arguments` do not take
  `${userArguments}`; using it is `plugin.template.invalid`.

A task's ID is separate from the type IDs: a build system may declare a type
and a task with the same ID. A step saves the task's identity and the visible
fields' values in the configuration's `beforeLaunch` list as `{task:
{pluginID, buildSystemID, type, values}}`, so removing the plugin keeps the
step, which then refuses to start until the plugin is installed again.

A step runs in the working directory of the configuration whose list names
it, without that configuration's program arguments or environment. To run
one, the host calls `build.beforeLaunchTaskPlan` with the manifest, the build
system, `workspaceRoot`, the step and its working directory as
`runConfig.beforeLaunch` returned them, and the detected instances of the
build system with their stored options. Core picks the deepest instance
containing the step's working directory, or the only instance, and returns
a task plan: the global arguments, then the task's
`arguments` with the field values and the instance's options. A saved value
that no longer fits the task is `plugin.build.invalidBeforeLaunchTask` with
`field: "values.<fieldID>"`; a task the plugin no longer declares is
`plugin.build.unavailable`.

A step a project file names runs code from the repository, so the host asks
the user before running a list it has not seen, as for steps that start other
configurations. The text is translated under
`beforeLaunchTasks.<buildSystemID>.<taskID>`, shaped like
`runConfigurationTypes`.

### Build providers

Some tools expose their model only through their own API (the Gradle Tooling
API, Mix inside the BEAM). Their plugin declares `provider`: a long-lived LSP
base-protocol process of the same kind as a language server, which answers
`lithe/build/model` and `lithe/build/dependencies`. It has one of two forms,
told apart by the `languageServer` key; mixing them is
`plugin.manifest.invalid`.

| Form | Fields |
| --- | --- |
| Shared | `{ "languageServer": "<languageID>", "executesProjectCode", "mayUseNetwork" }`. The named language must have a language server in the same plugin; the host sends the requests to that session. |
| Own process | The language-server process fields (`launch`, `arguments`, `validationArguments`, `runtimeDependencies`, `initializationOptions`) plus `executesProjectCode` and `mayUseNetwork`. Validation, launch templates, settings, and the install review are those of a language server, so it needs the `languageServer.launch` permission and is listed as a `packagedBuildProvider`, `systemBuildProvider`, or `runtimeBuildProvider` program with its `buildSystemID` and flags. |

`model.source: "provider"` and `dependencies.source: "provider"` use it. A
provider no source uses, or a provider source without a provider, is
`plugin.manifest.invalid`. Detection never starts a provider: its instances
report `modelSource: "provider"` without a model.

The host starts the process from `plugins.declarative.languageServerLaunchPlan`
with `buildSystemID`. For a provider source, `build.modelPlan` and
`build.dependencyPlan` return `provider: { method, params }` instead of
`arguments`, with the request's `decoder` (`litheBuildModel` or
`litheDependencies`) and the declared `executesProjectCode` and
`mayUseNetwork`:

| Param | Meaning |
| --- | --- |
| `buildSystemID`, `root` | The instance. A model request always covers the whole instance. |
| `options` | Effective option values by option ID. |
| `optionArguments`, `environment` | Arguments from options, and selected runtime/tool environment merged with option contributions (options win). The host does not forward its complete environment. Providers use this environment for every invocation and cache identity. |
| `project` | Dependencies only: `{ name, path }` of the selected project, or `null` for the workspace. |
| `refresh` | Optional; `true` when the user asked to reload (Reload, Retry, a page's `build.refresh`) or the host reloads after a `sync` file changed. The provider must then recompute instead of answering from a cache or snapshot of its own. Absent or `false` for a first load, which a provider may answer from a snapshot it still trusts. |
| `tool` | Optional; `{ executable }`, the absolute path of the build tool the host resolved from the build system's `invocation` the same way a task run does (project wrapper, environment variable, `PATH`, the user's override), so the provider need not repeat the lookup. Absent when the host could not resolve it; the provider then looks the tool up itself. An explicit toolchain or path selection that cannot resolve fails before the Provider request; it cannot be dropped to trigger independent lookup. |

The Core-only `tool` hint may include a bounded `environment` string map. Core merges it into the existing Provider `environment` parameter, with option contributions taking precedence; the wire `tool` remains `{ executable }`. This also applies to native configuration, tasks and tests. The backend process runtime and the selected build runtime are independent.

The host passes `refresh` and `tool` in the `build.modelPlan` and
`build.dependencyPlan` requests; Core validates `tool.executable` as an
absolute path (`plugin.build.invalidSelection` otherwise) and copies both into
`params` only when they are set. Command sources ignore both.

The host passes the JSON result of the request as `result` to
`build.decodeModel` or `build.decodeDependencies`, which decode it like the
canonical format of a command; a missing result is
`plugin.build.invalidSelection`, and a result over the output limit or of
another format is `plugin.build.invalidOutput`.

The manifest only says how to start the provider and which sources use it.
Everything else is negotiated in the LSP `initialize` exchange, so a provider
can add optional data:

- The host sends `capabilities.experimental.litheBuild: { "versions": [0, 1] }`,
  the `buildProviders` versions it implements. The protocol has no version of
  its own: it is a version of `buildProviders`, and version 1 only adds
  `lithe/build/runEntrypoints`. It is the two requests above with the params and result
  formats of this section, together with the typed requests of the sections
  before it: task and test execution, test discovery, run preparation, native
  debug preparation, source-backed native configuration, inline test reports,
  opaque semantic test selectors, typed run options, and registry connection
  debugging, and, from version 1, run entrypoints. Read-only model inputs travel in the plan requests. A host presents
  the features it implements; a provider must not depend on a host calling any
  of them.
- The provider answers `capabilities.experimental.litheBuild: { "version": 1 }`
  with the version it chose from that list, the newest it speaks. A provider that answers no
  version, or one the host did not offer, is not used for build requests; a
  shared provider still serves its language.
- Both sides ignore fields they do not know in these objects and in request
  params and results, so either side can add optional data. A change the other
  side must not ignore gives `buildProviders` version `1`, and the protocol
  moves with it.

A host that declares `buildProviders` keeps providers apart:

- One process per plugin, build system, and workspace. Two plugins never
  share a process, and the host never relays messages between processes; each
  speaks JSON-RPC only with the host, over its own stdio.
- The shared form reuses a language server session of the same plugin only.
- A provider starts on the first build request, which only a user action
  sends, and stops when its workspace closes, its plugin is disabled, or
  before its package is replaced, rolled back, or uninstalled. A change of
  the plugin's settings restarts it on the next request, so its launch
  arguments always reflect the current values. A provider that exits is
  started again by the next request.
- Installation refuses a package whose provider launch points at a missing
  file, a path outside the package or through a symbolic link, or a packaged
  executable or runtime that is not a regular executable file.
- Its writable state lives in the plugin's state directory for that build
  system, never in the plugin package or the host's installation.

### Host integrations

`hostIntegration` is reserved. Core implements no build-system host
integration: every build tool is described by its plugin's data and backend,
and no host code branches on a build-system ID. Any `hostIntegration` name is
refused with `plugin.integration.unsupported` at
`buildSystems[<n>].hostIntegration`. Model, test-result, and dependency sources
have no `hostIntegration` kind: build systems use the declarative contract, a
canonical format, or a `provider`.

### Commands

The `build.*` Core commands (see `rust/contracts/rust-core-api.md`) take the
manifest, the build-system ID, and a request, and revalidate the manifest
every time. `build.detect` walks the workspace once for all enabled plugins
and returns instances, with the model of `static` sources. `build.invocationPlan` and `build.checkToolVersion` resolve the
executable; `build.localizeOptions` applies the `buildOptions` translations to the manifests before detection; `build.resolveOptions` checks stored option values and returns
each option's value, `source` (`user`, `default` or `unset`), choices,
`description`, and, for a `text`, `file` or `directory` option without a
declared `default`, the `implicitValue` the loaded model reported (also while
the user overrides it, as what removing the value returns to), with the
configuration fingerprint; `build.modelPlan`, `build.taskPlan`,
`build.testPlan`, and `build.dependencyPlan` return argv plans, or provider
requests for a provider source;
`build.decodeModel`, `build.decodeTestResults`, `build.decodeDiagnostics`,
and `build.decodeDependencies` normalize output or a provider result; `build.syncFingerprint`
digests the watched files. A selection the declaration does not allow is
`plugin.build.invalidSelection`; an operation it does not declare is
`plugin.build.unavailable`.

## VCS providers

`vcsProviders[]` declares version control systems the plugin implements, each
through its own LSP base-protocol process: the VCS ID, the static markers that
identify a root, metadata paths the host watches, the VCS client the provider
drives, and the shared [process launch](#process-start)
fields. The ID `git` is reserved for the built-in support
(`plugin.vcs.reserved`). Capabilities belong to the running process and are
negotiated in `initialize` under `experimental.litheVcs`, never declared in
the manifest. The host offers `{ "versions": [0] }`: the protocol's version is the
version of `vcsProviders`.

Detection never starts a process: `vcs.detectRoots` finds roots from the
markers, and the host starts a provider only after a root of its VCS is
found. Directory markers, such as `.svn`, are the VCS's metadata: while the
plugin is enabled every workspace hides them like the folders a build system
lists in `layout.hiddenDirectoryNames`, except that Search Everywhere never
lists their file names. The install review lists the provider as a `…VcsProvider` program with
its VCS client (`vcsTool`) and `mayUseNetwork`. Requests go through
`lsp.vcsRequest` by operation name and replies are normalized by
`vcs.decodeReply`. The fields, operations, reply shapes, and error codes are
defined in [the VCS provider contract](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/contracts/vcs-provider-v1.md).

## Debug adapters

`debugAdapters[]` declares debug adapters the plugin implements. Each one is a
DAP process the host starts and talks to over stdio or a loopback TCP port, or
a debug server hosted by the plugin's own language server.
The declaration is data only: there is no resolver, workflow, host integration,
or `initializationOptions`, and any such field is refused. It requires
`debugAdapters: 0` in `requires`, `debug.launch` (plus `debug.attach` when an
adapter serves `attach`), and one `onDebug:<id>` activation event per adapter.

| Field | Meaning |
| --- | --- |
| `id`, `displayName` | Plugin-local ID (unique in the plugin) and its title. |
| `adapterID` | DAP `initialize.adapterID`; defaults to `id`. |
| `requests` | `launch` (default) and/or `attach`. |
| `selectors[]` | Optional automatic matching by `targetKinds` and `languageIDs`; absent means explicit selection only. |
| `configurationTypes` | Imported `launch.json` `type` values the adapter answers to. |
| `configurations.launch/attach` | Default DAP `arguments`, a packaged `parametersSchema`, and `targetBindings` (`program` ← `target.path`, or ← `target.directory`, the directory containing `target.path`, for an adapter that debugs a package such as Delve, `processId` ← `target.processID`, `cwd` ← `target.workingDirectory`, or ← `target.directory` for an adapter whose program runs where its package is, and for `attach` only `connection` ← `target.host`, `target.port`, `target.pathMappings`; also bounded `args` ← `target.arguments` and `env` ← `target.environment`; see [Remote attach](#remote-attach)). A configuration for an undeclared request is refused. |
| `targetBindings.adapterArguments` | Up to 8 DAP arguments only this adapter defines, as `{"<argument path>": "<fact>"}` with the fact `target.path`, `target.directory` or `target.workingDirectory`. A path is dotted like a `connection` destination: up to 4 names, each a letter or `_` followed by up to 63 letters, digits, `_` or `-`, and objects on the way are created. For example Delve builds the program in `dlvCwd`, not in its own working directory, which the host sets to the workspace root, and `go build` reads the directory from `PWD`, which Delve leaves at that root; so a Go plugin binds `{"dlvCwd": "target.directory", "env.PWD": "target.directory"}` here, and `cwd` ← `target.directory` so the program also runs in its package directory rather than in the workspace root the debug engine gives a launch without `cwd`. Inside `env` a path names one variable and adds it to the environment the configuration or the `env` binding gives; elsewhere a path may not start at `program`, `processId`, `cwd`, `args` or `env`, which have their own bindings, or at `noDebug` or a `__` name, which belong to the host. Two paths may not overlap, nor a path and a `connection` destination, and a path may not land on or under a value the configuration's own `arguments` set, which the binding could never fill. Like every binding it fills the argument only when nothing holds it, so a variable the target's environment sets stays, and a target without the fact is refused with `debug.target.invalid`. |
| `configurations.launch.executable` | For a target that is a source file the adapter cannot debug directly (C, C++, Swift): `setting` names a `file` setting of the plugin, `candidates[]` (up to 8) are the paths a host looks for when the user chose nothing. Each candidate starts with `${workspaceFolder}/` or `${fileDirectory}/`, may use `${fileBasenameNoExtension}`, and has no `..`. Declare it together with `targetBindings.program: "target.executable"` and neither without the other; a launch fills `program` with the path the host settled on. The user's setting always wins: a chosen path that is not a runnable file is an error, not a reason to try the candidates. The host shows the path it found in that setting, and the setting's `description` is where the plugin teaches how to produce the program. |
| `capabilities.disabled` | Standard DAP `supports*` names or `exceptionBreakpointFilters` the host treats as absent. |
| `features` | Adapter-specific DAP extensions the adapter implements; see [Adapter features and defaults](#adapter-features-and-defaults). |
| `defaults` | Values the adapter starts with when the user configured none. |
| `console.commandPrefix` | Text (1–8 characters, no spaces; for lldb-dap a backtick) the adapter's console reads as "what follows is a debugger command". The host puts it before the typed line only when it asks the adapter to complete, and moves the adapter's answer back, so the user never sees it. An adapter that treats a bare word as an expression (lldb-dap) otherwise mixes language keywords such as `long` into the first word's completions; the prefix limits them to commands. Declare at least this or `console.commands[]` when `console` is present. |
| `console.commands[]` | Debugger commands (`name`, optional one-line `description`) the Debug Console offers as completions. An adapter that answers the DAP `completions` request completes live, so list commands only for an adapter that does not (or whose live answers lack some commands); the host merges both. Names are inserted as written, unique, and not padded with spaces; descriptions are shown as written and not translated. |
| `launch.process` | The shared [process launch](#process-start) fields without `initializationOptions`, or `{"launch": {"kind": "runSelectedRuntime", "tool": "<tool-definition ID>"}}`, which starts the adapter with the executable the user's runtime selection resolves for that tool; the ID is listed in `requiresTools` (see [Tool references](#tool-references)). Required for every transport except `tcpLanguageServer`, which must not have it. |
| `launch.transport` | `stdio`, `tcpAnnounced` (`endpoint`: `stream`, one-line `prefix`, `value` = `port` or `hostPort`), `tcpPort` (`endpoint.port`), or `tcpLanguageServer` (`endpoint`: `languageID`, `command`). TCP endpoints are loopback only. |
| `launch.endpoint` for `tcpAnnounced` | The host reads the adapter's `stream` line by line. The first line that contains `prefix` is the announcement: the rest of that line after the prefix, without surrounding whitespace, is a port from 1 to 65535 (`port`, on `127.0.0.1`) or `host:port` with a loopback host, IPv6 in brackets (`hostPort`, for example Delve's `DAP server listening at: 127.0.0.1:54321`). Any other announcement fails the start, and so does no announcement within 10 seconds. The adapter's other output goes to the Debug Console. |
| `launch.endpoint` for `tcpLanguageServer` | For an adapter that lives inside a language server and has no process of its own (the Java Debug Server runs in JDT LS). `languageID` names a `languageServers[]` entry of the same plugin; `command` (1–128 characters of letters, digits, `.`, `_`, `:`, `-`) is the `workspace/executeCommand` the host sends to that server for each session. The server answers with the loopback port to connect to, as an integer or a numeric string from 1 to 65535. The host starts the language server for the workspace first, as it does for editing. The command is part of what the user reviews at install and update, and changing it asks again. |

### Adapter features and defaults

A host never recognizes an adapter by its ID. What is specific to an adapter
is declared, and the shared debug engine follows the declaration:

```json
"features": {
  "hotCodeReplace": {
    "request": "script.reload",
    "event": "script.reloaded",
    "changedField": "modules",
    "errorField": "error"
  },
  "stepFilters": { "argument": "stepFilters" }
},
"defaults": {
  "stepFilters": { "classNameFilters": ["_vendor.*"], "skipSynthetics": true }
}
```

| Declaration | Meaning |
| --- | --- |
| `features.hotCodeReplace.request` | The DAP command the host sends to apply changed code to the running debuggee (the standard `redefineClasses` inspection). Present means the adapter supports hot code replace; absent means it does not. |
| `features.hotCodeReplace.event` | A DAP event whose `message` the Debug Console shows as progress. |
| `features.hotCodeReplace.changedField`, `errorField` | Fields of the response body: the list of replaced units, and a failure message the adapter reports in a successful response. A response without `changedField` replaces nothing visible. |
| `features.stepFilters.argument` | The launch argument through which the adapter receives stepping filters, shaped `{skipClasses, skipSynthetics, skipStaticInitializers, skipConstructors}`. An argument the user's configuration already sets is kept. |
| `defaults.stepFilters` | `classNameFilters` (up to 256 single-line patterns of at most 256 characters), `skipSynthetics`, `skipStaticInitializers`, `skipConstructors`, `hideFilteredStackFrames`. Used when neither the launch nor the user's settings name filters. |

Names (`request`, `event`, fields, `argument`) are a letter followed by up to
63 letters, digits, `.`, `_`, or `-`. A declaration with a malformed name, or a
field the contract does not define, is `plugin.manifest.invalid` with the
`field` that names it. The launch plan's `debugAdapter` descriptor carries
both so the host can pass them to `debug.createSession`.

### Remote attach

An adapter that attaches to a debuggee on another machine (a JVM started with
`-agentlib:jdwp`, a Node inspector, gdbserver) is an
ordinary `attach` adapter whose `targetBindings` carry a `connection`. The
adapter itself still runs on this machine, stdio or loopback as before; only
the debuggee is remote. This is the adapter-to-debuggee endpoint, not the
IDE-to-adapter DAP endpoint. By default, debugpy `--listen` starts a DAP adapter
on the target machine: the IDE must connect to that adapter, not start another
local adapter and merely pass it `connect.host` and `connect.port`.

`connection` does not declare an external DAP endpoint or remote adapter
execution (for example `ssh host gdb -i=dap`). These require separate host
integration; this contract and its validation do not establish that support.
The current transport scope is ordinary TCP connection and lifecycle, reusing
the existing socket implementation and retaining stdio. It does not add proxy,
tunnel, port-forwarding or SSH execution settings, or relax the loopback rule
for language-server-provided DAP endpoints. Adapter-native source mapping should
be reused where applicable, without translating the same path pair again in Core.

The following illustrates nested argument binding and mapping shape, not a
complete debugpy `--listen` connection configuration:

```json
"configurations": {
  "attach": {
    "arguments": { "timeout": 5000 },
    "targetBindings": {
      "connection": {
        "host": "connect.host",
        "port": "connect.port",
        "pathMappings": { "shape": "objects", "argument": "pathMappings",
                          "localKey": "localRoot", "remoteKey": "remoteRoot" }
      }
    }
  }
}
```

- The values of `connection` are paths into the adapter's `arguments`, because
  adapters name and nest them differently: java-debug takes `hostName` and
  `port`, nested arguments may use `connect.host` and `connect.port`, js-debug
  `address` and `port`, gdb one `host:port` string. A path is 1–4 dot-separated names of
  letters, digits, `_`, and `-` (up to 64 characters each) and Core creates the
  intermediate objects.
- `host` and `port` are declared together. `address` takes the endpoint as one
  `host:port` string, with an IPv6 host in brackets (`[2001:db8::1]:2345`); it
  stands in for `host` and `port`, or is a further destination beside them.
  At least one of the two forms is required.
- `pathMappings` is optional and has four shapes, one per way adapters take
  source roots:
  - `objects` fills a list of objects, one per mapping: `argument` names the
    list, `localKey` and `remoteKey` the keys of each object
    (`[{ "localRoot": …, "remoteRoot": … }]`).
  - `first` is for an adapter with a single pair of roots and fills `local`
    and `remote` from the one mapping. It takes exactly one: a target with
    more is refused rather than truncated.
  - `dictionary` fills an object at `argument` whose keys are the roots of one
    side and whose values are the other (`"keyedBy": "remote"` gives
    `{ "<remote>": "<local>" }`). A target with the same root twice on the
    keyed side is refused, since an object holds one value per key.
  - `pairs` fills a list of two-element arrays at `argument`, in the order
    `"order": "remoteLocal"` (`[["<remote>", "<local>"]]`) or `"localRemote"`.
  An adapter whose mapping is none of these declares no `pathMappings`, and a
  start that sends mappings to it is refused.
- A destination may not equal, contain, or sit inside another destination, the
  `processId` or `cwd` argument a declared binding of the same configuration
  fills, or a name that the configuration's own `arguments` already sets. Defaults win over bindings, so without this rule
  the adapter would connect somewhere other than the endpoint the user
  confirmed. A manifest that does it is `plugin.manifest.invalid`.
- `connection` is allowed under `attach` only. `selectors[].targetKinds` may
  list `connection`, and only for an adapter that declares the binding.

The target of a start is `{ "kind": "connection", "host", "port",
"pathMappings"?: [{ "local", "remote" }] }`. `host` is a DNS name or an IP
literal of 1–253 characters, with labels of up to 63 letters, digits, and
hyphens, none leading or trailing: no scheme, port, brackets, whitespace, or
user. A dotted run of digits must be a valid IPv4 address, and IPv6 zone IDs and
internationalized names (use punycode) are not accepted. `port` is 1–65535.
There are at most 16 mappings; each root is 1–1024 characters without control
characters, and a `local` root must be an absolute path on the host's
platform.

`debug.adapters.startArguments` answers with the filled `arguments` and a
`connection: { host, port, remote }`. `remote` is `false` only for what
Core can prove local from the text: `localhost` (any case), `127.0.0.0/8`, and
`::1`. Every other host, including `0.0.0.0`, `::ffff:127.0.0.1`, and
`localhost.example.org`, is `remote: true`. A host asks the user to confirm each
`remote` endpoint it has not been told to trust, and does not remember the
answer by default. `debug.attach` covers remote attach; there is no separate
permission. `plan.debugAdapter.remoteAttach` in the launch plan is
`{ "pathMappings": boolean, "maxPathMappings": number }` for an adapter that
takes a connection (`maxPathMappings` is 0 without mappings, 1 for `first`, and
16 otherwise) and `null` otherwise, so a host knows whether to offer the
host-and-port form and the mapping list, and how many rows the list may hold.

The cases in `fixtures/debug-adapters/debug-adapter-rejections.json` state the stable code and
field for each refusal and the layer that reports it: `schema`,
`coreManifest`, `corePlan` (one start: stale registration or runtime
selection, a target missing a fact a binding reads; the request defaults to
`launch` and `context.request` sets `attach`), and `coreInstall`
(a `parametersSchema` that is missing or links outside the package).
`fixtures/debug-adapters/debug-adapter.plugin.json` is a valid example, and
`fixtures/debug-adapters/debug-adapter-remote-attach.plugin.json` declares six adapters that
differ in how they take a connection.

## Output link matchers

`outputLinkMatchers[]` lets a plugin make text in the Run and Test consoles
clickable, such as a traceback frame or a panic location. The host knows no
tool's output format: the plugin declares the pattern and Core finds the
matches. Requires `outputLinkMatchers: 0` in `requires`. Matchers alone are a
complete plugin: one that declares nothing else is valid, and matchers without
`resolve` need no backend.

```json
"outputLinkMatchers": [
  { "id": "traceback", "pattern": "File \"(?P<path>[^\"]+)\", line (?P<line>\\d+)", "scope": "run" }
]
```

| Field | Meaning |
| --- | --- |
| `id` | Plugin-local ID: a lowercase letter followed by up to 63 lowercase letters, digits, `.`, `_`, or `-`; unique in the plugin. At most 16 matchers per plugin. |
| `pattern` | A regular expression of 1 to 512 bytes matched against one console line. It uses the Rust `regex` syntax, so it has no backreferences and no look-around, and its compiled size is limited. It names at most 8 groups, each a camelCase name, and must not match empty text. |
| `scope` | `run` or `test`: the console the matcher applies to. Absent means both. |
| `resolve.function` | Optional ID of a function of the plugin's `backend` process. Needs `backend`. |

Without `resolve`, the pattern must capture a `path` group and may capture
`line` and `column` (1-based); no other group name is accepted. A relative
`path` becomes one candidate per workspace root, an absolute one counts only
inside a workspace root, and a path that leaves every root is not linked: a
console line can never open a file outside the workspace. With `resolve`, the
pattern may name any groups. The host calls the function when the user
clicks, with `{groups}` (the captured text by name) and the window's `context`,
and the backend answers `{uri, range?}` or `null`.

Core owns the decision, so every host links the same text:

- `outputLinks.match` takes `{scope, platform, caseSensitive?, workspaceRoots,
  sources: [{pluginID, matchers}], lines}` and returns `{links}`. Each link is
  `{line, start, length, pluginID, matcherID}` plus either `location` (`{paths,
  line?, column?}`: the host opens the first existing path) or `resolve`
  (`{function, groups}`). `start` and `length` count UTF-16 code units of the
  line. Matches are found in source order and then declaration order, and a match
  overlapping an earlier link is skipped. Core then links the `http` and
  `https` addresses a line prints, which no plugin declares: such a link is
  `{line, start, length, url}` with no plugin, the host opens `url` in the
  browser, and an address inside a plugin's link stays part of that link. An
  address ends at whitespace, a quote or an angle bracket, without the
  sentence punctuation after it or a closing bracket it did not open. The links
  of a line are returned in the order they start. A request carries at most 256
  lines, 64 sources and 16 roots; a line is scanned up to 4096 bytes and links
  at most 8 times.
- `outputLinks.decodeResolution` takes `{reply}` (what the function answered)
  and returns the location to open, `{path, line?, column?}` with 1-based
  positions, or `null`. A reply opens only a `file:` URI and only the members
  `uri` and `range` are accepted; a 0-based `range.start` is converted.

The host runs matching only for lines it displays, never for output a plugin
has not been enabled for, and ignores a link whose file does not exist. Fixtures:
`fixtures/run/output-link-matchers.json` and `fixtures/run/tool-reference-launch.plugin.json`.

## Plugin backend

A plugin with its own pages may need code that a page cannot run: database
drivers, network clients, file formats. `backend` declares one long-lived
process that serves only the plugin itself, independent of any build system,
language, or workspace:

```json
"pluginAPIVersion": 0,
"requires": { "backend": 0, "ui": 0 },
"permissions": ["backend.launch", "credentials.store", "ui.contribute", "ui.web"],
"backend": { "launch": { "kind": "packaged", "path": "bin/backend" }, "arguments": ["--state", "${stateDir}"] }
```

- `backend` holds exactly the [process launch](#process-start) fields, with the
  same templates and install review; the review lists it as `packagedBackend`,
  `systemCommandBackend`, or `runtimeBackend`. It needs `requires.backend: 0`
  and the `backend.launch` permission. `fixtures/backend/backend.plugin.json` is
  the example; the backend cases of `fixtures/manifest/invalid-manifests.json` are the
  rejections.
- It belongs to no workspace: `${workspaceStateDir}` is refused, and its
  writable state is `${stateDir}`. The host starts it with `${stateDir}` as
  root and working directory.
- The host keeps at most one backend process per plugin, starts it on the first
  trigger listed under [Backend lifecycle and workspace contexts](#backend-lifecycle-and-workspace-contexts),
  and ends it when the plugin is disabled, updated, or uninstalled, and when the
  host quits.
  Its consent is the install review: it needs no per-workspace prompt.
- The backend speaks the [plugin-function protocol](#plugin-functions) and may
  ask the host for the [host services](#host-services) its permissions grant.
  It owns no domain contract: it never answers `lithe/build/*` or VCS
  requests.
- A plugin publishes one archive per platform, so its packaged path does not
  vary by `${arch}`; a macOS archive may carry a universal binary.

### Backend lifecycle and workspace contexts

A backend may start without a page and learns about workspaces only through
contexts. `fixtures/backend/backend-workspace.plugin.json` is the example.

- It may start without a page. It needs at least one way to start: a web
  tool window (first page call), a `ui.commands[]` entry
  (its `onCommand:<id>` also starts the backend), the `onWorkspaceOpen`
  event, or a provider the host queries: a toolchain or keymap import
  provider, or a language server with `launch.kind: "backend"`, whose start
  asks the backend for its command line. A backend with none of them is
  `plugin.manifest.invalid`.
- It is one process per plugin, shared by every open workspace and
  window, on the machine that runs Lithe, also for remote workspaces. No
  workspace is process state: each call names its workspace explicitly
  (a context, below), so `${workspaceStateDir}` is refused.
- Starting on `onWorkspaceOpen` never delays opening the workspace. The host
  starts such backends after the window is shown, one plugin at a time, and
  a backend that does not finish `initialize` within its initialize timeout
  is stopped and reported like any failed server start. A failed or
  disabled backend never blocks the workspace.
- The `workspace.metadata` permission grants the
  [`workspace` host service](#workspace-contexts-and-capability-discovery).
  It reveals the open workspaces' names, root folders and IDs, their
  authority, and the plugin's per-workspace state directory; it reads no
  file content.
- Activation is gated only by the plugin being installed, enabled, and
  reviewed. A build operation that runs project code also needs a trusted
  workspace (see [Workspace trust](#workspace-trust)); any other operation that
  runs or changes something asks the user per operation, as the host's own
  controls do.

## Project generators

`projectGenerators` (requires `projectGenerators: 0` in `requires` and the
`project.generate` permission) adds entries to the host's New Project window. The host owns the
window: the entry list and its search, the project name and location, the
"create Git repository" option, the empty project, and the Create button. A
generator declares only its own fields, the languages and build systems the
result needs, and the steps that produce it. No host code branches on a
generator ID.

```json
"projectGenerators": [{
  "id": "spring-boot",
  "displayName": "Spring Boot",
  "section": "generator",
  "icon": {"path": "icons/spring-boot.svg", "mode": "original"},
  "fields": [
    {"id": "groupId", "type": "text", "title": "Group", "default": "com.example", "pattern": "[a-z][a-z0-9.]*"},
    {"id": "packageName", "type": "text", "title": "Package name",
     "default": "${field.groupId}.${project.name|package}", "pattern": "[a-z][a-z0-9.]*"},
    {"id": "jdk", "type": "tool", "title": "JDK", "tool": "java", "minimumVersion": "17"}
  ],
  "requirements": [{"languageID": "java"}, {"buildSystemID": "maven"}],
  "steps": [{"kind": "archive", "url": "https://start.spring.io/starter.zip",
             "query": [{"name": "packageName", "value": "${field.packageName}"}]}]
}]
```

| Field | Meaning |
| --- | --- |
| `id` | Lowercase letters, digits, and hyphens; unique in the plugin. |
| `section` | `project` lists the entry under "New Project" (a plain project of a language); `generator` lists it under "Generators" (a framework or scaffolding tool). |
| `order` | 0–1000, default 100. Entries are sorted by section, `order`, then name. |
| `fields` | The generator's inputs, drawn and evaluated in order. A field may read only fields declared before it. |
| `requirements` | `languageID` or `buildSystemID` (optionally `when`) the created project needs. The catalog lists the installed plugins that provide each, with their enabled state, so the host can offer to enable one or open the marketplace. A requirement never blocks creation. |
| `steps` | Run in order in a staging directory that becomes the project root. |
| `openFiles` | Project-relative files the host opens after creating the project, optionally with `when`. |

### Fields

| `type` | Value | Properties |
| --- | --- | --- |
| `text` | A string | `default` (a template over `${project.name}` and earlier fields, re-evaluated until the user edits the field), `pattern` (matched as a whole), `maxLength` (default 256, at most 1024), `placeholder`, `optional`, `maximumVersion` |
| `enum` | One option | `options` (1–16 `{value, title}`), `default` (the first option otherwise), `presentation` (`segmented` or `menu`), `maximumVersion` |
| `boolean` | `true` or `false` | `default` |
| `multiSelect` | A list of options, kept in declaration order | `options` (1–128, each with an optional `group` heading), `default` |
| `tool` | An installation of a [tool definition](toolchains/README.md), by home directory | `tool` (a tool definition ID), `minimumVersion`, `pinAs`, `optional`. See [Tool fields](#tool-fields). |

A field type is `text`, `enum`, `boolean`, `multiSelect`, or `tool`; any other
type is `plugin.manifest.invalid`. `minimumVersion` is allowed only on a `tool`
field, and `optional` only on a `text` or `tool` field.

`maximumVersion` is a template over earlier fields naming the highest version
the value may be, compared numerically by dotted components. It reads
`${field.<id>.major}` of a `tool` field: a target language level that must not
exceed the selected compiler declares `"maximumVersion": "${field.compiler.major}"`.
While the bound expands to an empty string (no tool chosen yet) nothing is
checked.

Every field takes `id` (lowerCamelCase), `title`, `description` (shown behind
an info tip), `advanced` (listed under the collapsed "Advanced Settings"
group), and `visibleWhen`. A hidden field is not validated, and templates and
conditions read it as absent.

Text values never start with `-` and never contain control characters, so a
value cannot become an option or split a line. A project name is one
directory name: not empty, at most 128 characters, no `/`, `\`, `:`, or
control characters, not `.` or `..`, no leading `-`, and no surrounding
spaces.

### Tool fields

A `tool` field lets the user pick one
installation of any tool that a plugin declares in
[`toolDefinitions`](toolchains/README.md), such as a Python interpreter, a Node.js
runtime, or a JDK. The generator names no host feature and no language: the
host lists installations through its generic tool discovery and the user's
tool settings, exactly as the tool's own settings page does.

```json
{"id": "sdk", "type": "tool", "title": "Python", "tool": "python",
 "minimumVersion": "3.11", "pinAs": "project"}
```

| Property | Meaning |
| --- | --- |
| `tool` | Required. A tool definition ID: a short ID such as `compiler` defined by exactly one enabled plugin, or `<pluginID>:<id>`. There is no closed list; a tool nobody declares has no installations, and neither has a short ID several enabled plugins define, because it belongs to none of them. |
| `minimumVersion` | Optional, one to three dotted numbers. An installation below it is not selectable. |
| `pinAs` | Optional. `project` saves the selection as the created project's own tool choice (the same per-directory pin the tool's settings page edits), so the project builds and runs with the installation picked here. Without it the selection only feeds the steps. |
| `optional` | Optional. Without a usable installation an optional field stays empty and leaves no pin; otherwise the form reports `required`. An installation below `minimumVersion` is not usable. |

A tool field has no `default`; the host chooses the initial value. Core picks,
among the installations the host listed, the user's choice if it is still
listed and otherwise the newest version (the host's first among equal
versions). Versions compare as releases, so trailing zeros do not count:
`3.11`, `3.11.0`, and `3.11.0.0` are equal. An installation below
`minimumVersion` is marked in the field's `installations`, so the host can
show it disabled. When the selection is below `minimumVersion` the form
reports `toolTooOld`, except that an optional field the user has not set
stays empty rather than picking an installation that is too old.

### Conditions

`when` and `visibleWhen` take one condition or a list that must all hold:
`{"field": "<id>", "equals" | "notEquals": <value>}` or `{"field": "<id>",
"oneOf": [<value>, …]}`. Values are option values for `enum`, `true`/`false`
for `boolean`, and strings for `text` and `tool` (`notEquals: ""`
tests that an optional field is filled; a tool field compares as its home
directory). A condition on a hidden field is false.

### Templates

Strings in the manifest use `${…}`: `${project.name}`, `${field.<id>}` (a
text or option value, `true`/`false`, a comma-separated list, or the home
directory of a tool), `${field.<id>.version}` and `${field.<id>.major}` (tool
fields only: the version of the chosen installation as its definition's
`version.pattern` captured it, and that version's first numeric component,
`21` for `21.0.4`; a definition whose tool uses another numbering, such as a
legacy `1.` prefix, normalizes it in the pattern), and `${field.<id>.executable}`
(tool fields only: the path of the tool's executable in the chosen
installation). The three suffixed variables are rejected when the manifest is
validated if the field is not a `tool` field. Modifiers follow `|` and apply
in order: `path` turns dots into `/`, `package` makes a valid package name
(`com.Example.my-app` → `com.example.myapp`), and `lower` lowercases.

Template files use `{{ … }}` with the same variables, because POMs, Gradle
scripts, and properties files already contain `${…}` that must reach the
project unchanged. Only `{{` followed by `project.` or `field.` is a
variable; any other `{{` is copied as text. A file name may also contain
`${…}` variables, although a step's `target` is the portable place for them.
A path segment that starts with `dot-` becomes a dotfile (`dot-gitignore` →
`.gitignore`), because packages keep no hidden files. Hidden files in a
template directory are skipped; symbolic links are refused.

### Steps

| `kind` | Properties | The host… |
| --- | --- | --- |
| `files` | `source` (package-relative directory), `target` (project-relative directory template), `when` | writes the files of the plan, setting the executable bit where the template file has one. |
| `command` | `command` (a bare executable name), `arguments` (strings or `{value, when}`), `environment`, `collect`, `mayUseNetwork`, `timeoutSeconds` (default 600, at most 1800), `when` | resolves the executable like a build tool, runs it with the staging directory as the working directory, and, with `collect`, moves the contents of that subdirectory up into the project root. Arguments that expand to an empty string are left out. |
| `archive` | `url` (no query or fragment), `query` (`{name, value, when, omitEmpty}`), `when` | downloads the zip and calls `project.extractArchive` into the staging directory. |

Archive addresses must expand to HTTPS URLs (HTTP only for `localhost`,
`127.0.0.1`, or `[::1]`) without credentials. The URL may come from a field,
such as a configurable Spring Initializr server; the host shows the server
before creating. Template directories are limited to 1024 files, 1 MiB per
file, and 8 MiB in total; archives to 20,000 entries and 512 MiB extracted.

### Commands

| Command | Payload | Result |
| --- | --- | --- |
| `project.generatorCatalog` | `{plugins: [{manifest, enabled}], hostCapabilities?}` | `catalog: {generators: [{pluginID, pluginName, pluginEnabled, generator, requirements: [{languageID or buildSystemID, providers: [{pluginID, pluginName, enabled}]}], tools: [{fieldID, tool, providers: [{pluginID, pluginName, enabled}]}], usesNetwork}], issues: [{pluginID, error}]}` |
| `project.generatorForm` | `{manifest, generatorID, form}` | `form: {projectName: {problem}, fields: [{id, visible, value, problem?, installations?}], activeRequirements, valid}` |
| `project.generationPlan` | `{manifest, generatorID, packagePath, form}` | `plan: {projectName, steps, openFiles, toolPins: [{field, tool, pinAs, home, executable}], usesNetwork}` |
| `project.checkName` | `{projectName}` | `name: {problem}` |
| `project.extractArchive` | `{archivePath, destination}` | `extracted: {files}` |

`form` is `{projectName, values, edited, tools}`. `values` maps field
IDs to values; a text value counts only when its ID is in `edited`, so derived
defaults keep following the fields they read. `tools` maps tool field IDs
to the installations the host found for the field's tool, in the order the
host prefers them, as `[{home, executable, version}]`; the field's `values`
entry is the `home` of the user's choice. A tool field in the result carries
`installations: [{home, executable, version, meetsMinimum}]`, so the host
draws its picker from Core's answer, and its `value` is the installation Core
selected. A field `problem` is one of `required`, `tooLong`, `invalidFormat`,
`unsupportedValue`, `toolTooOld`, and `versionTooHigh`; hosts
localize by code. Plan steps are `{kind:
"writeFiles", files: [{path, contents, executable}]}`, `{kind: "command",
command, arguments, environment, collect, mayUseNetwork, timeoutSeconds}`, and
`{kind: "archive", url, host, format: "zip"}`. `toolPins` lists, for each
visible tool field with `pinAs` and an installation, the tool and the chosen
`home` and `executable` that the host saves as the project's pin.

The catalog entry's `tools` lists, for each tool field, the plugins that
declare the field's tool and whether they are enabled, in the same way as
`requirements`: the host can offer to enable one, or open the marketplace,
when the field has no installation. Core only names declarations; whether a
definition is trusted, and whether the tool is installed, is decided by the
host's tool registry and discovery.

After the steps, the host moves the staging directory to its final location
(refusing an existing one), saves the `toolPins` as the project's tool
choices through the same store the tool settings page edits, creates the Git
repository when asked, and opens the project. A failure or cancellation
removes the staging directory and leaves nothing behind
(`plugin.project.generationFailed`).

## Plugin functions

A plugin backend (a language server, build provider, or
[plugin backend](#plugin-backend) process the plugin starts) can offer
**plugin functions**: actions with
serializable params and results. A function is private to its plugin unless
the manifest exports it: the host invokes a private function on behalf of the
same plugin, for example from that plugin's own window, and another plugin
can call only an [exported function](#cross-plugin-calls).
Functions complement the
typed domain protocols; they never replace them. The protocol is implemented in
Rust Core (`rust/crates/plugin-rpc/src/` and
`rust/crates/plugin-function-relay/src/lib.rs`).
A host starts every build provider process as a session of the plugin that
owns it and invokes its functions from that plugin's web pages (the
`functions.*` bridge methods below). A provider served by a shared language
server is started by the host for every plugin of its language, so it serves
no plugin's functions. The
[official SDKs](#official-backend-sdk) implement the backend side.

### Capability ownership

Every capability has one owner, who defines its contract, and one direction:

| Direction | Contract owner | Implemented by | Travels as | Examples |
| --- | --- | --- | --- | --- |
| Plugin provides a domain capability | Lithe: the typed domain contract | The plugin, declaratively or by its backend | A manifest `source`, Core plans and decodes, a typed `lithe/build/*` request | Build model, dependency tree; later task execution |
| Plugin provides a plugin function | The plugin: its own params and results | The plugin's backend | `lithe/function/invoke` | Refresh a private snapshot, a step behind a button of the plugin's own UI |
| Plugin calls a host capability | The host | The host | `lithe/host/*` requests, offered by the protocol | Run a process through the host's runner, write output, store state |
| Plugin calls another plugin's exported function | The exporting plugin: its own params and results, under an exported version | The exporting plugin's backend | `lithe/host/functions/invoke`, relayed by Core as `lithe/function/invoke` | A language plugin asks a build plugin for the options it contributes |

Two rules keep these directions from forming a loop:

- A backend never asks the host to dispatch a domain operation or a function.
  The host could route that request to the same backend, whose handler would
  then wait for itself. A backend request for `lithe/function/invoke` or any
  `lithe/build/*` method is refused with `plugin.function.recursiveCall`.
  A handler that needs, say, a Maven run asks for the host's process
  capability, never for the task that its own plugin implements. The
  [relay methods](#cross-plugin-calls) are a different
  request: they name another plugin's exported function, Core refuses a call
  that would come back to a plugin already waiting on the call chain, and
  the old refusals above stay in force.
- Host capabilities are primitive host services, never another entry into a
  domain registry. Each is defined together with its params, permission, and
  the operation it must belong to. A request for a service the host does not
  offer is refused with `plugin.host.unavailable`; the services are listed
  under [Host services](#host-services).

### Declarative or backend implementation

- A domain operation has exactly one implementation, chosen in the manifest
  (`source`: `static`, `command`, or `provider`). Core plans
  only that one. There is no runtime fallback from a backend to a template or
  back: the two may differ in meaning, and a fallback would hide the failure.
- A backend implements a domain operation through its typed method and Core's
  plan and decode commands, never through a plugin function, so IDE
  workflows, presentation, and other hosts keep a single contract.
- A plugin-private function needs no manifest field. What a running process
  can answer is negotiated, as for [build providers](#build-providers); so
  declaring, adding, or removing private functions changes neither
  `pluginAPIVersion` nor any `requires` version. Only a function that
  another plugin may call, and a call to such a function, are declared, in
  [`pluginFunctions`](#cross-plugin-calls).
- A new kind of domain implementation is a new version of the capability that
  owns it, so only plugins that use it are filtered from hosts that cannot run
  it.

### Negotiation

For a session owned by a plugin (`lsp.startServer.pluginId`), the host offers
`capabilities.experimental.litheFunctions: { "versions": [0] }` in
`initialize`, next to and independent of `litheBuild`. The protocol has no
version of its own: it is the version of `backend`. Host-owned sessions do
not offer or accept this protocol, retain no plugin-function registry, and
reject register/unregister with `-32601`. A plugin backend that offers
functions answers:

```json
"experimental": { "litheFunctions": { "version": 0, "functions": [{ "id": "snapshot.refresh" }] } }
```

`functions` is optional and defaults to none. An answer with a version the
host did not offer, or a malformed or duplicate function ID, does not
negotiate the protocol at all, so a session never starts with half a
registry. Unknown fields of the answer and of descriptors are ignored.
After a plugin-owned session successfully negotiates the protocol in
`initialize`, Core publishes its initial
registry as `functionsChanged { functions: [<sorted IDs>], removed: [] }`,
including when the negotiated registry is empty. Hosts can discover the
functions from events without issuing an invocation first.

### References and IDs

A function reference is `{ "pluginID", "functionID" }`. The host routes by
plugin first: only a session the plugin started (`lsp.startServer` with
`pluginId`) serves its functions, and a reference to another plugin is
`plugin.function.wrongPlugin`. Host-owned servers serve none.

A function ID is dot-separated lowerCamelCase segments
(`snapshot.refresh`), at most 128 bytes, unique within the session. A
session holds at most 256 functions.

### Registration and withdrawal

After `initialize` a backend may send:

| Request | Params | Effect |
| --- | --- | --- |
| `lithe/function/register` | `{ "functions": [{ "id" }] }`, nonempty | Adds the functions. |
| `lithe/function/unregister` | `{ "ids": ["…"] }`, nonempty | Removes them and revokes their running calls. |

Each request is all or nothing: a malformed ID, a function listed twice, an
already registered ID, an unregistered ID, or exceeding the limit refuses the
whole request with JSON-RPC error `-32602` and
`data.code: "plugin.function.invalidRegistration"`. A successful change
answers `null` and publishes `functionsChanged` with the sorted IDs. A
backend that did not negotiate the protocol gets `-32601` for both.

### Invocation

`lsp.functionRequest { sessionId, operationId?, function, params?,
timeoutMilliseconds?, context? }` (`context` is described under
[workspace contexts](#workspace-contexts-and-capability-discovery)) returns `{ operationId }` and sends:

```json
{ "method": "lithe/function/invoke", "params": { "functionID": "snapshot.refresh", "operationID": "…", "params": { … } } }
```

`params` must be a JSON object and defaults to `{}`. Core assigns every
backend invocation a unique, opaque `operationID`. The backend must echo it
unchanged in progress notifications; it must not derive it from the host's
`operationId`. The host continues to cancel and receive events using its own
`operationId`, which can be reused after a call ends. Core translates progress
back to that host ID, including `requestProgress.result.operationID`. This
prevents a late report for an earlier call from being attributed to a new call.
Core checks, in this order, and refuses without writing anything:

| Check | Core error | `details` |
| --- | --- | --- |
| Plugin or function ID malformed | `invalid_request` | `plugin.function.invalidReference` |
| `params` not an object | `invalid_request` | `plugin.function.invalidParams` |
| Session belongs to no plugin | `not_supported` | `plugin.function.unavailable` |
| Session belongs to another plugin | `permission_denied` | `plugin.function.wrongPlugin` |
| Protocol not negotiated | `not_supported` | `plugin.function.unavailable` |
| Function not registered | `not_supported` | `plugin.function.unknown` |

The call ends with exactly one `requestCompleted` event:

| Outcome | Event |
| --- | --- |
| Result | `result: { "value": <result> }`; `null` is a result. |
| Backend error | `error.code: "plugin.function.failed"`, the backend's `message` (at most 1024 bytes), and `error.pluginErrorCode` from `error.data.code` when it is an identifier of at most 64 bytes. The plugin code is for the plugin's own UI; Lithe never branches on it. |
| No `result`, or a result over 1 MiB serialized | `plugin.function.invalidResult` |
| Function withdrawn while running | `plugin.function.revoked`; the host also sends `$/cancelRequest` |
| Cancelled through `lsp.cancelOperation` | `requestCancelled`; the host sends `$/cancelRequest` |
| Deadline passed | `requestTimeout`; the host sends `$/cancelRequest`. `timeoutMilliseconds` is capped at one hour and defaults to the session's request timeout. |
| Process exited or the transport failed | The session's terminal error, as for every pending request |

After a call has ended, its late reply is ignored, and so is any progress
report for it. Every terminal path removes its function/request association;
withdrawing that function later cannot cancel a new operation reusing the
host ID, including a build request. A backend should stop work on
`$/cancelRequest`, but the host does not rely on it.

### Progress

A backend reports progress of a running call with the notification
`lithe/function/progress { operationID, message?, percentage? }`, where
`message` is cut to 1024 bytes and `percentage` is a whole number from 0 to
100. Core publishes it as `requestProgress` with the operation ID and the
normalized report. A malformed report, or one for an operation that is not
running on that session, is dropped.

### Host services

A backend may send these requests to the host.
Each service is granted by one manifest permission; Core receives the
granted services as `lsp.startServer.hostServices` (from the launch plan's
`backend.hostServices`, less the method grants the host does not serve) and
checks every request before the host sees it.

| Request | Params | Result | Permission |
| --- | --- | --- | --- |
| `lithe/host/credentials/get` | `{ key }` | `{ secret }`, `null` when nothing is stored | `credentials.store` |
| `lithe/host/credentials/set` | `{ key, secret }` | `null` | `credentials.store` |
| `lithe/host/credentials/delete` | `{ key }` | `null`, also when nothing was stored | `credentials.store` |

- A `key` is 1–128 ASCII letters, digits, `.`, `_`, `:`, or `-`; a `secret`
  is a string of at most 16 KiB.
- Credentials are private to the plugin ID: the host stores them in the
  platform's secret store (the Keychain on macOS) in a namespace of that ID,
  never in `${stateDir}` or the package. They survive disabling and updating
  the plugin and are deleted when it is uninstalled.
- Core refuses, without involving the host, in this order: a method the
  protocol does not define (`-32601`, `plugin.host.unavailable`), a service the
  plugin is not granted (`-32010`, `plugin.host.permissionDenied`), and
  malformed params (`-32602`, `plugin.host.invalidParams`; a field the method
  does not define is `-32018`, `plugin.host.unsupportedOption`, see
  [Extending the host services](#extending-the-host-services)). The code is in
  `error.data.code`.
- An accepted request becomes the runtime event `hostRequest` with `method`
  and `result: { hostRequestID, service, params }`. The host answers it with
  `lsp.hostResponse { sessionId, hostRequestID, result? | error: { message } }`;
  Core sends `result` (or `null`) to the backend, or `-32603` with
  `plugin.host.failed` and the host's message cut to 1024 bytes. An unknown
  `hostRequestID`, including one already answered, is `invalid_request`.
- A session holds at most 16 unanswered host requests; a further one is
  refused with `plugin.host.failed`. Unanswered requests end with the session.
- `hostServices` needs `pluginId` and names only services this Core defines
  (`credentials`, `workspace`, or registered method-grant names); otherwise `lsp.startServer` is
  `invalid_request`.

### Build configuration host service

This service is available for local workspace contexts when advertised by
`getCapabilities`. Each request carries `context: { contextID, generation }`.

| Request | Additional params | Result | Permission |
| --- | --- | --- | --- |
| `lithe/host/build/getOptions` | `{ buildSystemID, root }` | `{ options: { <optionID>: <value> } }` | `build.options.read` |
| `lithe/host/build/watchConfiguration` | `{ files: [<path>] }` | `{ subscriptionID }` | `build.configuration.watch` |

- `root` is a slash-separated workspace-relative build root (`.` for the
  workspace folder). Only a build system declared by the caller may be queried.
  Core applies local preferences over team preferences, resolves the instance
  and declaration defaults, and checks the values against the manifest. No
  build model is loaded and no process is run. A foreign build-system ID
  returns `plugin.host.foreignContribution`.
- The watch covers the workspace's build preferences and up to four named
  configuration files. The supplied paths are absolute on the local host,
  normalized, at most 4096 UTF-8 bytes each, and may name files outside the
  workspace. They may be absent when the watch starts. Directories and globs
  cannot be watched. A change sends an existing `subscriptions/notify` event
  `{ type: "changed" }`, without paths, source contents, or values. Read
  the effective configuration again after subscribing and on invalidation.
- File creation, replacement and removal invalidate the snapshot. A terminal
  `{ type: "watchFailed" }` event ends a watch the host could not restore.
  The host releases watches on disposal, context expiration or session end.
  At most 16 build configuration subscriptions may be held per backend.
- Malformed requests get `plugin.host.invalidParams`, unknown request members
  get `plugin.host.unsupportedOption`, an expired context gets
  `plugin.host.contextExpired`, and an exhausted subscription budget gets
  `plugin.host.limitExceeded`. A local read or observation failure gets
  `plugin.host.failure`.

### Workspace contexts and capability discovery

The requests below give a backend capability discovery, subscription
disposal, and workspace contexts. A context is named on each call, host
failures carry typed details, and the host can cancel requests and send host
notifications.

| Request | Params | Result | Granted by |
| --- | --- | --- | --- |
| `lithe/host/getCapabilities` | none or `{}` | `{ protocolVersion, backendAuthority, services, limits }` | every plugin-owned session |
| `lithe/host/subscriptions/dispose` | `{ subscriptionID }` | `null`, also for an unknown or already disposed ID | every plugin-owned session |
| `lithe/host/workspace/listContexts` | none or `{}` | `{ revision, contexts: [<context>] }` | `workspace.metadata` |
| `lithe/host/workspace/getContext` | `{ context: { contextID, generation } }` | `<context>` | `workspace.metadata` |
| `lithe/host/workspace/getFolders` | `{ context: { contextID, generation } }` | `{ folders: [<folder>] }` | `workspace.metadata` |
| `lithe/host/workspace/getHostPaths` | `{ context: { contextID, generation }, rootIDs?: [<rootID>] }` | `{ folders: [{ rootID, hostPath }] }` | `workspace.metadata`, as a method grant of its own |

- Core answers `getCapabilities` itself; `protocolVersion` is the negotiated `litheFunctions` version. `services` maps each service the
  session may use (the implicit `host`, `subscriptions`, and `commands`, and
  the granted ones) to `{ methods, notifications }`: the requests it may
  send and the [host notifications](#host-notifications)
  it may receive. It lists nothing the session may not use, and a host
  grants only the services it has handlers for, so a listed method never
  fails for want of one. `backendAuthority` is `"local"`: the backend runs on
  the machine that runs Lithe. `limits` gives `pendingHostRequests`,
  `functionResultBytes`, `hostRequestBytes`, `hostResultBytes`, `errorMessageBytes` (the cut of a host failure's
  message), `errorDetailsBytes`, and `notificationBytes`. A backend must
  check `services` before it calls a method.
- A service entry may also carry `options`: what the host declared for that
  service. When `editor/registerActions` is granted, `services.editor.options`
  has `actionIcons` (the icon tokens the host draws) and `limits` gains
  `editorActionSourceBytes` (the largest document the host sends a
  collector). When `editor/registerProviders` is granted,
  `services.editor.options.providerTypes` lists the provider types the host
  routes.
- A context is one open workspace in one window:

  ```json
  {
    "contextID": "w-3f2a9c",
    "generation": 2,
    "name": "orders",
    "authority": { "kind": "local" },
    "folders": [{ "rootID": "r0c1d2e3f4a5b6c7d", "name": "orders", "path": "/Users/me/orders" }],
    "stateDirectory": "/Users/me/Library/Application Support/Lithe/PluginState/<plugin>/backend/workspaces/<digest>"
  }
  ```

  `contextID` uses the key alphabet and lives as long as the window shows
  that workspace: a window that switches to another workspace closes the
  context and opens one with a new `contextID`, so state a backend keyed by
  the ID never carries over to a different workspace. `generation` starts at
  1 and grows every time the same workspace's folders or authority change; a
  request naming an older generation, or a closed context, is
  `plugin.host.contextExpired`, and the backend asks again. `revision` of
  `listContexts` grows whenever a context opens, closes, or changes.
- `rootID` identifies one root folder stably for the same location, across
  restarts, without revealing the path. It is keyed per plugin with a host
  secret, so two plugins see different IDs for the same folder and an ID
  cannot be computed from a guessed path. `path` is present only for a
  `local` authority.
- `authority.kind` is `local` or `remote` (a remote workspace, which also
  has `authority.id`, an opaque connection ID). For a `remote` context the
  folders carry no `path`: the local directory the host keeps for a remote
  workspace is an empty placeholder, not the workspace. File, search, and
  process services answer a request for a context whose authority they do
  not implement with `plugin.host.authorityUnsupported`; they never read the
  placeholder or run the operation locally instead.
- A `remote` authority also has `platform: { os, arch }`, the remote host's
  operating system and CPU architecture, with the values of
  [managed runtimes](#managed-runtimes): `os` is `linux` and `arch` is
  `arm64` or `x86_64`. A backend uses it to choose the package or runtime a
  process on the host needs. A `local` authority has no `platform`; the
  backend runs on that machine. A backend ignores an `os` or `arch` it does
  not know, as a later host may connect to other systems.

  ```json
  "authority": { "kind": "remote", "id": "c-7d41", "platform": { "os": "linux", "arch": "x86_64" } }
  ```
- `getHostPaths` gives the paths of a `remote` context's root folders on the
  remote host. They are not part of the context, so a backend that does not
  need them never sees the remote user name or directory layout, and a
  backend written before remote workspaces cannot take one for a local path.
  - It is granted by `workspace.metadata` as a method grant of its own, not
    with the `workspace` service: a host that does not serve it leaves it
    out of `getCapabilities`, and the service grant alone is
    `plugin.host.permissionDenied`.
  - `rootIDs`, when present, names 1 to 256 distinct root IDs of the
    context, in the key alphabet; `folders` then holds those roots in that
    order. Without it, `folders` holds every root in `getFolders` order. A
    root ID the context does not have is `plugin.host.notFound`.
  - A `local` context is `plugin.host.authorityUnsupported`: its folders
    already carry `path`. A closed context, or one whose generation changed,
    is `plugin.host.contextExpired`, and earlier answers are void.
  - `hostPath` is an absolute POSIX path on the remote host. A backend uses
    it only in what runs on that host: process arguments and working
    directories, a debug configuration's `program` and `cwd`, a language
    server's `initializationOptions`. It never names a file on the machine
    that runs Lithe, so a backend never reads or writes it locally; files of
    the workspace are read through the file services by `{ rootID, path }`.
  - The official SDKs give `path` and `hostPath` distinct types, and their
    helper that picks the path a process uses returns `path` for a `local`
    context without asking the host.
- `stateDirectory` is private to the plugin and the workspace, outside the
  plugin package and the application bundle. Only `getContext` returns it,
  and creates it then; `listContexts` entries carry every other field and
  create nothing, so listing many windows leaves no directories behind.
  `${stateDir}` is the plugin-wide state. Both survive disabling and
  updating the plugin and are deleted when it is uninstalled.
- A page call may name its context: `lsp.functionRequest.context`
  (`{ contextID, generation }`) reaches the backend as
  `params.context` of `lithe/function/invoke`. A malformed reference is
  `plugin.function.invalidParams`. The host fills it from the page's window.
- A host failure may carry a code and details:
  `lsp.hostResponse.error: { message, code?, details? }`. Core passes these
  codes to the backend, each with its own JSON-RPC error code so a backend
  can branch on the number alone; any other code, or no code, gives
  `plugin.host.failed`. `details` is an object of at most 4 KiB serialized
  (for example `{ currentVersion }` with `versionConflict`, or
  `{ retryAfterMilliseconds }` with `rateLimited`); it reaches the backend as
  `error.data.details`. Details that are not an object, or larger, are
  dropped; the failure itself still reaches the backend.
  `fixtures/backend/function-protocol.json` (`hostFailures`) lists the
  mapping.

  | Code | JSON-RPC | Meaning |
  | --- | --- | --- |
  | `plugin.host.unavailable` | `-32601` | The host offers no such service or method |
  | `plugin.host.invalidParams` | `-32602` | Params malformed or out of range |
  | `plugin.host.failed` | `-32603` | Any other failure |
  | `plugin.host.cancelled` | `-32800` | The request was cancelled |
  | `plugin.host.permissionDenied` | `-32010` | Permission not declared, or the user withheld consent |
  | `plugin.host.contextExpired` | `-32011` | The context is closed or its generation replaced |
  | `plugin.host.authorityUnsupported` | `-32012` | The service does not support the workspace authority |
  | `plugin.host.versionConflict` | `-32013` | A document or resource version differs from the expected one |
  | `plugin.host.limitExceeded` | `-32014` | A host limit was exceeded |
  | `plugin.host.timeout` | `-32015` | The host's deadline passed |
  | `plugin.host.notFound` | `-32016` | The named resource does not exist |
  | `plugin.host.readOnly` | `-32017` | The resource or workspace cannot be written |
  | `plugin.host.unsupportedOption` | `-32018` | The host does not implement a requested option; `details.option` names the field |
  | `plugin.host.preconditionFailed` | `-32019` | A state the request requires does not hold, such as a missing user confirmation |
  | `plugin.host.busy` | `-32020` | The resource is in use; retry later |
  | `plugin.host.rateLimited` | `-32021` | Too many requests in a period; retry after `details.retryAfterMilliseconds` when given |
  | `plugin.host.languageServerUnavailable` | `-32022` | The plugin's language server is not running for the context, or stopped while the request was in flight |
  | `plugin.host.languageServerError` | `-32023` | The language server answered with a JSON-RPC error; `details` is `{code, message}` |
  | `plugin.host.methodNotDeclared` | `-32024` | The method, or the `workspace/executeCommand` command, is not in the server's declared session |
  | `plugin.host.toolUnresolved` | `-32025` | A tool definition the request names resolves to no executable for this workspace; `details` is `{ tool, reason }`, `reason` being `notFound` (no candidate of the four layers meets the definition and its minimum version) or `unusable` (a chosen executable cannot be used) |
  | `plugin.host.foreignContribution` | `-32026` | The named server or contribution belongs to another plugin |
  | `plugin.host.remoteDisconnected` | `-32078` | The connection to the remote host was lost while the operation ran; whether it took effect on the host is unknown, and the backend asks again once the context is current |
  | `plugin.host.platformUnavailable` | `-32079` | The plugin has no package or managed runtime for the remote host's `platform`, so the process the request needs cannot run there |
  | `plugin.host.alreadyExists` | `-32080` | The target of a create, copy, rename or save-as already exists; the host never overwrites it |
  | `plugin.host.diskChanged` | `-32081` | The file changed or was deleted on disk after the document read it; nothing was written, and the user chooses which version to keep |

  JSON-RPC reserves `-32768` to `-32000` for the protocol and lets
  implementations define `-32099` to `-32000` as server errors. `-32000`
  to `-32009` stay free for backends' own failures, which many JSON-RPC
  libraries already use from `-32000`, so host codes start at `-32010` and
  grow by one; a code is never reused for another meaning.
- A backend may cancel its own pending host request with `$/cancelRequest`.
  Core answers it at once with `-32800` and `plugin.host.cancelled`, frees
  the slot, and publishes `hostRequestCancelled { hostRequestID }` so the host
  stops the work; a later `lsp.hostResponse` for that ID is
  `invalid_request`. The 17th pending request of a session is
  `plugin.host.limitExceeded`.
- A service that creates a subscription returns a `subscriptionID` of the key
  alphabet; its notifications name it. Ending the session, disabling the
  plugin, or closing the subscription's context disposes it.
- A backend still cannot ask the host to call a function or a domain
  operation (`plugin.function.recursiveCall`), so a page call never waits on
  itself through the host.

### Host notifications

The host tells a backend about changes with JSON-RPC
notifications; the backend answers none of them. The host sends them through
Core with `lsp.hostNotify { sessionId, method, params }`, which returns
`{ delivered }`, `true` for an accepted notification.

| Notification | Params | Granted by |
| --- | --- | --- |
| `lithe/host/workspace/didChangeContexts` | `{ revision }` | `workspace.metadata` |
| `lithe/host/subscriptions/notify` | `{ subscriptionID, sequence, event }` | every plugin-owned session |
| `lithe/host/commands/didRun` | `{ commandID, context? }` | every plugin-owned session |
| `lithe/host/run/didExit` | `{ runID, exitCode?, signal? }` | `run.start` |
| `lithe/host/languageServer/didChangeState` | `{ context, serverID, state, generation }` | `languageServer.request` |
| `lithe/host/languageServer/didReceiveNotification` | `{ context, serverID, generation, method, params?, truncated? }` | `languageServer.request` |

- `didChangeContexts` follows every change `revision` of `listContexts`
  counts; the backend calls `listContexts` for the contexts. A backend that
  starts later reads them itself.
- `subscriptions/notify` carries one `event` object of the named
  subscription. `sequence` starts at 1 and grows by one per subscription, so
  a backend can detect a gap; nothing follows a disposal.
- `commands/didRun` reaches the plugin's backend when the user runs one of
  the plugin's commands. `commandID` is the command's ID; `context`
  (`{ contextID, generation }`) is the window's context when it has one. A
  running backend receives it in order with the other notifications; a
  stopped one is started for it only when it declares
  `onCommand:<commandID>`, and receives it once ready.
- `run/didExit` reports the end of a Run session the backend started with
  [`run/start`](#run-and-debug-sessions). `exitCode` is the
  process's exit status and `signal` (1 to 127) the signal that ended it; both
  are absent when the host lost the process. It is sent once per run, only
  to a session that holds the `run.start` grant.
- `revision` and `sequence` are positive integers, `event` is an object,
  and serialized params are at most `limits.notificationBytes`. Core refuses
  any other method, a service the session is not granted, malformed params,
  a session that is not plugin-owned, or one that is not ready, with
  `invalid_request`. `fixtures/backend/function-protocol.json`
  (`hostNotifications`) holds the cases.

### Extending the host services

Host capabilities are added without a new version, and a backend discovers
them with `getCapabilities` instead of a version number. A later host adds,
without a new version:

- a service with its methods and notifications, granted by a new manifest
  permission, or a method or notification of an existing service;
- an optional field of request params (an *option*) whose absence keeps the
  earlier behavior, or a field of a result, a notification, `details`, or
  `limits`;
- a failure code, which takes the next JSON-RPC number no code of this
  contract uses yet; a number never changes meaning.

A change an existing backend would misread gives `backend` version 1, and the
protocol moves with it: removing or renaming a method, notification, or
field, changing a field's meaning or type, making an option required,
changing a code's number, or weakening an ordering or lifecycle guarantee
above.

So that a backend works with every host:

- It calls a method only when `getCapabilities.services` lists it; another
  is `-32601`, `plugin.host.unavailable`.
- It ignores result, notification, `details`, and `limits` fields it does
  not know, and any `lithe/host/*` notification it does not know.
- Core refuses a field the method does not define with `-32018`,
  `plugin.host.unsupportedOption`, and `details: { option }` naming it, before
  the host sees the request; the backend may retry without that option. A
  service whose params Core does not type answers the same way itself.
- It treats a JSON-RPC number it does not know as a host failure, and
  branches on `error.data.code` when it knows that code.
- A package whose backend needs a new service declares the new permission.
  A host that does not know the permission refuses the package at install
  (`plugin.permission.unknown`), so a permission is never granted or
  ignored silently.

`fixtures/backend/function-protocol.json` (`hostRequests`) holds the
unsupported-option cases.

### Public document, file and window methods

A host that advertises `backend` support implements these methods.
`getCapabilities` lists only the intersection of the plugin's
permission-derived grants and this host's registered method handlers.

Grants in `lsp.startServer.hostServices` for the methods below use **full
method names**, not service names. The `workspace` grant admits only
`listContexts`, `getContext` and `getFolders`; it never grants file reads. The
`credentials` grant admits the three credentials methods. Registering another
handler cannot replace a method already registered.

All methods below require `context: {contextID, generation}`. Resource arguments
use `{rootID, path}` from that plugin's `getFolders` response; paths use `/` and
are relative to the root. A root ID is not a lifetime or an authorization token.
Opening the same folder in two windows creates independent context-owned leases;
closing one does not invalidate the other, and reopening works with generation 1
in its new context. Remote contexts are explicitly refused by these
methods. Unknown options fail before side effects with `unsupportedOption`.

| Methods (prefix `lithe/host/`) | Permission | Params in addition to `context` | Result |
| --- | --- | --- | --- |
| `documents/list` | `documents.read` | none | `{documents:[<summary>]}` |
| `documents/getSnapshot` | `documents.read` | `documentID` | `{document:<summary>, text}` from the dirty live buffer |
| `documents/save` | `documents.save` | `documentID,expectedVersion?` | `{document:<summary>}` after the write, `isDirty:false`; see [Saving documents and changing files](#saving-documents-and-changing-files) |
| `documents/saveAs` | `documents.save` | `documentID,target` (a `{rootID,path}` that does not exist) | `{document:<summary>}` of the new file, opened and selected |
| `documents/close` | `documents.save` | `documentID` | `null` after saving the document and closing its tab |
| `editor/registerActions` | `editor.actions` (+ `documents.read`) | `id,function,selector` | `{subscriptionID}`; independent editor actions, see below |
| `editor/registerProviders` | `editor.providers` (+ `documents.read`) | `id,selector,providers` | `{subscriptionID}`; editor language-feature providers, see below |
| `editor/getActive` | `documents.read` | none | summary of the foreground text tab, or `null` |
| `editor/getSelections` | `documents.read` | `documentID` | `{documentID,version,selections:[{anchor,active}],primaryIndex}` |
| `editor/applyEdits` | `editor.edit` | `documentID,expectedVersion,edits:[{range,text}]` | `{documentID,version,applied}` |
| `editor/replaceSelections` | `editor.edit` | `documentID,expectedVersion,selections:[{anchor,active}],replacements:[string]` | same; one replacement for all selections or one per selection |
| `editor/insertText` | `editor.edit` | `documentID,expectedVersion,at,text` | same |
| `workspace/readDirectory` | `workspace.files.read` | `resource,cursor?,limit?` | `{entries:[{name,resource,kind}],nextCursor}` |
| `workspace/stat` | `workspace.files.read` | `resource` | `{resource,kind,size,modifiedMilliseconds}` |
| `workspace/readFile` | `workspace.files.read` | `resource,maximumBytes?` | `{resource,source:"disk",byteCount,isTruncated,identity,text?,encoding?,base64?}` |
| `workspace/watch` | `workspace.files.watch` | `resource,recursive?` (default false) | `{subscriptionID}`; events contain `{rootID,changedPaths,requiresRescan}` |
| `workspace/findFiles` | `workspace.search` | `resource` naming the root (`path:""`), `query,fileMask?,cursor?,limit?` | `{matches:[<resource>],nextCursor,isTruncated,source:"disk"}` |
| `workspace/create` | `workspace.files.write` | `resource,kind` (`file`, `directory` or `symlink`), `text?` (files only), `linkTarget` (symlinks only) | `{resource,kind}`; never overwrites |
| `workspace/rename` | `workspace.files.write` | `resource,target` | `{resource}` of the target; open documents follow |
| `workspace/copy` | `workspace.files.write` | `resource,target` | `{resource}` of the copy; never overwrites |
| `workspace/trash` | `workspace.files.write` | `resource` | `null` after moving it to the system trash |
| `workspace/findText` | `workspace.search` | root `resource,pattern,fileMask?,caseSensitive?,wholeWords?,regularExpression?,cursor?,limit?` | same envelope, matches `{resource,line,preview,range:null}`; the upstream search owner currently reports lines, not exact match spans |
| `editor/open`, `editor/reveal` | `editor.open` | `resource,activation?` (`select` or `background`), `range?,tab?` (`regular` only) | document identity/version and state, `wasAlreadyOpen,windowIsForeground,appliedSelection` |
| `preview/openHTML` | `preview.open` | HTML `resource,presentation?` (`editorAndPreview` or `previewOnly`), `range?` | open result plus `usesLiveBuffer:true,presentation`; explicitly changes the existing tab's presentation |
| `browser/openExternal` | `browser.open` | exactly one of HTTP(S) `url` or HTML `resource`; `unsaved?` (`openSavedFile` or `requireSaved`) for a resource | `{opened:true,source:"web"|"disk",hasUnsavedChanges?}` |
| `window/showInformation`, `window/showWarning`, `window/showError` | `window.messages` | `text,actions?` | `{selectedIndex}`; zero-based, or `null` on dismissal |

A document summary has `documentID,version,path,languageID,isReadOnly,isDirty,
encoding,lineEnding`. `path` is workspace-relative or null for a document outside
the root; `languageID` may be null. Positions `{line,column}` are one-based UTF-16;
ranges `{start,end}` are half-open and cannot split a surrogate pair. Selections
keep anchor/active direction. Batches are atomic and one Monaco undo step. Stale
versions, read-only buffers, pending editor synchronization and IME composition
are refused. Background documents without a mounted editor can be listed/read,
but edits/selections may return `busy`; opening a foreground tab does not imply
that its web editor is already ready. Retry reads after readiness, not edits
blindly. `editor.open/reveal` currently applies a range's start as a caret and
reports `{kind:"caret",line,column}`, not a fabricated full selection.

Limits: snapshots and file reads expose at most 128 KiB of source text/bytes per
call; oversized snapshots return `limitExceeded`, file reads report truncation.
This leaves room for worst-case JSON escaping under the 1 MiB serialized host
request/result budget. Batches contain at most 1,000 edits/selections. Directory
pages default to 200 and cap at 1,000; search pages default to 100 and cap at 200,
with the search owner's overall 200-result cap. Explicit file reads use
component-wise `O_NOFOLLOW`; listings omit symlinks. Hidden-file rules affect
listing/search/watch, not authorization for explicit reads. Open/browser paths
retain the existing canonical containment checks and their documented
check-to-open race; native backends are not OS-sandboxed.

Workspace disk reads (API-06) require exactly `{rootID,path}`. `path` is at
most 4096 UTF-8 bytes, uses `/`, and has no empty, `.` or `..` component,
backslash, absolute prefix or NUL. On Windows, reserved device names, alternate
data streams, trailing dots/spaces and other non-storable names are refused.
`path:""` names the root for `readDirectory` and `stat`, but not `readFile`.
Unknown top-level or resource fields return `unsupportedOption`; malformed
paths, cursors and bounds return `invalidParams`. Missing roots/entries return
`notFound`, filesystem access failures and attempted link traversal return
`permissionDenied`, and other I/O failures return `failed`. Remote contexts
return `authorityUnsupported` before any local I/O.

`readDirectory.limit` is an integer in 1–1000 (default 200). Entries are ordered
by their UTF-8 name bytes, omit symbolic links/reparse points, names that cannot
be represented as a resource, and the workspace's effective hidden rules.
A page may be shorter than `limit` to respect the serialized response budget.
`nextCursor` is an opaque string, or null when exhausted. It is bound to the
root, directory and filters, and resumes strictly after the last returned name;
changing a directory between pages can omit newly inserted earlier names. It
is a live listing, not a retained snapshot. `stat` reports the leaf itself
(`file`, `directory`, `symlink` or `other`), with byte `size` and epoch
`modifiedMilliseconds` (null when unavailable); no ancestor link is followed.

`readFile.maximumBytes` is an integer in 1–131072 (default 131072).
`byteCount` counts the returned raw prefix bytes. `isTruncated` is true when
at least one additional byte was observed. `identity` is
`{sha256,byteCount,isTruncated}`, where `sha256` is lowercase SHA-256 of exactly
the returned bytes, never a claim about an unread suffix or an atomic whole-file
snapshot. Valid UTF-8 containing no control characters other than tab, CR and
LF returns `text` and `encoding:"utf-8"`, preserving any UTF-8 BOM. Every other
prefix returns standard `base64` instead, including UTF-8 cut mid-character.
Exactly one of `text` and `base64` is present. Only regular files can be read;
there is no buffer fallback, encoding guessing or lossy substitution. Unix
opens every plugin-supplied component relative to a held directory descriptor
with `O_NOFOLLOW`; Windows rejects reparse points and holds directory ancestors
without write/delete sharing until the operation finishes.

Workspace search and watches (API-07/08) use the same strict resource paths,
local authority and no-follow policy as disk reads. Unknown fields are
`unsupportedOption`; malformed flags/bounds/queries/cursors and invalid regex
are `invalidParams`. A missing target is `notFound`, access/link traversal is
`permissionDenied`, and other I/O failures are `failed`.

- `findFiles` and `findText` require a root resource (`path:""`). `query` or
  `pattern` must contain 1–4096 UTF-8 bytes; `fileMask` has a 4096-byte maximum
  and is a comma-separated list of basename globs such as `*.md,*.txt`.
  File-name search is case insensitive. Text search defaults all flags to false
  and uses the existing workspace matcher's literal/regex semantics; regex mode
  uses the expression's own word boundaries. Pages default to 100 and accept
  1–200, with at most 200 results across continuations. Results sort by UTF-8
  path bytes then one-based line. Text results contain one hit per line,
  `range:null`, and a trimmed preview capped at 500 characters. Search reads
  disk only, applies effective hidden/indexing/Git-ignore rules, never follows
  links, omits binary/undecodable files, and uses the existing search text
  decoder. A text file is limited to 2 MiB and directory depth to 128. These
  omissions or the overall hit cap set `isTruncated:true`; no exact count is
  claimed. A page may be smaller than requested to fit the response budget.
  `nextCursor` is null on exhaustion; it binds root, query/options and effective
  policy and resumes after the last result without a retained snapshot.
  Page size may change, but changing the query or policy invalidates the cursor.
- `watch` accepts an existing regular file or directory and defaults `recursive`
  to false. File watches observe replacement/removal of that file; directory
  watches observe immediate entries or the subtree when recursive. Events carry
  root-relative, sorted and deduplicated `changedPaths` with the calling
  plugin's `rootID`. Hidden entries and linked descendants are omitted.
  Native streams may coalesce changes, report directories instead of every
  descendant, or lose names; `requiresRescan:true` means reread the watched
  scope. Registration emits an initial rescan event to close the setup gap.
  A batch has at most 1000 paths and 900 KiB; overflow also requests rescan.
  At most 16 limited native subscriptions may be held by a backend session;
  excess registration returns `limitExceeded`. Disposal is idempotent and
  checked against the caller's plugin/session. Context closure, backend exit
  or plugin stop releases the stream; notifications stop before disposal
  returns. If the workspace root is deleted or replaced, the host emits a final
  rescan and releases the old stream; register again after rereading the scope.
  Sequence starts at 1, increments per emitted notification, and never
  transfers to a replacement session/context. Registration and notifications
  use native filesystem events, with no idle polling or filesystem writes.

Machine host requests have a 30-second host deadline. User-choice messages wait
until the user answers, the request is cancelled, or the context/session closes.
Messages retain the per-plugin/window rate and queue limits (5/10 seconds,
3 pending per plugin, 10 per window, 2,000 text characters, 3 actions of 40
characters). Session exit revokes grants and cancels requests/subscriptions;
old session callbacks cannot affect a replacement. Closing a context cancels its
requests with `contextExpired`. Notifications have a 128-entry per-session queue;
context revisions coalesce to the latest, and overflow ends the session instead
of allowing unbounded memory or silently losing commands. Notification/response
writes share the process write watchdog and stop cleanup with function requests.

Cancellation or timeout **does not promise rollback**: an edit or external open
may already have committed. The SDK must not replay a mutation on these failures.
On `versionConflict`, reread and recompute; on `contextExpired`, resolve the
original target again, never substitute the focused window. `busy`/`rateLimited`
may be retried with bounds; `permissionDenied`/`readOnly` need user intervention.

#### Saving documents and changing files

`documents.save` grants `documents/save`, `documents/saveAs` and
`documents/close`. `workspace.files.write` grants `workspace/create`,
`workspace/rename`, `workspace/copy` and `workspace/trash`. Neither grants
any other method, and no other permission grants these; a plugin that only
reads keeps reading. The host makes each change through the same owner as the
user's command, so a saved document keeps its version, undo history and
language session, and an open document follows its file when the file moves.

None of these methods asks the user anything. They have the 30-second machine
deadline, and a change that would need the user's choice is refused instead:

- **Nothing is overwritten.** `saveAs`, `create`, `rename` and `copy` fail with
  `alreadyExists` when the target exists. A rename that changes only the case
  of a name on a case-insensitive file system is not a collision.
- **Nothing is discarded.** `save` writes the document's current text, including
  edits the user made after the plugin asked; a save already running finishes
  first, and the result reports the version that was written. `close` saves
  first and closes only after the save succeeds and the file on disk matches
  the document. `rename` and `trash` save the open documents under the resource
  first; when one of those saves fails, nothing is moved.
- **Nothing is deleted.** `trash` moves the file or directory to the system
  trash, from which the user restores it. Its open documents are saved and
  their tabs close. There is no permanent delete.
- **A disk change is the user's to resolve.** When the file changed or was
  deleted on disk after the document read it, `save` and `close` fail with
  `diskChanged` and write nothing; the editor keeps showing the user its own
  choice between the two versions.

| Method | Errors in addition to `invalidParams`, `contextExpired`, `authorityUnsupported` and `permissionDenied` |
| --- | --- |
| `documents/save` | `notFound` (no such document); `versionConflict` with `details.currentVersion` when `expectedVersion` is given and differs; `busy` (IME composition); `preconditionFailed` (an untitled document, which needs `saveAs`; a document with no file, such as a virtual one; a hard-linked file, whose save the user must choose); `diskChanged`; `failed` (the write failed; the user also sees the host's save-failed notice) |
| `documents/saveAs` | `notFound` (no such document, or the target's parent directory does not exist); `alreadyExists`; `busy` (IME composition, or a save of the document is running); `failed` |
| `documents/close` | the errors of `save`; `preconditionFailed` is not among them, since an untitled or virtual document closes without a write (an untitled document keeps its draft) |
| `workspace/create` | `notFound` (the parent directory does not exist); `alreadyExists`; `limitExceeded` (`text` is over 128 KiB of UTF-8); `failed` (including a system that refuses to create a link, such as Windows without Developer Mode or administrator rights) |
| `workspace/rename` | `notFound` (the resource or the target's parent); `alreadyExists`; `diskChanged`, `busy` or `failed` from saving an open document; `failed` |
| `workspace/copy` | `notFound` (the resource or the target's parent); `alreadyExists`; `failed` |
| `workspace/trash` | `notFound`; `diskChanged`, `busy` or `failed` from saving an open document; `failed` (the system trash refused the item) |

Paths: `resource` and `target` are `{rootID,path}` resources of one root, and
`target` is the full new path, not a directory to move into. A path has one
spelling: `/`-separated components, none empty, `.` or `..`, at most 4,096
bytes, and on Windows none that Windows cannot store as written (a reserved
name such as `CON`, a trailing dot or space, or one of `\ : * ? " < > |`).
The root itself (`path:""`) cannot be created, renamed, copied or trashed, and
a target inside its own source is refused; these are `invalidParams`. The target's parent must
exist; nothing creates missing directories. The host checks that the parent of
each path resolves inside the root, and acts on the last component itself: a
symbolic link is renamed, copied or trashed as a link, never as the file it
points to. A copy keeps symbolic links inside a copied directory as links.

`create` writes `text` as UTF-8, or makes an empty file; a directory takes no
`text`. It does not open the file; call `editor/open` for that. A symbolic link
takes `linkTarget`, 1 to 4,096 bytes without NUL, and no `text`; `linkTarget` on
a file or directory is `invalidParams`. The link stores `linkTarget` verbatim:
an absolute path, or one relative to the directory that holds the link, which
may lead outside the root or to nothing. Creating a link grants nothing:
`workspace/readFile`, listings and `editor/open` keep their own containment
checks, so a link to a file outside the root does not make that file readable,
and no file is ever created through a parent directory that leaves the root.
On Windows each `/` in `linkTarget` is stored as `\`, and the link is a
directory link when `linkTarget` names an existing directory at creation,
otherwise a file link. `saveAs` writes
the document's current text as UTF-8 with its line endings, then opens the new
file and selects it; the original document stays open and unchanged, except
that an untitled document whose text did not change during the write closes,
as it does for the user's Save As. `rename`, `copy` and `trash` refresh the
project tree and version control status as the user's commands do.

On cancellation or timeout, the change may already be on disk; read the target
back with `workspace/stat` before retrying. Watch events report these changes
like any other.

### Plugin-owned native terminals

[`terminal-backend.plugin.json`](fixtures/backend/terminal-backend.plugin.json) is the
minimal manifest. `terminal.create` grants `lithe/host/terminal/create`, `show`, `close`, `reopen`,
`dispose`, and `onExit`. It requires `backend`; discover the individual
methods through `getCapabilities` before using them. A host without this
handler does not advertise its methods. The
plugin's web page calls its backend through `functions.invoke`; the backend
uses these host requests with the invocation's workspace context. There is
no direct web `terminal.*` bridge.

| Method (prefix `lithe/host/terminal/`) | Params, in addition to `context` | Result |
| --- | --- | --- |
| `create` | `executable`, optional `arguments`, `title`, `cwd`, `environment`, `show`, `retainedContextBytes` | `{terminalID,generation:1}` |
| `show` | `terminalID` | `null` |
| `close` | `terminalID`, optional `retainedContextBytes` | `{retainedContextBytes}` after bounded native cleanup |
| `reopen` | `terminalID`, optional `show` | `{terminalID,generation,restoredContextBytes}` |
| `dispose` | `terminalID` | `null` after closing and forgetting the handle |
| `onExit` | `terminalID` | `{subscriptionID}` |

`executable` is an absolute path or a bare command resolved by the native
terminal's PATH lookup; relative paths containing separators are refused.
`arguments` is an argv array, never shell text. `cwd` is a `{rootID,path}`
workspace directory resource (an empty path selects the folder root); without
it, the workspace root is used. The native adapter rejects a canonical cwd
outside that workspace. `environment` maps variable names to string overrides
or `null` removals. Process-ownership scope variables cannot be overridden.
`show` defaults to true; it selects the terminal in its own window without
raising a background window. With `show:false`, creation and reopening preserve
the current terminal selection and do not reveal the terminal panel or request
input focus; the native tab is mounted when the plugin explicitly calls `show`.
Unknown options fail before any side effect.

Creation and reopening ask for execution consent with the program, argv and
working directory, all available in a scrollable, selectable native text view,
then revalidate context and cancellation before launching
through the existing PTY terminal module. The response waits for user choice
without a machine-operation deadline, but cancellation/session/context closure
still ends it. Native launch and process cleanup retain their own bounds.
The host never types a launch into an existing shell or reads other terminals.
There is no `sendText`, user-terminal enumeration, web emulator, or Docker
special case. First release is local authority only; a remote context fails
with `plugin.host.authorityUnsupported` instead of starting anything locally.

Each handle belongs to one plugin backend session and one context generation.
A handle from another plugin, context or replacement backend is `notFound`.
At most eight handles per plugin are retained, including closed handles;
`dispose` releases one. Limits are 256 arguments, 16 KiB per argument or
environment value, 64 environment entries, 64 KiB combined launch data, and
128 UTF-8 bytes in a nonempty single-line title. `busy` refuses concurrent
close/reopen/dispose on the same handle. Reopening a running terminal is
`preconditionFailed`; showing a closed terminal asks the plugin to reopen it.

#### Close, retained context and reopen

`close` ends the process tree and removes the native tab, keeping the handle's
launch configuration and a plain-text suffix of its displayed terminal output.
`retainedContextBytes` is a nonnegative UTF-8 byte budget: default **16384**,
maximum **262144**, and **0** disables history. It can be chosen at create or
changed at close; over-limit values are refused, never silently expanded.
The result reports actual retained bytes, which can be smaller to avoid cutting
a Unicode scalar. The history is a rendering snapshot, not input events, raw
escape sequences, a resumable process image, or shell state. Secrets already
printed or echoed by a program can be part of that display; choose 0 when no
history should be retained.

`reopen` uses the saved executable, argv, cwd and environment in a **new** PTY
process. It feeds retained plain text to the display before spawning, never to
stdin. Shell variables, unsubmitted input, process state and automatic command
replay are not restored. Natural exit and user tab close are also reopenable;
the snapshot is bounded by the same budget. It keeps `terminalID` and increments
`generation`; late exit callbacks of previous generations are ignored. These
handles/history exist only in memory, not across app/backend restart or
workspace close. No state is written to the app bundle or plugin package.

Natural exit and user tab close run the same bounded native cleanup before
retaining the budgeted snapshot and releasing the complete native session and
render buffer. The handle is `busy` during cleanup; its exit notification is
sent after cleanup, so a subscriber can reopen immediately. A zero-byte budget
retains no display text on any close or exit path.

`dispose` closes any live process and permanently forgets the launch settings
and history. Ending the backend session, disabling/updating the plugin or
closing/replacing its workspace does the same for only its owned terminals;
user-created terminals are unaffected. Closing a page alone does not close a
completed terminal creation. Cancellation before launch creates nothing;
cancellation or context loss during launch closes the newly created process.
Cancelled creation/reopen is not safe to retry automatically.

`onExit` supports one live subscription per handle. Its event is
`{type:"exit",terminalID,generation,exitCode,reason}`, with an integer or null
exit code and `reason:"exited"|"closed"`. Each generation publishes one outcome.
Subscribing after exit delivers its recorded outcome once; the subscription
continues across reopen. Disposing it stops notifications and keeps the
terminal open. A disposed terminal has no further events.

The executable and argv remain plugin-owned semantics. A container plugin can
launch the user's Docker CLI with an explicit engine and container ID; another
tool can launch its interactive client through exactly the same service.
Neither requires a fake Build/Run provider.

### Run and debug sessions

A backend can start a session that the host presents as its own: a Run session
in the Run tool window, or a debug session on one of the plugin's own adapters.
The backend decides what to start; the host owns the process, the console, the
Stop button and the end of the session. A Run session adds host presentation
(a Run window entry the user can stop, the console, the exit status) on top of
what `process.run` already allows; it grants no other capability. `run/start`
and `debug/start` take `context: {contextID, generation}` and the permission
shown. `run/stop` takes no `context`, so a run can still be stopped after the
context it started in has closed.

| Method (prefix `lithe/host/`) | Permission | Params | Result |
| --- | --- | --- | --- |
| `run/start` | `run.start` | `context, label, process, temporaryFiles?, debug?, rerun?` | `{runID}` |
| `run/stop` | `run.start` | `{runID}` or `{sessionID}` | `null` |
| `debug/start` | `debug.start` | `context, label, adapter, request, configuration` | `{sessionID}` |

`run/start`:

- `label` is 1 to 256 characters without control characters. It titles the
  Run window entry.
- `process` is `{executable, arguments, workingDirectory, environment,
  outputEncoding?}`, the first four required. `executable` is `{kind: "tool", tool}`, a tool that the
  manifest lists in `requiresTools` (the host resolves it through the
  [tool-definition layers](toolchains/README.md)), or `{kind: "packaged", path}`, a
  normalized relative path of a file inside the plugin package. Arguments, the
  working directory (relative to the workspace root and an existing directory
  inside it) and the environment have the bounds of
  [run preparation](#provider-run-preparation). `outputEncoding` declares the
  encoding the process writes, as in run preparation; a value the host cannot
  decode lines in is refused at `process.outputEncoding`. An unknown
  `process` field is refused.
- `temporaryFiles` is at most four files of up to 4 MiB each. A file is
  `{name, contents}`, text the host writes as UTF-8, or `{name, base64}`,
  bytes in standard base64 the host writes unchanged; a file with both or
  neither is refused at `temporaryFiles[<n>]`. Bytes are for a format whose
  encoding the plugin must choose, such as a launcher that reads its argument
  file in the system code page: the plugin encodes the file and the host never
  transcodes it. The host writes the files into its own temporary directory
  before the process starts, removes them when the run ends, and replaces
  `${temporaryFile:<name>}` in `process.arguments` and `process.environment`
  with the file's path. A name is unique, 1 to 64 letters, digits, `.`, `_` or
  `-`, and starts with a letter or digit. A plugin therefore never needs to
  know where the host keeps temporary files.
- `debug` is `{configurationType, target}` and makes the run a debug session.
  `configurationType` is a public debug adapter alias, as in
  [registry connection debugging](#registry-connection-debugging).
  `target` is `{kind: "connection", port}`: the process listens for a debugger on
  `port` (1 to 65535; the backend puts it into the arguments), the host waits
  for it and attaches the adapter. Or `{kind: "launch"}`: the adapter starts the
  process itself, taking program, arguments, directory and environment from
  `process` through its `targetBindings`.
- `rerun` is `{function, data}`: one of the plugin's registered functions and
  its params, a JSON object of up to 64 KiB. When the user chooses Run again,
  the host invokes the function with `data` as the params and the current
  `context`, and the backend answers with a new `run/start`, so the backend
  prepares the restart. Without `rerun` the entry cannot be rerun.
- The reply carries an opaque `runID`. The host sends
  [`run/didExit`](#host-notifications) when the process ends.

`run/stop` stops a run by `runID` or a debug session by `sessionID` and answers
`null`, also when it already ended. Cancelling the host request is the same as
stopping. A backend can stop only runs and sessions it started.

`debug/start`:

- `label` is as in `run/start`. `adapter` is one of the plugin's own adapters,
  by its local ID or its qualified `<plugin-id>/<local-id>` ID. An adapter of
  another plugin is refused with `plugin.host.notFound`, and one the plugin does
  not declare is `plugin.host.notFound` too.
- `request` is `launch` or `attach`, and the adapter must serve it.
- `configuration` is a JSON object of at most 256 KiB that the host passes to
  the adapter as the DAP launch or attach arguments, on top of the adapter's
  declared defaults. `noDebug` and every key that starts with `__` belong to
  the host and are refused.

Both methods refuse malformed params with `plugin.host.invalidParams`; the
rejection from Core (`plugin.run.invalidStart` or `plugin.debug.invalidStart`,
with the `field`) is in `details.code` and `details.field`. A closed context is
`plugin.host.contextExpired`, and a `tool` that resolves to no executable is
`plugin.host.toolUnresolved`.

The host keeps running processes when the plugin is disabled or its backend
exits; it only stops telling the backend. The run then ends the way a run
without a backend ends. A backend that starts again does not receive
`run/didExit` for a run it did not see begin.

Fixtures: `fixtures/run/run-start.json` (what `run.decodeStart` returns for each
case, including the rejections) and `fixtures/debug-adapters/debug-start.json` (the same for
`debug.decodeBackendConfiguration`). Temporary files depend on the host's
temporary directory and are covered by Core's own tests.

### Language server host methods

The `languageServer` host service lets a plugin backend talk to the language
servers its own plugin declares. It is granted by the `languageServer.request`
permission, only for a language server that lists `requestMethods`
or `forwardNotifications`, and each method is granted by its full name.
`getCapabilities` lists the methods this host implements. A host without
`backend` support advertises none of them.

Every method takes `context: {contextID, generation}` and `serverID`, the
`languageID` of one of the calling plugin's servers. A server of another
plugin is `plugin.host.foreignContribution`.

| Methods (prefix `lithe/host/languageServer/`) | Params in addition to `context`, `serverID` | Result |
| --- | --- | --- |
| `request` | `method`, `params?` (an object, default `{}`), `timeoutMilliseconds?` (1 to 300000, default 60000) | `{value}`: the server's JSON-RPC `result`, which may be `null`. |
| `notify` | `method`, `params?` | `{delivered}`. `false` only for `workspace/didChangeConfiguration` sent before the server is initialized; the value is then sent with `initialized`. |
| `setStatus` | `state` (`preparing`, `ready`, `degraded`, or `failed`), `message?`, `percentage?` (a whole number from 0 to 100) | `{}` |

- `request` and `notify` are checked by the [session policy](#plugin-owned-sessions):
  `plugin.host.methodNotDeclared` for a method or command the server did not
  declare, `plugin.host.languageServerUnavailable` when the server is not
  running for this context, `plugin.host.languageServerError` when the server
  answers with an error (`details: {code, message}`), `plugin.host.timeout`,
  `plugin.host.cancelled`, `plugin.host.busy` for the 129th waiting request,
  and `plugin.host.limitExceeded` for a result over the host's result bound.
  Cancelling the host request (`$/cancelRequest`) cancels the server request.
- `setStatus` shows the plugin's own preparation state next to the server in
  the host's status UI: `preparing` with an optional `message` and
  `percentage` while the plugin imports a project or indexes, `ready` when the
  server can answer, `degraded` when it works with reduced results, and
  `failed` when it cannot. `message` is user-facing text of the plugin, shown
  as given and cut to 1024 bytes on a character boundary. The latest status
  replaces the earlier one of the same server; a new
  server `generation` starts without one, and the host removes it when the
  server stops. It is not `lithe/function/progress`, which belongs to the
  function call that is running, while preparation outlives every call.
- The language server keeps no state of its own in the host: when the process
  stops or crashes, every request in flight ends with
  `plugin.host.languageServerUnavailable`, and the backend asks again after
  `didChangeState` reports `running` with a new `generation`.

Notifications to the backend, granted by the same permission:

| Notification | Params | Meaning |
| --- | --- | --- |
| `lithe/host/languageServer/didChangeState` | `{context, serverID, state, generation}` | `state` is `starting`, `running`, `stopped`, or `crashed`. |
| `lithe/host/languageServer/didReceiveNotification` | `{context, serverID, generation, method, params?, truncated?}` | A notification the server sent and `forwardNotifications` lists. |

`generation` is a positive integer the host raises each time it starts the
server for this context, so a backend can tell which process a state or a
notification belongs to; the host never delivers one of a replaced process
after the new one has started. A forwarded notification larger than
`limits.notificationBytes` is delivered once with `truncated: true` and
without `params`. Core enforces the request policy (whitelist, gates,
serialization, timeouts) when it relays a request, and
`fixtures/backend/function-protocol.json` holds the request, response, and
notification shapes every host must reproduce.

### Content providers

A plugin whose server hands out documents that have no file (for example the
decompiled source of a library class) serves them as a URI scheme:

```json
{
  "context": {"contextID": "w1", "generation": 1},
  "scheme": "metals-library",
  "function": "documents.read"
}
```

`lithe/host/documents/registerContentProvider`, granted by
`documents.contentProvider`, returns `{subscriptionID}`; dispose it with the
existing subscription API. Registration only records the binding and never
calls the backend; the function must already be registered on the session.

- A scheme is 1 to 64 lowercase ASCII letters, digits, `+`, `-`, or `.`, and
  starts with a letter. The schemes the host or the platform resolves are
  refused: `about`, `blob`, `data`, `file`, `ftp`, `http`, `https`,
  `javascript`, `lithe`, `mailto`, `untitled`, `ws`, and `wss`. A scheme has
  one provider per workspace window; registering it again is
  `plugin.host.preconditionFailed`.
- When a document whose URI has that scheme is opened, the host calls the
  function through `lithe/function/invoke` with the workspace `context` and
  `{uri}`. The answer is a string, or `{content: string}`, of at most 1 MiB.
  The document opens read-only; any other answer, a failure, or a timeout
  (the host waits 30 seconds) shows the user an error and opens nothing.
- The provider is the only source of the text. The host keeps no copy once
  the document closes, and it asks again when the document is reopened.
  Ending the session or closing the context withdraws the provider.

### Backend-only commands

A command in `ui.commands` may omit `toolWindowID`, and `toolWindows` may be
empty or omitted. A command without `toolWindowID` requires `backend`, the
`ui.contribute` and `backend.launch` permissions, and its `onCommand:<id>`
event. See `fixtures/backend/command-backend.plugin.json`.

The command appears through the ordinary workbench command contributions and
sends `commands/didRun` without opening a dummy tool window. Commands arriving
while initialize is pending wait until ready; a crashed backend can start again
on its declared command event. Background workspace activation has two concurrent
start slots; explicit command activation does not wait for unrelated background
plugins. `didRun` remains an event, not an acknowledgement that the command's
business action succeeded; a future result-bearing execute API must use the
existing request lifecycle rather than infer success from notification delivery.

### Independent editor actions

Editor actions are a host presentation capability of an ordinary tool backend,
not an LSP, Build, Run, Test or Debug provider. A play icon is presentation, not
an execution kind. The host understands the bounded range/title/icon/invocation
shape; HTTP/SQL parsing and execution remain private to the plugin. Native domain
workflows still use their own typed contracts, never a private-function substitute.

A backend declares `backend`, `documents.read` and `editor.actions`, discovers
`lithe/host/editor/registerActions` with `getCapabilities`, then registers per
workspace context:

```json
{
  "context": {"contextID": "w1", "generation": 1},
  "id": "requestActions",
  "function": "requests.collectActions",
  "selector": {"fileExtensions": ["http", "rest"], "fileNames": []}
}
```

- Registration returns `{subscriptionID}`. Dispose with the existing subscription
  API. It only records the binding and never calls the backend while answering;
  collection runs later. The function must already be registered on that session.
- No manifest `runMarkers` section or capability, no special command notification,
  and no language-server/Run declaration is added. The first implementation starts
  through existing `onWorkspaceOpen` (or an existing explicit entry); registration
  cannot discover a backend that has not started. Backend installation still needs
  restart. Standalone backends have an application-scoped enablement module,
  without a fake UI or language provider; safe mode/quarantine/disable still veto
  their startup. Example package: `fixtures/editor/editor-actions.plugin.json`.
- A selector has 1 to 32 literal names/extensions; extensions accept an optional
  leading dot. Matching is case-insensitive. At most 32 bindings per plugin across
  contexts; duplicate registration IDs in a context are refused.
- The host invokes the collection function via existing `lithe/function/invoke`
  with workspace context and `{document:{documentID,version,path},text}`. Version
  is the same public buffer version as `documents/getSnapshot`, not a WebView
  revision. Source is the dirty live buffer, capped at the host's
  `limits.editorActionSourceBytes`. Remote workspaces and
  read-only preview surfaces are not served.
- Collection is read-only by contract: no network request or business execution.
  Edits invalidate the old actions immediately and cancel older collection work;
  the host debounces fresh collection and rejects stale or withdrawn results.
  The host bounds the whole collection batch to eight seconds, not eight seconds per
  binding. Refresh acknowledges scheduling without waiting for collectors; a slow
  or failed collector must not fail editing or the save synchronization barrier.
- Return `{actions:[{id,range:{start:{line,column},end:{line,column}},title,icon,
  invoke:{functionID,params?}}]}`. Positions are one-based UTF-16, half-open and
  cannot split surrogate pairs. Each range stays in the exact input document.
  `id` is 1 to 128 bytes without controls, unique per answer; `title` is 1 to 200
  characters on one line. `icon` is a host-owned presentation token from
  `services.editor.options.actionIcons`. At most 1000 actions and 1 MiB serialized; malformed answers
  are refused whole. Same-line actions share a menu rather than silently replacing
  each other.
- An action may carry `alternatives`: at most 4 `{id,title,icon,invoke}` entries
  with the same rules and IDs unique within the action, offered in the action's
  menu after the action itself (for example Debug or Test beside Run).
- The Lithe desktop host draws `run`, `run-all`, `debug`, `test`, `test-passed`,
  `test-failed`, `test-skipped`, `navigate-super` and `navigate-implementations`.
  An icon names a picture, not an operation: a test icon does not make the
  action a test, and the host never starts anything because of an icon.
- Each invocation target is a currently registered function of the same plugin;
  `params` is a plugin-private JSON object. The host stores it behind an opaque
  per-batch action key, never accepts invocation payloads from the web editor, and
  validates document/context/session/registration again on click. It calls the
  function with `{documentID,expectedVersion,actionID,data:<params>}` through the
  ordinary invocation lifecycle; a chosen alternative adds `alternativeID` and
  passes that alternative's `params` as `data`. The plugin validates/re-reads the
  live document before side effects.
- A click result may carry `{reveal:{locations:[{uri,range}]}}`: 1 to 100
  absolute URIs with zero-based LSP ranges. The host opens a single location and
  lists several in its navigation results. Other result members stay the
  plugin's own.
- A click that runs or debugs code starts it through the host's run and debug
  methods (`lithe/host/run/start`, `lithe/host/debug/start`) once
  `getCapabilities` lists them, so the run appears in the host's Run and Debug
  windows. A private process started by the plugin is not a substitute. Click executions are not cancelled by unrelated re-parsing;
  withdrawing the binding, ending its session/context or tearing down the owning
  editor cancels them. The host never retries side effects automatically.
- Toolbar and menu Run/Debug in the Lithe desktop reuse these actions for a language the
  package declares in `languages` without an execution or debug module and
  without the matching `hostCapabilities` entry: with a current-file Run
  configuration the host saves the document, collects the document's actions as
  the gutter does, and invokes the first action (ordered by range start) whose
  icon is `run` or `run-all`; Debug invokes that action's first alternative whose
  icon is `debug`, else the first action whose own icon is `debug`. No matching
  action, a stale document or a withdrawn binding produces a user-visible notice
  and nothing starts. This is only a different entry point to the plugin's own
  function; the host still never starts code because of an icon, so the plugin
  decides what the action does and still uses `lithe/host/run/start` or
  `lithe/host/debug/start` to run it.
- Registration/collection validation is the shared Core command
  `plugins.editorActions.decode`. Its request is
  `{version, kind, icons, sourceBytes, text, value}`: `version` is required and
  is `0` (Core accepts only `0`, otherwise `Unsupported editor action
  validation version.`), `kind` is `registration`, `actions` or
  `invokeResult`, and `kind: actions` also needs the host's `icons` and
  `sourceBytes` (the `actionIcons` and source bound it reports). It admits
  alternatives and click results (`kind: invokeResult`). Cases:
  `fixtures/editor/editor-actions.json`. Public host failures use the
  distinct codes (`invalidParams`, `permissionDenied`, `versionConflict`,
  etc.). No tool-private error or result is interpreted as a build/test
  result.
- `commands/didRun {commandID,context?}` is the command notification. A
  command-palette entry can read the document/selection and call the same
  plugin implementation.
- A host that does not list `lithe/host/editor/registerActions` in
  `getCapabilities` presents no editor actions, so a plugin keeps its
  command/window entry point. A package requiring `editor.actions` is refused by
  a host that does not recognize that permission, rather than silently granted.
  Grammar capability admission is separate.

### Editor language-feature providers

A provider adds a language feature the file's language server does not
supply: a cross-file fact only the plugin knows, such as a key in a template
that names an entry in a message bundle, or a framework declaration in a
configuration file that names a method in source. It never replaces the
language server. Where a language server answers, its answer comes first and
plugin results only add to it; a plugin that repeats what the server already
says shows the user the same thing twice.

A backend declares `backend`, `documents.read` and `editor.providers`,
finds `lithe/host/editor/registerProviders` and
`services.editor.options.providerTypes` with `getCapabilities`, then registers
per workspace context:

```json
{
  "context": {"contextID": "w1", "generation": 1},
  "id": "message-keys",
  "selector": {"languages": ["html"], "fileExtensions": ["properties"], "fileNames": []},
  "providers": {
    "definition": {"function": "keys.definition"},
    "hover": {"function": "keys.hover"},
    "completion": {"function": "keys.complete", "triggerCharacters": ["."]},
    "semanticTokens": {"function": "keys.tokens",
      "legend": {"tokenTypes": ["property", "string"], "tokenModifiers": ["declaration"]}},
    "colorDecorations": {"function": "rainbow.decorate",
      "colorLegend": [{"id": "depth-0", "light": "#B03060", "dark": "#F08AAA"}]}
  }
}
```

- Registration returns `{subscriptionID}`; dispose it with the subscription
  API. Ending the session, the context or the plugin's activation withdraws
  every registration it made, and a plugin activated later registers again.
  Each named function must already be registered on that session.
- A selector has 1 to 32 entries in total: language IDs, literal extensions
  (optional leading dot) and literal file names, matched case-insensitively.
  A document matches when any entry matches. At most 32 registrations per
  plugin across contexts; duplicate IDs in a context are refused.
- `providers` maps a provider type to its function and any type-specific options. The Lithe desktop host routes
  `definition`, `hover`, `completion`, `codeLens`, `foldingRange`,
  `semanticTokens`, `colorDecorations` and `documentLink`. Completion may add 1 to 16
  single-character `triggerCharacters`; semantic tokens must add their
  `legend` (at most 64 types and 64 modifiers). `colorDecorations` adds a
  `colorLegend` of 1 to 64 unique `{id, light, dark}` entries; each color is
  `#RRGGBB`. The provider returns `{decorations: [{range, colorId}]}` with
  nonempty, single-line, zero-based UTF-16 ranges. Ranges from one source may
  touch but must not overlap. Each provider result is limited to 10,000 ranges.
  A merged answer accepts at most 64 sources and 10,000 input ranges total;
  its output is limited to 10,000 ranges and 1 MiB. A type
  outside the host's
  `providerTypes` fails with `unsupportedOption` and
  `details.option: "providers.<type>"` before anything is registered.
  Diagnostics are not a provider type.
- The [`colorDecorations` registration and result schema](editor-color-decorations.schema.json)
  describes the provider-specific JSON shape. Registration itself remains a
  runtime `editor/registerProviders` request and is validated by Core, not a
  plugin manifest field.
- The host calls the function through `lithe/function/invoke` with the
  workspace context and `params: {document: {documentID, version}}`; definition, hover and
  completion add `position: {line,character}` (zero-based, UTF-16), and
  completion adds `triggerCharacter` when typing started it. The plugin reads
  the text with `documents/getSnapshot` and answers for that version.
- Results use the LSP 3.17 shapes of the matching request: definition
  `Location | Location[] | LocationLink[] | null`, `Hover | null`,
  `CompletionItem[] | CompletionList | null` (no `data`, `command` or resolve
  round trip), `CodeLens[] | null`, `FoldingRange[] | null`,
  `SemanticTokens | null` indexed by the registered legend,
  `{decorations: [{range, colorId}]}` for `colorDecorations`, and
  `DocumentLink[] | null` with absolute targets. Every code lens carries its
  `command`, whose `command` is a function of the same plugin and whose
  `arguments` holds at most one params object; a click calls that function
  with `{documentID,expectedVersion,data:<params>}`. A malformed result is
  dropped whole.
- Budgets are the host's: hover and completion 400 ms, definition 1500 ms,
  the rest 2000 ms. A late answer, or one for an older document version, is
  dropped without an error. A registration cannot set its own budget.
- The host merges: hover sections and completion items follow the language
  server's; definitions add plugin locations to the server's (a location at
  the requested position itself yields to the plugin's); code lenses, folding
  ranges and links are added; semantic tokens apply only to files whose
  language server has none. Color decorations are independent of semantic
  tokens and are merged across matching sources by
  `plugins.editorProviders.decode` with `kind: "mergeColorDecorations"`.
  The host supplies sources using the stable `sourceId` form
  `<pluginID>/<registrationID>`. Core sorts sources by `sourceId` using
  Unicode code-point order; the lexicographically earliest source wins at
  each UTF-16 position, and later ranges are clipped to uncovered portions.
  The merged result is sorted by line and start position and contains resolved
  light/dark colors. A plugin location equal to one already kept is
  dropped. The merge is Core command `plugins.editorProviders.decode` with
  `kind: "mergeDefinitions"`, so every host merges definitions alike.
- Validation is shared Core command `plugins.editorProviders.decode`. Cases:
  `fixtures/editor/editor-providers.json`. Example package:
  `fixtures/editor/editor-providers.plugin.json`.
- A host that does not list `lithe/host/editor/registerProviders` in
  `getCapabilities` presents no editor providers. A package requiring
  `editor.providers` is refused by a host that does not recognize the
  permission.

### Cross-plugin calls

A plugin function is private to its plugin unless the plugin exports it. An
exported function can be called by another enabled plugin whose manifest
declares that call. Core relays every call: the calling backend never
reaches the provider's process, the provider's backend never learns more than
the call it serves, and the host takes part only by starting a provider's
backend that is not running (see below). The relay is Rust Core logic, the same
on every platform that runs plugin backends.
`fixtures/backend/plugin-functions-exporter.plugin.json` and
`fixtures/backend/plugin-functions-caller.plugin.json` are the examples.

#### Declaring exports and calls

```json
"requires": { "pluginFunctions": 0, "backend": 0 },
"pluginFunctions": {
  "exports": [{ "id": "document.format", "version": 1, "context": "workspace" }],
  "calls": [{ "id": "project.describe", "version": 2 }]
}
```

- `pluginFunctions` needs a `backend`, and `requires` lists
  `pluginFunctions: 0` and `backend: 0`. The section holds at least one of
  `exports` and `calls`, each at most 32 entries sorted by `id`, with no `id`
  twice. Anything else, including an unknown field, is
  `plugin.manifest.invalid`. `requires` lists `pluginFunctions` exactly when
  the section exists.
- An export is an object, never a bare function ID. `id` is a function ID
  as in [References and IDs](#references-and-ids). `version` is the
  exported contract version, an integer from 1 to 1000; an incompatible
  change to the params or result takes a new version. `context` is a closed
  set that holds only `"workspace"`: the function runs only for a local
  workspace context, on a call Core relays. A missing or unknown
  `context` refuses the whole manifest with `plugin.manifest.invalid`, so a
  function whose context is unstated is never callable.
- A call is `{ id, version }`. It names a function ID and the version the
  caller was written against, never the providing plugin's ID: the caller
  discovers providers at run time. Core requires the versions to be equal;
  otherwise the call fails with `plugin.function.versionMismatch`.
- Declaring an export does not register it. The provider's backend still
  registers the function with `lithe/function/register`, and a function it
  has not registered, or has withdrawn, is `plugin.function.notExported`.
  A function the manifest does not export is private whatever the backend
  registers, and a reference to it keeps answering
  `plugin.function.wrongPlugin`.
- A plugin that exports needs no page, command or `onWorkspaceOpen` to start
  its backend: a call starts it. A plugin that only calls needs one of
  those, as every [`backend`](#backend-lifecycle-and-workspace-contexts) does.
- `pluginFunctions` grants no manifest permission. The caller's `calls`
  and the provider's `exports` are the two sides' consent, and the host
  grants a plugin that declares `calls` the `functions` host service,
  through `backend.hostServices` of its launch plan, like every other
  service. A host without `pluginFunctions` in `hostCapabilities` refuses
  such a manifest with `plugin.api.unsupported`.

#### What an exported function must and must not do

- **Trust is not transitive.** The provider runs with its own permissions,
  its own state directory and its own credentials. The caller gains none of
  them, and the provider gains none of the caller's. Core never writes the
  caller's plugin ID into the provider's session; it appears only in the
  `caller` field of one request, below.
- **The provider owns its resources and processes.** An exported function
  must not accept an executable path, command line or script from its caller
  and start a process with it, nor read or write a path the caller chose
  without checking it against what the provider itself may access. If a
  caller needs the provider to run something, the function takes a request
  the provider interprets (a task name, a document) and the provider decides
  the executable.
- A function that decides by who asks reads the `caller` field below, never
  a plugin ID in `params`, which any caller can write.

#### Discovery and invocation

A backend whose plugin declares at least one call may send two requests; a
plugin that declares no call gets `-32010`, `plugin.host.permissionDenied`.
`getCapabilities` lists the methods the session may call under the `functions`
service.

| Request | Params | Result |
| --- | --- | --- |
| `lithe/host/functions/providers` | `{ functionID }` | `{ providers: [{ pluginID, version }] }` |
| `lithe/host/functions/invoke` | `{ pluginID, functionID, context: { contextID, generation }, params?, timeoutMilliseconds?, parentOperationID? }` | `{ value }` |

- `providers` lists the enabled plugins other than the caller that export
  `functionID`, sorted by plugin ID, each with its exported `version`.
  Disabling, enabling, replacing and uninstalling a plugin take effect for
  the list at once; a host that registers backend plugins only when it
  launches lists a newly installed or updated backend plugin
  only after a restart of Lithe (see [Hot activation](#hot-activation)). The
  caller must have declared the function, otherwise
  `plugin.function.notDeclared`. A listed provider's backend need not be
  running.
- `invoke` calls one provider. `context` is required: a workspace context
  the caller learned from the [workspace service](#workspace-contexts-and-capability-discovery),
  checked like every call context. `params` is a JSON object and defaults
  to `{}`. `timeoutMilliseconds` is a positive integer, defaults to 60 000
  and is capped at 600 000. A field the method does not define is `-32018`,
  `plugin.host.unsupportedOption`. The reply is `{ value }`, where `value`
  is the provider's result (`null` is a result).
- `parentOperationID` is optional: a string of 1 to 128 bytes, otherwise
  `-32602`, `plugin.host.invalidParams`. A backend that makes a nested call
  while it serves a relayed call sets it to the `operationID` of the
  `lithe/function/invoke` it is serving (the field shown in
  [The caller field](#the-caller-field)), so Core knows which call the new
  one belongs to. Its effects are described in
  [The call chain](#the-call-chain). A backend should always set it: without
  it Core treats the new call as a child of every relayed call the backend is
  serving.
- The caller cancels a call with `$/cancelRequest` for its `invoke` request
  id; it fails with `plugin.host.cancelled`.

Core checks, in this order, and refuses a call before anything is written to
the provider; the first failing check wins:

| Check | Code |
| --- | --- |
| The caller declared the function | `plugin.function.notDeclared` |
| The provider is installed and enabled | `plugin.function.providerDisabled` |
| The provider exports the function | `plugin.function.notExported` |
| The declared and exported versions are equal | `plugin.function.versionMismatch` |
| The context is open, and has this generation | `plugin.host.contextExpired` |
| The params are at most 256 KiB | `plugin.function.payloadTooLarge` |
| The call named by `parentOperationID` is still running | `plugin.host.cancelled` |
| The provider is not on the call chain, the caller included | `plugin.function.callCycle` |
| The chain has at most 4 calls | `plugin.function.depthExceeded` |
| The caller has at most 8 relayed calls running | `plugin.function.tooManyCalls` |

A provider's own failure keeps its meaning. The reply is a JSON-RPC error
with `data: { "code", "details" }`; when the provider's function answered with
an error, `code` is `plugin.function.failed` (`-32070`), `message` is the
provider's message cut to 1024 bytes, and `details.pluginErrorCode` is the
provider's own `error.data.code`. A reply with no `result`, or with one over
1 MiB serialized, is `plugin.function.invalidResult` (`-32071`); any other
result over 256 KiB is `plugin.function.payloadTooLarge`. Neither number is
`-32603`, which stays `plugin.host.failed`, a failure inside Core. The relay's deadline passing is
`plugin.host.timeout`. A call that is running normally ends through the
provider session's own timeout; if the provider has still not answered 2
seconds after the deadline, the relay ends the call with
`plugin.host.timeout` itself and cancels the provider's operation. A
provider whose backend exits, or that withdraws
the function, while the call runs ends it with
`plugin.function.providerUnavailable`; a provider that is disabled or
uninstalled, or whose update stops exporting the function or changes its
version, ends it with `plugin.function.providerDisabled`. A caller whose
update stops declaring the call ends it with `plugin.function.notDeclared`.
A workspace context that the host closes, or replaces with a newer
generation, ends the relayed calls that carry it with
`plugin.host.contextExpired`: both the calls still waiting for a provider and
the calls already running, whose provider also receives `$/cancelRequest`.
Core never restarts a backend or retries a call to finish one.

`fixtures/backend/function-protocol.json` holds the cases: the host requests
(`hostRequests`), the numbers of every failure (`relayErrors`) and the
`caller` field (`invocationCallers`).

#### The call chain

Core records the chain a call belongs to. A provider that makes a call while it
serves a relayed call names that call in `parentOperationID`. The new call is
then a child of that call only:

- Its chain is the chain of that call plus the caller.
- Cancelling that call, its timing out, or its ending cancels the new call.

A call to a plugin already on the chain, including a plugin calling itself, is
`plugin.function.callCycle`: the plugin on the chain is waiting for the call
that would reach it. A chain has at most 4 calls. So a provider that is asked
to apply values must not call back the plugin that asked inside that call; it
records the values, or calls after the call ends.

- If `parentOperationID` names a call the host started directly, not a relayed
  one, the new call starts a new chain.
- If it names a call this backend is not serving (one that has finished, or
  one that never existed), the call is refused with `plugin.host.cancelled`
  and the message "The call this request belongs to is not running."
- If it is omitted, the call counts as a child of every relayed call the
  backend is serving at that moment; SDKs should always fill it in.

Cancelling a call, its timing out or ending, or the end of its caller's
session, cancels the calls made as its children, down the chain, and each
provider receives `$/cancelRequest`. The relay never retries a call.

#### Isolation between backends

Core writes to each backend session through a queue of its own. A backend
that stops reading its standard
input only delays the calls and replies sent to it; the relayed calls of other
plugins continue. If a write to a backend stays blocked past the request
timeout, Core ends that backend's session as a transport failure
(`transportFailed`, stage `relayWrite`), and the calls of that session end like
those of any session failure.

#### The caller field

For every relayed call Core forwards `lithe/function/invoke` to the provider
as for any call (see [Invocation](#invocation)) and adds one top-level field:

```json
{
  "functionID": "project.describe",
  "operationID": "7",
  "context": { "contextID": "w1", "generation": 1 },
  "params": { "project": "app" },
  "caller": { "pluginID": "dev.lithe.fixture.python" }
}
```

- `caller.pluginID` is written by Core from the identity of the calling
  session. It is a sibling of `functionID`, `operationID`, `params` and
  `context`, never inside `params`, and the caller cannot supply it: a
  `lithe/host/functions/invoke` request carrying a top-level `caller` is
  refused with `-32018`, `plugin.host.unsupportedOption`. A `caller` key
  inside the caller's own `params` is ordinary data and is passed unchanged.
- A call the host itself started has no `caller` field. Absent means the
  host.
- A provider that keeps per-caller state (for example a slot per calling
  plugin) keys it by this ID, which cannot be forged.

#### Providers that are not running

If the provider's backend is not running, Core waits for it and emits the
runtime event `functionProviderStartRequired` with
`result: { "pluginID" }` on the calling session, once per waiting provider.
The host starts that plugin's backend as an ordinary plugin start, and Core
dispatches the call when the provider has negotiated the protocol and registered
the function. If that takes longer than 30 seconds the call fails with
`plugin.function.providerUnavailable`; the relay's deadline still applies.
A host that cannot start the backend ignores the event, and the call ends the
same way.

#### Remote workspaces

Backends run on the machine that runs Lithe for a remote workspace too, so the
relay dispatches a call whose context is a remote workspace like a local one,
and the exported `context` value `"workspace"` means a workspace of either
authority. The callee learns the authority from `getContext`, and its own host
requests for that context reach the remote host or are
`plugin.host.authorityUnsupported` by the rules of each service; the relay
neither reads nor rewrites paths in the params or the result.

#### Hot activation

Disabling, enabling, replacing and uninstalling a plugin take effect for the
relay at once and need no restart of Lithe or of any other plugin.
The host sends the plugin and workspace-context snapshot with the Core
command `lsp.configureFunctionRelay`
([Rust Core API](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/contracts/rust-core-api.md)),
and calls the new snapshot no longer allows end at once with the errors above.

Installing or updating a plugin that has a backend is different on a host that
registers backend plugins only when it launches: that host lists
a newly installed or updated backend plugin in `providers` only after a
restart of Lithe.

### Compatibility

- Private functions add no manifest field. `pluginFunctions` is a capability
  (`requires.pluginFunctions: 0`), so only a plugin that exports or calls
  functions is refused by a host that cannot relay them.
- A backend that does not answer `litheFunctions` (a build-only backend) keeps
  serving its language and its `litheBuild` requests and receives no function
  calls; its function requests are answered with `-32601`.
- A new host capability or optional field is added within the version
  ([Extending the host services](#extending-the-host-services)); only a change
  an existing backend would misread gives `backend` version 1, and the protocol
  moves with it.
- `fixtures/backend/function-protocol.json` holds the golden cases: host
  requests (`hostRequests`), call contexts (`invocationContexts`), the caller
  of a relayed call (`invocationCallers`), host failure codes
  (`hostFailures`), relay failure codes (`relayErrors`), and host
  notifications (`hostNotifications`). Rust Core tests run them, and a
  scripted backend covers routing, result, failure, cancellation, deadline,
  late reply, revocation, and a build-only backend.

### Official backend SDK

The official SDKs live next to this contract in
[`shared/plugins/sdk/`](../../sdk) and are released from this repository;
they implement this contract and never define protocol.

[`sdk/node`](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/shared/plugins/sdk/node/README.md) is the official SDK for backends
started through a `node` runtime launch. It implements this protocol with
its host services, `litheBuild`, and VCS providers, so a plugin author writes
only handlers. It is released (`sdk-node-v<version>`) as a `.tgz` with its
SHA-256 for plugins to vendor into their package. It enforces the rules above on the backend side: `lithe/*` methods are never sent, stdout
carries only protocol frames, `PluginError` is the only way to set a
`pluginErrorCode`, processes started for a call end with it, and its state
store refuses the plugin package.

[`sdk/java`](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/shared/plugins/sdk/java/README.md) is the Java twin for backends
started through a `java` runtime launch. It implements this protocol with
its host services and `litheBuild`, but not VCS providers. It needs Java 8 or
newer, has no runtime dependencies, and is released as a JAR with its SHA-256
for plugins to shade into their backend. It enforces the same rules with the same error
mapping, with one difference: on Java 8 it stops only a cancelled tool's
direct child, and the host's process scope ends the rest.

[`sdk/rust`](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/shared/plugins/sdk/rust/README.md) is the SDK for
[plugin backends](#plugin-backend) and own-process
[build providers](#build-providers), which ship as packaged executables. It
implements this protocol with its host services and `litheBuild`, but not VCS
providers. It needs Rust 1.87 or newer, depends only on `serde`, `serde_json`
and `base64`, and is released (`sdk-rust-v<version>`) as a `.crate` archive with its SHA-256, or
used as a Git dependency pinned by the tag. It enforces the same rules: on Unix it
points stdout at stderr so only protocol frames reach the host, and a panic
ends only its call, reported like any unexpected failure.

[`sdk/go`](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/shared/plugins/sdk/go/README.md) is the Go SDK for
[plugin backends](#plugin-backend), which ship as packaged executables. It
implements this protocol with its host services and `litheKeymapImport`, but
neither build nor VCS providers. It needs Go 1.27.4 or newer, uses only the
standard library, and is released as Go module tags
(`shared/plugins/sdk/go/v<version>`) that plugins pin through `go.sum`. It
enforces the same rules: on Unix it moves the protocol to a duplicate of the
stdout descriptor and points stdout at stderr so only protocol frames reach the
host, and a panic in a handler ends only that call, reported like any
unexpected failure.

The Node.js and Java SDKs each have a sample build provider
(`sdk/*/examples/sample-build`) that answers one end-to-end scenario driven
through a real Core session; the Rust and Go SDKs' sample backends
(`sdk/rust/examples/sample-backend`, `sdk/go/examples/sample-backend`) answer
the same plugin backend scenario, in which
the test plays the host: it serves the credential requests and the
workspace, file watch and window requests, delivers a command and a context
change, and checks that cancellation reaches the host. That repository runs
them against the Lithe commit its contract copy pins, and weekly against the
latest development branch of Lithe.

No SDK adds a wire field: a backend written without one is equally valid.

## Outcomes and error codes

`plugins.declarative.*`, `plugins.marketplace.*`, `build.*`, `vcs.*`, and `project.*` commands return `{"status": "ok", …}` or
`{"status": "rejected", "error": {"code", "message", "field"?}}`. A malformed
command payload is a Core `invalid_request` failure instead.

| Code | Reported by | Meaning |
| --- | --- | --- |
| `plugin.manifest.invalid` | Core | Malformed manifest, unknown field, or broken cross reference |
| `plugin.schema.unsupported` | Core | `schemaVersion` is not `0` |
| `plugin.api.unsupported` | Core | `pluginAPIVersion`, or a capability version in `requires`, is not supported by this Core build or, when the host sent `hostCapabilities`, by the host |
| `plugin.permission.unknown` | Core | Unknown permission |
| `plugin.permission.undeclared` | Core | A provider or build system is declared without its permission |
| `plugin.activation.invalid` | Core | Missing or orphan activation event |
| `plugin.runtime.undeclared` | Core | A tool that a launch, a runtime dependency or a debug adapter names and `requiresTools` does not list; `field` names where it is used |
| `plugin.run.invalidStart` | Core | The params of `lithe/host/run/start` are malformed, name a tool the manifest does not require, or leave the workspace or package; `field` names the part |
| `plugin.debug.invalidStart` | Core | The params of `lithe/host/debug/start` are malformed, or `configuration` holds a key the host reserves; `field` names the part |
| `plugin.debug.unknownAdapter` | Core | `lithe/host/debug/start` names an adapter the plugin does not declare |
| `plugin.debug.foreignAdapter` | Core | `lithe/host/debug/start` names an adapter of another plugin |
| `plugin.debug.limitExceeded` | Core | A `debug/start` configuration is larger than 256 KiB |
| `plugin.toolchain.launchMismatch` | Core | A tool definition's `launch` template cannot carry the entry, runtime arguments or arguments a launch supplies, or needs a part the launch left out |
| `plugin.template.invalid` | Core | Unknown, misplaced, or unguarded template variable |
| `plugin.path.invalid` | Core | A request path leaves the workspace, or a declared path leaves its base directory |
| `plugin.test.invalidIdentifier` | Core | Empty or multi-line test identifier |
| `plugin.test.unsupportedProject` | Core | No project marker in the workspace |
| `plugin.execution.unavailable` | Core | The plugin has no run/test provider for the language |
| `plugin.integration.unsupported` | Core | A `hostIntegration` ID; Core implements none |
| `plugin.launch.unavailable` | Core | The plugin has no language server for the language, no provider process for the build system, or no such VCS provider |
| `plugin.build.unavailable` | Core | The plugin declares no such build system, or the build system does not declare the requested operation |
| `plugin.build.invalidSelection` | Core | A build request selects a scope, target, filter, or argument the declaration does not allow, or misses a required path |
| `plugin.build.invalidOutput` | Core | Build-tool output or a report does not fit its decoder |
| `plugin.build.untrustedWorkspace` | Core | The plan would run project code and the user has not trusted the workspace; see [Workspace trust](#workspace-trust) |
| `plugin.build.invalidRunConfiguration` | Core | A saved configuration of a [run configuration type](#run-configuration-types) holds a value the type no longer accepts; `field` is `values.<fieldID>` |
| `plugin.build.invalidBeforeLaunchTask` | Core | A step of a [before-launch task](#before-launch-tasks) holds a value the task no longer accepts; `field` is `values.<fieldID>` |
| `plugin.build.invalidLocalization` | Core | The `runConfigurationTypes`, `beforeLaunchTasks` or `buildOptions` part of a [translation file](#translations) does not fit the declarations; the host shows the next candidate or the manifest text |
| `plugin.launch.invalidRequest` | Core | A launch plan request has an unusable directory or an unknown architecture |
| `plugin.launch.untrustedWorkspace` | Core | The process would run project code and the user has not trusted the workspace; see [Workspace trust](#workspace-trust) |
| `plugin.launch.notBackend` | Core | `decodeLaunch` named a language server that the plugin backend does not launch |
| `plugin.launch.invalidResult` | Core | A [backend-resolved launch](#backend-resolved-launch) answer is malformed, unknown in a field, or over a bound |
| `plugin.launch.outsidePackage` | Core | The answer's packaged executable leaves the plugin package |
| `plugin.launch.undeclaredTool` | Core | The answer's tool is not one of the plugin's tool definitions |
| `plugin.launch.unwritablePath` | Core | The answer writes outside the state, workspace state, log, and temporary directories |
| `plugin.grammar.invalid` | Core | A language's grammar file is not a TextMate grammar Core accepts; `field` names the failing part |
| `plugin.grammar.unavailable` | Core | The plugin declares no grammar for the language |
| `plugin.settings.invalidLocalization` | Core | A [translation file](#translations) does not fit its manifest or format; the host shows the next candidate or the manifest text |
| `plugin.vcs.reserved` | Core | A VCS provider claims the ID of the built-in Git support |
| `plugin.vcs.invalidOutput` | Core | A VCS provider's reply does not fit the [VCS provider contract](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/contracts/vcs-provider-v1.md) |
| `plugin.index.invalid` | Core | Malformed index root or entry, or an invalid URL, checksum, platform, version, or date |
| `plugin.index.schemaUnsupported` | Core | The index `schemaVersion` is not `0` |
| `plugin.index.officialNamespace` | Core | A third-party source lists a `dev.lithe` ID |
| `plugin.index.duplicate` | Core | An index lists the same plugin ID more than once |
| `plugin.index.platformUnsupported` | Core | No version has an archive for this platform (availability, not a rejection) |
| `plugin.index.apiUnsupported` | Core | Only versions needing a plugin API or a capability version the host does not run list this platform (availability, not a rejection) |
| `plugin.recommendations.invalid` | Core | Malformed companion recommendation catalog root or entry (`pluginID`, `markers`, `extensions`, or `coveredBy`) |
| `plugin.recommendations.schemaUnsupported` | Core | The companion recommendation catalog `schemaVersion` is not `0` |
| `plugin.recommendations.duplicate` | Core | A companion recommendation catalog lists the same `pluginID` more than once |
| `plugin.source.invalid` | Core | A marketplace source URL cannot be used |
| `plugin.package.mismatch` | Core | A downloaded package's ID or version differs from its index entry |
| `plugin.archive.checksumMismatch` | Host | A downloaded archive does not match the index SHA-256 |
| `plugin.download.failed` | Host | An index or archive could not be downloaded |
| `plugin.signature.missing`, `plugin.signature.invalid`, `plugin.signature.untrusted` | Host | Package signature problems |
| `plugin.launch.missing` | Host | The packaged language server is missing or not executable |
| `plugin.command.missing` | Host | None of the commands a `systemCommand` launch names is on `PATH`, in the plugin's managed command directory, or in a default directory the host recorded; the host offers the [command installer](#command-installers) when there is one |
| `plugin.commandInstaller.unknown` | Core | `installerID` names no command installer of the plugin |
| `plugin.commandInstaller.targetUndeclared` | Core | The installer does not declare the requested target (`field: "target"`), or has no `toolDefault` to read a default directory for (`field: "installerID"`) |
| `plugin.commandInstaller.directoryInvalid` | Core | A tool installer's `managed` plan has no absolute `installDirectory` for this platform, or a `toolDefault` or download plan was sent one |
| `plugin.commandInstaller.probeOutputsInvalid` | Core | `outputs` does not hold one entry for each directory probe |
| `plugin.commandInstaller.platformInvalid` | Core | `platform` names an operating system or architecture no archive may target, or a download plan was sent no `platform` |
| `plugin.commandInstaller.unavailable` | Core | A download installer has no archive for the requested `platform` |
| `plugin.commandInstaller.failed` | Host | A tool installation exited unsuccessfully |
| `plugin.commandInstaller.downloadFailed` | Host | A download installation could not download its archive from `url` or any mirror |
| `plugin.commandInstaller.checksumMismatch` | Host | A downloaded archive does not match its pinned SHA-256 |
| `plugin.commandInstaller.archiveInvalid` | Host | A downloaded archive is not a safe `zip` or `tar.gz` below its `stripPrefix` |
| `plugin.commandInstaller.timedOut` | Host | The installation ran past `timeoutMilliseconds` and was stopped |
| `plugin.commandInstaller.commandsMissing` | Host | The installation finished but a command it provides is not in its directory |
| `plugin.runtime.missing` | Host | A tool a launch needs cannot be found, or is older than its `minimumVersion`, when the server starts |
| `plugin.web.unknownMethod` | Host | A plugin page called a bridge method the host does not implement |
| `plugin.web.invalidRequest` | Host | A bridge call has malformed parameters |
| `plugin.web.forbidden` | Host | A bridge call names an instance, run, or path outside the page's plugin or workspace, or the page is closed |
| `plugin.function.invalidReference`, `plugin.function.invalidParams`, `plugin.function.unavailable`, `plugin.function.wrongPlugin`, `plugin.function.unknown` | Core | A [plugin-function](#plugin-functions) call is refused before it reaches the backend |
| `plugin.function.failed`, `plugin.function.invalidResult`, `plugin.function.revoked` | Core | A plugin-function call ended without a usable result |
| `plugin.function.invalidRegistration` | Core | A backend's register or unregister request was refused as a whole |
| `plugin.function.notDeclared`, `plugin.function.notExported`, `plugin.function.providerDisabled`, `plugin.function.versionMismatch` | Core | A [relayed call](#cross-plugin-calls) is refused: the caller did not declare it, the provider does not export (or has not registered) the function, the provider is not enabled, or the declared and exported versions differ |
| `plugin.function.callCycle`, `plugin.function.depthExceeded`, `plugin.function.tooManyCalls`, `plugin.function.payloadTooLarge` | Core | A relayed call is refused by a bound: the provider is already on the call chain, the chain is too long, the caller has too many relayed calls running, or the params or result are too large |
| `plugin.function.providerUnavailable` | Core | The provider's backend did not start in time, exited, or withdrew the function while a relayed call waited for it |
| `plugin.project.unavailable` | Core | The plugin declares no such project generator |
| `plugin.project.invalidInput` | Core | The project name or a field value is not accepted; `field` is `projectName` or `values.<id>`, or a step whose expanded path leaves the project |
| `plugin.project.templateInvalid` | Core | A template directory is missing, too large, contains a symbolic link or non-UTF-8 file, or uses an unknown variable |
| `plugin.project.invalidURL` | Core | An archive address does not expand to an HTTPS URL (HTTP only for loopback hosts) without credentials, query, or fragment |
| `plugin.project.archiveInvalid` | Core | A downloaded archive is not a zip, has an entry outside the destination, a symbolic link, a file that already exists, or expands beyond the size limit |
| `plugin.project.generationFailed` | Host | A command step failed or timed out, a download failed, or the project could not be moved into place |
| `plugin.function.recursiveCall` | Core | A backend asked the host to dispatch a domain operation or function |
| `plugin.host.unavailable` | Core | A backend asked for a host method the protocol does not define |
| `plugin.host.permissionDenied` | Core | A backend asked for a [host service](#host-services) its plugin's permissions do not grant |
| `plugin.host.invalidParams` | Core | A host-service request has malformed params |
| `plugin.host.unsupportedOption` | Core, Host | A host-service request names a field the host does not implement; `details.option` names it |
| `plugin.host.failed` | Core | The host could not complete a host-service request |
| `plugin.host.contextExpired` | Host | The request names a workspace context that closed or has a newer generation |
| `plugin.host.authorityUnsupported` | Host | The context's authority (for example a remote workspace) does not implement the operation |
| `plugin.host.versionConflict` | Host | A versioned write names an older version than the current one |
| `plugin.host.limitExceeded` | Core, Host | A host limit on size, count, or rate was exceeded |
| `plugin.host.cancelled` | Core, Host | The backend cancelled the host request, or the host stopped it |
| `plugin.host.timeout` | Host | The host's own deadline for the operation passed |
| `plugin.host.notFound` | Host | The named resource does not exist |
| `plugin.host.toolUnresolved` | Host | A tool definition the request names resolves to no executable; `details` is `{ tool, reason }` |
| `plugin.host.readOnly` | Host | The resource or workspace cannot be written |
| `plugin.host.preconditionFailed` | Host | A state the request requires does not hold |
| `plugin.host.busy` | Host | The resource is in use; retry later |
| `plugin.host.rateLimited` | Host | Too many requests in a period |
| `plugin.host.languageServerUnavailable` | Core | The plugin's language server is not running for the context, or stopped while a request was in flight |
| `plugin.host.languageServerError` | Core | The language server answered a forwarded request with an error; `details` is `{code, message}` |
| `plugin.host.methodNotDeclared` | Core | A forwarded method or command is not in the server's declared session |
| `plugin.host.foreignContribution` | Core | The named language server belongs to another plugin |
| `plugin.host.remoteDisconnected` | Host | The connection to the remote host was lost while the operation ran |
| `plugin.host.platformUnavailable` | Host | The plugin has no package or managed runtime for the remote host's platform |
| `plugin.host.alreadyExists` | Host | The target of a file change already exists |
| `plugin.host.diskChanged` | Host | A save found the file changed or deleted on disk and wrote nothing |
| `plugin.backend.exited` | Host | A page's call, or a host operation for the backend, ended because the backend process exited |

Host-facing plugin states are `notInstalled`, `installed` (enabled or
disabled), `running`, `restartRequired`, `updateRequired`, and `failed`.

## Host-owned workbench UI

`ui` contributions need `requires.ui: 0` and the `ui.contribute` permission.
UI-only packages may omit `languages`. See `fixtures/manifest/hello-lithe.plugin.json`
and the UI cases in `fixtures/manifest/invalid-manifests.json`.

- `ui.toolWindows`: 1–64 entries with stable `id`, `title`, initial `placement`
  (`left`, `bottom` or `right`), integer `order` (default 0), and plain `text`.
- `ui.commands`: up to 128 entries with stable `id`, `title`, `toolWindowID`,
  integer `order` (default 0), and nonempty `placements` (`commandPalette`,
  `toolbar`, `menu`). A command opens one of the same plugin's tool windows.
- IDs use the plugin ID followed by a dot and a suffix. Duplicate or foreign
  identities, invalid references, unsupported placements and executable UI
  fields are rejected with `plugin.manifest.invalid`.
- Titles are at most 256 UTF-8 bytes and window text at most 65536 UTF-8 bytes.
  Text is displayed literally; it is never evaluated as HTML or a script.
  A window shows exactly one of `text`, [`web`](#web-content) and
  [`view`](#plugin-views).
- Declare `onView:<window-id>` and `onCommand:<command-id>` for every window and
  command, plus `ui.contribute`. The existing permission and activation error
  codes apply. IDs and presentation order are normalized deterministically.
- The host exposes metadata without instantiating the module. Opening an
  entry activates the plugin's UI module on demand; a disabled, quarantined
  or safe-mode-suppressed module has no executable UI entries. Disabling it
  removes the entries and closes its visible content.
- The host owns header controls, widgets, menus, theme, ordering and layout.
  Saved workspace preferences override a declaration's initial placement and
  order. Moving or hiding content does not create a new module or session.
  Unknown and temporarily absent contributions never execute an action;
  placement/order preferences remain available if the plugin returns.

A host may support mouse docking through the existing shared dock state;
that presentation behavior does not grant additional plugin execution capabilities.

### Web content

A plugin does not have to bring a UI: build systems get a host-drawn window
(see [Presentation](#presentation)), and text windows need no code. A plugin
that wants its own UI can show its own page in a tool window instead of
`text`:

```json
"requires": { "ui": 0 },
"permissions": ["ui.contribute", "ui.web"],
"ui": { "toolWindows": [{ "id": "…", "title": "…", "placement": "right", "web": { "entry": "ui/index.html" } }] }
```

- `web` needs `requires.ui: 0` and the `ui.web` permission, which the install
  review lists like every permission.
- `entry` is a package-relative `.html` path. `fixtures/ui/web-panel.plugin.json`
  is the example; the web cases of `fixtures/manifest/invalid-manifests.json` are the
  rejections.

The window shell stays the host's: dock, title, activity-bar entry, order,
visibility, layout, and lifecycle are declared or managed exactly as for a
text window. The page fills the body only and cannot open, place, size, or
title a window.

Hosts load a page under these rules:

- Only files of the installed package are served, read-only, from an origin
  private to the plugin. The host's content security policy refuses network
  connections, remote resources, frames, inline scripts, and `eval`.
  Navigation away from the package, popups, and downloads are blocked.
- Web storage is not persistent. A page keeps its state through the bridge.
- The package path `.lithe-host/` is reserved: the host answers
  `.lithe-host/bridge.js` with its own bridge script and serves no package
  file under it.
- The host creates the page when the window is first shown. It may destroy
  the page whenever the window is hidden, and does destroy it when the plugin
  is disabled, updated, or uninstalled. A page restores itself from its
  state.
- The root element carries `data-theme` (`light` or `dark`) and the host's
  `--lithe-*` CSS colour and font variables, updated with the theme.

Before the page's scripts run, the host defines `window.lithe`:
`lithe.version`, the bridge version (`0`), `lithe.context` (`pluginID`, `windowID`, `platform`, `locale`,
`theme`), `lithe.request(method, params)`, which returns a promise of the
result or rejects with `{ code, message }`, and `lithe.on(event, listener)`,
which returns an unsubscribe function. A page reaches only its own plugin's
build systems and the open workspace. Requests that run anything ask for the
same consent as the host's own controls, including
[workspace trust](#workspace-trust), and
their output appears in the host's Build output window.

| Method | Parameters | Result |
| --- | --- | --- |
| `build.instances` | none | `{ instances: [{ id, buildSystemID, root, state, model?, options, error? }] }`: this plugin's detected instances, `model` as `build.decodeModel` returns it, `options` the stored values |
| `build.refresh` | `{ instance }` | `{}`; reloads the model |
| `build.runTask` | `{ instance, task, project?, target?, modelTask?, userArguments? }`, the selection of `build.taskPlan` | `{ run }` |
| `build.runTests` | `{ instance, project?, target?, suite?, case? }`, the selection of `build.testPlan` | `{ run }` |
| `build.cancel` | `{ run }` | `{}` |
| `build.dependencies` | `{ instance, project? }` | the tree `build.decodeDependencies` returns |
| `build.setOptions` | `{ instance, values }` | `{ options }`, checked by `build.resolveOptions` and stored like values set in the host's option list |
| `workspace.openFile` | `{ path, line?, column? }`, a path inside the workspace | `{}` |
| `ui.getState`, `ui.setState` | none, `{ state }` | `{ state }`, `{}`; JSON of at most 64 KiB per window and workspace |
| `functions.list` | `{ instance? }` | `{ functions }`: sorted IDs of the functions the instance's provider, or without `instance` the plugin's [backend](#plugin-backend), currently offers; starts that process |
| `functions.invoke` | `{ instance?, function, params?, call? }`, without `instance` a call to the plugin's backend; `params` an object, `call` a page-chosen ID of at most 128 characters that the page never used before | `{ value }`, the function's result |
| `functions.cancel` | `{ call }` | `{}`; a call still awaiting consent never starts, a call waiting for its provider to start ends at once while the start goes on, and a call that already ended is ignored |
| `dialog.openFile` | `{ title?, extensions? }` | `{ path }`: the absolute path of the file the user chose, or `null` when the dialog was cancelled |
| `dialog.saveFile` | `{ title?, suggestedName?, extensions? }` | `{ path }` as for `dialog.openFile`; the host has asked before replacing an existing file, and nothing is written |

| Event | Payload |
| --- | --- |
| `build.instancesChanged` | `{ instances }`, as `build.instances` returns them |
| `build.runChanged` | `{ run, instance, state, exitCode? }`, `state` one of `running`, `succeeded`, `failed`, `cancelled` |
| `functions.progress` | `{ call, instance?, function, message?, percentage? }`, only for a call made with `call`; `instance` absent for a backend call |
| `ui.themeChanged` | `{ theme }` |

The file dialogs are the host's: they name the plugin that asks, a page
has at most one open at a time, and the page learns only the path the user
chose. `extensions` are up to 16 file name extensions without the dot;
`suggestedName` is a file name, not a path. A page passes the path to its
backend, which reads or writes the file; the page itself still reaches no
file.

Bridge errors are `plugin.web.unknownMethod`, `plugin.web.invalidRequest`,
and `plugin.web.forbidden`. Build requests also report the `build.*` codes.
Function requests report Core's codes unchanged (`plugin.function.*`,
`requestCancelled`, `requestTimeout`); a failure the plugin reported also
carries its own `pluginErrorCode`, which the host never interprets. A call
Core could not deliver, such as a write to a backend that exited, keeps
Core's transport code (`processFailed`) and carries the OS diagnostic only in
its message. A backend that stops reading its input is ended when a write to
it stays blocked past the call's timeout, or briefly after the call was
cancelled or the provider stopped. A page
has at most 16 function calls accepted at once, counting calls still awaiting
consent, and closing it cancels them. Reusing a `call` ID is
`plugin.web.invalidRequest`, also after that call ended.
`functions.list` and `functions.invoke` with an `instance` ask for the same
consent as the host's provider requests: once given for a plugin and
workspace, it holds until the project is closed. Calls to the plugin's
backend ask for none; the install review covered it.
### Existing community store

The community index is [Lithe Store](https://github.com/Lithe-IDEA/Lithe-store).
To add a host-rendered view to a community package:

1. Add `ui` to `requires`, `ui.contribute` to the permissions, and the new
   `onView` / `onCommand` events to the activation events.
2. Add `ui.toolWindows` and `ui.commands`; prefix every ID with the plugin ID,
   for example `io.github.lithe-idea.lua-support.tools`.
3. Keep `vendor.signatureRequirement: "none"` for community packages. The
   existing package checksum, user install review and content digest still
   apply; UI does not bypass them.
4. List the platforms whose hosts declare every capability the package
   requires; the index schema, release archive format and SHA-256 rules do not
   change.

`fixtures/manifest/hello-lithe.plugin.json` is a contract fixture in the reserved test
namespace, not a ready-to-publish community package. Change its ID and all
references to your own namespace before using it in Lithe Store.

### UI icons

Tool windows and commands accept an optional `icon`. Prefer a stable
host icon ID; it resolves through Lithe's existing icon library, not SF Symbols:

```json
"icon": { "host": "database" }
```

Supported host IDs: `tool-window`, `plugin`, `database`, `services`, `terminal`,
`search`, `refresh`, `run`, `debug`, `settings`, `add`, `remove`, `git`, `problems`.
These IDs are public; bundled SVG paths remain implementation details.

Custom SVG and PNG icons are relative to the plugin package root:

```json
"icon": {
  "path": "icons/database.svg",
  "darkPath": "icons/database-dark.svg",
  "mode": "template"
}
```

`mode` defaults to `template`: the host tints the image using the containing
control's theme and selected/hover state. `original` preserves brand colors;
control backgrounds and disabled opacity still follow the host. `darkPath` is
optional; without it the same resource is used in both themes. Brand authors
must make that resource legible in both themes or provide a dark variant.
`host` cannot be combined with resource paths or original-color mode.

The host owns icon size: 20pt tool-window rail and 16pt actions/menus/palette.
Panel titles are text-only: no title icon, outline or bottom divider. A declared
window icon remains available in the activity rail and management menus.
Aspect ratio is preserved inside each icon slot. Plugin-specified
size, color, stroke, arbitrary style, remote URLs and SF Symbol names are rejected.
Stroke geometry belongs to the asset; authors should follow the host icon
weight, rather than expecting the host to rewrite paths or PNG pixels.

Window and toolbar management submenus reuse the same declared icons at the
host menu size. Template icons in the command palette use the host selected-text token on the
selected row; brand icons retain their theme variant when keyboard selection moves.

Files must be regular files inside the verified package, without symbolic links
in their path. Each resource is limited to 256 KiB and 1024×1024; unique icon
bytes per package are limited to 8 MiB, with at most 4,194,304 total image pixels. PNG is decoded as a single image. SVG
accepts static shapes, groups, gradients, clip paths and the standard SVG version attribute; scripts, event handlers,
external references, embedded images, style elements/attributes, entities and
DOCTYPEs are refused. Use explicit presentation attributes for SVG colors.
Unknown fields and unsupported formats are rejected at installation and launch.

Window entries without an icon use the shared generic tool-window icon. Commands
without an icon remain text actions (palette retains its existing action marker).
Missing/unreadable declared resources reject the package instead of silently
using a different image. Assets are read-only and held in process memory; no
runtime disk cache or app-bundle write is created. Installed icon bytes follow
the existing package signature/digest, update and rollback lifecycle.

Lithe 桌面端布局菜单允许把已注册工具窗口移到左侧、右侧或底部，并保存入口
顺序与显隐；`ui.toolWindows[].placement` 只声明首次使用的 `bottom`/`right`
位置。项目、提交和搜索采用相同的工具窗口注册路径，
原生业务内容保持宿主所有；左侧恢复插件窗口仍按 `onView` 激活所属模块。

Lithe 桌面端动作的可见、可用、勾选和激活中状态由宿主计算，不接受清单中的可执行
条件脚本。窗口尚在激活时，其打开命令在工具栏、菜单和命令面板禁用；窗口入口
仍可取消显示。多个窗口同时请求同一插件模块时只构造一次；禁用或关闭会取消
并清理在途激活，旧的菜单或命令引用不能重新调用已禁用的贡献。

### Plugin views

A plugin view is content a plugin draws through one backend function: the
body of a tool window, or a [settings region](#settings-regions) on a settings
page. The plugin returns a view, an array of nodes, and the host draws it.
Host-control nodes (rows, sections, labels, inputs, buttons, tables, trees)
carry meaning only; every host draws them with its own native controls and
theme. A `canvas` node carries a bounded vector scene that the host keeps and
draws natively each frame, so a plugin can design a distinctive page, such as
a dependency graph or a build board, without a web view and without a
round trip per frame. Fixtures: [`view-panel.plugin.json`](fixtures/ui/view-panel.plugin.json)
declares both surfaces, and [`view-results.json`](fixtures/ui/view-results.json)
holds golden `plugins.ui.applyResult` cases.

```json
"settingRegions": [
  { "id": "overview", "keywords": ["module graph", "dependencies"],
    "view": { "function": "settings.overview" } }
],
"ui": { "toolWindows": [
  { "id": "dev.lithe.fixture.view-panel.explorer", "title": "Module Explorer",
    "placement": "right", "view": { "function": "explorer.draw" } }
] }
```

`view: {function}` names a private [plugin function](#plugin-functions) of the
plugin's own [backend](#plugin-backend) (letters and digits in dot-separated
segments, at most 128 characters). A manifest with a view needs `backend`,
`requires.ui: 0`, the `ui.contribute` permission, and, for a settings region,
`requires.settings: 0`. A tool window still declares `onView:<id>`; a settings
region starts the backend when its page opens. Results are described by
[`plugin-ui.schema.json`](plugin-ui.schema.json); the rules JSON Schema cannot
express are listed below and enforced by Core.

#### Events

The host calls the view function through `lithe/function/invoke` once per
event, with these params. A view shown for a workspace, such as a tool window
or a settings page opened from a project window, is called with that window's
[workspace context](#workspace-contexts-and-capability-discovery) as
`params.context` of the invoke, so the backend can use it for host methods that
take one, such as [`run/start`](#run-and-debug-sessions) in answer to a
`click`; a view shown without a workspace is called without one.

| Field | Meaning |
| --- | --- |
| `event` | `open`, `close`, `click`, `change`, `select`, `activate`, `viewport`, `resize`, or `settings` (below) |
| `instance` | Host-chosen string naming this showing of the view. A new `open` gets a new instance; the same tool window open in two workspaces has two. |
| `surface` | `{toolWindowID}` or `{settingRegionID}`: the declaration being drawn |
| `language` | Interface language tag, such as `en` or `zh-Hans`. View text is plugin content; the backend returns it in this language. |
| `settings` | `{effective, draft}`: the plugin's setting values by ID. `effective` is what is saved and in force; `draft` holds the values changed in the open settings window but not yet applied, and is `{}` in a tool window. |
| `node` | `click`, `change`, `select`, `activate`: the ID of the node the user acted on |
| `canvas` | `click` inside a canvas and `viewport`: the canvas ID; `node` is then the canvas group's ID |
| `value` | `change`: the control's new value: a string, or a boolean for `toggle`. For a control bound to a setting it is the new draft value, typed as the setting stores it, so a number for an `integer` setting. |
| `setting` | `change` of a control bound to a setting: the setting ID |
| `item` | `select`, `activate`: the tree item ID; `node` is the tree's ID |
| `viewport` | `viewport`: `{x, y, width, height}`, the visible part of the canvas in view-box coordinates |
| `width` | Every event, from a host that reports it: the width in points the host lays the view's nodes out in, which is also the width of a canvas at the top level, a whole number of at least 1. A host that reports it sends `open` only once it knows the width. |

| Event | Sent when | Result |
| --- | --- | --- |
| `open` | The view becomes visible | A whole view |
| `close` | The view is hidden, its page or window closes, or the plugin is disabled, updated, or removed | Ignored |
| `click` | A `button` without `action` is pressed, or a canvas group with `on: ["click"]` is clicked, or focused and activated with Enter or Space | Any |
| `change` | A `textField` commits on Enter, focus loss, or submit; a `select`, `segmented`, or `toggle` changes | Any |
| `select`, `activate` | A tree item is selected; double-clicked or activated with Return | Any |
| `viewport` | 400 ms after a pan or zoom stops, only for a canvas whose `on` lists `viewport` | Any |
| `resize` | 400 ms after the view's `width` stops changing, only while the drawn view has a canvas whose `on` lists `resize` | Any |
| `settings` | `effective` or `draft` changes: the user edits a declared row or a bound control, applies, cancels, or a value changes elsewhere | Any |

Hovering, pressing, keyboard focus and focus rings, tooltips, tree expansion,
scrolling, panning, zooming, and typing never reach the plugin. A text field
does not call the backend per keystroke. The host sends one event at a time
per instance, in order, and may merge waiting `viewport`, `resize`, or
`settings` events into the latest one. A view changes only in answer to an event; this version
has no way for a plugin to push a view on its own.

#### Results and patches

The function returns `{view}` (a whole view), `{patches}` (edits to the drawn
view), or `{}` (nothing changed). The first result of an instance returns a
view. Before drawing anything, the host passes the result to Core's
`plugins.ui.applyResult` with the manifest, the surface, and the view it drew
last ([Rust Core API](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/contracts/rust-core-api.md#plugin-views)).
Core checks the result, applies the patches to a copy, and returns the
complete next view, or refuses the whole result with `plugin.ui.invalidView`
and the JSON path of the first offending value. Hosts only render; they keep
no validator or patch engine of their own.

At most 512 patches apply in order. Each is addressed by node `id` within the
host controls, within one canvas scene (`canvas`), or within one tree's items
(`tree`), never both:

| `op` | Fields | Effect |
| --- | --- | --- |
| `set` | `id`, `fields` | Merges `fields` into the node; a `null` field removes it. `type` and `id` cannot change. |
| `replace` | `id`, `node` | Swaps the node, with its subtree, for `node` |
| `remove` | `id` | Deletes the node and its subtree |
| `insert` | `node`, optional `parent` and `index` | Adds `node` to the children of `parent`, or to the top level, at `index` or at the end |

A patch whose target is missing refuses the whole result, and the patched
view must pass every rule a whole view does.

#### Host controls

| `type` | Fields | Notes |
| --- | --- | --- |
| `row` | `children` | Children side by side |
| `section` | `title`, `info`, `children` | A titled group |
| `label` | `text`, `tone`, `style` (`title`, `strong`, `body`, `caption`, `mono`), `width` | `width` reserves that many characters (1–120) |
| `textField` | `id`, `label`, `info`, `value` or `setting`, `placeholder`, `width` (`fill` or 1–120 characters), `mono`, `enabled` | Binds a `text`, `file`, `directory`, or `integer` setting |
| `select` | `id`, `label`, `info`, `value` and 1–128 `options` `{value, label}`, or `setting`, `enabled` | Binds an `enum` setting, which supplies the options; `value` is one of the options |
| `segmented` | As `select`, with 2–8 options | Binds an `enum` setting with 2–8 options |
| `toggle` | `id`, `label`, `info`, `value` or `setting`, `enabled` | Binds a `boolean` setting |
| `button` | `id`, `text`, `tone` (`primary`), `enabled`, `icon`, `action` | Sends `click`; with `action`, runs that [setting action](#placement-scope-effect-and-page-buttons) through the host's confirmation flow instead |
| `keyValue` | 1–64 `items` `{label, value, tone}` | Label and value pairs |
| `table` | 1–16 `columns` `{title, align}`, up to 256 `rows` | Read-only; each row has one single-line cell per column |
| `banner` | `text`, `tone` | A message across the view |
| `setting` | `setting` | The host's own row for a declared setting, exactly as the page draws it |
| `tool` | `tool`, `choose` | The host's row for a tool the plugin defines (`toolDefinitions`) or requires (`requiresTools`), as the toolchain pages draw it; with `choose`, also its [picker](#tool-rows-and-pickers) |
| `tree` | `id`, `label`, `items` `{id, label, icon, detail, expanded, children}` | An outline; sends `select` and `activate` |
| `canvas` | Below | A vector scene |

`tone` is `secondary`, `muted`, `accent`, `success`, `warning`, or `error`;
`icon` names a [host icon](#ui-icons). Host controls take no colors, fonts,
sizes, or coordinates, so the host keeps light and dark themes, scaling,
accessibility, and input methods consistent. Labels, titles, options, and
placeholders are single-line text of at most 120 characters, tree labels and
details at most 240, body text and values at most 1024, and `info` at most
480. `info` draws the host's ⓘ beside a label or heading.

Some nodes belong to one surface. `setting`, `tool`, controls with `setting`,
and buttons with `action` appear only in settings regions; `tree` appears
only in tool windows. A control shows either its own `value` or a declared
`setting`, never both. A bound control edits the settings window's draft,
which the host's Apply commits and Cancel discards, exactly as for the
declared rows; a region never stores values of its own. Writing a tool's own
configuration, such as a build file, stays a setting action of the plugin and
follows [source-backed settings](#source-backed-settings-editors).

#### Tool rows and pickers

A `tool` row shows what the host resolves the tool to in the settings page's
scope (the global settings, or the project of a project window): the version,
the home, and the reason when nothing resolves. Pressing it opens the tool on
the host's toolchain page.

With `choose: true`, the row is also the tool's picker, like an interpreter menu: pressing it opens the host's choices for the
tool in that scope, the same choices the toolchain page lists. They are the
automatic choice, every installation the host found or the user added, with
its version and source, and adding an installation directory, plus a way to
the toolchain page. Choosing one edits the settings window's draft for the
tool, which Apply commits and Cancel discards, as on the toolchain page; the
row shows the pending choice until then. A region never stores the choice
itself; once applied, the plugin's launches and runs receive the chosen
installation as they receive any resolved tool (see
[Tool references](#tool-references)). `choose` is a boolean; any other value
refuses the whole result with `plugin.ui.invalidView` at `choose`. A host whose
Core predates `choose` refuses a result that writes it the same way, so a plugin
writes it once a desktop release that includes it is out.

#### Canvas

```json
{ "type": "canvas", "id": "graph", "label": "Module graph", "height": 360,
  "viewBox": [0, 0, 1200, 640], "pan": true, "zoom": [0.4, 3], "on": ["viewport"],
  "nodes": [
    { "type": "path", "id": "edge", "d": "M190 20C234 20 216 80 260 80", "stroke": "$line", "width": 1.5 },
    { "type": "group", "id": "guava", "x": 260, "y": 60, "w": 190, "h": 40,
      "on": ["click"], "label": "guava 31.1, not used", "tooltip": "Replaced by 33.0",
      "states": { "hover": { "fill": "$hover" }, "pressed": { "scale": 0.97 } },
      "children": [
        { "type": "rect", "x": 0, "y": 0, "w": 190, "h": 40, "radius": 8,
          "fill": { "light": "#fdf0ef", "dark": "#3a2726" }, "stroke": "$error", "dash": [4, 3] },
        { "type": "text", "x": 12, "y": 26, "style": "strong", "maxWidth": 168,
          "runs": [{ "text": "guava" }, { "text": "  31.1", "color": "$error" }] }
      ] }
  ] }
```

A canvas draws its `nodes` in order, in `viewBox` coordinates
(`[x, y, width, height]`), in an area as wide as the view and `height` points
tall (1–2000). `label` is the scene's accessible name. `pan` lets the user
drag the empty area, and `zoom` is the `[minimum, maximum]` range around 1
(0.05–20); the host draws the zoom buttons. The scroll wheel scrolls the page;
⌘ or Ctrl with the wheel zooms, and so does a pinch on a host that reports one.
`on` lists the events the canvas sends: `viewport` and `resize` (see
[Events](#events)).

At zoom 1 the host scales the `viewBox` in one of two ways:

- **Without `pan`**, the whole `viewBox` shows: it is scaled to the width of
  the view, but no taller than `height`, and starts at the area's top-left
  corner. A scene whose `viewBox` is as wide as the view and `height` tall
  draws at one point a unit. In a wider view it keeps that size, whole, with
  the area's trailing part left empty; in a narrower one it shrinks to the
  width and leaves the area's bottom empty.
- **With `pan`**, the `viewBox`'s width fills the view and its top edge is at
  the area's top; the user pans to the parts below the area. Zoomed out until
  the `viewBox` is narrower than the area, it is centered across it.

A scene that follows the window lays itself out for the event's `width`: it
gives its `viewBox` that width, picks `height` for what it draws there, and
lists `resize` in `on`, so the host sends `resize` with the new `width` once
the window stops being resized and the plugin returns the scene laid out
again. Until the answer arrives the host scales the scene it has, as above. A
host that does not report `width` sends no `resize` and may refuse a canvas
that lists it, so a plugin lists `resize` only in answer to an event that
carried `width`, and otherwise lays the scene out for a width of its own
choosing.

| `type` | Fields |
| --- | --- |
| `rect` | `x`, `y`, `w`, `h`, `radius`, `fill`, `stroke`, `width`, `dash`, `shadow`, `opacity` |
| `path` | `d` (SVG path data, at most 8192 bytes), `bounds` `[x, y, width, height]`, `fill`, `stroke`, `width`, `dash`, `cap`, `rule` (`nonzero`, `evenodd`), `opacity` |
| `text` | `x`, `y`, `text` or up to 64 `runs` `{text, color}`, `style` (`title`, `strong`, `body`, `caption`, `display`, `mono`, `monoSmall`), `align`, `color`, `maxWidth`, `opacity` |
| `image` | `x`, `y`, `w`, `h`, `src`: a PNG path inside the plugin package, `opacity` |
| `group` | `x`, `y`, `scale`, `w`, `h`, `radius`, `clip`, `children`, `on` (`click`), `label`, `tooltip`, `states` (`hover`, `pressed`, `selected`), `selected`, `opacity`, `shadow`, `hidden` |

- Text is one line with its baseline at `y`. `align` puts the line's start
  (the default), middle, or end at `x`. Text uses the host's interface and
  monospace fonts at the named style; a plugin cannot choose a font. Text
  longer than `maxWidth` ends with an ellipsis.
- A color is `#RRGGBB`, `#RRGGBBAA`, a theme token (`$text`, `$textSecondary`,
  `$textMuted`, `$accent`, `$success`, `$warning`, `$error`, `$surface`,
  `$background`, `$sidebar`, `$line`, `$hover`, `$selection`, `$badge`), a
  `{light, dark}` pair, or `{linear: [[offset, color], …], angle}` with 2–8
  stops. Tokens and pairs follow the theme; a fixed color is for content that
  shows a color itself, such as a color-scheme preview.
- A gradient's `angle` is in degrees clockwise from pointing up, as in CSS
  `linear-gradient`: 0 runs upward, 90 to the right, and 180, the default,
  downward. Its line passes through the middle of the shape (a path's
  `bounds`, or the box of its points) and is as long as it takes for the
  corners the angle points at to take the first and the last stop.
- A group's `x` and `y` place its origin, and `scale` scales what is inside
  it: its `children`, `w`, `h`, and `radius` are in the group's own
  coordinates.
- A group with `on: ["click"]` is a control and needs `id`, `w` and `h` (its
  hit area), and `label`, read by screen readers. Tab moves between clickable
  groups. The host draws `states` without a round trip: `fill`, `stroke`,
  `width`, and `shadow` paint the group's `w` × `h` area with its `radius`
  behind the children, and `scale` and `opacity` apply to the whole group.
- `shadow` is an elevation step 1–3 mapped to the host's shadows. Coordinates
  and sizes are within ±1,000,000. Blur, filters, blend modes, radial
  gradients, custom fonts, animation, and text input are not part of a canvas;
  input uses host controls beside it.
- An `image` is read from the installed package the way its icons are: the
  path stays inside the package, through real directories, to a regular PNG
  file within the limits below. The host loads it off the window thread the
  first time a scene draws it and keeps it until the view closes. Until then,
  and for a file that is missing, too large, or cannot be decoded, the scene
  draws without it; the host logs a failure once.
- Canvas text is not searched. A settings region names its words in
  [`keywords`](#settings-regions).

#### Budgets and identifiers

| Limit | Value |
| --- | --- |
| One view, as JSON | 3 MiB |
| Host-control nodes | 512 |
| Nesting of nodes, tree items, or canvas groups | 16 levels |
| Items of one tree | 2000 |
| Canvases of one view | 8 |
| Nodes of one canvas | 2000 |
| One canvas scene, as JSON | 256 KiB |
| One canvas image file | 256 KiB, at most 1024 × 1024 pixels |
| Patches of one result | 512 |

Node, tree item, and canvas node IDs are 1–64 letters, digits, `.`, `_`, or
`-`, starting with a letter or digit, and unique among the host controls, the
items of one tree, and the nodes of one canvas. Fields are omitted rather
than sent as `null`.

#### Lifecycle and failure

The host sends `open` when the view becomes visible and `close` when it goes
away; it cancels a pending call on close, on disable, update, or removal of
the plugin, and when the workspace closes, and drops late replies. Each call
has a 2 second timeout. A failed, timed-out, or refused call leaves the last
view drawn and shows the error in the view's own frame, with a Retry action;
the rest of the window stays usable. A settings region without a drawn view
falls back to the plugin's declared setting rows on that page, which can still
be edited. The host owns the frame, clipping, scrolling, and every popup; a
view cannot open dialogs, send notifications, or change anything outside its
frame. View state lives in the backend and the host's memory, never in the
installed package or application bundle.
