# 插件参考 / Plugin reference

[中文](#中文) · [English](#english)

## 中文

这里按功能整理插件契约：每个编号的 API 现在能用什么，每个能力、协议和格式的版本，以及 Core 和 Lithe 桌面端各自支持哪些部分。所有版本号都从 0 开始。规范以英文的 [`contract/v1/README.md`](../contract/v1/README.md) 为准，本目录只做导航和对照；两边不一致时以契约为准，并请到 [Core 仓库提交问题](https://github.com/Lithe-IDEA/Lithe-core-rust/issues)。

### 从哪里开始

| 想知道 | 看这里 |
| --- | --- |
| API-01–44 每一项现在能用什么、还缺什么 | [插件 API 总表](api-catalog.md) |
| `requires` 能力版本、进程协议版本、文档格式版本，以及 Core 和 Lithe 桌面端支持哪些 | [能力与协议版本对照](capabilities-and-versions.md) |
| 每个协议、能力和 API 是哪天、哪个 PR 改的 | [插件协议更新记录](protocol-changelog.md)（只有中文） |
| 某个功能的字段、宿主现状、契约章节和示例 | 下方的功能页 |
| 一步步做出一个插件 | [开发指南](../guides/) |

### 功能页

| 功能页 | 内容 | API 编号 | 能力（`requires`） |
| --- | --- | --- | --- |
| [清单与打包](domains/manifest.md) | `plugin.json` 字段、权限与激活事件、包目录 | 不在编号内 | — |
| [语言、语言服务器与运行/测试](domains/languages.md) | 语言、语法、语言服务器、进程启动、运行和测试命令、控制台输出链接 | API-25 | `languages`、`languageServers`、`executionProviders`、`outputLinkMatchers` |
| [插件后端与插件函数](domains/backend.md) | 后端进程、函数协议、宿主方法、工作区上下文、文件、凭据、终端、跨插件调用 | API-01、04–08、20–22、24、35、37、40、41 | `backend`、`pluginFunctions` |
| [编辑器、文档与预览](domains/editor.md) | 文档和选区、版本化编辑、打开与定位、HTML 预览、编辑器动作、编辑器语言功能 Provider、虚拟文档内容提供者 | API-09、10、12、15、16、25、27 | `backend` |
| [界面](domains/ui.md) | 工具窗口、命令、包内网页、原生控件、后端命令、消息框 | API-02、03、13、14、17 | `ui` |
| [插件设置](domains/settings.md) | 声明式设置、放置位置、作用域、生效方式、设置页按钮、包内翻译 | API-19、API-37 | `settings` |
| [声明式构建系统](domains/build-systems.md) | 构建检测、模型、命令规划、项目准备角色 | API-29 | `buildSystems` |
| [构建提供者](domains/build-providers.md) | `litheBuild` 协议：同步、任务、测试、运行和调试准备、模型输入 | API-29 | `buildProviders`、`buildModelInputs` |
| [调试适配器](domains/debug-adapters.md) | DAP 适配器声明、启动和传输 | API-30 | `debugAdapters` |
| [版本控制提供者](domains/vcs.md) | VCS 提供者进程和协议 | API-31 | `vcsProviders` |
| [项目生成器](domains/project-generators.md) | 新建项目的模板和步骤 | API-33 | `projectGenerators` |
| [工具链](domains/toolchains.md) | 工具定义、可执行文件候选、工具链提供者、Core 命令 | API-23 | `toolDefinitions`、`toolchainProviders` |
| [快捷键导入](domains/keymap-import.md) | 从其他编辑器导入快捷键的提供者 | 不在编号内 | `keymapImportProviders` |
| [包信任](domains/package-trust.md) | 签名要求、签名文件格式、随应用发布的插件 | 不在编号内 | `packageTrust` |
| [插件市场](domains/marketplace.md) | 索引来源和格式、可用性判断、安装审查和确认 | 不在编号内 | — |

计划中、还没有方法的 API（API-11、18、26、28、32、34、36、38、39、42–44）只在[总表](api-catalog.md#计划中的-api)列出，没有功能页。

### 每个功能页的结构

1. 用途和摘要表：能力 ID、Core 命令或协议方法、Lithe 桌面端现状、官方 SDK。
2. 字段、方法、错误码等细节表。
3. 契约章节：链接到 `contract/v1/README.md` 的对应小节。
4. 示例与用例：`contract/v1/fixtures/` 里的文件和用例数量。
5. 相关指南。

---

## English

This directory organizes the plugin contract by feature: what each numbered API offers today, the version of each capability, protocol, and format, and which parts Core and the Lithe desktop client each support. Every version number starts at 0. The English [`contract/v1/README.md`](../contract/v1/README.md) is normative; this directory only navigates and compares. When they disagree the contract wins, and please [file an issue in the Core repository](https://github.com/Lithe-IDEA/Lithe-core-rust/issues).

### Where to start

| To find | Go to |
| --- | --- |
| What each of API-01–44 offers today and what is missing | [Plugin API catalog](api-catalog.md) |
| `requires` capability versions, process protocol versions, document format versions, and which of them Core and the Lithe desktop client support | [Capability and protocol versions](capabilities-and-versions.md) |
| When, and in which PR, each protocol, capability, and API changed | [Plugin protocol changelog](protocol-changelog.md) (Chinese only) |
| A feature's fields, host status, contract sections, and examples | The feature pages below |
| Building a plugin step by step | [Guides](../guides/) |

### Feature pages

| Page | Covers | API numbers | Capabilities (`requires`) |
| --- | --- | --- | --- |
| [Manifest and packaging](domains/manifest.md) | `plugin.json` fields, permissions and activation events, package layout | Not numbered | — |
| [Languages, language servers, run and test](domains/languages.md) | Languages, grammars, language servers, process launch, run and test commands, console output links | API-25 | `languages`, `languageServers`, `executionProviders`, `outputLinkMatchers` |
| [Plugin backend and plugin functions](domains/backend.md) | Backend process, function protocol, host methods, workspace contexts, files, credentials, terminals, cross-plugin calls | API-01, 04–08, 20–22, 24, 35, 37, 40, 41 | `backend`, `pluginFunctions` |
| [Editor, documents, and previews](domains/editor.md) | Documents and selections, versioned edits, open and reveal, HTML preview, editor actions, editor language-feature providers, virtual-document content providers | API-09, 10, 12, 15, 16, 25, 27 | `backend` |
| [UI](domains/ui.md) | Tool windows, commands, package web content, native controls, backend commands, message boxes | API-02, 03, 13, 14, 17 | `ui` |
| [Plugin settings](domains/settings.md) | Declarative settings, placement, scope, apply behavior, settings page actions, packaged translations | API-19, API-37 | `settings` |
| [Declarative build systems](domains/build-systems.md) | Build detection, models, command planning, project preparation roles | API-29 | `buildSystems` |
| [Build providers](domains/build-providers.md) | The `litheBuild` protocol: sync, tasks, tests, run and debug preparation, model inputs | API-29 | `buildProviders`, `buildModelInputs` |
| [Debug adapters](domains/debug-adapters.md) | DAP adapter declarations, launch, and transports | API-30 | `debugAdapters` |
| [VCS providers](domains/vcs.md) | VCS provider processes and protocol | API-31 | `vcsProviders` |
| [Project generators](domains/project-generators.md) | New-project templates and steps | API-33 | `projectGenerators` |
| [Toolchains](domains/toolchains.md) | Tool definitions, executable candidates, toolchain providers, Core commands | API-23 | `toolDefinitions`, `toolchainProviders` |
| [Shortcut import](domains/keymap-import.md) | Providers that import shortcuts from other editors | Not numbered | `keymapImportProviders` |
| [Package trust](domains/package-trust.md) | Signature requirements, the signature file format, plugins bundled with the application | Not numbered | `packageTrust` |
| [Marketplace](domains/marketplace.md) | Index sources and format, availability, install review and confirmation | Not numbered | — |

Planned APIs that have no methods yet (API-11, 18, 26, 28, 32, 34, 36, 38, 39, 42–44) are listed only in the [catalog](api-catalog.md#planned-apis) and have no feature page.

### Layout of each feature page

1. Purpose and a summary table: capability IDs, Core commands or protocol methods, Lithe desktop client status, official SDKs.
2. Detail tables for fields, methods, error codes, and so on.
3. Contract sections: links to the matching sections of `contract/v1/README.md`.
4. Examples and cases: files in `contract/v1/fixtures/` and their case counts.
5. Related guides.
