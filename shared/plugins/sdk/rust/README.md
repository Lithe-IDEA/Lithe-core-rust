# Lithe plugin backend SDK for Rust

`lithe-plugin-sdk` is the official way to write a
[plugin backend](../../contract/v1/README.md#plugin-backend) in Rust: the one
long-lived process a plugin declares under `backend` in `plugin.json`, which
Lithe starts on demand (a page's first call, a command, workspace open; see
[Backend lifecycle and workspace contexts](../../contract/v1/README.md#backend-lifecycle-and-workspace-contexts)).
The backend is a packaged executable, so a plugin ships one archive per
platform. It speaks the LSP base protocol on stdio and implements, from the
[plugin contract](../../contract/v1/README.md), which lives in this repository
next to the SDK and changes with it:

- **`litheFunctions`**: plugin functions with progress, cancellation, and
  registration at runtime, and the [view functions](#plugin-views) that draw
  a plugin's tool windows and settings regions.
- **Host services**: the plugin's own credentials, kept by the host in the
  platform's secret store, workspace contexts, files,
  documents, editors, preview, the browser, window messages, commands, and
  subscriptions, and [calls to the functions other plugins export](#calling-other-plugins),
  relayed by Core, with the `Caller` a function called that way sees.
- **`litheKeymapImport`**: independently negotiated shortcut import providers
  reading only explicitly selected source snapshots.
- **`litheBuild`**: [typed build provider requests](#build-providers) for a
  build system whose model, dependencies, tasks, tests, or run and debug
  preparation come from the plugin's own process.
- **`litheToolchains`**: [typed toolchain discovery providers](#toolchain-providers).
- **`litheLogging`**: typed diagnostics, required at initialization; see
  [Logging](#logging).

The SDK owns the wire format, negotiation, cancellation, and error mapping.
Your code supplies only handlers. It depends on `serde`, `serde_json` and `base64`, runs
every call on a thread of its own, and needs Rust 1.87 or newer.

This SDK has no VCS provider API; write VCS providers with the
[Node.js](../node/README.md) SDK.

## What the Lithe desktop serves

The Lithe desktop declares `backend` and `pluginFunctions` (both version 0) and
`buildProviders` (versions 0 and 1), so the function, host-service and build
provider protocols below run there. It does not declare
`keymapImportProviders`: a plugin whose manifest requires it is not offered for
installation there, although the Core end-to-end tests still drive the protocol.
Which host services a session may call is reported at run time; see
[Host services](#host-services).

## Build providers

A build system whose `model` or `dependencies` source is `provider` names a
[build provider](../../contract/v1/README.md#build-providers). Written with
this SDK, it is a packaged executable with a process of its own: the
language-server process fields plus `executesProjectCode` and `mayUseNetwork`,
under the `languageServer.launch` permission (and `process.build`, which every
build system needs). The host starts one process per
plugin, build system and workspace, and offers `litheBuild` at initialization.
The SDK answers the newest offered version it speaks (`0` or `1`) when at least
one build handler is declared, and does not answer `litheBuild` otherwise:

```json
"requires": { "buildSystems": 0, "buildProviders": 0 },
"permissions": ["languageServer.launch", "process.build"],
"buildSystems": [{
  "id": "example", "displayName": "Example",
  "detection": { "markers": ["example.toml"] },
  "invocation": { "executables": [{ "kind": "command", "command": "example" }] },
  "model": { "source": "provider" },
  "provider": {
    "launch": { "kind": "packaged", "path": "bin/example-build" },
    "executesProjectCode": true, "mayUseNetwork": false
  }
}]
```

```rust
use lithe_plugin_sdk::{Backend, PluginError};
use serde_json::json;

fn main() {
    let mut backend = Backend::new("example-build").version(env!("CARGO_PKG_VERSION"));
    backend
        .build_model(|params, context| {
            context.check_cancelled()?;
            let root = params["root"].as_str().unwrap_or(".");
            Ok(json!({ "buildSystemID": "example", "root": root, "projects": [] }))
        })
        .build_prepare_debug(|params, context| {
            context.progress("Building", None);
            // Build the target and stream the tool's output with context.output(…);
            // stop the tool once context.cancellation() fires.
            let target = params["target"]["name"].as_str().ok_or_else(|| {
                PluginError::new("example.noTarget", "Select a target to debug.")
            })?;
            Ok(json!({ "program": format!("out/{target}"), "arguments": [],
                "workingDirectory": ".", "environment": {} }))
        });
    std::process::exit(backend.listen());
}
```

| API | Request |
| --- | --- |
| `build_model(handler)`, `build_dependencies(handler)` | `lithe/build/model`, `lithe/build/dependencies` |
| `build_execute(handler)`, `build_test(handler)` | `lithe/build/execute`, `lithe/build/test`: `{ exitCode, output }` |
| `build_discover_tests(handler)` | `lithe/build/discoverTests` |
| `build_run_entrypoints(handler)` | `lithe/build/runEntrypoints`, routed only when the host negotiated `litheBuild` `1` |
| `build_prepare_run(handler)`, `build_prepare_debug(handler)` | `lithe/build/prepareRun` (a launch plan), `lithe/build/prepareDebug` (exactly `program`, `arguments`, `workingDirectory`, `environment`) |
| `handle.build_version()`, `Initialize::build_version()` | The negotiated `litheBuild` version, or `None` |

A handler is `Fn(Value, &BuildContext) -> Result<Value, FunctionError>` and
runs on a thread of its own. The SDK stamps `format` and `version` on the
model, dependency, test discovery and run entrypoint documents, so a handler
may return the plain document; a document that names another format or
version, or a result of another shape, fails as an internal error before Core
sees it. Core still validates the document's content and workspace
containment. A result must serialize to at most 32 MiB of JSON.
`execute`, `test`, `prepareRun` and `prepareDebug` need the wire `operationID`
Core gives them; a request without it is refused before its handler runs. Only
they report progress: `progress(message, percentage)` cuts the message to
4 KiB and the percentage to 100, and `output(text)` streams the tool's output
in reports of at most 64 KiB, split at character boundaries. The result's
`output` still carries the complete text. Reports from the other requests,
after the request ended, or after cancellation are dropped. A build tool
failure is a result with its exit code. An `Err` fails the request, and for run
and debug preparation starts nothing: a `PluginError` answers a JSON-RPC error
with its message and code, which Core reports in the build's own error category
without forwarding a `pluginErrorCode`; any other error or a panic answers a
generic internal error, and a cancelled request answers `requestCancelled`.
Terminal diagnostics are automatic, as for functions. The SDK has no process runner: stop the build
tool's whole process tree when `cancellation()` fires. `host()` works only when
the host also negotiated plugin functions with the process.

## Toolchain providers

Declare the same provider ID in the manifest's
[`toolchainProviders`](../../contract/v1/toolchains/README.md#provider-backend-negotiation)
and via `Backend::toolchain_provider`. The host offers the IDs it recognizes at
initialization. The SDK selects registered IDs in the offer and routes each
query to that handler. Existing code defaults to version 0. To use
`requires.toolchainProviders: 1`, explicitly select the same version before
listening; upgrading the SDK alone never changes a provider's protocol:

```rust
use lithe_plugin_sdk::{
    toolchains::{ToolchainProviderVersion, ToolchainQueryResult}, Backend,
};

let mut backend = Backend::new("version-manager")
    .toolchain_provider_version(ToolchainProviderVersion::V1);
backend.toolchain_provider("manager", |_query, cancellation| {
    cancellation.check()?;
    // Read this manager's own data for query.tools and query.workspace.
    Ok(ToolchainQueryResult::default())
}).expect("valid provider ID");
```

The SDK negotiates only the configured version: version 1 does not silently
fall back to version 0 on an older host. Version 1 encodes `PermissionDenied`
as `-32080` and `AuthorityUnsupported` as `-32081`; version 0 retains `-32070`
and `-32071`. All stable string codes and other error numbers are unchanged.

Handlers return installations, version statements and watch paths. An empty
result is valid. Set a statement's `installation` to `None` for an unmet
requirement. Use `ToolchainQueryError` for a query failure and check the
cancellation token during potentially long scans. The host observes native
filesystem facts, and Core validates the returned candidates with those facts.
The desktop does not yet declare this capability. The Rust SDK end-to-end test
runs the protocol through Core.

## Shortcut import providers

Declare `keymap_import_provider(id, handler)` before listening. The host
negotiates IDs independently of plugin functions; a handler receives a
`KeymapImportRequest` and a call-scoped `Context`. Use
`request.list_inputs(context.host())` and `request.read_chunk(context.host(),
file_id, offset, maximum_bytes)` to read declared files in bounded chunks.
`KeymapSourceFile.path` is a relative label; the file ID is the authority.
Return `{ "bindings": [...] }` in the normalized contract format. The host
cancels the call on import cancellation and revokes the snapshot when it ends.
Do not convert conditional or multi-stroke rules into global bindings.

## A minimal backend

```json
"pluginAPIVersion": 0,
"requires": { "backend": 0, "ui": 0 },
"permissions": ["backend.launch", "credentials.store", "ui.contribute", "ui.web"],
"backend": { "launch": { "kind": "packaged", "path": "bin/backend" }, "arguments": ["--state", "${stateDir}"] }
```

```rust
use lithe_plugin_sdk::{Backend, PluginError};
use serde_json::json;

fn main() {
    let mut backend = Backend::new("example-backend").version(env!("CARGO_PKG_VERSION"));
    backend
        .function("connection.test", |params, context| {
            context.progress("Connecting", Some(10));
            let key = params["connection"].as_str().ok_or_else(|| {
                PluginError::new("example.invalidParams", "\"connection\" must be a string.")
            })?;
            let password = context.credentials().get(&format!("connection:{key}"))?;
            context.check_cancelled()?;
            Ok(json!({ "hasPassword": password.is_some() }))
        })
        .expect("a valid function ID");
    std::process::exit(backend.listen());
}
```

The page calls it through the host bridge, without an `instance`:

```js
const { value } = await lithe.request('functions.invoke', { function: 'connection.test', params: { connection: 'main' } });
```

[`examples/sample-backend`](examples/sample-backend) is a complete plugin:
the manifest, a web page, notes kept in `${stateDir}` with progress, a
function registered at runtime, a secret in the credential store,
cancellation, a watch on the page's workspace, a command answered with a
message in its window, and a handler for workspace changes.
From the root of this repository,
`node shared/plugins/scripts/sdk-e2e.mjs` drives it through a real Core session
with the scenario in
[`sdk/e2e/plugin_sdk/backend.rs`](../e2e/plugin_sdk/backend.rs).

## Plugin views

A tool window or settings region whose manifest entry has
`view: { function }` is drawn by that backend function
([Plugin views](../../contract/v1/README.md#plugin-views)). Declare it with
`backend.view(id, handler)`. The handler gets a `view::ViewEvent`: `kind`,
`instance`, `surface`, `language`, the `effective` and `draft` setting
values, and the fields of that event. Params that are not a view event fail
the call with `sdk.view.invalidEvent` before the handler runs; an optional
field that is `null` counts as absent. The handler answers a
`view::ViewResult`:

- `View(nodes)`: a whole view. The first answer after `Open` must be one.
- `Patches(patches)`: edits to the view the host is drawing, built with
  `view::Patch`.
- `Unchanged`: nothing changed. The SDK answers `{}`.

Nodes stay `serde_json` values shaped as
[`plugin-ui.schema.json`](../../contract/v1/plugin-ui.schema.json) describes.
Core checks every answer, and the host draws nothing that Core refused. Return
view text in `event.language`. A host that reports the view's width sends
it as `event.width` on every event; a canvas that follows the window lays
itself out at that width, lists `resize` in its `on`, and answers
`ViewEventKind::Resize` with the scene laid out again
([Canvas](../../contract/v1/README.md#canvas)).

```rust
use lithe_plugin_sdk::view::{Patch, ViewEventKind, ViewResult};

backend.view("explorer.draw", |event, _context| {
    Ok(match (event.kind, event.node.as_deref()) {
        (ViewEventKind::Open, _) => ViewResult::View(vec![json!({ "type": "row", "children": [
            { "type": "label", "text": "Modules" },
            { "type": "button", "id": "refresh", "text": "Refresh" },
        ] })]),
        (ViewEventKind::Click, Some("refresh")) => {
            ViewResult::Patches(vec![Patch::set("refresh", json!({ "enabled": false }))])
        }
        _ => ViewResult::Unchanged,
    })
})?;
```

## Getting the SDK

The SDK is released from this repository, tagged `sdk-rust-v<version>`; the
current release is
[0.4.0](https://github.com/Lithe-IDEA/Lithe-core-rust/releases/tag/sdk-rust-v0.4.0).
The SDK is not published to crates.io. Either way below fetches it at build
time, never at runtime, and pins its exact contents:

- **Git dependency.** Name the tag; `Cargo.lock` records the commit, and
  `cargo build --locked` refuses anything else:

  ```toml
  [dependencies]
  lithe-plugin-sdk = { git = "https://github.com/Lithe-IDEA/Lithe-core-rust", tag = "sdk-rust-v0.4.0" }
  ```

- **Vendored archive.** A release ships `lithe-plugin-sdk-rust-<version>.crate`
  (a gzipped tarball, as `cargo package` makes it) and a `.sha256` file.
  Check the archive against the SHA-256 you pinned, unpack it into your
  repository, and use it as a `path` dependency.

## Logging

The SDK requires `experimental.litheLogging` at initialization and refuses a
host that does not offer it.
See [plugin logging](../../guides/plugin-logging.md) for the typed diagnostics
API, host-owned catalog, exact levels, operation correlation and `${logDir}`.
Use `diagnostics().degraded/recovered` for handled fallback or recovery facts.
Function terminal facts are automatic; do not log a failure before returning
it to the SDK. Suppress successful high-frequency reads with
`suppress_success_diagnostics`. There is no free-text `log` API, no stderr
fallback and no plugin-owned file writer.

## API

| API | Purpose |
| --- | --- |
| `Backend::new(name).version(version)` | Creates the backend; `name` and `version` are reported as `serverInfo`, and `name` prefixes log lines. |
| `backend.function(id, handler)` | Declares a function offered in `initialize`. |
| `backend.view(id, handler)` | Declares a view function; see [Plugin views](#plugin-views). |
| `backend.on_initialize(hook)` | Runs before the `initialize` answer with an `Initialize`: the host's `params()`, `functions_version()` (`Some(0)` when the host offered `litheFunctions`, otherwise `None`), `root_path()` (the plugin's `${stateDir}`), and `function(id, handler)` to add functions that depend on state found at startup. An error refuses `initialize`. |
| `backend.on_shutdown(hook)` | Runs on `shutdown`, after running calls were cancelled. |
| `backend.handle()` | A clonable `BackendHandle` for other threads. |
| `backend.listen()` | Serves stdio until `exit` or the end of input and returns the exit code: `0` after `shutdown` and `exit`, `1` otherwise. |
| `backend.serve(input, output)` | The same on other streams, for tests. |
| `handle.register_function(id, handler)`, `handle.unregister_function(id)` | Adds or withdraws a function after `initialize` and waits until the host accepted it. Withdrawing revokes and cancels running calls; a handler must not withdraw its own function. |
| `handle.function_ids()`, `handle.functions_version()` | What the backend serves, and `Some(0)` when the host offered `litheFunctions` (otherwise `None`). |
| `backend.on_command(id, handler)` | Runs `Fn(&str, Option<WorkspaceContext>)` when the user runs one of the plugin's commands. |
| `backend.on_contexts_changed(handler)`, `backend.on_host_notification(method, handler)` | Handle `lithe/host/workspace/didChangeContexts` and any other `lithe/host/*` notification a later host adds. |
| `handle.host()`, `context.host()` | The [host services](#host-services). `handle.host()` is bound to no call. |
| `handle.credentials()`, `context.credentials()` | Shorthand for `host().credentials()`. |
| `backend.build_model(handler)` and the other `build_*` methods | Declare the typed build requests; see [Build providers](#build-providers). |
| `PluginError::new(code, message)` | A failure meant for your own UI. |

A handler is `Fn(Value, &Context) -> Result<Value, FunctionError>`. Its
context offers:

| Member | Meaning |
| --- | --- |
| `cancellation()` | A `Cancellation`, fired when Lithe cancels the call, its deadline passes, the function is withdrawn, or the session ends: `is_cancelled()`, `check()`, `wait_timeout(duration)` (use it instead of `thread::sleep`), and `on_cancel(callback)`, for example to cancel a running database query from the driver. |
| `check_cancelled()` | `Err(Cancelled)` once cancelled, for `?`. |
| `progress(message, percentage)` | The message is cut to 1024 bytes and the percentage to 100; reports after the call ended are dropped. |
| `host()` | Host services bound to the call: a request still waiting for the host when the call is cancelled ends at once. `credentials()` is `host().credentials()`. |
| `workspace_context()` | The `WorkspaceContext` of the window whose page made the call; `None` when no window made it. |
| `caller()` | The `Caller` (`plugin_id()`) of the plugin whose backend called this function through Core's relay; `None` when the host started the call and when the host's `caller` is not exactly `{ pluginID }` with a non-empty string. |
| `backend()`, `function_id()`, `diagnostics()` | The handle, the function's ID, and typed diagnostics correlated with this call. |

## Host services

A backend may ask the host for services; each is granted by a manifest
permission and refused otherwise. They are available when the host offers
`litheFunctions` in `initialize`; without it, host requests fail locally with
`plugin.host.unavailable` and send nothing. Every method blocks its thread
until the host answers and returns `Result<_, SdkError>`; wrong arguments fail
locally and send nothing.

```rust
backend
    .function("query.run", |params, context| {
        // Permission credentials.store: the platform's secret store, per plugin.
        let password = context
            .host()
            .credentials()
            .get("connection.prod:password")?
            .ok_or_else(|| PluginError::new("query.noPassword", "Sign in first."))?;
        // The window the page runs in.
        let mut folders = None;
        if let Some(workspace) = context.workspace_context() {
            if context.host().supports("lithe/host/workspace/getFolders")? {
                folders = Some(context.host().workspace().get_folders(workspace)?);
            }
        }
        run_query(&params, &password, folders)
    })
    .expect("a valid function ID");

let host = backend.handle().host();
backend
    .on_command("db.refresh", move |_command_id, workspace| {
        if let Some(workspace) = workspace {
            host.window()
                .show_information(&workspace, json!({ "text": "Refreshed." }))?;
        }
        Ok(())
    })
    .expect("a valid command ID");
```

| `Host` member | Purpose |
| --- | --- |
| `credentials().get(key)`, `.set(key, secret)`, `.delete(key)` | Secrets in the platform's store; see [Credentials](#credentials). |
| `capabilities()`, `supports(method)` | What this session may call, from `lithe/host/getCapabilities` (asked once). |
| `workspace().list_contexts()`, `.get_context(&context)`, `.get_folders(&context)` | Workspace contexts; only `get_context` returns, and creates, the context's `stateDirectory`. |
| `workspace().read_directory`, `.stat`, `.read_file`, `.find_files`, `.find_text`, `.watch` | Files of a local context, addressed as `{ rootID, path }` from `get_folders`. |
| `workspace().get_host_paths(&context, root_ids)`, `.process_roots(&full_context)` | Root paths on the remote host of a remote context; see [Remote workspaces](#remote-workspaces). |
| `documents().list`, `.get_snapshot`; `editor().get_active`, `.get_selections`, `.apply_edits`, `.replace_selections`, `.insert_text`, `.open`, `.reveal`, `.register_actions` | Live documents and editors. Edits name the `expectedVersion` they were computed against. |
| `preview().open_html`, `browser().open_external`, `window().show_information`, `.show_warning`, `.show_error` | Preview, browser, and messages. |
| `functions().providers(function_id)`, `.invoke(&context, call)` | The plugins that export a function, and a call to one of them; see [Calling other plugins](#calling-other-plugins). |
| `request(method, params)`, `subscribe(method, params)` | Any `lithe/host/*` method, including ones a later host adds. |
| `subscriptions().dispose(id)` | Ends a subscription; `Subscription::dispose()` does the same. |
| `with_cancellation(&cancellation)` | The same services, cancelled together with `cancellation`. |
| `negotiated()` | `true` when the host offered `litheFunctions` in this session; `false` before `initialize`. |

A host registers only the services it implements, and `capabilities()` lists
exactly those; call `supports(method)` before depending on one of them. What
the Lithe desktop does not register yet is listed under
[Capability versions](../../reference/capabilities-and-versions.md#capability-versions-requires).

Methods taking a workspace context take a `&WorkspaceContext`: one from
`context.workspace_context()`, `WorkspaceContext::new(id, generation)`, or
`WorkspaceContext::from_value` on a whole context from `list_contexts`. Their
`params` are the contract's, without `context`, and they return the
contract's result as a `Value`. The full params and results are in
[the contract](../../contract/v1/README.md#public-document-file-and-window-methods).

What the SDK does for you, and what it leaves to you:

- **Refusals keep the host's code.** Branch on `code()`, such as
  `plugin.host.contextExpired`; `rpc_code()` is the JSON-RPC number and
  `details()` the host's object, if any. A code this SDK does not know is
  kept as sent. On `versionConflict` reread and recompute; on
  `contextExpired` resolve the original target again, never the focused
  window; retry `busy` and `rateLimited` (after the `retryAfterMilliseconds` detail) with a
  bound; `permissionDenied` and `readOnly` need the user. An unknown param is
  `plugin.host.unsupportedOption` naming it in the `option` detail, so you can
  retry without it.
- **Cancellation has no rollback.** Cancelling the token given to
  `with_cancellation`, or ending the call whose `context.host()` sent a
  request, fails it at once with `sdk.call.cancelled`; the SDK also sends
  `$/cancelRequest` so the host stops. An edit or an external open may
  already have taken effect, and the SDK never sends a request again.
- **At most 16 host requests are unanswered.** Further ones wait in the SDK
  for a free slot instead of being refused.
- **Subscriptions route by ID.** `workspace().watch`,
  `editor().register_actions`, and `subscribe` return a `Subscription`;
  `recv()` and `recv_timeout(duration)` give each `Event` in order, with
  `sequence()`, `event()`, and `gap()`, which is true when events were
  missed, so rescan. Events the host sends before its answer are kept. No
  event is delivered after `dispose()` or the end of the session; once the
  queued ones are read, `recv()` returns `None`.
- **Commands are events.** `on_command` runs on a thread of its own when the
  user runs one of the plugin's commands, with the window's
  `WorkspaceContext` when the host sent one. The host waits for no result,
  and a failure is only logged. A stopped backend is started for it only
  when the manifest declares `onCommand:<commandID>`. Host notifications
  without a handler are ignored.

### Credentials

A key is 1–128 ASCII letters, digits, `.`, `_`, `:`, or `-`; a secret is at
most 16 KiB. Credentials are private to your plugin ID, survive disabling and
updating the plugin, and are deleted when it is uninstalled. They are never
written to `${stateDir}` or the package. `get` returns `Ok(None)` when
nothing is stored.

| Failure (`SdkError::code()`) | Meaning |
| --- | --- |
| `plugin.host.unavailable` | The host did not offer `litheFunctions`, so it has no host services. Nothing was sent. |
| `plugin.host.permissionDenied` | The plugin lacks the `credentials.store` permission. |
| `plugin.host.invalidParams`, `sdk.credentials.invalidKey`, `sdk.credentials.invalidSecret` | A malformed key or secret. |
| `plugin.host.failed` | The host's secret store failed, for example a locked secret store; the message says why and never contains the secret. |
| `sdk.call.cancelled` | The call was cancelled while waiting; `?` answers as cancelled. |
| `sdk.session.notReady`, `sdk.session.closed` | The session has not finished `initialize`, or it ended while waiting. |

### Remote workspaces

A backend runs on the machine that runs Lithe also when the workspace is on a
remote host. Such a context has `authority.kind` `remote` and
`authority.platform` `{ os, arch }`, and its folders carry no `path`.
`get_host_paths` gives their paths on the host. It needs the
`workspace.metadata` permission, and only a host that serves it lists it, so
check `supports("lithe/host/workspace/getHostPaths")` first.

```rust
use lithe_plugin_sdk::ProcessRoot;

let full = host.workspace().get_context(&workspace)?;
for root in host.workspace().process_roots(&full)? {
    match root {
        ProcessRoot::Local { root_id, path } => { /* a path on this machine */ }
        ProcessRoot::Remote { root_id, host_path } => { /* a path on the remote host */ }
    }
}
```

- `HostPath` is its own type and converts to no `Path`, so it is not
  passed to `std::fs` or `std::process` by mistake: the machine that runs
  the backend has no file there. Give `as_str()` only to what runs on the
  host, such as process arguments or a debug configuration; it serializes as
  that string.
- `process_roots` takes a full context. For a `local` one it reads the
  folders without asking the host; otherwise it calls `get_host_paths`.
- `get_host_paths(&context, None)` asks for every root; `Some(ids)` takes
  1–256 distinct root IDs, and others fail locally with
  `sdk.host.invalidParams`. An answer without absolute paths is
  `sdk.host.invalidResult`.
- `plugin.host.remoteDisconnected` means the connection was lost while the
  request ran, so whether it took effect is unknown;
  `plugin.host.platformUnavailable` means the plugin has nothing for the
  host's platform. The
  [remote workspaces guide](../../guides/plugin-backend-remote-workspaces.md)
  says which services serve a remote context.

### Calling other plugins

A backend may call a function another plugin exports; Core relays the call and
the provider's backend never learns more than the call it serves. The
manifest declares both sides: the caller lists `{ id, version }` under
`pluginFunctions.calls`, and the provider lists
`{ id, version, context: "workspace" }` under `pluginFunctions.exports` and
registers the function with `backend.function(id, handler)`. See
[cross-plugin calls](../../contract/v1/README.md#cross-plugin-calls)
for the manifest, the order of Core's checks, and the call chain.

```rust
backend
    .function("project.options", |_params, context| {
        let workspace = context
            .workspace_context()
            .ok_or_else(|| PluginError::new("example.noWorkspace", "Open a folder first."))?;
        let host = context.host();
        if !host.supports("lithe/host/functions/invoke")? {
            return Ok(json!({ "options": [] }));
        }
        let providers = host.functions().providers("project.describe")?;
        let Some(provider) = providers["providers"][0]["pluginID"].as_str() else {
            return Ok(json!({ "options": [] }));
        };
        let answer = host.functions().invoke(
            workspace,
            json!({
                "pluginID": provider,
                "functionID": "project.describe",
                "params": { "project": "app" },
                "timeoutMilliseconds": 5000,
            }),
        )?;
        Ok(json!({ "options": answer["value"] }))
    })
    .expect("a valid function ID");

backend
    .function("project.describe", |params, context| {
        // Core wrote the caller from the calling session; it cannot be forged.
        let caller = context.caller().map(|caller| caller.plugin_id());
        Ok(json!({ "project": params["project"], "askedBy": caller }))
    })
    .expect("a valid function ID");
```

- **Check before calling.** `supports("lithe/host/functions/invoke")` and
  `supports("lithe/host/functions/providers")` tell whether this session may
  call. Host services alone are not enough: a plugin that declares no `calls`
  gets `plugin.host.permissionDenied`. When the host did not offer
  `litheFunctions`, `providers`, `invoke`, and `host().request` with either
  method fail locally with `plugin.host.unavailable` and send nothing.
- **`providers(function_id)`** answers `{ "providers": [{ "pluginID",
  "version" }] }`: the enabled plugins other than this one that export the
  function, sorted by plugin ID. This plugin must have declared the function,
  or the host answers `plugin.function.notDeclared`. A provider's backend need
  not be running.
- **`invoke(&context, call)`** answers `{ "value": ... }`, where `value` is the
  provider's result and may be `null`. `context` is a `WorkspaceContext` this
  backend learned from the workspace service or `context.workspace_context()`;
  the relay accepts local workspaces only. `call` is an object with
  `pluginID` and `functionID`, and optionally `params` (an object, `{}` by
  default), `timeoutMilliseconds` (a positive integer; the host defaults it to
  60000 and caps it at 600000, and the SDK does not clamp it), and
  `parentOperationID`. Any other key, such as `context` or `caller`, is not
  sent; the host would refuse it as `plugin.host.unsupportedOption`. A call
  that is not an object, lacks `pluginID` or `functionID`, or has a field of
  the wrong type fails locally with `sdk.functions.invalidCall`, and a
  `function_id` that is not a function ID does the same for `providers`.
- **Limits are the host's.** A chain has at most 4 calls and does not return
  to a plugin already on it (`plugin.function.callCycle`); a caller has at
  most 8 relayed calls running; params and a result are at most 256 KiB. A
  provider that is asked to apply values must not call the plugin that asked
  inside that call.
- **`parentOperationID` is filled in for you inside a handler.** A call made
  with `context.host().functions().invoke(...)` while a function runs names
  that call's `operationID`, so Core makes it a child: cancelling or ending
  the handler's call cancels the relayed call. An explicit
  `parentOperationID` in `call` overrides it (1 to 128 bytes). A
  `handle.host()` serves no call and a generic `request` is sent as written,
  so neither fills it in. An ID that names a call this backend is no longer
  serving fails with `plugin.host.cancelled`.
- **A relayed call occupies a request slot.** It counts among the 16
  unanswered host requests until the provider answers, which can take up to
  600 seconds; further requests wait in the SDK. Keep relayed calls bounded
  with `timeoutMilliseconds`.
- **`context.caller()` names the calling plugin.** It is `Some(Caller)` only
  for a call Core relayed, and `None` for a call the host started. The SDK
  accepts only a `caller` that is exactly
  `{ "pluginID": "<non-empty string>" }`; any other shape is read as no
  caller. It names who called, not what they may do: authorize with the
  plugin's own rules.

| Failure (`SdkError::code()`) | Meaning |
| --- | --- |
| `sdk.functions.invalidCall` | The `call` or the function ID was malformed. Nothing was sent. |
| `plugin.host.unavailable` | The host did not offer `litheFunctions`. Nothing was sent. |
| `plugin.host.permissionDenied` | The plugin declares no `pluginFunctions.calls`. |
| `plugin.function.notDeclared`, `.providerDisabled`, `.notExported`, `.versionMismatch` | The call is not declared, the provider is missing or disabled, does not export the function now, or exports another version. |
| `plugin.function.callCycle`, `.depthExceeded`, `.tooManyCalls`, `.payloadTooLarge` | The chain, the caller's running calls, or a payload exceeded a limit. |
| `plugin.function.providerUnavailable`, `.invalidResult` | The provider's backend stopped or did not start in time, or answered with no result or one over 1 MiB. |
| `plugin.function.failed` | The provider's function returned an error. The `pluginErrorCode` entry of `details()` is the provider's own `PluginError` code, and `message()` is its message. |
| `plugin.host.contextExpired`, `plugin.host.timeout`, `plugin.host.cancelled` | The context was closed or replaced, the deadline passed, or the call (or its parent) was cancelled. A remote workspace's context is relayed like a local one. |
| `sdk.host.invalidResult` | The host answered with something other than an object. |

## Results and errors

| Handler outcome | What Lithe sees |
| --- | --- |
| `Ok(value)` (including `Value::Null`) | `result.value` |
| `Err(PluginError)` | `plugin.function.failed` with your message and `pluginErrorCode`. The code is for your plugin's own UI; Lithe never branches on it. |
| Any other error, or a panic | `plugin.function.failed` with a generic message, so paths, secrets, and stack traces never reach the UI. Automatic SDK diagnostics record only a fixed failure category. A panic ends only its call; SDK catch boundaries suppress raw payload printing. |
| `Err(Cancelled)`, or any outcome after cancellation | `requestCancelled` (or Lithe's timeout or revocation outcome) |

`?` converts `PluginError`, `Cancelled`, `SdkError`, `std::io::Error`, and
`serde_json::Error` into `FunctionError`; `FunctionError::internal(details)`
wraps anything else. A result must serialize to at most 1 MiB of JSON. A
function ID is dot-separated lowerCamelCase segments (`snapshot.refresh`), at
most 128 bytes, and a session holds at most 256 functions. A `PluginError`
code matches `[A-Za-z][A-Za-z0-9._-]*`, at most 64 bytes; `PluginError::new`
panics on any other code.

## Rules that keep Lithe in charge

1. **stdout belongs to the protocol.** On Unix, `listen()` moves the
   protocol to a duplicate of the stdout descriptor and points stdout at
   stderr, so a stray `println!` in your code or a dependency lands in the
   host's log. Elsewhere, never print to stdout.
2. **The package is read-only.** Keep caches and databases under `${stateDir}`,
   which Lithe also passes as the root and working directory. Diagnostics use
   the SDK and host; native log files require an explicitly supplied `${logDir}`.
   Writing to the package breaks code signing and update deltas.
3. **Secrets go to the credentials service.** Never write a password or
   token to `${stateDir}`, to a log line, or to a `PluginError` message.
4. **Work ends with its call.** Check the cancellation in long loops, use
   `wait_timeout` between retries, and register an `on_cancel` callback for
   blocking driver calls. Lithe has already ended the call when it cancels;
   the SDK answers `requestCancelled` whatever the handler returns.
5. **No calls back into Lithe's dispatch.** The SDK never sends
   `lithe/function/invoke` or a `lithe/build/*` request (it sends only the
   `lithe/build/progress` notification); Lithe would refuse them as
   `plugin.function.recursiveCall`. `Host::request` sends only
   `lithe/host/*` and refuses anything else as `sdk.host.method`. To call
   another plugin, use [`host().functions()`](#calling-other-plugins), which
   sends `lithe/host/functions/invoke` and lets Core relay it.

## Native terminal API

The SDK includes wrappers for plugin-owned native terminal create/show/close,
reopen with bounded display history, disposal and exit subscriptions. They are
host methods discovered through `capabilities()`; they require a host that
offers them and the `terminal.create` permission. See
[the native terminal guide](../../guides/native-terminals.md).

## Testing

Run these from the root of this repository:

```sh
cargo test --locked --manifest-path shared/plugins/sdk/rust/Cargo.toml
cargo fmt --manifest-path shared/plugins/sdk/rust/Cargo.toml --all -- --check
cargo clippy --locked --manifest-path shared/plugins/sdk/rust/Cargo.toml --all-targets -- -D warnings
node shared/plugins/scripts/sdk-e2e.mjs
```

The unit tests drive a session through in-process pipes with a scripted
host; `plugin-sdk/tests/host_services.rs` checks the host services against the
contract's fixture. The end-to-end check builds the sample, installs it as a
package, and runs it through a real Core session of this checkout. It needs
Rust, Node.js 20.3 or newer, Go 1.27.4 or newer, Java
(`LITHE_PLUGIN_SDK_JAVA_RUNTIME` or `java` on `PATH`) and Maven; `LITHE_PLUGIN_SDK_RUST_SAMPLE` names an already built
`sample-backend`.
