# 插件协议更新记录

本页记录插件协议和 API 的每次修改：哪天、哪个 PR、谁改的、改了什么。只有中文。

- **收录范围**：Lithe-core-rust 仓库 `main` 分支上的 PR，只要它修改了下面三处中的任意一处：插件契约 `shared/plugins/contract/v1/`、Core 能力表 [`capabilities.rs`](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/crates/plugin-model/src/capabilities.rs)、插件函数协议 [`plugin-rpc/src/`](https://github.com/Lithe-IDEA/Lithe-core-rust/tree/main/rust/crates/plugin-rpc/src)。三处都没改的 PR 不收录。
- **日期**：PR 合入 `main` 的日期。
- **修改人**：PR 的作者（GitHub 账号）。
- **每一版的内容和宿主现状**：见[能力与协议版本对照](capabilities-and-versions.md)。字段和行为以 [`contract/v1/README.md`](../contract/v1/README.md) 为准。

时间线的两列这样读：

- **API 编号**：这个 PR 涉及的 API-01–44 规划编号，来自 [API 实施计划](https://github.com/1lck/Lithe-IDEA/blob/preview/docs/development/macos-plugin-api-plan.md)，和协议版本、能力版本分开记录。— 表示没有改到具体 API，例如清单、市场、包信任、SDK 或契约文档整理。
- **协议 / 能力**：**加粗**的是这个 PR 新增的版本号；没有加粗的，是同一版本内的补充、宿主接入或说明修正。

## 按协议查首次出现

所有版本都从 `0` 开始。以后一个 PR 给某个能力加版本 `1` 时，在对应的表里为它加一行。

### 插件 API 版本与文档格式

| 名称 | 版本 | 日期 | PR |
| --- | --- | --- | --- |
| 插件 API 版本 `pluginAPIVersion` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| 插件清单 `schemaVersion` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| 市场索引 `schemaVersion` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| 原生 UI 文档 `schemaVersion` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `litheBuildModel`、`litheDependencies`、`litheTestResults`、`litheTestDiscovery`、`litheBuildConfiguration`、`litheConfigurationEdit` 和 `modelInputs` 快照的 `version` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `plugins.editorActions.decode` 请求的 `version` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| 插件网页桥 `window.lithe.version` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |

| `litheSettingsSources`、`litheSettingsSourceEdit` | 0 | 2026-11-15 | [#57](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/57) |

### 能力版本（`requires`）

各项能力都从版本 `0` 开始。

| 能力 | 版本 | 日期 | PR |
| --- | --- | --- | --- |
| `backend` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `buildModelInputs` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `buildProviders` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `buildSystems` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `commandInstallers` | 0 | 2026-11-15 | [#53](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/53) |
| `debugAdapters` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `executionProviders` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `keymapImportProviders` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `languageServers` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `languages` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `outputLinkMatchers` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `packageTrust` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `pluginFunctions` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `projectGenerators` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `sourceSettings` | 0 | 2026-11-15 | [#57](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/57) |
| `settings` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `toolDefinitions` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `toolchainProviders` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `ui` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `vcsProviders` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |

### 进程协议版本

进程协议没有独立的版本，它的版本就是拥有它的能力的版本（`litheLogging` 是 `pluginAPIVersion` 的版本）。

| 协议 | 版本 | 日期 | PR |
| --- | --- | --- | --- |
| `litheFunctions`（`backend`） | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `litheBuild`（`buildProviders`） | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `litheVcs`（`vcsProviders`） | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `litheToolchains`（`toolchainProviders`） | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `litheKeymapImport`（`keymapImportProviders`） | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `litheLogging`（`pluginAPIVersion`） | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |

### 宿主方法（按 API 编号）

向后兼容地新增宿主方法、通知或可选字段时不升版本（见契约 [Extending the host services](../contract/v1/README.md#extending-the-host-services)），上面的版本表看不出某个方法是什么时候加的，所以宿主方法和通知在这里逐个登记。一个 API 编号下可以有多个方法，每个方法一行，记首次加入的日期和 PR。方法名省略前缀 `lithe/host/`；用途和权限见 [API 总表](api-catalog.md#已公开的宿主方法)。`keymapImport/*` 属于 `litheKeymapImport` 协议，不在 API-01–44 内。`functions/providers`、`functions/invoke` 是 `litheFunctions` 里的跨插件调用，只对声明了至少一个 `calls` 的后端开放。

| API | 方法 | 协议 | 日期 | PR |
| --- | --- | --- | --- | --- |
| API-01、API-37 | `getCapabilities` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-01 | `subscriptions/dispose` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-02 | `commands/didRun`（通知） | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-04、API-05 | `workspace/didChangeContexts`（通知） | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-05 | `workspace/listContexts` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-05 | `workspace/getContext` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-05 | `workspace/getFolders` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-06 | `workspace/readDirectory` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-06 | `workspace/stat` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-06 | `workspace/readFile` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-07 | `workspace/watch` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-07、API-24 | `subscriptions/notify`（通知） | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-08 | `workspace/findFiles` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-08 | `workspace/findText` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-09 | `documents/list` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-09 | `documents/getSnapshot` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-09 | `editor/getActive` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-09 | `editor/getSelections` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-10 | `editor/applyEdits` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-10 | `editor/replaceSelections` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-10 | `editor/insertText` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-12 | `documents/registerContentProvider` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-15 | `editor/open` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-15 | `editor/reveal` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-16 | `preview/openHTML` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-16 | `browser/openExternal` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-17 | `window/showInformation` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-17 | `window/showWarning` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-17 | `window/showError` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-21 | `credentials/get` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-21 | `credentials/set` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-21 | `credentials/delete` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-22、API-25 | `languageServer/setStatus` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-23 | `run/start` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-23 | `run/stop` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-23 | `run/didExit`（通知） | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-24 | `terminal/create` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-24 | `terminal/show` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-24 | `terminal/close` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-24 | `terminal/reopen` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-24 | `terminal/dispose` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-24 | `terminal/onExit` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-25 | `editor/registerProviders` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-25 | `languageServer/request` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-25 | `languageServer/notify` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-25 | `languageServer/didChangeState`（通知） | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-25 | `languageServer/didReceiveNotification`（通知） | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-27 | `editor/registerActions` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-30 | `debug/start` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-35 | `functions/providers` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-35 | `functions/invoke` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| — | `keymapImport/listInputs` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| — | `keymapImport/readChunk` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-23 | `run/start`：`temporaryFiles` 增加 `{name, base64}` 字节文件 | 0 | 2026-11-13 | [#23](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/23) |

## 时间线（新的在上）

### 2026-11-17

| PR | 修改人 | API 编号 | 协议 / 能力 | 改了什么 |
| --- | --- | --- | --- | --- |
| [#77](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/77) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-29 | `buildSystems` 0 内的说明修正 | Core 不再内置运行发现探测器（[1lck/Lithe-IDEA#1856](https://github.com/1lck/Lithe-IDEA/issues/1856) 第 4 项）：原来按 `Cargo.toml`、`go.mod`、`package.json`、`pyproject.toml`、`Makefile`、`justfile`、`Procfile`、`compose.yaml` 自己生成的 Run 配置、指纹输入和工具链要求全部删除。Run 列表里自动生成的条目只来自已启用构建插件声明的 `buildSystems[].run`，模型何时过期只由插件的 `sync.watch` 决定。插件想让项目出现在 Run 列表里，就要声明 `run`。契约 README 只改了一句措辞（不再提“探测器生成的 Run 配置”）；清单字段、校验、fixture 和能力版本都没有变，没有新增宿主方法或通知。 |

### 2026-11-16

| PR | 修改人 | API 编号 | 协议 / 能力 | 改了什么 |
| --- | --- | --- | --- | --- |
| [#76](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/76) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-29 | `buildSystems` 0 内的补充 | Build 工具窗口的树改由 Core 统一排布，所有构建工具一样：项目按模型的 `parentID` 嵌套，每个项目依次列出按 `category`（没有时按 `group`）分好文件夹的任务、`dependencies.scopes` 含 `project` 时的依赖节点、带目标级任务（按 `targetTags` 匹配）的目标节点，然后是子项目；有 `project` 作用域的声明任务出现在每个项目里，只有 `workspace` 作用域或 `requiresUserArguments` 的任务、工作区依赖和测试发现只在根项目旁边出现一次；模型只有一个根项目时，它的内容直接作为构建系统的内容，任务仍在根项目上运行，所以只针对子项目的参数要用 `nestedProjectSelected` 保护。过滤时保留匹配的整个节点和通往它的项目与文件夹。新增宿主命令 `build.modelTree`（Core API 文档“Build model tree”），清单和模型格式不变，没有新增宿主方法、通知或能力版本。 |
| [#72](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/72) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-29、API-31 | `buildSystems` 0、`vcsProviders` 0 内的补充 | 构建系统新增可选字段 `layout.hiddenDirectoryNames`（[1lck/Lithe-IDEA#1856](https://github.com/1lck/Lithe-IDEA/issues/1856)）：插件启用期间所有工作区在任意层级隐藏的目录名，最多 32 个。每个名字是一段路径，忽略 ASCII 大小写比较，不是 glob；是 `.` 或 `..`、有首尾空白、含 `/`、`\`、`*`、`?`、`[`、`]` 或控制字符、以 `!` 开头时，以 `plugin.path.invalid`（字段 `buildSystems[<n>].layout.hiddenDirectoryNames[<i>]`）拒绝。VCS 提供者的目录标记（如 `.svn`）是 VCS 元数据，插件启用期间同样隐藏。Core 不再内置 `.svn`、`.build`、`node_modules`、`target`、`build`、`.gradle`、`.venv` 等工具目录，默认只隐藏自己的 `.git`、`.worktree`、`.worktrees`；宿主用命令 `workspace.configureVisibility` 把已启用插件的清单交给 Core，项目树、搜索、文件事件、本地历史、运行发现、构建探测和 VCS、Git 仓库发现都读这份名单。Search Everywhere 在 `includeGeneratedFileNames` 时仍列出构建系统声明目录里的文件名，不列 VCS 元数据。旧宿主不认识这个字段，会拒绝带它的清单，所以插件要等包含本改动的桌面端发布后再声明。`workspace.configureVisibility` 是宿主调用 Core 的命令，不是插件方法；没有新增宿主方法、通知或能力版本。 |
| [#69](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/69) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-23 | `buildSystems` 0 内的修正 | Core 不再把 `go`、`node` 当作宿主管理的运行时（[1lck/Lithe-IDEA#1856](https://github.com/1lck/Lithe-IDEA/issues/1856) 第 3 项）：`runtimeDependencies` 里没有 `executables` 的运行时一律按同 ID 的工具定义解析，`go`、`node` 也不例外。`build.invocationPlan` 对它们返回 `{ "kind": "toolchain", "toolchainID": <id>, "root": <root> }` 和 `version: null`，不再返回 `{ "kind": "runtime" }` 候选和 Core 自带的版本参数（`["version"]`、`["--version"]`）；`build.checkToolVersion` 对它们以 `plugin.build.unavailable` 拒绝，版本由工具定义检查。运行时计划删掉 `hostManaged` 字段。`go`、`node` 写了自己的 `executables` 时按普通运行时校验，不再报“由宿主发现”。已发布的清单不用改：Go 插件本来就提供 `go` 工具定义。没有新增方法、字段或能力版本。 |
| [#68](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/68) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-25 | `languageServers` 0 内的补充 | 语言服务器新增可选字段 `indexingExclusions`（[1lck/Lithe-IDEA#1856](https://github.com/1lck/Lithe-IDEA/issues/1856)）：声明工作区配置里列出排除目录的数组 `section`、一项的写法 `entry`（恰好一个 `${path}`）、根目录写法 `rootEntry?`、配置没有该数组时的 `default?`（最多 32 项）和 `pathSyntax?`（`literal` 默认或 `glob`，`glob` 时名字含通配符的目录不写入）。LSP 引擎只按这份声明把排除目录追加进服务器读到的配置，不再按 `rust`、`go` 识别 rust-analyzer 和 gopls；没有声明的服务器配置不变。启动计划的 `sessionPolicy` 带上这份声明，宿主原样交给 `lsp.startServer`。违反时以 `plugin.manifest.invalid`（字段 `languageServers[<n>].indexingExclusions.<field>`）拒绝。旧宿主不认识这个字段，会拒绝带它的清单，所以 Go 插件要等包含本改动的桌面端发布后再声明。没有新增宿主方法、通知或能力版本。 |
| [#61](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/61) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | — | `languageServers` 0 内的补充 | LSP 引擎在一次会话里保留语言服务器的悬浮提示答案（[1lck/Lithe-IDEA#1995](https://github.com/1lck/Lithe-IDEA/issues/1995)）：同一文档版本的同一位置，或答案 `range` 在同一行内覆盖的位置，直接用保留的答案完成请求，不再发 `textDocument/hover`。每个会话最多 32 个，先丢最久没用的；空答案、错误、文本超过 64 KiB 的答案和清空之后才到达的答案照常交付但不保留。文档打开、修改、保存、关闭，工作区文件变化，设置或索引排除项变化，运行命令，后端的 `languageServer/request`、`languageServer/notify` 和宿主通知，服务器发来的任何请求和 `$/progress` 结束，以及会话停止时全部清空。`requestCompleted` 的结果形状不变，没有新增方法、字段或能力版本。 |

### 2026-11-15

| PR | 修改人 | API 编号 | 协议 / 能力 | 改了什么 |
| --- | --- | --- | --- | --- |
| [#58](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/58) | [@1lck](https://github.com/1lck) | API-19 | `sourceSettings` 0 内修正 | 修正来源编辑器绑定的 serde 名称，使 `buildSystemID`、`languageID` 与公开 schema、动作计划和桌面桥接一致；往返回归覆盖读取与规范序列化。明确来源字段 ID 只允许小写 ASCII 字母、数字和连字符。未新增宿主方法。 |
| [#57](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/57) | [@1lck](https://github.com/1lck) | API-19、API-29 | **`sourceSettings` 0**；`buildSystems` 0 内补充 | 原生配置来源编辑：同插件独立读取/暂存函数、编译器页、上游语言/构建上下文、实际值与移除覆盖后的继承值、严格限额、显式保存及 SHA-256 收据。Core 新增 sourceState/sourceEdit/sourceReceipt 命令，原文不跨协议。构建运行时计划返回明确候选与 pathVariable，由原生宿主先验证运行时再启动构建工具；未新增宿主 RPC 方法或通知。 |
| [#56](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/56) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-30 | `debugAdapters` 0 内的补充 | `targetBindings.cwd` 除了 `target.workingDirectory`，也可以绑到 `target.directory`（源文件所在目录）。调试引擎在 launch 参数缺 `cwd` 时会填工作区根目录；Go 插件绑定 `cwd` ← `target.directory` 后，被调程序在包目录运行，按包目录写的相对路径能读到。其他值以 `plugin.manifest.invalid`（字段 `targetBindings.cwd`）拒绝；目标路径是裸文件名、取不出目录时以 `debug.target.invalid`（字段 `target.path`）拒绝。只是可选取值，不需要新能力版本。 |
| [#55](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/55) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-30 | `debugAdapters` 0 内的补充 | `targetBindings` 新增 `adapterArguments`：最多 8 个只有这个适配器才有的 DAP 参数，`{"<参数路径>": "<事实>"}`，路径和 `connection` 的目标一样用点分隔、最多 4 段，事实只能是 `target.path`、`target.directory` 或 `target.workingDirectory`，和其他绑定一样只在没有值时填入。`env.<变量名>` 往环境里加一个变量，`arguments` 和 `env` 绑定给的变量都保留。例如 Delve 在 `dlvCwd` 里编译程序，不在宿主给它的工作目录（工作区根目录）里编译，`go build` 又从 `PWD` 读当前目录；Go 插件绑定 `dlvCwd` 和 `env.PWD` ← `target.directory` 后，工作区根目录没有 `go.mod`、一个工作区放了多个模块、路径经过符号链接时都能调试。路径不能从 `program`、`processId`、`cwd`、`args`、`env` 本身、`noDebug`、`__` 开头的名字开始，路径之间、路径和 `connection` 的目标之间不能重叠，也不能落在 `arguments` 已有的值上或它的下面，违反时以 `plugin.manifest.invalid`（字段 `targetBindings.adapterArguments[.<参数路径>]`）拒绝；目标缺少事实时以 `debug.target.invalid`（字段 `target.path` 或 `target.workingDirectory`）拒绝。只是可选字段，不需要新能力版本。 |
| [#53](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/53) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-23 | **`commandInstallers` 0** | 新能力 `commandInstallers` 和权限 `commands.install`：清单顶层 `commandInstallers[]` 声明怎样用插件已依赖的工具（`requiresTools`）安装 `systemCommand` 启动缺的裸命令，例如用 `go install` 装 `dlv`。每个安装声明 `commands`、`tool`、`minimumVersion?`、字面量 `arguments`、`mayUseNetwork`、`timeoutSeconds?`（1–3600，默认 600）和至少一个目标：`managed` 装进宿主为插件管理的目录，参数或环境变量必须用唯一的模板变量 `${installDirectory}`，`commandDirectory` 指命令所在的子目录；`toolDefault` 装到工具默认位置，`directory` 最多 4 个探针，按顺序运行同一工具，从输出读目录（`pathList`、`append`）。新增宿主命令 `plugins.commandInstallers.requirements`、`plan`、`defaultDirectory` 和错误码 `plugin.commandInstaller.unknown`、`targetUndeclared`、`directoryInvalid`、`probeOutputsInvalid`（Core），以及宿主报告的 `plugin.command.missing`、`plugin.commandInstaller.failed`、`timedOut`、`commandsMissing`。安装声明进入安装审查（`kind: "commandInstaller"`）。宿主只在用户确认命令行、网络和目标后安装，每次安装用 `diagnostics.recordPluginOperation` 记一条 `commandInstaller/install`（命令执行生命周期，安装后命令仍缺记 `decode`）。 |
| [#52](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/52) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-23 | `toolDefinitions` 0 内的补充 | 工具定义的版本探针 `version.arguments` 第一个参数可以是子命令（如 Go 的 `go version` 写 `["version"]`），之后的参数仍然只能是选项，第一个之后出现裸词以 `plugin.manifest.invalid`（字段 `version.arguments`）拒绝；清单 schema 改用 `prefixItems` 并补上 `uniqueItems`。不需要新能力版本。 |
| [#47](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/47) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-30 | `debugAdapters` 0 内的补充 | `targetBindings.program` 新增来源 `target.directory`：`target.path` 所在的目录，供 Delve 这类按包构建再调试的适配器使用；路径没有所在目录时以 `debug.target.invalid`（字段 `target.path`）拒绝。新增宿主命令 `debug.adapters.readAnnouncement`，宿主把 `tcpAnnounced` 适配器的输出逐行交给它，第一行含 `prefix` 的输出是公告，前缀之后是 `port`（连 `127.0.0.1`）或带回环主机的 `host:port`，返回 `none`、`announced` 或 `invalid`（`malformed`、`notLoopback`）；契约写明宿主怎样读公告、10 秒内没有公告就启动失败。清单格式只多了一个枚举值，不需要新能力版本。 |

### 2026-11-14

| PR | 修改人 | API 编号 | 协议 / 能力 | 改了什么 |
| --- | --- | --- | --- | --- |
| [#39](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/39) | [@1lck](https://github.com/1lck) | — | `pluginAPIVersion` 0 内的补充 | 同一语言只由一个已启用的插件提供语言服务器：启用插件、或确认安装后插件处于启用状态时，其他已启用且最新版本为同一语言声明了语言服务器的插件会被停用，宿主告诉用户停用了哪个插件。宿主命令 `plugins.installed.lifecycle` 的 `installed[]` 新增可选 `languageServers`，新增步骤 `{ kind: "displacePlugin", pluginID, languageIDs }`；规则之前已同时启用的插件按 ID 只保留第一个，等用户启用其中一个时再定下来。清单格式不变，不需要新能力版本。 |
| [#36](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/36) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-37 | `pluginAPIVersion` 0 内的补充 | 插件包可以带 `l10n/<语言>.json`（新增 [`plugin-l10n.schema.json`](../contract/v1/plugin-l10n.schema.json)），按设置 ID、选项值、分组原文和按钮 ID 翻译设置的标题、说明、占位符、单位、选项标题、分组标题和按钮文字；ID、值、分组和顺序不变，没翻译的文字显示清单原文。宿主命令 `plugins.declarative.settingsState` 新增可选 `language`、`localizations`，返回 `localization`（候选文件、选中的文件、被拒绝的文件）；候选按语言标签逐级截短（`zh-Hans-CN` → `zh-Hans` → `zh`），选第一个通过检查的文件，有问题的文件整份不用，以新错误码 `plugin.settings.invalidLocalization` 报告。不需要新能力版本，不读翻译的宿主照常显示原文。 |
| [#35](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/35) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | — | `pluginAPIVersion` 0 内的补充 | `build.detect`（`buildSystems[].detection`）还会跳过工作区的忽略路径：`.gitignore` 语法、相对工作区根、只对目录生效，存在团队构建选项 `.lithe/build/options.json` 或本机构建选项的 `ignoredPaths` 中，本机的整份替换团队的；都没有时默认 `[".*"]`，即所有名字以 `.` 开头的目录（如 `.claude/worktrees`）。这些规则先于 Git 规则判断，`!` 可以放回 Git 忽略的目录，共享扫描策略排除的缓存和构建输出仍然跳过。新增宿主命令 `build.editIgnoredPaths` 和错误码 `plugin.build.invalidIgnoredPath`（字段 `ignoredPaths[<序号>]`）；`build.resolveStoredOptions` 返回 `ignoredPaths: { patterns, source }`。自动运行项发现、指纹扫描和 Node 清单发现不受影响。 |
| [#33](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/33) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | — | `pluginAPIVersion` 0 内的补充 | `build.detect`（`buildSystems[].detection`）和自动运行项发现不再把被 Git 忽略的目录当作候选根目录。规则来自工作区内途经的 `.gitignore`，以及根目录在工作区内的仓库的 `info/exclude`（链接工作树读主仓库共享的那份），嵌套仓库另起一套规则；不读工作区根以上的目录、`core.excludesFile` 和索引，所以被忽略目录下已跟踪的清单文件也不再被发现。契约 README 同时写明检测最多向下 6 层。指纹扫描和 Node 清单发现仍然遍历被忽略的目录。 |
| [#28](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/28) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | — | `pluginAPIVersion` 0 内的说明修正 | 清单 schema 删除构建系统 `model`、`test.results`、`dependencies` 里不可达的 `source: "hostIntegration"` 分支，以及语言服务器 `hostIntegration` 下的 `capabilities`、`resources`。`hostIntegration` 字段仍是保留字段：构建系统和语言服务器写了任何值，都以 `plugin.integration.unsupported` 拒绝，字段路径不变；`model`、`test.results`、`dependencies` 写 `source: "hostIntegration"` 时按未知来源以 `plugin.manifest.invalid` 拒绝（原先同时写了 `hostIntegration` 名字时报 `plugin.integration.unsupported`）。README 与指南同步删掉这些来源和“社区插件”说法。能力表移到 `rust/crates/plugin-model/src/capabilities.rs`，插件函数协议移到 `rust/crates/plugin-rpc/src/`，跨插件调用规则移到 `rust/crates/plugin-function-relay/`，内容不变。 |

### 2026-11-13

| PR | 修改人 | API 编号 | 协议 / 能力 | 改了什么 |
| --- | --- | --- | --- | --- |
| [#23](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/23) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-23 | `pluginAPIVersion` 0 内的补充 | `run/start` 的 `temporaryFiles` 可写 `{name, base64}` 字节文件，宿主原样写入、不转码，供编码必须由插件决定的格式使用（例如按系统代码页读取的参数文件）；`hostIntegration` 明确为保留字段：Core 不实现任何宿主集成，任何 ID 都以 `plugin.integration.unsupported` 拒绝，清单 schema 去掉只含 `jdt`、`maven` 的枚举；项目生成器的 `${field.<id>.major}` 取版本的第一个数字段，不再把 `1.8.0_412` 特判成 `8`；契约去掉 Java、Maven、JDT 专属说明。 |

### 2026-11-11

| PR | 修改人 | API 编号 | 协议 / 能力 | 改了什么 |
| --- | --- | --- | --- | --- |
| [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | — | **单一基线 `0`**：`pluginAPIVersion`、清单 `schemaVersion`、18 个能力、进程协议和文档格式 | 单一基线：插件 API、清单格式、全部能力、进程协议和文档格式从 0 开始；契约、指南和 SDK 并入 Core 仓库 `shared/plugins/`。 |

## 维护

修改 `shared/plugins/contract/v1/`、`rust/crates/plugin-model/src/capabilities.rs` 或 `rust/crates/plugin-rpc/src/` 的 Core PR，在**同一个 PR** 里更新本页：

1. 把这个 PR 补到时间线最上面（按合入日期分组）：PR、修改人、API 编号、协议 / 能力、改了什么。新增的版本号要加粗，并写进“按协议查首次出现”里对应的表：例如给 `buildProviders` 加版本 `1`，就在能力表里为它加一行，并在进程协议表里同步 `litheBuild`。一个 PR 只给一个能力加一个版本。
2. “API 编号”填这个 PR 涉及的全部 API-01–44 编号，取自 PR 描述里的“原计划编号”。PR 没写时按 [API 实施计划](https://github.com/1lck/Lithe-IDEA/blob/preview/docs/development/macos-plugin-api-plan.md) 判断，不要自编新号；不属于任何编号的写 —。
3. PR 新增或修改了宿主方法或通知时，每个方法在“宿主方法（按 API 编号）”表里加一行，写明 API 编号、协议、日期和 PR；同时在 [API 总表](api-catalog.md#已公开的宿主方法)的中英两张方法表里各加一行，写明用途、权限和协议。同一个 API 下的方法分几个 PR 加入时，各自登记自己的 PR。
4. “修改人”填 PR 作者的 GitHub 账号，写成 `[@账号](https://github.com/账号)`。账号可以这样查：

   ```sh
   gh pr view <PR 编号> --repo Lithe-IDEA/Lithe-core-rust --json author --jq .author.login
   ```

5. 同时更新[能力与协议版本对照](capabilities-and-versions.md)和受影响的功能页。
