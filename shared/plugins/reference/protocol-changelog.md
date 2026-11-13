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

### 能力版本（`requires`）

18 个能力都从版本 `0` 开始。

| 能力 | 版本 | 日期 | PR |
| --- | --- | --- | --- |
| `backend` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `buildModelInputs` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `buildProviders` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `buildSystems` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `debugAdapters` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `executionProviders` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `keymapImportProviders` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `languageServers` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `languages` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `outputLinkMatchers` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `packageTrust` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `pluginFunctions` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
| `projectGenerators` | 0 | 2026-11-11 | [#17](https://github.com/Lithe-IDEA/Lithe-core-rust/pull/17) |
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

### 2026-11-14

| PR | 修改人 | API 编号 | 协议 / 能力 | 改了什么 |
| --- | --- | --- | --- | --- |
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
