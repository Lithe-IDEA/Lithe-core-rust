# 工具链 / Toolchains

[中文](#中文) · [English](#english)

## 中文

插件可以告诉 Lithe 一个工具（JDK、Maven、Node、Go 等）长什么样、去哪里找、怎么判断版本，也可以用后端回答“这台机器装了哪些版本、这个项目要哪个版本”。宿主没有内置任何工具定义、别名表或按工具名的分支：工具定义只来自已启用的插件，Core 负责校验和选择，宿主只做文件系统观察、运行版本探针和启动后端这些本机 I/O。

| 项目 | 内容 |
| --- | --- |
| 清单字段 | `toolDefinitions[]`（工具定义）、`toolchainProviders[]`（工具链提供者）、顶层 `requiresTools`（启动时引用的工具定义 ID） |
| 能力 | `toolDefinitions` 0（用到 `category`、`audience` 时为 1）；`toolchainProviders` 0 或 1（还需要 `backend` 0） |
| 协议 | 工具链提供者协议 `litheToolchains`，版本 `0` 或 `1`：宿主通过 `toolchains/query` 向提供者的后端查询。协商见[契约](../../contract/v1/toolchains/README.md#provider-backend-negotiation) |
| API 编号 | [API-23](../api-catalog.md#现在能用的-api)：工具链与受管进程（部分完成） |
| 权限 | 工具定义不需要权限；提供者需要 `backend.launch` 和 `toolchains.provide` |
| 激活事件 | 不需要。提供者由宿主的第一次查询唤起，不需要页面或启动事件 |
| 后端启动 | 提供者的后端只能是包内可执行文件（`packaged`），或包内运行时（`runtime` 加 `packagedRuntime`），不能依赖需要发现的运行时 |
| 桌面端 | 声明 `toolDefinitions` 和 `toolchainProviders` 0；后者的版本 1 尚未声明，见“桌面端现状” |
| 官方 SDK | [Rust SDK](../../sdk/rust/README.md#toolchain-providers) 提供类型化的 `toolchains/query`、能力协商和取消处理。Go、Node 和 Java 后端自行处理该协议 |

### 两条安装来源

一个工具可以走其中一条、两条都走，或者都不走（只靠固定、项目文件、全局默认和登录 shell 里的命令）。

| 来源 | 由谁声明 | 适合 | 谁执行 |
| --- | --- | --- | --- |
| 声明式发现 | 工具定义里的 `discovery`、`homeLayouts`、`homeVariable`、`commands`、`version` | 目录形状固定的安装：用户主目录、环境变量或系统绝对目录下的单段 `*` 通配，可选的主目录后缀，加上对版本探针输出的正则 | 宿主展开 `discoveryPlan` 的扫描，不运行插件代码 |
| 提供者查询 | `toolchainProviders[]` 回答 `toolchains/query` | 声明式原语表达不了的位置：注册表、属性列表、版本管理器数据库、命令输出、版本管理器自己的版本文件 | 提供者的包内后端 |

声明式原语已经冻结：根目录只有 `home`、`environment`、`absolute` 三种，通配只有单段 `*`。新的安装形状由提供者回答，不再增加根目录类型或宿主解析器。没有 `watchFiles` 的提供者只报告安装，应答里不能有版本要求。

版本探针 `version.arguments` 的第一个参数可以是选项（`-version`、`--version`），也可以是子命令（Go 这类只用 `go version` 打印版本的工具写 `version`）；之后的参数只能是选项，选项不带 `=值`。第一个参数之后出现不带 `-` 的词就拒绝，所以探针不会把脚本、文件之类的操作数交给工具。

外部安装报告的可执行文件是解析掉链接后的规范路径。例外是 `executableLayouts` 里的文件链接到另一个文件名：这时报告规范主目录下的布局路径。rustup 把 `cargo`、`rust-analyzer` 链接到自身，按启动时的名字决定运行哪个工具，从链接目标启动只会运行 rustup；版本探针和启动都用布局路径。

外部安装的程序解析后默认必须在主目录内。虚拟环境的 `bin/python3` 链接到环境外的基础解释器，Homebrew 的 `libexec/bin/python3` 链接到 `libexec` 外的 framework，这类工具的定义写 `executableLinksOutsideHome: true`（`toolDefinitions` 0 内的补充，不升版本）。写了以后，程序可以经过一次或多次链接指向主目录外的普通可执行文件，但主目录到程序之间的各级目录（例如 `bin`）解析后必须仍在主目录内，主目录和链接目标都不能在工作区内。这类程序的版本探针和启动都用规范主目录下的布局路径，不用链接目标，所以 Python 仍能按启动路径找到环境的 `pyvenv.cfg` 和包。固定、`homeVariable`、发现规则和提供者报告都按这条规则检查；项目里的候选按 `projectKind` 的规则，不受这个字段影响。Core 早于这个字段的宿主会拒绝写了它的清单，所以用它的插件要求包含它的宿主版本。

### 选择顺序

| 层 | 规则 |
| --- | --- |
| 固定（pin） | 最近的祖先目录的固定；只按完整路径段匹配，前缀相似的兄弟目录不继承 |
| 项目 | 最近的声明目录；同一深度 wrapper > 项目环境（如 `.venv`） > 提供者声明；同一来源里原生读取器优先于兼容读取器。离得近的项目要求遮住更上层的要求。项目候选必须经本机显式信任（`projectCodeAllowed: true`），否则跳过且不运行探针；项目环境的根目录、中间父目录（如 `bin`/`Scripts`）、环境内 `requires` 标记与存在的 `pathPrepend` 目录不得软链逃逸环境或工作区，而叶子可执行文件允许是指向工作区内或工作区外常规可执行文件的单跳/多跳符号链接，校验通过后保留字面路径（如 `<root>/.venv/bin/python`）；wrapper 链接目标仍必须留在工作区根之内 |
| 全局 | Lithe 的全局默认 > 没有歧义（或用户已选）的提供者全局默认 > 登录 shell |
| 自动 | 已安装、满足最低版本的最高版本。只用于固定的安装不参与自动选择。不同主目录有相同最高版本时算冲突，要求用户选择 |

一次操作还可以带 `fixed: {home}` 或 `fixed: {executable}`，它排在目录固定之前。这个显式选择无效时直接失败，更低的层不能替代它。

没有安装的项目要求会带着原因保留为“未满足”，即使更低的层选中了别的安装。冲突必须由用户明确选择，不按插件 ID 或路径先后决定。

### 工具 ID 与冲突

- 任何插件都可以定义短 ID（例如 `compiler`）或限定 ID `<自己的插件 ID>:<本地 ID>`。限定 ID 的前缀必须是定义它的插件自己的 ID，所以不同插件的限定 ID 永远不会冲突。宿主不提供来源证明，插件自称的来源也不会被读取。提供者可以回答短 ID。
- 同一个短 ID 被多个已启用插件定义时，它不归其中任何一方：注册表不登记这些定义，其余定义照常登记；`discoveryPlan` 不给它列扫描和提供者；解析它的消费者把它当作没有人声明。归属从不按到达顺序或插件 ID 顺序决定，否则恶意插件可以靠先加载或取更小的 ID 抢走工具。
- 注册表的输出带 `conflicts: [{tool, pluginIDs}]`：每个被争用的工具一条，按 `tool` 排序，`pluginIDs` 是所有定义它的插件 ID（也排序）。宿主应当告诉用户是哪些插件在争用。
- 同一个插件 ID 在一批输入里重复出现，仍然拒绝整批，不会替换先前登记的归属。

### 设置页里的分组

工具定义可以写 `category`，决定它在设置页「工具链」列表里归哪一组：`runtime`（运行时与 SDK）、`build`（构建工具）、`package`（包管理器）、`debug`（调试与分析）、`lint`（检查与格式化）、`other`（其他，默认）。插件只给自己用的运行时（例如语言服务器背后的 JVM）写 `audience: "plugin"`，它们单独放在最后一组，默认折叠；默认是 `user`。这两个字段只影响显示位置，不影响解析、固定和启动。写别的值会拒绝整个清单。它们属于 `toolDefinitions` 版本 1：用到任一字段的插件声明 `toolDefinitions: 1`，只支持 0 的宿主不会安装；不用它们的插件继续写 `0`。

### Core 命令

所有命令都只处理 UTF-8 JSON，输入上限 4 MiB，不做任何本机 I/O。成功返回 `{status: "ok", <键>: 值}`，拒绝返回 `{status: "rejected", error: {code, message, field?}}`。

| 命令 | 输入 | 成功时的键 | 用途 |
| --- | --- | --- | --- |
| `toolchains.registry` | `{plugins: [{manifest}]}` | `registry` | 重新校验完整清单，汇总定义和提供者，返回 `{definitions, providers, conflicts}`；宿主不提供来源证明；被多个插件争用的短 ID 不登记，列在 `conflicts`；最多 128 个插件、512 个定义、256 个提供者 |
| `toolchains.decodeReply` | 查询请求、提供者应答、宿主观察到的文件系统事实 | `snapshot` | 校验一次 `toolchains/query` 应答，得到 `{installations, statements, watch, diagnostics}` |
| `toolchains.failureReply` | `{params, code}` | `reply` | 查询没有应答时生成对应的 JSON-RPC 错误；`code` 只能是 `plugin.toolchain.timeout` 或 `plugin.toolchain.providerUnavailable` |
| `toolchains.resolvePins` | `{document, directory, tool}` | `resolution` | 在 `local.json` 里找最近的固定，返回 `{pin, sourceDirectory}` |
| `toolchains.resolve` | 工作区目录或[无工作区的全局解析](../../contract/v1/toolchains/README.md#pins-and-snapshot-resolution) | `resolution` | 走四层选择，结果是 `resolved`、`skip` 或 `terminalFailure`，附带 `trace`、`projectCandidates`、`unmet`、`conflicts` |
| `toolchains.checkVersion` | `{registry, tool, output, minimumVersion?}` | `version` | 用定义的版本正则取版本并比较；最多 8 段数字、64 字节，不解释别名 |
| `toolchains.instantiateLaunch` | `{registry, tool, executable, home?, entry?, runtimeArguments, arguments}` | `launch` | 按工具定义的 `launch` 模板，为宿主选中的可执行文件展开启动参数，返回 `{executable, arguments, versionArguments}`；见“按工具定义 ID 启动进程” |
| `toolchains.discoveryPlan` | `{registry, platform}` | `plan` | 每个工具的扫描计划、声明它的提供者、登录 shell 变量和平台过滤后的 `projectFiles`；最多 256 个根目录、4096 个条目、64 层祖先 |
| `toolchains.candidates` | 设置页的发现输入 | `candidates` | 设置页显示的根级项目候选、登录 shell 和自动层：`{tool, project, shell, installations, skipped, selected, conflicts}`；未授权项目候选不给出待探测 `executable`，已授权且安全校验通过后首次调用把待探测可执行文件列为 `probeFailed`，宿主运行探针后再调用一次 |
| `toolchains.settingsView` | `{tools, query, issueLimit?}` | `view` | 设置页工具列表的投影：按解析原因给每个工具定出 `error`、`warning`、`ok`，按文字和“需要处理 / 已手动指定 / 未应用”筛选，按种类、插件或不分组，返回要显示的行 `rows`、前几项问题 `issues` 和计数 `counts`；最多 20,000 个工具。宿主不自己分类、排序或计数，见[Settings list](../../contract/v1/toolchains/README.md#settings-list) |

### 提供者应答的限制

| 项目 | 规则 |
| --- | --- |
| 应答内容 | `result: {operationID, generation, providerID, installations, statements, watch}`；安装是 `{tool, version, home}`，版本要求是 `{tool, scope, source, request, installation, kind, reader}` |
| 大小 | 最多 1 MiB、512 个安装、256 条要求、256 个监视路径 |
| 路径 | 文件系统事实来自宿主，不来自提供者；主目录不能在任何工作区根目录里，必须符合定义的可执行文件布局 |
| 身份 | JSON-RPC `id`、`operationID`、`generation`、`providerID` 必须和请求一致；数字 `1` 和字符串 `"1"` 是不同的 ID；远程 authority 一律拒绝 |
| 版本要求 | `request` 的写法对 Core 不透明（例如 `lts`），宿主不解释；`installation: null` 表示要求没有被满足 |

下面是版本 1 的 JSON-RPC 错误码。`decodeReply` 从重新校验的清单读取 `requires.toolchainProviders`，再校验数字与稳定字符串码。版本 0 继续使用 `-32070`（permissionDenied）、`-32071`（authorityUnsupported），其余号码相同；两个版本不能混用这两组号码。原样转发的 `plugin.function.*` 错误在两个版本里都被拒绝为 `plugin.toolchain.invalidReply`。

| JSON-RPC（版本 1） | 语义码 | 含义 |
| --- | --- | --- |
| `-32601` | `plugin.toolchain.methodUnavailable` | 查询方法或能力没有协商 |
| `-32602` | `plugin.toolchain.invalidParams` | 请求格式错误或声明的输入无效 |
| `-32800` | `plugin.toolchain.cancelled` | 调用方取消了请求 |
| `-32082` | `plugin.toolchain.permissionDenied` | 缺少权限、审查或启用状态 |
| `-32083` | `plugin.toolchain.authorityUnsupported` | 没有这个 authority 的适配器 |
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

### 安装缺失的命令

`systemCommand` 启动只写裸命令名（如 `dlv`、`gopls`、`codelldb`），机器上没有时启动失败。清单顶层 `commandInstallers[]` 告诉宿主怎样安装它，用户在插件管理里点安装即可，不用开终端。安装有两种：**工具安装**用插件已经依赖的工具（`requiresTools` 里的工具定义）运行安装命令；**下载安装**（能力 `commandInstallers` 1）下载上游发布的固定归档，插件包里不用带这个命令。需要能力 `commandInstallers` 0（有下载安装时为 1）和权限 `commands.install`。

| 项目 | 规则 |
| --- | --- |
| 声明 | `id`、`commands`（1–8 个本清单 `systemCommand` 启动或本插件 Agent 工具定义的 `commands` 用到的裸命令，一个命令只归一个安装）、`mayUseNetwork`、`timeoutSeconds?`（1–3600，默认 600）、`targets`；工具安装另有 `tool`、`minimumVersion?`、字面量 `arguments`，下载安装另有 `download` |
| 下载安装 | `download.version` 是上游版本；`download.artifacts` 1–6 个归档，每个 `os`/`arch` 最多一个，各有 HTTPS `url`、可选 `mirrors`（最多 8 个）、完整 `sha256`、`archive`（`zip` 或 `tar.gz`）、可选 `stripPrefix`。只能用 `managed` 目标，不带 `arguments`、`environment`；`commandDirectory` 相对解压根目录。`mayUseNetwork` 必须为 `true`。没有当前平台归档时不提供这个安装 |
| `managed` 目标 | 装进宿主为插件管理的目录（应用数据目录，不在应用包、安装目录或插件包里，卸载插件时删除）。`arguments`（替换安装的参数）或 `environment` 必须用唯一的模板变量 `${installDirectory}`；`commandDirectory` 是命令所在的子目录 |
| `toolDefault` 目标 | 装到工具默认位置，值都是字面量。`directory` 最多 4 个探针，按顺序用同一工具运行，第一个输出绝对目录的探针决定位置；`pathList` 取路径列表第一项，`append` 拼接子目录 |
| 宿主做什么 | `plugins.commandInstallers.requirements` 带上本机 `platform`，列出每个启动缺哪些命令、由哪个安装提供；`toolDefault` 先运行探针，把输出交给 `defaultDirectory`；向用户展示工具和完整命令行（下载安装展示版本和下载地址）、是否联网和每个目标的目录，用户选定后用 `plan` 展开并运行，显示输出、可取消、超时停止，结束后确认命令确实在目录里。下载安装的 `plan` 带 `platform`，返回 `download`（选中平台的归档和 `cacheKey`）；宿主依次尝试 `url` 和镜像下载，解压前校验 SHA-256，安全解压到受管目录下自己的子目录，保留文件权限 |
| 之后的启动 | 宿主查找这个插件的 `systemCommand`、解析它的 Agent 工具的 `kind: "command"` 候选时，除 `PATH` 外还查插件的受管命令目录和记录下来的默认目录 |
| Agent 命令行工具 | `commandInstallers` 0 内的补充：Agent 的 `tool` 是本插件定义的工具时，`requirements` 在 `systemCommand` 启动之后列出 `kind: "agent"`、`id` 为 Agent ID 的使用方，例如用 npm 安装 Agent 的命令行工具；旧 Core 以 `plugin.manifest.invalid` 拒绝只有 Agent 工具用到的安装，需要它的插件同时要求 `agents` 0 |
| 不覆盖 | 构建系统和执行提供者通过 `invocation.executables` 解析的可执行文件 |
| 安装审查 | 每个安装都是审查里的程序（`kind: "commandInstaller"`），改了要运行的内容需要重新批准；下载安装的程序列出全部归档，改地址、镜像、摘要或版本也要重新批准 |

### 独立于插件版本的受管运行时

[`managedRuntimes`](../../contract/v1/README.md#managed-runtimes) 0 为固定归档提供安装前准备与缓存复用。清单声明 `resolve` 后端函数和 `dependencies`，每个依赖固定版本、平台、HTTPS URL、SHA-256、解压格式与布局；可选 `tool` 只供本插件语言服务器会话使用。权限 `runtimes.install` 和全部下载来源参与正常安装审查。

宿主把显式配置、工具发现信息和已缓存候选交给插件函数；函数检查兼容性，返回完整划分 `{runtimes:[{id,home,version}],missing:[id]}`。宿主仅下载未满足项，在暂存目录校验摘要、安全解压，再交插件检查，完整成功后原子发布。外部安装只读复用，无效的显式配置须报错。缓存键不含插件版本，依赖未变的插件更新不重复下载；缺失状态可在启动前修复。插件后端及其自身运行时必须随包提供。

Core 的 `plugins.managedRuntimes.plan` 选择准确平台并生成缓存键，`plugins.managedRuntimes.decodeResolution` 校验应答身份和路径；它们没有文件或网络 I/O。语言服务器启动可收到只读 `runtimes: {id:{home,version}}` 映射。平台宿主负责下载、文件锁、完整性和原子发布，不判断特定工具的版本；全局与项目工具链不随之改变。该能力需要配套宿主接入并声明后才能安装使用。

### 本机文档

`.lithe/toolchains/local.json` 的 `version` 为 `1`，`toolchains.resolvePins` / `editPins` / `editPinSources` 校验并按条件更新目录级固定 `{home, version, provider}`，不授予项目代码执行权限。本机应用存储目录下的 `toolchains/global.json`（经 `toolchains.catalogState` / `editCatalog` 管理）保存本机注册的 SDK 目录、全局默认以及显式信任的工作区根路径 `trustedWorkspaces`（绝不写进工作区 `.lithe/`）。

### 桌面端现状

| 用途 | Lithe 桌面端 |
| --- | --- |
| 工具定义（`toolDefinitions`） | 已声明（macOS、Windows、Linux）。设置页发现、项目环境信任、目录固定保存，以及构建、运行、调试、语言服务器和新建项目选择工具，都通过共享 Core 解析器完成 |
| 工具链提供者（`toolchainProviders`） | 已声明 `0`，按需通过共享插件后端查询，并由 Core 校验原始应答与本机文件系统事实；版本 `1` 尚未声明 |
| 安装缺失的命令（`commandInstallers`） | 已声明 `0`。插件管理页展示工具、命令行、网络与目标目录，用户确认后安装到受管目录或工具默认目录；下载安装（`1`）由配套宿主接入后声明 |
| 语言服务器的 `{kind: "tool"}` 启动 | 按工作区工具链四层选择解析后启动（见[语言](languages.md)） |
| `toolchainConsumers`、提供者热激活 | 契约尚未提供 |

`toolchains.provide` 可以影响某个已定义工具启动哪个程序，但不授权替换任意命令、安装软件、访问网络或执行工作区代码。

### 契约章节

| 章节 | 内容 |
| --- | --- |
| [Plugin-owned toolchains (Core foundation)](../../contract/v1/README.md#plugin-owned-toolchains-core-foundation) | 契约总入口里的工具链说明和当前宿主状态 |
| [Manifest admission and registry](../../contract/v1/toolchains/README.md#manifest-admission-and-registry) | 能力和权限要求、定义字段、提供者字段、ID 规则（短 ID 与限定 ID） |
| [Two installation routes](../../contract/v1/toolchains/README.md#two-installation-routes) | 声明式发现和提供者查询 |
| [Commands and envelopes](../../contract/v1/toolchains/README.md#commands-and-envelopes) | 十个 Core 命令、返回格式、注册表限制、`conflicts` |
| [Launching a tool definition](../../contract/v1/toolchains/README.md#launching-a-tool-definition) | `toolchains.instantiateLaunch`：按定义的 `launch` 模板展开启动参数、`launchMismatch` |
| [Tool references](../../contract/v1/README.md#tool-references) | 清单 `requiresTools`、引用位置、启动计划的各部分、`plugin.host.toolUnresolved` |
| [Run and debug sessions](../../contract/v1/README.md#run-and-debug-sessions) | `run/start` 的 `process.executable`：`{kind: "tool"}` 或 `{kind: "packaged"}` |
| [Provider reply validation](../../contract/v1/toolchains/README.md#provider-reply-validation) | `decodeReply`、应答格式与限制 |
| [Error replies](../../contract/v1/toolchains/README.md#error-replies) | 错误回复的格式，以及 11 个稳定失败码的数字码对照 |
| [Pins and snapshot resolution](../../contract/v1/toolchains/README.md#pins-and-snapshot-resolution) | `local.json`、`resolve`、四层选择、`checkVersion`、`discoveryPlan` |
| [Settings candidates](../../contract/v1/toolchains/README.md#settings-candidates) | `candidates` 和两次调用的探测流程 |
| [Settings list](../../contract/v1/toolchains/README.md#settings-list) | `settingsView`：状态分类、筛选、分组、折叠默认值和计数 |
| [Executable resolution](../../contract/v1/README.md#executable-resolution) | 可执行文件候选的排序和严格选择 |
| [Command installers](../../contract/v1/README.md#command-installers) | `commandInstallers[]` 的字段、两个目标、三个 Core 命令和宿主步骤 |
| [Provider backend negotiation](../../contract/v1/toolchains/README.md#provider-backend-negotiation) | `litheToolchains` 协商、`lsp.toolchainRequest`、超时 |

### 示例与用例

| 文件 | 内容 |
| --- | --- |
| [`toolchains/tool-definition.plugin.json`](../../contract/v1/fixtures/toolchains/tool-definition.plugin.json) | 生产形态的工具定义（`java`）：项目和全局设置、工具链请求、`JAVA_HOME`、`java` 命令等候选，以及 macOS 的 `JavaVirtualMachines` 目录发现 |
| [`toolchains/toolchain-provider.plugin.json`](../../contract/v1/fixtures/toolchains/toolchain-provider.plugin.json) | 生产形态的版本管理器提供者（`mise`）：覆盖多个工具，监视项目版本文件（`.tool-versions` 用兼容读取器），后端是包内可执行文件 |
| [`run/tool-reference-launch.plugin.json`](../../contract/v1/fixtures/run/tool-reference-launch.plugin.json) | 清单顶层 `requiresTools` 列出插件自己定义的 `interpreter`，语言服务器的 `runtime` 启动按这个工具定义 ID 引用运行时 |
| [`command-installers/command-installer.plugin.json`](../../contract/v1/fixtures/command-installers/command-installer.plugin.json)、[`command-installer-plans.json`](../../contract/v1/fixtures/command-installers/command-installer-plans.json) | 语言服务器和调试适配器缺的命令各有一个安装：一个两种目标都有、用环境变量传目录，一个只有 `managed`、替换参数并指定命令子目录；以及三个命令在 Unix 和 Windows 上的期望结果 |

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
| Capabilities | `toolDefinitions` 0; `toolchainProviders` 0 or 1 (also needs `backend` 0) |
| Protocol | The toolchain provider protocol `litheToolchains`, version `0` or `1`: the host queries the provider's backend with `toolchains/query`. Negotiation is in the [contract](../../contract/v1/toolchains/README.md#provider-backend-negotiation) |
| API IDs | [API-23](../api-catalog.md#apis-usable-now): toolchains and managed processes (partly complete) |
| Permissions | None for definitions; a provider needs `backend.launch` and `toolchains.provide` |
| Activation | None needed. The host's first query starts a provider; no page or startup event is needed |
| Backend launch | The provider's backend must be `packaged`, or `runtime` with `packagedRuntime`, and must not depend on a discovered runtime |
| Desktop | Declares `toolDefinitions` and `toolchainProviders` 0; version 1 of the latter is not declared yet. See "Desktop status" |
| Official SDKs | The [Rust SDK](../../sdk/rust/README.md#toolchain-providers) handles typed `toolchains/query`, negotiation, and cancellation. Go, Node, and Java backends handle the protocol directly |

### Two installation routes

A tool may use one route, both, or neither (relying only on pins, project files, global defaults, and commands from the login shell).

| Route | Declared by | Fits | Runs |
| --- | --- | --- | --- |
| Declarative discovery | The definition's `discovery`, `homeLayouts`, `homeVariable`, `commands`, and `version` | Installations with a fixed directory shape: a single-component `*` glob under the user home, an environment variable, or an absolute system directory, an optional home suffix, and a regular expression over the version probe's output | The host expands the `discoveryPlan` scans; no plugin code runs |
| Provider query | `toolchainProviders[]` answering `toolchains/query` | Locations those primitives cannot express: registries, property lists, manager databases, command output, manager-specific version files | The provider's packaged backend |

The declarative primitives are frozen: root kinds are `home`, `environment`, and `absolute`, and patterns are single-component `*` globs. A new installation shape is answered by a provider, not by a new root kind or host parser. A provider without `watchFiles` only reports installations, and its replies may not contain version requirements.

The first argument of the version probe `version.arguments` is a flag (`-version`, `--version`) or a subcommand (`version`, for a tool such as Go that prints its version only with `go version`); every later argument is a flag, and flags carry no `=value`. A bare word after the first argument is refused, so a probe never hands the tool an operand such as a script or a file.

An external installation reports its canonical executable, with links resolved. The exception is a file in `executableLayouts` that links to a file with another name: Core then reports the layout path beneath the canonical home. rustup links `cargo` and `rust-analyzer` to itself and picks the tool from the name it starts with; started by the link target's name it runs rustup. Version probes and launches use the layout path.

By default an external installation's program must resolve inside its home. A virtual environment's `bin/python3` links to its base interpreter outside the environment, and Homebrew's `libexec/bin/python3` links to the framework outside `libexec`; a definition for such a tool sets `executableLinksOutsideHome: true` (an addition within `toolDefinitions` 0, with no version bump). The program may then be a single-hop or multi-hop link to a regular executable file outside the home, but every directory between the home and the program (such as `bin`) must still resolve inside the home, and neither the home nor the link target may lie in a workspace. Version probes and launches use the layout path beneath the canonical home, not the link target, so Python still finds the environment's `pyvenv.cfg` and packages from the path it starts with. Pins, `homeVariable`, discovery and provider reports all go through this check; project candidates follow their `projectKind` rules and ignore the field. A host whose Core predates the field rejects manifests that set it, so a plugin that uses it requires a host release that has it.

### Selection order

| Layer | Rule |
| --- | --- |
| Fixed (pin) | The nearest ancestor directory's pin. Matched by whole path segments only; a sibling with a similar prefix does not inherit it |
| Project | The nearest declaring directory; at the same depth, wrapper > project environment (such as `.venv`) > provider declaration; within one source, the native reader beats the compatible reader. A nearer project requirement masks a higher one. Project candidates require explicit machine-local workspace trust (`projectCodeAllowed: true`) and are never passively probed; for a project environment, `home`, intermediate parent directories (`bin` / `Scripts`), in-home `requires` markers, and existing `pathPrepend` directories must remain canonically inside `home` and the workspace, while the leaf executable may be a single-hop or multi-hop symbolic link to a regular executable inside or outside the workspace and preserves its lexical `executable` path (such as `<root>/.venv/bin/python`); wrapper targets must remain inside the workspace root |
| Global | Lithe's global default > an unambiguous (or user-chosen) provider global default > the login shell |
| Automatic | The highest installed version that meets the minimum. Installations added only as pins never take part in automatic selection. Equal highest versions in different homes are a conflict that the user must resolve |

A single operation may pass `fixed: {home}` or `fixed: {executable}`. It precedes directory pins. An invalid explicit choice fails terminally, and lower layers cannot replace it.

A requirement with no installation stays "unmet", with its reason, even when a lower layer selects another installation. Conflicts require an explicit user choice; they are never decided by plugin ID or path order.

### Tool IDs and conflicts

- Any plugin may define a short ID (for example `compiler`) or a qualified ID `<its own plugin ID>:<local ID>`. The prefix of a qualified ID must be the defining plugin's own ID, so qualified IDs of different plugins never collide. The host supplies no provenance evidence, and a plugin's claim of origin is not read. A provider may answer a short ID.
- A short ID defined by more than one enabled plugin belongs to none of them. The registry omits those definitions and registers the others as usual; `discoveryPlan` lists no scans or providers for it; a consumer that resolves it treats it as declared by nobody. Ownership is never decided by arrival order or plugin ID order, since a hostile plugin could otherwise take a tool by loading first or by choosing a smaller ID.
- The registry returns `conflicts: [{tool, pluginIDs}]`: one entry per contested tool, sorted by `tool`, with every defining plugin ID, sorted. A host should tell the user which plugins contest the tool.
- A plugin ID repeated within one input still rejects the whole batch, without replacing earlier owners.

### Groups on the settings page

A tool definition may set `category` to choose its group in the settings page's Toolchains list: `runtime` (runtimes and SDKs), `build` (build tools), `package` (package managers), `debug` (debuggers and profilers), `lint` (checkers and formatters) or `other` (the default). A runtime a plugin only runs for itself, such as the JVM behind a language server, sets `audience: "plugin"`; such tools sit in their own last group, collapsed by default. The default is `user`. Both fields only change where the tool is shown, never resolution, pins or launches. Any other value rejects the whole manifest. They are `toolDefinitions` version 1: a plugin that sets either declares `toolDefinitions: 1`, and a host that supports only 0 does not install it; plugins that do not use them keep `0`.

### Core commands

Every command accepts UTF-8 JSON, has a 4 MiB input budget, and performs no native I/O. Success is `{status: "ok", <key>: value}`; refusal is `{status: "rejected", error: {code, message, field?}}`.

| Command | Input | Success key | Purpose |
| --- | --- | --- | --- |
| `toolchains.registry` | `{plugins: [{manifest}]}` | `registry` | Revalidates complete manifests and returns `{definitions, providers, conflicts}`. The host supplies no provenance proof. Short IDs contested by several plugins are not registered and are listed in `conflicts`. Limits: 128 plugins, 512 definitions, 256 providers |
| `toolchains.decodeReply` | The query request, the provider's reply, and the host's observed filesystem facts | `snapshot` | Validates one `toolchains/query` reply and returns `{installations, statements, watch, diagnostics}` |
| `toolchains.failureReply` | `{params, code}` | `reply` | Builds the JSON-RPC error when a query gets no reply. `code` is `plugin.toolchain.timeout` or `plugin.toolchain.providerUnavailable` |
| `toolchains.resolvePins` | `{document, directory, tool}` | `resolution` | Finds the nearest pin in `local.json`; returns `{pin, sourceDirectory}` |
| `toolchains.resolve` | A workspace directory or [global resolution without a workspace](../../contract/v1/toolchains/README.md#pins-and-snapshot-resolution) | `resolution` | Runs the four layers. The result is `resolved`, `skip`, or `terminalFailure`, with `trace`, `projectCandidates`, `unmet`, and `conflicts` |
| `toolchains.checkVersion` | `{registry, tool, output, minimumVersion?}` | `version` | Reads the version with the definition's regular expression and compares it. At most 8 numeric parts and 64 bytes; aliases are not interpreted |
| `toolchains.instantiateLaunch` | `{registry, tool, executable, home?, entry?, runtimeArguments, arguments}` | `launch` | Expands the definition's `launch` template for the executable the host selected, returning `{executable, arguments, versionArguments}`; see "Launching a process by tool-definition ID" |
| `toolchains.discoveryPlan` | `{registry, platform}` | `plan` | Each tool's scan plan, the providers that declare it, login-shell variables, and platform-filtered `projectFiles`. Limits: 256 roots, 4096 entries, 64 ancestors |
| `toolchains.candidates` | The settings page's discovery input | `candidates` | Root project candidates, login-shell homes, and the automatic layer shown on a settings page: `{tool, project, shell, installations, skipped, selected, conflicts}`. Untrusted project candidates emit no `executable` to probe; once trusted and checked, the first call lists executables to probe as `probeFailed`, and the host runs the probes and calls again |
| `toolchains.settingsView` | `{tools, query, issueLimit?}` | `view` | Projects the settings page's tool list: classifies each tool as `error`, `warning` or `ok` from its resolver reason, filters by text and by "needs attention / chosen manually / not applied", groups by category, plugin or nothing, and returns the rows to show (`rows`), the first problems (`issues`) and `counts`. At most 20,000 tools. The host does not classify, sort or count by itself; see [Settings list](../../contract/v1/toolchains/README.md#settings-list) |

### Limits on provider replies

| Item | Rule |
| --- | --- |
| Content | `result: {operationID, generation, providerID, installations, statements, watch}`. An installation is `{tool, version, home}`; a requirement is `{tool, scope, source, request, installation, kind, reader}` |
| Size | At most 1 MiB, 512 installations, 256 requirements, and 256 watch paths |
| Paths | Filesystem facts come from the host, not the provider. A home may not lie inside any workspace root and must match the definition's executable layout |
| Identity | The JSON-RPC `id`, `operationID`, `generation`, and `providerID` must match the request. The number `1` and the string `"1"` are different IDs. Remote authorities are always refused |
| Version requirements | `request` is opaque to Core (for example `lts`) and the host does not interpret it. `installation: null` means the requirement is unmet |

These are the version 1 JSON-RPC error numbers. `decodeReply` reads `requires.toolchainProviders` from the revalidated manifest before checking the number and stable code. Version 0 retains `-32070` (permissionDenied) and `-32071` (authorityUnsupported); the other numbers are identical. A provider cannot mix the two versions' numbers. Forwarded `plugin.function.*` errors are rejected as `plugin.toolchain.invalidReply` in both versions.

| JSON-RPC (version 1) | Semantic code | Meaning |
| --- | --- | --- |
| `-32601` | `plugin.toolchain.methodUnavailable` | The query method or capability was not negotiated |
| `-32602` | `plugin.toolchain.invalidParams` | Malformed request or invalid declared input |
| `-32800` | `plugin.toolchain.cancelled` | The caller cancelled the request |
| `-32082` | `plugin.toolchain.permissionDenied` | A permission, review, or enabled-state prerequisite is missing |
| `-32083` | `plugin.toolchain.authorityUnsupported` | The provider has no adapter for this authority |
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

### Installing missing commands

A `systemCommand` launch names only a bare command (such as `dlv`, `gopls` or `codelldb`), and fails when the machine lacks it. The top-level `commandInstallers[]` tells the host how to install it, so the user installs it from plugin management instead of a terminal. A **tool installer** runs a tool the plugin already requires (a tool definition in `requiresTools`); a **download installer** (capability `commandInstallers` 1) downloads a pinned archive its upstream publishes, so the plugin package does not carry the command. It requires the `commandInstallers` 0 capability (1 with a download installer) and the `commands.install` permission.

| Item | Rule |
| --- | --- |
| Declaration | `id`, `commands` (1 to 8 bare commands that `systemCommand` launches of this manifest name; one installer per command), `mayUseNetwork`, `timeoutSeconds?` (1 to 3600, 600 by default), `targets`; a tool installer adds `tool`, `minimumVersion?` and literal `arguments`, a download installer adds `download` |
| Download installer | `download.version` is the upstream version; `download.artifacts` holds 1 to 6 archives, at most one per `os`/`arch`, each with an HTTPS `url`, optional `mirrors` (up to 8), the full `sha256`, `archive` (`zip` or `tar.gz`) and optional `stripPrefix`. Only the `managed` target, without `arguments` or `environment`; `commandDirectory` is relative to the extracted root. `mayUseNetwork` is `true`. The installer is not offered on a platform without an archive |
| `managed` target | Installs into a directory the host manages for the plugin (in application data, never in the app bundle, installation directory or plugin package; removed with the plugin). `arguments` (replacing the installer's) or `environment` must use `${installDirectory}`, the only template variable; `commandDirectory` is the subdirectory the commands land in |
| `toolDefault` target | Installs wherever the tool installs by default; every value is literal. `directory` holds up to 4 probes run in order with the same tool; the first to print an absolute directory decides; `pathList` takes the first entry of a path list and `append` adds a subdirectory |
| Host steps | `plugins.commandInstallers.requirements`, sent with the host `platform`, lists which commands each launch lacks and which installer provides them; for `toolDefault` the host runs the probes and sends their output to `defaultDirectory`; it shows the tool and the full command line (for a download installer the version and download URL), the network use and the directory of each target, then expands the chosen one with `plan` and runs it, showing the output, allowing cancel and stopping it at the timeout, and finally checks the commands are in the directory. A download installer's `plan` takes `platform` and returns `download` (the platform's archive and its `cacheKey`); the host tries `url` and then the mirrors, checks the SHA-256 before extracting, and extracts safely into a directory of its own below the managed directory, keeping file modes |
| Later launches | The host looks for the plugin's `systemCommand` launches in its managed command directory and the recorded default directories as well as on `PATH` |
| Not covered | Executables of build systems and execution providers resolved through `invocation.executables` |
| Install review | Every installer is a reviewed program (`kind: "commandInstaller"`); a change to what it runs needs approval again. A download installer's program lists every archive, so a new URL, mirror, digest or version needs approval again too |

### Managed runtimes independent of plugin version

[`managedRuntimes`](../../contract/v1/README.md#managed-runtimes) 0 provides install preflight and reuse of pinned archives. A manifest declares a `resolve` backend function and `dependencies`; each dependency pins a version, exact platform, HTTPS URL, SHA-256, archive format and layout. An optional `tool` binding serves only this plugin's language-server session. `runtimes.install` and every download origin participate in normal install review.

The host supplies effective settings, discovered tools and cached candidates to the plugin function, which qualifies compatibility and returns the complete partition `{runtimes:[{id,home,version}],missing:[id]}`. The host downloads only missing items, verifies bytes and safe extraction in staging, and asks the plugin to qualify them before atomic publication of the complete set. External installations are reused read-only; invalid explicit settings are errors. Cache keys omit plugin version, so unchanged dependencies survive updates without downloads. Missing state can be repaired before launch. The backend and its own runtime must be packaged.

Core's `plugins.managedRuntimes.plan` selects the exact platform and generates cache identities; `plugins.managedRuntimes.decodeResolution` validates reply identity and paths. Neither performs file or network I/O. Server launch may receive a read-only `runtimes: {id:{home,version}}` map. Native hosts own downloads, locking, integrity and atomic publication without tool-specific rules; global and project toolchains stay independent. A companion host must implement and advertise the capability before plugins can require it.

### Local document

`.lithe/toolchains/local.json` keeps `version` `1`; `toolchains.resolvePins`, `editPins`, and `editPinSources` validate and conditionally update directory pins `{home, version, provider}`, granting no execution authority over project code. Machine-local `toolchains/global.json` under the application preference directory (managed through `toolchains.catalogState` and `editCatalog`) stores registered SDK homes, global defaults, and explicitly trusted workspace roots `trustedWorkspaces` (never written inside `.lithe/`).

### Desktop status

| Use | Lithe desktop |
| --- | --- |
| Tool definitions (`toolDefinitions`) | Declared (macOS, Windows, and Linux). Settings discovery, workspace project-environment trust, saving pins, and Build, Run, Debug, language-server, and project-generator tool selection all go through the shared Core resolver |
| Toolchain providers (`toolchainProviders`) | Declares `0`: queries the shared plugin backend on demand and passes raw replies and native filesystem facts to Core for validation; version `1` is not declared yet |
| Installing missing commands (`commandInstallers`) | `0` declared. Plugin management displays the tool, command line, network use, and target directory, and installs into the managed directory or tool default directory after user confirmation; download installers (`1`) are declared with companion host integration |
| Language server `{kind: "tool"}` launch | Resolved through the workspace's four toolchain layers and started (see [Languages](languages.md)) |
| `toolchainConsumers`, live provider activation | Not provided by the contract yet |

`toolchains.provide` can influence which program a defined tool launches. It does not grant arbitrary command replacement, installation, network access, or execution of workspace code.

### Contract sections

| Section | Content |
| --- | --- |
| [Plugin-owned toolchains (Core foundation)](../../contract/v1/README.md#plugin-owned-toolchains-core-foundation) | The toolchain overview in the contract entry point and the current host status |
| [Manifest admission and registry](../../contract/v1/toolchains/README.md#manifest-admission-and-registry) | Capability and permission requirements, definition and provider fields, ID rules (short and qualified IDs) |
| [Two installation routes](../../contract/v1/toolchains/README.md#two-installation-routes) | Declarative discovery and provider query |
| [Commands and envelopes](../../contract/v1/toolchains/README.md#commands-and-envelopes) | The ten Core commands, return shapes, registry limits, `conflicts` |
| [Launching a tool definition](../../contract/v1/toolchains/README.md#launching-a-tool-definition) | `toolchains.instantiateLaunch`: expanding the launch template, `launchMismatch` |
| [Tool references](../../contract/v1/README.md#tool-references) | The manifest's `requiresTools`, reference sites, the parts of a launch plan, `plugin.host.toolUnresolved` |
| [Run and debug sessions](../../contract/v1/README.md#run-and-debug-sessions) | `process.executable` of `run/start`: `{kind: "tool"}` or `{kind: "packaged"}` |
| [Provider reply validation](../../contract/v1/toolchains/README.md#provider-reply-validation) | `decodeReply`, reply shape and limits |
| [Error replies](../../contract/v1/toolchains/README.md#error-replies) | The error reply format and the numeric codes of the 11 stable failure codes |
| [Pins and snapshot resolution](../../contract/v1/toolchains/README.md#pins-and-snapshot-resolution) | `local.json`, `resolve`, the four layers, `checkVersion`, `discoveryPlan` |
| [Settings candidates](../../contract/v1/toolchains/README.md#settings-candidates) | `candidates` and its two-call probe flow |
| [Settings list](../../contract/v1/toolchains/README.md#settings-list) | `settingsView`: status classes, filters, groups, default folding and counts |
| [Executable resolution](../../contract/v1/README.md#executable-resolution) | Ordering and strict selection of executable candidates |
| [Command installers](../../contract/v1/README.md#command-installers) | The fields of `commandInstallers[]`, the two targets, the three Core commands and the host steps |
| [Provider backend negotiation](../../contract/v1/toolchains/README.md#provider-backend-negotiation) | `litheToolchains` negotiation, `lsp.toolchainRequest`, timeouts |

### Examples and cases

| File | Content |
| --- | --- |
| [`toolchains/tool-definition.plugin.json`](../../contract/v1/fixtures/toolchains/tool-definition.plugin.json) | A production-shaped tool definition (`java`): candidates from project and global settings, toolchain requests, `JAVA_HOME`, the `java` command, and discovery of `*.jdk` under macOS's `JavaVirtualMachines` directory |
| [`toolchains/toolchain-provider.plugin.json`](../../contract/v1/fixtures/toolchains/toolchain-provider.plugin.json) | A production-shaped version-manager provider (`mise`): several tools, watched project version files (`.tool-versions` through the compatible reader), with a packaged backend |
| [`run/tool-reference-launch.plugin.json`](../../contract/v1/fixtures/run/tool-reference-launch.plugin.json) | `requiresTools` lists the plugin's own `interpreter` definition, and a language server's `runtime` launch references the runtime by that tool-definition ID |
| [`command-installers/command-installer.plugin.json`](../../contract/v1/fixtures/command-installers/command-installer.plugin.json), [`command-installer-plans.json`](../../contract/v1/fixtures/command-installers/command-installer-plans.json) | One installer each for the commands of a language server and a debug adapter: one with both targets that passes the directory through the environment, one with only `managed` that replaces the arguments and names a command subdirectory; and the expected results of the three commands on Unix and Windows |

The Core acceptance tests are in [`toolchain_definitions.rs`](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/crates/core/src/tests/toolchain_definitions.rs), with `toolchain_pins.rs`, `toolchain_queries.rs`, and `toolchain_resolution.rs` beside it. The fixture documents, query round trips, and the error reply for each stable failure code shared by these tests are inlined in [`toolchain_vectors.rs`](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/crates/core/src/tests/toolchain_vectors.rs).

### Related guides

- [Build, run, and test for language plugins](../../guides/build-tool-integration.md): a `toolchain` executable in a build candidate set
- [Native build, test, and run preparation](../../guides/native-build-workflows.md): `executable.kind: "toolchain"` in run preparation results
