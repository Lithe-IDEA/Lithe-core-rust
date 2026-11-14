# 能力与协议版本对照 / Capability and protocol versions

[中文](#中文) · [English](#english)

## 中文

本页把插件会碰到的所有版本号放在一起：`pluginAPIVersion`、清单格式、18 个能力、进程协议和文档格式。**它们现在的版本全部是 `0`。** 规范以 [`contract/v1/README.md`](../contract/v1/README.md) 的 [Versioning](../contract/v1/README.md#versioning)、[Capabilities](../contract/v1/README.md#capabilities)、[Host capabilities](../contract/v1/README.md#host-capabilities) 和 [Publishing](../contract/v1/README.md#publishing) 为准；本页与契约不一致时以契约为准，并请到 [Lithe-core-rust 的 Issues](https://github.com/Lithe-IDEA/Lithe-core-rust/issues) 反馈。

Core 列以本仓库 `main` 上的 Core 源码为准（[`capabilities.rs`](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/crates/plugin-model/src/capabilities.rs) 的 `SUPPORTED_CAPABILITIES`，[`plugin-manifest/src/lib.rs`](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/crates/plugin-manifest/src/lib.rs) 的 `SUPPORTED_PLUGIN_API_VERSIONS`）；Lithe 桌面端列以桌面端的[声明与说明](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go)为准。Core 能校验不等于宿主能运行：桌面端声明了 12 个能力，另外 6 个没有声明，需要它们的插件不会被提供安装。

### 四类版本号

| 版本号 | 写在哪里 | 谁检查 | 不满足时 |
| --- | --- | --- | --- |
| 插件 API 版本 `pluginAPIVersion` | `plugin.json` | Core | `plugin.api.unsupported`，`field` 为 `pluginAPIVersion` |
| 能力版本 `requires.<能力>` | `plugin.json`（必填） | Core 的支持表，再和宿主的 `hostCapabilities` 取交集 | `plugin.api.unsupported`，`field` 为 `requires.<能力>`；市场不提供这个插件版本 |
| 进程协议版本 `experimental.lithe*` | 不写进清单；进程启动时在 LSP `initialize` 里协商 | 宿主给出版本列表，后端从中选一个 | 该协议不启用，例如不向这个进程发构建请求 |
| 文档格式版本 | 各文档自己的 `schemaVersion` 或 `version` 字段 | Core | 解析失败 |

能力版本决定**能不能安装**；协议版本决定**进程启动后能说什么**。进程协议没有独立的版本，它的版本就是拥有它的能力的版本，宿主给出的列表恰好是这一个版本。

以后的变化遵循契约的规则：

- 一次改动只给**一个**能力加一个新版本（`1`），清单段和协议一起变；没用到这次改动的插件继续写 `0`，在原来的宿主上继续可装。
- 已发布的能力版本不改变含义，宿主一直支持它。
- 插件只声明自己用到的能力，每个能力取能覆盖它用法的**最低**版本。没用到新内容就提高版本，只会让旧宿主装不上。
- 把插件改到新的能力版本必须发新的插件版本；市场索引保留上一个旧要求的版本，旧宿主仍有版本可装。
- 支持某个版本的宿主先发布，声明它的插件后发布。

### 插件 API 版本与清单格式

| 版本 | 内容 | Core | Lithe 桌面端 | 详见 |
| --- | --- | --- | --- | --- |
| `pluginAPIVersion` `0` | 基础字段：`schemaVersion`、`id`、`displayName`、`version`、`vendor`、`permissions`、`activationEvents`、`requires`、`requiresTools`；同时是 `litheLogging` 的版本。其他取值一律被拒绝 | ✅ | ✅ | [清单与打包](domains/manifest.md)、[Versioning](../contract/v1/README.md#versioning) |
| `schemaVersion` `0` | `plugin.json` 的文件格式；其他取值是 `plugin.schema.unsupported` | ✅ | ✅ | [`plugin-manifest.schema.json`](../contract/v1/plugin-manifest.schema.json) |

### 能力版本（`requires`）

每一行是一个能力。`requires` 必须**恰好**列出清单用到的能力：漏一个或多列一个都是 `plugin.manifest.invalid`；每个能力的值都是 `0`。启动进程（[Process start](../contract/v1/README.md#process-start)）和可执行文件解析（[Executable resolution](../contract/v1/README.md#executable-resolution)）属于基础契约，不是能力；写 `requires.serverProcess` 会像未知能力一样被拒绝。

| 版本 | 内容 | Core | Lithe 桌面端 | 详见 |
| --- | --- | --- | --- | --- |
| `languages` `0` | `languages[]`：语言 ID、文件扩展名，以及包内 TextMate [`grammar` 与 `configuration`](../contract/v1/README.md#grammars-and-editor-configuration)；宿主从包里读取并在编辑器里应用，不启动进程 | ✅ | ✅ | [语言](domains/languages.md) |
| `packageTrust` `0` | `vendor.bundledSignatureRequirement`：随宿主发布的发布者签名插件副本按来源信任 | ✅ | 未声明 | [包信任](domains/package-trust.md)、[Trust](../contract/v1/README.md#trust) |
| `languageServers` `0` | `languageServers[]`（启动字段按[启动进程](../contract/v1/README.md#process-start)），包括[`workspaceConfiguration`](../contract/v1/README.md#workspace-configuration)和[插件自有服务器的会话字段](../contract/v1/README.md#plugin-owned-sessions) | ✅ | ✅¹ | [语言](domains/languages.md)、[Language servers](../contract/v1/README.md#language-servers) |
| `executionProviders` `0` | `executionProviders[]`：用声明的命令运行和发现测试 | ✅ | 未声明 | [语言](domains/languages.md)、[Execution providers](../contract/v1/README.md#execution-providers) |
| `settings` `0` | `settings[]` 与 `settingActions[]`，含[放置位置、作用域、生效方式、页面按钮](../contract/v1/README.md#placement-scope-effect-and-page-buttons) | ✅ | ✅ | [插件设置](domains/settings.md)、[Plugin settings](../contract/v1/README.md#plugin-settings) |
| `ui` `0` | [`ui`](../contract/v1/README.md#host-owned-workbench-ui)：文本、[包内网页](../contract/v1/README.md#web-content)和[原生控件](../contract/v1/README.md#host-rendered-native-content)的工具窗口、命令，以及[不打开工具窗口的后端命令](../contract/v1/README.md#backend-only-commands) | ✅ | ✅¹ | [界面](domains/ui.md) |
| `buildSystems` `0` | 不带 `provider` 的 [`buildSystems[]`](../contract/v1/README.md#build-systems)：检测、选项、项目模型、任务、运行入口、测试、诊断、依赖、同步和[声明式项目准备](../contract/v1/README.md#declarative-project-preparation) | ✅ | ✅¹ | [声明式构建系统](domains/build-systems.md) |
| `buildProviders` `0` | `buildSystems[].provider`：[构建提供者](../contract/v1/README.md#build-providers)，可以是自有进程或共享语言服务器；说 `litheBuild`，包含任务执行、测试执行与发现、运行准备、原生调试准备、原生配置、内联测试报告、语义测试选择器、类型化运行选项和注册表连接调试 | ✅ | ✅¹ | [构建提供者](domains/build-providers.md) |
| `buildModelInputs` `0` | `buildSystems[].modelSharing` 与 `modelInputs`：[已启用工作区插件之间共享的只读模型输入](../contract/v1/README.md#optional-read-only-model-inputs)（可选） | ✅ | 未声明 | [构建提供者](domains/build-providers.md) |
| `vcsProviders` `0` | [`vcsProviders[]`](../contract/v1/README.md#vcs-providers)；说 `litheVcs` | ✅ | 未声明 | [版本控制提供者](domains/vcs.md) |
| `debugAdapters` `0` | [`debugAdapters[]`](../contract/v1/README.md#debug-adapters)：独立适配器、语言服务器内的适配器、[远程附加](../contract/v1/README.md#remote-attach)、参数和环境变量、[`features` 与 `defaults`](../contract/v1/README.md#adapter-features-and-defaults) | ✅ | ✅ | [调试适配器](domains/debug-adapters.md) |
| `outputLinkMatchers` `0` | [`outputLinkMatchers[]`](../contract/v1/README.md#output-link-matchers)：插件声明的匹配规则让 Run 和测试控制台的输出出现链接；Core 负责匹配，`resolve` 函数在点击时才调用 | ✅ | ✅ | [语言](domains/languages.md) |
| `projectGenerators` `0` | [`projectGenerators[]`](../contract/v1/README.md#project-generators)：新建项目窗口里的条目，含 [`tool` 字段](../contract/v1/README.md#tool-fields) | ✅ | ✅ | [项目生成器](domains/project-generators.md) |
| `backend` `0` | [`backend`](../contract/v1/README.md#plugin-backend)：插件自己的长驻进程，带或不带页面启动，按权限提供宿主服务，说 `litheFunctions` | ✅ | ✅¹ | [插件后端与插件函数](domains/backend.md) |
| `toolDefinitions` `0` | `toolDefinitions[]`：[工具身份、布局、发现和四层候选](../contract/v1/toolchains/README.md) | ✅ | ✅¹ | [工具链](domains/toolchains.md) |
| `toolchainProviders` `0` | `toolchainProviders[]`：[提供者声明和按权限校验应答](../contract/v1/toolchains/README.md)；说 `litheToolchains` | ✅ | 未声明 | [工具链](domains/toolchains.md) |
| `pluginFunctions` `0` | 清单顶层 [`pluginFunctions`](../contract/v1/README.md#cross-plugin-calls)：导出的函数（`exports`）和要调用的其他插件函数（`calls`）；不授予任何清单权限，调用经 Core 中转 | ✅ | ✅¹ | [插件后端与插件函数](domains/backend.md) |
| `keymapImportProviders` `0` | `keymapImportProviders[]`：[独立的快捷键导入提供者](../contract/v1/README.md#shortcut-import-providers-core-foundation)；说 `litheKeymapImport` | ✅ | 未声明 | [快捷键导入](domains/keymap-import.md) |

¹ 桌面端声明了这个能力，但其中有些部分还没有界面：对应的部分没有按钮、菜单或窗口，插件照常安装，也不会因此失败。尚未呈现的部分以桌面端源码里 `Declared()` 上方的注释为准（[`pluginhost.go`](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go)）：

- 语言服务器只呈现诊断；补全、悬停、跳转、重命名和格式化没有编辑器界面；启动应答 `{kind: "tool"}` 会被拒绝。
- 系统命令的工具发现只读 `PATH`；依赖项目工具链或用户覆盖的计划会被拒绝。
- 构建提供者的运行准备、测试、调试准备、模型输入、原生配置、内联测试报告和运行期调试；构建系统的实例选项和任务准备。
- 不属于任何工具窗口的 `ui` 命令。
- 后端宿主服务：工作区文件与搜索（`workspace/readDirectory`、`stat`、`readFile`、`watch`、`findFiles`、`findText`）和 `preview/openHTML` 尚未注册，请求这些权限的插件被拒绝。运行期以 `lithe/host/getCapabilities` 的返回为准，见 [API 总表](api-catalog.md)。

### 进程协议版本

这些协议在进程启动时协商，不写进清单。宿主在 `initialize` 的 `capabilities.experimental.<协议>` 里给出 `{ "versions": [0] }`，后端回答 `{ "version": 0 }`。回答了宿主没给出的版本，或格式不对，该协议整个不启用。每个协议的版本就是拥有它的能力的版本。

| 版本 | 内容 | Core | Lithe 桌面端 | 详见 |
| --- | --- | --- | --- | --- |
| `litheFunctions` `0`（`backend`） | 插件函数：注册、撤回、调用、结果、进度、取消；宿主服务 `lithe/host/*`（凭据、工作区上下文与能力发现、宿主通知、公共文档/文件/窗口方法、插件自有原生终端、独立编辑器动作、编辑器语言功能 Provider、运行与调试会话、语言服务器方法、内容提供者）；跨插件调用 `functions/providers`、`functions/invoke`。提供给插件拥有的会话 | ✅ | ✅¹ | [Plugin functions](../contract/v1/README.md#plugin-functions) |
| `litheBuild` `0`（`buildProviders`） | `lithe/build/*`：模型与依赖、任务执行、测试执行与发现、运行准备、原生调试准备、只读模型输入、原生配置、内联测试报告、语义测试选择器、类型化运行选项、注册表连接调试。提供给构建提供者 | ✅ | ✅¹ | [Build providers](../contract/v1/README.md#build-providers) |
| `litheVcs` `0`（`vcsProviders`） | VCS 提供者的能力和请求（`lithe/vcs/*`）。提供给 VCS 提供者 | ✅ | 未声明 | [VCS providers](../contract/v1/README.md#vcs-providers) |
| `litheToolchains` `0`（`toolchainProviders`） | `toolchains/query`；后端从声明的提供者中选出子集。提供给声明了工具链提供者的插件会话 | ✅ | 未声明 | [Provider backend negotiation](../contract/v1/toolchains/README.md#provider-backend-negotiation) |
| `litheKeymapImport` `0`（`keymapImportProviders`） | 后端从声明的提供者中选出要启用的子集；`lithe/keymap/import`。提供给声明了 `keymapImportProviders` 的插件会话 | ✅ | 未声明 | [Shortcut import](../contract/v1/README.md#shortcut-import-providers-core-foundation) |
| `litheLogging` `0`（`pluginAPIVersion`） | 结构化诊断通知 `lithe/log`；官方 SDK 要求宿主提供它，否则拒绝初始化。提供给每个插件拥有的进程 | ✅ | ✅ | [Structured diagnostics](../contract/v1/README.md#structured-plugin-diagnostics) |

- `litheFunctions` 的宿主服务不靠版本号区分：后端调用 `lithe/host/getCapabilities` 得到宿主此刻注册了哪些方法，并受权限约束。新增宿主方法、通知或可选字段不需要新版本，见 [Extending the host services](../contract/v1/README.md#extending-the-host-services)。
- 构建提供者（`litheBuild`）里宿主不呈现的部分按上面的脚注¹处理：后端可以实现，宿主不会调用。
- 插件私有函数不需要清单字段，增删它们不改变任何版本。

### 文档格式版本

| 版本 | 内容 | Core | Lithe 桌面端 | 详见 |
| --- | --- | --- | --- | --- |
| 原生 UI 文档 `schemaVersion` `0` | 宿主渲染的原生控件文档 | ✅ | ✅（随 `ui`） | [`native-ui.schema.json`](../contract/v1/native-ui.schema.json) |
| 市场索引 `schemaVersion` `0` | 市场来源的索引文件 | ✅ | ✅ | [`plugin-index.schema.json`](../contract/v1/plugin-index.schema.json)、[插件市场](domains/marketplace.md) |
| 构建模型 `litheBuildModel`、依赖树 `litheDependencies`、测试结果 `litheTestResults`、测试发现 `litheTestDiscovery` 的 `version` `0` | 规范构建文档 | ✅ | ✅¹ | [Model and outputs](../contract/v1/README.md#model-and-outputs)、[Test discovery](../contract/v1/README.md#test-discovery) |
| 原生构建配置 `litheBuildConfiguration`、`litheConfigurationEdit` 的 `version` `0` | 基于真实来源的原生配置文档 | ✅ | 尚未呈现¹ | [Optional native configuration](../contract/v1/README.md#optional-native-configuration) |
| `modelInputs` 快照的 `version` `0` | 共享给其他工作区插件的只读模型输入 | ✅ | 未声明（`buildModelInputs`） | [Optional read-only model inputs](../contract/v1/README.md#optional-read-only-model-inputs) |
| `plugins.editorActions.decode` 请求的 `version` `0` | Core 命令的请求版本；必填，Core 只接受 `0` | ✅ | 尚未呈现¹ | [Independent editor actions](../contract/v1/README.md#independent-editor-actions) |
| 插件网页桥 `window.lithe.version` `0` | 包内网页可见的桥版本（`bridgeVersion`） | ✅ | ✅ | [Web content](../contract/v1/README.md#web-content) |
| 启动计划里的 `backend.version` | 取清单的 `requires.backend`，即 `0` | ✅ | ✅ | [Plugin backend](../contract/v1/README.md#plugin-backend) |

导出或调用的插件函数的 `version`（1 到 1000）不是契约版本：它归导出该函数的插件所有。

### 官方 SDK

Node.js SDK 0.5.0、Java SDK 0.5.0 和 Rust SDK 0.4.0 说的是版本 `0` 的协议；各 SDK 封装哪些协议见契约的 [Official backend SDK](../contract/v1/README.md#official-backend-sdk) 和各自的 [Node.js](../sdk/node/README.md)、[Java](../sdk/java/README.md)、[Rust](../sdk/rust/README.md) README。下载地址在 [Lithe-core-rust 的 Releases](https://github.com/Lithe-IDEA/Lithe-core-rust/releases)。

---

## English

This page puts every version number a plugin meets in one place: `pluginAPIVersion`, the manifest format, the 18 capabilities, the process protocols, and the document formats. **Every one of them is `0`.** The normative text is [Versioning](../contract/v1/README.md#versioning), [Capabilities](../contract/v1/README.md#capabilities), [Host capabilities](../contract/v1/README.md#host-capabilities), and [Publishing](../contract/v1/README.md#publishing) in [`contract/v1/README.md`](../contract/v1/README.md). Where this page disagrees with the contract, the contract wins; please report it in the [Lithe-core-rust issues](https://github.com/Lithe-IDEA/Lithe-core-rust/issues).

The Core column follows the Core source on this repository's `main` (`SUPPORTED_CAPABILITIES` in [`capabilities.rs`](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/crates/plugin-model/src/capabilities.rs), `SUPPORTED_PLUGIN_API_VERSIONS` in [`plugin-manifest/src/lib.rs`](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/crates/plugin-manifest/src/lib.rs)); the Lithe desktop column follows the desktop client's [declarations and notes](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go). Validating a capability is not running it: the desktop client declares 12 capabilities and does not declare the other 6, so a plugin that needs one of those is not offered for installation.

### Four kinds of version

| Version | Where it is written | Who checks it | When it is not met |
| --- | --- | --- | --- |
| Plugin API version `pluginAPIVersion` | `plugin.json` | Core | `plugin.api.unsupported` with `field: "pluginAPIVersion"` |
| Capability version `requires.<capability>` | `plugin.json` (required) | Core's support table, intersected with the host's `hostCapabilities` | `plugin.api.unsupported` with `field: "requires.<capability>"`; the marketplace does not offer that plugin version |
| Process protocol version `experimental.lithe*` | Not in the manifest; negotiated in LSP `initialize` when the process starts | The host offers a list, the backend picks one | The protocol is not enabled, for example no build request goes to that process |
| Document format version | The document's own `schemaVersion` or `version` field | Core | Decoding fails |

A capability version decides **whether a plugin installs**; a protocol version decides **what a running process can say**. A process protocol has no version of its own: its version is that of the capability that owns it, and the host offers exactly that one version.

Future changes follow the contract's rules:

- A change adds one new version (`1`) to exactly **one** capability, and the manifest section and the protocol move together. A plugin that does not use the change keeps `0` and stays installable wherever it was.
- A released capability version never changes meaning, and hosts keep supporting it.
- A plugin declares only the capabilities it uses, each at the **lowest** version that covers its use. Raising one without using what it adds only cuts off older hosts.
- Moving a plugin to a new capability version needs a new plugin version; the index keeps the last version with the previous requirements so older hosts still have one to install.
- A host release that supports a version ships before any plugin that declares it.

### Plugin API version and manifest format

| Version | Content | Core | Lithe desktop | See |
| --- | --- | --- | --- | --- |
| `pluginAPIVersion` `0` | The base fields: `schemaVersion`, `id`, `displayName`, `version`, `vendor`, `permissions`, `activationEvents`, `requires`, `requiresTools`; also the version of `litheLogging`. Any other value is refused | ✅ | ✅ | [Manifest and packaging](domains/manifest.md), [Versioning](../contract/v1/README.md#versioning) |
| `schemaVersion` `0` | The file format of `plugin.json`; any other value is `plugin.schema.unsupported` | ✅ | ✅ | [`plugin-manifest.schema.json`](../contract/v1/plugin-manifest.schema.json) |

### Capability versions (`requires`)

Each row is one capability. `requires` lists **exactly** the capabilities a manifest uses: leaving one out or listing an unused one is `plugin.manifest.invalid`; every value is `0`. [Process start](../contract/v1/README.md#process-start) and [Executable resolution](../contract/v1/README.md#executable-resolution) belong to the base contract and are not capabilities; `requires.serverProcess` is refused like any unknown capability.

| Version | Content | Core | Lithe desktop | See |
| --- | --- | --- | --- | --- |
| `languages` `0` | `languages[]`: language IDs, file extensions, and packaged TextMate [`grammar` and `configuration`](../contract/v1/README.md#grammars-and-editor-configuration); the host reads them from the package and applies them in the editor without starting a process | ✅ | ✅ | [Languages](domains/languages.md) |
| `packageTrust` `0` | `vendor.bundledSignatureRequirement`: origin-scoped trust for a host-sealed bundled copy of a publisher-signed plugin | ✅ | Not declared | [Package trust](domains/package-trust.md), [Trust](../contract/v1/README.md#trust) |
| `languageServers` `0` | `languageServers[]` (launch fields as in [Process start](../contract/v1/README.md#process-start)), including [`workspaceConfiguration`](../contract/v1/README.md#workspace-configuration) and the [session fields for a plugin's own server](../contract/v1/README.md#plugin-owned-sessions) | ✅ | ✅¹ | [Languages](domains/languages.md), [Language servers](../contract/v1/README.md#language-servers) |
| `executionProviders` `0` | `executionProviders[]`: run and discover tests through the declared commands | ✅ | Not declared | [Languages](domains/languages.md), [Execution providers](../contract/v1/README.md#execution-providers) |
| `settings` `0` | `settings[]` and `settingActions[]`, with [placement, scope, effect, and page buttons](../contract/v1/README.md#placement-scope-effect-and-page-buttons) | ✅ | ✅ | [Plugin settings](domains/settings.md), [Plugin settings](../contract/v1/README.md#plugin-settings) |
| `ui` `0` | [`ui`](../contract/v1/README.md#host-owned-workbench-ui): text, [web](../contract/v1/README.md#web-content), and [native](../contract/v1/README.md#host-rendered-native-content) tool windows, commands, and [backend-only commands](../contract/v1/README.md#backend-only-commands) without a tool window | ✅ | ✅¹ | [UI](domains/ui.md) |
| `buildSystems` `0` | [`buildSystems[]`](../contract/v1/README.md#build-systems) without `provider`: detection, options, project model, tasks, run entries, tests, diagnostics, dependencies, sync, and [declarative preparation](../contract/v1/README.md#declarative-project-preparation) | ✅ | ✅¹ | [Declarative build systems](domains/build-systems.md) |
| `buildProviders` `0` | `buildSystems[].provider`: [build providers](../contract/v1/README.md#build-providers), in their own process or a shared language server; they speak `litheBuild`, which covers task execution, test execution and discovery, run preparation, native debug preparation, native configuration, inline test reports, semantic test selectors, typed run options, and registry connection debugging | ✅ | ✅¹ | [Build providers](domains/build-providers.md) |
| `buildModelInputs` `0` | `buildSystems[].modelSharing` and `modelInputs`: [optional read-only model inputs](../contract/v1/README.md#optional-read-only-model-inputs) shared between enabled workspace plugins | ✅ | Not declared | [Build providers](domains/build-providers.md) |
| `vcsProviders` `0` | [`vcsProviders[]`](../contract/v1/README.md#vcs-providers); they speak `litheVcs` | ✅ | Not declared | [VCS providers](domains/vcs.md) |
| `debugAdapters` `0` | [`debugAdapters[]`](../contract/v1/README.md#debug-adapters): standalone adapters, adapters inside a language server, [remote attach](../contract/v1/README.md#remote-attach), arguments and environment, [`features` and `defaults`](../contract/v1/README.md#adapter-features-and-defaults) | ✅ | ✅ | [Debug adapters](domains/debug-adapters.md) |
| `outputLinkMatchers` `0` | [`outputLinkMatchers[]`](../contract/v1/README.md#output-link-matchers): plugin-declared patterns that turn Run and Test console output into links; Core does the matching, and the `resolve` function is called only on click | ✅ | ✅ | [Languages](domains/languages.md) |
| `projectGenerators` `0` | [`projectGenerators[]`](../contract/v1/README.md#project-generators): entries of the New Project window, including [`tool` fields](../contract/v1/README.md#tool-fields) | ✅ | ✅ | [Project generators](domains/project-generators.md) |
| `backend` `0` | [`backend`](../contract/v1/README.md#plugin-backend): the plugin's own long-lived process, started with or without a page, serving the host services its permissions grant and speaking `litheFunctions` | ✅ | ✅¹ | [Plugin backend and plugin functions](domains/backend.md) |
| `toolDefinitions` `0` | `toolDefinitions[]`: [tool identities, layouts, discovery, and four-layer candidates](../contract/v1/toolchains/README.md) | ✅ | ✅¹ | [Toolchains](domains/toolchains.md) |
| `toolchainProviders` `0` | `toolchainProviders[]`: [provider declarations and permission-bound answer validation](../contract/v1/toolchains/README.md); they speak `litheToolchains` | ✅ | Not declared | [Toolchains](domains/toolchains.md) |
| `pluginFunctions` `0` | The manifest's top-level [`pluginFunctions`](../contract/v1/README.md#cross-plugin-calls): the functions the plugin exports (`exports`) and the functions of other plugins it calls (`calls`); grants no manifest permission, and Core relays the calls | ✅ | ✅¹ | [Plugin backend and plugin functions](domains/backend.md) |
| `keymapImportProviders` `0` | `keymapImportProviders[]`: [independent shortcut import providers](../contract/v1/README.md#shortcut-import-providers-core-foundation); they speak `litheKeymapImport` | ✅ | Not declared | [Shortcut import](domains/keymap-import.md) |

¹ The desktop client declares this capability, but some of its parts have no surface yet: the missing part has no button, menu, or window, and the plugin installs and does not fail because of it. The notes above `Declared()` in the desktop source are authoritative for what is not presented ([`pluginhost.go`](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go)):

- Language servers present only diagnostics; completion, hover, navigation, rename, and formatting have no editor surface; a launch answer of `{kind: "tool"}` is refused.
- Tool discovery for system commands reads `PATH` only; a plan that depends on a project toolchain or a user override is refused.
- For build providers: run preparation, tests, debug preparation, model inputs, native configuration, inline test reports, and runtime debugging; for build systems: instance options and task preparations.
- `ui` commands that belong to no tool window.
- Backend host services: the workspace file and search methods (`workspace/readDirectory`, `stat`, `readFile`, `watch`, `findFiles`, `findText`) and `preview/openHTML` are not registered, so a plugin that asks for one of those permissions is refused. At runtime, the reply of `lithe/host/getCapabilities` is authoritative; see the [API catalog](api-catalog.md).

### Process protocol versions

These protocols are negotiated when a process starts and never appear in the manifest. The host offers `{ "versions": [0] }` under `capabilities.experimental.<protocol>` in `initialize`; the backend answers `{ "version": 0 }`. An answer with a version the host did not offer, or a malformed answer, leaves the whole protocol disabled. A protocol's version is the version of the capability that owns it.

| Version | Content | Core | Lithe desktop | See |
| --- | --- | --- | --- | --- |
| `litheFunctions` `0` (`backend`) | Plugin functions: register, withdraw, invoke, results, progress, cancellation; the `lithe/host/*` host services (credentials, workspace contexts and capability discovery, host notifications, public document, file, and window methods, plugin-owned native terminals, independent editor actions, editor language-feature providers, run and debug sessions, language server methods, content providers); cross-plugin calls `functions/providers` and `functions/invoke`. Offered to plugin-owned sessions | ✅ | ✅¹ | [Plugin functions](../contract/v1/README.md#plugin-functions) |
| `litheBuild` `0` (`buildProviders`) | `lithe/build/*`: model and dependencies, task execution, test execution and discovery, run preparation, native debug preparation, read-only model inputs, native configuration, inline test reports, semantic test selectors, typed run options, registry connection debugging. Offered to build providers | ✅ | ✅¹ | [Build providers](../contract/v1/README.md#build-providers) |
| `litheVcs` `0` (`vcsProviders`) | VCS provider capabilities and requests (`lithe/vcs/*`). Offered to VCS providers | ✅ | Not declared | [VCS providers](../contract/v1/README.md#vcs-providers) |
| `litheToolchains` `0` (`toolchainProviders`) | `toolchains/query`; the backend selects a subset of its declared providers. Offered to plugin sessions that declare toolchain providers | ✅ | Not declared | [Provider backend negotiation](../contract/v1/toolchains/README.md#provider-backend-negotiation) |
| `litheKeymapImport` `0` (`keymapImportProviders`) | The backend selects a subset of its declared providers; `lithe/keymap/import`. Offered to plugin sessions with declared `keymapImportProviders` | ✅ | Not declared | [Shortcut import](../contract/v1/README.md#shortcut-import-providers-core-foundation) |
| `litheLogging` `0` (`pluginAPIVersion`) | Structured diagnostic notification `lithe/log`; the official SDKs require the host to offer it and refuse to initialize otherwise. Offered to every plugin-owned process | ✅ | ✅ | [Structured diagnostics](../contract/v1/README.md#structured-plugin-diagnostics) |

- The host services of `litheFunctions` are not told apart by a version: a backend calls `lithe/host/getCapabilities` to learn which methods the host has registered right now, bound by its permissions. A new host method, notification, or optional field needs no new version; see [Extending the host services](../contract/v1/README.md#extending-the-host-services).
- Parts of `litheBuild` that the host does not present are handled as in footnote¹ above: a backend may implement them, and the host never calls them.
- A plugin-private function needs no manifest field, so adding or removing one changes no version.

### Document format versions

| Version | Content | Core | Lithe desktop | See |
| --- | --- | --- | --- | --- |
| Native UI document `schemaVersion` `0` | The document of host-rendered native controls | ✅ | ✅ (with `ui`) | [`native-ui.schema.json`](../contract/v1/native-ui.schema.json) |
| Marketplace index `schemaVersion` `0` | The index file of a marketplace source | ✅ | ✅ | [`plugin-index.schema.json`](../contract/v1/plugin-index.schema.json), [Marketplace](domains/marketplace.md) |
| `version` `0` of the build model `litheBuildModel`, dependency tree `litheDependencies`, test results `litheTestResults`, and test discovery `litheTestDiscovery` | Canonical build documents | ✅ | ✅¹ | [Model and outputs](../contract/v1/README.md#model-and-outputs), [Test discovery](../contract/v1/README.md#test-discovery) |
| `version` `0` of the native build configuration `litheBuildConfiguration` and `litheConfigurationEdit` | Source-backed native configuration documents | ✅ | Not presented yet¹ | [Optional native configuration](../contract/v1/README.md#optional-native-configuration) |
| `version` `0` of the `modelInputs` snapshot | Read-only model inputs shared with other workspace plugins | ✅ | Not declared (`buildModelInputs`) | [Optional read-only model inputs](../contract/v1/README.md#optional-read-only-model-inputs) |
| `version` `0` of the `plugins.editorActions.decode` request | The request version of a Core command; required, and Core accepts only `0` | ✅ | Not presented yet¹ | [Independent editor actions](../contract/v1/README.md#independent-editor-actions) |
| Plugin web bridge `window.lithe.version` `0` | The bridge version package web content sees (`bridgeVersion`) | ✅ | ✅ | [Web content](../contract/v1/README.md#web-content) |
| `backend.version` in a launch plan | Taken from the manifest's `requires.backend`, that is `0` | ✅ | ✅ | [Plugin backend](../contract/v1/README.md#plugin-backend) |

The `version` of an exported or called plugin function (1 to 1000) is not a contract version: the plugin that exports the function owns it.

### Official SDKs

Node.js SDK 0.5.0, Java SDK 0.5.0, and Rust SDK 0.4.0 speak the version `0` protocols; which protocols each SDK wraps is in the contract's [Official backend SDK](../contract/v1/README.md#official-backend-sdk) and in the [Node.js](../sdk/node/README.md), [Java](../sdk/java/README.md), and [Rust](../sdk/rust/README.md) READMEs. Downloads are on the [Lithe-core-rust Releases](https://github.com/Lithe-IDEA/Lithe-core-rust/releases) page.
