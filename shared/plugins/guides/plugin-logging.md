# 插件接入统一日志 / Plugin logging

[中文](#中文) · [English](#english)

## 中文

插件使用 `litheLogging` 0 与 `lithe/log` 发送受限的操作事实，由宿主的共享事件目录、脱敏、队列和文件写入器落盘。三个 SDK 不提供自由文本 `log` 接口，也没有 stderr 回退。插件无需实现日志文件格式、文件轮转、脱敏器或后台写入线程。

宿主必须在 initialize 中提供 `capabilities.experimental.litheLogging.versions: [0]`，SDK 回复 `capabilities.experimental.litheLogging.version: 0`。缺少协议时，SDK 拒绝初始化；不自动退回其他日志方式。初始化尚未协商时的失败只通过原有 RPC 错误与宿主进程生命周期记录，不能提前发送未协商的日志通知。

本文的 API 由 Node.js SDK 0.5.0、Java SDK 0.5.0 和 Rust SDK 0.4.0 提供，发布标签分别为 `sdk-node-v0.5.0`、`sdk-java-v0.5.0` 和 `sdk-rust-v0.4.0`。插件在构建输入中锁定对应的 release tag 与 SHA-256，不要引用开发机的绝对 path 依赖。这些 SDK 要求宿主提供 `litheLogging` 0；在提供它的 Lithe 稳定版发布之前，用这些 SDK 构建的插件把 GitHub Release 标为预发布。

### 每种事实与级别

| `kind` | 目录事件 | 级别与用途 |
| --- | --- | --- |
| `operationFinished` | `plugins.operation.finished` | INFO，关键用户操作的真实成功终态 |
| `operationFailed` | `plugins.operation.failed` | ERROR，影响请求结果的失败；SDK 在拥有结果的边界发送一次 |
| `degraded` | `plugins.operation.fallback_selected` | WARN，已处理并选择回退的失败 |
| `recovered` | `plugins.operation.recovery_finished` | WARN，曾失败后的真实恢复 |

`operationFinished` 和 `operationFailed` 必须有单调时钟计算的 `durationMs`。失败、降级、恢复使用受控的 `code`；成功没有 `code`。可选 `count` 与 `durationMs` 都是 0…9007199254740991 的整数。API 不接受原始 message、stack、request、URL、SQL、命令输出或字段字典。

`operation` 只接受本 SDK 已注册的函数 ID 或封闭协议操作，超过 64 字节的已注册函数 ID 统一为 `function-call`。公开的协议操作包括 initialize/shutdown/notification/function-call/result-encoding、SDK 已知的 `lithe/build/*` 与 `lithe/vcs/*` 方法，以及快捷键导入请求 `lithe/keymap/import`（三种 SDK 自动上报其终态）；任意用户文本即使字符合法也不接受。`code` 是最长 64 字节的 ASCII 类别标识，不是错误正文。

通道由宿主从已校验的安装插件 ID 得到 `plugins/<plugin>/host`，插件不能更改通道或拼接路径。上下文诊断优先携带宿主原始 `operationID`，缺少合法值时沿用当前请求原始 JSON-RPC ID（安全字符串或整数的十进制字符串），wire 名为 `operationId`，只保留最长 128 字节的受控 ASCII 标识；无真实身份时省略。通知没有请求 ID，不生成回退身份。后台共享任务可以使用 backend/handle 的无 op 诊断，不能伪造一个操作 ID。

### 三个 SDK 的用法

```js
backend.function('project.refresh', async (params, context) => {
  // 只在决定继续执行的真实降级分支调用，不把异常正文传进日志。
  context.diagnostics.degraded('project.refresh', 'cache_unavailable', 1);
  return refreshWithoutCache(params);
});
// 高频读取或轮询不产生 INFO 成功噪声，失败仍自动记录。
backend.function('project.inspect', (params) => inspectProject(params));
backend.suppressSuccessDiagnostics('project.inspect');
backend.diagnostics.recovered('project.refresh', 'cache_restored');
```

```java
context.diagnostics().degraded("lithe/build/model", "snapshot_unavailable", 1L);
backend.diagnostics().recovered("lithe/build/model", "snapshot_restored");
backend.suppressSuccessDiagnostics("lithe/build/dependencies");
```

```rust
context.diagnostics().degraded("query.run", "rows_truncated", Some(1))?;
handle.diagnostics().recovered("query.run", "connection_restored", None)?;
backend.suppress_success_diagnostics("query.inspect")?;
```

`degraded`、`recovered` 的 operation 必须已注册，静态示例需要放到实际的注册完成之后。Rust 的返回错误只表达调用参数非法；协议写入复用现有 writer，诊断传输失败不会把已经成功的用户操作改成失败。没有额外 writer 或后台队列。

混合用户操作和高频读取的单个入口可以在当前调用上使用 `context.suppressSuccessDiagnostics()`（Rust 为 `context.suppress_success_diagnostics()`）；该决定只影响本次成功事实，不影响其他调用与失败。

SDK 在函数与受管 build/VCS 请求的结果边界自动发送一次终态，通知在 RPC 响应之前发送，宿主才能关联仍活动的请求。不要在处理函数里先发送失败再返回或抛出同一次失败。取消不产生失败事件，正常缺失的收尾对象不应产生 WARN。高频只读、预览、统计与轮询通过 `suppressSuccessDiagnostics` / `suppress_success_diagnostics` 关闭成功事实；失败与真正降级仍保留。

Rust SDK 用一次安装的 panic hook 包装器，仅在线程处于 SDK 拥有的 catch 边界时屏蔽原始 payload 输出，随后发送固定 `panic` 类别。其他线程和边界外的 panic 继续执行原 hook。不要把用户内容放进 panic，插件也不要在 SDK 启动后替换该 hook 来恢复正文输出。

### 上游诊断与文件位置

上游语言服务器的 stderr、`window/logMessage` 继续由宿主接收；它们不是插件自己的结构化日志入口。插件不得把源码、协议载荷、SQL、HTTP body 或构建输出复制进上游诊断，以为脱敏与截断能使其安全。控制台输出走已有 Build、Run、Test、VCS 或插件界面结果。

日志窗口与导出的平台支持以 Lithe 桌面端的实际实现为准。Core 接收目录事件，不等于每个平台的日志窗口都已经展示它；只有在对应目标平台执行工作流后才能声明该平台已验证。

只有上游工具确实只能写文件时，才使用 `${logDir}`。宿主显式提供当前会话的 `plugins/<id>/files/`，通过可写存储适配器创建，不能从插件安装路径或 app bundle 推导：

- 日志目录只属于当前会话，不保存数据库、索引、下载或工作区状态，不跨工作树共享。
- 缓存和状态继续使用 `${stateDir}`、`${workspaceStateDir}`。
- 日志存储不可用时，宿主不提供该变量。远程进程不能使用本机日志目录；引用 `${logDir}` 却没有绝对 `logDirectory` 的启动计划会失败。
- 不需要原生文件的插件不要增加这个启动依赖。先核对目标宿主和平台确实支持，再修改启动模板。
- 原生文件不会自动变成时间线事件，也不能把整个文件打印进 stderr。
- 已发布的应用包、插件包和安装目录运行时只读；日志与锁文件不得改变代码签名或增量更新的基线。

### 最小验证

1. 用已有有界协议测试确认 logging 协商、四类事实、级别、原始操作关联、成功抑制、取消和非法参数拒绝。
2. 触发真实失败，确认原有用户错误码与提示保持不变，诊断只有固定类别，不含请求正文、原始异常或堆栈。
3. 使用真实 Core 查询 `diagnostics.drainCoreEvents`，核对目录事件与 `op`。
4. 在目标宿主中安装并执行对应工作流，核对日志窗口。SDK 组件通过不能替代产品验收。
5. 原生日志位置有变化时比较工作流前后安装目录清单或哈希，确认发行资源不变；结束后清理本轮进程。

---

## English

Plugins use `litheLogging` 0 and `lithe/log` to send restricted operation facts. The host's shared event catalog, redaction, queue, and file writer persist them. The three SDKs offer no free-text `log` interface and have no stderr fallback. A plugin does not implement the log file format, file rotation, a redactor, or a background writer thread.

The host must offer `capabilities.experimental.litheLogging.versions: [0]` in initialize, and the SDK replies with `capabilities.experimental.litheLogging.version: 0`. When the protocol is missing, the SDK refuses to initialize; it does not fall back to another logging path. A failure before initialization has negotiated logging is recorded only through the existing RPC error and the host's process lifecycle; the plugin must not send a log notification before negotiation.

The APIs in this guide are provided by Node.js SDK 0.5.0, Java SDK 0.5.0, and Rust SDK 0.4.0, with release tags `sdk-node-v0.5.0`, `sdk-java-v0.5.0`, and `sdk-rust-v0.4.0`. Pin the matching release tag and SHA-256 in your build inputs; do not depend on an absolute path on a development machine. These SDKs require the host to offer `litheLogging` 0; until a stable Lithe release offers it, mark the GitHub Release of a plugin built with these SDKs as a prerelease.

### Facts and levels

| `kind` | Catalog event | Level and use |
| --- | --- | --- |
| `operationFinished` | `plugins.operation.finished` | INFO, the real successful end of a key user operation |
| `operationFailed` | `plugins.operation.failed` | ERROR, a failure that affects the request's result; the SDK sends it once at the boundary that owns the result |
| `degraded` | `plugins.operation.fallback_selected` | WARN, a failure that was handled by choosing a fallback |
| `recovered` | `plugins.operation.recovery_finished` | WARN, a real recovery after an earlier failure |

`operationFinished` and `operationFailed` must carry a `durationMs` measured with a monotonic clock. Failure, degradation, and recovery use a controlled `code`; success has no `code`. The optional `count` and `durationMs` are integers from 0 to 9007199254740991. The API does not accept raw messages, stacks, requests, URLs, SQL, command output, or field dictionaries.

`operation` accepts only function IDs registered with this SDK or closed protocol operations; a registered function ID longer than 64 bytes is reported as `function-call`. The public protocol operations are initialize, shutdown, notification, function-call, and result-encoding, the `lithe/build/*` and `lithe/vcs/*` methods the SDK knows, and the shortcut import request `lithe/keymap/import` (all three SDKs report its end state automatically). Arbitrary user text is not accepted even when its characters are valid. `code` is an ASCII category identifier of at most 64 bytes, not an error message.

The host derives the channel `plugins/<plugin>/host` from the validated installed plugin ID; the plugin cannot change the channel or build a path. Contextual diagnostics carry the host's original `operationID` first; without a valid one they reuse the current request's original JSON-RPC ID (a safe string, or an integer as a decimal string). The wire name is `operationId`, and only a controlled ASCII identifier of at most 128 bytes is kept; it is omitted when there is no real identity. Notifications have no request ID, and no fallback identity is generated for them. Shared background work can use the backend or handle diagnostics without an operation, but must not invent an operation ID.

### Using the three SDKs

```js
backend.function('project.refresh', async (params, context) => {
  // Call only in a real degraded branch that decides to continue; never pass exception text to the log.
  context.diagnostics.degraded('project.refresh', 'cache_unavailable', 1);
  return refreshWithoutCache(params);
});
// High-frequency reads or polling produce no INFO success noise; failures are still recorded automatically.
backend.function('project.inspect', (params) => inspectProject(params));
backend.suppressSuccessDiagnostics('project.inspect');
backend.diagnostics.recovered('project.refresh', 'cache_restored');
```

```java
context.diagnostics().degraded("lithe/build/model", "snapshot_unavailable", 1L);
backend.diagnostics().recovered("lithe/build/model", "snapshot_restored");
backend.suppressSuccessDiagnostics("lithe/build/dependencies");
```

```rust
context.diagnostics().degraded("query.run", "rows_truncated", Some(1))?;
handle.diagnostics().recovered("query.run", "connection_restored", None)?;
backend.suppress_success_diagnostics("query.inspect")?;
```

The operation passed to `degraded` and `recovered` must be registered, so put static examples after registration has actually finished. A Rust error return means only that the call's arguments are invalid. Protocol writes reuse the existing writer, and a failed diagnostic send never turns a successful user operation into a failure. There is no extra writer or background queue.

A single entry point that mixes user operations with high-frequency reads can call `context.suppressSuccessDiagnostics()` (in Rust, `context.suppress_success_diagnostics()`) for the current call. That decision affects only this call's success fact, not other calls or failures.

The SDK sends one end state automatically at the result boundary of functions and managed build and VCS requests. The notification goes out before the RPC response so the host can relate it to the still-active request. Do not send a failure from a handler and then return or throw the same failure. Cancellation produces no failure event, and a normally absent cleanup object should not produce a WARN. High-frequency reads, previews, statistics, and polling turn off success facts with `suppressSuccessDiagnostics` / `suppress_success_diagnostics`; failures and real degradations are kept.

The Rust SDK installs a panic hook wrapper once. It suppresses the raw payload output only while the thread is inside a catch boundary the SDK owns, and then sends the fixed `panic` category. Panics on other threads and outside the boundary still run the original hook. Do not put user content in a panic, and do not replace the hook after the SDK starts to restore message output.

### Upstream diagnostics and file locations

The host still receives an upstream language server's stderr and `window/logMessage`; they are not the plugin's own structured logging path. A plugin must not copy source code, protocol payloads, SQL, HTTP bodies, or build output into upstream diagnostics in the belief that redaction and truncation make them safe. Console output goes through the existing Build, Run, Test, VCS, or plugin UI results.

Log window and export support follows the Lithe desktop app's actual implementation. Core accepting catalog events does not mean every platform's log window already shows them. Declare a platform verified only after running the workflow on that target platform.

Use `${logDir}` only when an upstream tool really can write only to files. The host explicitly provides the current session's `plugins/<id>/files/`, created through the writable storage adapter; it is never derived from the plugin's install path or the app bundle:

- The log directory belongs only to the current session. It holds no databases, indexes, downloads, or workspace state and is not shared across worktrees.
- Caches and state keep using `${stateDir}` and `${workspaceStateDir}`.
- When log storage is unavailable, the host does not provide the variable. Remote processes cannot use a local log directory; a launch plan that references `${logDir}` without an absolute `logDirectory` fails.
- A plugin that needs no native files should not add this launch dependency. Check that the target host and platform support it before changing the launch template.
- Native files do not become timeline events automatically, and the whole file must not be printed to stderr instead.
- Published app bundles, plugin packages, and install directories are read-only at run time; logs and lock files must not change the code signature or the incremental-update baseline.

### Minimum verification

1. Use the existing bounded protocol tests to confirm logging negotiation, the four facts, levels, original operation correlation, success suppression, cancellation, and rejection of invalid arguments.
2. Trigger a real failure and confirm that the existing user error code and message are unchanged and that diagnostics contain only fixed categories, with no request body, raw exception, or stack.
3. Query `diagnostics.drainCoreEvents` against a real Core and check the catalog events and `op`.
4. Install the plugin in the target host, run the matching workflow, and check the log window. Passing SDK component tests does not replace product acceptance.
5. When the native log location changes, compare the install directory listing or hashes before and after the workflow to confirm the release resources are unchanged, and clean up this run's processes afterwards.
