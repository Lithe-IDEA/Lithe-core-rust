# 工具链 / Toolchains

[中文](#中文) · [English](#english)

## 中文

插件可以告诉 Lithe 一个工具（JDK、Maven、Node、Go 等）长什么样、去哪里找、怎么判断版本，也可以用后端回答“这台机器装了哪些版本、这个项目要哪个版本”。宿主没有内置任何工具定义、别名表或按工具名的分支：工具定义只来自已启用的插件，Core 负责校验和选择，宿主只做文件系统观察、运行版本探针和启动后端这些本机 I/O。

| 项目 | 内容 |
| --- | --- |
| 清单字段 | `toolDefinitions[]`（工具定义）、`toolchainProviders[]`（工具链提供者）、顶层 `requiresTools`（启动时引用的工具定义 ID） |
| 能力 | `toolDefinitions` 0；`toolchainProviders` 0（还需要 `backend` 0） |
| 协议 | 工具链提供者协议 `litheToolchains`，版本 `0`：宿主通过 `toolchains/query` 向提供者的后端查询。协商见[契约](../../contract/v1/toolchains/README.md#provider-backend-negotiation) |
| API 编号 | [API-23](../api-catalog.md#现在能用的-api)：工具链与受管进程（部分完成） |
| 权限 | 工具定义不需要权限；提供者需要 `backend.launch` 和 `toolchains.provide` |
| 激活事件 | 不需要。提供者由宿主的第一次查询唤起，不需要页面或启动事件 |
| 后端启动 | 提供者的后端只能是包内可执行文件（`packaged`），或包内运行时（`runtime` 加 `packagedRuntime`），不能依赖需要发现的运行时 |
| 桌面端 | 声明 `toolDefinitions`；未声明 `toolchainProviders`，见“桌面端现状” |
| 官方 SDK | 三个官方 SDK 都还没有实现 `litheToolchains`，提供者后端要自己处理 `toolchains/query` |

### 两条安装来源

一个工具可以走其中一条、两条都走，或者都不走（只靠固定、项目文件、全局默认和登录 shell 里的命令）。

| 来源 | 由谁声明 | 适合 | 谁执行 |
| --- | --- | --- | --- |
| 声明式发现 | 工具定义里的 `discovery`、`homeLayouts`、`homeVariable`、`commands`、`version` | 目录形状固定的安装：用户主目录、环境变量或系统绝对目录下的单段 `*` 通配，可选的主目录后缀，加上对版本探针输出的正则 | 宿主展开 `discoveryPlan` 的扫描，不运行插件代码 |
| 提供者查询 | `toolchainProviders[]` 回答 `toolchains/query` | 声明式原语表达不了的位置：注册表、属性列表、版本管理器数据库、命令输出、版本管理器自己的版本文件 | 提供者的包内后端 |

声明式原语已经冻结：根目录只有 `home`、`environment`、`absolute` 三种，通配只有单段 `*`。新的安装形状由提供者回答，不再增加根目录类型或宿主解析器。没有 `watchFiles` 的提供者只报告安装，应答里不能有版本要求。

### 选择顺序

| 层 | 规则 |
| --- | --- |
| 固定（pin） | 最近的祖先目录的固定；只按完整路径段匹配，前缀相似的兄弟目录不继承 |
| 项目 | 最近的声明目录；同一深度 wrapper > 环境变量 > 声明；同一来源里原生读取器优先于兼容读取器。离得近的项目要求遮住更上层的要求 |
| 全局 | Lithe 的全局默认 > 没有歧义（或用户已选）的提供者全局默认 > 登录 shell |
| 自动 | 已安装、满足最低版本的最高版本。只用于固定的安装不参与自动选择。不同主目录有相同最高版本时算冲突，要求用户选择 |

一次操作还可以带 `fixed: {home}` 或 `fixed: {executable}`，它排在目录固定之前。这个显式选择无效时直接失败，更低的层不能替代它。

没有安装的项目要求会带着原因保留为“未满足”，即使更低的层选中了别的安装。冲突必须由用户明确选择，不按插件 ID 或路径先后决定。

### 工具 ID 与冲突

- 任何插件都可以定义短 ID（例如 `compiler`）或限定 ID `<自己的插件 ID>:<本地 ID>`。限定 ID 的前缀必须是定义它的插件自己的 ID，所以不同插件的限定 ID 永远不会冲突。宿主不提供来源证明，插件自称的来源也不会被读取。提供者可以回答短 ID。
- 同一个短 ID 被多个已启用插件定义时，它不归其中任何一方：注册表不登记这些定义，其余定义照常登记；`discoveryPlan` 不给它列扫描和提供者；解析它的消费者把它当作没有人声明。归属从不按到达顺序或插件 ID 顺序决定，否则恶意插件可以靠先加载或取更小的 ID 抢走工具。
- 注册表的输出带 `conflicts: [{tool, pluginIDs}]`：每个被争用的工具一条，按 `tool` 排序，`pluginIDs` 是所有定义它的插件 ID（也排序）。宿主应当告诉用户是哪些插件在争用。
- 同一个插件 ID 在一批输入里重复出现，仍然拒绝整批，不会替换先前登记的归属。

### Core 命令

所有命令都只处理 UTF-8 JSON，输入上限 4 MiB，不做任何本机 I/O。成功返回 `{status: "ok", <键>: 值}`，拒绝返回 `{status: "rejected", error: {code, message, field?}}`。

| 命令 | 输入 | 成功时的键 | 用途 |
| --- | --- | --- | --- |
| `toolchains.registry` | `{plugins: [{manifest}]}` | `registry` | 重新校验完整清单，汇总定义和提供者，返回 `{definitions, providers, conflicts}`；宿主不提供来源证明；被多个插件争用的短 ID 不登记，列在 `conflicts`；最多 128 个插件、512 个定义、256 个提供者 |
| `toolchains.decodeReply` | 查询请求、提供者应答、宿主观察到的文件系统事实 | `snapshot` | 校验一次 `toolchains/query` 应答，得到 `{installations, statements, watch, diagnostics}` |
| `toolchains.failureReply` | `{params, code}` | `reply` | 查询没有应答时生成对应的 JSON-RPC 错误；`code` 只能是 `plugin.toolchain.timeout` 或 `plugin.toolchain.providerUnavailable` |
| `toolchains.resolvePins` | `{document, directory, tool}` | `resolution` | 在 `local.json` 里找最近的固定，返回 `{pin, sourceDirectory}` |
| `toolchains.resolve` | 某个目录的完整解析输入 | `resolution` | 走四层选择，结果是 `resolved`、`skip` 或 `terminalFailure`，附带 `trace`、`unmet`、`conflicts` |
| `toolchains.checkVersion` | `{registry, tool, output, minimumVersion?}` | `version` | 用定义的版本正则取版本并比较；最多 8 段数字、64 字节，不解释别名 |
| `toolchains.instantiateLaunch` | `{registry, tool, executable, home?, entry?, runtimeArguments, arguments}` | `launch` | 按工具定义的 `launch` 模板，为宿主选中的可执行文件展开启动参数，返回 `{executable, arguments, versionArguments}`；见“按工具定义 ID 启动进程” |
| `toolchains.discoveryPlan` | `{registry, platform}` | `plan` | 每个工具的扫描计划和声明它的提供者；最多 256 个根目录、4096 个条目、64 层祖先 |
| `toolchains.candidates` | 设置页的发现输入 | `candidates` | 设置页显示的全局层和自动层：`{tool, shell, installations, skipped, selected, conflicts}`；第一次调用把待探测的可执行文件列为 `probeFailed`，宿主运行探针后再调用一次 |

### 提供者应答的限制

| 项目 | 规则 |
| --- | --- |
| 应答内容 | `result: {operationID, generation, providerID, installations, statements, watch}`；安装是 `{tool, version, home}`，版本要求是 `{tool, scope, source, request, installation, kind, reader}` |
| 大小 | 最多 1 MiB、512 个安装、256 条要求、256 个监视路径 |
| 路径 | 文件系统事实来自宿主，不来自提供者；主目录不能在任何工作区根目录里，必须符合定义的可执行文件布局 |
| 身份 | JSON-RPC `id`、`operationID`、`generation`、`providerID` 必须和请求一致；数字 `1` 和字符串 `"1"` 是不同的 ID；远程 authority 一律拒绝 |
| 版本要求 | `request` 的写法对 Core 不透明（例如 `lts`），宿主不解释；`installation: null` 表示要求没有被满足 |

提供者可以在 JSON-RPC 错误里使用下面的错误码，`decodeReply` 按这张表校验（一个数字只对应一个语义）：

| JSON-RPC | 语义码 | 含义 |
| --- | --- | --- |
| `-32601` | `plugin.toolchain.methodUnavailable` | 查询方法或能力没有协商 |
| `-32602` | `plugin.toolchain.invalidParams` | 请求格式错误或声明的输入无效 |
| `-32800` | `plugin.toolchain.cancelled` | 调用方取消了请求 |
| `-32070` | `plugin.toolchain.permissionDenied` | 缺少权限、审查或启用状态 |
| `-32071` | `plugin.toolchain.authorityUnsupported` | 没有这个 authority 的适配器 |
| `-32072` | `plugin.toolchain.timeout` | 超过本地截止时间 |
| `-32073` | `plugin.toolchain.providerUnavailable` | 后端启动失败、断开或上游查询失败 |
| `-32074` | `plugin.toolchain.invalidReply` | 应答格式错误，或安装引用了别的工具 |
| `-32075` | `plugin.toolchain.limitExceeded` | 超过应答、扫描或监视预算 |
| `-32076` | `plugin.toolchain.unsafePath` | 绝对路径、包含关系或可执行文件证据不安全 |
| `-32077` | `plugin.toolchain.operationExpired` | 发出查询时上下文或代次已经失效 |

### 按工具定义 ID 启动进程

插件用工具定义 ID 启动进程时（`launch.kind` 为 `tool` 的启动计划，或后端的 `run/start` 带 `{kind: "tool"}`），从不自己写这个工具的 argv。定义的 `launch` 模板说明工具怎样接收入口文件、运行时参数和程序参数，宿主和插件都不按工具名分支。

| 项目 | 规则 |
| --- | --- |
| 声明 | 清单顶层 `requiresTools` 列出插件启动进程用到的所有工具定义 ID：最多 32 个，短 ID 或限定 ID，不重复，保存时排序。定义来自插件自己或别的已启用插件的 `toolDefinitions` |
| 引用位置 | `runtime` 启动的 `launch.tool`、`runtimeDependencies[].tool`、调试适配器的 `runSelectedRuntime.tool`。引用 `requiresTools` 没列出的 ID 是 `plugin.runtime.undeclared`，带 `field` |
| 启动计划 | 返回各部分而不是命令行：`launch: {kind: "tool", tool, entry, runtimeArguments, minimumVersion?, packagedRuntime?}`；`arguments` 只含插件自己的参数；`runtimeRequirements` 列出宿主要解析的每个工具。`packagedRuntime` 是插件自带运行时的路径，宿主直接把这个文件当作工具的可执行文件，不再查找 |
| `run/start` | `process.executable` 可以是 `{kind: "tool", tool}`（`tool` 必须在清单 `requiresTools` 里，否则是 `plugin.run.invalidStart`，`field` 为 `process.executable.tool`），或 `{kind: "packaged", path}`（插件包内文件）；见[插件后端](backend.md) |
| 宿主做什么 | 按四层选择解析每个工具，用 `toolchains.checkVersion` 检查 `minimumVersion`，再用 `toolchains.instantiateLaunch` 展开，然后启动 |
| 解析不到 | 工具解析不到可执行文件是 `plugin.host.toolUnresolved`（JSON-RPC `-32025`）；`details` 是 `{tool, reason}`，`reason` 为 `notFound`（四层里没有候选满足定义和最低版本）或 `unusable`（选中的可执行文件不能用） |

`toolchains.instantiateLaunch` 的 `executable` 必填；`home` 是解析出的工具主目录，`entry` 是绝对文件路径，`runtimeArguments` 和 `arguments` 是字符串列表。文本最多 4096 字节，列表最多 1024 项、每项最多 64 KiB，都不含 NUL。模板里的 `${executable}`、`${home}`、`${entry}`、`${runtimeArguments}`、`${arguments}` 被对应的值替换，列表原位展开；其他占位符原样保留。调用方给了模板带不动的值（给不启动入口的工具一个 `entry`，或给不接受参数的工具参数），或漏了模板需要的 `${home}`、`${entry}`，是 `plugin.toolchain.launchMismatch`；未知或已停用的 `tool` 是 `plugin.toolchain.invalidParams`，`field` 为 `tool`。这个命令不做任何本机 I/O：选可执行文件、探测版本、启动进程都由宿主完成。

### 本机文档

`.lithe/toolchains/local.json` 的 `version` 仍为 `1`，这是 Core 当前校验的格式。`toolchains.resolvePins` 校验 `{version: 1, toolchains, pins?}`，`pins` 按目录记录 `{home, version, provider}`。这个文档只读取，不授予执行权限，也不写文件；保存固定尚未实现。

### 桌面端现状

| 用途 | Lithe 桌面端 |
| --- | --- |
| 工具定义（`toolDefinitions`） | 已声明。设置页的发现结果，以及构建和运行选择工具，都通过共享 Core 解析器完成 |
| 工具链提供者（`toolchainProviders`） | 未声明。提供者查询路线不可用，需要它的插件不会被提供安装 |
| 语言服务器的 `{kind: "tool"}` 启动应答 | 启动时被拒绝，因为工具还没有经过工具链注册表解析（见[语言](languages.md)） |
| 保存固定（pin）、`toolchainConsumers`、通用运行时启动、提供者热激活 | 契约尚未提供 |

`toolchains.provide` 可以影响某个已定义工具启动哪个程序，但不授权替换任意命令、安装软件、访问网络或执行工作区代码。

### 契约章节

| 章节 | 内容 |
| --- | --- |
| [Plugin-owned toolchains (Core foundation)](../../contract/v1/README.md#plugin-owned-toolchains-core-foundation) | 契约总入口里的工具链说明和当前宿主状态 |
| [Manifest admission and registry](../../contract/v1/toolchains/README.md#manifest-admission-and-registry) | 能力和权限要求、定义字段、提供者字段、ID 规则（短 ID 与限定 ID） |
| [Two installation routes](../../contract/v1/toolchains/README.md#two-installation-routes) | 声明式发现和提供者查询 |
| [Commands and envelopes](../../contract/v1/toolchains/README.md#commands-and-envelopes) | 九个 Core 命令、返回格式、注册表限制、`conflicts` |
| [Launching a tool definition](../../contract/v1/toolchains/README.md#launching-a-tool-definition) | `toolchains.instantiateLaunch`：按定义的 `launch` 模板展开启动参数、`launchMismatch` |
| [Tool references](../../contract/v1/README.md#tool-references) | 清单 `requiresTools`、引用位置、启动计划的各部分、`plugin.host.toolUnresolved` |
| [Run and debug sessions](../../contract/v1/README.md#run-and-debug-sessions) | `run/start` 的 `process.executable`：`{kind: "tool"}` 或 `{kind: "packaged"}` |
| [Provider reply validation](../../contract/v1/toolchains/README.md#provider-reply-validation) | `decodeReply`、应答格式与限制 |
| [Error replies](../../contract/v1/toolchains/README.md#error-replies) | 错误回复的格式，以及 11 个稳定失败码的数字码对照 |
| [Pins and snapshot resolution](../../contract/v1/toolchains/README.md#pins-and-snapshot-resolution) | `local.json`、`resolve`、四层选择、`checkVersion`、`discoveryPlan` |
| [Settings candidates](../../contract/v1/toolchains/README.md#settings-candidates) | `candidates` 和两次调用的探测流程 |
| [Executable resolution](../../contract/v1/README.md#executable-resolution) | 可执行文件候选的排序和严格选择 |
| [Provider backend negotiation](../../contract/v1/toolchains/README.md#provider-backend-negotiation) | `litheToolchains` 协商、`lsp.toolchainRequest`、超时 |

### 示例与用例

| 文件 | 内容 |
| --- | --- |
| [`toolchains/tool-definition.plugin.json`](../../contract/v1/fixtures/toolchains/tool-definition.plugin.json) | 生产形态的工具定义（`java`）：项目和全局设置、工具链请求、`JAVA_HOME`、`java` 命令等候选，以及 macOS 的 `JavaVirtualMachines` 目录发现 |
| [`toolchains/toolchain-provider.plugin.json`](../../contract/v1/fixtures/toolchains/toolchain-provider.plugin.json) | 生产形态的版本管理器提供者（`mise`）：覆盖多个工具，监视项目版本文件（`.tool-versions` 用兼容读取器），后端是包内可执行文件 |
| [`run/tool-reference-launch.plugin.json`](../../contract/v1/fixtures/run/tool-reference-launch.plugin.json) | 清单顶层 `requiresTools` 列出插件自己定义的 `interpreter`，语言服务器的 `runtime` 启动按这个工具定义 ID 引用运行时 |

Core 的验收测试在主仓库的 [`toolchain_definitions.rs`](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/crates/core/src/tests/toolchain_definitions.rs) 和同目录的 `toolchain_pins.rs`、`toolchain_queries.rs`、`toolchain_resolution.rs`。这三个测试共用的固定值文档、查询往返和每个稳定失败码的错误回复内联在 [`toolchain_vectors.rs`](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/crates/core/src/tests/toolchain_vectors.rs) 里。

### 相关指南

- [为语言插件接入构建、运行与测试](../../guides/build-tool-integration.md)：构建候选集里的 `toolchain` 可执行文件
- [原生构建、测试与运行准备](../../guides/native-build-workflows.md)：运行准备返回的 `executable.kind: "toolchain"`

---

## English

A plugin can tell Lithe what a tool (a JDK, Maven, Node, Go and so on) looks like, where to find it, and how to read its version. A backend can also answer which versions this machine has and which version a project asks for. The host has no built-in tool definitions, alias table, or tool-name branches. Definitions come only from enabled plugins; Core validates and selects; the host performs only native I/O such as filesystem observation, version probes, and backend launches.

| Item | Value |
| --- | --- |
| Manifest fields | `toolDefinitions[]`, `toolchainProviders[]`, and the top-level `requiresTools` (tool-definition IDs that launches reference) |
| Capabilities | `toolDefinitions` 0; `toolchainProviders` 0 (also needs `backend` 0) |
| Protocol | The toolchain provider protocol `litheToolchains`, version `0`: the host queries the provider's backend with `toolchains/query`. Negotiation is in the [contract](../../contract/v1/toolchains/README.md#provider-backend-negotiation) |
| API IDs | [API-23](../api-catalog.md#apis-usable-now): toolchains and managed processes (partly complete) |
| Permissions | None for definitions; a provider needs `backend.launch` and `toolchains.provide` |
| Activation | None needed. The host's first query starts a provider; no page or startup event is needed |
| Backend launch | The provider's backend must be `packaged`, or `runtime` with `packagedRuntime`, and must not depend on a discovered runtime |
| Desktop | Declares `toolDefinitions`; does not declare `toolchainProviders`. See "Desktop status" |
| Official SDKs | None of the three official SDKs implements `litheToolchains` yet; a provider backend handles `toolchains/query` itself |

### Two installation routes

A tool may use one route, both, or neither (relying only on pins, project files, global defaults, and commands from the login shell).

| Route | Declared by | Fits | Runs |
| --- | --- | --- | --- |
| Declarative discovery | The definition's `discovery`, `homeLayouts`, `homeVariable`, `commands`, and `version` | Installations with a fixed directory shape: a single-component `*` glob under the user home, an environment variable, or an absolute system directory, an optional home suffix, and a regular expression over the version probe's output | The host expands the `discoveryPlan` scans; no plugin code runs |
| Provider query | `toolchainProviders[]` answering `toolchains/query` | Locations those primitives cannot express: registries, property lists, manager databases, command output, manager-specific version files | The provider's packaged backend |

The declarative primitives are frozen: root kinds are `home`, `environment`, and `absolute`, and patterns are single-component `*` globs. A new installation shape is answered by a provider, not by a new root kind or host parser. A provider without `watchFiles` only reports installations, and its replies may not contain version requirements.

### Selection order

| Layer | Rule |
| --- | --- |
| Fixed (pin) | The nearest ancestor directory's pin. Matched by whole path segments only; a sibling with a similar prefix does not inherit it |
| Project | The nearest declaring directory; at the same depth, wrapper > environment variable > declaration; within one source, the native reader beats the compatible reader. A nearer project requirement masks a higher one |
| Global | Lithe's global default > an unambiguous (or user-chosen) provider global default > the login shell |
| Automatic | The highest installed version that meets the minimum. Installations added only as pins never take part in automatic selection. Equal highest versions in different homes are a conflict that the user must resolve |

A single operation may pass `fixed: {home}` or `fixed: {executable}`. It precedes directory pins. An invalid explicit choice fails terminally, and lower layers cannot replace it.

A requirement with no installation stays "unmet", with its reason, even when a lower layer selects another installation. Conflicts require an explicit user choice; they are never decided by plugin ID or path order.

### Tool IDs and conflicts

- Any plugin may define a short ID (for example `compiler`) or a qualified ID `<its own plugin ID>:<local ID>`. The prefix of a qualified ID must be the defining plugin's own ID, so qualified IDs of different plugins never collide. The host supplies no provenance evidence, and a plugin's claim of origin is not read. A provider may answer a short ID.
- A short ID defined by more than one enabled plugin belongs to none of them. The registry omits those definitions and registers the others as usual; `discoveryPlan` lists no scans or providers for it; a consumer that resolves it treats it as declared by nobody. Ownership is never decided by arrival order or plugin ID order, since a hostile plugin could otherwise take a tool by loading first or by choosing a smaller ID.
- The registry returns `conflicts: [{tool, pluginIDs}]`: one entry per contested tool, sorted by `tool`, with every defining plugin ID, sorted. A host should tell the user which plugins contest the tool.
- A plugin ID repeated within one input still rejects the whole batch, without replacing earlier owners.

### Core commands

Every command accepts UTF-8 JSON, has a 4 MiB input budget, and performs no native I/O. Success is `{status: "ok", <key>: value}`; refusal is `{status: "rejected", error: {code, message, field?}}`.

| Command | Input | Success key | Purpose |
| --- | --- | --- | --- |
| `toolchains.registry` | `{plugins: [{manifest}]}` | `registry` | Revalidates complete manifests and returns `{definitions, providers, conflicts}`. The host supplies no provenance proof. Short IDs contested by several plugins are not registered and are listed in `conflicts`. Limits: 128 plugins, 512 definitions, 256 providers |
| `toolchains.decodeReply` | The query request, the provider's reply, and the host's observed filesystem facts | `snapshot` | Validates one `toolchains/query` reply and returns `{installations, statements, watch, diagnostics}` |
| `toolchains.failureReply` | `{params, code}` | `reply` | Builds the JSON-RPC error when a query gets no reply. `code` is `plugin.toolchain.timeout` or `plugin.toolchain.providerUnavailable` |
| `toolchains.resolvePins` | `{document, directory, tool}` | `resolution` | Finds the nearest pin in `local.json`; returns `{pin, sourceDirectory}` |
| `toolchains.resolve` | The full resolution input for one directory | `resolution` | Runs the four layers. The result is `resolved`, `skip`, or `terminalFailure`, with `trace`, `unmet`, and `conflicts` |
| `toolchains.checkVersion` | `{registry, tool, output, minimumVersion?}` | `version` | Reads the version with the definition's regular expression and compares it. At most 8 numeric parts and 64 bytes; aliases are not interpreted |
| `toolchains.instantiateLaunch` | `{registry, tool, executable, home?, entry?, runtimeArguments, arguments}` | `launch` | Expands the definition's `launch` template for the executable the host selected, returning `{executable, arguments, versionArguments}`; see "Launching a process by tool-definition ID" |
| `toolchains.discoveryPlan` | `{registry, platform}` | `plan` | Each tool's scan plan and the providers that declare it. Limits: 256 roots, 4096 entries, 64 ancestors |
| `toolchains.candidates` | The settings page's discovery input | `candidates` | The global and automatic layers shown on a settings page: `{tool, shell, installations, skipped, selected, conflicts}`. The first call lists executables to probe as `probeFailed`; the host runs the probes and calls again |

### Limits on provider replies

| Item | Rule |
| --- | --- |
| Content | `result: {operationID, generation, providerID, installations, statements, watch}`. An installation is `{tool, version, home}`; a requirement is `{tool, scope, source, request, installation, kind, reader}` |
| Size | At most 1 MiB, 512 installations, 256 requirements, and 256 watch paths |
| Paths | Filesystem facts come from the host, not the provider. A home may not lie inside any workspace root and must match the definition's executable layout |
| Identity | The JSON-RPC `id`, `operationID`, `generation`, and `providerID` must match the request. The number `1` and the string `"1"` are different IDs. Remote authorities are always refused |
| Version requirements | `request` is opaque to Core (for example `lts`) and the host does not interpret it. `installation: null` means the requirement is unmet |

A provider may use these codes in its JSON-RPC errors; `decodeReply` validates against this table, and each number has one meaning:

| JSON-RPC | Semantic code | Meaning |
| --- | --- | --- |
| `-32601` | `plugin.toolchain.methodUnavailable` | The query method or capability was not negotiated |
| `-32602` | `plugin.toolchain.invalidParams` | Malformed request or invalid declared input |
| `-32800` | `plugin.toolchain.cancelled` | The caller cancelled the request |
| `-32070` | `plugin.toolchain.permissionDenied` | A permission, review, or enabled-state prerequisite is missing |
| `-32071` | `plugin.toolchain.authorityUnsupported` | The provider has no adapter for this authority |
| `-32072` | `plugin.toolchain.timeout` | The local deadline passed |
| `-32073` | `plugin.toolchain.providerUnavailable` | The backend failed to start or disconnected, or its upstream query failed |
| `-32074` | `plugin.toolchain.invalidReply` | Malformed reply, or an installation refers to another tool |
| `-32075` | `plugin.toolchain.limitExceeded` | A reply, scan, or watch budget was exceeded |
| `-32076` | `plugin.toolchain.unsafePath` | An unsafe absolute path, containment relation, or executable evidence |
| `-32077` | `plugin.toolchain.operationExpired` | The context or generation was already stale when the query was sent |

### Launching a process by tool-definition ID

When a plugin starts a process by tool-definition ID (a launch plan with `launch.kind` `tool`, or a backend `run/start` with `{kind: "tool"}`), it never writes that tool's argv. The definition's `launch` template says how the tool receives the entry file, runtime arguments, and program arguments, so neither the host nor the plugin branches on a tool name.

| Item | Rule |
| --- | --- |
| Declaration | The top-level `requiresTools` lists every tool-definition ID the plugin's process launches use: at most 32, short or qualified, without duplicates, stored sorted. Definitions come from the plugin itself or another enabled plugin's `toolDefinitions` |
| References | The `launch.tool` of a `runtime` launch, `runtimeDependencies[].tool`, and a debug adapter's `runSelectedRuntime.tool`. An ID that `requiresTools` does not list is `plugin.runtime.undeclared`, with `field` |
| Launch plan | Returns parts, not a command line: `launch: {kind: "tool", tool, entry, runtimeArguments, minimumVersion?, packagedRuntime?}`; `arguments` holds only the plugin's own arguments; `runtimeRequirements` lists every tool the host resolves. `packagedRuntime` is the path of a runtime the plugin ships; the host uses that file as the tool's executable and does not look the tool up |
| `run/start` | `process.executable` may be `{kind: "tool", tool}` (`tool` must be in `requiresTools`, or the result is `plugin.run.invalidStart` with `field` `process.executable.tool`) or `{kind: "packaged", path}` (a file in the plugin package); see [Plugin backend](backend.md) |
| Host steps | Resolve each tool through the four layers, check `minimumVersion` with `toolchains.checkVersion`, expand the start with `toolchains.instantiateLaunch`, then start |
| Unresolved | A tool with no executable is `plugin.host.toolUnresolved` (JSON-RPC `-32025`); `details` is `{tool, reason}`, where `reason` is `notFound` (no candidate in the four layers meets the definition and minimum version) or `unusable` (the selected executable cannot be used) |

`toolchains.instantiateLaunch` requires `executable`. `home` is the resolved tool home, `entry` an absolute file path, and `runtimeArguments` and `arguments` string lists. A text is at most 4096 bytes; a list has at most 1024 items of 64 KiB each; none may hold a NUL. `${executable}`, `${home}`, `${entry}`, `${runtimeArguments}`, and `${arguments}` are replaced by their values, lists expanding in place; other tokens are kept as written. A value the template cannot carry (an `entry` for a tool that launches none, or arguments for one that takes none), or a missing `${home}` or `${entry}`, is `plugin.toolchain.launchMismatch`. An unknown or disabled `tool` is `plugin.toolchain.invalidParams` with `field` `tool`. The command performs no native I/O: selecting the executable, probing its version, and starting the process are the host's work.

### Local document

`.lithe/toolchains/local.json` keeps `version` `1`, which is the format Core validates today. `toolchains.resolvePins` validates `{version: 1, toolchains, pins?}`, where `pins` maps directory keys to `{home, version, provider}`. The document is read, not written; it grants no execution authority; saving pins is not implemented.

### Desktop status

| Use | Lithe desktop |
| --- | --- |
| Tool definitions (`toolDefinitions`) | Declared. Settings discovery results, and Build and Run tool selection, go through the shared Core resolver |
| Toolchain providers (`toolchainProviders`) | Not declared. The provider query route is unavailable, and a plugin that needs it is not offered for installation |
| Language server `{kind: "tool"}` launch answer | Refused at start, because the tool is not yet resolved through the toolchain registry (see [Languages](languages.md)) |
| Saving pins, `toolchainConsumers`, generic runtime launches, live provider activation | Not provided by the contract yet |

`toolchains.provide` can influence which program a defined tool launches. It does not grant arbitrary command replacement, installation, network access, or execution of workspace code.

### Contract sections

| Section | Content |
| --- | --- |
| [Plugin-owned toolchains (Core foundation)](../../contract/v1/README.md#plugin-owned-toolchains-core-foundation) | The toolchain overview in the contract entry point and the current host status |
| [Manifest admission and registry](../../contract/v1/toolchains/README.md#manifest-admission-and-registry) | Capability and permission requirements, definition and provider fields, ID rules (short and qualified IDs) |
| [Two installation routes](../../contract/v1/toolchains/README.md#two-installation-routes) | Declarative discovery and provider query |
| [Commands and envelopes](../../contract/v1/toolchains/README.md#commands-and-envelopes) | The nine Core commands, return shapes, registry limits, `conflicts` |
| [Launching a tool definition](../../contract/v1/toolchains/README.md#launching-a-tool-definition) | `toolchains.instantiateLaunch`: expanding the launch template, `launchMismatch` |
| [Tool references](../../contract/v1/README.md#tool-references) | The manifest's `requiresTools`, reference sites, the parts of a launch plan, `plugin.host.toolUnresolved` |
| [Run and debug sessions](../../contract/v1/README.md#run-and-debug-sessions) | `process.executable` of `run/start`: `{kind: "tool"}` or `{kind: "packaged"}` |
| [Provider reply validation](../../contract/v1/toolchains/README.md#provider-reply-validation) | `decodeReply`, reply shape and limits |
| [Error replies](../../contract/v1/toolchains/README.md#error-replies) | The error reply format and the numeric codes of the 11 stable failure codes |
| [Pins and snapshot resolution](../../contract/v1/toolchains/README.md#pins-and-snapshot-resolution) | `local.json`, `resolve`, the four layers, `checkVersion`, `discoveryPlan` |
| [Settings candidates](../../contract/v1/toolchains/README.md#settings-candidates) | `candidates` and its two-call probe flow |
| [Executable resolution](../../contract/v1/README.md#executable-resolution) | Ordering and strict selection of executable candidates |
| [Provider backend negotiation](../../contract/v1/toolchains/README.md#provider-backend-negotiation) | `litheToolchains` negotiation, `lsp.toolchainRequest`, timeouts |

### Examples and cases

| File | Content |
| --- | --- |
| [`toolchains/tool-definition.plugin.json`](../../contract/v1/fixtures/toolchains/tool-definition.plugin.json) | A production-shaped tool definition (`java`): candidates from project and global settings, toolchain requests, `JAVA_HOME`, the `java` command, and discovery of `*.jdk` under macOS's `JavaVirtualMachines` directory |
| [`toolchains/toolchain-provider.plugin.json`](../../contract/v1/fixtures/toolchains/toolchain-provider.plugin.json) | A production-shaped version-manager provider (`mise`): several tools, watched project version files (`.tool-versions` through the compatible reader), with a packaged backend |
| [`run/tool-reference-launch.plugin.json`](../../contract/v1/fixtures/run/tool-reference-launch.plugin.json) | `requiresTools` lists the plugin's own `interpreter` definition, and a language server's `runtime` launch references the runtime by that tool-definition ID |

The Core acceptance tests are in [`toolchain_definitions.rs`](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/crates/core/src/tests/toolchain_definitions.rs), with `toolchain_pins.rs`, `toolchain_queries.rs`, and `toolchain_resolution.rs` beside it. The fixture documents, query round trips, and the error reply for each stable failure code shared by these tests are inlined in [`toolchain_vectors.rs`](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/crates/core/src/tests/toolchain_vectors.rs).

### Related guides

- [Build, run, and test for language plugins](../../guides/build-tool-integration.md): a `toolchain` executable in a build candidate set
- [Native build, test, and run preparation](../../guides/native-build-workflows.md): `executable.kind: "toolchain"` in run preparation results
