# 插件协议更新记录

本页记录插件协议和 API 的每次修改：哪天、哪个 PR、谁改的、改了什么。只有中文。

- **收录范围**：Lithe-core-rust 仓库 `main` 分支上的 PR，只要它修改了下面三处中的任意一处：插件契约 `shared/plugins/contract/v1/`、Core 能力表 [`capabilities.rs`](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/crates/plugin-model/src/capabilities.rs)、插件函数协议 [`plugin-rpc/src/`](https://github.com/Lithe-IDEA/Lithe-core-rust/tree/main/rust/crates/plugin-rpc/src)。三处都没改的 PR 不收录。
- **日期**：PR 合入 `main` 的日期。
- **修改人**：PR 的作者（GitHub 账号）。
- **每一版的内容和宿主现状**：见[能力与协议版本对照](capabilities-and-versions.md)。字段和行为以 [`contract/v1/README.md`](../contract/v1/README.md) 为准。

时间线的两列这样读：

- **API 编号**：这个 PR 涉及的 API-01–44 规划编号，来自 [API 实施计划](https://github.com/1lck/Lithe-IDEA/blob/main/docs/development/macos-plugin-api-plan.md)，和协议版本、能力版本分开记录。— 表示没有改到具体 API，例如清单、市场、包信任、SDK 或契约文档整理。
- **协议 / 能力**：**加粗**的是这个 PR 新增的版本号；没有加粗的，是同一版本内的补充、宿主接入或说明修正。

## 待合入

- 待补 PR 号（合入日期待合并时填写），[@xiaoyumuxi](https://github.com/xiaoyumuxi)，**`runConfigurationTypes` 1**（API-29）：`buildSystems[].run[]` 新增可选字段 `configurationType`（引用同一构建系统的 `runConfigurationTypes[].id`）和 `values`（按字段 ID 填写初始值，字符串与 `arguments` 列表项支持 `${root}`、`${project.*}`、`${target.*}` 模板展开），必须配合 `fromTargets` 使用，与 `source: "provider"`、`debug`、`entrypoints`、`fromTasks`、`arguments`、`extension`、`execution` 互斥；使用它们的清单需声明 `requires.runConfigurationTypes: 1`。`build.decodeModel` 展开模板并在 `model.run[]` 中输出 `provider: "build.run-type"` 与 `runType: {pluginID, buildSystemID, type, values}`；`runConfig.generate` 的 `providerRuns` 快照支持物化为确定性 ID（`generated:run-type:<hash>`）的 `build.run-type` 生成层配置，与手动创建的同类型配置共用分组、图标、编辑器表单和 `build.runConfigurationPlan` 启动计划；`runConfig.editorForm` 和 `runConfig.saveConfiguration` 支持直接读取并覆盖生成层的 `build.run-type` 配置，且在层合并时整体替换 `extensions.build` 以免被清空的可选字段回渗。没有新增宿主方法、通知或 SDK 版本。

- [#194](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/194)（待合入），[@xiaoyumuxi](https://github.com/xiaoyumuxi)，`pluginAPIVersion` 0 / `litheFunctions` 0 内修正（API-03、API-16）：`plugins.workflow` 在原生副作用前校验状态项引用的命令属于调用会话清单、Markdown/HTML 资源扩展名及浏览器资源的 `unsaved` 默认值；预览省略 `presentation` 时仍为 `editorAndPreview`，浏览器省略 `unsaved` 时仍为 `requireSaved`。补齐内核命令总表遗漏的 `plugins.workflow`，修正 Rust SDK 测试模块位置，并在四种官方 SDK 验证进度取消、结束与订阅释放。没有新增方法、通知、权限、错误码或协议版本。

- [#193](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/193)（合入日期待合并时填写），[@xiaoyumuxi](https://github.com/xiaoyumuxi)，`pluginAPIVersion` 0 / `litheFunctions` 0 内补充（API-03/12/13/16/17/18/22/38）：新增 19 个独立授权方法，补齐输入、内联只读预览、文本对比、视图刷新、输出、可取消进度、状态项、剪贴板、文件管理器和有界 HTTP；已有 HTML 预览新增内联来源，浏览器资源形式补齐已保存/拒绝未保存语义。树新增 `hasChildren`、`cursor` 和 `expand`、`loadMore`、`refresh` 事件；后端保留完整已加载范围，分页无进展停止，失败显式重试。共享 `plugins.workflow` 负责严格参数、默认值、限额、搜索、排序、分页和输出增量，四种官方 SDK 同步包装与事件。没有新协议版本；宿主可用方法仍由 `getCapabilities` 发现。

- [#191](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/191)，[@xiaoyumuxi](https://github.com/xiaoyumuxi)（—，贡献规则与版本说明）：新增插件 API PR 提交指南，明确 API-01–44 是可扩展领域、特有需求不得提升任何系统 API 版本，公共变化也须先证明兼容扩展不足；保留已有 1、2 等版本并按实际用法选最低值。要求统一日志具备可定位语义并集中脱敏、错误码全契约查重、Rust/Go/Node/Java 四个 SDK 在同一 PR 补齐，example 可后补。Core AI 入口及中英导航、契约和版本页同步纠正机械升版与全部填 0 的旧表述。没有新增运行时字段、方法、通知、错误码或版本；合入日期在合并时补齐。
- [#190](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/190)，[@xiaoyumuxi](https://github.com/xiaoyumuxi)（API-07/08，`pluginAPIVersion` 0 内补充）：Core 新增 `workspace.watchPlan`、`workspace.watchEvents`、`workspace.findFiles`、`workspace.findText`，宿主方法保持原名和权限。路由前严格校验监听与搜索参数，复用文件匹配、编码和隐藏/索引规则，以 no-follow 句柄处理实时磁盘；搜索分页有查询绑定游标、200 条总上限、取消和 30 秒预算，监听投影有排序去重、隐藏过滤和溢出重扫。桌面使用 Viva 原生事件，订阅受插件/会话/上下文约束，注册后先发重扫，释放前停止通知；无空闲轮询。macOS 原生事件与资源释放已验证，Windows 原生验证本轮未执行。没有新增协议版本、宿主方法或通知；合入日期在合并时补齐。

- [#190](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/190)，[@xiaoyumuxi](https://github.com/xiaoyumuxi)（API-06，`pluginAPIVersion` 0 内补充）：Core 新增 `workspace.readDirectory`、`workspace.stat`、`workspace.readFile` 三个执行命令；宿主方法保持原名和权限。路由前严格校验上下文、资源、未知字段与分页/读取上限。补齐 no-follow、Windows reparse point、稳定有界分页、磁盘 UTF-8/base64 和截断前缀 identity 的公开行为及共享 fixture；Node SDK 同步明确前缀 identity 类型与可空修改时间；其他 SDK 保留通用 JSON 返回值。没有新增协议版本、方法或通知。合入日期在合并时补齐。

- [#188](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/188)，[@mirakyux](https://github.com/mirakyux)，`pluginAPIVersion` 0 内补充（API-25）：`editor/registerProviders` 新增 `colorDecorations`，注册普通后端函数和最多 64 个有稳定 ID 的明暗 `#RRGGBB` 颜色；结果为 UTF-16 单行范围与颜色 ID。Core 拒绝单源重叠、空范围、未知颜色 ID 和超限结果，并用 `plugins.editorProviders.decode` 的 `kind: "mergeColorDecorations"` 按 `<pluginID>/<registrationID>` 字典序合并，较早来源优先且裁切后续范围。没有新增宿主方法、通知、能力版本或 SDK 包装。
- [#182](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/182)（关联 [1lck/Lithe-IDEA#2260](https://github.com/1lck/Lithe-IDEA/issues/2260)），`lithe-support[bot]`，**`toolchainProviders` 1 / `litheToolchains` 1**（API-23）：权限和 authority 错误改用 `-32082`、`-32083`，稳定字符串码不变。Core 按重新校验的清单版本解码；版本 0 保留 `-32070`、`-32071`，不同版本的号码互不接受。Rust SDK 保留版本 0 默认语义，通过 `Backend::toolchain_provider_version(ToolchainProviderVersion::V1)` 显式选择新版本；没有匹配的协商版本就不启用提供者。四个 SDK 的 `plugin.function.*` 号码不变。
- [#184](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/184)，[@xiaoyumuxi](https://github.com/xiaoyumuxi)，`pluginAPIVersion` 0 内的补充（API-11）：`workspace/create` 的 `kind` 新增 `symlink`，配合新字段 `linkTarget`（1–4096 字节，不含 NUL）新建符号链接。链接原样保存目标，可以是绝对路径，也可以相对链接所在目录，可以指向根目录外或不存在的路径；新建链接不放宽任何检查，`workspace/readFile`、列表和 `editor/open` 仍做根目录检查，也不能穿过离开根目录的父目录新建任何东西；Windows 上目标里的 `/` 存成 `\`，目标是已存在的目录就建目录链接，否则建文件链接，系统拒绝建链接时返回 `failed`。API-11 七个方法的参数改为在路由前严格检查：`resource`、`target` 只能是 `{rootID,path}`，路径只有一种写法（`/` 分隔，没有空段、`.`、`..`，最长 4096 字节，Windows 上不能原样保存的名字也拒绝），移动的目标在同一根目录且不在源之内，`create` 的字段必须和 `kind` 一致，否则返回 `invalidParams`。fixture `backend/function-protocol.json` 新增 3 个用例，四个官方 SDK 的 `create` 注释写明 `symlink`，Node 类型按 `kind` 分成三种参数形状。Rust Core 新增内核命令 `workspace.createEntry`、`renameEntry`、`copyEntry`、`resolveEntry` 和错误码 `entry_not_found`、`entry_exists`，路径解析、不覆盖的重命名、符号链接和复制都在 Core 里完成，宿主只负责文档、视图和系统废纸篓。桌面端接入见 [1lck/Lithe-IDEA#2358](https://github.com/1lck/Lithe-IDEA/pull/2358)。没有新增能力版本、宿主方法、通知或 SDK 版本。
- [#183](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/183)，[@xiaoyumuxi](https://github.com/xiaoyumuxi)，`pluginAPIVersion` 0 内的补充（API-11）：新增宿主方法 `documents/save`、`documents/saveAs`、`documents/close`（新权限 `documents.save`）和 `workspace/create`、`workspace/rename`、`workspace/copy`、`workspace/trash`（新权限 `workspace.files.write`）。每个权限只授予自己的方法，原有权限（包括聚合的 `workspace` 授权和 `documents.read`）都不授予它们。宿主经文档所有者保存，保留版本、撤销记录和语言会话；`close` 先保存再关闭，从不丢弃修改；`saveAs`、`create`、`rename`、`copy` 的目标已存在时失败，从不覆盖；`trash` 只移到系统废纸篓，没有永久删除；`rename`、`trash` 先保存其下打开的文档，保存失败就不动。这些方法不弹询问：需要用户选择的情形（未命名文档、硬链接文件、文件在磁盘上已改变）直接拒绝。新增失败码 `plugin.host.alreadyExists`（`-32080`，目标已存在）和 `plugin.host.diskChanged`（`-32081`，文件在磁盘上被改动或删除，没有写入）。清单 schema 的权限枚举加上两个新权限，fixture `backend/function-protocol.json` 新增 4 个宿主请求用例和 2 个失败码用例，`rust/fixtures/plugins/host-api-v1.json` 新增 4 个授权用例。四个官方 SDK 新增 7 个方法的封装和两个失败码。桌面端实现见 [1lck/Lithe-IDEA#2358](https://github.com/1lck/Lithe-IDEA/pull/2358)。没有新增能力版本、通知或 SDK 版本。
- [#176](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/176)，[@xiaoyumuxi](https://github.com/xiaoyumuxi)，`languageServers` 0 内的说明补充（—）：写明后端决定启动方式的语言服务器在 `immediate` 设置变化时的行为。启动应答自带 `workspaceConfiguration` 时，宿主用这次启动的四个目录、工作区 `context` 和新的 `settings` 再调用一次启动函数，同样交给 `plugins.languageServers.decodeLaunch` 解码，只把计划里的 `workspaceConfiguration` 通过 `lsp.updateWorkspaceConfiguration` 发出（整体替换原配置），应答的其余部分等下次启动；应答不带时发送声明的展开，不再调用函数。所以这类启动函数每次启动可能被调用多次，安装或下载只应在缺少时进行。契约 README 的 Workspace configuration 和 Backend-resolved launch 两节、`reference/domains/languages.md` 与 `settings.md` 的中英文随之补充。桌面端实现见 1lck/Lithe-IDEA#2319。Core 代码、能力版本、宿主方法、通知和协议字段都不变。
- [#175](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/175)，[@xiaoyumuxi](https://github.com/xiaoyumuxi)，`ui` 0 内的补充（API-13、API-23）：撤回 #174 新增的 `ui` 1，设置区域 `tool` 行的可选布尔字段 `choose` 改为 `ui` 0 内的补充，清单写 `requires.ui: 0` 即可使用。`capabilities.rs` 和清单 schema 的 `requires.ui` 回到只有 0，能力对照表删去 `ui` 1 一行。`choose` 不是布尔值时 Core 以 `plugin.ui.invalidView` 拒绝整个结果，位置指向 `choose`；旧宿主的 Core 不认识这个字段，同样会拒绝带它的视图，所以插件要等包含本改动的桌面端发布后再写。fixture `ui/view-results.json` 改为两个用例（接受选择器、`choose` 不是布尔值时拒绝），替换原来“`requires.ui` 为 0 时拒绝”的用例。`ui` 1 没有随任何宿主发布过。没有新增能力版本、宿主方法、通知或进程协议字段。
- [#174](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/174)，[@xiaoyumuxi](https://github.com/xiaoyumuxi)，**`ui` 1**（API-13、API-23）：设置区域视图的 `tool` 行新增可选的布尔字段 `choose`，带上后这一行同时是该工具的选择器，像解释器下拉框一样列出自动选择、已发现和已添加的安装（带版本和来源）、添加目录和打开工具链页的入口；选择只改设置窗口的草稿，“应用”提交、“取消”丢弃，行上显示待提交的选择，应用后启动和运行拿到选中的安装。清单 `requires.ui` 为 0 时 Core 以 `plugin.ui.invalidView` 拒绝整个结果，位置指向 `choose`。契约 README 的插件视图一节补充：为某个工作区显示的视图（项目窗口里的工具窗口，或从项目窗口打开的设置页）调用时，带上该窗口的工作区上下文作为 invoke 的 `params.context`，后端可以在 `click` 里用它调用 `run/start` 等需要上下文的宿主方法；没有工作区时不带。fixture `ui/view-results.json` 新增一个用例，Node SDK 的视图节点类型加上 `choose`，Rust 和 Go SDK 的工作区上下文说明随之修正。没有新增宿主方法、通知或进程协议字段。（`ui` 1 已由 #175 撤回，`choose` 改为 `ui` 0 内的补充。）
- [#173](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/173)，[@xiaoyumuxi](https://github.com/xiaoyumuxi)，市场索引 `schemaVersion` 0 内的补充（—）：`plugins.marketplace.resolveSource` 的结果新增 `revision: { repository: { owner, name }, ref }`（托管在 GitHub 且不带查询参数的索引才有，否则为 null），输入新增可选的 `commit`，带上后结果多出 `pinned: { commit, indexURL, recommendationsURL }`，即同一批文件在该提交上的地址；`commit` 不是完整的 SHA-1 或 SHA-256 时返回新的拒绝码 `plugin.source.commitInvalid`。`indexURL` 仍是来源身份。宿主借此绕过 raw.githubusercontent.com 对分支地址约 5 分钟的缓存，查询或固定下载失败时退回原地址。fixture `marketplace/source-urls.json` 从 11 个用例增加到 17 个。索引格式、插件清单、宿主方法和能力版本都不变。
- [#171](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/171)，[@xiaoyumuxi](https://github.com/xiaoyumuxi)，`pluginAPIVersion` 0 内的说明修正（Go SDK）：Go SDK 需要的最低 Go 版本从 1.22 提高到 1.27.4，和桌面端构建用的 Go 一致。`shared/plugins/sdk/go/go.mod` 改为 `go 1.27.4`，契约 README 的 Official backend SDK 中 `sdk/go` 一段、插件总览、原生构建指南和四个 SDK 的 README 随之更新。插件升级到这个版本的 SDK 后，自己的 `go.mod` 也要写 `go 1.27.4` 或更高。SDK 代码和对外接口不变，没有新增版本号、宿主方法、通知或协议字段。
- [#151](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/151)，[@xiaoyumuxi](https://github.com/xiaoyumuxi)，`pluginAPIVersion` 0 内的补充（API-05、API-35、API-40）：远程工作区上下文的 `authority` 新增 `platform: { os, arch }`，取值同托管运行时（`os` 为 `linux`，`arch` 为 `arm64` 或 `x86_64`），本地上下文没有这个字段。新增宿主方法 `workspace/getHostPaths`：返回远程上下文的根目录在远端主机上的绝对 POSIX 路径 `hostPath`，可选的 `rootIDs`（1–256 个不重复的根目录 ID）决定返回哪些根目录和顺序；它是 `workspace.metadata` 下单独的方法授权，本地上下文返回 `plugin.host.authorityUnsupported`，未知根目录返回 `plugin.host.notFound`；声明 `workspace.metadata` 的插件，启动计划的 `backend.hostServices` 因此多出 `lithe/host/workspace/getHostPaths`，宿主传给 `lsp.startServer` 时去掉自己没有注册处理器的方法授权，后端调用它们得到 `plugin.host.permissionDenied`。新增失败码 `plugin.host.remoteDisconnected`（`-32078`，操作进行中与远端主机断开）和 `plugin.host.platformUnavailable`（`-32079`，插件没有适合远端平台的程序包或托管运行时）。插件函数中转不再拒绝远程工作区的上下文，导出的 `context: "workspace"` 指两种来源的工作区，Core 不读也不改写参数和结果里的路径。四个官方 SDK 新增远端平台读取、`getHostPaths`、按来源选择进程所用路径的辅助方法和两个失败码，`path` 与 `hostPath` 是不同类型。没有新增能力版本、通知或 SDK 版本。
- [#150](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/150)，[@Mucheen](https://github.com/Mucheen)，`runConfigurationTypes` 0 内的行为修正（API-29）。`build.runConfigurationPlan` 在工作目录为工作区根且没有匹配实例时，用唯一生效的文件字段解析后的工作区内目标定位最深的构建实例。`./` 等价于工作区根，指向工作区外的链接不参与定位。进程工作目录和执行前任务选择规则保持原样。没有新增宿主方法、通知或协议版本。

## 按协议查首次出现

所有版本都从 `0` 开始。以后一个 PR 给某个能力加版本 `1` 时，在对应的表里为它加一行。

### 插件 API 版本与文档格式

| 名称 | 版本 | 日期 | PR |
| --- | --- | --- | --- |
| 插件 API 版本 `pluginAPIVersion` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| 插件清单 `schemaVersion` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| 市场索引 `schemaVersion` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| 原生 UI 文档 `schemaVersion`（已删除：`ui` 0 内改为插件视图，视图结果不带版本号，见 [#104](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/104)） | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `litheBuildModel`、`litheDependencies`、`litheTestResults`、`litheTestDiscovery`、`litheBuildConfiguration`、`litheConfigurationEdit` 和 `modelInputs` 快照的 `version` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `plugins.editorActions.decode` 请求的 `version` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| 插件网页桥 `window.lithe.version` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |

| `litheSettingsSources`、`litheSettingsSourceEdit` | 0 | 2026-11-15 | [#57](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/57) |
| 市场推荐索引 `schemaVersion` | 0 | 2026-11-20 | [#130](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/130) |

### 能力版本（`requires`）

各项能力都从版本 `0` 开始。

| 能力 | 版本 | 日期 | PR |
| --- | --- | --- | --- |
| `backend` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `buildModelInputs` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `buildProviders` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `buildProviders` | 1 | 2026-11-18 | [#89](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/89) |
| `buildSystems` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `buildSystems` | 1 | 2026-11-19 | [#112](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/112) |
| `buildSystems` | 2 | 2026-11-20 | [#139](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/139) |
| `commandInstallers` | 0 | 2026-11-15 | [#53](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/53) |
| `commandInstallers` | 1 | 2026-11-20 | [#133](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/133) |
| `debugAdapters` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `executionProviders` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `keymapImportProviders` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `languageServers` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `languageServers` | 1 | 2026-11-20 | [#141](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/141) |
| `languages` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `outputLinkMatchers` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `packageTrust` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `pluginFunctions` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `projectGenerators` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `beforeLaunchTasks` | 0 | 2026-11-18 | [#97](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/97) |
| `managedRuntimes` | 0 | 2026-11-18 | [#96](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/96) |
| `managedRuntimes` | 1 | 2026-11-19 | [#108](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/108) |
| `runConfigurationTypes` | 0 | 2026-11-18 | [#94](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/94) |
| `runConfigurationTypes` | 1 | 待合入 | 待补 PR 号 |
| `sourceSettings` | 0 | 2026-11-15 | [#57](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/57) |
| `sourceSettings` | 1 | 2026-11-17 | [#79](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/79) |
| `settings` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `toolDefinitions` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `toolDefinitions` | 1 | 2026-11-18 | [#99](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/99) |
| `toolchainProviders` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `toolchainProviders` | 1 | 待合入 | [#182](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/182)（[#2260](https://github.com/1lck/Lithe-IDEA/issues/2260)） |
| `ui` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `vcsProviders` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |

### 进程协议版本

进程协议没有独立的版本，它的版本就是拥有它的能力的版本（`litheLogging` 是 `pluginAPIVersion` 的版本）。

| 协议 | 版本 | 日期 | PR |
| --- | --- | --- | --- |
| `litheFunctions`（`backend`） | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `litheBuild`（`buildProviders`） | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `litheBuild`（`buildProviders`） | 1 | 2026-11-18 | [#89](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/89) |
| `litheVcs`（`vcsProviders`） | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `litheToolchains`（`toolchainProviders`） | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `litheToolchains`（`toolchainProviders`） | 1 | 待合入 | [#182](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/182)（[#2260](https://github.com/1lck/Lithe-IDEA/issues/2260)） |
| `litheKeymapImport`（`keymapImportProviders`） | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `litheLogging`（`pluginAPIVersion`） | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |

### 宿主方法（按 API 编号）

向后兼容地新增宿主方法、通知或可选字段时不升版本（见契约 [Extending the host services](../contract/v1/README.md#extending-the-host-services)），上面的版本表看不出某个方法是什么时候加的，所以宿主方法和通知在这里逐个登记。一个 API 编号下可以有多个方法，每个方法一行，记首次加入的日期和 PR；后续的执行或行为补充另加一行，保留首次记录。方法名省略前缀 `lithe/host/`；用途和权限见 [API 总表](api-catalog.md#已公开的宿主方法)。`keymapImport/*` 属于 `litheKeymapImport` 协议，不在 API-01–44 内。`functions/providers`、`functions/invoke` 是 `litheFunctions` 里的跨插件调用，只对声明了至少一个 `calls` 的后端开放。

| API | 方法 | 协议 | 日期 | PR |
| --- | --- | --- | --- | --- |
| API-01、API-37 | `getCapabilities` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-01 | `subscriptions/dispose` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-02 | `commands/didRun`（通知） | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-03 | `statusBar/setItem` | 0 | 2026-11-23 | [#193](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/193) |
| API-03 | `statusBar/setItem` | 0（原生执行前校验与默认值） | 待合入 | [#194](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/194) |
| API-03 | `statusBar/removeItem` | 0 | 2026-11-23 | [#193](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/193) |
| API-04、API-05 | `workspace/didChangeContexts`（通知） | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-05 | `workspace/listContexts` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-05 | `workspace/getContext` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-05 | `workspace/getFolders` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-06 | `workspace/readDirectory` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-06 | `workspace/readDirectory` | 0（执行/校验补充） | 待合入 | [#190](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/190) |
| API-06 | `workspace/stat` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-06 | `workspace/stat` | 0（执行/校验补充） | 待合入 | [#190](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/190) |
| API-06 | `workspace/readFile` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-06 | `workspace/readFile` | 0（执行/校验补充） | 待合入 | [#190](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/190) |
| API-07 | `workspace/watch` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-07 | `workspace/watch` | 0（执行/校验补充） | 待合入 | [#190](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/190) |
| API-07、API-24 | `subscriptions/notify`（通知） | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-08 | `workspace/findFiles` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-08 | `workspace/findFiles` | 0（执行/校验补充） | 待合入 | [#190](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/190) |
| API-08 | `workspace/findText` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-08 | `workspace/findText` | 0（执行/校验补充） | 待合入 | [#190](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/190) |
| API-09 | `documents/list` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-09 | `documents/getSnapshot` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-09 | `editor/getActive` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-09 | `editor/getSelections` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-10 | `editor/applyEdits` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-10 | `editor/replaceSelections` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-10 | `editor/insertText` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-11 | `documents/save` | 0 | 待合入 | [#183](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/183) |
| API-11 | `documents/saveAs` | 0 | 待合入 | [#183](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/183) |
| API-11 | `documents/close` | 0 | 待合入 | [#183](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/183) |
| API-11 | `workspace/create` | 0 | 待合入 | [#183](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/183) |
| API-11 | `workspace/rename` | 0 | 待合入 | [#183](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/183) |
| API-11 | `workspace/copy` | 0 | 待合入 | [#183](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/183) |
| API-11 | `workspace/trash` | 0 | 待合入 | [#183](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/183) |
| API-12 | `editor/showDiff` | 0 | 2026-11-23 | [#193](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/193) |
| API-12 | `documents/registerContentProvider` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-13 | `views/refresh` | 0 | 2026-11-23 | [#193](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/193) |
| API-15 | `editor/open` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-15 | `editor/reveal` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-16 | `preview/openMarkdown` | 0 | 2026-11-23 | [#193](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/193) |
| API-16 | `preview/openMarkdown` | 0（原生执行前校验与默认值） | 待合入 | [#194](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/194) |
| API-16 | `preview/openHTML`（内联来源） | 0 | 2026-11-23 | [#193](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/193) |
| API-16 | `preview/openHTML` | 0（原生执行前校验与默认值） | 待合入 | [#194](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/194) |
| API-16 | `browser/openExternal`（资源打开） | 0 | 2026-11-23 | [#193](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/193) |
| API-16 | `browser/openExternal` | 0（原生执行前校验与默认值） | 待合入 | [#194](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/194) |
| API-16 | `preview/openHTML` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-16 | `browser/openExternal` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-17 | `window/showQuickPick` | 0 | 2026-11-23 | [#193](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/193) |
| API-17 | `window/showInputBox` | 0 | 2026-11-23 | [#193](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/193) |
| API-17 | `window/showInformation` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-17 | `window/showWarning` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-17 | `window/showError` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-18 | `clipboard/readText` | 0 | 2026-11-23 | [#193](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/193) |
| API-18 | `clipboard/writeText` | 0 | 2026-11-23 | [#193](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/193) |
| API-18 | `workspace/reveal` | 0 | 2026-11-23 | [#193](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/193) |
| API-21 | `credentials/get` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-21 | `credentials/set` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-21 | `credentials/delete` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-22 | `output/create` | 0 | 2026-11-23 | [#193](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/193) |
| API-22 | `output/append` | 0 | 2026-11-23 | [#193](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/193) |
| API-22 | `output/clear` | 0 | 2026-11-23 | [#193](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/193) |
| API-22 | `output/show` | 0 | 2026-11-23 | [#193](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/193) |
| API-22 | `output/dispose` | 0 | 2026-11-23 | [#193](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/193) |
| API-22 | `progress/create` | 0 | 2026-11-23 | [#193](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/193) |
| API-22 | `progress/report` | 0 | 2026-11-23 | [#193](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/193) |
| API-22 | `progress/end` | 0 | 2026-11-23 | [#193](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/193) |
| API-22、API-25 | `languageServer/setStatus` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-23 | `run/start` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-23 | `run/stop` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-23 | `run/didExit`（通知） | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-23 | `run/start`：`temporaryFiles` 增加 `{name, base64}` 字节文件 | 0 | 2026-11-13 | [#23](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/23) |
| API-23 | `run/start`：`process.outputEncoding` 可选声明 | 0 | 2026-11-20 | [#128](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/128) |
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
| API-29 | `build/getOptions` | 0 | 2026-11-21 | [#155](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/155) |
| API-29 | `build/watchConfiguration` | 0 | 2026-11-21 | [#155](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/155) |
| API-29 | `lithe/build/prepareRun`：返回值 `outputEncoding` 可选声明（提供者方法） | `litheBuild` 0、1 | 2026-11-20 | [#128](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/128) |
| API-30 | `debug/start` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-35 | `functions/providers` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-35 | `functions/invoke` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| API-38 | `network/request` | 0 | 2026-11-23 | [#193](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/193) |
| API-40 | `workspace/getHostPaths` | 0 | 待合入 | [#151](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/151) |
| — | `keymapImport/listInputs` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| — | `keymapImport/readChunk` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |

## 时间线（新的在上）

### 2026-11-23

| PR | 修改人 | API 编号 | 协议 / 能力 | 改动 |
| --- | --- | --- | --- | --- |
| [#193](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/193) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-03、API-12、API-13、API-16、API-17、API-18、API-22、API-38 | `pluginAPIVersion` 0 / `litheFunctions` 0 | 新增 19 个独立授权方法，补齐输入、内联只读预览、文本对比、视图刷新、输出、可取消进度、状态项、剪贴板、文件管理器和有界 HTTP；已有 HTML 预览新增内联来源，浏览器资源形式补齐已保存/拒绝未保存语义。树新增 `hasChildren`、`cursor` 和 `expand`、`loadMore`、`refresh` 事件；后端保留完整已加载范围，分页无进展停止，失败显式重试。共享 `plugins.workflow` 负责严格参数、默认值、限额、搜索、排序、分页和输出增量，四种官方 SDK 同步包装与事件。没有新协议版本；宿主可用方法仍由 `getCapabilities` 发现。 |

### 2026-11-21

| PR | 修改人 | API 编号 | 协议 / 能力 | 改动 |
| --- | --- | --- | --- | --- |
| [#155](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/155) | [@1lck](https://github.com/1lck) | API-29 | `pluginAPIVersion` 0 | 增加 `build/getOptions` 和 `build/watchConfiguration`。构建后端可读取本插件的生效选项，订阅工作区构建选项及声明的配置文件变更。增加独立的方法权限和 Java SDK 调用入口。 |

### 2026-11-20

| PR | 修改人 | API 编号 | 协议 / 能力 | 改了什么 |
| --- | --- | --- | --- | --- |
| [#141](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/141) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-25、API-29、API-31 | **`languageServers` 1** | 语言服务器新增可选 `executesProjectCode`（不写按 `true`），启动请求新增 `projectCodeAllowed`（不写按未信任，[1lck/Lithe-IDEA#2208](https://github.com/1lck/Lithe-IDEA/issues/2208)）。未信任的工作区里，`plugins.declarative.languageServerLaunchPlan` 和 `plugins.languageServers.decodeLaunch` 只给声明 `executesProjectCode: false` 的语言服务器计划；自带进程的构建提供者按 `buildSystems[].provider.executesProjectCode` 判断，共用语言服务器的跟随该服务器；VCS 提供者一律不给计划。拒绝码是新的 `plugin.launch.untrustedWorkspace`。插件后端和独立调试适配器不受影响。用 `executesProjectCode` 要声明 `requires.languageServers: 1`。没有新增宿主方法、通知或 SDK 版本。 |
| [#139](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/139) | [@1lck](https://github.com/1lck) | API-29 | **`buildSystems` 2** | 构建提供者的 `application` 运行声明（含 `entrypoints: true`）可以通过 `debug.configurationType` 复用已注册的连接调试器。`lithe/build/prepareRun` 复用原有 `launchOptions.debug`，入口运行仍不带 `target`。已有服务运行声明继续适用旧版本。没有新增 RPC 方法、通知或 SDK 版本。 |
| [#135](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/135) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-23 | `toolDefinitions` 0 内的行为修正 | 外部安装的 `executableLayouts` 文件链接到另一个文件名时，`toolchains.candidates` 的待探针路径和 `toolchains.resolve` 的 `executable` 改为规范主目录下的布局路径，不再是链接目标。rustup 把 `cargo`、`rust-analyzer` 链接到自身、按启动时的名字决定运行哪个工具，以前 Core 返回 `rustup`，版本探针失败。链接目标的类型、可执行位和越界检查不变；同名链接仍返回规范路径。没有新增字段、版本号、宿主方法、通知或 SDK 版本。 |
| [#133](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/133) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-23 | **`commandInstallers` 1** | 命令安装新增[下载安装](../contract/v1/README.md#download-installers)：安装声明 `download`（与 `tool`、`minimumVersion`、`arguments` 互斥），固定上游 `version` 和 1–6 个平台归档，每个归档有 `os`、`arch`、HTTPS `url`、可选 `mirrors`（最多 8 个）、完整 `sha256`、`archive`（`zip`/`tar.gz`）和可选 `stripPrefix`，规则与 `managedRuntimes` 归档相同；只能用 `managed` 目标，不带 `arguments`、`environment`，`commandDirectory` 相对解压根目录，`mayUseNetwork` 必须为 `true`。`plugins.commandInstallers.requirements` 和 `plan` 新增可选 `platform: {os, arch}`：带上时没有该平台归档的下载安装不提供；安装条目新增 `download: null | {version}`；计划新增 `download`（工具安装为 `null`，下载安装为选中平台的归档、`cacheKey` 和 `commandDirectory`）。新增 Core 错误码 `plugin.commandInstaller.platformInvalid`、`unavailable`，宿主错误码 `plugin.commandInstaller.downloadFailed`、`checksumMismatch`、`archiveInvalid`；`failed` 只指工具安装失败。下载安装的审查程序列出全部归档，改地址、镜像、摘要或版本需要重新批准；只声明 `commandInstallers` 0 的宿主拒绝安装带下载安装的插件。没有新增宿主方法、通知或 SDK 版本。用于 Rust Support 插件按需下载 CodeLLDB。 |
| [#130](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/130) | [@1lck](https://github.com/1lck) | — | **市场推荐索引 `schemaVersion` 0** | 市场契约新增同级推荐索引格式 `plugin-recommendations.schema.json`（`schemaVersion: 0`，独立于只增不破旧版的 `lithe-plugin-index.json`）；`plugins.marketplace.resolveSource` 额外返回 `recommendationsURL`，并新增宿主内部 Core 命令 `plugins.marketplace.recommendForWorkspace`（不是插件 RPC），基于工作区采集文件名与已有市场索引匹配可安装的缺失插件。见 [1lck/Lithe-IDEA#2228](https://github.com/1lck/Lithe-IDEA/issues/2228)。 |
| [#128](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/128) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-23、API-29 | `pluginAPIVersion` 0 内的补充 | `run/start` 的 `process` 和提供者 `lithe/build/prepareRun` 的返回值新增可选 `outputEncoding`，用稳定编码 ID 或别名声明进程写入管道的编码；Core 校验并规范化，拒绝未知或不兼容 ASCII 的编码。用户固定编码优先；自动模式逐行尝试声明、UTF-8，以及仅 Windows 的 GBK。省略字段时所有平台默认 UTF-8，Windows 兼容 GBK；不修改进程环境或终端代码页。新增宿主内部 Core 命令 `outputEncoding.resolve`，返回同一解析规则的候选与可选编码列表，不是插件 RPC。见 [1lck/Lithe-IDEA#2186](https://github.com/1lck/Lithe-IDEA/issues/2186)。 |
| [#127](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/127) | [@Mucheen](https://github.com/Mucheen) | API-25 | `languages` 0 内的补充 | 从 `grammar.path` 指定的 TextMate 语法或独立的 `grammars/<language-id>.first-line.json` 读取可选 `firstLineMatch`。独立文件可沿用宿主现有高亮。旧桌面仍可安装 `languages: 0` 的同一插件包。Core 解码返回可选提示并匹配完整首行，既有路径与用户关联优先。见 [1lck/Lithe-IDEA#2122](https://github.com/1lck/Lithe-IDEA/issues/2122)。 |

### 2026-11-19

| PR | 修改人 | API 编号 | 协议 / 能力 | 改了什么 |
| --- | --- | --- | --- | --- |
| [#112](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/112) | [@Mucheen](https://github.com/Mucheen) | API-29 | **`buildSystems` 1** | 清单新增 `buildSystems[].test.discoveryCommand`，已有测试运行可以独立采用命令或 provider。命令使用现有构建工具，把 `litheTestDiscovery` v0 写到宿主提供的 `${outputFile}`。Core 的 `build.testDiscoveryPlan`、`build.decodeTestDiscovery` 和测试树兼容两种发现来源，检查每条选择器可原样运行，命令尝试沿用 `lithe/build/discoverTests` 的结构化终态。没有新增宿主方法、通知或 SDK 版本。见 [1lck/Lithe-IDEA#2122](https://github.com/1lck/Lithe-IDEA/issues/2122)。 |
| [#122](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/122) | [@1lck](https://github.com/1lck) | API-29、API-37 | `buildSystems` 0 内的补充 | 包内翻译文件新增 `buildOptions.<buildSystemID>.<optionID>`，可翻译构建选项的 `displayName`、`description` 和固定选项值的 `title`，规则与 `runConfigurationTypes` 相同，不合规时整份拒绝，错误码 `plugin.build.invalidLocalization`。新 Core 命令 `build.localizeOptions` 接收已启用构建插件的清单和界面语言，采用两阶段读取翻译文件（与 `runConfig.configurationTypes` 相同），返回替换了选项文字的清单，宿主用它检测，选项对话框、工具栏开关和构建树都显示译文。插件清单、宿主方法、通知和能力版本都不变；旧 Core 会忽略翻译文件中的 `buildOptions`。见 [1lck/Lithe-IDEA#2204](https://github.com/1lck/Lithe-IDEA/issues/2204)。 |
| [#121](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/121) | [@1lck](https://github.com/1lck) | API-29 | `buildSystems` 0 内的行为修正 | `build.resolveOptions` 对没有声明 `default` 的 `text`、`file`、`directory` 选项，用户覆盖时也返回 `implicitValue`，作为撤销覆盖后回到的值，宿主据此保留“覆盖”复选框。没有新增字段、版本号、宿主方法或通知。见 [1lck/Lithe-IDEA#2152](https://github.com/1lck/Lithe-IDEA/issues/2152)。 |
| [#120](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/120) | [@1lck](https://github.com/1lck) | API-29 | `buildSystems` 0 内的补充 | 清单的 `options[]` 新增 `description`（1–512 字符），作为宿主 ⓘ 中的说明，只用于显示。`litheBuildModel` 和 `model.map` 的 `optionDefaults` 除 `choicesFromModel` 选项外，也可以填 `text`、`file`、`directory` 选项，值是选项未设置时工具实际使用的那一个（如 Maven 的用户设置文件和本地仓库），Core 只保留第一个值。`build.resolveOptions` 对未设置的这类选项返回 `implicitValue`，并返回 `description`。`implicitValue` 只用于显示，不生成参数，也不进入选项指纹。旧 Core 会拒绝声明了 `description` 或给路径选项填 `optionDefaults` 的清单。没有新增版本号、宿主方法、通知或 SDK 版本。见 [1lck/Lithe-IDEA#2152](https://github.com/1lck/Lithe-IDEA/issues/2152)、[#2204](https://github.com/1lck/Lithe-IDEA/issues/2204)。 |
| [#118](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/118) | [@1lck](https://github.com/1lck) | — | `buildSystems` 0 内的行为补充 | 契约新增 [Workspace trust](../contract/v1/README.md#workspace-trust) 一节，取代“Lithe 没有工作区信任状态”的说法。构建计划请求新增 `projectCodeAllowed`；工作区未受信任时，Core 以新错误码 `plugin.build.untrustedWorkspace` 拒绝所有任务、测试、运行、调试准备，以及声明了 `executesProjectCode: true` 的模型、依赖命令和构建提供者请求。声明 `false` 的模型和依赖命令在安全模式下照常加载。插件清单和后端协议都不变；宿主必须在计划请求里传入信任，否则会运行项目代码的计划都会被拒绝。没有新增版本号、宿主方法、通知或 SDK 版本。见 [1lck/Lithe-IDEA#2201](https://github.com/1lck/Lithe-IDEA/issues/2201)。 |
| [#111](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/111) | [@Mucheen](https://github.com/Mucheen) | API-23 | `toolchainProviders` 0 内的修正 | 无项目时，`toolchains.resolve` 可使用同一代次的全局提供者回包。解析目录为 `.`，查询的 `workspace` 和 `context` 显式为 `null`，解析上下文也为空。工作区查询继续校验根目录和相对目录，两种查询都核对文件系统快照及排除根。 |
| [#113](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/113) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | — | `pluginAPIVersion` 0 内的说明补充（官方 SDK 新增 Go） | Go 插件后端 SDK 放在 `shared/plugins/sdk/go`，模块路径 `github.com/Lithe-IDEA/Lithe-core-rust/shared/plugins/sdk/go`，需要 Go 1.22 或更高版本，只用标准库，按模块标签 `shared/plugins/sdk/go/v<版本>` 发布（还没有推送标签）。它实现函数协议和宿主服务（凭据、工作区、文档与编辑器、预览、浏览器、窗口消息、`functions/providers` 与 `functions/invoke`、原生终端、订阅）、插件视图 `View(id, handler)` 和 `litheKeymapImport` 0，不实现构建和 VCS 提供者。契约 README 的 Official backend SDK 加上 `sdk/go` 一段：在 Unix 上把协议移到 stdout 的副本、stdout 指向 stderr，处理函数 panic 只结束这一次调用。Go 示例后端和 Rust 示例后端跑同一个插件后端场景，`sdk-e2e.mjs` 新增 `plugin_sdk_go`。插件总览、日志指南和各参考页列出 Go SDK，并推荐新插件优先用 Rust 或 Go SDK（构建后端照旧用 Node.js 或 Java SDK，VCS 后端用 Node.js SDK）。没有新增版本号、宿主方法、通知或协议字段。 |
| [#109](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/109) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-13 | `ui` 0 内的补充（API-13） | 视图事件新增 `width` 和 `resize`，画布可以跟随窗口宽度重新排版。会报告宽度的宿主在每个视图事件里带 `width`（排版视图节点用的宽度，单位为点，至少为 1 的整数，也是顶层画布的宽度），并且量到宽度之后才发 `open`；画布的 `on` 除 `viewport` 外可以写 `resize`，视图里有这样的画布时，宽度停止变化 400 ms 后宿主发 `resize` 和新的 `width`，等待中的 `resize` 可以合并成最新的一个。插件只有在收到带 `width` 的事件后才写 `resize`：不报告宽度的宿主不发 `resize`，它的 Core 也会拒绝写了 `resize` 的画布。契约 README 同时写明画布在缩放 1 时怎样放 `viewBox`：不写 `pan` 时显示整个 `viewBox`，按视图宽度缩放但不超过 `height`，从区域左上角开始；写了 `pan` 时宽度占满视图、上边对齐区域顶部，缩小到比区域窄时水平居中。以前的说明只有“按视图宽度缩放、`height` 点高”，在更宽的窗口里会裁掉场景下半部分。Core 的画布校验接受 `on: ["viewport", "resize"]`，`plugin-ui.schema.json` 的 canvas 说明和 `on` 同步；Rust SDK 新增 `ViewEventKind::Resize` 和 `ViewEvent::width`，Java SDK 新增 `Kind.RESIZE` 和 `width()`，Node SDK 的 `ViewEventKind` 加上 `resize`、`ViewEvent` 加上 `width`、画布 `on` 的类型放宽，三个 SDK 的 README 补充用法；`domains/settings.md` 中英两段说明区域画布怎样按 `width` 排版，以及旧宿主下的固定宽度（语言页约 610 点、构建系统页和编译器页右侧详情约 340 点）（[1lck/Lithe-IDEA#2181](https://github.com/1lck/Lithe-IDEA/issues/2181)）。没有新增能力版本、宿主方法、通知或 SDK 版本，取代 [#107](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/107)。 |
| [#108](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/108) | [@1lck](https://github.com/1lck) | API-23 | **`managedRuntimes` 1** | 受管运行时的每个平台归档可以写 `mirrors`：最多 8 个备用 HTTPS 地址，提供与 `url` 同一个 SHA-256 固定的归档，不改变 `cacheKey`。Core 要求镜像地址不带凭据和片段、彼此不同且不同于 `url`；写了 `mirrors` 却没有声明 `managedRuntimes: 1` 的清单被拒绝，其他插件继续写 0。清单 schema 的 `requires.managedRuntimes` 加上版本 1、平台归档加上 `mirrors`；[能力与协议版本对照](capabilities-and-versions.md)把 `managedRuntimes` 0 的桌面端一列改为 ✅，新增版本 1 一行。没有新增宿主方法、通知或 SDK 版本。宿主须先声明 `managedRuntimes` 1，插件才可要求它。 |
| [#105](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/105) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-13、API-19 | `ui` 0、`settings` 0 内的说明补充（API-13、API-19） | 契约 README 写明画布 `image` 节点怎样读取：路径留在插件包内、经过真实目录、指向普通 PNG 文件，单个文件最多 256 KiB、每边最多 1024 像素（预算表新增一行）；宿主在场景第一次画到它时于窗口线程外读取，保留到视图关闭，读到之前或读取失败时场景不画它，失败只记一次日志。`image` 节点的字段表补上 `opacity`。文字是一行，基线在 `y`，`align` 把行首（默认）、中点或行尾放在 `x`；渐变 `angle` 与 CSS `linear-gradient` 一致（0 向上、90 向右、默认 180 向下），渐变线穿过形状中心；分组的 `x`、`y` 放置原点，`scale` 缩放内部，`children`、`w`、`h`、`radius` 都在分组自己的坐标里；捏合缩放只在宿主能上报捏合时可用。fixture `fixtures/ui/view-results.json` 的文字基线移到卡片中间。[能力与协议版本对照](capabilities-and-versions.md)删掉“插件视图尚未在桌面端绘制”的脚注，`domains/ui.md`、`domains/settings.md` 的桌面端一行改为已呈现插件视图、设置区域和包内翻译 `l10n/`（[1lck/Lithe-IDEA#2181](https://github.com/1lck/Lithe-IDEA/issues/2181)）。没有新增版本号、宿主方法、通知或 SDK 版本。 |

### 2026-11-18

| PR | 修改人 | API 编号 | 协议 / 能力 | 改了什么 |
| --- | --- | --- | --- | --- |
| [#104](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/104) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-13、API-19、API-37 | `ui` 0、`settings` 0 内的替换 | 插件视图取代宿主绘制的原生 UI 文档。插件后端的视图函数（经已有的 `lithe/function/invoke` 调用）按事件（`open`、`close`、`click`、`change`、`select`、`activate`、`viewport`、`settings`）返回界面树或补丁：宿主控件（行、分区、文字、输入框、下拉、分段、开关、按钮、键值、表格、横幅、绑定设置的设置行、显示插件工具解析结果的工具行）、树和画布节点（可带圆角的矩形、路径、文字、图片、可点击分组及悬停/按下/选中状态），新 schema [`plugin-ui.schema.json`](../contract/v1/plugin-ui.schema.json) 取代 `native-ui.schema.json`，原生 UI 文档的 `schemaVersion` 一并删除。Core 新增命令 `plugins.ui.applyResult`：按清单、界面（工具窗口或设置区域）和上一版视图校验每个结果，返回新视图和是否变化，超出预算或违反界面规则以 `plugin.ui.invalidView`（带 `field`）拒绝。清单新增 `settingRegions`（最多 16 个）：插件在语言、构建系统或编译器页（不写 `page` 时为插件自己的设置标签）放搜索关键词、插件视图或两者，设置搜索按关键词找到页面；`l10n/<语言>.json` 新增 `settingRegions.<id>.keywords`，翻译的关键词追加在声明的后面、忽略大小写去重。fixture `fixtures/ui/view-panel.plugin.json`、`fixtures/ui/view-results.json`，`fixtures/settings/` 补设置区域与中文关键词。Node、Rust、Java 三个 SDK 新增 `view(id, handler)`：把参数读成视图事件（不合法时 `sdk.view.invalidEvent`，处理函数不会运行），把返回的整个视图、补丁或“没有变化”写成结果；Node SDK 的 `index.d.ts` 给出界面节点、画布节点和补丁的类型。没有新增版本号、宿主方法、通知或 SDK 版本；这一版插件不能主动推送视图。 |
| [#102](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/102) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-23、API-19 | `toolDefinitions` 1 内的补充 | `toolchains.settingsView` 的 `issues` 每项改为 `{index,status,group?}`：查询分组时 `group` 是工具所在分组的键，宿主从问题列表打开这个工具时可以一并展开它的分组，不必自己拼分组键；契约 README 同时写明 `issues` 一直不受查询影响、先错误后警告并各按名称排序。没有新增版本号、宿主方法、通知或 SDK 版本。 |
| [#99](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/99) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-23、API-19 | **`toolDefinitions` 1** | 工具定义新增只影响显示位置的 `category`（`runtime`、`build`、`package`、`debug`、`lint`、`other`，默认 `other`）和 `audience`（`user` 默认，`plugin` 表示插件自用的运行时）；用到任一字段的插件声明 `toolDefinitions: 1`，只支持 0 的宿主不会安装，其他插件继续写 0。新增 Core 命令 `toolchains.settingsView`：按宿主已有的解析原因、固定和未应用状态给设置页工具列表分类（`error`、`warning`、`ok`）、筛选、分组、折叠和计数，最多 20,000 个工具。`plugins.settings.navigation` 支持三级、每项可选 `limit`（超出的子项折叠成一行“全部 N 个”）以及每行的 `matched` 与 `path`。清单 schema 的 `requires.buildProviders` 补上 #89 已支持的版本 1。没有新增宿主方法、通知或 SDK 版本。宿主须先声明 `toolDefinitions` 1，插件才可要求它。 |
| [#97](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/97) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-29 | **`beforeLaunchTasks` 0** | 运行配置“启动前”的插件任务（对标 IntelliJ IDEA 的“添加新任务”，[1lck/Lithe-IDEA#2100](https://github.com/1lck/Lithe-IDEA/issues/2100)）：构建系统新增 `beforeLaunchTasks`（最多 16 个），声明方式和 `runConfigurationTypes` 相同（`id`、`displayName`、可选 `description`、`icon`、最多 16 个 `fields` 和 `arguments` 模板），但任务总是运行到结束，不能写 `execution`，`arguments` 不能用 `${userArguments}`；任务 ID 与类型 ID 各自唯一。运行配置的 `beforeLaunch` 步骤除 `{configuration}` 外新增 `{task: {pluginID, buildSystemID, type, values}}`，任务在拥有这个步骤列表的配置的工作目录里运行，不带程序参数和环境变量；步骤对象不允许出现别的键。Core 新增命令 `runConfig.checkBeforeLaunchTask`（校验“添加新任务”小表单的值，返回要放进字段值的步骤）、`runConfig.beforeLaunchField`（给不用 `runConfig.editorForm` 的编辑器，例如构建运行的编辑器，返回已保存配置的 `beforeLaunch` 字段）和 `build.beforeLaunchTaskPlan`（把任务步骤展开成 argv 计划）；`runConfig.editorForm` 的 `beforeLaunch` 字段新增 `tasks`（已安装插件声明的任务种类及其表单），字段值里的任务步骤是 `{task, name, summary, values}`，插件已卸载的是 `{task, name, summary, missing: true}`；`runConfig.beforeLaunch`、`runConfig.updateOptions`、`runConfig.saveEditorChanges` 新增请求字段 `runTypePlugins`，`runConfig.beforeLaunch` 的任务步骤返回 `{task, name, summary, workingDirectory}`，拒绝码新增 `taskNotDeclared` 和 `invalidTaskValues`（带 `task`），确认指纹也覆盖任务及其值和工作目录。`runConfigurationTypes` 的必填 `arguments` 字段留空现在会被拒绝（以前没有执行 `required`）。语言文件新增 `beforeLaunchTasks.<构建系统 ID>.<任务 ID>`；保存的值不合法时是 `plugin.build.invalidBeforeLaunchTask`（字段 `values.<id>`）。新增 fixture `before-launch-tasks/`。用到它的插件声明 `beforeLaunchTasks: 0`，不认识它的宿主不会安装。没有新增宿主方法或通知。 |
| [#96](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/96) | [@lithe-support[bot]](https://github.com/apps/lithe-support) | API-23 | **`managedRuntimes` 0** | 受管运行时：新增固定 HTTPS 归档与 SHA-256、平台选择、与插件版本无关的缓存身份，以及插件后端对外部/暂存依赖的资格检查契约。新增权限 `runtimes.install`、审查程序 `managedRuntime`、Core 命令 `plugins.managedRuntimes.plan` 与 `plugins.managedRuntimes.decodeResolution`；正常语言服务器启动可接收只读 `runtimes` 映射。原生 `lsp.startServer` 新增可选 `functionRelayEnabled`（默认 true），准备会话设为 false 隔离跨插件调用但保留宿主直接调用。无新增宿主 RPC 或 SDK 版本。宿主须先发布对应能力，插件才可要求它。 |
| [#94](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/94) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | API-29 | **`runConfigurationTypes` 0** | 插件声明自己的运行配置类型（[1lck/Lithe-IDEA#2100](https://github.com/1lck/Lithe-IDEA/issues/2100)）：构建系统新增 `runConfigurationTypes`（最多 16 个），每个类型有 `id`、`displayName`、可选的 `description`、`execution`（`application` 或 `task`）、`arguments` 模板和最多 16 个字段 `fields`（`text`、`choice`、`checkbox`、`file`、`directory`、`arguments`，可带 `default`、`required`、`placeholder`、`visibleWhen` 和每个字段自己的 `arguments`）。新模板变量 `${field:<id>}` 是整个参数，每个字段必须在类型参数或字段参数里恰好用一次（只控制显示、不带参数的字段除外）；`${userArguments}` 恰好出现一次时，编辑器才显示程序参数。用户保存的配置类型是 `build.run-type`，`extensions.build` 记 `{pluginID, buildSystemID, type, values}`，仍存在 `.lithe/run/plugin/<插件 ID>.toml`。所有运行配置（含 Core 自带的）新增通用开关 `allowMultipleInstances`：关时再次启动先停掉正在运行的进程，开时宿主在旁边再起一个。Core 新增命令 `build.runConfigurationPlan`（把保存的配置展开成 argv 计划），`runConfig.configurationTypes`、`runConfig.editorForm`、`runConfig.saveConfiguration` 新增请求字段 `runTypePlugins`（清单和语言文件），语言文件新增 `runConfigurationTypes.<构建系统 ID>.<类型 ID>`。字段值不合法时是 `plugin.build.invalidRunConfiguration`（字段 `values.<id>`），语言文件不合法时是 `plugin.build.invalidLocalization`。新增 fixture `run-configuration-types/`。用到它的插件声明 `runConfigurationTypes: 0`，不认识它的宿主不会安装。没有新增宿主方法或通知。 |
| [#93](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/93) | [@Mucheen](https://github.com/Mucheen) | API-23 | `toolDefinitions` 0 内的补充 | 项目内工具环境解释器符号链接链校验与项目候选返回（[1lck/Lithe-IDEA#2122](https://github.com/1lck/Lithe-IDEA/issues/2122)）：`projectCodeAllowed != true` 时所有 `projectFile` 候选仍以 `projectCodeRequiresAction` 跳过且不返回探针目标；`projectKind: "environment"`（如 `.venv`）要求 `home`、到可执行文件之间的父目录（如 `.venv/bin`、`.venv/Scripts`）、环境内 `requires` 标记以及实际存在的 `pathPrepend` 目录规范化后留在工作区和 `home` 内（记为 `missing` 的跨平台 `pathPrepend` 子目录忽略，未观察到或软链逃逸以 `unsafePath` 拒绝），叶子可执行文件允许是指向工作区内或工作区外常规可执行文件的单跳或多跳符号链接，探测与选中结果均保留字面 `executable` 路径；`wrapper` 仍要求链接目标留在工作区根内。`toolchains.discoveryPlan` 的每个工具新增平台过滤后的 `projectFiles`；`toolchains.candidates` 新增根级 `project` 候选列表；`toolchains.resolve` 新增 `projectCandidates`，且没有候选解析成功时保留解析顺序中的首个失败原因。最近的项目声明遮住根目录的声明，可选发现无法表示的路径会被跳过，已验证的固定选择仍可生效。项目环境和外部安装都可以省略明确观测为缺失的 PATH 目录。`toolchains.catalogState` / `editCatalog` 支持在本机 `toolchains/global.json` 维护规范化的 `trustedWorkspaces` 并查询 `projectCodeAllowed`。同步更新过时宿主能力对照说明；清单格式和能力版本不变，没有新增插件 RPC 方法或通知。 |
| [#89](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/89) | [@1lck](https://github.com/1lck) | — | **`buildProviders` 1**、**`litheBuild` 1** | 按源文件找运行入口（[1lck/Lithe-IDEA#2104](https://github.com/1lck/Lithe-IDEA/issues/2104)）：运行声明可写 `entrypoints: true`（只能是提供者的 `application`，不写 `fromTargets`、`fromTasks`、`arguments`、`extension`、`debug`），它不对应任何目标，宿主不按目标列出它。新提供者方法 `lithe/build/runEntrypoints`（参数多一个工作区相对路径 `file`）返回 `litheRunEntrypoints` 0 文档，列出这个文件里可运行的入口 `{run, name, entrypoint, project}`，最多 64 个；Core 新增 `build.runEntrypointsPlan` 和 `build.decodeRunEntrypoints` 校验。`lithe/build/prepareRun` 对入口运行不带 `target`，必须带 `launchOptions.entrypoint`。宿主提供 `litheBuild` 版本 `[0, 1]`，三个 SDK 回答宿主提供的最新版本，Java/Node SDK 新增 `onRunEntrypoints`/`build.runEntrypoints`。用入口运行的插件声明 `buildProviders: 1`，只支持 0 的宿主不会安装；其他插件不受影响。新增 fixture `build-provider-run-entrypoints.plugin.json`、`build-run-entrypoints.json`。没有新增宿主方法或通知。 |
| [#86](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/86) | [@Mucheen](https://github.com/Mucheen) | API-29 | `buildSystems` 0 内的补充 | 构建树与选项编辑支持模型提供的默认激活项、实际激活集合和取消前缀（[1lck/Lithe-IDEA#2092](https://github.com/1lck/Lithe-IDEA/issues/2092)）。规范模型可报告 `optionDefaults`、`optionExcludePrefixes` 和 `optionActiveChoices`。Core 按选项指纹匹配实际激活集合，保留兄弟构建根和继承值，并在生成运行条目时解析取消前缀。清单中的 `excludePrefix` 和 `model.map.optionDefaults` 需要支持这些字段的 Core。旧 Core 可忽略规范模型新增字段。宿主方法、通知和能力版本沿用现有契约。 |
| [#85](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/85) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | — | 插件清单 `schemaVersion` 0 内的补充 | 清单新增可选字段 `repository`（[1lck/Lithe-IDEA#2100](https://github.com/1lck/Lithe-IDEA/issues/2100)）：值是插件源码仓库的 GitHub 地址 `https://github.com/<作者>/<仓库>`，格式不对时以 `plugin.manifest.invalid`（字段 `repository`）拒绝。项目里由插件提供的 Run 配置改存 `.lithe/run/plugin/<插件 ID>.toml`（共享）和 `<插件 ID>.local.toml`（本机），文件头 `[plugin]` 记录插件 ID、名称、作者、仓库、版本、版本要求（默认 `>=保存时的版本`）、`pluginAPIVersion` 和用到的能力，作者和仓库取自 `repository`，用来区分同名插件；插件没有安装、仓库不一致或版本不满足要求时，这些配置在 Run 列表里显示为不可用并说明原因，不会让整个列表读取失败。Core 自带的脚本和程序运行（shell、PowerShell、Windows 批处理、可执行文件）不依赖任何插件。没写 `repository` 的插件照常安装，Run 文件里不记作者和仓库、也不比对仓库；清单格式和能力版本不变，没有新增宿主方法或通知。 |

### 2026-11-17

| PR | 修改人 | API 编号 | 协议 / 能力 | 改了什么 |
| --- | --- | --- | --- | --- |
| [#84](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/84) | [@1lck](https://github.com/1lck) | — | `outputLinkMatchers` 0 内的补充 | 运行和测试控制台里的网址可点击（[1lck/Lithe-IDEA#1822](https://github.com/1lck/Lithe-IDEA/issues/1822)）：`outputLinks.match` 跑完插件匹配器后，再识别行内的 `http`/`https` 网址，返回不带插件的 `{line, start, length, url}`，宿主在浏览器中打开。网址止于空白、引号、尖括号，不含句末标点和未配对的右括号；和插件链接重叠时插件链接优先；一行内的链接按起点排序。fixture `output-link-matchers.json` 新增一例。清单格式和能力版本不变，插件无需改动；没有新增宿主方法或通知。 |
| [#83](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/83) | [@xiaoyumuxi](https://github.com/xiaoyumuxi) | — | `languages` 0 内的补充 | 编辑器的 TextMate 引擎按 vscode-textmate 的方式运行语法的 `injections`（[1lck/Lithe-IDEA#2112](https://github.com/1lck/Lithe-IDEA/issues/2112)）：只有给文档分词的那个语法应用自己的 `injections`，被它包含的语法的不生效；某个位置的 scope 符合选择器时，注入规则和当前规则一起尝试，起点更靠前的匹配胜出，起点相同时当前规则胜出，除非选择器带 `L:` 前缀。Core 随之校验它：`injections` 是以 scope 选择器为键的规则对象，最多 64 项，选择器 1–1024 字节，每条规则和其他规则一样检查并计入 16384 条上限，违反时以 `plugin.grammar.invalid`（字段 `grammar.injections[.<选择器>…]`）拒绝。`injectionSelector` 仍不运行、不检查。清单格式和能力版本不变，不运行 `injections` 的旧宿主照常加载这些语法；没有新增宿主方法或通知。 |
| [#79](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/79) | [@1lck](https://github.com/1lck) | API-19 | **`sourceSettings` 1** | 原生来源设置可以做成“项目结构”页（[1lck/Lithe-IDEA#2086](https://github.com/1lck/Lithe-IDEA/issues/2086)）：分组布局 `sidebar` 把来源列成左侧模块列表，来源用 `parent` 嵌套（同组、在前、最多 8 层）；字段 `section` 在 `sidebar` 中是标签页、在 `form` 中是小标题；新字段类型 `entryList` 是有序的类型化条目列表，`entryKinds` 声明添加菜单（`file` 可带扩展名、`directory`、`choice` 带可选项），`attributes` 声明至多 4 个布尔或枚举列。条目的 `title`、`detail` 只用于显示，比较和写回时忽略；新增的 `choice` 条目必须来自当前可选项或原值。`SourceInput` 新增 `entries`，`effectiveDisplays` 对条目列表给出 `count`。快照 `version` 仍为 0；用到这些成员的插件声明 `sourceSettings: 1`，只支持 0 的宿主不会安装。新增 fixture `settings-sources-modules.json`。没有新增宿主方法或通知。 |
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

1. 把这个 PR 补到时间线最上面（按合入日期分组）：PR、修改人、API 编号、协议 / 能力、改了什么。不升版的兼容扩展同样登记；是否允许引入新版本，先按契约 [Versioning](../contract/v1/README.md#versioning) 和 [API PR 提交指南](../guides/plugin-api-pr-guide.zh-CN.md#3-api-版本怎么处理) 判断，不能照搬历史升版先例。确有新版本时才加粗，并写进“按协议查首次出现”里的对应表；所属能力与进程协议保持一致。
2. “API 编号”填这个 PR 涉及的全部 API-01–44 编号，取自 PR 描述里的“原计划编号”。PR 没写时按 [API 实施计划](https://github.com/1lck/Lithe-IDEA/blob/main/docs/development/macos-plugin-api-plan.md) 判断，不要自编新号；不属于任何编号的写 —。
3. PR 新增或修改了宿主方法或通知时，每个方法在“宿主方法（按 API 编号）”表里加一行，写明 API 编号、协议、日期和 PR；同时在 [API 总表](api-catalog.md#已公开的宿主方法)的中英两张方法表里各加一行，写明用途、权限和协议。同一个 API 下的方法分几个 PR 加入时，各自登记自己的 PR。
4. “修改人”填 PR 作者的 GitHub 账号，写成 `[@账号](https://github.com/账号)`。账号可以这样查：

   ```sh
   gh pr view <PR 编号> --repo Lithe-IDEA/Lithe-core-rust --json author --jq .author.login
   ```

5. 同时更新[能力与协议版本对照](capabilities-and-versions.md)和受影响的功能页。
