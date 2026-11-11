# 插件后端：工作区上下文与宿主服务 / Plugin backend: workspace contexts and host services

[中文](#中文) · [English](#english)

## 中文

这篇指南面向写插件后端（`backend`）的作者：让后端在没有页面时也能启动、知道用户打开了哪些工作区、响应命令，并按需调用宿主能力，包括调用其他插件导出的函数。

字段与错误码的权威定义在[契约](../contract/v1/README.md#backend-lifecycle-and-workspace-contexts)，这里只讲怎么用、为什么这样设计。

> **宿主支持。** Lithe 桌面端声明 `backend` 和 `pluginFunctions`（版本都是 `0`），所以本篇的函数协议、宿主服务和跨插件调用都在桌面端运行。宿主只授予它实现了的服务：一次会话能调用哪些方法，以运行时 `lithe/host/getCapabilities` 的 `services` 为准，不要按文档假定（第 5 节、第 10 节）。桌面端的声明和说明见
> [`pluginhost.go`](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go) 中 `Declared()` 上方的注释。
> 三个官方 SDK 都实现了本篇的协议：Node.js SDK 0.5.0、Java SDK 0.5.0 和 Rust SDK 0.4.0
> （见各自 README 的 Host services：[Node.js](../sdk/node/README.md#host-services)、[Java](../sdk/java/README.md#host-services)、
> [Rust](../sdk/rust/README.md#host-services)；发布页：[sdk-node-v0.5.0](https://github.com/Lithe-IDEA/Lithe-core-rust/releases/tag/sdk-node-v0.5.0)、
> [sdk-java-v0.5.0](https://github.com/Lithe-IDEA/Lithe-core-rust/releases/tag/sdk-java-v0.5.0)、
> [sdk-rust-v0.4.0](https://github.com/Lithe-IDEA/Lithe-core-rust/releases/tag/sdk-rust-v0.4.0)）。用其他语言需要自己处理下文的请求和通知。
> 各能力在 Core 和桌面端的支持情况见[能力与协议版本对照](../reference/capabilities-and-versions.md)。对契约有疑问，请到
> [Lithe-core-rust 的 Issues](https://github.com/Lithe-IDEA/Lithe-core-rust/issues) 反馈。

### 1. 三个机制各管什么

| 写在哪里 | 值 | 决定什么 |
| --- | --- | --- |
| `requires.backend` | `0` | 插件能不能安装，后端进程**什么时候启动**，能不能声明 `workspace.metadata` 和 `onWorkspaceOpen` |
| `initialize` 应答的 `litheFunctions.version` | `0` | 进程启动后的**线路协议**：函数注册与调用、宿主服务、能力发现、工作区上下文、宿主通知、带类型的失败码、跨插件调用。协议没有独立的版本，它的版本就是 `backend` 的版本 |
| `getCapabilities.services` | 运行时查询结果 | 本次会话**实际能调用**哪些宿主方法、会收到哪些通知 |

清单只做声明，宿主读清单决定装不装、什么时候启动；进程起来以后能做什么，靠协商和 `getCapabilities` 判断。

### 2. 最小清单

```json
{
  "schemaVersion": 0,
  "id": "io.github.<你的用户名>.orders",
  "displayName": "Orders",
  "version": "1.0.0",
  "pluginAPIVersion": 0,
  "requires": { "backend": 0 },
  "vendor": { "id": "io.github.<你的用户名>", "displayName": "<你的名字>", "signatureRequirement": "none" },
  "permissions": ["backend.launch", "workspace.metadata"],
  "activationEvents": ["onWorkspaceOpen"],
  "languages": [],
  "backend": {
    "launch": { "kind": "packaged", "path": "bin/backend" },
    "arguments": ["--state", "${stateDir}"]
  }
}
```

- `requires` 必须恰好列出清单用到的能力。启动进程（[Process start](../contract/v1/README.md#process-start)）属于基础契约，不是能力，不写进 `requires`。
- `backend` 至少要有一种启动方式：网页工具窗口（首次页面调用）、`ui.commands[]` 里的命令（`onCommand:<id>` 也会启动后端）、`onWorkspaceOpen`，或宿主向它查询的提供者（工具链提供者、快捷键导入提供者，或 `launch.kind` 为 `backend` 的语言服务器，它们启动时向后端要命令行）。一种都没有是 `plugin.manifest.invalid`。
- 只想要命令、不要工具窗口时，在 `requires` 里加 `ui: 0`：命令可以不写 `toolWindowID`，`toolWindows` 可以为空或省略，见第 7 节。
- `workspace.metadata` 只让后端知道打开的工作区名字、根目录、ID、来源（本地或远程）和每个工作区的私有状态目录，
  不读任何文件内容。不需要这些信息就不要声明，安装审查会把它列给用户。
- 拒绝 `${workspaceStateDir}`：一个插件只有一个后端进程，被所有窗口共享，工作区不是进程状态。
  每个工作区的状态目录从 `getContext` 的 `stateDirectory` 取；插件自己的整体状态目录是 `${stateDir}`，也是后端的启动根目录和工作目录。
- 一个平台一个发布包，包内路径不按 `${arch}` 变化。

### 3. 启动时机

- 一个插件最多一个后端进程，所有工作区和窗口共享，在 Lithe 所在的机器上运行，远程工作区也一样。
- `onWorkspaceOpen` 不会拖慢打开工作区。宿主在窗口显示之后逐个启动这些后端；`initialize` 超时的后端会被停止，
  并像其他启动失败一样报告，不会挡住工作区。后端失败或被禁用也不会阻塞工作区。
- 后端在插件被禁用、更新、卸载和宿主退出时结束。
- 激活只看插件是否已安装、启用、通过审查。Lithe 目前没有工作区信任状态；会运行或修改东西的操作，由宿主逐次询问用户。
- 后端只服务插件自己：它不回答 `lithe/build/*` 或 VCS 请求。只回答构建请求的后端（不回答 `litheFunctions`）收不到函数请求，函数请求得到 `-32601`。

### 4. 协商协议

宿主在 `initialize` 里提供 `capabilities.experimental.litheFunctions: { "versions": [0] }`。后端实现了协议就这样作答：

```json
"experimental": { "litheFunctions": { "version": 0, "functions": [{ "id": "orders.refresh" }] } }
```

`functions` 可省略，省略表示没有私有函数；之后也可以用 `lithe/function/register` 和 `lithe/function/unregister` 增删。回答宿主没有提供的版本，或函数 ID 格式错误、重复，整个协议都不会协商成功，这次会话不会带着半份注册表启动。不认识的字段双方都忽略。

函数 ID 是用 `.` 分隔的小驼峰段（`orders.refresh`），最多 128 字节，一个会话最多 256 个函数。

### 5. 先问宿主能做什么

协商成功以后，第一件事是调 `lithe/host/getCapabilities`（不带参数或 `{}`）：

```json
{
  "protocolVersion": 0,
  "backendAuthority": "local",
  "services": {
    "commands": { "methods": [], "notifications": ["lithe/host/commands/didRun"] },
    "host": { "methods": ["lithe/host/getCapabilities"], "notifications": [] },
    "subscriptions": {
      "methods": ["lithe/host/subscriptions/dispose"],
      "notifications": ["lithe/host/subscriptions/notify"]
    },
    "workspace": {
      "methods": ["lithe/host/workspace/listContexts", "lithe/host/workspace/getContext", "lithe/host/workspace/getFolders"],
      "notifications": ["lithe/host/workspace/didChangeContexts"]
    }
  },
  "limits": {
    "pendingHostRequests": 16,
    "functionResultBytes": 1048576,
    "hostRequestBytes": 1048576,
    "hostResultBytes": 1048576,
    "errorMessageBytes": 1024,
    "errorDetailsBytes": 4096,
    "notificationBytes": 1048576
  }
}
```

上例是声明了 `workspace.metadata` 的会话。`host`、`subscriptions`、`commands` 每个插件会话都有；`services` 只列本次会话可用的方法，
没声明的权限、宿主没有实现的服务都不会出现，所以列出的方法不会因为宿主缺少处理器而失败。数值以实际返回为准。`protocolVersion` 是协商到的 `litheFunctions` 版本。
一些服务条目还带 `options`，例如已授予编辑器动作或 Provider 时，`services.editor.options` 列出宿主绘制的图标和路由的 Provider 类型，`limits` 多一项 `editorActionSourceBytes`。
**按 `services` 判断能不能调用**，见第 10 节。

### 6. 工作区上下文

一个上下文就是“某个窗口里打开的某个工作区”：

```json
{
  "contextID": "w-3f2a9c",
  "generation": 2,
  "name": "orders",
  "authority": { "kind": "local" },
  "folders": [{ "rootID": "r0c1d2e3f4a5b6c7d", "name": "orders", "path": "/Users/me/orders" }],
  "stateDirectory": "/path/chosen/by/the/host/<digest>"
}
```

| 字段 | 怎么用 |
| --- | --- |
| `contextID` | 窗口显示这个工作区期间不变。窗口切到别的工作区时，旧上下文关闭、新上下文换新 ID，所以按 ID 缓存的状态不会串到别的工作区 |
| `generation` | 从 1 起；同一工作区的根目录或来源变化时加一。请求里带旧的 `generation`，或上下文已关闭，会得到 `plugin.host.contextExpired`，重新 `listContexts` 即可 |
| `rootID` | 同一位置跨重启稳定，不暴露路径；按插件加密派生，两个插件对同一目录看到的 ID 不同 |
| `path` | 只有本地工作区有。远程工作区（`authority.kind` 为 `remote`，另有不透明的 `authority.id`）没有 `path`，宿主在本地为它保留的目录只是空占位 |
| `stateDirectory` | 插件与工作区私有，在插件包和应用包之外，插件被禁用或更新后保留、卸载时删除。只有 `getContext` 返回它并在那时创建；`listContexts` 不创建目录 |

常用请求：

| 请求 | 参数 | 结果 |
| --- | --- | --- |
| `lithe/host/workspace/listContexts` | 无 | `{ revision, contexts }`，条目不含 `stateDirectory`；`revision` 在上下文打开、关闭或变化时增长 |
| `lithe/host/workspace/getContext` | `{ context: { contextID, generation } }` | 完整上下文 |
| `lithe/host/workspace/getFolders` | `{ context: { contextID, generation } }` | `{ folders }` |

页面调用后端函数时，宿主会按页面所在窗口填好上下文：后端在 `lithe/function/invoke` 的
`params.context` 里收到 `{ contextID, generation }`，直接拿它调 `getContext`。

### 7. 宿主通知

宿主用 JSON-RPC 通知告诉后端发生了变化，后端不应答：

| 通知 | 参数 | 什么时候来 |
| --- | --- | --- |
| `lithe/host/workspace/didChangeContexts` | `{ revision }` | 有上下文打开、关闭或变化；收到后重新 `listContexts` |
| `lithe/host/commands/didRun` | `{ commandID, context? }` | 用户运行了插件自己的命令。后端没在运行且声明了 `onCommand:<commandID>` 时，宿主先启动它，就绪后再送达 |
| `lithe/host/subscriptions/notify` | `{ subscriptionID, sequence, event }` | 某个订阅的事件。`sequence` 每个订阅从 1 起逐一递增，可以据此发现丢失；用 `lithe/host/subscriptions/dispose` 取消订阅 |
| `lithe/host/run/didExit` | `{ runID, exitCode?, signal? }` | 后端用 `run/start` 启动的运行结束了；只发给持有 `run.start` 的会话 |
| `lithe/host/languageServer/didChangeState`、`didReceiveNotification` | 见契约 | 本插件自己的语言服务器状态变化，或转发它的通知；需要 `languageServer.request` |

后端晚启动时不会补发之前的 `didChangeContexts`，启动后自己调一次 `listContexts`。

`didRun` 只是事件，不代表命令要做的事已经成功；宿主也不会为了送达它而打开一个空工具窗口。命令到达时若 `initialize` 还没完成，会等到就绪；崩溃的后端可以由它声明的命令事件再次启动。

#### 纯后端命令

```json
"requires": { "backend": 0, "ui": 0 },
"permissions": ["backend.launch", "ui.contribute"],
"activationEvents": ["onCommand:io.github.<你的用户名>.orders.refresh"],
"ui": {
  "toolWindows": [],
  "commands": [
    { "id": "io.github.<你的用户名>.orders.refresh", "title": "Refresh Orders", "placements": ["commandPalette"] }
  ]
}
```

不写 `toolWindowID` 的命令要求 `backend`、`ui.contribute` 与 `backend.launch` 权限，以及对应的 `onCommand:<id>`。
桌面端会呈现挂在工具窗口上的命令；不属于任何工具窗口的 `ui` 命令桌面端尚未呈现（见本页开头的宿主说明）。完整示例见
[`fixtures/backend/command-backend.plugin.json`](../contract/v1/fixtures/backend/command-backend.plugin.json)。

### 8. 文档、文件与窗口方法

宿主开放一组公共方法，每组对应一个权限。`getCapabilities.services` 只列出“清单权限推出的授权”与“本宿主已实现的方法”的交集，所以照第 10 节第 1 条先查再调。

| 方法（前缀 `lithe/host/`） | 权限 | 用途 |
| --- | --- | --- |
| `credentials/get`、`credentials/set`、`credentials/delete` | `credentials.store` | 插件私有的密钥，存在平台的密钥库里，不在状态目录或插件包中；`key` 是 1–128 个字母、数字、`.`、`_`、`:`、`-`，密钥最多 16 KiB；卸载插件时删除 |
| `documents/list`、`documents/getSnapshot`、`editor/getActive`、`editor/getSelections` | `documents.read` | 列出打开的文档，读编辑器里未保存的内容、当前标签页和选区 |
| `editor/applyEdits`、`editor/replaceSelections`、`editor/insertText` | `editor.edit` | 带 `expectedVersion` 的编辑；一批编辑是一次原子操作、一步撤销 |
| `workspace/readDirectory`、`workspace/stat`、`workspace/readFile` | `workspace.files.read` | 读磁盘上的目录和文件 |
| `workspace/watch` | `workspace.files.watch` | 订阅文件变化，事件经 `subscriptions/notify` 送达 |
| `workspace/findFiles`、`workspace/findText` | `workspace.search` | 按文件名或内容搜索，分页返回 |
| `editor/open`、`editor/reveal` | `editor.open` | 打开文件并定位到某处 |
| `preview/openHTML` | `preview.open` | 用内置预览打开 HTML 文件 |
| `browser/openExternal` | `browser.open` | 用系统浏览器打开 HTTP(S) 链接或 HTML 文件 |
| `window/showInformation`、`window/showWarning`、`window/showError` | `window.messages` | 显示消息，可带最多 3 个按钮，返回用户选了哪个 |

参数、结果和上限的权威定义在[契约](../contract/v1/README.md#public-document-file-and-window-methods)。写后端时要注意：

- 每个方法都要带 `context: { contextID, generation }`。文件用 `{ rootID, path }` 指明，`path` 用 `/`、相对于
  `getFolders` 返回的根目录。这组方法明确拒绝远程工作区（`authorityUnsupported`）。
- `workspace.metadata` 只给工作区信息，**不授权读文件**；读文件要另外声明 `workspace.files.read`。
- 位置 `{ line, column }` 从 1 开始、按 UTF-16 计；范围是半开区间。
- 单次读取最多返回 128 KiB：快照超出时得到 `limitExceeded`，读文件则用 `isTruncated` 标明截断。一批编辑或选区最多 1,000 项；目录分页默认 200 项、最多 1,000 项，搜索分页默认 100 项、最多 200 项；宿主侧的期限是 30 秒。
- 取消或超时**不代表已回滚**：编辑或打开可能已经生效，不要自动重放修改。遇到 `versionConflict` 先重新读取再计算；
  遇到 `contextExpired` 要重新找到原来的目标，不要改用当前聚焦的窗口；`busy`、`rateLimited` 可以有限次重试；
  `permissionDenied`、`readOnly` 需要用户处理。
- 后台没有挂载编辑器的文档可以列出和读取，但编辑与选区可能返回 `busy`。
- 消息有频率和排队上限（每插件 10 秒 5 条、最多 3 条待答；同一窗口最多 10 条；正文最多 2,000 个字符，按钮最多 3 个、每个最多 40 个字符），用户回答前请求一直挂起。

另有几组方法，各自带权限，调用前同样先查 `getCapabilities.services`，清单条件见对应的契约章节：

- 插件自有的编辑器动作（`editor.actions`）、编辑器语言功能 Provider（`editor.providers`）和原生终端（`terminal.create`），分别见[编辑器、文档与预览](../reference/domains/editor.md)和[从插件打开 Lithe 原生终端](native-terminals.md)。
- 运行与调试会话（权限 `run.start`、`debug.start`）：`run/start`、`run/stop` 启动并停止由宿主拥有的 Run 会话（Run 窗口条目、控制台、停止按钮、退出状态），`debug/start` 在本插件自己声明的调试适配器上启动调试会话，`run/didExit` 通知运行结束；见[插件后端与插件函数](../reference/domains/backend.md)、[调试适配器](../reference/domains/debug-adapters.md)和契约的[运行与调试会话](../contract/v1/README.md#run-and-debug-sessions)。
- 自有语言服务器（权限 `languageServer.request`，只对声明了 `requestMethods` 或 `forwardNotifications` 的服务器开放）：`languageServer/request`、`languageServer/notify` 向本插件自己声明的语言服务器发送请求和通知，`languageServer/setStatus` 在宿主状态界面显示它的准备状态；见[语言、语言服务器与运行/测试](../reference/domains/languages.md)和契约的[语言服务器宿主方法](../contract/v1/README.md#language-server-host-methods)。
- 内容 Provider（权限 `documents.contentProvider`）：`documents/registerContentProvider` 把一个 URI scheme 绑定到后端函数，用户打开该 scheme 的文档时宿主调用这个函数取只读内容；见[编辑器、文档与预览](../reference/domains/editor.md)和契约的[内容 Provider](../contract/v1/README.md#content-providers)。

官方 SDK 为凭据、工作区、文档、编辑器、预览、浏览器、窗口消息、命令、订阅和跨插件调用提供了封装；`run/*`、`debug/start`、`languageServer/*`、`documents/registerContentProvider` 没有专门的封装，用 SDK 的通用宿主请求调用。

### 9. 宿主失败与错误码

宿主失败带类型：JSON-RPC 号码与 `error.data.code` 一一对应，号码永不改义，部分失败还带 `error.data.details`。

| `error.data.code` | 号码 | 后端通常怎么做 |
| --- | --- | --- |
| `plugin.host.unavailable` | `-32601` | 宿主没有这个方法；先查 `getCapabilities` |
| `plugin.host.invalidParams` | `-32602` | 参数格式错误，修代码 |
| `plugin.host.failed` | `-32603` | 其他失败，报告给用户 |
| `plugin.host.cancelled` | `-32800` | 你自己用 `$/cancelRequest` 取消了它 |
| `plugin.host.permissionDenied` | `-32010` | 没声明权限，或用户拒绝 |
| `plugin.host.contextExpired` | `-32011` | 重新 `listContexts` 后再试 |
| `plugin.host.authorityUnsupported` | `-32012` | 该服务不支持远程工作区，宿主不会改在本地执行 |
| `plugin.host.versionConflict` | `-32013` | 资源版本已变；`details.currentVersion` 给出当前版本 |
| `plugin.host.limitExceeded` | `-32014` | 超出限制，例如同时挂起的请求超过 `limits.pendingHostRequests` |
| `plugin.host.timeout` | `-32015` | 宿主超时 |
| `plugin.host.notFound` | `-32016` | 资源不存在 |
| `plugin.host.readOnly` | `-32017` | 资源或工作区不可写 |
| `plugin.host.unsupportedOption` | `-32018` | 宿主不认识请求里的某个字段；`details.option` 指明是哪个，去掉它重试 |
| `plugin.host.preconditionFailed` | `-32019` | 所需状态不成立，例如用户没有确认 |
| `plugin.host.busy` | `-32020` | 资源正被占用，稍后重试 |
| `plugin.host.rateLimited` | `-32021` | 请求太频繁；有 `details.retryAfterMilliseconds` 时按它等待 |
| `plugin.host.languageServerUnavailable` | `-32022` | 本插件的语言服务器在这个上下文里没有运行，或请求进行中它停止了；等 `didChangeState` 报告新的 `generation` 再试 |
| `plugin.host.languageServerError` | `-32023` | 语言服务器用 JSON-RPC 错误应答；`details` 是 `{code, message}` |
| `plugin.host.methodNotDeclared` | `-32024` | 方法或 `workspace/executeCommand` 的命令不在服务器声明的会话范围内 |
| `plugin.host.toolUnresolved` | `-32025` | 请求里的工具定义在这个工作区解析不到可执行文件；`details` 是 `{ tool, reason }` |
| `plugin.host.foreignContribution` | `-32026` | 指名的服务器或贡献属于别的插件 |

`-32000` 到 `-32009` 留给后端自己的失败，宿主号码从 `-32010` 起。跨插件调用另有 `plugin.function.failed`（`-32070`）和 `plugin.function.invalidResult`（`-32071`），见第 11 节。

一个会话最多同时有 16 个未应答的宿主请求，第 17 个得到 `plugin.host.limitExceeded`。后端可以用 `$/cancelRequest` 取消自己挂起的宿主请求，Core 立刻回 `-32800`。

### 10. 宿主服务只增不改：后端要遵守的四条

宿主再开放新能力（新服务及其方法和通知、可选参数、结果字段、错误码）都在同一版本内新增，不升版本；只有会让已有后端读错的改动（删除或改名、改字段含义或类型、把可选参数改成必填、改错误码号码、削弱顺序或生命周期保证）
才会出新的版本（`backend` 版本 `1`，协议随它一起变）。这样插件只要按下面四条实现一次，就能在今后支持这一版本的所有宿主上工作：

1. **只调用 `getCapabilities.services` 列出的方法。** 其他方法会得到 `-32601` `plugin.host.unavailable`。
2. **忽略不认识的字段和通知。** 结果、通知、`details`、`limits` 里多出来的字段，以及不认识的 `lithe/host/*` 通知，都直接忽略。
3. **新选项不被支持时会得到明确回答。** 请求里带了宿主不认识的字段，Core 在宿主看到请求之前回
   `-32018` `plugin.host.unsupportedOption`，`details: { "option": "<字段名>" }`。宿主不会悄悄忽略它，
   你可以去掉这个选项重试，或告诉用户需要更新 Lithe。
4. **不认识的号码按宿主失败处理。** 新宿主可能带来你没见过的错误码；认识 `error.data.code` 时按它分支，否则当作一般失败。

新服务总是配一个新的清单权限。不认识这个权限的宿主会在安装时以 `plugin.permission.unknown` 拒绝整个插件包，
权限不会被悄悄授予或忽略。

### 11. 跨插件调用

插件函数默认只属于插件自己。导出函数后，另一个已启用、且在清单里声明了这次调用的插件可以经 Core 调用它：调用方接触不到被调方的进程，被调方的后端也只知道自己正在服务的那次调用。完整规则见[插件后端与插件函数](../reference/domains/backend.md#跨插件调用)和契约的[Cross-plugin calls](../contract/v1/README.md#cross-plugin-calls)，这里只列要点：

- **清单。** 声明 `pluginFunctions`：`requires` 里写 `pluginFunctions: 0` 和 `backend: 0`；`exports` 的每一项是 `{ id, version, context: "workspace" }`，`calls` 的每一项是 `{ id, version }`。它不授予任何清单权限：调用方的 `calls` 和被调方的 `exports` 就是双方的同意。不支持 `pluginFunctions` 的宿主以 `plugin.api.unsupported` 拒绝这类清单。
- **注册。** 声明导出不等于注册：后端仍要用 `lithe/function/register` 注册该函数。只导出函数的插件不需要网页、命令或 `onWorkspaceOpen`，被调用时启动后端；只调用的插件仍要有其中一种启动方式。
- **调用方的两个中转方法。** 先用 `functions/providers` 找到导出某个函数的插件，再用 `functions/invoke` 调用其中一个。调用方没有声明任何 `calls` 会得到 `-32010` `plugin.host.permissionDenied`。
- **被调方收到的 `caller`。** 中转的 `lithe/function/invoke` 顶层多一个 `caller: { pluginID }`，由 Core 按调用方会话写入；没有这个字段就表示宿主发起的调用。要按调用方判断的被调方只读这个顶层字段，不读 `params` 里自称的插件 ID。
- **嵌套调用。** 被调方在处理中转调用时再发起调用，要把这次调用顶层的 `operationID` 填进 `parentOperationID`，Core 才知道嵌套调用属于哪条调用链；调用链最长 4 次，调用链上已有的插件不能再被调用（`plugin.function.callCycle`）。
- **信任不传递。** 被调方不得接受调用方给的可执行路径或命令行去启动进程。
- 远程工作区的上下文被拒绝（`plugin.host.authorityUnsupported`）。

官方 SDK 封装了 `functions/providers`、`functions/invoke`，被调函数能读到 `caller`；`parentOperationID` 不传时，SDK 取正在处理的 `lithe/function/invoke` 的 `operationID`。
下面以 Node.js 为例，[Java](../sdk/java/README.md#calling-other-plugins) 和 [Rust](../sdk/rust/README.md#calling-other-plugins) 的对应方法见各自 README：

```js
// Caller. The manifest declares pluginFunctions.calls for document.format.
backend.function('format.current', async (params, context) => {
  const workspace = context.workspaceContext;
  if (!workspace || !await context.host.supports('lithe/host/functions/invoke')) {
    throw new PluginError('format.unavailable', 'Cross-plugin calls are unavailable.');
  }
  const { providers } = await context.host.functions.providers('document.format');
  if (providers.length === 0) return null;
  const { value } = await context.host.functions.invoke(workspace, {
    pluginID: providers[0].pluginID,
    functionID: 'document.format',
    params: { path: params.path },
    timeoutMilliseconds: 30000,
  });
  return value;
});

// Provider. `caller` is { pluginID } for a relayed call and null when the host started the call.
backend.function('document.format', async (params, context) => {
  const callerID = context.caller?.pluginID ?? null;
  return formatDocument(params, callerID);
});
```

### 12. 验证

- 用契约里的 [`fixtures/backend/function-protocol.json`](../contract/v1/fixtures/backend/function-protocol.json) 核对你的实现：
  `hostRequests` 有不认识的选项等请求用例，`invocationContexts` 有调用上下文，`invocationCallers` 有被中转调用的 `caller`，
  `hostFailures` 有失败码映射，`relayErrors` 有中转失败码，`hostNotifications` 有通知用例。
- 清单示例是 [`fixtures/backend/backend-workspace.plugin.json`](../contract/v1/fixtures/backend/backend-workspace.plugin.json)；
  纯后端命令的示例是 [`fixtures/backend/command-backend.plugin.json`](../contract/v1/fixtures/backend/command-backend.plugin.json)；
  跨插件调用的示例是 [`fixtures/backend/plugin-functions-exporter.plugin.json`](../contract/v1/fixtures/backend/plugin-functions-exporter.plugin.json) 和
  [`fixtures/backend/plugin-functions-caller.plugin.json`](../contract/v1/fixtures/backend/plugin-functions-caller.plugin.json)；
  [`fixtures/manifest/invalid-manifests.json`](../contract/v1/fixtures/manifest/invalid-manifests.json) 里的 backend 用例是会被拒绝的写法。
- 官方 SDK 的示例后端可以在本仓库根目录用 `node shared/plugins/scripts/sdk-e2e.mjs` 对当前工作树的 Core 做端到端检查；
  文档和契约的离线检查是 `node shared/plugins/scripts/verify-docs.mjs`。

---

## English

This guide is for authors writing a plugin backend (`backend`): start without a page, know which workspaces the user has open, respond to commands, and call host services when needed, including functions that other plugins export.

The [contract](../contract/v1/README.md#backend-lifecycle-and-workspace-contexts) is normative for fields and error codes; this guide covers how to use them and why they are designed this way.

> **Host support.** The Lithe desktop declares `backend` and `pluginFunctions` (both version `0`), so the function protocol, host services, and cross-plugin calls in this guide run there. The host grants only the services it implements: which methods a session can call is decided by `services` in the run-time `lithe/host/getCapabilities` answer, so do not assume it from this document (sections 5 and 10). The desktop's declarations and notes are in the comment above `Declared()` in
> [`pluginhost.go`](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go).
> All three official SDKs implement the protocol in this guide: Node.js SDK 0.5.0, Java SDK 0.5.0, and Rust SDK 0.4.0
> (see Host services in each README: [Node.js](../sdk/node/README.md#host-services), [Java](../sdk/java/README.md#host-services),
> [Rust](../sdk/rust/README.md#host-services); release pages: [sdk-node-v0.5.0](https://github.com/Lithe-IDEA/Lithe-core-rust/releases/tag/sdk-node-v0.5.0),
> [sdk-java-v0.5.0](https://github.com/Lithe-IDEA/Lithe-core-rust/releases/tag/sdk-java-v0.5.0),
> [sdk-rust-v0.4.0](https://github.com/Lithe-IDEA/Lithe-core-rust/releases/tag/sdk-rust-v0.4.0)). With another language, handle the requests and notifications below yourself.
> For how far Core and the desktop support each capability, see the [capability and protocol versions](../reference/capabilities-and-versions.md). Questions about the contract go to
> [Lithe-core-rust Issues](https://github.com/Lithe-IDEA/Lithe-core-rust/issues).

### 1. What the three mechanisms control

| Where | Value | Controls |
| --- | --- | --- |
| `requires.backend` | `0` | Whether the plugin installs, **when** the backend process starts, and whether it may declare `workspace.metadata` and `onWorkspaceOpen` |
| `litheFunctions.version` in the `initialize` answer | `0` | The **wire protocol** once the process runs: function registration and invocation, host services, capability discovery, workspace contexts, host notifications, typed failure codes, and cross-plugin calls. The protocol has no version of its own; it is the version of `backend` |
| `getCapabilities.services` | Runtime query result | Which host methods this session **can actually call**, and which notifications it receives |

The manifest only declares. The host reads it to decide whether to install and when to start; what the running process can do is decided by negotiation and `getCapabilities`.

### 2. Minimal manifest

```json
{
  "schemaVersion": 0,
  "id": "io.github.<your-username>.orders",
  "displayName": "Orders",
  "version": "1.0.0",
  "pluginAPIVersion": 0,
  "requires": { "backend": 0 },
  "vendor": { "id": "io.github.<your-username>", "displayName": "<your name>", "signatureRequirement": "none" },
  "permissions": ["backend.launch", "workspace.metadata"],
  "activationEvents": ["onWorkspaceOpen"],
  "languages": [],
  "backend": {
    "launch": { "kind": "packaged", "path": "bin/backend" },
    "arguments": ["--state", "${stateDir}"]
  }
}
```

- `requires` must list exactly the capabilities the manifest uses. Starting a process ([Process start](../contract/v1/README.md#process-start)) is part of the base contract, not a capability, and is not written in `requires`.
- A `backend` needs at least one way to start: a web tool window (the first page call), a command in `ui.commands[]` (`onCommand:<id>` also starts the backend), `onWorkspaceOpen`, or a provider the host queries (a toolchain provider, a keymap import provider, or a language server whose `launch.kind` is `backend`, which asks the backend for its command line when it starts). Having none is `plugin.manifest.invalid`.
- For commands without a tool window, add `ui: 0` to `requires`: commands may omit `toolWindowID`, and `toolWindows` may be empty or omitted; see section 7.
- `workspace.metadata` only tells the backend the open workspaces' names, roots, IDs, authority (local or remote), and each workspace's private state directory;
  it reads no file contents. Do not declare it unless you need this information; installation review lists it to the user.
- `${workspaceStateDir}` is refused: a plugin has one backend process shared by every window, so a workspace is not process state.
  Take each workspace's state directory from `stateDirectory` in `getContext`; the plugin-wide state directory is `${stateDir}`, which is also the backend's start root and working directory.
- A plugin publishes one archive per platform, so the packaged path does not vary by `${arch}`.

### 3. When the backend starts

- A plugin has at most one backend process, shared by every workspace and window, running on the machine that runs Lithe, for remote workspaces too.
- `onWorkspaceOpen` does not slow down opening a workspace. The host starts these backends one by one after the window appears; a backend whose `initialize` times out is stopped
  and reported like any other start failure, without blocking the workspace. A failed or disabled backend never blocks the workspace either.
- The backend ends when the plugin is disabled, updated, or uninstalled, and when the host quits.
- Activation only checks that the plugin is installed, enabled, and approved in review. Lithe has no workspace trust state yet; the host asks the user each time before an operation runs or changes something.
- A backend serves only its own plugin: it never answers `lithe/build/*` or VCS requests. A build-only backend (one that does not answer `litheFunctions`) receives no function requests, and those get `-32601`.

### 4. Negotiating the protocol

The host offers `capabilities.experimental.litheFunctions: { "versions": [0] }` in `initialize`. A backend that implements the protocol answers:

```json
"experimental": { "litheFunctions": { "version": 0, "functions": [{ "id": "orders.refresh" }] } }
```

`functions` is optional; leaving it out means no private functions, and you can add or remove them later with `lithe/function/register` and `lithe/function/unregister`. An answer with a version the host did not offer, or with a malformed or duplicate function ID, does not negotiate the protocol at all, so a session never starts with half a registry. Both sides ignore unknown fields.

A function ID is dot-separated lowerCamelCase segments (`orders.refresh`), at most 128 bytes, and a session holds at most 256 functions.

### 5. Ask the host what it can do first

Once the protocol is negotiated, first call `lithe/host/getCapabilities` (no params, or `{}`):

```json
{
  "protocolVersion": 0,
  "backendAuthority": "local",
  "services": {
    "commands": { "methods": [], "notifications": ["lithe/host/commands/didRun"] },
    "host": { "methods": ["lithe/host/getCapabilities"], "notifications": [] },
    "subscriptions": {
      "methods": ["lithe/host/subscriptions/dispose"],
      "notifications": ["lithe/host/subscriptions/notify"]
    },
    "workspace": {
      "methods": ["lithe/host/workspace/listContexts", "lithe/host/workspace/getContext", "lithe/host/workspace/getFolders"],
      "notifications": ["lithe/host/workspace/didChangeContexts"]
    }
  },
  "limits": {
    "pendingHostRequests": 16,
    "functionResultBytes": 1048576,
    "hostRequestBytes": 1048576,
    "hostResultBytes": 1048576,
    "errorMessageBytes": 1024,
    "errorDetailsBytes": 4096,
    "notificationBytes": 1048576
  }
}
```

The example is a session that declared `workspace.metadata`. Every plugin session has `host`, `subscriptions`, and `commands`; `services` lists only the methods this session may use,
so undeclared permissions and services the host does not implement never appear, and a listed method never fails for want of a handler. Treat the numbers as whatever the host returns. `protocolVersion` is the negotiated `litheFunctions` version.
Some service entries also carry `options`: when editor actions or providers are granted, `services.editor.options` lists the icons the host draws and the provider types it routes, and `limits` gains `editorActionSourceBytes`.
**Decide what you can call from `services`**; see section 10.

### 6. Workspace contexts

A context is "one workspace open in one window":

```json
{
  "contextID": "w-3f2a9c",
  "generation": 2,
  "name": "orders",
  "authority": { "kind": "local" },
  "folders": [{ "rootID": "r0c1d2e3f4a5b6c7d", "name": "orders", "path": "/Users/me/orders" }],
  "stateDirectory": "/path/chosen/by/the/host/<digest>"
}
```

| Field | How to use it |
| --- | --- |
| `contextID` | Stable while the window shows this workspace. When the window switches to another workspace, the old context closes and the new one gets a new ID, so state cached by ID never leaks into another workspace |
| `generation` | Starts at 1 and increments when the same workspace's roots or authority change. A request with a stale `generation`, or for a closed context, gets `plugin.host.contextExpired`; call `listContexts` again |
| `rootID` | Stable for the same location across restarts and does not reveal the path; derived cryptographically per plugin, so two plugins see different IDs for the same folder |
| `path` | Present only for local workspaces. A remote workspace (`authority.kind` is `remote`, with an opaque `authority.id`) has no `path`, and the local directory the host keeps for it is only an empty placeholder |
| `stateDirectory` | Private to the plugin and the workspace, outside the plugin package and the app bundle; it survives disabling and updating the plugin and is deleted on uninstall. Only `getContext` returns it, creating it at that moment; `listContexts` creates no directory |

Common requests:

| Request | Params | Result |
| --- | --- | --- |
| `lithe/host/workspace/listContexts` | none | `{ revision, contexts }`; entries have no `stateDirectory`; `revision` grows whenever a context opens, closes, or changes |
| `lithe/host/workspace/getContext` | `{ context: { contextID, generation } }` | The full context |
| `lithe/host/workspace/getFolders` | `{ context: { contextID, generation } }` | `{ folders }` |

When a page calls a backend function, the host fills in the context of the page's window: the backend receives `{ contextID, generation }`
in `params.context` of `lithe/function/invoke` and can pass it straight to `getContext`.

### 7. Host notifications

The host tells the backend about changes with JSON-RPC notifications; the backend does not answer them:

| Notification | Params | When it arrives |
| --- | --- | --- |
| `lithe/host/workspace/didChangeContexts` | `{ revision }` | A context opened, closed, or changed; call `listContexts` again |
| `lithe/host/commands/didRun` | `{ commandID, context? }` | The user ran one of the plugin's own commands. If the backend is not running and declares `onCommand:<commandID>`, the host starts it first and delivers the notification once it is ready |
| `lithe/host/subscriptions/notify` | `{ subscriptionID, sequence, event }` | An event of a subscription. `sequence` starts at 1 per subscription and increments by one, so gaps reveal losses; cancel with `lithe/host/subscriptions/dispose` |
| `lithe/host/run/didExit` | `{ runID, exitCode?, signal? }` | A run the backend started with `run/start` ended; sent only to a session holding `run.start` |
| `lithe/host/languageServer/didChangeState`, `didReceiveNotification` | See the contract | State changes of the plugin's own language server, or a notification forwarded from it; needs `languageServer.request` |

A backend that starts late gets no replay of earlier `didChangeContexts`; call `listContexts` once after starting.

`didRun` is only an event and does not mean the command's work succeeded; the host also never opens an empty tool window just to deliver it. A command that arrives while `initialize` is pending waits until the backend is ready, and a crashed backend can start again on its declared command event.

#### Backend-only commands

```json
"requires": { "backend": 0, "ui": 0 },
"permissions": ["backend.launch", "ui.contribute"],
"activationEvents": ["onCommand:io.github.<your-username>.orders.refresh"],
"ui": {
  "toolWindows": [],
  "commands": [
    { "id": "io.github.<your-username>.orders.refresh", "title": "Refresh Orders", "placements": ["commandPalette"] }
  ]
}
```

A command without `toolWindowID` requires `backend`, the `ui.contribute` and `backend.launch` permissions, and the matching `onCommand:<id>`.
The desktop presents commands that belong to a tool window; `ui` commands that belong to no tool window are not presented on the desktop yet (see the host note at the top of this page). For a complete example, see
[`fixtures/backend/command-backend.plugin.json`](../contract/v1/fixtures/backend/command-backend.plugin.json).

### 8. Document, file, and window methods

The host opens a set of public methods, each group behind a permission. `getCapabilities.services` lists only the intersection of "grants derived from the manifest's permissions" and "methods this host implements", so check before calling, as rule 1 of section 10 says.

| Method (prefix `lithe/host/`) | Permission | Purpose |
| --- | --- | --- |
| `credentials/get`, `credentials/set`, `credentials/delete` | `credentials.store` | Secrets private to the plugin, kept in the platform's secret store, never in the state directory or the package; a `key` is 1 to 128 letters, digits, `.`, `_`, `:`, or `-`, a secret is at most 16 KiB; deleted when the plugin is uninstalled |
| `documents/list`, `documents/getSnapshot`, `editor/getActive`, `editor/getSelections` | `documents.read` | List open documents; read unsaved editor content, the active tab, and selections |
| `editor/applyEdits`, `editor/replaceSelections`, `editor/insertText` | `editor.edit` | Edits with `expectedVersion`; a batch is one atomic operation and one undo step |
| `workspace/readDirectory`, `workspace/stat`, `workspace/readFile` | `workspace.files.read` | Read directories and files on disk |
| `workspace/watch` | `workspace.files.watch` | Subscribe to file changes; events arrive through `subscriptions/notify` |
| `workspace/findFiles`, `workspace/findText` | `workspace.search` | Search by file name or content, paged |
| `editor/open`, `editor/reveal` | `editor.open` | Open a file and go to a location |
| `preview/openHTML` | `preview.open` | Open an HTML file in the built-in preview |
| `browser/openExternal` | `browser.open` | Open an HTTP(S) link or HTML file in the system browser |
| `window/showInformation`, `window/showWarning`, `window/showError` | `window.messages` | Show a message with up to 3 buttons and return which one the user chose |

The [contract](../contract/v1/README.md#public-document-file-and-window-methods) is normative for params, results, and limits. When writing a backend:

- Every method takes `context: { contextID, generation }`. Files are named by `{ rootID, path }`, where `path` uses `/` and is relative to
  a root returned by `getFolders`. These methods refuse remote workspaces explicitly (`authorityUnsupported`).
- `workspace.metadata` only gives workspace information and **does not grant file reads**; declare `workspace.files.read` separately to read files.
- Positions `{ line, column }` start at 1 and count UTF-16 units; ranges are half-open.
- One read returns at most 128 KiB: an oversized snapshot gets `limitExceeded`, and a file read marks truncation with `isTruncated`. A batch of edits or selections has at most 1,000 items; a directory page defaults to 200 items and holds at most 1,000, a search page defaults to 100 and holds at most 200; the host-side deadline is 30 seconds.
- Cancellation or timeout **does not mean rollback**: an edit or open may already have taken effect, so do not replay changes automatically. On `versionConflict`, reread and recompute;
  on `contextExpired`, find the original target again instead of switching to the focused window; `busy` and `rateLimited` may be retried a bounded number of times;
  `permissionDenied` and `readOnly` need the user.
- Documents open in the background without a mounted editor can be listed and read, but edits and selections may return `busy`.
- Messages have rate and queue limits (5 per 10 seconds per plugin, at most 3 awaiting an answer, at most 10 per window; text of at most 2,000 characters, at most 3 buttons of at most 40 characters each), and the request stays pending until the user answers.

Several more groups of methods, each behind its own permission, also need a `getCapabilities.services` check before calling; their manifest conditions are in the contract sections named below:

- The plugin's own editor actions (`editor.actions`), editor language-feature providers (`editor.providers`), and native terminals (`terminal.create`); see [Editor, documents, and previews](../reference/domains/editor.md) and [Open a native Lithe terminal from a plugin](native-terminals.md).
- Run and debug sessions (permissions `run.start`, `debug.start`): `run/start` and `run/stop` start and stop a host-owned Run session (Run window entry, console, Stop button, exit status), `debug/start` starts a debug session on a debug adapter the plugin itself declares, and the `run/didExit` notification reports that a run ended; see [Plugin backend and plugin functions](../reference/domains/backend.md), [Debug adapters](../reference/domains/debug-adapters.md), and the contract's [Run and debug sessions](../contract/v1/README.md#run-and-debug-sessions).
- Own language servers (permission `languageServer.request`, open only for servers that list `requestMethods` or `forwardNotifications`): `languageServer/request` and `languageServer/notify` send requests and notifications to a language server the plugin itself declares, and `languageServer/setStatus` shows its preparation state in the host's status UI; see [Languages, language servers, run and test](../reference/domains/languages.md) and the contract's [Language server host methods](../contract/v1/README.md#language-server-host-methods).
- Content providers (permission `documents.contentProvider`): `documents/registerContentProvider` binds a URI scheme to a backend function, and the host calls that function for read-only content when the user opens a document of that scheme; see [Editor, documents, and previews](../reference/domains/editor.md) and the contract's [Content providers](../contract/v1/README.md#content-providers).

The official SDKs wrap credentials, workspaces, documents, editors, preview, the browser, window messages, commands, subscriptions, and cross-plugin calls; `run/*`, `debug/start`, `languageServer/*`, and `documents/registerContentProvider` have no dedicated wrappers, so call them with the SDK's generic host request.

### 9. Host failures and error codes

Host failures are typed: each JSON-RPC number maps one-to-one to an `error.data.code`, a number never changes meaning, and some failures carry `error.data.details`.

| `error.data.code` | Number | What a backend usually does |
| --- | --- | --- |
| `plugin.host.unavailable` | `-32601` | The host has no such method; check `getCapabilities` first |
| `plugin.host.invalidParams` | `-32602` | Malformed params; fix the code |
| `plugin.host.failed` | `-32603` | Any other failure; report it to the user |
| `plugin.host.cancelled` | `-32800` | You cancelled it yourself with `$/cancelRequest` |
| `plugin.host.permissionDenied` | `-32010` | The permission is not declared, or the user refused |
| `plugin.host.contextExpired` | `-32011` | Call `listContexts` again, then retry |
| `plugin.host.authorityUnsupported` | `-32012` | The service does not support remote workspaces, and the host will not run it locally instead |
| `plugin.host.versionConflict` | `-32013` | The resource version changed; `details.currentVersion` gives the current version |
| `plugin.host.limitExceeded` | `-32014` | A limit was exceeded, for example more pending requests than `limits.pendingHostRequests` |
| `plugin.host.timeout` | `-32015` | The host timed out |
| `plugin.host.notFound` | `-32016` | The resource does not exist |
| `plugin.host.readOnly` | `-32017` | The resource or workspace is not writable |
| `plugin.host.unsupportedOption` | `-32018` | The host does not know a field in the request; `details.option` names it, so drop it and retry |
| `plugin.host.preconditionFailed` | `-32019` | A required state does not hold, for example the user did not confirm |
| `plugin.host.busy` | `-32020` | The resource is in use; retry later |
| `plugin.host.rateLimited` | `-32021` | Too many requests; wait `details.retryAfterMilliseconds` when given |
| `plugin.host.languageServerUnavailable` | `-32022` | The plugin's language server is not running for the context, or stopped while the request was in flight; retry after `didChangeState` reports a new `generation` |
| `plugin.host.languageServerError` | `-32023` | The language server answered with a JSON-RPC error; `details` is `{code, message}` |
| `plugin.host.methodNotDeclared` | `-32024` | The method, or the `workspace/executeCommand` command, is not in the server's declared session |
| `plugin.host.toolUnresolved` | `-32025` | A tool definition the request names resolves to no executable for this workspace; `details` is `{ tool, reason }` |
| `plugin.host.foreignContribution` | `-32026` | The named server or contribution belongs to another plugin |

`-32000` through `-32009` are reserved for the backend's own failures; host numbers start at `-32010`. Cross-plugin calls add `plugin.function.failed` (`-32070`) and `plugin.function.invalidResult` (`-32071`); see section 11.

A session has at most 16 unanswered host requests, and the 17th gets `plugin.host.limitExceeded`. A backend can cancel its own pending host request with `$/cancelRequest`, and Core answers it at once with `-32800`.

### 10. Host services only grow: four rules for backends

New host capabilities (new services with their methods and notifications, optional params, result fields, error codes) are added within the same version without raising it; only changes that would make an existing backend misread something (removing or renaming, changing a field's meaning or type, making an optional param required, changing an error number, weakening an ordering or lifecycle guarantee)
lead to a new version (`backend` version `1`, with the protocol moving with it). So a plugin that follows these four rules once works on every future host that supports this version:

1. **Call only methods listed in `getCapabilities.services`.** Any other method gets `-32601` `plugin.host.unavailable`.
2. **Ignore unknown fields and notifications.** Ignore extra fields in results, notifications, `details`, and `limits`, and any unknown `lithe/host/*` notification.
3. **An unsupported new option gets an explicit answer.** If a request carries a field the host does not know, Core answers
   `-32018` `plugin.host.unsupportedOption` with `details: { "option": "<field name>" }` before the host sees the request. The host never silently ignores it;
   drop the option and retry, or tell the user to update Lithe.
4. **Treat unknown numbers as host failures.** A newer host may bring error codes you have not seen; branch on `error.data.code` when you know it, and otherwise treat it as a general failure.

A new service always comes with a new manifest permission. A host that does not know the permission refuses the whole plugin package at installation with `plugin.permission.unknown`,
so the permission is never silently granted or ignored.

### 11. Cross-plugin calls

A plugin function is private to its plugin unless the plugin exports it. An exported function can be called, through Core, by another enabled plugin whose manifest declares that call: the caller never reaches the provider's process, and the provider's backend never learns more than the call it serves. The full rules are in [Plugin backend and plugin functions](../reference/domains/backend.md#cross-plugin-calls) and the contract's [Cross-plugin calls](../contract/v1/README.md#cross-plugin-calls); the essentials:

- **Manifest.** Declare `pluginFunctions`: `requires` lists `pluginFunctions: 0` and `backend: 0`; each `exports` entry is `{ id, version, context: "workspace" }` and each `calls` entry is `{ id, version }`. It grants no manifest permission: the caller's `calls` and the provider's `exports` are the consent of the two sides. A host without `pluginFunctions` refuses such a manifest with `plugin.api.unsupported`.
- **Registration.** Declaring an export does not register it: the backend still registers the function with `lithe/function/register`. A plugin that only exports needs no page, command, or `onWorkspaceOpen`, because a call starts its backend; a plugin that only calls still needs one of them.
- **Two relay methods for the caller.** It finds the plugins that export a function with `functions/providers`, then calls one of them with `functions/invoke`. A plugin that declares no `calls` gets `-32010` `plugin.host.permissionDenied`.
- **`caller` in a relayed call.** The `lithe/function/invoke` a provider receives has one more top-level field, `caller: { pluginID }`, which Core writes from the calling session; its absence means the host started the call. A provider that decides by who is asking reads only this top-level field, never a plugin ID that `params` claims.
- **Nested calls.** When a provider makes a call while serving a relayed call, it puts that call's top-level `operationID` in `parentOperationID`, so Core knows which call chain the new call belongs to. A chain has at most 4 calls, and a plugin already on the chain cannot be called again (`plugin.function.callCycle`).
- **Trust is not transitive.** A provider must not accept an executable path or command line from its caller and start a process with it.
- A call whose context is a remote workspace is refused (`plugin.host.authorityUnsupported`).

The official SDKs wrap `functions/providers` and `functions/invoke` and expose `caller` to the called function; when `parentOperationID` is omitted, the SDK uses the `operationID` of the `lithe/function/invoke` being handled.
The example below uses Node.js; for the matching methods in [Java](../sdk/java/README.md#calling-other-plugins) and [Rust](../sdk/rust/README.md#calling-other-plugins), see their READMEs:

```js
// Caller. The manifest declares pluginFunctions.calls for document.format.
backend.function('format.current', async (params, context) => {
  const workspace = context.workspaceContext;
  if (!workspace || !await context.host.supports('lithe/host/functions/invoke')) {
    throw new PluginError('format.unavailable', 'Cross-plugin calls are unavailable.');
  }
  const { providers } = await context.host.functions.providers('document.format');
  if (providers.length === 0) return null;
  const { value } = await context.host.functions.invoke(workspace, {
    pluginID: providers[0].pluginID,
    functionID: 'document.format',
    params: { path: params.path },
    timeoutMilliseconds: 30000,
  });
  return value;
});

// Provider. `caller` is { pluginID } for a relayed call and null when the host started the call.
backend.function('document.format', async (params, context) => {
  const callerID = context.caller?.pluginID ?? null;
  return formatDocument(params, callerID);
});
```

### 12. Verification

- Check your implementation against the contract's [`fixtures/backend/function-protocol.json`](../contract/v1/fixtures/backend/function-protocol.json):
  `hostRequests` has request cases such as unknown options, `invocationContexts` has invocation contexts, `invocationCallers` has the `caller` of a relayed call,
  `hostFailures` has the failure code mapping, `relayErrors` has the relay failure codes, and `hostNotifications` has notification cases.
- The manifest example is [`fixtures/backend/backend-workspace.plugin.json`](../contract/v1/fixtures/backend/backend-workspace.plugin.json);
  the backend-only command example is [`fixtures/backend/command-backend.plugin.json`](../contract/v1/fixtures/backend/command-backend.plugin.json);
  the cross-plugin call examples are [`fixtures/backend/plugin-functions-exporter.plugin.json`](../contract/v1/fixtures/backend/plugin-functions-exporter.plugin.json) and
  [`fixtures/backend/plugin-functions-caller.plugin.json`](../contract/v1/fixtures/backend/plugin-functions-caller.plugin.json);
  the backend cases in [`fixtures/manifest/invalid-manifests.json`](../contract/v1/fixtures/manifest/invalid-manifests.json) are the forms that are refused.
- From the repository root, `node shared/plugins/scripts/sdk-e2e.mjs` runs the official SDKs' sample backends against this working tree's Core;
  `node shared/plugins/scripts/verify-docs.mjs` is the offline check of the documents and the contract.
