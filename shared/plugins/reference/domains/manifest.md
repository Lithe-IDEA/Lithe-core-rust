# 清单与打包 / Manifest and packaging

[中文](#中文) · [English](#english)

## 中文

每个插件包的根目录有一份 `plugin.json`。它说明插件是谁、按哪个插件 API 和能力版本编写、需要哪些权限、在什么时候启动。宿主遇到不认识的字段会拒绝整个包，而不是忽略其中一部分。其他功能页只讲各自的清单字段，本页讲所有插件共用的部分。

| 项目 | 内容 |
| --- | --- |
| 文件 | `plugin.json`（[`plugin-manifest.schema.json`](../../contract/v1/plugin-manifest.schema.json)）；发布者签名 `lithe-plugin-signature.json`（见[包信任](package-trust.md)） |
| 基础字段 | `schemaVersion`、`id`、`displayName`、`version`、`pluginAPIVersion`、`requires`、`requiresTools`、`vendor`、`permissions`、`activationEvents` |
| 版本 | `schemaVersion` 和 `pluginAPIVersion` 都固定为 `0`。`requires` 列出清单用到的每个能力，版本都是 `0`。新的能力版本只加给发生变化的那一个能力，其他能力仍写 `0`（规则见[版本与能力](../capabilities-and-versions.md)） |
| API 编号 | 不在 API-01–44 内，属于安装和打包层面的契约 |
| 宿主 | Lithe 桌面端。声明的能力见下文“能力与宿主” |

### 清单字段归属

| 清单字段 | `requires` 里的能力 | 功能页 |
| --- | --- | --- |
| `languages` | `languages` | [语言](languages.md) |
| `languageServers` | `languageServers`；`launch.kind` 为 `backend` 时还需要 `backend` | [语言](languages.md) |
| `languageServers[].hostIntegration` | 保留字段：任何 ID 都返回 `plugin.integration.unsupported` | [语言](languages.md) |
| `executionProviders` | `executionProviders`（桌面端未声明） | [语言](languages.md) |
| `outputLinkMatchers` | `outputLinkMatchers`；匹配器带 `resolve` 时还需要 `backend` | [语言](languages.md) |
| `settings`、`settingActions` | `settings` | [设置](settings.md) |
| `ui` | `ui` | [界面](ui.md) |
| `buildSystems`（不含 `provider`） | `buildSystems`；`modelSharing` 与 `modelInputs` 还需要 `buildModelInputs`（桌面端未声明） | [声明式构建](build-systems.md) |
| `buildSystems[].provider` | `buildProviders` | [构建提供者](build-providers.md) |
| `buildSystems` 里的 `hostIntegration` | 保留字段：任何 ID 都返回 `plugin.integration.unsupported` | [声明式构建](build-systems.md) |
| `vcsProviders` | `vcsProviders`（桌面端未声明） | [VCS](vcs.md) |
| `backend` | `backend` | [插件后端](backend.md) |
| `pluginFunctions` | `pluginFunctions`、`backend`；不授予权限 | [插件后端](backend.md) |
| `projectGenerators` | `projectGenerators` | [项目生成器](project-generators.md) |
| `toolDefinitions` | `toolDefinitions` | [工具链](toolchains.md) |
| `toolchainProviders` | `toolchainProviders`、`backend`（桌面端未声明） | [工具链](toolchains.md) |
| `requiresTools` | 无；列出清单的启动和运行时依赖引用的全部工具定义 ID | [工具链](toolchains.md) |
| `keymapImportProviders` | `keymapImportProviders`、`backend`（桌面端未声明） | [快捷键导入](keymap-import.md) |
| `vendor.bundledSignatureRequirement` | `packageTrust`（桌面端未声明） | [包信任](package-trust.md) |
| `debugAdapters` | `debugAdapters` | [调试适配器](debug-adapters.md) |

`requires` 必须写，并且恰好列出清单用到的能力：漏列或多列都是 `plugin.manifest.invalid`。能力名不是契约定义的，或版本不是 `0`，是 `plugin.api.unsupported`，`field` 为 `requires.<能力名>`。

语言服务器和构建系统都有 `hostIntegration` 字段，两处都是保留字段：Core 不实现任何宿主集成，声明任何 ID 都返回 `plugin.integration.unsupported`（见[语言](languages.md)和[声明式构建](build-systems.md)）。语言或构建工具需要的一切，都由插件自己的后端、提供者和声明给出。

### 能力与宿主

Lithe 桌面端声明下列能力，版本都是 `0`：`backend`、`buildProviders`、`buildSystems`、`debugAdapters`、`languageServers`、`languages`、`outputLinkMatchers`、`pluginFunctions`、`projectGenerators`、`settings`、`toolDefinitions`、`ui`。

桌面端**未声明** `buildModelInputs`、`executionProviders`、`keymapImportProviders`、`packageTrust`、`toolchainProviders`、`vcsProviders`。需要这些能力的插件在安装审查时以 `plugin.api.unsupported` 被拒绝，不会被提供安装。

声明了能力不等于每个功能都可见。桌面端已呈现和未呈现的细节见 [`desktop/internal/pluginhost/pluginhost.go`](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go) 中 `Declared()` 上方的注释，各功能页也会注明。

### 权限与激活事件

| 权限 | 什么时候必须声明 | 功能页 |
| --- | --- | --- |
| `languageServer.launch` | `languageServers` 或 `vcsProviders` 不为空，或构建提供者启动自己的进程 | [语言](languages.md)、[VCS](vcs.md)、[构建提供者](build-providers.md) |
| `process.run`、`process.test` | 有执行提供者声明了 `run`、`test`（桌面端未声明 `executionProviders`） | [语言](languages.md) |
| `process.build` | `buildSystems` 不为空 | [声明式构建](build-systems.md) |
| `process.vcs` | `vcsProviders` 不为空 | [VCS](vcs.md) |
| `debug.launch` | `debugAdapters` 不为空 | [调试适配器](debug-adapters.md) |
| `debug.attach` | 适配器在 `requests` 里列出 `attach` 时才需要；也覆盖连接远程主机和端口 | [调试适配器](debug-adapters.md) |
| `project.generate` | `projectGenerators` 不为空 | [项目生成器](project-generators.md) |
| `ui.contribute`、`ui.web` | 声明了 `ui`；工具窗口显示网页内容时加 `ui.web` | [界面](ui.md) |
| `backend.launch` | 声明了 `backend` | [插件后端](backend.md) |
| `keymaps.provide`、`keymaps.readSelectedSources` | `keymapImportProviders` 不为空；后者只读取显式选择的来源里声明的文件 | [快捷键导入](keymap-import.md) |
| `toolchains.provide` | 声明了 `toolchainProviders` | [工具链](toolchains.md) |
| `run.start` | 后端启动和停止宿主持有的 Run 会话（`run/start`、`run/stop`，通知 `run/didExit`）；需要 `backend` | [插件后端](backend.md) |
| `debug.start` | 后端在自己的调试适配器上启动调试会话（`debug/start`）；需要 `backend` 和已声明的 `debugAdapters` | [插件后端](backend.md)、[调试适配器](debug-adapters.md) |
| `languageServer.request` | `languageServers` 中有服务器列出 `requestMethods` 或 `forwardNotifications` 时才可声明；需要 `backend`。只授予对本插件自己服务器的 `languageServer/*` 宿主方法和两个通知 | [语言](languages.md) |
| `documents.contentProvider` | 后端注册只读文档内容提供者；需要 `backend`。只授予 `documents/registerContentProvider`，不授予读取或编辑文档 | [语言](languages.md) |
| `credentials.store`、`workspace.metadata`、`documents.read`、`editor.actions`、`editor.providers`、`preview.open`、`browser.open`、`window.messages`、`terminal.create` | 后端调用对应的宿主方法 | [API 总表](../api-catalog.md#已公开的宿主方法) |

| 激活事件 | 含义 |
| --- | --- |
| `onLanguage:<id>` | 项目里打开该语言的文档时启动语言服务器 |
| `onRun:<id>` | 用户运行该语言的文件时激活运行提供者（桌面端未声明 `executionProviders`） |
| `onTest:<id>` | 用户列出或运行测试时激活测试提供者（桌面端未声明 `executionProviders`） |
| `onBuildSystem:<id>` | 构建系统第一次计划、建模或解码之前激活 |
| `onVcsRoot:<id>` | 工作区里检测到该 VCS 的根目录后启动 VCS 提供者 |
| `onDebug:<id>` | 调试会话需要该适配器时启动 |
| `onView:<window-id>`、`onCommand:<command-id>` | 打开工具窗口、执行命令时激活界面模块；后端命令的 `onCommand` 同时启动后端 |
| `onWorkspaceOpen` | 工作区打开后启动 `backend` 进程；唯一可选的事件，不带 ID |

每个提供者都要有自己的事件，每个事件（`onWorkspaceOpen` 除外）都要对应一个提供者。没有“每次启动都运行”的事件（`onStartupFinished`、`*`），声明了会被拒绝为 `plugin.activation.invalid`。启用插件只是允许激活，事件发生之前什么都不会启动。

### 契约章节

| 章节 | 内容 |
| --- | --- |
| [Package layout](../../contract/v1/README.md#package-layout) | 包的目录结构；路径规则；不允许符号链接；安装后的包只读 |
| [Manifest](../../contract/v1/README.md#manifest) | 全部顶层字段；未知字段一律拒绝（`plugin.manifest.invalid`） |
| [Versioning](../../contract/v1/README.md#versioning) | 版本规则、各文档的版本字段 |
| [Capabilities](../../contract/v1/README.md#capabilities) | `requires`、能力与清单章节的对应关系、运行协议 |
| [Host capabilities](../../contract/v1/README.md#host-capabilities) | 宿主声明能力版本的规则，以及 `validate`、`installReview` 的拒绝 |
| [Process start](../../contract/v1/README.md#process-start) | 语言服务器、构建提供者、VCS 提供者、调试适配器和插件后端共用的启动字段 |
| [Permissions and activation](../../contract/v1/README.md#permissions-and-activation) | 权限表与激活事件表 |
| [Trust](../../contract/v1/README.md#trust) | 包信任与签名要求 |
| [Outcomes and error codes](../../contract/v1/README.md#outcomes-and-error-codes) | 结果与稳定错误码 |
| [Existing community store](../../contract/v1/README.md#existing-community-store) | 社区商店（Lithe Store）与给社区包增加宿主渲染界面的步骤 |

### 示例与用例

| 文件 | 内容 |
| --- | --- |
| [`manifest/hello-lithe.plugin.json`](../../contract/v1/fixtures/manifest/hello-lithe.plugin.json) | 最小的界面插件：一个工具窗口、一个命令。ID 在保留的测试命名空间里，不能直接发布 |
| [`manifest/invalid-manifests.json`](../../contract/v1/fixtures/manifest/invalid-manifests.json) | 189 个拒绝用例，覆盖全部功能。每个用例给某个合法 fixture 打补丁，并写明稳定的拒绝码和字段 |

### 相关指南

- [从零构建一个完整生命周期的 LSP 插件](../../guides/lsp-plugin-lifecycle.md)：清单、打包、状态目录、安装、发布
- [插件接入统一日志](../../guides/plugin-logging.md)

---

## English

Every plugin package has a `plugin.json` at its root. It says who the plugin is, which plugin API and capability versions it was written against, which permissions it needs, and when it starts. A host that meets an unknown field refuses the whole package instead of ignoring part of it. The other domain pages cover their own manifest fields; this page covers what every plugin shares.

| Item | Value |
| --- | --- |
| Files | `plugin.json` ([`plugin-manifest.schema.json`](../../contract/v1/plugin-manifest.schema.json)); publisher signature `lithe-plugin-signature.json` (see [Package trust](package-trust.md)) |
| Base fields | `schemaVersion`, `id`, `displayName`, `version`, `pluginAPIVersion`, `requires`, `requiresTools`, `vendor`, `permissions`, `activationEvents` |
| Versions | `schemaVersion` and `pluginAPIVersion` are always `0`. `requires` lists every capability the manifest uses, each at version `0`. A new capability version is added only to the one capability that changes; the others stay `0` (rules in [Versions and capabilities](../capabilities-and-versions.md)) |
| API IDs | Outside API-01–44; this is an installation and packaging contract |
| Host | Lithe desktop. The capabilities it declares are listed below under "Capabilities and host" |

### Which page owns each manifest field

| Manifest field | Capabilities in `requires` | Domain page |
| --- | --- | --- |
| `languages` | `languages` | [Languages](languages.md) |
| `languageServers` | `languageServers`; plus `backend` when `launch.kind` is `backend` | [Languages](languages.md) |
| `languageServers[].hostIntegration` | Reserved: every ID returns `plugin.integration.unsupported` | [Languages](languages.md) |
| `executionProviders` | `executionProviders` (not declared by the desktop host) | [Languages](languages.md) |
| `outputLinkMatchers` | `outputLinkMatchers`; plus `backend` when a matcher has `resolve` | [Languages](languages.md) |
| `settings`, `settingActions` | `settings` | [Settings](settings.md) |
| `ui` | `ui` | [UI](ui.md) |
| `buildSystems` (without `provider`) | `buildSystems`; `modelSharing` and `modelInputs` also need `buildModelInputs` (not declared by the desktop host) | [Declarative build systems](build-systems.md) |
| `buildSystems[].provider` | `buildProviders` | [Build providers](build-providers.md) |
| `hostIntegration` inside `buildSystems` | Reserved: every ID returns `plugin.integration.unsupported` | [Declarative build systems](build-systems.md) |
| `vcsProviders` | `vcsProviders` (not declared by the desktop host) | [VCS](vcs.md) |
| `backend` | `backend` | [Plugin backend](backend.md) |
| `pluginFunctions` | `pluginFunctions`, `backend`; grants no permission | [Plugin backend](backend.md) |
| `projectGenerators` | `projectGenerators` | [Project generators](project-generators.md) |
| `toolDefinitions` | `toolDefinitions` | [Toolchains](toolchains.md) |
| `toolchainProviders` | `toolchainProviders`, `backend` (not declared by the desktop host) | [Toolchains](toolchains.md) |
| `requiresTools` | None; lists every tool-definition ID the manifest's launches and runtime dependencies name | [Toolchains](toolchains.md) |
| `keymapImportProviders` | `keymapImportProviders`, `backend` (not declared by the desktop host) | [Shortcut import](keymap-import.md) |
| `vendor.bundledSignatureRequirement` | `packageTrust` (not declared by the desktop host) | [Package trust](package-trust.md) |
| `debugAdapters` | `debugAdapters` | [Debug adapters](debug-adapters.md) |

`requires` is required and must list **exactly** the capabilities the manifest uses: a missing or unused one is `plugin.manifest.invalid`. A capability name the contract does not define, or a version other than `0`, is `plugin.api.unsupported` with `field` set to `requires.<capability>`.

Language servers and build systems each have a `hostIntegration` field, and both are reserved: Core implements no host integration, and declaring any ID returns `plugin.integration.unsupported` (see [Languages](languages.md) and [Declarative build systems](build-systems.md)). Everything a language or build tool needs comes from the plugin's own backend, providers, and declarations.

### Capabilities and host

The Lithe desktop host declares these capabilities, each at version `0`: `backend`, `buildProviders`, `buildSystems`, `debugAdapters`, `languageServers`, `languages`, `outputLinkMatchers`, `pluginFunctions`, `projectGenerators`, `settings`, `toolDefinitions`, `ui`.

The desktop host does **not** declare `buildModelInputs`, `executionProviders`, `keymapImportProviders`, `packageTrust`, `toolchainProviders`, or `vcsProviders`. A plugin that needs one of them is refused at install review with `plugin.api.unsupported` and is not offered for installation.

Declaring a capability does not mean every feature of it is visible. What the desktop host presents and what it does not is described in the comment above `Declared()` in [`desktop/internal/pluginhost/pluginhost.go`](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go), and each domain page says so where it matters.

### Permissions and activation events

| Permission | Required when | Domain page |
| --- | --- | --- |
| `languageServer.launch` | `languageServers` or `vcsProviders` is not empty, or a build provider starts its own process | [Languages](languages.md), [VCS](vcs.md), [Build providers](build-providers.md) |
| `process.run`, `process.test` | an execution provider declares `run` or `test` (the desktop host does not declare `executionProviders`) | [Languages](languages.md) |
| `process.build` | `buildSystems` is not empty | [Declarative build systems](build-systems.md) |
| `process.vcs` | `vcsProviders` is not empty | [VCS](vcs.md) |
| `debug.launch` | `debugAdapters` is not empty | [Debug adapters](debug-adapters.md) |
| `debug.attach` | only when an adapter lists `attach` in `requests`; it also covers attaching to a remote host and port | [Debug adapters](debug-adapters.md) |
| `project.generate` | `projectGenerators` is not empty | [Project generators](project-generators.md) |
| `ui.contribute`, `ui.web` | `ui` is declared; `ui.web` when a tool window shows web content | [UI](ui.md) |
| `backend.launch` | `backend` is declared | [Plugin backend](backend.md) |
| `keymaps.provide`, `keymaps.readSelectedSources` | `keymapImportProviders` is not empty; the second permits reading only declared files in an explicitly selected source | [Shortcut import](keymap-import.md) |
| `toolchains.provide` | `toolchainProviders` is declared | [Toolchains](toolchains.md) |
| `run.start` | the backend starts and stops host-owned Run sessions (`run/start`, `run/stop`, notification `run/didExit`); needs `backend` | [Plugin backend](backend.md) |
| `debug.start` | the backend starts a debug session on one of its own adapters (`debug/start`); needs `backend` and a declared `debugAdapters` entry | [Plugin backend](backend.md), [Debug adapters](debug-adapters.md) |
| `languageServer.request` | a `languageServers` server lists `requestMethods` or `forwardNotifications`, and only then; needs `backend`. Grants the `languageServer/*` host methods for the plugin's own servers and their two notifications, nothing else | [Languages](languages.md) |
| `documents.contentProvider` | the backend registers a read-only document content provider; needs `backend`. Grants `documents/registerContentProvider` only, not reading or editing documents | [Languages](languages.md) |
| `credentials.store`, `workspace.metadata`, `documents.read`, `editor.actions`, `editor.providers`, `preview.open`, `browser.open`, `window.messages`, `terminal.create` | the backend calls the matching host methods | [API catalog](../api-catalog.md#public-host-methods) |

| Activation event | Meaning |
| --- | --- |
| `onLanguage:<id>` | Start the language server when a document of the language opens in a project |
| `onRun:<id>` | Activate the run provider when the user runs a file of the language (the desktop host does not declare `executionProviders`) |
| `onTest:<id>` | Activate the test provider when the user lists or runs tests (the desktop host does not declare `executionProviders`) |
| `onBuildSystem:<id>` | Activate the build system before its first plan, model, or decode |
| `onVcsRoot:<id>` | Start the VCS provider once a root of its VCS is detected |
| `onDebug:<id>` | Start the debug adapter when a debug session needs it |
| `onView:<window-id>`, `onCommand:<command-id>` | Activate the UI module when a tool window opens or a command runs; an `onCommand` for a backend command also starts the backend |
| `onWorkspaceOpen` | Start a `backend` process once a workspace is open; the only optional event, and it names no ID |

Each provider needs its event and each event (except `onWorkspaceOpen`) needs a provider. There is no event that starts a plugin on every launch (`onStartupFinished`, `*`); declaring one is `plugin.activation.invalid`. Enabling a plugin only permits activation; nothing starts until its event occurs.

### Contract sections

| Section | Content |
| --- | --- |
| [Package layout](../../contract/v1/README.md#package-layout) | Package structure, path rules, no symbolic links, installed packages are read-only |
| [Manifest](../../contract/v1/README.md#manifest) | Every top-level field; unknown fields are refused (`plugin.manifest.invalid`) |
| [Versioning](../../contract/v1/README.md#versioning) | Versioning rules and the version field of each document |
| [Capabilities](../../contract/v1/README.md#capabilities) | `requires`, the mapping from capabilities to manifest sections, and runtime protocols |
| [Host capabilities](../../contract/v1/README.md#host-capabilities) | How a host declares capability versions, and the refusals of `validate` and `installReview` |
| [Process start](../../contract/v1/README.md#process-start) | Launch fields shared by language servers, build providers, VCS providers, debug adapters and the plugin backend |
| [Permissions and activation](../../contract/v1/README.md#permissions-and-activation) | Permission and activation event tables |
| [Trust](../../contract/v1/README.md#trust) | Package trust and signature requirements |
| [Outcomes and error codes](../../contract/v1/README.md#outcomes-and-error-codes) | Outcomes and stable error codes |
| [Existing community store](../../contract/v1/README.md#existing-community-store) | The community store (Lithe Store) and the steps to give a community package a host-rendered view |

### Examples and cases

| File | Content |
| --- | --- |
| [`manifest/hello-lithe.plugin.json`](../../contract/v1/fixtures/manifest/hello-lithe.plugin.json) | The smallest UI plugin: one tool window and one command. Its ID is in the reserved test namespace and cannot be published as is |
| [`manifest/invalid-manifests.json`](../../contract/v1/fixtures/manifest/invalid-manifests.json) | 189 rejection cases across every domain. Each patches a valid fixture and names the stable rejection code and field |

### Related guides

- [Building an LSP plugin end to end](../../guides/lsp-plugin-lifecycle.md): manifest, packaging, state directories, installation, publishing
- [Plugin logging](../../guides/plugin-logging.md)
