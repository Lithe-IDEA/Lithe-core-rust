# Lithe plugin backend SDK for Java

`lithe-plugin-sdk-java` is the official way to write a plugin backend in Java:
the process a plugin starts through a `runtime` launch
(`{ "kind": "runtime", "tool": "java", "minimumVersion": "8", "entry": "backend/app.jar" }`).
It is the Java twin of the [Node.js SDK](../node/README.md). Both implement the
following protocols from the
[plugin contract](../../contract/v1/README.md), which lives in this repository
next to the SDK and changes with it, with the same rules and error mapping:

- **`litheBuild`**: typed `lithe/build/*` requests, for a build system whose
  `model` or `dependencies` source is `provider`: models and dependency trees,
  bounded task execution, tests, run and debug preparation, and read-only
  model inputs.
- **`litheFunctions`**: plugin functions, which are plugin-private actions
  that Lithe invokes for the same plugin, with progress, cancellation, and
  registration at runtime; [host services](#host-services) such as
  credentials, workspace contexts, capability discovery, host notifications,
  and the public document, file, editor, and window methods; and
  [calls to other plugins' functions](#calling-other-plugins), which Core
  relays. A function also learns which plugin called it.
- **`litheKeymapImport`**: independently negotiated shortcut import
  providers reading only explicitly selected source snapshots.
- **`litheLogging`**: typed diagnostics, required in every session.

The Java SDK has no VCS provider API; only the Node.js SDK has one. The SDK
requires Java 8 or newer and has no runtime dependencies. JSON values are plain
`Map<String, Object>`, `List<Object>`, `String`, `Long`, `Double`, `Boolean`,
and `null` (see `Json`).

## What the Lithe desktop serves

The Lithe desktop declares `backend`, `pluginFunctions`, `buildProviders` and
`buildSystems` (all version 0), so the function, host-service and build
protocols below run there. It does not declare `keymapImportProviders` or
`buildModelInputs`: a plugin whose manifest requires one of them is not offered
for installation there. The Core end-to-end tests still drive those protocols.
Which host services a session may call is reported at run time; see
[Host services](#host-services).

## Shortcut import providers

Declare `backend.keymapImportProvider(id, handler)` before listening. Its
handler receives a `KeymapSnapshot` with `providerId()`, `snapshotId()`,
`listInputs()` and `readChunk(fileId, offset, maximumBytes)`, plus the normal
call context. Paths returned by `listInputs` are relative labels; reads use
opaque file IDs. Return a normalized `{ bindings: [...] }` map and observe
the call's cancellation. The host revokes the snapshot when the call ends.
Do not turn conditional or multi-stroke rules into global bindings.

## A minimal backend

```java
Backend backend = Backend.create("example-backend", "1.0.0");

backend.onModel((params, context) -> {
    Map<String, Object> project = new LinkedHashMap<>();
    project.put("name", "app");
    project.put("path", Json.string(params, "root"));
    Map<String, Object> model = new LinkedHashMap<>();
    model.put("projects", Collections.singletonList(project));
    return model;
});

backend.function("snapshot.refresh", (params, context) -> {
    context.progress("Refreshing", 10.0);
    Processes.Result result = context.run(Arrays.asList("example-tool", "refresh"));
    if (result.exitCode() != 0) {
        throw new PluginError("example.refreshFailed", "The refresh failed.");
    }
    return Collections.singletonMap("refreshedAt", System.currentTimeMillis());
});

backend.listen();
```

[`examples/sample-build`](examples/sample-build) is a complete plugin. It has
a build provider, functions, persistent state, a function registered at
runtime, and cancellation. From the root of this repository,
`node shared/plugins/scripts/sdk-e2e.mjs` drives it through a real Core session
with the scenario in [`sdk/e2e`](../e2e/plugin_sdk/mod.rs), the same one the
Node.js sample answers.

## Getting the SDK

The SDK is released from this repository, tagged `sdk-java-v<version>`; the
current release is
[0.5.0](https://github.com/Lithe-IDEA/Lithe-core-rust/releases/tag/sdk-java-v0.5.0).
A release ships `lithe-plugin-sdk-java-<version>.jar` and a `.sha256` file
holding the JAR's SHA-256. The SDK is not published to a Maven repository. Use
it like this:

1. Download the JAR during your plugin's build, never at runtime.
2. Check the JAR against the SHA-256 you pinned in your repository.
3. Shade it into your backend JAR, as the sample's `pom.xml` does.

A plugin package is installed read-only and must run offline, so the backend
JAR it starts must hold everything it loads.

## Logging

The SDK requires `experimental.litheLogging` at initialization and refuses a
host that does not offer it.
See [plugin logging](../../guides/plugin-logging.md) for the typed diagnostics
API, host-owned catalog, exact levels, operation correlation and `${logDir}`.
Use `diagnostics().degraded/recovered` for handled fallback or recovery facts.
Function and build terminal facts are automatic; do not log a failure before
returning it to the SDK. Suppress successful high-frequency reads with
`suppressSuccessDiagnostics(operation)`. There is no free-text `log` API, no
stderr fallback and no plugin-owned file writer.

## API

| API | Purpose |
| --- | --- |
| `Backend.create(name, version[, capabilities])` | Creates the backend. `capabilities` are standard LSP server capabilities; the SDK negotiates its experimental protocols. |
| `onModel(handler)`, `onDependencies(handler)` | Implement the typed build requests. The SDK stamps `format` and `version`. Core validates the document. `litheBuild` is answered only when a handler exists. |
| `onExecute(handler)` | A bounded provider-owned task, returning `{ exitCode, output }`. |
| `onTest(handler)`, `onDiscoverTests(handler)` | Exact test execution and runnable selectors; the discovery format and version are stamped. |
| `onPrepareRun(handler)`, `onPrepareDebug(handler)` | Prepare a portable run or debug plan; the host owns the application. |
| `negotiated().build()` | Whether the host offered `litheBuild` and the backend declared a build handler; build methods are refused before their handler runs when it is false. |
| `function(id, handler)` | Declares a function offered in `initialize`. |
| `registerFunction(id, handler)`, `unregisterFunction(id)` | Add or withdraw a function. Called before the `initialize` answer (for example from `onInitialize`), they change the initial list. Later calls send `lithe/function/register` or `lithe/function/unregister`, in order. Both return a `CompletableFuture`. Withdrawing a function revokes its running calls. |
| `negotiated().functions()` | Whether the host offered plugin functions. `host().negotiated()` reports the same. |
| `host()` | [Host services](#host-services) for work outside a call, such as a command handler. Its calls to other plugins name no parent call. |
| `onCommand(commandId, handler)` | Runs `handler.handle(commandId, workspaceContext)` when the user runs one of the plugin's commands. |
| `onContextsChanged(handler)` | `lithe/host/workspace/didChangeContexts` with its `{ revision }`. |
| `onHostNotification(method, handler)` | A `lithe/host/*` notification a later host adds. The three this SDK routes itself are reserved. |
| `onInitialize(hook)`, `onShutdown(hook)` | Lifecycle hooks. `onInitialize` receives the `initialize` params and what was negotiated. |
| `onRequest(method, handler)`, `onNotification(method, handler)` | Ordinary LSP methods. Lifecycle and `lithe/*` methods are reserved. |
| `sendRequest`, `sendNotification` | Ordinary LSP messages to the host. `lithe/*` methods are reserved. |
| `diagnostics().degraded(operation, code[, count])`, `.recovered(...)` | Typed handled fallback/recovery; no payload text. |
| `listen()` | Serves stdio until `exit` or the end of input, then exits the JVM with the protocol's exit code. `listen(in, out, err)` serves other streams and returns the code instead. |
| `PluginError(code, message)` | A failure meant for your own UI. |
| `Processes.run(command[, options])` | Runs a process without a shell. Its stdin is closed. `Options` set the directory, environment, output limits, a cancellation, and output callbacks. |
| `StateStore.open(directory)` | A small atomic JSON store in a host-provided directory. |

Handlers run on their own threads. Every handler receives a `CallContext`:

| Member | Meaning |
| --- | --- |
| `cancellation()`, `isCancelled()` | Cancelled when any of these happens: Lithe cancels the call, its deadline passes, the function is withdrawn, the session ends, or the handler finishes. Finishing the handler stops the work it left running. |
| `throwIfCancelled()` | Throws `CancelledException` once the call is cancelled. |
| `run(command[, options])` | `Processes.run` bound to the call: cancelling or ending the call stops the process. |
| `diagnostics()` | Typed diagnostics correlated with this call. |
| `functionId()` | Functions only. |
| `host()` | [Host services](#host-services) bound to the call: ending the call cancels the requests it left unanswered. Its calls to other plugins name this call as their parent. |
| `workspaceContext()` | Functions: the `{ contextID, generation }` of the window that invoked the call, or null. |
| `caller()`, `callerPluginId()` | Functions: the plugin that called this function through the relay, as an unmodifiable `{ pluginID }` map and as its ID. Null when the host started the call. |
| `operationId()` | Opaque wire identity for functions and bounded build/test/run-preparation calls; null for other requests. It falls back to the JSON-RPC request ID, so never use it as a `parentOperationID`. |
| `progress(message, percentage[, output])` | Functions use 1024-byte messages. Build/test/run preparation use 4096-byte messages and optional output up to 64 KiB. Percentages are clamped to 0–100. Cancelled or settled calls cannot report. Model/dependency/discovery reports are ignored. |

The typed handlers for task execution, tests, and run and debug preparation
require the corresponding host capabilities (`buildProviders`); the manifest
requirement and runtime negotiation are both checked, and registering a handler
never bypasses either boundary. See [native build workflows](../../guides/native-build-workflows.md)
and the [no-page Java sample](examples/sample-native-build/src/main/java/org/example/samplenativebuild/SampleNativeBuild.java).
Build tool failure is an exit-code result; throwing an error aborts preparation.

Private function results and errors map exactly as in the Node.js SDK; typed
build operations follow their domain contract:

- A returned value becomes `result.value`.
- `PluginError` becomes `plugin.function.failed` with your message and
  `pluginErrorCode`.
- Any other exception is reported with a generic message; automatic diagnostics
  keep a fixed failure category without its message, cause or stack.
- `CancelledException` ends the call as cancelled.
- A result larger than 1 MiB of JSON fails like an unexpected error.

## Host services

A backend may ask the host for services when the host offers plugin
functions (`litheFunctions`); each is granted by a manifest permission and
refused otherwise. When the host did not offer them, every host request fails
locally with `HostError` `plugin.host.unavailable` (-32601), the answer Core
would give, and nothing is sent. Every method returns a `CompletableFuture`;
wrong arguments fail it rather than throwing.

```java
backend.function("query.run", (params, context) -> {
    // Permission credentials.store: the platform's secret store, per plugin.
    String password = context.host().credentials().get("connection.prod:password").get();
    if (password == null) {
        throw new PluginError("query.noPassword", "Sign in first.");
    }
    // The window the page runs in.
    Map<String, Object> workspace = context.workspaceContext();
    if (workspace != null && context.host().supports("lithe/host/workspace/getFolders").get()) {
        Object folders = context.host().workspace().getFolders(workspace).get().get("folders");
        // ...
    }
    return runQuery(params, password);
});

backend.onCommand("db.refresh", (commandId, workspaceContext) -> {
    if (workspaceContext != null) {
        backend.host().window().showInformation(workspaceContext, Collections.singletonMap("text", "Refreshed."));
    }
});
```

| `Host` member | Purpose |
| --- | --- |
| `negotiated()` | Whether the host offered plugin functions. When false, every other member fails or reports false locally. |
| `credentials().get(key)`, `.set(key, secret)`, `.delete(key)` | Secrets in the platform's store. `get` completes with null when nothing is stored. A key is 1–128 of `[A-Za-z0-9._:-]`, a secret at most 16 KiB; others fail locally. |
| `capabilities()`, `supports(method)` | What this session may call, from `lithe/host/getCapabilities` (asked once). `supports` is false when the host offered no plugin functions. |
| `workspace().listContexts()`, `.getContext(context)`, `.getFolders(context)` | Workspace contexts; only `getContext` returns, and creates, the context's `stateDirectory`. |
| `workspace().readDirectory`, `.stat`, `.readFile`, `.findFiles`, `.findText`, `.watch` | Files of a local context, addressed as `{ rootID, path }` from `getFolders`. |
| `documents().list`, `.getSnapshot`; `editor().getActive`, `.getSelections`, `.applyEdits`, `.replaceSelections`, `.insertText`, `.open`, `.reveal`, `.registerActions` | Live documents and editors. Edits name the `expectedVersion` they were computed against. |
| `preview().openHtml`, `browser().openExternal`, `window().showInformation`, `.showWarning`, `.showError` | Preview, browser, and messages. |
| `functions().providers(functionId)`, `.invoke(context, call)` | [Calls to other plugins' functions](#calling-other-plugins), relayed by Core. |
| `request(method, params)`, `subscribe(method, params, listener)` | Any `lithe/host/*` method, including ones a later host adds. |
| `subscriptions().dispose(id)` | Ends a subscription; `Subscription.dispose()` does the same. |
| `withCancellation(cancellation)` | The same services, cancelled together with `cancellation`. |

A host registers only the services it implements, and `capabilities()` lists
exactly those; call `supports(method)` before depending on one of them. What
the Lithe desktop does not register yet is listed under
[Capability versions](../../reference/capabilities-and-versions.md#capability-versions-requires).

Methods taking a workspace context accept `{ contextID, generation }` or a
whole context from `listContexts`; their `params` are the contract's, without
`context`. The full params and results are in
[the contract](../../contract/v1/README.md#public-document-file-and-window-methods).

What the SDK does for you, and what it leaves to you:

- **Refusals are `HostError`.** Branch on `code()`, such as
  `plugin.host.contextExpired`; a code this SDK does not know is still a
  `HostError`, and `details()` holds the host's object. On `versionConflict`
  reread and recompute; on `contextExpired` resolve the original target
  again, never the focused window; retry `busy` and `rateLimited` (after
  `details().get("retryAfterMilliseconds")`) with a bound; `permissionDenied`
  and `readOnly` need the user. An unknown param is
  `plugin.host.unsupportedOption` naming it in `details().get("option")`, so
  you can retry without it.
- **Cancellation has no rollback.** Cancelling the token given to
  `withCancellation`, or ending the call that sent a request, fails it at
  once with `CancelledException`, and the SDK also sends `$/cancelRequest` so
  the host stops. An edit or an external open may already have taken effect,
  and the SDK never sends a request again. Cancelling the returned future
  itself does not reach the host.
- **At most 16 host requests are unanswered.** Further ones wait in the SDK
  for a free slot instead of being refused.
- **Subscriptions route by ID.** `workspace().watch` and `subscribe` call
  the `Listener` with each `Host.Event` in order, one at a time; `gap()` is
  true when events were missed, so rescan. Nothing arrives after `dispose()`
  or the end of the session.
- **Commands are events.** `onCommand` runs when the user runs one of the
  plugin's commands; the host waits for no result. A stopped backend is
  started for it only when the manifest declares `onCommand:<commandID>`.

## Calling other plugins

A backend may call a function that another plugin exports. The plugin must
declare the call in its manifest, and Core relays it: the backend never talks
to the other plugin. A plugin that declares no call gets
`plugin.host.permissionDenied`. The exports, the declaration and the checks
Core makes are in the
[contract](../../contract/v1/README.md#cross-plugin-calls).

```java
backend.function("project.sync", (params, context) -> {
    Host.Functions functions = context.host().functions();
    Object providers = functions.providers("languageImport.settings").get().get("providers");
    // ... choose a provider ...
    Map<String, Object> call = new LinkedHashMap<>();
    call.put("pluginID", "dev.example.go");
    call.put("functionID", "languageImport.settings");
    call.put("params", Collections.singletonMap("project", "app"));
    call.put("timeoutMilliseconds", 5000L);
    // Core decides who may be called for which workspace; use the one the user's call ran in.
    Object settings = functions.invoke(context.workspaceContext(), call).get().get("value");
    return Collections.singletonMap("settings", settings);
});
```

| Member | Purpose |
| --- | --- |
| `functions().providers(functionId)` | Completes with `{ providers: [{ pluginID, version }] }`: the enabled plugins other than yours that export the function, sorted by plugin ID. You must have declared the function. |
| `functions().invoke(context, call)` | Completes with `{ value }`, the provider's result (`value` may be null). `context` is a workspace context, `{ contextID, generation }` or a whole one from `listContexts`; the call runs for that workspace. `call` is a map with `pluginID` and `functionID`, and optionally `params` (a map, default `{}`), `timeoutMilliseconds` (a positive integer; Core defaults it to 60 000 and caps it at 600 000) and `parentOperationID`. |
| `CallContext.caller()`, `.callerPluginId()` | On the providing side: the plugin that called this function, as `{ pluginID }` and as its ID. Core writes it from the calling session. Null when the host started the call. |
| `HostError.pluginErrorCode()` | For `plugin.function.failed`, the provider's own error code (from `details.pluginErrorCode`); null otherwise. |

What the SDK does for you, and what it leaves to you:

- **The relay needs plugin functions.** When the host did not offer
  `litheFunctions`, `providers`, `invoke`, and a `request` for either method
  fail at once with `HostError` `plugin.host.unavailable` (-32601), the answer
  Core would give, and nothing is sent. Check `negotiated().functions()` or
  `supports(method)` first.
- **The parent call is filled in for you.** Inside a function, `context.host()`
  sets `parentOperationID` to the wire `operationID` of the function call it
  belongs to, so Core knows which call the new one is a child of. The ID is
  used only when it is a string of 1 to 128 bytes; otherwise the field is
  omitted. The SDK never uses `operationId()` for this, because it falls back
  to the JSON-RPC request ID. A `parentOperationID` you put in `call` wins.
  `withCancellation` keeps the parent. `Backend.host()`, which belongs to no
  call, names none. Without a parent Core treats the new call as a child of
  every relayed call the backend is serving.
- **Wrong arguments fail locally.** A malformed call completes the future
  with `PluginSdkError` `sdk.functions.invalidCall` (a missing or wrong
  workspace context is `sdk.host.invalidContext`) and sends nothing: a
  `functionID` that is not a function ID, an empty `pluginID`, `params` that
  is not a map, a `timeoutMilliseconds` that is not a positive integer, or a
  `parentOperationID` that is not 1 to 128 bytes of text. A null optional
  field counts as absent. Only the fields above are sent; any other key,
  `caller` included, is dropped, because Core refuses a caller-chosen
  `caller`. The SDK does not cap `timeoutMilliseconds`; Core does.
- **Refusals are `HostError`.** Besides the host codes, the relay adds
  `plugin.function.notDeclared`, `.notExported`, `.providerDisabled`,
  `.versionMismatch`, `.callCycle`, `.depthExceeded`, `.tooManyCalls`,
  `.payloadTooLarge`, `.providerUnavailable`, `.failed` and `.invalidResult`.
  `plugin.function.failed` carries the provider's own code in
  `pluginErrorCode()`. A call to a context the host closed fails with
  `plugin.host.contextExpired`; a remote workspace with
  `plugin.host.authorityUnsupported`; a deadline with `plugin.host.timeout`.
  Core never retries a call, and neither does the SDK.
- **Calls end with their caller.** Ending or cancelling the function call that
  made a relayed call, or cancelling the token given to `withCancellation`,
  fails the future with `CancelledException` and sends `$/cancelRequest`, so
  Core cancels the provider's operation and the calls it made.
- **Limits are Core's.** A chain has at most 4 calls, and a plugin already on
  it, the caller included, cannot be called (`callCycle`): a provider asked to
  apply values must not call the asker back inside that call. A caller has at
  most 8 relayed calls running; params and results are at most 256 KiB. Only
  local workspaces are supported. A relayed call holds one of the 16
  unanswered host-request slots until it ends, which may be 600 seconds, so a
  backend that starts many long calls delays its other host requests.
- **Decide by `caller()`, never by params.** A plugin ID in `params` is data
  the caller wrote; `context.caller()` is the identity of the calling session.

## Rules that keep Lithe in charge

These are the [Node.js SDK's rules](../node/README.md#rules-that-keep-lithe-in-charge).
In Java:

1. **Domain capabilities stay typed.** Build models and dependency trees go
   through `onModel` and `onDependencies`, never through a function.
2. **No calls back into Lithe's dispatch.** `sendRequest` and
   `sendNotification` refuse any `lithe/*` method. Host services go through
   `host()`, which sends only `lithe/host/*`.
3. **stdout belongs to the protocol.** `listen()` keeps the real stdout for
   protocol frames and points `System.out` at stderr. Never write to
   `FileDescriptor.out`.
4. **The package is read-only.** Keep state under the directories Lithe
   passes through launch templates (`${stateDir}`, `${workspaceStateDir}`).
   `StateStore.open` refuses a directory inside the plugin package. It finds
   the package by walking up from the SDK's own JAR to `plugin.json`.
   Diagnostics use the SDK and host; native log files require an explicitly
   supplied `${logDir}`.
5. **Work ends with its call.** Start processes with `context.run`, and check
   `isCancelled()` in long loops. On cancellation the SDK first asks the
   process to stop, then stops it forcibly after a grace period.
   - **macOS and Linux, every Java version:** each tool leads its own process
     group, as in the Node SDK. The SDK starts it through the system `perl`,
     which calls `setpgrp` and then becomes the tool. A stop reaches the whole
     group, and when the tool exits on its own, whatever it left running in
     its group is killed.
   - **Windows, or no `perl`:** on Java 9 or newer the SDK stops the
     descendants still attached to the tool; Java 8 stops only the direct
     child.

   A descendant that leaves the group (`setsid`) or that the fallback cannot
   reach is ended by Lithe's process scope together with the backend.
   A cancellation listener that throws is logged and does not keep the other
   listeners or the reply from running.

   On `shutdown`, `exit`, or the end of input the SDK ends every process it
   started. `Processes.run` keeps Lithe's process-scope markers even when
   `Options.environment` replaces the environment.
6. **Errors stay presentable.** Throw `PluginError` for failures the user
   should see; every other exception is reported generically.

## Read-only build model inputs

The build protocol accepts the host's optional `params.modelInputs` snapshot.
A consumer declares exact `modelInputs` producer slots and requires
`buildProviders: 0` plus `buildModelInputs: 0`. A producer opts into
`modelSharing` and selects any ordinary build option values it wants to share.
The host supplies normalized projects, targets and content roots; tasks, run
plans, settings and unselected option values are excluded.

Treat `revision` as an opaque workspace snapshot identity. Recompute cached
work when it changes. A slot can be unavailable, and an instance can be loading,
stale or failed; only ready instances carry facts. Do not invoke another plugin
or install/activate a missing producer. Input sharing grants no process or
private-function authority. Producers cannot consume and re-export
inputs. Core validates the shape and the host revokes in-flight work when its
producer changes.

The two native workflow samples demonstrate independent producer and consumer
backends in the Core end-to-end tests. Their engine is fictional. The Lithe
desktop does not declare `buildModelInputs`.

## Native terminal API

The SDK includes wrappers for plugin-owned native terminal create/show/close,
reopen with bounded display history, disposal and exit subscriptions. They
require `terminal.create`. See
[the native terminal guide](../../guides/native-terminals.md).

## Testing

Run these from the root of this repository:

```bash
mvn -B -ntp -f shared/plugins/sdk/java/pom.xml verify

# End to end: the sample backends of all three SDKs against this checkout's Core.
node shared/plugins/scripts/sdk-e2e.mjs
```

This builds the SDK and the sample for Java 8 with any newer JDK, then runs
the JUnit tests. Add `-Djvm=/path/to/java8/bin/java` to run the tests on a
Java 8 runtime, as CI does.

The end-to-end script needs Rust, Node.js 20.3 or newer, Java
(`LITHE_PLUGIN_SDK_JAVA_RUNTIME` or `java` on `PATH`) and Maven;
`LITHE_PLUGIN_SDK_JAVA_SAMPLE` and `LITHE_PLUGIN_SDK_JAVA_NATIVE_SAMPLE` name
already built sample JARs.
