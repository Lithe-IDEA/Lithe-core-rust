# 插件后端：远程工作区 / Plugin backend: remote workspaces

[中文](#中文) · [English](#english)

## 中文

工作区在远端主机上时，插件后端仍然运行在运行 Lithe 的这台机器上：每个插件只有一个后端进程，所有窗口和工作区共用。本页说明后端怎样认出远程上下文、怎样取得根目录在远端主机上的路径，以及现在哪些宿主服务能用于远程上下文。规则以[契约](../contract/v1/README.md#public-document-file-and-window-methods)为准；上下文的通用字段见[插件后端：工作区](plugin-backend-workspace.md)。

本页的 SDK 封装（`getHostPaths`、`processRoots` 和独立的远端路径类型）还没有发布，会随 Node.js、Java、Rust SDK 的下一个版本和 Go SDK 的下一个标签提供。

### 认出远程上下文

`listContexts`、`getContext` 返回的上下文里，`authority` 说明工作区在哪里：

```json
"authority": { "kind": "remote", "id": "c-7d41", "platform": { "os": "linux", "arch": "x86_64" } }
```

| 字段 | 含义 |
| --- | --- |
| `kind` | `local` 或 `remote` |
| `id` | 只有 `remote` 有；不透明的连接 ID，后端不要解析它 |
| `platform` | 只有 `remote` 有；远端主机的系统和 CPU 架构，取值同[受管运行时](../contract/v1/README.md#managed-runtimes)：`os` 为 `linux`，`arch` 为 `arm64` 或 `x86_64`。后端用它选择在远端运行的进程需要的包或运行时；遇到不认识的 `os`、`arch` 时忽略，因为以后的宿主可能连接其他系统 |

远程上下文的 `folders` 只有 `rootID` 和 `name`，没有 `path`。宿主在本机为远程工作区保留的目录只是空占位，不是工作区本身，后端不要读写它。

### 用 getHostPaths 取远端路径

根目录在远端主机上的路径不放在上下文里，要单独请求 `lithe/host/workspace/getHostPaths`。这样不需要它的后端看不到远端用户名和目录结构，远程工作区出现之前写的后端也不会把它当成本地路径。

| 请求 | 结果 |
| --- | --- |
| `{ context: { contextID, generation }, rootIDs?: [<rootID>] }` | `{ folders: [{ rootID, hostPath }] }` |

- 权限：manifest 声明 `workspace.metadata`。它是这个方法自己的授权，不随 `workspace` 服务给出；宿主不提供这个方法时，`getCapabilities` 里没有它。所以调用前先用 `supports("lithe/host/workspace/getHostPaths")` 探测。
- 不带 `rootIDs` 时返回全部根目录，顺序同 `getFolders`；带 `rootIDs` 时它必须是 1 到 256 个互不相同、符合键字母表（ASCII 字母、数字、`.`、`_`、`:`、`-`，1 到 128 个字符）的根 ID，结果按给出的顺序返回。SDK 在本地拒绝不合规的 `rootIDs`（`sdk.host.invalidParams`），不发请求。
- 失败：

| 代码 | 什么时候 | 后端怎么做 |
| --- | --- | --- |
| `plugin.host.authorityUnsupported` | 上下文是 `local` | 本地根目录已经带 `path`，直接用 |
| `plugin.host.notFound` | 上下文没有这个根 ID | 重新读取 `getFolders` |
| `plugin.host.contextExpired` | 上下文已关闭或 `generation` 变了 | 之前取得的路径全部作废，重新解析原来的目标，不要改用当前聚焦的窗口 |
| `plugin.host.permissionDenied` | 没有这个方法的授权：manifest 没有声明 `workspace.metadata`，或宿主不提供这个方法 | 补上权限；宿主不提供时 `supports` 为 false，不要调用 |
| `plugin.host.unsupportedOption` | 带了契约没有的参数 | `details.option` 指出是哪个参数 |

### path 与 hostPath

`hostPath` 是远端主机上的 POSIX 绝对路径，只用于在远端主机上运行的东西：进程参数和工作目录、调试配置的 `program` 和 `cwd`、语言服务器的 `initializationOptions`。它在运行 Lithe 的这台机器上不对应任何文件，后端永远不在本地读写它；工作区文件按 `{ rootID, path }` 通过文件服务读取。

为了不把两者混用，官方 SDK 给 `hostPath` 一个单独的类型，不能直接交给本地文件或进程接口：

| SDK | 远端路径类型 | 取出字符串 |
| --- | --- | --- |
| Node.js | `HostPath`（不是字符串，`fs`、`path` 会拒绝它） | `.value` |
| Go | `lithe.HostPath` | `.String()` |
| Rust | `HostPath`（不能转换成 `Path`） | `.as_str()` |
| Java | `Host.HostPath`（不是 `String`） | `.value()` |

写进 JSON 时，四个 SDK 都把它写成那个字符串。

后端要为每个根目录决定进程用哪个路径时，用 `processRoots`。它接收 `listContexts` 或 `getContext` 返回的完整上下文：`local` 上下文直接返回各根目录的 `path`，不请求宿主；其他 authority 调用 `getHostPaths` 返回 `hostPath`。只传 `{ contextID, generation }` 时看不出 authority，SDK 在本地失败为 `sdk.host.invalidContext`。

### 远程上下文现在能用哪些服务

| 服务 | 对远程上下文 |
| --- | --- |
| `listContexts`、`getContext`、`getFolders` | 可用，返回上面的 `authority` 和不带 `path` 的根目录 |
| `getHostPaths` | 契约已定义；Lithe 桌面端还没有注册，`supports` 返回 false |
| 工作区文件、搜索、监听（`readDirectory`、`stat`、`readFile`、`findFiles`、`findText`、`watch`） | `plugin.host.authorityUnsupported`；宿主不会读取本地占位目录，也不会改在本地执行 |
| 原生终端（`terminal/create`、`reopen`） | 首个实现只支持本地，远程上下文为 `plugin.host.authorityUnsupported`，不会在本地启动任何东西 |
| 插件函数中转（`functions/invoke`） | 和本地上下文一样转发；被调用的插件用 `getContext` 得知 authority，中转不读也不改参数和结果里的路径 |

契约目前没有定义在远端主机上启动插件进程（语言服务器、构建、运行、调试）的规则。后端不要假定这些进程会在远端运行，也不要在本地替远程工作区运行它们。

### 两个与远端有关的错误码

| 代码 | 数值 | 含义 | 后端怎么做 |
| --- | --- | --- | --- |
| `plugin.host.remoteDisconnected` | `-32078` | 操作进行中和远端主机断开，不知道它是否已在远端生效 | 上下文恢复后先重新读取状态再决定，不要自动重放修改 |
| `plugin.host.platformUnavailable` | `-32079` | 插件没有适用于远端 `platform` 的包或受管运行时，请求需要的进程不能在那里运行 | 告诉用户缺少哪个平台；不要退回到在本地运行 |

### Node.js 示例

```js
const host = context.host;
const workspace = context.workspaceContext;
if (workspace && await host.supports('lithe/host/workspace/getHostPaths')) {
  const full = await host.workspace.getContext(workspace);
  const { folders } = await host.workspace.processRoots(full);
  for (const root of folders) {
    // 本地根目录有 path；远程根目录有 hostPath（HostPath）。
    const cwd = root.path ?? root.hostPath.value;
    // ...
  }
  // 只要部分根目录：
  const some = await host.workspace.getHostPaths(workspace, { rootIDs: [full.folders[0].rootID] });
}
```

### Go 示例

```go
full, err := host.Workspace().GetContext(*workspace)
if err != nil {
	return nil, err
}
roots, err := host.Workspace().ProcessRoots(full)
if err != nil {
	return nil, err
}
for _, root := range roots {
	cwd := root.Path
	if cwd == "" {
		cwd = root.HostPath.String()
	}
	// ...
}
// 只要部分根目录：host.Workspace().GetHostPaths(*workspace, rootID)
```

### Rust 示例

```rust
use lithe_plugin_sdk::ProcessRoot;

let full = host.workspace().get_context(&workspace)?;
for root in host.workspace().process_roots(&full)? {
    match root {
        ProcessRoot::Local { root_id, path } => { /* 本机路径 */ }
        ProcessRoot::Remote { root_id, host_path } => { /* 远端路径：host_path.as_str() */ }
    }
}
// 全部根目录：host.workspace().get_host_paths(&workspace, None)
// 部分根目录：host.workspace().get_host_paths(&workspace, Some(&[root_id]))
```

### Java 示例

```java
Map<String, Object> full = context.host().workspace().getContext(workspace).get();
for (Host.ProcessRoot root : context.host().workspace().processRoots(full).get()) {
    if (root.isRemote()) {
        Host.HostPath hostPath = root.hostPath(); // 远端路径：hostPath.value()
    } else {
        String path = root.path(); // 本机路径
    }
}
// 全部根目录：workspace().getHostPaths(context)
// 部分根目录：workspace().getHostPaths(context, Arrays.asList(rootId))
```

## English

When a workspace is on a remote host, the plugin backend still runs on the machine that runs Lithe: a plugin has one backend process, shared by every window and workspace. This page says how a backend recognises a remote context, how it gets the paths of the roots on the remote host, and which host services serve a remote context today. The [contract](../contract/v1/README.md#public-document-file-and-window-methods) is authoritative; the common fields of a context are in [Plugin backend: workspace](plugin-backend-workspace.md).

The SDK wrappers on this page (`getHostPaths`, `processRoots`, and a type of its own for remote paths) are not released yet. They ship with the next release of the Node.js, Java, and Rust SDKs and the next tag of the Go SDK.

### Recognising a remote context

In a context from `listContexts` or `getContext`, `authority` says where the workspace is:

```json
"authority": { "kind": "remote", "id": "c-7d41", "platform": { "os": "linux", "arch": "x86_64" } }
```

| Field | Meaning |
| --- | --- |
| `kind` | `local` or `remote` |
| `id` | `remote` only; an opaque connection ID that a backend does not parse |
| `platform` | `remote` only; the remote host's operating system and CPU architecture, with the values of [managed runtimes](../contract/v1/README.md#managed-runtimes): `os` is `linux` and `arch` is `arm64` or `x86_64`. A backend uses it to choose the package or runtime a process on the host needs, and ignores an `os` or `arch` it does not know, as a later host may connect to other systems |

The `folders` of a remote context have `rootID` and `name` but no `path`. The local directory the host keeps for a remote workspace is an empty placeholder, not the workspace, and a backend neither reads nor writes it.

### Remote paths with getHostPaths

The paths of the roots on the remote host are not part of the context; a backend asks for them with `lithe/host/workspace/getHostPaths`. A backend that does not need them never sees the remote user name or directory layout, and a backend written before remote workspaces cannot take one for a local path.

| Request | Result |
| --- | --- |
| `{ context: { contextID, generation }, rootIDs?: [<rootID>] }` | `{ folders: [{ rootID, hostPath }] }` |

- Permission: the manifest declares `workspace.metadata`. It grants this method on its own, not with the `workspace` service, and a host that does not serve the method leaves it out of `getCapabilities`. Probe `supports("lithe/host/workspace/getHostPaths")` before calling it.
- Without `rootIDs`, `folders` holds every root in `getFolders` order. With it, `rootIDs` is 1 to 256 distinct root IDs in the key alphabet (ASCII letters, digits, `.`, `_`, `:`, `-`; 1 to 128 characters), and `folders` holds those roots in that order. The SDKs refuse other `rootIDs` locally with `sdk.host.invalidParams` and send nothing.
- Failures:

| Code | When | What the backend does |
| --- | --- | --- |
| `plugin.host.authorityUnsupported` | The context is `local` | Use the `path` its roots already carry |
| `plugin.host.notFound` | The context has no such root ID | Read `getFolders` again |
| `plugin.host.contextExpired` | The context closed, or its `generation` changed | Every path obtained before is void; resolve the original target again, never the focused window |
| `plugin.host.permissionDenied` | No grant for the method: the manifest does not declare `workspace.metadata`, or the host does not serve the method | Declare the permission; when the host does not serve it, `supports` is false, so do not call it |
| `plugin.host.unsupportedOption` | A param the contract does not have | `details.option` names it |

### path and hostPath

`hostPath` is an absolute POSIX path on the remote host. A backend uses it only in what runs on that host: process arguments and working directories, a debug configuration's `program` and `cwd`, a language server's `initializationOptions`. It names no file on the machine that runs Lithe, so a backend never reads or writes it locally; files of the workspace are read through the file services by `{ rootID, path }`.

So that the two are not mixed up, the official SDKs give `hostPath` a type of its own that local file and process APIs do not take:

| SDK | Remote path type | As a string |
| --- | --- | --- |
| Node.js | `HostPath` (not a string; `fs` and `path` refuse it) | `.value` |
| Go | `lithe.HostPath` | `.String()` |
| Rust | `HostPath` (converts to no `Path`) | `.as_str()` |
| Java | `Host.HostPath` (not a `String`) | `.value()` |

All four SDKs write it into JSON as that string.

When a backend needs, for each root, the path a process uses, it calls `processRoots` with a full context from `listContexts` or `getContext`. For a `local` context it returns the roots' `path` without asking the host; for any other authority it calls `getHostPaths` and returns `hostPath`. A bare `{ contextID, generation }` does not say the authority, and the SDK fails it locally with `sdk.host.invalidContext`.

### Which services serve a remote context today

| Service | For a remote context |
| --- | --- |
| `listContexts`, `getContext`, `getFolders` | Served, with the `authority` above and roots without `path` |
| `getHostPaths` | Defined by the contract; the Lithe desktop does not register it yet, so `supports` is false |
| Workspace files, search, and watching (`readDirectory`, `stat`, `readFile`, `findFiles`, `findText`, `watch`) | `plugin.host.authorityUnsupported`; the host never reads the local placeholder or runs the operation locally instead |
| Native terminals (`terminal/create`, `reopen`) | The first release is local only; a remote context is `plugin.host.authorityUnsupported`, and nothing starts locally |
| The function relay (`functions/invoke`) | Relayed as for a local context; the callee learns the authority from `getContext`, and the relay neither reads nor rewrites paths in the params or the result |

The contract does not yet define how a plugin's processes (language servers, builds, runs, debugging) start on a remote host. A backend does not assume they run there, and does not run them locally for a remote workspace instead.

### Two codes for remote hosts

| Code | Number | Meaning | What the backend does |
| --- | --- | --- | --- |
| `plugin.host.remoteDisconnected` | `-32078` | The connection to the remote host was lost while the operation ran; whether it took effect on the host is unknown | Once the context is current again, read the state before deciding; never replay a change on its own |
| `plugin.host.platformUnavailable` | `-32079` | The plugin has no package or managed runtime for the remote host's `platform`, so the process the request needs cannot run there | Tell the user which platform is missing; never fall back to running locally |

### Node.js

```js
const host = context.host;
const workspace = context.workspaceContext;
if (workspace && await host.supports('lithe/host/workspace/getHostPaths')) {
  const full = await host.workspace.getContext(workspace);
  const { folders } = await host.workspace.processRoots(full);
  for (const root of folders) {
    // A local root has `path`; a remote one has `hostPath`, a HostPath.
    const cwd = root.path ?? root.hostPath.value;
    // ...
  }
  // Some roots only:
  const some = await host.workspace.getHostPaths(workspace, { rootIDs: [full.folders[0].rootID] });
}
```

### Go

```go
full, err := host.Workspace().GetContext(*workspace)
if err != nil {
	return nil, err
}
roots, err := host.Workspace().ProcessRoots(full)
if err != nil {
	return nil, err
}
for _, root := range roots {
	cwd := root.Path
	if cwd == "" {
		cwd = root.HostPath.String()
	}
	// ...
}
// Some roots only: host.Workspace().GetHostPaths(*workspace, rootID)
```

### Rust

```rust
use lithe_plugin_sdk::ProcessRoot;

let full = host.workspace().get_context(&workspace)?;
for root in host.workspace().process_roots(&full)? {
    match root {
        ProcessRoot::Local { root_id, path } => { /* a path on this machine */ }
        ProcessRoot::Remote { root_id, host_path } => { /* on the remote host: host_path.as_str() */ }
    }
}
// Every root: host.workspace().get_host_paths(&workspace, None)
// Some roots: host.workspace().get_host_paths(&workspace, Some(&[root_id]))
```

### Java

```java
Map<String, Object> full = context.host().workspace().getContext(workspace).get();
for (Host.ProcessRoot root : context.host().workspace().processRoots(full).get()) {
    if (root.isRemote()) {
        Host.HostPath hostPath = root.hostPath(); // on the remote host: hostPath.value()
    } else {
        String path = root.path(); // a path on this machine
    }
}
// Every root: workspace().getHostPaths(context)
// Some roots: workspace().getHostPaths(context, Arrays.asList(rootId))
```
