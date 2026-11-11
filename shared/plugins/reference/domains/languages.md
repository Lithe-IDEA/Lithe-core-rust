# 语言、语言服务器与运行/测试 / Languages, language servers, run and test

[中文](#中文) · [English](#english)

## 中文

让 Lithe 认识一种语言：识别文件，用包内 TextMate 语法高亮，启动 LSP 语言服务器，输出链接，以及（如果插件提供执行提供者）“运行当前文件”和测试。这些都是数据声明，宿主负责进程生命周期、LSP 会话和停止进程，插件不需要写宿主代码。命令行要到启动时才能确定、或后端需要驱动自己的服务器时，可以让插件后端回答启动命令，并在自己声明的范围内向服务器发请求（见“后端决定启动方式”和“插件自有会话”）。

| 项目 | 内容 |
| --- | --- |
| 清单字段 | `languages`、`languageServers`、`executionProviders`、`outputLinkMatchers` |
| 能力 | `languages`、`languageServers`、`executionProviders`、`outputLinkMatchers`，版本都是 `0`。`languageServers` 的后端启动方式需要 `backend`；会话、输出链接的 `resolve`、内容提供者都需要 `backend` |
| 协议 | LSP。后端驱动服务器用的 `languageServer/*` 宿主方法和通知见下文，它们属于插件后端的 `litheFunctions` 协议（见[插件后端](backend.md)） |
| API 编号 | [API-25](../api-catalog.md#现在能用的-api)：语言、语法、LSP。[API-12](../api-catalog.md#现在能用的-api)：按 URI scheme 提供的只读文档（`documents/registerContentProvider`）。[API-20](../api-catalog.md#现在能用的-api)：语言服务器按工作区的状态目录及其回收。[API-22](../api-catalog.md#现在能用的-api)：语言服务器的准备状态（`languageServer/setStatus`）和 Run、测试控制台输出里的链接。插件函数提供的编辑器语言功能见[编辑器](editor.md) |
| 权限 | `languageServer.launch`；`executionProviders` 声明 `run`、`test` 时加 `process.run`、`process.test`；`languageServer.request`（服务器列出 `requestMethods` 或 `forwardNotifications` 时才可声明，需要 `backend`）；`documents.contentProvider`（需要 `backend`） |
| 激活事件 | `onLanguage:<id>`；`onRun:<id>`、`onTest:<id>`（执行提供者） |
| 桌面端 | 语言服务器启动、会话、输出链接、只读文档内容提供者均已接通；执行提供者未声明。详见下文“桌面端现状” |

### 语言与语法（`languages`）

`languages[]` 的 `id`、`fileExtensions`、`fileNames`、`projectFileNames` 和 `dependencies` 的规则见[契约的 Languages 一节](../../contract/v1/README.md#languages)。语法和编辑器配置是纯数据：宿主从包里读取，编辑器直接应用，不启动任何进程。只有语法的插件不需要语言服务器、后端或激活事件。

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

- `grammar.kind` 只有 `textmate` 一种。`grammar.path` 指向包内的 `.tmLanguage.json`（plist 和 YAML 语法要先转成 JSON）。宿主只读地从已安装的包里读取它，交给 `plugins.declarative.decodeGrammar` 校验，然后用返回的 `scopeName` 注册校验过的 `source`。Core 拒绝的语法是 `plugin.grammar.invalid`，并带出错字段；该语言按纯文本打开，插件其余部分照常工作。
- 上限：语法最大 2 MiB，规则（含每个 pattern、repository 项和捕获）最多 16384 条，嵌套 32 层，正则 32 KiB，捕获组 4096 个，`name`、`contentName` 1024 字节，`include` 目标和 repository 键 256 字节。
- 正则使用 TextMate 语法所用的 Oniguruma 语法。编辑器的引擎编译不了的模式永不匹配，语法的其余部分照常生效。
- `include` 可以是 `$self`、`$base`、`#name`（repository 项），或 `scope`、`scope#name`（另一个语法）。`injections` 和 `injectionSelector` 不支持，会被忽略。
- 插件语法在启用期间替换编辑器里同一 `scopeName` 的语法。两个插件语言不能注册同一个 scope：先注册的保留，另一个报告后按纯文本打开。
- `configuration` 包含 `lineComment`、`blockComment`（开始和结束）、`brackets`、`autoClosingPairs`。每个定界符 1–16 字节且不含控制字符，每个列表最多 32 对。
- 插件语法从不在编辑器界面线程上运行。宿主限制每次正则搜索的回溯和每行的耗时，无法在预算内分词的行按纯文本绘制。

字段的完整规则和拒绝条件见[契约](../../contract/v1/README.md#grammars-and-editor-configuration)。

### 语言服务器（`languageServers`）

| 字段 | 含义 |
| --- | --- |
| `languageID` | 已声明的语言。 |
| `languageIdentifier` | 文档的 LSP `languageId`，默认等于 `languageID`。 |
| `launch` | `{"kind": "packaged", "path"}` 运行包内可执行文件。`{"kind": "systemCommand", "commands"}` 列出裸命令名，由宿主的工具发现解析（PATH、项目工具链、用户覆盖）。`{"kind": "runtime", …}` 用包内或宿主发现的运行时启动包内入口文件。`{"kind": "backend", …}` 由插件后端在每次启动时回答命令行。启动字段的共同规则见[契约的 Process start](../../contract/v1/README.md#process-start)。 |
| `arguments` | 传给服务器的参数，是[启动模板](#启动模板)。 |
| `validationArguments` | 候选在使用前必须以状态 0 退出的探针；为空时宿主信任候选，由 LSP 握手决定。 |
| `runtimeDependencies` | 宿主要发现并传给服务器的工具，每一项是 `requiresTools` 列出的工具定义 ID。宿主把每个路径导出为 `LITHE_<TOOL>_PATH`（用户设置的值优先），并把其目录加到服务器的 `PATH` 前面。项目本地副本不会被选中。 |
| `initializationOptions` | 作为 LSP `initializationOptions` 发送的 JSON 对象；其中每个字符串值都是启动模板。 |
| `hostIntegration` | 任何插件都可以声明，见下一节。 |
| `requestMethods`、`commands`、`serializedCommands`、`forwardNotifications`、`state`、`remote` | 后端可以向运行中的服务器发什么，以及宿主怎样保存它的状态，见“插件自有会话”。 |

宿主拥有语言服务器进程的生命周期、LSP 会话，以及在提供者被停用时停止进程。

#### 宿主集成（`hostIntegration`）

有些服务器需要数据表达不了的行为，例如从构建文件算出的项目模型、虚拟文档、测试和调试的接线。这些行为是宿主代码，称为宿主集成。服务器按名称选择一个集成，并说明它需要的文件在插件自己包里的哪个位置：

```json
"hostIntegration": {
  "id": "jdt",
  "capabilities": ["debug", "run", "testing"],
  "resources": { "configuration": "language-server/configuration/${arch}", "lombokAgent": "…" }
}
```

- 插件拥有文件，宿主拥有行为。宿主只对声明它的服务器启用集成，从不按语言 ID 匹配，也不在包外查找文件。
- 集成是宿主代码，但只读取声明插件自己包里的资源，交给同一个插件启动的语言服务器或辅助进程。它不授予插件运行自己服务器进程之外的权限。
- `capabilities` 列出语言服务器的哪些功能由集成提供，取值是 `run`、`testing`、`debug`。没有列出、又没有执行提供者提供的功能，对这种语言不可用。
- 每个集成定义自己提供的能力和读取的资源。能力未提供、资源未声明、或在执行提供者已经提供的语言上又列出 `run` 或 `testing`，都是 `plugin.manifest.invalid`。
- 契约目前只定义了 `jdt` 这一个集成 ID：它要求 `runtime` 启动（`java` 工具），`configuration`、`lombokAgent` 为必需资源。宿主没有实现的 ID 返回 `plugin.integration.unsupported`。桌面端源码中目前没有找到 `jdt` 的实现，因此声明它会被拒绝（见“桌面端现状”）。

#### 启动模板

`arguments`、`validationArguments`、`runtimeArguments` 和 `initializationOptions` 的字符串值可以使用下列变量。包里因此从不出现机器路径，宿主决定状态放在哪里。

| 变量 | 值 |
| --- | --- |
| `${pluginDir}` | 已安装的只读包的根。每个值最多用一次，后面必须是包内以 `/` 开头的路径，该路径只能因 `${arch}` 而变化。 |
| `${stateDir}` | 宿主为本插件服务器保留的可写目录，卸载插件时删除。不能用于 `validationArguments`。 |
| `${workspaceStateDir}` | 服务器在当前工作区的可写状态目录。不能用于 `validationArguments`，也不能用于插件后端。 |
| `${logDir}` | 本次会话的 `plugins/<id>/files/` 目录，由宿主作为 `logDirectory` 提供。只用于语言服务器和插件后端，不用于 `validationArguments`。 |
| `${arch}` | 宿主 CPU 架构：`arm64` 或 `x86_64`。 |
| `${setting:<id>}` | 已声明的[插件设置](settings.md)的当前值。不能用于 `validationArguments`，也不能出现在包路径里。 |

#### 工作区配置（`workspaceConfiguration`）

语言服务器可以声明 `workspaceConfiguration`：一个对象，其中的字符串叶子可以是 `${setting:<id>}`。Core 用合并后的设置展开它，按节路径回答服务器的 `workspace/configuration` 请求，并在 `initialized` 之后发送 `workspace/didChangeConfiguration`。`immediate` 设置变化时，宿主通过 `lsp.updateWorkspaceConfiguration` 推送新值，运行中的服务器无需重启即可收到。

### 后端决定启动方式（`launch.kind: "backend"`）

服务器的命令行取决于启动时才知道的东西（解析出的版本、引导出来的工具、从工程推导的选项）时，不写死 `launch`，让插件后端每次回答：

```json
"launch": {"kind": "backend", "function": "launch.resolve", "timeoutMilliseconds": 30000}
```

| 项目 | 规则 |
| --- | --- |
| 适用 | 只有语言服务器能用。插件需要声明 `backend`。`function` 是插件函数 ID；`timeoutMilliseconds` 为 1000–120000，默认 30000 |
| 其他字段 | 服务器不能再声明 `arguments`、`validationArguments`、`runtimeDependencies`、`initializationOptions` 和 `hostIntegration`：前四项由后端的应答携带，宿主集成是宿主代码，正是这种启动方式要避免的。`workspaceConfiguration` 仍可声明式给出，除非应答自带 |
| 启动计划 | `plugins.declarative.languageServerLaunchPlan` 返回第一步而不是命令行：`launch: {kind: "backend", function, timeoutMilliseconds}`、`resolveParams`、`requiredDirectories`（固定为 `state`、`workspaceState`、`log`、`temporary`）、`settings`、`sessionPolicy`。请求要带绝对路径的 `temporaryDirectory`，否则是 `plugin.launch.invalidRequest` |
| 调用 | 宿主建好四个目录，必要时启动插件后端，用 `lithe/function/invoke` 调用 `function`，参数是 `resolveParams`（`serverID`、`platform`、四个目录、`settings`）和工作区 `context`。调用失败、超时，或工作区代次已变，这次启动就不产生进程 |
| 应答 | `executable`（`{kind: "packaged", path}` 是包内只读文件；`{kind: "tool", tool}` 是插件自己定义的工具，由宿主按工具解析）、`arguments`、`environment`、`initializationOptions`、`workspaceConfiguration`、`gates`（最多 8 个）、`writablePaths`（最多 32 个，都在四个目录之内）；不认识的字段会被拒绝 |
| 解码 | 宿主把 `{manifest, languageID, request, result}` 交给 `plugins.languageServers.decodeLaunch`，得到普通的启动计划（`launch` 为 `packaged` 或 `tool`），`sessionPolicy` 另带应答的 `gates` |
| 拒绝码 | `plugin.launch.invalidResult`、`plugin.launch.outsidePackage`、`plugin.launch.undeclaredTool`、`plugin.launch.unwritablePath`、`plugin.launch.invalidRequest`、`plugin.launch.unavailable`、`plugin.launch.notBackend` |

### 插件自有会话（`languageServers` 的会话字段）

后端只能通过服务器声明的内容驱动它自己的服务器。服务器可以声明：

| 字段 | 含义 |
| --- | --- |
| `requestMethods` | 后端可以用 `languageServer/request`、`languageServer/notify` 发送的 LSP 方法，最多 64 个 |
| `commands` | 后端可以运行的 `workspace/executeCommand` 命令，最多 128 个；恰好在 `requestMethods` 列出 `workspace/executeCommand` 时声明 |
| `serializedCommands` | `commands` 的子集，一次只运行一个：同一命令已在运行时，新请求并入它，得到同一个答案 |
| `forwardNotifications` | 宿主转发给后端的服务器通知，最多 32 个 |
| `state` | `{scope: "workspace", retention: {maxEntries, maxAgeDays}}`，`maxEntries` 为 1–64，`maxAgeDays` 为 1–365 |
| `remote` | `supported`（默认）或 `unsupported`；`unsupported` 的服务器不为远程工作区启动 |

- 名称是不含空白和控制字符、最多 256 字节的非空文本，在各自列表里唯一。宿主自己发送的方法（`initialize`、`initialized`、`shutdown`、`exit`、`textDocument/didOpen` 等文档同步方法、`workspace/didChangeWorkspaceFolders` 和所有 `$/` 方法）不能出现在 `requestMethods` 或 `forwardNotifications` 里。
- 服务器列出 `requestMethods` 或 `forwardNotifications` 时必须声明 `languageServer.request` 权限；没有这样的服务器却声明该权限，是 `plugin.manifest.invalid`。
- 启动计划的 `sessionPolicy` 就是这些字段（排序后的列表）加 `serverID`（即 `languageID`）和 `gates`。宿主原样交给 `lsp.startServer.sessionPolicy`，由 Core 的 LSP 引擎执行：宿主不能放宽，后端不能越界。
- 请求必须在 `requestMethods` 里，`workspace/executeCommand` 的 `params.command` 必须在 `commands` 里，否则是 `plugin.host.methodNotDeclared`。通知可以是 `workspace/didChangeConfiguration`，或 `requestMethods` 里除 `workspace/executeCommand` 以外的方法；宿主自己的方法仍然被拒绝。
- 会话必须属于调用方插件，并且正在运行；请求等待最多 `timeoutMilliseconds`（默认 60000，最大 300000），取消时向服务器发 `$/cancelRequest`。服务器的 JSON-RPC 错误不转发 `data`。
- **gate** 是 `{id, until: "notification", method, match}`，由启动应答声明：服务器发出该 `method` 的通知、且参数包含 `match` 的所有成员时才打开。所有 gate 打开之前，`request` 按顺序等待，最多 128 个，第 129 个是 `plugin.host.busy`；通知从不被挡住。
- 同一 `serializedCommands` 命令的后续请求在第一个运行或等待时并入它，并得到第一个请求的结果。取消第一个请求会结束所有并入的请求。
- 服务器停止、崩溃、重启或插件被停用时，所有未完成的请求以 `plugin.host.languageServerUnavailable` 结束；后端在 `didChangeState` 报告新的 `generation` 的 `running` 之后再请求。

**状态目录与回收（API-20）。** 宿主在插件的存储目录下（不在插件包或应用包里）为每个服务器、每个工作区保留一个状态目录，并把它作为 `workspaceStateDirectory` 交给服务器；停用和更新插件都不会删除。目录内的子目录名由插件决定，每个工作区目录本身的名字由宿主决定，宿主从不查看里面的内容。宿主每次启动服务器时刷新该目录的修改时间，并在启动后于后台回收：先用 Core 的 `plugins.languageServers.stateRetention` 决定要删除的目录，即超过 `maxAgeDays` 的，以及其余目录里扣除打开的工作区占用的名额后只保留最新 `maxEntries` 个以外的。打开的工作区从不被回收，没有声明 `state.retention` 的插件保留全部。

**准备状态。** `languageServer/setStatus` 报告服务器的准备程度。它与 `lithe/function/progress` 不同：后者属于正在运行的调用，而导入工程、建索引的准备工作会超过任何一次调用。

### 宿主方法与通知

后端用这些方法和通知与自己的语言服务器交谈；方法名省略前缀 `lithe/host/`。`languageServer/*` 的每个方法都带 `context: {contextID, generation}` 和 `serverID`（本插件某个服务器的 `languageID`），别的插件的服务器返回 `plugin.host.foreignContribution`。

| API | 方法 | 用途 | 权限 |
| --- | --- | --- | --- |
| API-25 | `languageServer/request` | 向本插件自己的语言服务器发 JSON-RPC 请求，参数加 `method`、`params?`、`timeoutMilliseconds?`（1–300000，默认 60000），返回 `{value}`（服务器的 `result`，可以是 `null`）；受会话策略检查 | `languageServer.request` |
| API-25 | `languageServer/notify` | 向本插件自己的语言服务器发通知，参数加 `method`、`params?`，返回 `{delivered}`；只有服务器初始化前发出的 `workspace/didChangeConfiguration` 是 `false`，这时值会随 `initialized` 送出 | `languageServer.request` |
| API-22、API-25 | `languageServer/setStatus` | 在宿主状态界面显示插件自己的准备状态：`state` 为 `preparing`、`ready`、`degraded` 或 `failed`，可带 `message?`（原样显示，截到 1024 字节）和 `percentage?`（0–100 的整数），返回 `{}`；服务器停止时宿主移除它 | `languageServer.request` |
| API-12 | `documents/registerContentProvider` | 把一个 URI scheme 绑定到后端函数：打开该 scheme 的文档时，宿主调用函数取只读内容（不超过 1 MiB，宿主等 30 秒）；每个工作区窗口每个 scheme 只能有一个提供者，返回 `{subscriptionID}`。不带 `serverID`，用 `subscriptions/dispose` 撤销 | `documents.contentProvider` |

| API | 通知 | 含义 | 权限 |
| --- | --- | --- | --- |
| API-25 | `languageServer/didChangeState` | `{context, serverID, state, generation}`；`state` 为 `starting`、`running`、`stopped` 或 `crashed` | `languageServer.request` |
| API-25 | `languageServer/didReceiveNotification` | `{context, serverID, generation, method, params?, truncated?}`：服务器发出且 `forwardNotifications` 列出的通知；超过 `limits.notificationBytes` 时只送一次 `truncated: true`，不带 `params` | `languageServer.request` |

`generation` 是宿主每次为这个上下文启动服务器时递增的正整数，后端据此分辨状态或通知属于哪个进程；新进程启动后，宿主不再送旧进程的通知。

| 错误码 | JSON-RPC | 含义 |
| --- | --- | --- |
| `plugin.host.methodNotDeclared` | `-32024` | 方法或 `workspace/executeCommand` 命令不在服务器声明的会话里 |
| `plugin.host.languageServerUnavailable` | `-32022` | 服务器没有为这个上下文运行，或请求进行中服务器已停止 |
| `plugin.host.languageServerError` | `-32023` | 服务器以 JSON-RPC 错误应答；`details` 是 `{code, message}` |
| `plugin.host.foreignContribution` | `-32026` | 指名的服务器属于别的插件 |
| `plugin.host.timeout` | `-32015` | 超过宿主的截止时间 |
| `plugin.host.cancelled` | `-32800` | 请求被取消 |
| `plugin.host.busy` | `-32020` | 第 129 个等待中的请求（见 gate） |
| `plugin.host.limitExceeded` | `-32014` | 结果超过宿主的上限 |
| `plugin.host.preconditionFailed` | `-32019` | `documents/registerContentProvider`：这个 scheme 已有提供者 |

内容提供者的 scheme 规则：1–64 个小写 ASCII 字母、数字、`+`、`-`、`.`，以字母开头；`about`、`file`、`http`、`https`、`lithe` 等由宿主或平台占用的 scheme 被拒绝。提供者只保存绑定，注册时不调用后端；打开文档时才调用函数，结果只读，每次重新打开都会重新请求。

### 运行与测试（`executionProviders`）

执行提供者让用户运行当前文件、列出并运行测试。`run` 和 `test` 各自指定一个 `executable`：宿主工具路径上的裸 `command`，或宿主拥有的项目 `toolchain` ID（如 `project-go`）。工作目录永远是工作区根。

`test` 还声明 `frameworkID`、`workspaceLabel`、`projectMarkers`、`testFilePatterns` 和 `workspaceArguments`、`fileArguments`、`testCaseArguments`。参数模板是 argv 元素，从不是 shell 文本。元素可以是字符串，或 `{"value": …, "when": "fileSelected" | "noFileSelected"}`；条件元素只允许出现在 `testCaseArguments` 中。

| 变量 | 可用位置 | 值 |
| --- | --- | --- |
| `${file}` | 运行、文件、测试用例 | 工作区相对文件路径；以 `-` 开头的名字变为 `./<名字>` |
| `${fileDirectory}` | 运行、文件、测试用例 | 文件所在目录的 `./<目录>`，工作区根为 `.` |
| `${testName}` | 测试用例 | 去掉首尾空白的测试标识 |
| `${testName:regex}` | 测试用例 | 转义了正则元字符的测试标识 |
| `${userArguments}` | 运行 | 用户的程序参数；必须是整个元素，可以展开为零个元素 |

发现测试时返回 `{id: "<语言>:workspace", kind: "workspace"}`，然后是按路径排序的 `{id: "<语言>:file:<路径>", kind: "file", relativeFilePath}`；没有项目标记时返回空列表。

### 输出链接（`outputLinkMatchers`）

`outputLinkMatchers[]` 让插件把 Run 和测试控制台里的文字变成可点击的链接，例如回溯里的一帧或 panic 的位置。宿主不认识任何工具的输出格式：插件声明匹配规则，Core 找出匹配。只声明匹配器本身就是完整的插件；不带 `resolve` 的匹配器不需要后端。

```json
"outputLinkMatchers": [
  { "id": "traceback", "pattern": "File \"(?P<path>[^\"]+)\", line (?P<line>\\d+)", "scope": "run" }
]
```

| 字段 | 含义 |
| --- | --- |
| `id` | 插件内的 ID：小写字母开头，后面最多 63 个小写字母、数字、`.`、`_` 或 `-`，在插件内唯一。每个插件最多 16 个匹配器 |
| `pattern` | 对控制台的一行做匹配的正则，1–512 字节，使用 Rust `regex` 语法，所以没有反向引用和环视，编译后的大小也有上限。最多命名 8 个分组，每个是 camelCase 名称，且不能匹配空文本 |
| `scope` | `run` 或 `test`：匹配器作用的控制台。缺省表示两者 |
| `resolve.function` | 可选，插件 `backend` 进程里的函数 ID；需要 `backend` |

- 不带 `resolve` 时，规则必须捕获 `path` 分组，可以捕获 `line` 和 `column`（从 1 开始），不接受其他分组名。相对 `path` 对每个工作区根生成一个候选，绝对路径只在某个工作区根之内有效，离开所有根的路径不生成链接：控制台里的一行不可能打开工作区之外的文件。
- 带 `resolve` 时，规则可以命名任意分组。用户点击时宿主调用该函数，参数是 `{groups}`（按名称的捕获文本）和窗口的 `context`，后端返回 `{uri, range?}` 或 `null`。
- 匹配由 Core 决定，所有宿主链接同样的文字：`outputLinks.match` 接收 `{scope, platform, caseSensitive?, workspaceRoots, sources: [{pluginID, matchers}], lines}`，返回 `{links}`。每个链接是 `{line, start, length, pluginID, matcherID}` 加 `location`（`{paths, line?, column?}`，宿主打开第一个存在的路径）或 `resolve`（`{function, groups}`）。`start` 和 `length` 按该行的 UTF-16 代码单元计数。按来源顺序、再按声明顺序找匹配，与前面链接重叠的匹配被跳过。一次请求最多 256 行、64 个来源、16 个根；每行最多扫描 4096 字节，最多 8 个链接。
- `outputLinks.decodeResolution` 接收 `{reply}`（函数的应答），返回要打开的位置 `{path, line?, column?}`（从 1 开始）或 `null`。应答只会打开 `file:` URI，只接受 `uri` 和 `range` 两个成员；从 0 开始的 `range.start` 会被转换。
- 宿主只对显示出来的行做匹配，不对没有启用该插件的输出做匹配，并忽略文件不存在的链接。

### 桌面端现状

| 功能 | 桌面端 |
| --- | --- |
| 语言、语法高亮与编辑器配置 | 已支持：语法由 Core 校验后注册到编辑器，分隔符进入注释和括号规则 |
| 语言服务器启动 | 已支持 `packaged`、包内运行时、`systemCommand`（只在 PATH 上查找，依赖项目工具链或用户覆盖的启动计划会被拒绝）和后端应答的命令行。后端应答 `{kind: "tool"}` 在启动时被拒绝，因为工具还没有经过工具链注册表解析 |
| 语言服务器呈现 | 只呈现诊断。补全、悬停、跳转、重命名、格式化没有编辑器界面，插件即使提供了也不会显示 |
| 插件自有会话与 `languageServer/*` 方法 | 已接通，策略由 Core 执行 |
| `jdt` 宿主集成 | 桌面端源码中未找到实现，声明会收到 `plugin.integration.unsupported` |
| 执行提供者（`run`、`test`、`executionProviders`） | 未声明。需要它的插件不会被提供安装 |
| 输出链接 | 已支持。Build、Run、Test 控制台的文字变成链接，带 `resolve` 的匹配器在点击时调用后端 |
| 内容提供者 | 已支持 `documents/registerContentProvider` |

### 契约章节

| 章节 | 内容 |
| --- | --- |
| [Languages](../../contract/v1/README.md#languages) | 语言 ID、扩展名、文件名、项目标记、依赖路径 |
| [Grammars and editor configuration](../../contract/v1/README.md#grammars-and-editor-configuration) | TextMate 语法的上限和拒绝规则、注释与括号配置、在后台运行 |
| [Language servers](../../contract/v1/README.md#language-servers) | 启动方式（含 `backend`）、参数、探测参数、运行时依赖 |
| [Language server host integrations](../../contract/v1/README.md#language-server-host-integrations) | 宿主集成与 `jdt` |
| [Workspace configuration](../../contract/v1/README.md#workspace-configuration) | `workspaceConfiguration` 的展开与推送 |
| [Plugin-owned sessions](../../contract/v1/README.md#plugin-owned-sessions) | 会话字段、会话策略、gate、状态目录与回收、准备状态 |
| [Runtime launch](../../contract/v1/README.md#runtime-launch) | 用 `java` 或 `node` 等工具启动包内入口文件 |
| [Tool references](../../contract/v1/README.md#tool-references) | 按工具定义 ID 引用运行时、启动计划里的 `tool` 启动方式 |
| [Launch templates](../../contract/v1/README.md#launch-templates) | `${pluginDir}`、`${stateDir}`、`${workspaceStateDir}` 等模板变量 |
| [Backend-resolved launch](../../contract/v1/README.md#backend-resolved-launch) | 后端回答命令行：`resolveParams`、应答字段、拒绝码 |
| [Language server host methods](../../contract/v1/README.md#language-server-host-methods) | `languageServer/request`、`notify`、`setStatus` 和两个通知 |
| [Content providers](../../contract/v1/README.md#content-providers) | `documents/registerContentProvider`：按 URI scheme 提供只读文档 |
| [Execution providers](../../contract/v1/README.md#execution-providers) | `run`、`test` 的可执行文件、参数模板、测试发现 |
| [Output link matchers](../../contract/v1/README.md#output-link-matchers) | Run 与测试控制台输出里的链接、`outputLinks.match`、`outputLinks.decodeResolution` |

### 示例与用例

| 文件 | 内容 |
| --- | --- |
| [`languages/language-packaged-server.plugin.json`](../../contract/v1/fixtures/languages/language-packaged-server.plugin.json) | 包内语言服务器加运行/测试提供者（PHP） |
| [`languages/language-system-command.plugin.json`](../../contract/v1/fixtures/languages/language-system-command.plugin.json) | 用系统命令启动的语言服务器加运行/测试提供者（Go） |
| [`languages/node-runtime.plugin.json`](../../contract/v1/fixtures/languages/node-runtime.plugin.json) | 用 `node` 运行时启动，带插件设置 |
| [`languages/language-grammar.plugin.json`](../../contract/v1/fixtures/languages/language-grammar.plugin.json) | 只有语法、不启动进程的插件 |
| [`languages/language-server-backend-launch.plugin.json`](../../contract/v1/fixtures/languages/language-server-backend-launch.plugin.json) | 命令行由后端回答、声明会话字段和 `languageServer.request` 的语言服务器 |
| [`languages/grammars/http.tmLanguage.json`](../../contract/v1/fixtures/languages/grammars/http.tmLanguage.json) | 完整的 TextMate 语法示例 |
| [`languages/command-plans.json`](../../contract/v1/fixtures/languages/command-plans.json) | 运行/测试模板和语言服务器启动计划的展开结果的标准答案 |
| [`languages/language-server-launch-resolution.json`](../../contract/v1/fixtures/languages/language-server-launch-resolution.json) | 后端解析启动的第一步（`languageServerLaunchPlan`）和解码应答（`plugins.languageServers.decodeLaunch`）得到的启动计划或拒绝码 |
| [`run/output-link-matchers.json`](../../contract/v1/fixtures/run/output-link-matchers.json) | `outputLinks.match` 的请求与返回的链接，以及 `outputLinks.decodeResolution` 的结果 |
| [`run/tool-reference-launch.plugin.json`](../../contract/v1/fixtures/run/tool-reference-launch.plugin.json) | 语言服务器的 `runtime` 启动按工具 ID `interpreter` 引用运行时，并带一个 `outputLinkMatchers` 条目 |

语言相关的拒绝用例在 [`manifest/invalid-manifests.json`](../../contract/v1/fixtures/manifest/invalid-manifests.json)。

### 相关指南

- [从零构建一个完整生命周期的 LSP 插件](../../guides/lsp-plugin-lifecycle.md)
- [为语言插件接入构建、运行与测试](../../guides/build-tool-integration.md)

---

## English

Languages let Lithe recognize a language: identify its files, highlight them with a TextMate grammar shipped in the package, start an LSP language server, make output links clickable, and, when the plugin provides an execution provider, offer "run the current file" and tests. All of this is data. The host owns process lifecycle, the LSP session, and stopping processes, so a plugin writes no host code. When the command line is known only at start, or the backend must drive its own server, the plugin backend can answer the launch command and send requests to the server within the scope the server declares (see "Backend-resolved launch" and "Plugin-owned sessions").

| Item | Value |
| --- | --- |
| Manifest fields | `languages`, `languageServers`, `executionProviders`, `outputLinkMatchers` |
| Capabilities | `languages`, `languageServers`, `executionProviders`, `outputLinkMatchers`, each at version `0`. A `languageServers` backend launch needs `backend`; sessions, a matcher's `resolve`, and content providers need `backend` too |
| Protocols | LSP. The `languageServer/*` host methods and notifications below belong to the plugin backend's `litheFunctions` protocol (see [Plugin backend](backend.md)) |
| API IDs | [API-25](../api-catalog.md#apis-usable-now): languages, grammars, LSP. [API-12](../api-catalog.md#apis-usable-now): read-only documents by URI scheme (`documents/registerContentProvider`). [API-20](../api-catalog.md#apis-usable-now): per-workspace state directories of language servers, and their pruning. [API-22](../api-catalog.md#apis-usable-now): language server preparation status (`languageServer/setStatus`) and links in Run and Test output. Editor language features supplied by plugin functions are in [Editor](editor.md) |
| Permissions | `languageServer.launch`; `process.run`, `process.test` when an execution provider declares `run` or `test`; `languageServer.request` (only when a server lists `requestMethods` or `forwardNotifications`; needs `backend`); `documents.contentProvider` (needs `backend`) |
| Activation events | `onLanguage:<id>`; `onRun:<id>`, `onTest:<id>` (execution providers) |
| Desktop | Language server launch, sessions, output links, and read-only content providers are wired; execution providers are not declared. See "Desktop status" below |

### Languages and grammars (`languages`)

The rules for `id`, `fileExtensions`, `fileNames`, `projectFileNames`, and `dependencies` are in [the contract's Languages section](../../contract/v1/README.md#languages). A grammar and editor configuration are pure data: the host reads them from the package and the editor applies them without starting any process. A plugin with only grammars needs no language server, backend, or activation event.

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

- `grammar.kind` is only `textmate`. `grammar.path` names a `.tmLanguage.json` file in the package (plist and YAML grammars are converted to JSON first). The host reads it read-only from the installed package, passes it to `plugins.declarative.decodeGrammar`, and registers the returned `source` under the returned `scopeName`. A grammar Core refuses is `plugin.grammar.invalid` with the failing field; the language then opens as plain text and the rest of the plugin keeps working.
- Limits: a grammar is at most 2 MiB; at most 16384 rules, counting every pattern, repository entry, and capture; nesting 32 deep; regexes of 32 KiB; 4096 capture groups; `name` and `contentName` of 1024 bytes; `include` targets and repository keys of 256 bytes.
- Regexes use the Oniguruma syntax TextMate grammars are written in. A pattern the editor's engine cannot compile never matches, and the rest of the grammar still applies.
- `include` is `$self`, `$base`, `#name` for a repository entry, or `scope` or `scope#name` for another grammar. `injections` and `injectionSelector` are not supported and are ignored.
- A plugin grammar replaces the editor's own grammar of the same `scopeName` while the plugin is enabled. Two plugin languages cannot register the same scope: the first keeps it, and the other opens as plain text.
- `configuration` sets `lineComment`, `blockComment` (open and close), `brackets`, and `autoClosingPairs`. Each delimiter is 1 to 16 bytes without control characters; each list has at most 32 pairs.
- Plugin grammars never run on the editor UI thread. The host bounds the backtracking of each regex search and the time one line may take; a line that cannot be tokenized within that budget is drawn as plain text.

### Language servers (`languageServers`)

| Field | Meaning |
| --- | --- |
| `languageID` | A declared language. |
| `languageIdentifier` | LSP `languageId` for documents; defaults to `languageID`. |
| `launch` | `{"kind": "packaged", "path"}` runs an executable inside the package. `{"kind": "systemCommand", "commands"}` lists bare command names the host resolves with its tool discovery (PATH, project toolchains, user overrides). `{"kind": "runtime", …}` starts a package entry file with a packaged or discovered runtime. `{"kind": "backend", …}` has the plugin backend answer the command line at each start. The shared launch rules are in [the contract's Process start](../../contract/v1/README.md#process-start). |
| `arguments` | Arguments passed to the server, written as [launch templates](#launch-templates). |
| `validationArguments` | A probe that must exit with status 0 before a candidate is used; empty means the host trusts the candidate and lets the LSP handshake decide. |
| `runtimeDependencies` | Tools the host discovers and passes to the server, each a tool-definition ID that `requiresTools` lists. The host exports each path as `LITHE_<TOOL>_PATH` (a user-set value takes precedence) and prepends its directory to the server's `PATH`. A project-local copy is never chosen. |
| `initializationOptions` | A JSON object sent as the LSP `initializationOptions`. Every string value inside it is a launch template. |
| `hostIntegration` | Any plugin may declare one; see the next section. |
| `requestMethods`, `commands`, `serializedCommands`, `forwardNotifications`, `state`, `remote` | What the backend may ask of the running server and how the host keeps its state; see "Plugin-owned sessions". |

The host owns the language server process lifecycle, the LSP session, and stopping the process when the provider is disabled.

#### Host integrations (`hostIntegration`)

Some servers need behavior data cannot express: a project model computed from build files, virtual documents, test and debug wiring. That behavior is host code, called a host integration. A server selects one by name and says where the files it needs are inside the plugin's own package:

```json
"hostIntegration": {
  "id": "jdt",
  "capabilities": ["debug", "run", "testing"],
  "resources": { "configuration": "language-server/configuration/${arch}", "lombokAgent": "…" }
}
```

- The plugin owns the files; the host owns the behavior. The host enables an integration only for the server that declares it, never matches on a language ID, and never looks for files outside the package.
- The integration is host code, but it reads only resources inside the declaring plugin's own package and hands them to the language server or helper process that same plugin starts. It grants no permission beyond running the plugin's own server process.
- `capabilities` lists which features the integration provides for the language: `run`, `testing`, or `debug`. A feature that is neither listed nor supplied by an execution provider is unavailable for the language.
- Each integration defines the capabilities it provides and the resources each one reads. A capability it does not provide, a capability whose resource is not declared, or `run` or `testing` for a language whose execution provider already supplies it, is `plugin.manifest.invalid`.
- The contract defines one integration ID, `jdt`: it requires a `runtime` launch with the `java` tool, and `configuration` and `lombokAgent` are required resources. An ID the host does not implement returns `plugin.integration.unsupported`. No implementation of `jdt` was found in the desktop source, so declaring it is refused (see "Desktop status").

#### Launch templates

`arguments`, `validationArguments`, `runtimeArguments`, and the string values of `initializationOptions` may use these variables. A package therefore never contains a machine path, and the host decides where state lives.

| Variable | Value |
| --- | --- |
| `${pluginDir}` | Root of the installed, read-only package. Used at most once per value and followed by a `/`-rooted path inside the package that may vary only by `${arch}`. |
| `${stateDir}` | Writable directory the host keeps for this plugin's server; removed when the plugin is uninstalled. Not available in `validationArguments`. |
| `${workspaceStateDir}` | Writable directory for the server's state in the current workspace. Not available in `validationArguments` or in a plugin backend. |
| `${logDir}` | The current session's `plugins/<id>/files/` directory, supplied by the host as `logDirectory`. Available to language servers and plugin backends, never in `validationArguments`. |
| `${arch}` | Host CPU architecture: `arm64` or `x86_64`. |
| `${setting:<id>}` | Current value of a declared [plugin setting](settings.md). Not available in `validationArguments`, and never inside a package path. |

#### Workspace configuration

A language server may declare `workspaceConfiguration`, an object whose string leaves may be `${setting:<id>}` templates. Core expands it with the merged settings, answers the server's `workspace/configuration` requests by section path, and sends `workspace/didChangeConfiguration` after `initialized`. When an `immediate` setting changes, the host sends the new value with `lsp.updateWorkspaceConfiguration`, and the running server receives it without a restart.

### Backend-resolved launch (`launch.kind: "backend"`)

When a server's command line depends on what is known only at start (a version it resolves, a tool it bootstraps, options derived from the project), do not write `launch`; let the plugin backend answer it each time:

```json
"launch": {"kind": "backend", "function": "launch.resolve", "timeoutMilliseconds": 30000}
```

| Item | Rule |
| --- | --- |
| Scope | Only a language server can use it. The plugin declares `backend`. `function` is a plugin function ID; `timeoutMilliseconds` is 1000 to 120000 and defaults to 30000 |
| Other fields | The server declares none of `arguments`, `validationArguments`, `runtimeDependencies`, `initializationOptions`, or `hostIntegration`: the answer carries the first four, and a host integration is host code, which this launch exists to avoid. `workspaceConfiguration` stays declarative unless the answer carries its own |
| Launch plan | `plugins.declarative.languageServerLaunchPlan` returns the first step, not a command line: `launch: {kind: "backend", function, timeoutMilliseconds}`, `resolveParams`, `requiredDirectories` (always `state`, `workspaceState`, `log`, and `temporary`), `settings`, and `sessionPolicy`. The request must carry an absolute `temporaryDirectory`, or the result is `plugin.launch.invalidRequest` |
| Call | The host creates the four directories, starts the plugin backend if needed, and calls `function` through `lithe/function/invoke` with `resolveParams` (`serverID`, `platform`, the four directories, `settings`) and the workspace `context`. A failed or timed-out call, or a changed workspace generation, ends the start without a process |
| Answer | `executable` (`{kind: "packaged", path}` is a read-only file in the package; `{kind: "tool", tool}` names a tool definition of the plugin, which the host resolves), `arguments`, `environment`, `initializationOptions`, `workspaceConfiguration`, `gates` (at most 8), and `writablePaths` (at most 32, all inside the four directories). An unknown field is refused |
| Decoding | The host passes `{manifest, languageID, request, result}` to `plugins.languageServers.decodeLaunch`, which returns an ordinary launch plan (`launch` is `packaged` or `tool`); its `sessionPolicy` also carries the answer's `gates` |
| Rejections | `plugin.launch.invalidResult`, `plugin.launch.outsidePackage`, `plugin.launch.undeclaredTool`, `plugin.launch.unwritablePath`, `plugin.launch.invalidRequest`, `plugin.launch.unavailable`, `plugin.launch.notBackend` |

### Plugin-owned sessions

A plugin backend drives its own server only through what the server declares:

| Field | Meaning |
| --- | --- |
| `requestMethods` | LSP methods the backend may send with `languageServer/request` or `languageServer/notify`; at most 64 |
| `commands` | The `workspace/executeCommand` commands the backend may run; at most 128. Declared exactly when `requestMethods` lists `workspace/executeCommand` |
| `serializedCommands` | A subset of `commands` that runs one at a time: a request for a command already running joins it and receives the same answer |
| `forwardNotifications` | Server notifications the host forwards to the backend; at most 32 |
| `state` | `{scope: "workspace", retention: {maxEntries, maxAgeDays}}`; `maxEntries` is 1 to 64, `maxAgeDays` 1 to 365 |
| `remote` | `supported` (default) or `unsupported`; an `unsupported` server is not started for a remote workspace |

- A name is non-empty text of at most 256 bytes without whitespace or control characters, unique within its list. The names the host sends itself (`initialize`, `initialized`, `shutdown`, `exit`, the document sync methods such as `textDocument/didOpen`, `workspace/didChangeWorkspaceFolders`, and every `$/` method) cannot appear in `requestMethods` or `forwardNotifications`.
- A server that lists `requestMethods` or `forwardNotifications` requires the `languageServer.request` permission; declaring the permission without such a server is `plugin.manifest.invalid`.
- The launch plan's `sessionPolicy` is these fields as sorted lists plus `serverID` (the `languageID`) and `gates`. The host passes it unchanged to `lsp.startServer.sessionPolicy`; Core's LSP engine enforces it, so the host cannot widen it and the backend cannot step outside it.
- A request must be in `requestMethods`, and a `workspace/executeCommand` must carry a `params.command` in `commands`; anything else is `plugin.host.methodNotDeclared`. A notification may be `workspace/didChangeConfiguration`, or a `requestMethods` entry other than `workspace/executeCommand`. The host's own methods stay refused.
- The session must belong to the calling plugin and be running. A request waits at most `timeoutMilliseconds` (default 60000, at most 300000); cancelling it sends the server `$/cancelRequest`. A server's JSON-RPC error does not forward its `data`.
- A **gate** is `{id, until: "notification", method, match}`, declared by the launch answer. It opens when the server sends a notification with that `method` whose params contain every member of `match`. Until every gate has opened, `request` calls wait in order, at most 128; the 129th is `plugin.host.busy`. Notifications are never held.
- The first request for a `serializedCommands` command runs; a later request for the same command, while the first is running or waiting, joins it and gets the first one's outcome. Cancelling the first request ends the joined requests with it.
- When the server stops, crashes, restarts, or the plugin is disabled, every pending request ends with `plugin.host.languageServerUnavailable`; the backend asks again after `didChangeState` reports `running` with a new `generation`.

**State and retention (API-20).** The host keeps one state directory per workspace for each server, under the plugin's storage (never under the package or application bundle), and gives it to the server as `workspaceStateDirectory`. Disabling and updating the plugin do not delete it. The plugin chooses sub-directory names inside it; the host chooses the per-workspace directory names and never looks inside them. The host refreshes a workspace directory's modification time on every start of the server and sweeps in the background after a start, using `plugins.languageServers.stateRetention`: it deletes directories older than `maxAgeDays`, and of the rest, all but the newest `maxEntries` that open workspaces leave free. An open workspace is never pruned. A plugin that declares no `state.retention` keeps everything.

**Preparation status.** `languageServer/setStatus` reports how ready the server is. It exists because `lithe/function/progress` belongs to a call that is running, while preparation (importing a build, indexing) outlives any call.

### Host methods and notifications

The backend uses these methods and notifications to talk to its own language servers; the names omit the `lithe/host/` prefix. Every `languageServer/*` method carries `context: {contextID, generation}` and `serverID` (the `languageID` of one of the plugin's servers). A server of another plugin returns `plugin.host.foreignContribution`.

| API | Method | Purpose | Permission |
| --- | --- | --- | --- |
| API-25 | `languageServer/request` | Send a JSON-RPC request to the plugin's own language server. Parameters: `method`, `params?`, `timeoutMilliseconds?` (1 to 300000, default 60000). Returns `{value}` (the server's `result`, possibly `null`). Checked by the session policy | `languageServer.request` |
| API-25 | `languageServer/notify` | Send a notification to the plugin's own language server. Parameters: `method`, `params?`. Returns `{delivered}`; only a `workspace/didChangeConfiguration` sent before the server initializes is `false`, and its value is then sent with `initialized` | `languageServer.request` |
| API-22, API-25 | `languageServer/setStatus` | Show the plugin's preparation state in the host's status UI: `state` is `preparing`, `ready`, `degraded`, or `failed`, with optional `message` (shown as given, cut to 1024 bytes) and `percentage` (integer 0 to 100). Returns `{}`. The host removes it when the server stops | `languageServer.request` |
| API-12 | `documents/registerContentProvider` | Bind a URI scheme to a backend function: when a document of that scheme opens, the host calls the function for read-only content (at most 1 MiB; the host waits 30 seconds). One provider per scheme per workspace window. Returns `{subscriptionID}`; revoke with `subscriptions/dispose`. No `serverID` | `documents.contentProvider` |

| API | Notification | Meaning | Permission |
| --- | --- | --- | --- |
| API-25 | `languageServer/didChangeState` | `{context, serverID, state, generation}`; `state` is `starting`, `running`, `stopped`, or `crashed` | `languageServer.request` |
| API-25 | `languageServer/didReceiveNotification` | `{context, serverID, generation, method, params?, truncated?}`: a notification the server sent and `forwardNotifications` lists. Above `limits.notificationBytes` it is delivered once with `truncated: true` and without `params` | `languageServer.request` |

`generation` is a positive integer the host raises each time it starts the server for this context, so the backend can tell which process a state or notification belongs to. After the new process starts, the host never delivers the old one's notifications.

| Error code | JSON-RPC | Meaning |
| --- | --- | --- |
| `plugin.host.methodNotDeclared` | `-32024` | The method, or the `workspace/executeCommand` command, is not in the server's declared session |
| `plugin.host.languageServerUnavailable` | `-32022` | The server is not running for this context, or it stopped while the request was in flight |
| `plugin.host.languageServerError` | `-32023` | The server answered with a JSON-RPC error; `details` is `{code, message}` |
| `plugin.host.foreignContribution` | `-32026` | The named server belongs to another plugin |
| `plugin.host.timeout` | `-32015` | The host's deadline passed |
| `plugin.host.cancelled` | `-32800` | The request was cancelled |
| `plugin.host.busy` | `-32020` | The 129th waiting request (see gate) |
| `plugin.host.limitExceeded` | `-32014` | The result exceeds the host's bound |
| `plugin.host.preconditionFailed` | `-32019` | `documents/registerContentProvider`: the scheme already has a provider |

Content provider schemes are 1 to 64 lowercase ASCII letters, digits, `+`, `-`, or `.`, starting with a letter. Schemes the host or platform uses, such as `about`, `file`, `http`, `https`, and `lithe`, are refused. Registration only records the binding and never calls the backend; the function is called when a document opens, the content is read-only, and it is requested again each time the document is reopened.

### Run and test (`executionProviders`)

An execution provider lets the user run the current file and list and run tests. `run` and `test` each name an `executable`: exactly one of a bare `command` resolved on the host's tool path, or a host-owned project `toolchain` ID such as `project-go`. The working directory is always the workspace root.

`test` also declares `frameworkID`, `workspaceLabel`, `projectMarkers`, `testFilePatterns`, and the argument templates `workspaceArguments`, `fileArguments`, and `testCaseArguments`. Argument templates are argv elements, never shell text. Each element is a string or `{"value": …, "when": "fileSelected" | "noFileSelected"}`; conditional elements are only allowed in `testCaseArguments`.

| Variable | Available in | Value |
| --- | --- | --- |
| `${file}` | run, file, test case | Workspace-relative file. A name starting with `-` becomes `./<name>` |
| `${fileDirectory}` | run, file, test case | `./<directory>` of the file, or `.` at the workspace root |
| `${testName}` | test case | Test identifier with surrounding whitespace removed |
| `${testName:regex}` | test case | The identifier with regular-expression metacharacters escaped |
| `${userArguments}` | run | The user's program arguments; must be a whole element and may expand to zero elements |

Discovery returns `{id: "<language>:workspace", kind: "workspace"}` followed by `{id: "<language>:file:<path>", kind: "file", relativeFilePath}` for matching files sorted by path, or an empty list when no project marker is present.

### Output links (`outputLinkMatchers`)

`outputLinkMatchers[]` makes text in the Run and Test consoles clickable, such as a traceback frame or a panic location. The host knows no tool's output format: the plugin declares the pattern and Core finds the matches. A plugin that declares only matchers is complete; matchers without `resolve` need no backend.

```json
"outputLinkMatchers": [
  { "id": "traceback", "pattern": "File \"(?P<path>[^\"]+)\", line (?P<line>\\d+)", "scope": "run" }
]
```

| Field | Meaning |
| --- | --- |
| `id` | Plugin-local ID: a lowercase letter, then up to 63 lowercase letters, digits, `.`, `_`, or `-`; unique in the plugin. At most 16 matchers per plugin |
| `pattern` | A regular expression of 1 to 512 bytes matched against one console line. It uses Rust `regex` syntax (no backreferences, no look-around), and its compiled size is limited. At most 8 named groups, each camelCase, and it must not match empty text |
| `scope` | `run` or `test`: the console the matcher applies to. Absent means both |
| `resolve.function` | Optional ID of a function of the plugin's `backend` process; needs `backend` |

- Without `resolve`, the pattern must capture a `path` group and may capture `line` and `column` (1-based); no other group name is accepted. A relative `path` becomes one candidate per workspace root; an absolute path counts only inside a workspace root, so a console line can never open a file outside the workspace.
- With `resolve`, the pattern may name any groups. When the user clicks, the host calls the function with `{groups}` (captured text by name) and the window's `context`; the backend answers `{uri, range?}` or `null`.
- Core owns the decision, so every host links the same text. `outputLinks.match` takes `{scope, platform, caseSensitive?, workspaceRoots, sources: [{pluginID, matchers}], lines}` and returns `{links}`. Each link is `{line, start, length, pluginID, matcherID}` plus either `location` (`{paths, line?, column?}`; the host opens the first existing path) or `resolve` (`{function, groups}`). `start` and `length` count UTF-16 code units of the line. Matches are found in source order and then declaration order, and a match overlapping an earlier link is skipped. A request carries at most 256 lines, 64 sources, and 16 roots; a line is scanned up to 4096 bytes and yields at most 8 links.
- `outputLinks.decodeResolution` takes `{reply}` (what the function answered) and returns the location to open, `{path, line?, column?}` with 1-based positions, or `null`. A reply opens only a `file:` URI and accepts only the members `uri` and `range`; a 0-based `range.start` is converted.
- The host matches only lines it displays, never output of a plugin that is not enabled, and ignores a link whose file does not exist.

### Desktop status

| Feature | Lithe desktop |
| --- | --- |
| Languages, grammar highlighting, editor configuration | Supported: Core validates the grammar and the host registers it with the editor; delimiters reach the comment and bracket rules |
| Language server launch | Supported for `packaged`, a packaged runtime, `systemCommand` (PATH only; a plan that depends on a project toolchain or a user override is refused), and a backend-answered command line. A backend answer of `{kind: "tool"}` is refused at start, because the tool is not yet resolved through the toolchain registry |
| Language server presentation | Only diagnostics are shown. Completion, hover, navigation, rename, and formatting have no editor surface; a plugin that provides them sees no effect |
| Plugin-owned sessions and `languageServer/*` methods | Wired; Core enforces the policy |
| `jdt` host integration | No implementation found in the desktop source; declaring it is refused with `plugin.integration.unsupported` |
| Execution providers (`run`, `test`, `executionProviders`) | Not declared. A plugin that needs them is not offered for installation |
| Output links | Supported. Console text in Build, Run, and Test becomes links; a matcher with `resolve` calls the backend on click |
| Content providers | `documents/registerContentProvider` supported |

### Contract sections

| Section | Content |
| --- | --- |
| [Languages](../../contract/v1/README.md#languages) | Language IDs, extensions, file names, project markers, dependency paths |
| [Grammars and editor configuration](../../contract/v1/README.md#grammars-and-editor-configuration) | TextMate grammar limits and refusal rules, comment and bracket configuration, background execution |
| [Language servers](../../contract/v1/README.md#language-servers) | Launch kinds (including `backend`), arguments, probe arguments, runtime dependencies |
| [Language server host integrations](../../contract/v1/README.md#language-server-host-integrations) | Host integrations and `jdt` |
| [Workspace configuration](../../contract/v1/README.md#workspace-configuration) | Expanding and pushing `workspaceConfiguration` |
| [Plugin-owned sessions](../../contract/v1/README.md#plugin-owned-sessions) | Session fields, session policy, gates, state directories and pruning, preparation status |
| [Runtime launch](../../contract/v1/README.md#runtime-launch) | Starting a package entry file with a tool such as `java` or `node` |
| [Tool references](../../contract/v1/README.md#tool-references) | Referencing runtimes by tool-definition ID; the `tool` launch in a launch plan |
| [Launch templates](../../contract/v1/README.md#launch-templates) | `${pluginDir}`, `${stateDir}`, `${workspaceStateDir}` and the other template variables |
| [Backend-resolved launch](../../contract/v1/README.md#backend-resolved-launch) | The backend answers the command line: `resolveParams`, answer fields, rejection codes |
| [Language server host methods](../../contract/v1/README.md#language-server-host-methods) | `languageServer/request`, `notify`, `setStatus`, and the two notifications |
| [Content providers](../../contract/v1/README.md#content-providers) | `documents/registerContentProvider`: read-only documents by URI scheme |
| [Execution providers](../../contract/v1/README.md#execution-providers) | `run` and `test` executables, argument templates, test discovery |
| [Output link matchers](../../contract/v1/README.md#output-link-matchers) | Links in Run and Test console output, `outputLinks.match`, `outputLinks.decodeResolution` |

### Examples and cases

| File | Content |
| --- | --- |
| [`languages/language-packaged-server.plugin.json`](../../contract/v1/fixtures/languages/language-packaged-server.plugin.json) | A packaged language server plus run and test providers (PHP) |
| [`languages/language-system-command.plugin.json`](../../contract/v1/fixtures/languages/language-system-command.plugin.json) | A language server started by a system command, plus run and test providers (Go) |
| [`languages/node-runtime.plugin.json`](../../contract/v1/fixtures/languages/node-runtime.plugin.json) | Started with a `node` runtime, with plugin settings |
| [`languages/language-grammar.plugin.json`](../../contract/v1/fixtures/languages/language-grammar.plugin.json) | Grammars only; starts no process |
| [`languages/language-server-backend-launch.plugin.json`](../../contract/v1/fixtures/languages/language-server-backend-launch.plugin.json) | A language server whose command line the backend answers, declaring session fields and `languageServer.request` |
| [`languages/grammars/http.tmLanguage.json`](../../contract/v1/fixtures/languages/grammars/http.tmLanguage.json) | A complete TextMate grammar example |
| [`languages/command-plans.json`](../../contract/v1/fixtures/languages/command-plans.json) | Golden expansions of run and test templates and language server launch plans |
| [`languages/language-server-launch-resolution.json`](../../contract/v1/fixtures/languages/language-server-launch-resolution.json) | Golden results of the first backend-resolution step (`languageServerLaunchPlan`) and of decoding the answer (`plugins.languageServers.decodeLaunch`): a launch plan or a rejection code |
| [`run/output-link-matchers.json`](../../contract/v1/fixtures/run/output-link-matchers.json) | Requests and returned links of `outputLinks.match`, and the `outputLinks.decodeResolution` result for a matcher's `resolve` reply |
| [`run/tool-reference-launch.plugin.json`](../../contract/v1/fixtures/run/tool-reference-launch.plugin.json) | A language server whose `runtime` launch references the runtime by tool ID `interpreter`, with an `outputLinkMatchers` entry |

Language-related rejection cases are in [`manifest/invalid-manifests.json`](../../contract/v1/fixtures/manifest/invalid-manifests.json).

### Related guides

- [Building an LSP plugin end to end](../../guides/lsp-plugin-lifecycle.md)
- [Build, run, and test for language plugins](../../guides/build-tool-integration.md)
