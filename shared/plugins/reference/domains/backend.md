# 插件后端与插件函数 / Plugin backend and plugin functions

[中文](#中文) · [English](#english)

## 中文

插件后端是插件自己的长驻进程，用来运行网页做不到的事：数据库驱动、网络客户端、文件格式解析、调用宿主服务。它只服务插件自己，不属于任何构建系统、语言或工作区，也不回答 `lithe/build/*`、VCS 等领域请求。后端与宿主之间说插件函数协议（`litheFunctions`），可以按权限请求宿主服务，可以启动由宿主拥有的 Run 会话和调试会话，也可以经 Core 调用其他插件导出的函数。

> **宿主支持。** Lithe 桌面端声明 `backend` 和 `pluginFunctions`（版本都是 `0`）。一次会话能调用哪些宿主方法，以运行时 `lithe/host/getCapabilities` 返回的 `services` 为准，不要按本页假定。
> 桌面端的 `Declared()` 注释列出了已注册的后端宿主服务：工作区上下文（`workspace/listContexts`、`getContext`、`getFolders` 和 `workspace/didChangeContexts`）、`subscriptions/dispose`、`languageServer/*`、跨插件调用中转、`credentials/*`、`documents/list` 与 `getSnapshot`、`editor/getActive`、`getSelections`、`open`、`reveal`、`insertText`、`replaceSelections`、`applyEdits`、`window/*`、`browser/openExternal`（只接受 HTTP(S) 地址）、`terminal/*` 和 `documents/registerContentProvider`；与窗口绑定的方法在窗口附着后才注册。
> 工作区文件与搜索（`workspace/readDirectory`、`stat`、`readFile`、`watch`、`findFiles`、`findText`）、`preview/openHTML`、`editor/registerActions` 和 `editor/registerProviders` 没有注册，申请对应权限的插件会被拒绝。Run 与调试会话方法（`run/start`、`run/stop`、`debug/start`）是否可用，同样以 `getCapabilities` 为准。
> 说明见 [`pluginhost.go`](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go) 中 `Declared()` 上方的注释；各能力的对照见[能力与协议版本对照](../capabilities-and-versions.md)。
> 对契约有疑问，请到 [Lithe-core-rust 的 Issues](https://github.com/Lithe-IDEA/Lithe-core-rust/issues) 反馈。

| 项目 | 内容 |
| --- | --- |
| 清单字段 | `backend`（与[启动进程](../../contract/v1/README.md#process-start)的字段相同，`launch` 加 `arguments` 等）；跨插件调用另有 `pluginFunctions` 段（`exports`、`calls`） |
| 能力版本 | `backend` `0`；`ui` 的后端命令需要同时声明 `ui: 0` 和 `backend: 0`；跨插件调用需要 `pluginFunctions: 0` 和 `backend: 0`。`requires` 必须恰好列出清单用到的能力；启动进程是基础契约，不是能力，不写 `requires.serverProcess`。见[版本对照](../capabilities-and-versions.md#能力版本requires) |
| 协议 | `litheFunctions` `0`（版本就是 `backend` 的版本）、`litheLogging` `0`，见[进程协议版本](../capabilities-and-versions.md#进程协议版本) |
| API 编号 | API-01、04、05、06、07、08、20、21、22、23、24、35、37、40、41，见 [API 总表](../api-catalog.md#现在能用的-api) |
| 权限 | `backend.launch`；宿主服务按方法另加权限（`credentials.store`、`workspace.metadata`、`workspace.files.read`、`workspace.files.watch`、`workspace.search`、`terminal.create`、`run.start` 等），见[已公开的宿主方法](../api-catalog.md#已公开的宿主方法)。跨插件调用没有清单权限：调用方的 `calls` 和被调方的 `exports` 就是双方的同意 |
| 激活事件 | 至少要有一种启动方式：网页工具窗口第一次调用 `functions.*`；`ui.commands[]` 的命令（`onCommand:<id>`）；可选的 `onWorkspaceOpen`；或宿主查询的提供者（工具链或快捷键导入提供者，以及 `launch.kind: "backend"` 的语言服务器）。都没有是 `plugin.manifest.invalid`。只导出函数的插件不需要网页、命令或 `onWorkspaceOpen`，被调用时启动后端；只调用的插件仍要有其中一种 |
| 官方 SDK | [Node.js 0.5.0](../../sdk/node/README.md)、[Java 0.5.0](../../sdk/java/README.md)、[Rust 0.4.0](../../sdk/rust/README.md)，发布标签 `sdk-node-v0.5.0`、`sdk-java-v0.5.0`、`sdk-rust-v0.4.0`，见[官方 SDK](../capabilities-and-versions.md#官方-sdk)。三个 SDK 都实现函数协议，封装凭据、工作区、文档与编辑器（含 `editor/registerActions`）、预览、浏览器、窗口消息、`functions/providers` 和 `functions/invoke`，被调函数能读到 `caller`，用法见各 SDK 的 README；`parentOperationID` 不传时，SDK 取正在处理的 `lithe/function/invoke` 的 `operationID`。`run/*`、`debug/start`、`languageServer/*`、`documents/registerContentProvider` 和 `editor/registerProviders` 没有专门的封装，用 SDK 的通用宿主请求（`request`）调用。原生终端有封装，见[从插件打开原生终端](../../guides/native-terminals.md) |

### 后端的启动与工作区

- 每个插件最多一个后端进程，由所有打开的工作区和窗口共享，运行在运行 Lithe 的机器上，远程工作区也一样。工作区不是进程状态：每次调用都显式带工作区上下文，所以 `${workspaceStateDir}` 被拒绝；可写状态是 `${stateDir}`，宿主以它为根目录和工作目录启动后端。
- 宿主在上面“激活事件”一行的第一个触发点启动后端；插件被停用、更新、卸载，或宿主退出时结束它。同意来自安装审查，不需要按工作区弹确认；审查里它显示为 `packagedBackend`、`systemCommandBackend` 或 `runtimeBackend`。
- `onWorkspaceOpen` 不会拖慢打开工作区。宿主在窗口显示后一次启动一个这样的后端；在初始化时限内没有完成 `initialize` 的后端被停止，按普通的服务器启动失败处理。失败或被停用的后端从不阻塞工作区。
- `workspace.metadata` 权限授予 `workspace` 宿主服务：开放工作区的名称、根目录和 ID、authority 以及插件在该工作区的状态目录；它不读任何文件内容。
- 激活只取决于插件已安装、已启用且已审查。Lithe 目前没有工作区信任状态：会运行或修改内容的操作，像宿主自己的控件一样按操作询问用户。
- 插件按平台发布一个包，所以包内路径不随 `${arch}` 变化；macOS 的包可以带通用二进制。

### 协议要点

| 项目 | 规则 |
| --- | --- |
| 协商 | 对插件拥有的会话，宿主在 `initialize` 的 `capabilities.experimental.litheFunctions` 提供 `{"versions": [0]}`，与 `litheBuild` 互不依赖；后端回答 `{"version": 0, "functions": [{"id": "…"}]}`。回答了宿主没给出的版本，或函数 ID 格式错误、重复，整个协议不启用 |
| 函数 ID | 以点分隔的小驼峰段（`snapshot.refresh`），不超过 128 字节，会话内唯一，每个会话最多 256 个函数 |
| 注册与撤回 | 初始化之后 `lithe/function/register`、`unregister`，整个请求要么全成功要么全失败（`-32602`、`plugin.function.invalidRegistration`） |
| 调用 | 宿主发 `lithe/function/invoke`，顶层有 `functionID`、`operationID`、`context`、`params`（JSON 对象，默认 `{}`）；`operationID` 由 Core 分配，进度通知必须原样带回 |
| 结果 | 结果，或 `plugin.function.failed`（后端的消息最多 1024 字节，`pluginErrorCode` 来自 `error.data.code`），或 `plugin.function.invalidResult`（没有 `result`，或序列化后超过 1 MiB）；取消和超时由宿主发 `$/cancelRequest` |
| 进度 | `lithe/function/progress {operationID, message?, percentage?}`，`message` 截到 1024 字节，`percentage` 为 0–100 的整数 |
| 禁止回环 | 后端不能请求宿主分发领域操作或函数：对 `lithe/function/invoke` 或任何 `lithe/build/*` 的请求以 `plugin.function.recursiveCall` 拒绝。跨插件调用是另一种请求，见下文 |
| 私有函数 | 不需要清单字段，增删它们不改变任何版本；只有可被其他插件调用的函数和对它的调用才声明在 `pluginFunctions` |
| 不回答协议 | 不回答 `litheFunctions` 的后端（例如只做构建的后端）照常服务它的语言和 `litheBuild` 请求，收不到函数调用，它的函数请求得到 `-32601` |

### 宿主服务

后端向宿主发请求，前缀 `lithe/host/`；每个服务由一个清单权限授予，Core 在宿主看到请求之前检查。完整的方法、权限和限制见 [API 总表](../api-catalog.md#已公开的宿主方法)。

| 服务 | 方法 | 权限 |
| --- | --- | --- |
| 能力发现 | `getCapabilities`、`subscriptions/dispose` | 每个插件会话都有 |
| 凭据 | `credentials/get`、`set`、`delete` | `credentials.store` |
| 工作区上下文 | `workspace/listContexts`、`getContext`、`getFolders` | `workspace.metadata` |
| 文件与搜索 | `workspace/readDirectory`、`stat`、`readFile`、`watch`、`findFiles`、`findText` | `workspace.files.read`、`workspace.files.watch`、`workspace.search` |
| 文档与编辑器 | `documents/*`、`editor/*`，见[编辑器](editor.md) | `documents.read`、`editor.edit`、`editor.open`、`editor.actions`、`editor.providers` |
| 预览、浏览器、窗口消息 | `preview/openHTML`、`browser/openExternal`、`window/*` | `preview.open`、`browser.open`、`window.messages` |
| 原生终端 | `terminal/*` | `terminal.create` |
| Run 与调试会话 | `run/start`、`run/stop`、`debug/start` | `run.start`、`debug.start` |
| 语言服务器 | `languageServer/request`、`notify`、`setStatus` | `languageServer.request` |
| 内容提供者 | `documents/registerContentProvider` | `documents.contentProvider` |
| 跨插件调用 | `functions/providers`、`functions/invoke` | 无清单权限，见下文 |

- 每个会话最多 16 个未回答的宿主请求；超出的请求以 `plugin.host.failed` 拒绝。Core 按固定顺序拒绝：协议没有定义的方法（`-32601`、`plugin.host.unavailable`）、没有授予的服务（`-32010`、`plugin.host.permissionDenied`）、参数格式错误（`-32602`、`plugin.host.invalidParams`），错误码在 `error.data.code`。宿主自己的失败是 `-32603`、`plugin.host.failed`。
- 新增宿主服务、方法、通知、可选的请求字段、结果或 `limits` 里的字段、失败码，都不需要新版本；只有现有后端会误读的改动才给 `backend` 版本 `1`，协议随之变化。
- 为了在每个宿主上都能工作，后端要遵守四条：只调用 `getCapabilities.services` 列出的方法（其他返回 `-32601`、`plugin.host.unavailable`）；忽略不认识的结果、通知、`details`、`limits` 字段和不认识的 `lithe/host/*` 通知；Core 对方法没有定义的字段返回 `-32018`、`plugin.host.unsupportedOption`，`details.option` 指明字段名，后端可以去掉该选项重试；把不认识的 JSON-RPC 号当作宿主失败，认识 `error.data.code` 时按它分支。
- 后端需要新的服务就声明新的权限；不认识这个权限的宿主在安装时拒绝这个包（`plugin.permission.unknown`），权限不会被悄悄授予或忽略。
- 宿主通知（后端不回复）：`workspace/didChangeContexts`、`subscriptions/notify`、`commands/didRun`、`run/didExit`、`languageServer/didChangeState`、`languageServer/didReceiveNotification`，授予条件见 API 总表。后端命令（不打开工具窗口的 `ui` 命令）是事件：运行中的后端按顺序收到 `commands/didRun`，已停止的后端只有声明了 `onCommand:<commandID>` 才会为它启动。

### 跨插件调用

插件函数默认只属于插件自己。插件导出函数后，另一个已启用、且在清单里声明了这次调用的插件，可以经 Rust Core 调用它：调用方的后端碰不到被调方的进程，被调方的后端也只知道自己正在服务的那次调用；宿主只在被调方的后端没有运行时负责启动它。中转是 Core 的逻辑，在所有运行插件后端的宿主上一样。

#### 清单声明

| 项目 | 规则 |
| --- | --- |
| 前提 | 有 `backend`，`requires` 写 `pluginFunctions: 0` 和 `backend: 0`；`requires` 列出 `pluginFunctions` 当且仅当清单有 `pluginFunctions` 段 |
| 段内容 | `exports` 与 `calls` 至少有一个，各最多 32 项、按 `id` 排序、`id` 不重复；多余的字段是 `plugin.manifest.invalid` |
| 导出 | `{id, version, context}`。`id` 是函数 ID；`version` 是导出的契约版本（1–1000 的整数），参数或结果有不兼容变更就换新版本；`context` 目前只有 `"workspace"`，表示只在本地工作区上下文里运行，缺失或未知则整个清单被拒绝 |
| 调用 | `{id, version}`。只写函数 ID 和调用方对照的版本，不写被调方的插件 ID，运行时再发现提供者；两个版本必须相等，否则 `plugin.function.versionMismatch` |
| 注册 | 声明导出不等于注册：被调方的后端仍要 `lithe/function/register`，未注册或已撤回的函数是 `plugin.function.notExported`。清单没有导出的函数保持私有，引用它仍是 `plugin.function.wrongPlugin` |
| 宿主支持 | 宿主向声明了 `calls` 的插件开放 `functions` 宿主服务；`hostCapabilities` 里没有 `pluginFunctions` 的宿主拒绝这类清单（`plugin.api.unsupported`） |

#### 宿主方法

只对声明了至少一个 `calls` 的后端开放，没有声明任何 call 的插件返回 `-32010`、`plugin.host.permissionDenied`。`getCapabilities` 在 `functions` 服务下列出这两个方法。

| 方法（前缀 `lithe/host/`） | 参数 | 结果 |
| --- | --- | --- |
| `functions/providers` | `{functionID}` | `{providers: [{pluginID, version}]}` |
| `functions/invoke` | `{pluginID, functionID, context: {contextID, generation}, params?, timeoutMilliseconds?, parentOperationID?}` | `{value}` |

- `providers` 列出导出该函数、已启用、且不是调用方自己的插件，按插件 ID 排序，每项带导出的 `version`。调用方必须声明过该函数，否则 `plugin.function.notDeclared`。被调方的后端不必正在运行。
- `invoke` 调用一个提供者。`context` 必填，是调用方从工作区服务得到的本地工作区上下文；`params` 是 JSON 对象，默认 `{}`；`timeoutMilliseconds` 是正整数，默认 60000，上限 600000；未定义的字段返回 `-32018`、`plugin.host.unsupportedOption`。`value` 是被调方的结果，`null` 也是结果。
- 调用方用 `$/cancelRequest` 取消，得到 `plugin.host.cancelled`。
- Core 按固定顺序检查（调用方声明、被调方启用、被调方导出、版本相等、上下文、本地工作区、参数大小、所属调用、调用链、深度、并发数），在向被调方写任何内容之前拒绝，第一个失败的检查决定错误码。

#### 被调方收到的请求

Core 把中转的调用以 `lithe/function/invoke` 转给被调方。和任何函数调用一样，请求顶层有 `functionID`、`operationID`、`context`、`params`；中转的调用另有顶层字段 `caller: {pluginID}`。

- `caller.pluginID` 由 Core 按调用会话的身份写入，在 `params` 之外，调用方无法提供：`functions/invoke` 请求带顶层 `caller` 会被 `-32018` 拒绝。要按调用方区分行为或保存状态的函数，读这个字段，不要读 `params` 里的插件 ID。调用方 `params` 里叫 `caller` 的键只是普通数据，原样传递。
- 宿主自己发起的调用没有 `caller`，缺失表示宿主。
- `operationID` 标识正在服务的这次调用。被调方在服务一次中转的调用时再发起 `functions/invoke`（嵌套调用），要把这个 `operationID` 作为 `parentOperationID` 传入，Core 才知道新调用属于哪一次。`parentOperationID` 是 1–128 字节的字符串，否则 `-32602`、`plugin.host.invalidParams`；不传时，Core 把新调用当作后端正在服务的每一次中转调用的子调用，所以后端应当总是传。
- 调用链：新调用的链是父调用的链加上调用方。调用一个已在链上的插件（包括调用自己）是 `plugin.function.callCycle`，链最长 4 次调用。取消、超时或结束父调用，会取消它的子调用，各被调方收到 `$/cancelRequest`。`parentOperationID` 指向宿主直接发起的调用时，新调用开始一条新链；指向后端没有在服务的调用（已结束或不存在）时被拒绝，`plugin.host.cancelled`。
- 被调方的后端没有运行时，Core 等待，并在调用方会话上发出运行时事件 `functionProviderStartRequired`，宿主按普通的插件启动流程启动它；30 秒内没有协商并注册好函数，调用以 `plugin.function.providerUnavailable` 结束。不能启动后端的宿主忽略这个事件，调用同样结束。
- Core 为每个后端会话用各自的队列写入：一个后端不再读标准输入，只会拖慢发给它的调用和回复，其他插件的中转照常。向后端的写入在请求时限之后仍被阻塞时，Core 把该后端的会话当作传输失败结束（`transportFailed`，阶段 `relayWrite`）。

#### 信任不传递

- 被调方用自己的权限、状态目录和凭据运行；调用方不获得被调方的任何权限，被调方也不获得调用方的。调用方的插件 ID 只出现在 `caller` 字段里，不写进被调方的会话。
- 导出的函数不得接受调用方给的可执行路径、命令行或脚本去启动进程，也不得读写调用方指定的路径而不核对自己的访问范围。调用方需要被调方运行什么，函数应当接收一个由被调方自己解释的请求（任务名、文档），由被调方决定可执行文件。
- 这不是安装依赖：安装调用方不会连带安装被调方，停用被调方只会让调用找不到提供者；不参与安装闭包，不做版本求解。远程工作区的上下文被拒绝，`plugin.host.authorityUnsupported`；导出的 `context: "workspace"` 只指本地工作区，后端仍在运行 Lithe 的机器上。
- 停用、启用、替换和卸载插件对中转立即生效，不需要重启；宿主用 Core 命令 `lsp.configureFunctionRelay` 发送插件和工作区上下文的快照。只在启动时注册后端插件的宿主上，新安装或更新的后端插件要重启 Lithe 后才会出现在 `providers` 里。

#### 错误码

| 错误码 | 何时 |
| --- | --- |
| `plugin.function.notDeclared` | 调用方没有声明该函数；调用进行中调用方更新后不再声明，也是它 |
| `plugin.function.providerDisabled` | 被调方没有安装或未启用；调用进行中被调方被停用、卸载，或更新后不再导出该函数或改了版本，也是它 |
| `plugin.function.notExported` | 被调方的后端没有注册该函数，或已撤回 |
| `plugin.function.versionMismatch` | 调用方声明的版本与被调方导出的版本不相等 |
| `plugin.function.callCycle` | 被调方已在调用链上，包括调用自己 |
| `plugin.function.depthExceeded` | 调用链超过 4 次调用 |
| `plugin.function.tooManyCalls` | 调用方已有 8 个中转调用在运行 |
| `plugin.function.payloadTooLarge` | 参数超过 256 KiB，或被调方的结果超过 256 KiB（超过 1 MiB 是 `invalidResult`） |
| `plugin.function.providerUnavailable` | 被调方的后端 30 秒内没有就绪；或调用期间后端退出、撤回了该函数 |

被调方自己的失败保留原意：函数回答了错误时是 `plugin.function.failed`（`-32070`），消息截到 1024 字节，`details.pluginErrorCode` 是被调方的 `error.data.code`；没有 `result`，或结果序列化后超过 1 MiB，是 `plugin.function.invalidResult`（`-32071`）。这两个号都不是 `-32603`，后者仍是 Core 内部失败 `plugin.host.failed`。工作区上下文被关闭或换代是 `plugin.host.contextExpired`，截止时间到了是 `plugin.host.timeout`；被调方在截止时间后 2 秒仍未回答时，中转自己以 `plugin.host.timeout` 结束调用并取消被调方的操作。Core 不重启后端，也不重试调用。

### Run 与调试会话

后端可以启动宿主当作自己的会话呈现的运行或调试：后端决定启动什么，宿主拥有进程、控制台、停止按钮和会话的结束。Run 会话只增加宿主呈现（Run 窗口里的条目、控制台、退出状态），不授予其他能力。`run/start` 与 `debug/start` 带 `context: {contextID, generation}` 和对应权限；`run/stop` 不带 `context`，所以启动它的上下文关闭后仍能停止。

| 方法（前缀 `lithe/host/`） | 权限 | 参数 | 结果 |
| --- | --- | --- | --- |
| `run/start` | `run.start` | `context, label, process, temporaryFiles?, debug?, rerun?` | `{runID}` |
| `run/stop` | `run.start` | `{runID}` 或 `{sessionID}` | `null` |
| `debug/start` | `debug.start` | `context, label, adapter, request, configuration` | `{sessionID}`，见[调试适配器](debug-adapters.md#由插件后端发起调试会话) |

通知（后端不回复）：`lithe/host/run/didExit`，参数 `{runID, exitCode?, signal?}`，授予条件是 `run.start`。`run.start` 授予 `run/start`、`run/stop` 和 `run/didExit`。

`run/start` 的参数：

- `label`：1–256 个字符，不含控制字符，作为 Run 窗口条目的标题。
- `process`：`{executable, arguments, workingDirectory, environment}`，四个字段都必填，未知字段被拒绝。`executable` 是 `{kind: "tool", tool}`（`tool` 是清单 `requiresTools` 列出的工具定义 ID，宿主按[工具链](toolchains.md)的分层解析）或 `{kind: "packaged", path}`（插件包内文件的规范化相对路径）。参数、工作目录（相对工作区根、且是其中已存在的目录）和环境的上限同[运行准备](../../contract/v1/README.md#provider-run-preparation)。
- `temporaryFiles`：最多 4 个 `{name, contents}`，每个文本最多 4 MiB。宿主在自己的临时目录写入，运行结束时删除，并把 `process.arguments` 和 `process.environment` 里的 `${temporaryFile:<name>}` 替换成文件路径，插件不需要知道临时目录在哪。`name` 唯一，1–64 个字母、数字、`.`、`_`、`-`，以字母或数字开头。
- `debug`：`{configurationType, target}`，让这次运行成为调试会话，见[调试适配器](debug-adapters.md#由插件后端发起调试会话)。`configurationType` 是公开的调试适配器别名；`target` 是 `{kind: "connection", port}`（进程在 `port`，1–65535，监听调试器，由后端写进参数，宿主等待并附加适配器）或 `{kind: "launch"}`（适配器自己启动进程，按 `targetBindings` 从 `process` 取程序、参数、目录和环境）。
- `rerun`：`{function, data}`，插件已注册的函数和它的参数（最多 64 KiB 的 JSON 对象）。用户选“再次运行”时，宿主以 `data` 为参数、当前 `context` 调用该函数，后端再回答一次新的 `run/start`，所以重启由后端准备；没有 `rerun` 的条目不能再次运行。

`debug/start` 的参数：

- `label` 同 `run/start`。`adapter` 是本插件自己的适配器，写本地 ID 或限定的 `<plugin-id>/<local-id>`；其他插件的适配器和本插件没有声明的适配器都是 `plugin.host.notFound`。
- `request` 是 `launch` 或 `attach`，适配器必须支持它。
- `configuration` 是最多 256 KiB 的 JSON 对象，宿主在适配器声明的默认值之上把它作为 DAP 的 launch 或 attach 参数传给适配器；`noDebug` 和所有以 `__` 开头的键归宿主所有，被拒绝。

其他规则：

- `run/stop` 按 `runID` 停止运行，或按 `sessionID` 停止调试会话，已经结束时同样返回 `null`；取消宿主请求等同停止。后端只能停止自己启动的运行和会话。
- 进程结束时宿主发 `run/didExit`，每次运行只发一次，只发给持有 `run.start` 的会话；`exitCode` 是退出状态，`signal`（1–127）是结束它的信号，宿主丢失进程时两个字段都没有。
- 参数格式错误是 `plugin.host.invalidParams`，Core 的拒绝（`plugin.run.invalidStart`、`plugin.debug.invalidStart`）在 `details.code` 和 `details.field`；上下文已关闭是 `plugin.host.contextExpired`；`tool` 解析不到可执行文件是 `plugin.host.toolUnresolved`。
- 插件被禁用或后端退出时，宿主保留运行中的进程，只是不再通知后端；重新启动的后端不会收到它没见过开始的运行的 `run/didExit`。
- 想让 Run 控制台里的输出出现链接，用 `outputLinkMatchers` 声明匹配器：Core 负责匹配，`resolve` 函数在用户点击时才调用，见 [Output link matchers](../../contract/v1/README.md#output-link-matchers)。

### 契约章节

| 章节 | 内容 |
| --- | --- |
| [Plugin backend](../../contract/v1/README.md#plugin-backend) | 后端声明、状态目录、启动与停止时机、按平台发布 |
| [Backend lifecycle and workspace contexts](../../contract/v1/README.md#backend-lifecycle-and-workspace-contexts) | 启动方式、`onWorkspaceOpen` 不阻塞工作区、`workspace.metadata` |
| [Plugin functions](../../contract/v1/README.md#plugin-functions) | 协议总览 |
| [Capability ownership](../../contract/v1/README.md#capability-ownership) | 插件函数不能代替领域契约 |
| [Declarative or backend implementation](../../contract/v1/README.md#declarative-or-backend-implementation) | 什么时候用声明式数据、什么时候写后端 |
| [Negotiation](../../contract/v1/README.md#negotiation) | `initialize` 里的版本协商 |
| [References and IDs](../../contract/v1/README.md#references-and-ids) | 函数引用和 ID 规则 |
| [Registration and withdrawal](../../contract/v1/README.md#registration-and-withdrawal) | 注册和撤回函数 |
| [Invocation](../../contract/v1/README.md#invocation)、[Progress](../../contract/v1/README.md#progress) | 调用、结果、进度、取消 |
| [Host services](../../contract/v1/README.md#host-services) | 凭据服务 |
| [Workspace contexts and capability discovery](../../contract/v1/README.md#workspace-contexts-and-capability-discovery) | `getCapabilities`、上下文 `contextID`/`generation`、宿主失败码 |
| [Host notifications](../../contract/v1/README.md#host-notifications) | `workspace/didChangeContexts`、`subscriptions/notify`、`commands/didRun`、`run/didExit`、语言服务器通知 |
| [Extending the host services](../../contract/v1/README.md#extending-the-host-services) | 不升版本地增加方法，后端要遵守的四条 |
| [Public document, file and window methods](../../contract/v1/README.md#public-document-file-and-window-methods) | 文件读取、监听、搜索与窗口消息（文档和编辑器方法见[编辑器](editor.md)） |
| [Plugin-owned native terminals](../../contract/v1/README.md#plugin-owned-native-terminals)、[Close, retained context and reopen](../../contract/v1/README.md#close-retained-context-and-reopen) | 插件自有原生终端 |
| [Run and debug sessions](../../contract/v1/README.md#run-and-debug-sessions) | `run/start`、`run/stop`、`debug/start`、`run/didExit`：参数、限制、失败码 |
| [Language server host methods](../../contract/v1/README.md#language-server-host-methods) | 后端与本插件自己的语言服务器对话 |
| [Content providers](../../contract/v1/README.md#content-providers) | 把 URI scheme 绑定到后端函数 |
| [Backend-only commands](../../contract/v1/README.md#backend-only-commands) | 不打开工具窗口的命令 |
| [Cross-plugin calls](../../contract/v1/README.md#cross-plugin-calls) | `pluginFunctions` 清单声明、`functions/providers`、`functions/invoke`、检查顺序、调用链、`caller` 字段、未运行的提供者、热生效 |
| [Output link matchers](../../contract/v1/README.md#output-link-matchers) | Run 与测试控制台里的输出链接，`resolve` 函数在点击时调用 |
| [Structured plugin diagnostics](../../contract/v1/README.md#structured-plugin-diagnostics) | `lithe/log` 的字段和限制 |
| [Compatibility](../../contract/v1/README.md#compatibility)、[Official backend SDK](../../contract/v1/README.md#official-backend-sdk) | 兼容规则与官方 SDK |
| [Outcomes and error codes](../../contract/v1/README.md#outcomes-and-error-codes) | 结果与错误码 |

### 示例与用例

| 文件 | 内容 |
| --- | --- |
| [`backend/backend.plugin.json`](../../contract/v1/fixtures/backend/backend.plugin.json) | 网页工具窗口调用后端，使用凭据服务 |
| [`backend/backend-workspace.plugin.json`](../../contract/v1/fixtures/backend/backend-workspace.plugin.json) | `onWorkspaceOpen` 启动，`workspace.metadata` |
| [`backend/command-backend.plugin.json`](../../contract/v1/fixtures/backend/command-backend.plugin.json) | 后端加 `ui`：不打开工具窗口的命令，读文档并编辑 |
| [`backend/terminal-backend.plugin.json`](../../contract/v1/fixtures/backend/terminal-backend.plugin.json) | 插件自有原生终端的最小清单 |
| [`backend/function-protocol.json`](../../contract/v1/fixtures/backend/function-protocol.json) | 插件函数协议的标准报文：协商、注册、调用、结果、进度、宿主请求（`hostRequests`）、调用上下文（`invocationContexts`）、被中转调用的 `caller`（`invocationCallers`）、宿主失败码（`hostFailures`）、中转失败码（`relayErrors`）和宿主通知（`hostNotifications`） |
| [`backend/plugin-functions-exporter.plugin.json`](../../contract/v1/fixtures/backend/plugin-functions-exporter.plugin.json) | 只导出两个函数（`document.format` 版本 1、`symbols.list` 版本 2，`context` 为 `workspace`），没有网页、命令或 `onWorkspaceOpen` |
| [`backend/plugin-functions-caller.plugin.json`](../../contract/v1/fixtures/backend/plugin-functions-caller.plugin.json) | 声明调用 `document.format` 版本 1，`onWorkspaceOpen` 启动 |
| [`backend/plugin-logging.json`](../../contract/v1/fixtures/backend/plugin-logging.json) | `litheLogging`：提供与回答、通知、拒绝用例、级别 |
| [`run/run-start.json`](../../contract/v1/fixtures/run/run-start.json) | `run/start` 的参数和 `run.decodeStart` 对每个用例的返回（工具与包内可执行文件、`debug` 与 `rerun`、各种拒绝）；临时文件依赖宿主的临时目录，由 Core 自己的测试覆盖 |
| [`run/tool-reference-launch.plugin.json`](../../contract/v1/fixtures/run/tool-reference-launch.plugin.json) | `run-start.json` 使用的清单：`requiresTools` 列出工具 `interpreter`，声明 `run.start`、`debug.start`，并带 `outputLinkMatchers` |
| [`run/output-link-matchers.json`](../../contract/v1/fixtures/run/output-link-matchers.json) | `outputLinks.match` 的请求与返回的链接，以及匹配器 `resolve` 函数的回复经 `outputLinks.decodeResolution` 得到的结果 |
| [`debug-adapters/debug-start.json`](../../contract/v1/fixtures/debug-adapters/debug-start.json) | `debug.decodeBackendConfiguration` 对 `debug/start` 每个用例的返回，含拒绝 |

后端相关的拒绝用例在 [`manifest/invalid-manifests.json`](../../contract/v1/fixtures/manifest/invalid-manifests.json)。

### 相关指南

- [插件后端：工作区上下文与宿主服务](../../guides/plugin-backend-workspace.md)
- [从插件打开原生终端](../../guides/native-terminals.md)
- [插件接入统一日志](../../guides/plugin-logging.md)

---

## English

A plugin backend is the plugin's own long-lived process. It runs what a web page cannot: database drivers, network clients, file-format parsers, calls to host services. It serves only its plugin, belongs to no build system, language, or workspace, and never answers domain requests such as `lithe/build/*` or VCS. Backend and host speak the plugin-function protocol (`litheFunctions`); the backend may request the host services its permissions grant, may start Run and debug sessions that the host owns, and may call functions that other plugins export, through Core.

> **Host support.** The Lithe desktop declares `backend` and `pluginFunctions` (both version `0`). Which host methods a session may call is decided by `services` in the run-time `lithe/host/getCapabilities` answer; do not assume it from this page.
> The comment above `Declared()` in the desktop lists the backend host services it registers: workspace contexts (`workspace/listContexts`, `getContext`, `getFolders`, and `workspace/didChangeContexts`), `subscriptions/dispose`, `languageServer/*`, the cross-plugin call relay, `credentials/*`, `documents/list` and `getSnapshot`, `editor/getActive`, `getSelections`, `open`, `reveal`, `insertText`, `replaceSelections`, `applyEdits`, `window/*`, `browser/openExternal` (HTTP(S) addresses only), `terminal/*`, and `documents/registerContentProvider`; methods bound to a window are registered once the window attaches.
> Workspace files and search (`workspace/readDirectory`, `stat`, `readFile`, `watch`, `findFiles`, `findText`), `preview/openHTML`, `editor/registerActions`, and `editor/registerProviders` are not registered, and a plugin that asks for those permissions is refused. Whether the Run and debug session methods (`run/start`, `run/stop`, `debug/start`) are available is likewise decided by `getCapabilities`.
> See the comment above `Declared()` in [`pluginhost.go`](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go); for a per-capability comparison see the [capability and protocol versions](../capabilities-and-versions.md).
> Questions about the contract go to [Lithe-core-rust Issues](https://github.com/Lithe-IDEA/Lithe-core-rust/issues).

| Item | Value |
| --- | --- |
| Manifest field | `backend` (the same fields as [process start](../../contract/v1/README.md#process-start): `launch`, `arguments`, and so on); cross-plugin calls add a `pluginFunctions` section (`exports`, `calls`) |
| Capability versions | `backend` `0`; backend commands of `ui` need both `ui: 0` and `backend: 0`; cross-plugin calls need `pluginFunctions: 0` and `backend: 0`. `requires` must list exactly the capabilities the manifest uses; process start is base contract, not a capability, so do not write `requires.serverProcess`. See [versions](../capabilities-and-versions.md#capability-versions-requires) |
| Protocols | `litheFunctions` `0` (the version is the version of `backend`), `litheLogging` `0`; see [process protocol versions](../capabilities-and-versions.md#process-protocol-versions) |
| API IDs | API-01, 04, 05, 06, 07, 08, 20, 21, 22, 23, 24, 35, 37, 40, 41; see the [API catalog](../api-catalog.md#apis-usable-now) |
| Permissions | `backend.launch`; host services need their own permission per method (`credentials.store`, `workspace.metadata`, `workspace.files.read`, `workspace.files.watch`, `workspace.search`, `terminal.create`, `run.start`, …); see [public host methods](../api-catalog.md#public-host-methods). Cross-plugin calls have no manifest permission: the caller's `calls` and the provider's `exports` are the consent of the two sides |
| Activation | At least one way to start is required: a web tool window's first `functions.*` call; a `ui.commands[]` command (`onCommand:<id>`); the optional `onWorkspaceOpen`; or a provider the host queries (a toolchain or keymap import provider, or a language server with `launch.kind: "backend"`). None of them is `plugin.manifest.invalid`. A plugin that only exports functions needs no page, command, or `onWorkspaceOpen`: a call starts its backend; a plugin that only calls still needs one of them |
| Official SDKs | [Node.js 0.5.0](../../sdk/node/README.md), [Java 0.5.0](../../sdk/java/README.md), [Rust 0.4.0](../../sdk/rust/README.md), released as `sdk-node-v0.5.0`, `sdk-java-v0.5.0`, and `sdk-rust-v0.4.0`; see [official SDKs](../capabilities-and-versions.md#official-sdks). All three implement the function protocol and wrap credentials, workspaces, documents and editors (including `editor/registerActions`), preview, browser, window messages, `functions/providers`, and `functions/invoke`; a called function can read `caller`; see each SDK's README for usage. When `parentOperationID` is omitted, the SDK uses the `operationID` of the `lithe/function/invoke` being handled. `run/*`, `debug/start`, `languageServer/*`, `documents/registerContentProvider`, and `editor/registerProviders` have no dedicated wrappers; call them through the SDK's generic host request (`request`). Native terminals are wrapped; see [Open a native Lithe terminal from a plugin](../../guides/native-terminals.md) |

### Backend start and workspaces

- A plugin has at most one backend process, shared by every open workspace and window, on the machine that runs Lithe, also for remote workspaces. No workspace is process state: each call names its workspace context explicitly, so `${workspaceStateDir}` is refused; the writable state is `${stateDir}`, and the host starts the backend with it as root and working directory.
- The host starts the backend on the first trigger in the "Activation" row above, and ends it when the plugin is disabled, updated, or uninstalled, and when the host quits. Consent is the install review, which lists it as `packagedBackend`, `systemCommandBackend`, or `runtimeBackend`; no per-workspace prompt is needed.
- Starting on `onWorkspaceOpen` never delays opening the workspace. The host starts such backends after the window is shown, one at a time; a backend that does not finish `initialize` within its initialize timeout is stopped and reported like any failed server start. A failed or disabled backend never blocks the workspace.
- The `workspace.metadata` permission grants the `workspace` host service: the open workspaces' names, root folders and IDs, their authority, and the plugin's per-workspace state directory; it reads no file content.
- Activation is gated only by the plugin being installed, enabled, and reviewed. Lithe has no workspace trust state at the moment: an operation that runs or changes something asks the user per operation, as the host's own controls do.
- A plugin publishes one archive per platform, so its packaged path does not vary by `${arch}`; a macOS archive may carry a universal binary.

### Protocol essentials

| Item | Rule |
| --- | --- |
| Negotiation | For a plugin-owned session the host offers `{"versions": [0]}` as `capabilities.experimental.litheFunctions` in `initialize`, independent of `litheBuild`; the backend answers `{"version": 0, "functions": [{"id": "…"}]}`. An answer with a version the host did not offer, or a malformed or duplicate function ID, does not negotiate the protocol at all |
| Function ID | Dot-separated lowerCamelCase segments (`snapshot.refresh`), at most 128 bytes, unique within the session; a session holds at most 256 functions |
| Registration and withdrawal | After `initialize`, `lithe/function/register` and `unregister`; each request is all or nothing (`-32602`, `plugin.function.invalidRegistration`) |
| Invocation | The host sends `lithe/function/invoke` with top-level `functionID`, `operationID`, `context`, and `params` (a JSON object, default `{}`); Core assigns `operationID`, and progress notifications must echo it unchanged |
| Outcome | A result, or `plugin.function.failed` (the backend's message up to 1024 bytes, with `pluginErrorCode` from `error.data.code`), or `plugin.function.invalidResult` (no `result`, or over 1 MiB serialized); the host cancels and times out with `$/cancelRequest` |
| Progress | `lithe/function/progress {operationID, message?, percentage?}`; `message` is cut to 1024 bytes and `percentage` is a whole number from 0 to 100 |
| No loops | A backend never asks the host to dispatch a domain operation or a function: a request for `lithe/function/invoke` or any `lithe/build/*` method is refused with `plugin.function.recursiveCall`. Cross-plugin calls are a different request; see below |
| Private functions | They need no manifest field, and adding or removing them changes no version; only a function another plugin may call, and a call to it, is declared in `pluginFunctions` |
| Not answering the protocol | A backend that does not answer `litheFunctions` (a build-only backend, for example) keeps serving its language and its `litheBuild` requests and receives no function calls; its function requests get `-32601` |

### Host services

A backend sends requests to the host with the prefix `lithe/host/`. Each service is granted by one manifest permission, and Core checks every request before the host sees it. The full methods, permissions, and limits are in the [API catalog](../api-catalog.md#public-host-methods).

| Service | Methods | Permission |
| --- | --- | --- |
| Capability discovery | `getCapabilities`, `subscriptions/dispose` | Every plugin session has it |
| Credentials | `credentials/get`, `set`, `delete` | `credentials.store` |
| Workspace contexts | `workspace/listContexts`, `getContext`, `getFolders` | `workspace.metadata` |
| Files and search | `workspace/readDirectory`, `stat`, `readFile`, `watch`, `findFiles`, `findText` | `workspace.files.read`, `workspace.files.watch`, `workspace.search` |
| Documents and editors | `documents/*`, `editor/*`; see [Editor](editor.md) | `documents.read`, `editor.edit`, `editor.open`, `editor.actions`, `editor.providers` |
| Preview, browser, window messages | `preview/openHTML`, `browser/openExternal`, `window/*` | `preview.open`, `browser.open`, `window.messages` |
| Native terminals | `terminal/*` | `terminal.create` |
| Run and debug sessions | `run/start`, `run/stop`, `debug/start` | `run.start`, `debug.start` |
| Language servers | `languageServer/request`, `notify`, `setStatus` | `languageServer.request` |
| Content providers | `documents/registerContentProvider` | `documents.contentProvider` |
| Cross-plugin calls | `functions/providers`, `functions/invoke` | No manifest permission; see below |

- A session holds at most 16 unanswered host requests; a further one is refused with `plugin.host.failed`. Core refuses in this order: a method the protocol does not define (`-32601`, `plugin.host.unavailable`), a service the plugin is not granted (`-32010`, `plugin.host.permissionDenied`), and malformed params (`-32602`, `plugin.host.invalidParams`); the code is in `error.data.code`. A failure of the host itself is `-32603`, `plugin.host.failed`.
- A later host adds a service, a method, a notification, an optional request field, a field of a result or of `limits`, or a failure code without a new version; only a change an existing backend would misread gives `backend` version `1`, and the protocol moves with it.
- To work with every host, a backend follows four rules: it calls a method only when `getCapabilities.services` lists it (another is `-32601`, `plugin.host.unavailable`); it ignores result, notification, `details`, and `limits` fields it does not know, and any `lithe/host/*` notification it does not know; Core refuses a field the method does not define with `-32018`, `plugin.host.unsupportedOption`, and `details.option` names it, so the backend may retry without that option; and it treats a JSON-RPC number it does not know as a host failure, branching on `error.data.code` when it knows that code.
- A backend that needs a new service declares the new permission; a host that does not know the permission refuses the package at install (`plugin.permission.unknown`), so a permission is never granted or ignored silently.
- Host notifications (the backend does not reply): `workspace/didChangeContexts`, `subscriptions/notify`, `commands/didRun`, `run/didExit`, `languageServer/didChangeState`, and `languageServer/didReceiveNotification`; the grants are in the API catalog. Backend commands (a `ui` command that opens no tool window) are events: a running backend receives `commands/didRun` in order, and a stopped one is started for it only when it declares `onCommand:<commandID>`.

### Cross-plugin calls

A plugin function is private to its plugin unless the plugin exports it. An exported function can be called, through Rust Core, by another enabled plugin whose manifest declares that call: the calling backend never reaches the provider's process, the provider's backend never learns more than the call it serves, and the host takes part only by starting a provider's backend that is not running. The relay is Core logic, the same on every host that runs plugin backends.

#### Manifest declarations

| Item | Rule |
| --- | --- |
| Prerequisites | A `backend`, and `requires` with `pluginFunctions: 0` and `backend: 0`; `requires` lists `pluginFunctions` exactly when the manifest has a `pluginFunctions` section |
| Section | At least one of `exports` and `calls`, each at most 32 entries, sorted by `id`, no `id` twice; any other field is `plugin.manifest.invalid` |
| Export | `{id, version, context}`. `id` is a function ID; `version` is the exported contract version (an integer from 1 to 1000), and an incompatible change to the params or result takes a new version; `context` has one value for now, `"workspace"`, meaning the function runs only for a local workspace context; a missing or unknown `context` refuses the whole manifest |
| Call | `{id, version}`. It names a function ID and the version the caller was written against, never the providing plugin's ID: the caller discovers providers at run time; the two versions must be equal, otherwise `plugin.function.versionMismatch` |
| Registration | Declaring an export does not register it: the provider's backend still calls `lithe/function/register`, and a function it has not registered, or has withdrawn, is `plugin.function.notExported`. A function the manifest does not export stays private, and a reference to it is still `plugin.function.wrongPlugin` |
| Host support | The host grants a plugin that declares `calls` the `functions` host service; a host without `pluginFunctions` in `hostCapabilities` refuses such a manifest (`plugin.api.unsupported`) |

#### Host methods

Only a backend whose plugin declares at least one call may use them; a plugin that declares no call gets `-32010`, `plugin.host.permissionDenied`. `getCapabilities` lists both methods under the `functions` service.

| Method (prefix `lithe/host/`) | Params | Result |
| --- | --- | --- |
| `functions/providers` | `{functionID}` | `{providers: [{pluginID, version}]}` |
| `functions/invoke` | `{pluginID, functionID, context: {contextID, generation}, params?, timeoutMilliseconds?, parentOperationID?}` | `{value}` |

- `providers` lists the enabled plugins other than the caller that export the function, sorted by plugin ID, each with its exported `version`. The caller must have declared the function, otherwise `plugin.function.notDeclared`. A provider's backend need not be running.
- `invoke` calls one provider. `context` is required: a local workspace context the caller learned from the workspace service. `params` is a JSON object and defaults to `{}`. `timeoutMilliseconds` is a positive integer, defaults to 60000, and is capped at 600000. A field the method does not define is `-32018`, `plugin.host.unsupportedOption`. `value` is the provider's result, and `null` is a result.
- The caller cancels a call with `$/cancelRequest`; it fails with `plugin.host.cancelled`.
- Core checks in a fixed order (the caller declared the function, the provider is enabled, the provider exports it, the versions are equal, the context, a local workspace, the params size, the call it belongs to, the call chain, the depth, the concurrency) and refuses before anything is written to the provider; the first failing check decides the code.

#### What the provider receives

Core forwards a relayed call to the provider as `lithe/function/invoke`. As for any function call, the request has top-level `functionID`, `operationID`, `context`, and `params`; a relayed call adds one more top-level field, `caller: {pluginID}`.

- Core writes `caller.pluginID` from the identity of the calling session. It sits outside `params` and the caller cannot supply it: a `functions/invoke` request carrying a top-level `caller` is refused with `-32018`. A function that decides by who asks, or keeps per-caller state, reads this field, never a plugin ID in `params`. A key named `caller` inside the caller's own `params` is ordinary data and is passed unchanged.
- A call the host itself started has no `caller`; absent means the host.
- `operationID` identifies the call being served. A provider that makes a nested `functions/invoke` while serving a relayed call sets `parentOperationID` to this `operationID`, so Core knows which call the new one belongs to. `parentOperationID` is a string of 1 to 128 bytes, otherwise `-32602`, `plugin.host.invalidParams`. Without it, Core treats the new call as a child of every relayed call the backend is serving, so a backend should always set it.
- The call chain: the chain of a new call is the chain of its parent plus the caller. A call to a plugin already on the chain, including a plugin calling itself, is `plugin.function.callCycle`, and a chain has at most 4 calls. Cancelling, timing out, or ending the parent call cancels its children, and each provider receives `$/cancelRequest`. If `parentOperationID` names a call the host started directly, the new call starts a new chain; if it names a call this backend is not serving (finished, or never existed), the call is refused with `plugin.host.cancelled`.
- If the provider's backend is not running, Core waits for it and emits the runtime event `functionProviderStartRequired` on the calling session; the host starts the backend as an ordinary plugin start. If the provider has not negotiated the protocol and registered the function within 30 seconds, the call ends with `plugin.function.providerUnavailable`. A host that cannot start the backend ignores the event, and the call ends the same way.
- Core writes to each backend session through a queue of its own: a backend that stops reading its standard input only delays the calls and replies sent to it, and the relayed calls of other plugins continue. If a write to a backend stays blocked past the request timeout, Core ends that backend's session as a transport failure (`transportFailed`, stage `relayWrite`).

#### Trust is not transitive

- The provider runs with its own permissions, state directory, and credentials. The caller gains none of the provider's, and the provider gains none of the caller's. The caller's plugin ID appears only in the `caller` field, never in the provider's session.
- An exported function must not accept an executable path, command line, or script from its caller and start a process with it, nor read or write a path the caller chose without checking it against what the provider itself may access. If a caller needs the provider to run something, the function takes a request the provider interprets (a task name, a document) and the provider decides the executable.
- This is not an install dependency: installing the caller does not install a provider, and disabling a provider only leaves calls without one; it takes no part in the install closure and does no version solving. A call whose context is a remote workspace is refused with `plugin.host.authorityUnsupported`; the exported `context: "workspace"` means a local workspace only, and backends still run on the machine that runs Lithe.
- Disabling, enabling, replacing, and uninstalling a plugin take effect for the relay at once, with no restart; the host sends the snapshot of plugins and workspace contexts with the Core command `lsp.configureFunctionRelay`. On a host that registers backend plugins only when it launches, a newly installed or updated backend plugin appears in `providers` only after Lithe restarts.

#### Error codes

| Code | When |
| --- | --- |
| `plugin.function.notDeclared` | The caller did not declare the function; also when a caller update stops declaring it while the call runs |
| `plugin.function.providerDisabled` | The provider is not installed or not enabled; also when, while the call runs, the provider is disabled or uninstalled, or its update stops exporting the function or changes its version |
| `plugin.function.notExported` | The provider's backend has not registered the function, or has withdrawn it |
| `plugin.function.versionMismatch` | The version the caller declared differs from the version the provider exports |
| `plugin.function.callCycle` | The provider is already on the call chain, including the caller itself |
| `plugin.function.depthExceeded` | The chain would have more than 4 calls |
| `plugin.function.tooManyCalls` | The caller already has 8 relayed calls running |
| `plugin.function.payloadTooLarge` | The params are over 256 KiB, or the provider's result is over 256 KiB (over 1 MiB is `invalidResult`) |
| `plugin.function.providerUnavailable` | The provider's backend was not ready within 30 seconds, or it exited or withdrew the function while the call ran |

A provider's own failure keeps its meaning: when its function answered with an error, the code is `plugin.function.failed` (`-32070`), the message is cut to 1024 bytes, and `details.pluginErrorCode` is the provider's own `error.data.code`; a reply with no `result`, or a result over 1 MiB serialized, is `plugin.function.invalidResult` (`-32071`). Neither number is `-32603`, which stays `plugin.host.failed`, a failure inside Core. A workspace context that is closed or replaced by a newer generation is `plugin.host.contextExpired`, and the deadline passing is `plugin.host.timeout`; if the provider has still not answered 2 seconds after the deadline, the relay ends the call with `plugin.host.timeout` itself and cancels the provider's operation. Core never restarts a backend or retries a call.

### Run and debug sessions

A backend can start a run or a debug session that the host presents as its own: the backend decides what to start, and the host owns the process, the console, the Stop button, and the end of the session. A Run session adds host presentation (a Run window entry, the console, the exit status) and grants no other capability. `run/start` and `debug/start` take `context: {contextID, generation}` and the permission shown; `run/stop` takes no `context`, so a run can still be stopped after the context it started in has closed.

| Method (prefix `lithe/host/`) | Permission | Params | Result |
| --- | --- | --- | --- |
| `run/start` | `run.start` | `context, label, process, temporaryFiles?, debug?, rerun?` | `{runID}` |
| `run/stop` | `run.start` | `{runID}` or `{sessionID}` | `null` |
| `debug/start` | `debug.start` | `context, label, adapter, request, configuration` | `{sessionID}`; see [Debug adapters](debug-adapters.md#starting-a-debug-session-from-a-plugin-backend) |

Notification (the backend does not reply): `lithe/host/run/didExit` with `{runID, exitCode?, signal?}`, granted by `run.start`. `run.start` grants `run/start`, `run/stop`, and `run/didExit`.

`run/start` parameters:

- `label`: 1 to 256 characters without control characters; it titles the Run window entry.
- `process`: `{executable, arguments, workingDirectory, environment}`, all four required; an unknown field is refused. `executable` is `{kind: "tool", tool}` (`tool` is a tool-definition ID that the manifest lists in `requiresTools`, which the host resolves through the layers described in [Toolchains](toolchains.md)) or `{kind: "packaged", path}` (a normalized relative path of a file inside the plugin package). Arguments, the working directory (relative to the workspace root and an existing directory inside it), and the environment have the bounds of [run preparation](../../contract/v1/README.md#provider-run-preparation).
- `temporaryFiles`: at most four `{name, contents}` files of up to 4 MiB of text each. The host writes them into its own temporary directory, removes them when the run ends, and replaces `${temporaryFile:<name>}` in `process.arguments` and `process.environment` with the file's path, so the plugin never needs to know where the host keeps temporary files. A `name` is unique, 1 to 64 letters, digits, `.`, `_`, or `-`, and starts with a letter or digit.
- `debug`: `{configurationType, target}`, which makes the run a debug session; see [Debug adapters](debug-adapters.md#starting-a-debug-session-from-a-plugin-backend). `configurationType` is a public debug adapter alias; `target` is `{kind: "connection", port}` (the process listens for a debugger on `port`, 1 to 65535, which the backend puts into the arguments; the host waits for it and attaches the adapter) or `{kind: "launch"}` (the adapter starts the process itself, taking program, arguments, directory, and environment from `process` through its `targetBindings`).
- `rerun`: `{function, data}`, one of the plugin's registered functions and its params (a JSON object of up to 64 KiB). When the user chooses Run again, the host invokes the function with `data` as the params and the current `context`, and the backend answers with a new `run/start`, so the backend prepares the restart; without `rerun` the entry cannot be rerun.

`debug/start` parameters:

- `label` is as in `run/start`. `adapter` is one of the plugin's own adapters, by its local ID or its qualified `<plugin-id>/<local-id>`; an adapter of another plugin, and one the plugin does not declare, are both `plugin.host.notFound`.
- `request` is `launch` or `attach`, and the adapter must serve it.
- `configuration` is a JSON object of at most 256 KiB that the host passes to the adapter as the DAP launch or attach arguments, on top of the adapter's declared defaults; `noDebug` and every key that starts with `__` belong to the host and are refused.

Other rules:

- `run/stop` stops a run by `runID` or a debug session by `sessionID` and answers `null`, also when it already ended; cancelling the host request is the same as stopping. A backend can stop only runs and sessions it started.
- The host sends `run/didExit` when the process ends, once per run and only to a session that holds the `run.start` grant. `exitCode` is the exit status and `signal` (1 to 127) the signal that ended it; both are absent when the host lost the process.
- Malformed params are `plugin.host.invalidParams`, and Core's rejection (`plugin.run.invalidStart` or `plugin.debug.invalidStart`) is in `details.code` and `details.field`; a closed context is `plugin.host.contextExpired`; a `tool` that resolves to no executable is `plugin.host.toolUnresolved`.
- The host keeps running processes when the plugin is disabled or its backend exits; it only stops telling the backend. A backend that starts again does not receive `run/didExit` for a run it did not see begin.
- To make output in the Run console clickable, declare matchers with `outputLinkMatchers`: Core does the matching and the `resolve` function is called only when the user clicks; see [Output link matchers](../../contract/v1/README.md#output-link-matchers).

### Contract sections

| Section | Content |
| --- | --- |
| [Plugin backend](../../contract/v1/README.md#plugin-backend) | Declaration, state directory, when it starts and stops, per-platform archives |
| [Backend lifecycle and workspace contexts](../../contract/v1/README.md#backend-lifecycle-and-workspace-contexts) | Start routes, `onWorkspaceOpen` never blocks the workspace, `workspace.metadata` |
| [Plugin functions](../../contract/v1/README.md#plugin-functions) | Protocol overview |
| [Capability ownership](../../contract/v1/README.md#capability-ownership) | Plugin functions never replace a domain contract |
| [Declarative or backend implementation](../../contract/v1/README.md#declarative-or-backend-implementation) | When to use declarative data and when to write a backend |
| [Negotiation](../../contract/v1/README.md#negotiation) | Version negotiation in `initialize` |
| [References and IDs](../../contract/v1/README.md#references-and-ids) | Function references and ID rules |
| [Registration and withdrawal](../../contract/v1/README.md#registration-and-withdrawal) | Registering and withdrawing functions |
| [Invocation](../../contract/v1/README.md#invocation), [Progress](../../contract/v1/README.md#progress) | Calls, results, progress, cancellation |
| [Host services](../../contract/v1/README.md#host-services) | Credentials service |
| [Workspace contexts and capability discovery](../../contract/v1/README.md#workspace-contexts-and-capability-discovery) | `getCapabilities`, `contextID`/`generation` contexts, host failure codes |
| [Host notifications](../../contract/v1/README.md#host-notifications) | `workspace/didChangeContexts`, `subscriptions/notify`, `commands/didRun`, `run/didExit`, language server notifications |
| [Extending the host services](../../contract/v1/README.md#extending-the-host-services) | How services gain methods without a new version, and the four rules for a backend |
| [Public document, file and window methods](../../contract/v1/README.md#public-document-file-and-window-methods) | File reads, watches, search, and window messages (document and editor methods are on [Editor](editor.md)) |
| [Plugin-owned native terminals](../../contract/v1/README.md#plugin-owned-native-terminals), [Close, retained context and reopen](../../contract/v1/README.md#close-retained-context-and-reopen) | Plugin-owned native terminals |
| [Run and debug sessions](../../contract/v1/README.md#run-and-debug-sessions) | `run/start`, `run/stop`, `debug/start`, `run/didExit`: params, limits, failure codes |
| [Language server host methods](../../contract/v1/README.md#language-server-host-methods) | A backend talking to its own plugin's language servers |
| [Content providers](../../contract/v1/README.md#content-providers) | Binding a URI scheme to a backend function |
| [Backend-only commands](../../contract/v1/README.md#backend-only-commands) | Commands that open no tool window |
| [Cross-plugin calls](../../contract/v1/README.md#cross-plugin-calls) | The `pluginFunctions` manifest declarations, `functions/providers`, `functions/invoke`, the check order, the call chain, the `caller` field, providers that are not running, hot activation |
| [Output link matchers](../../contract/v1/README.md#output-link-matchers) | Links in Run and Test console output; the `resolve` function is called on click |
| [Structured plugin diagnostics](../../contract/v1/README.md#structured-plugin-diagnostics) | `lithe/log` fields and limits |
| [Compatibility](../../contract/v1/README.md#compatibility), [Official backend SDK](../../contract/v1/README.md#official-backend-sdk) | Compatibility rules and the official SDKs |
| [Outcomes and error codes](../../contract/v1/README.md#outcomes-and-error-codes) | Outcomes and error codes |

### Examples and cases

| File | Content |
| --- | --- |
| [`backend/backend.plugin.json`](../../contract/v1/fixtures/backend/backend.plugin.json) | A web tool window calls the backend, which uses the credentials service |
| [`backend/backend-workspace.plugin.json`](../../contract/v1/fixtures/backend/backend-workspace.plugin.json) | Starts on `onWorkspaceOpen`, uses `workspace.metadata` |
| [`backend/command-backend.plugin.json`](../../contract/v1/fixtures/backend/command-backend.plugin.json) | A backend with `ui`: a command without a tool window that reads and edits documents |
| [`backend/terminal-backend.plugin.json`](../../contract/v1/fixtures/backend/terminal-backend.plugin.json) | The smallest manifest for plugin-owned native terminals |
| [`backend/function-protocol.json`](../../contract/v1/fixtures/backend/function-protocol.json) | Canonical plugin-function messages: negotiation, registration, invocation, results, progress, host requests (`hostRequests`), call contexts (`invocationContexts`), the `caller` of a relayed call (`invocationCallers`), host failure codes (`hostFailures`), relay failure codes (`relayErrors`), and host notifications (`hostNotifications`) |
| [`backend/plugin-functions-exporter.plugin.json`](../../contract/v1/fixtures/backend/plugin-functions-exporter.plugin.json) | Only exports two functions (`document.format` version 1, `symbols.list` version 2, `context` `workspace`), with no page, command, or `onWorkspaceOpen` |
| [`backend/plugin-functions-caller.plugin.json`](../../contract/v1/fixtures/backend/plugin-functions-caller.plugin.json) | Declares a call to `document.format` version 1, starts on `onWorkspaceOpen` |
| [`backend/plugin-logging.json`](../../contract/v1/fixtures/backend/plugin-logging.json) | `litheLogging`: offer and answer, notifications, rejections, levels |
| [`run/run-start.json`](../../contract/v1/fixtures/run/run-start.json) | The params of `run/start` and what `run.decodeStart` returns for each case (tool and packaged executables, `debug` and `rerun`, the refusals); temporary files depend on the host's temporary directory and are covered by Core's own tests |
| [`run/tool-reference-launch.plugin.json`](../../contract/v1/fixtures/run/tool-reference-launch.plugin.json) | The manifest `run-start.json` uses: `requiresTools` lists the tool `interpreter`, and it declares `run.start`, `debug.start`, and `outputLinkMatchers` |
| [`run/output-link-matchers.json`](../../contract/v1/fixtures/run/output-link-matchers.json) | The requests of `outputLinks.match` and the links it returns, plus what `outputLinks.decodeResolution` makes of a matcher's `resolve` reply |
| [`debug-adapters/debug-start.json`](../../contract/v1/fixtures/debug-adapters/debug-start.json) | What `debug.decodeBackendConfiguration` returns for each `debug/start` case, including the refusals |

Backend rejections are in [`manifest/invalid-manifests.json`](../../contract/v1/fixtures/manifest/invalid-manifests.json).

### Related guides

- [Plugin backend: workspace contexts and host services](../../guides/plugin-backend-workspace.md)
- [Open a native Lithe terminal from a plugin](../../guides/native-terminals.md)
- [Plugin logging](../../guides/plugin-logging.md)
