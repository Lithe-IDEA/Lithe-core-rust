# 从插件打开 Lithe 原生终端 / Open a native Lithe terminal from a plugin

[中文](#中文) · [English](#english)

## 中文

这些 SDK 封装随 Node.js SDK 0.5.0、Java SDK 0.5.0 和 Rust SDK 0.4.0 提供，并且需要宿主声明终端方法。契约和 SDK 都在 Core 仓库的 `shared/plugins/` 中，规则见[原生终端契约](../contract/v1/README.md#plugin-owned-native-terminals)。

交互式客户端请使用宿主的集成终端。声明 `requires.backend: 2` 和权限 `terminal.create`；网页调用自己后端的函数，后端带着该函数的工作区上下文调用宿主。插件不需要实现终端模拟器，不需要通过插件函数转发按键，也不需要成为构建或运行提供者。

先用 `host.supports` 探测 `lithe/host/terminal/create` 和 `reopen`。宿主不提供这些方法时，探测返回 false。首个实现只支持本机工作区，并明确拒绝远程 authority。宿主检查调用方、显示执行确认，并管理 PTY、渲染、焦点、尺寸调整和有界的进程树关闭。传 `show:false` 时，create 和 reopen 保留当前的终端选择，不展开终端面板，也不请求输入焦点。需要让新标签页显示出来时，调用 `show`。

### Node.js 示例

```js
const host = context.host;
const workspace = context.workspaceContext;
if (!workspace || !await host.supports('lithe/host/terminal/reopen')) {
  throw new Error('This host has no native terminal API.');
}
const terminal = await host.terminal.create(workspace, {
  title: 'Interactive client',
  executable: 'example-client',
  arguments: ['--interactive'],
  retainedContextBytes: 16 * 1024,
});
// 退出订阅需要比这次调用活得更久时，把它放在后端生命周期的 host 上。
const events = await backend.host.terminal.onExit(workspace, terminal.terminalID, event => {
  console.error(event.generation, event.exitCode, event.reason);
});
await host.terminal.close(workspace, { terminalID: terminal.terminalID, retainedContextBytes: 4096 });
await host.terminal.reopen(workspace, { terminalID: terminal.terminalID });
await events.dispose();
await host.terminal.dispose(workspace, { terminalID: terminal.terminalID });
```

只传契约支持的字段。尤其是 close 要传 `{terminalID, retainedContextBytes:4096}`，不要把结果里的元数据复制进请求参数；宿主会拒绝未知字段。所有 SDK 都不会在取消后自动重试启动。

### Rust 示例

```rust,ignore
let workspace = context.workspace_context().ok_or_else(||
    lithe_plugin_sdk::PluginError::new("example.noWorkspace", "Open a workspace first."))?;
let host = context.host();
if !host.supports("lithe/host/terminal/reopen")? {
    return Err(lithe_plugin_sdk::PluginError::new(
        "example.unavailable", "This host has no native terminal API.").into());
}
let terminal = host.terminal();
let opened = terminal.create(workspace, serde_json::json!({
    "executable": "example-client", "arguments": ["--interactive"],
    "retainedContextBytes": 4096
}))?;
let id = opened["terminalID"].as_str().ok_or_else(||
    lithe_plugin_sdk::FunctionError::internal("Host returned no terminalID"))?;
terminal.close(workspace, id, Some(4096))?;
terminal.reopen(workspace, id)?;
terminal.dispose(workspace, id)?;
```

Rust 还提供 `terminal.on_exit(workspace,id)`，返回 SDK 已有的 `Subscription`，用有界的 `recv_timeout` 接收，并显式调用 `dispose`。

### Java 接口

`host.terminal()` 提供 `create(context,params)`、`show(context,params)`、`close(context,params)`、`reopen(context,params)`、`dispose(context,params)` 和 `onExit(context,params)`。参数是普通的 JSON Map。前五个返回 `CompletableFuture<Map<String,Object>>`（show 和 dispose 返回 null）；`onExit` 返回 `CompletableFuture<Subscription>`。上下文、取消、宿主失败和订阅路由与其他协议 3 方法使用同一通道。

### 启动与重开的语义

`executable` 是 PATH 中的裸命令名或绝对路径；`arguments` 保持 argv 数组。可选的 `cwd` 是工作区内的 `{rootID,path}` 目录资源，不能离开工作区。`environment` 是字符串覆盖值或表示删除的 null 组成的映射，不是 shell 赋值语句。不要把凭据放进标题、参数或日志。输入和输出都留在原生终端里；没有公开的 `sendText`、读取终端输出或枚举用户终端的 API。

`close` 移除标签页并停止进程，保留启动配置和已显示输出末尾的一段有界纯文本。`retainedContextBytes` 是 UTF-8 字节预算：默认 16384，最大 262144，0 表示不保留历史。可以在 create 时设置，也可以在 close 时覆盖。为了不截断 Unicode 字符，实际保留的字节数可能更少。显示或回显过的密钥可能出现在这段历史里，这类会话请设为 0。原始转义序列和 stdin 事件都不保留。

`reopen` 用同样的 executable、argv、cwd 和 environment 启动一个**新进程**，启动前先把保留的那段输出恢复到显示中。它不会重放输入、恢复进程内存或找回 shell 变量。它保留 `terminalID` 并递增 `generation`；事件带有 generation，旧进程的退出不会被误认为新进程的退出。退出订阅在 reopen 之后继续有效。进程自然退出和用户关闭标签页之后也可以 reopen。这两条路径都会先完成有界的原生清理，只保留选定的快照，并释放原生会话和渲染缓冲区，然后才通知订阅者。清理期间句柄处于忙碌状态；退出订阅者收到事件后可以立即 reopen。字节预算为 0 时，任何退出路径都不保留历史。

`dispose` 忘掉句柄、启动设置和历史；会话仍在运行时先关闭它。每个插件最多保留 8 个句柄（包括已关闭的），所以不会再 reopen 的句柄要 dispose。后端或会话重启、插件停用或更新、关闭或替换工作区，也会释放这些句柄。上下文和历史只在内存中，从不写入已安装的插件包。

Rust 示例的 `terminal.cycle` 函数和 Core 端到端场景覆盖协议的往返。该场景由测试提供宿主应答；真实的原生 PTY 输入、输出和历史由宿主自己的测试验证。

终端生命周期场景在 Core 仓库根目录运行，对象是当前工作树的 Core：

```sh
node shared/plugins/scripts/sdk-e2e.mjs
```

示例在已安装的清单里声明了 `terminal.create`，所以测试拿到的授权与已安装插件相同。没有这个权限的旧宿主会拒绝该示例清单；没有声明它的其他插件保持原有兼容性。

---

## English

These SDK wrappers ship from Node.js SDK 0.5.0, Java SDK 0.5.0, and Rust SDK
0.4.0, and require a host that advertises the terminal methods. The contract and the SDKs live in the
Core repository under `shared/plugins/`. See the
[native terminal contract](../contract/v1/README.md#plugin-owned-native-terminals).

Use the host's integrated terminal for an interactive client. Declare
`requires.backend: 2` and permission `terminal.create`; a web page invokes a
function in its own backend, which calls the host with the function's workspace
context. It does not need to implement a terminal emulator, stream keystrokes
through plugin functions, or become a Build/Run provider.

Discover `lithe/host/terminal/create` and `reopen` with `host.supports` first.
A host without these methods makes the probe return false. The first implementation supports
only a local workspace; it explicitly refuses a remote authority.
The host checks the caller, displays execution confirmation, and manages the
PTY, rendering, focus, resize and bounded process-tree shutdown.
With `show:false`, create/reopen preserve the current terminal selection and
do not reveal the terminal panel or request input focus. Call `show` when the
new tab should become visible.

### Node.js

```js
const host = context.host;
const workspace = context.workspaceContext;
if (!workspace || !await host.supports('lithe/host/terminal/reopen')) {
  throw new Error('This host has no native terminal API.');
}
const terminal = await host.terminal.create(workspace, {
  title: 'Interactive client',
  executable: 'example-client',
  arguments: ['--interactive'],
  retainedContextBytes: 16 * 1024,
});
// Keep an exit subscription on a backend-lifetime host if it must outlive this call.
const events = await backend.host.terminal.onExit(workspace, terminal.terminalID, event => {
  console.error(event.generation, event.exitCode, event.reason);
});
await host.terminal.close(workspace, { terminalID: terminal.terminalID, retainedContextBytes: 4096 });
await host.terminal.reopen(workspace, { terminalID: terminal.terminalID });
await events.dispose();
await host.terminal.dispose(workspace, { terminalID: terminal.terminalID });
```

Pass only supported fields. In particular, use `{terminalID,
retainedContextBytes:4096}` for close rather than copying result metadata into
request params; the host refuses unknown fields. No SDK retries a launch after
cancellation automatically.

### Rust

```rust,ignore
let workspace = context.workspace_context().ok_or_else(||
    lithe_plugin_sdk::PluginError::new("example.noWorkspace", "Open a workspace first."))?;
let host = context.host();
if !host.supports("lithe/host/terminal/reopen")? {
    return Err(lithe_plugin_sdk::PluginError::new(
        "example.unavailable", "This host has no native terminal API.").into());
}
let terminal = host.terminal();
let opened = terminal.create(workspace, serde_json::json!({
    "executable": "example-client", "arguments": ["--interactive"],
    "retainedContextBytes": 4096
}))?;
let id = opened["terminalID"].as_str().ok_or_else(||
    lithe_plugin_sdk::FunctionError::internal("Host returned no terminalID"))?;
terminal.close(workspace, id, Some(4096))?;
terminal.reopen(workspace, id)?;
terminal.dispose(workspace, id)?;
```

Rust also exposes `terminal.on_exit(workspace,id)` as an existing SDK
`Subscription`, with bounded `recv_timeout` and explicit `dispose`.

### Java

`host.terminal()` exposes `create(context,params)`, `show(context,params)`,
`close(context,params)`, `reopen(context,params)`, `dispose(context,params)` and
`onExit(context,params)`. Params are normal JSON maps. The first five return a
`CompletableFuture<Map<String,Object>>` (null for show/dispose); `onExit` returns
`CompletableFuture<Subscription>`. Context, cancellation, host failures and
subscription routing use the same channel as the other protocol-3 methods.

### Launch and reopen semantics

`executable` is a bare PATH command or absolute path; `arguments` stays an argv
array. Optional `cwd` is a workspace `{rootID,path}` directory resource and
cannot escape the workspace. `environment` is a map of string overrides/null
removals, not shell assignments. Never put credentials in titles, arguments or
logs. Input and output live in the native terminal; there is no public
`sendText`, terminal-output read or user-terminal enumeration API.

`close` removes the tab and stops the process, retaining the launch configuration
and a bounded plain-text suffix of the displayed output. `retainedContextBytes`
is a UTF-8 byte budget: default 16384, maximum 262144, 0 disables history. It may
be set on create or overridden at close. Actual retained bytes can be smaller
to keep Unicode intact. Displayed/echoed secrets can be in that history; choose
0 for such sessions. No raw escape sequence or stdin event is retained.

`reopen` uses a **new process** with the same executable/argv/cwd/environment,
restoring that suffix to the display before spawning. It never replays input,
recovers process memory or resumes shell variables. It keeps `terminalID` and
increases `generation`; events include generation so old exits cannot be
mistaken for the new process. The exit subscription continues across reopen.
Natural exit and user tab close are reopenable too.
Both paths finish bounded native cleanup, retain only the chosen snapshot,
and release the native session and render buffer before notifying subscribers.
The handle is busy during cleanup; an exit subscriber may reopen immediately
after receiving the event. A zero-byte budget keeps no history on any exit path.

`dispose` forgets the handle, launch settings and history, closing a live session
first. Eight handles per plugin (including closed ones) are retained, so dispose
handles that will not be reopened. A backend/session restart, plugin disable or
update, and closing/replacing the workspace also release them. Context and
history are in-memory only and never written to the installed package.

The Rust sample's `terminal.cycle` function and Core end-to-end scenario exercise
the protocol roundtrip. That scenario supplies host responses; the host's own
tests verify actual native PTY input and output/history.

The terminal lifecycle scenario runs from the Core repository root against the
working-tree Core:

```sh
node shared/plugins/scripts/sdk-e2e.mjs
```

The sample declares `terminal.create` in its installed manifest, so the test
uses the same grants an installed plugin receives. Older hosts without this
permission refuse that sample manifest; other plugins that do not declare it
retain their existing compatibility.
