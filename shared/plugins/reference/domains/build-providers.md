# 构建提供者（litheBuild） / Build providers (litheBuild)

[中文](#中文) · [English](#english)

## 中文

有些构建工具只能通过自己的 API 给出项目模型（Gradle Tooling API、BEAM 里的 Mix）。这类插件在 `buildSystems[].provider` 声明一个构建提供者：一个和语言服务器同类的长驻 LSP 基础协议进程，回答 `lithe/build/*` 请求。模型、依赖、任务、测试、运行准备、调试准备、原生配置都可以交给它；宿主仍然负责启动和停止进程、显示界面、运行最终的程序。

> **宿主支持。** Lithe 桌面端声明 `buildSystems` 和 `buildProviders`（版本都是 `0`），不声明 `buildModelInputs`：`requires` 里有它的插件（只读模型输入的生产者和消费者）不会被提供安装。构建窗口发现构建实例、加载提供者模型、解析可执行文件候选并运行命令任务和提供者任务。
> 构建提供者的运行准备、测试、调试准备、模型输入、原生配置、内联测试报告和运行时调试，桌面端尚未呈现；`Declared()` 上方的同一段注释还写到构建窗口会准备应用目标，具体以该注释为准。
> 宿主只呈现它实现的功能，提供者不得依赖宿主一定会调用其中任何一个。说明见 [`pluginhost.go`](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go)；各能力的对照见[能力与协议版本对照](../capabilities-and-versions.md#能力版本requires)。
> 对契约有疑问，请到 [Lithe-core-rust 的 Issues](https://github.com/Lithe-IDEA/Lithe-core-rust/issues) 反馈。

| 项目 | 内容 |
| --- | --- |
| 清单字段 | `buildSystems[].provider`，以及各声明里的 `source: "provider"` |
| 能力版本 | `buildProviders` `0`；任何 `buildSystems[].provider` 都要同时有 `buildSystems: 0` 和 `buildProviders: 0`，不论它自己启动进程还是共用语言服务器；只用模板的构建插件不写 `buildProviders`。`requires` 必须恰好列出清单用到的能力；启动进程是基础契约，不是能力，不写 `requires.serverProcess`。见[版本对照](../capabilities-and-versions.md#能力版本requires) |
| 协议 | `litheBuild` `0`：宿主在 `initialize` 里提供 `capabilities.experimental.litheBuild: {"versions": [0]}`，提供者回答 `{"version": 0}`。协议没有自己的版本，就是 `buildProviders` 的版本，见[进程协议版本](../capabilities-and-versions.md#进程协议版本) |
| API 编号 | [API-29](../api-catalog.md#现在能用的-api)：构建、任务、测试、运行准备 |
| 权限 | `process.build`；自有进程的提供者加 `languageServer.launch`；原生调试准备加 `debug.launch` |
| 激活事件 | `onBuildSystem:<id>`；提供者在第一次由用户操作触发的构建请求时启动 |
| 官方 SDK | [Node.js 0.5.0](../../sdk/node/README.md) 和 [Java 0.5.0](../../sdk/java/README.md) 有 `litheBuild` 的类型化处理器：模型、依赖、任务执行、测试执行、测试发现、运行准备、调试准备；原生配置（`lithe/build/configuration`、`editConfiguration`）没有类型化处理器。[Rust 0.4.0](../../sdk/rust/README.md) 没有构建提供者接口。发布标签 `sdk-node-v0.5.0`、`sdk-java-v0.5.0`、`sdk-rust-v0.4.0`，见[官方 SDK](../capabilities-and-versions.md#官方-sdk) |

### 两种形式

用 `languageServer` 键区分两种形式，混用是 `plugin.manifest.invalid`。声明了 `provider` 却没有任何来源使用它，或者来源是 `provider` 却没有声明它，也是 `plugin.manifest.invalid`。

| 形式 | 字段 | 说明 |
| --- | --- | --- |
| 共享 | `{"languageServer": "<languageID>", "executesProjectCode", "mayUseNetwork"}` | 同一插件里必须有该语言的语言服务器，请求发给它的会话；`requires` 里还要有 `languageServers: 0` 和 `languages: 0`。共享形式不能用原生配置 |
| 自有进程 | 语言服务器的启动字段（`launch`、`arguments`、`validationArguments`、`runtimeDependencies`、`initializationOptions`）加 `executesProjectCode`、`mayUseNetwork` | 校验、启动模板、设置和安装审查与语言服务器相同，需要 `languageServer.launch` 权限；审查里列为 `packagedBuildProvider`、`systemBuildProvider` 或 `runtimeBuildProvider`，并带 `buildSystemID` 和两个标志 |

检测从不启动提供者：来源为 `provider` 的实例只报告 `modelSource: "provider"`，没有模型。宿主按 `buildSystemID` 用 `plugins.declarative.languageServerLaunchPlan` 启动进程。

### 提供者能做什么

所有功能都属于 `buildProviders` `0`，用同一个 `litheBuild` 协议。一次会话里能用到哪些，取决于清单声明了哪个来源，以及宿主实现了哪些。

| 功能 | 清单里的声明 | 方法 | 契约章节 | 示例（在 `fixtures/build-providers/`，另有说明的除外） |
| --- | --- | --- | --- | --- |
| 项目模型和依赖 | `model.source`、`dependencies.source` 为 `provider` | `lithe/build/model`、`lithe/build/dependencies` | [Build providers](../../contract/v1/README.md#build-providers) | `build-provider`、`build-provider-language-server` |
| 任务执行 | `tasks[].source: "provider"`，`arguments` 必须是 `[]` | `lithe/build/execute`、`lithe/build/progress` | [Provider task execution](../../contract/v1/README.md#provider-task-execution) | `build-provider-execution`、`build-execution-protocol.json` |
| 测试执行 | `test.source: "provider"`，`arguments` 必须是 `[]`，不能用 `hostIntegration` 报告 | `lithe/build/test` | [Provider test execution](../../contract/v1/README.md#provider-test-execution) | `build-provider-tests`、`build-test-protocol.json` |
| 测试发现 | provider 测试声明 `discovery: true`，同时需要 `suite` 和 `case` 过滤器 | `lithe/build/discoverTests` | [Test discovery](../../contract/v1/README.md#test-discovery) | `build-test-discovery.json` |
| 运行准备 | `run[].source: "provider"`，用 `fromTargets.tags`；`execution` 为 `application`（默认）、`service` 或 `task` | `lithe/build/prepareRun` | [Provider run preparation](../../contract/v1/README.md#provider-run-preparation) | `build-provider-run-preparation`、`build-run-preparation.json` |
| 类型化运行选项 | 服务和任务的准备带 `launchOptions`（`execution`、`runtimeArguments`、`arguments`、`environment`，可选 `entrypoint`、`workingDirectory`） | 扩展 `lithe/build/prepareRun` | [Typed run options and service/task preparation](../../contract/v1/README.md#typed-run-options-and-servicetask-preparation) | `build-provider-run-options`、`build-run-options.json` |
| 注册表连接调试 | 服务运行声明 `debug: {"configurationType": "…"}`，由宿主选择已启用、声明了该类型且支持 `attach` 的调试适配器 | 沿用 `lithe/build/prepareRun` | [Registry connection debugging](../../contract/v1/README.md#registry-connection-debugging) | `build-connection-debug-options.json`（没有单独的清单示例） |
| 原生调试准备 | 运行声明 `debug: {"adapter": "native"}`；`requires` 要有 `debugAdapters: 0`，并申请 `debug.launch`；适配器必须是本插件自己声明的 | `lithe/build/prepareDebug` | [Native debug preparation](../../contract/v1/README.md#native-debug-preparation) | `build-provider-debug-preparation`、`build-debug-preparation.json` |
| 只读模型输入 | 消费者声明 `modelInputs`（最多 8 个槽），`requires` 要有 `buildModelInputs: 0` 和 `buildProviders: 0`；生产者声明 `modelSharing` | 沿用模型、依赖和各计划请求，快照在 `params.modelInputs` | [Optional read-only model inputs](../../contract/v1/README.md#optional-read-only-model-inputs) | `build-systems/build-model-producer`、`build-systems/build-model-consumer` |
| 以来源为准的原生配置 | `configuration: {"source": "provider"}`，只限自有进程；提供者在服务器特性里声明 `litheBuildConfiguration: true` | `lithe/build/configuration`、`lithe/build/editConfiguration` | [Optional native configuration](../../contract/v1/README.md#optional-native-configuration) | `build-provider-configuration`、`build-configuration.json` |
| 内联测试报告 | `results: {"decoder": "provider"}`，回复里带 `testResults` | 沿用 `lithe/build/test`、`execute` | [Inline provider test reports](../../contract/v1/README.md#inline-provider-test-reports) | `build-provider-reports`、`build-test-durations.json`（重复用例耗时求和） |
| 不透明的语义测试选择器 | provider 测试声明 `selectorKinds`（最多 16 个命名空间），请求带 `selectorKind` 和 `selectors` | 沿用 `lithe/build/test` | [Opaque semantic test selectors](../../contract/v1/README.md#opaque-semantic-test-selectors) | `build-provider-selectors` |
| 声明式项目准备 | 项目任务的 `preparation: {purpose, sourceRoles}`；属于 `buildSystems`，提供者任务经 `lithe/build/execute` 运行 | 沿用 `lithe/build/execute` | [Declarative project preparation](../../contract/v1/README.md#declarative-project-preparation) | `build-provider-preparation` |

- 提供者的结果必须符合[模型与输出](../../contract/v1/README.md#model-and-outputs)里的格式；SDK 为模型、依赖和测试发现补齐 `format` 和 `version`，处理器返回别的格式或版本会失败。
- 提供者必须拒绝它无法精确满足的选择器、过滤器和陈旧的项目事实，不能放宽范围去猜；任务和测试的非零退出码是已完成的工具失败，不是协议错误，畸形回复是 `plugin.build.invalidResult`，JSON-RPC 错误是 `plugin.build.failed`。
- 提供者永远不回退到宿主命令或插件私有函数；没有协商成功时，构建方法在进入处理器之前被拒绝。

### 协商与隔离

- 宿主发 `capabilities.experimental.litheBuild: {"versions": [0]}`，提供者回答自己选的 `{"version": 0}`。提供者没有回答版本，或回答了宿主没给出的版本，就不用于构建请求；共享形式的提供者仍然服务它的语言。双方都忽略这些对象和请求参数、结果里不认识的字段，所以任何一方都可以增加可选数据；只有对方不能忽略的改动，才会给 `buildProviders` 版本 `1`，协议随之变化。
- 一个插件、一个构建系统、一个工作区一个进程。两个插件从不共享进程，宿主也从不在进程之间转发消息；每个进程只经自己的标准输入输出与宿主说 JSON-RPC。共享形式只复用同一插件的语言服务器会话。
- 提供者在第一次构建请求时启动（只有用户操作才会发出），在工作区关闭、插件停用，或者包被替换、回滚、卸载之前停止。插件设置改变后，下一次请求重启它，所以启动参数总是反映当前值；退出的提供者由下一次请求重新启动。
- 安装时拒绝这样的包：提供者的启动指向不存在的文件、包外路径或经符号链接的路径，或者打包的可执行文件和运行时不是常规可执行文件。
- 可写状态在插件为该构建系统准备的状态目录里，不在插件包或宿主安装目录里。
- 请求每次都带 `buildSystemID`、`root`、`options`、`optionArguments` 和 `environment`；`environment` 是所选运行时或工具的环境与选项贡献的合并（选项优先），宿主不转发它的完整环境，提供者对每次调用和缓存标识都用它。`refresh: true` 表示用户要求重新加载，提供者必须重新计算而不是从自己的缓存回答。可选的 `tool: {executable}` 是宿主按构建系统的 `invocation` 解析出的构建工具的绝对路径，未给出时由提供者自己查找；明确指定的工具链或路径解析失败时，请求不会发出。

### 限制

| 项目 | 上限 |
| --- | --- |
| 进度 `message` | 4 KiB UTF-8 |
| 进度 `output` | 每条通知 64 KiB UTF-8 |
| 任务和测试的最终 `output` | 32 MiB UTF-8；`exitCode` 是有符号 32 位整数 |
| 启动结果 | 参数最多 1,024 项、每项 64 KiB；环境最多 64 项（键 `[A-Za-z_][A-Za-z0-9_]*` 至多 128 字节，值至多 64 KiB）；整个 JSON 至多 32 MiB |
| 测试发现 | 最多 10,000 项，32 MiB UTF-8 JSON |
| 内联测试报告 | 最多 10,000 个用例，8 MiB UTF-8 JSON |
| 语义选择器 | `selectorKinds` 最多 16 个（每个 120 字节）；每次请求最多 64 个选择器，每个 4096 字节 |
| 原生配置 | 最多 8 个来源，每个来源最多 64 个字段 |
| 模型输入 | 最多 8 个槽，每个来源 64 个实例，每个实例 2,000 个项目和 10,000 个目标或内容根，解析后的 JSON 8 MiB |

### 契约章节

| 章节 | 内容 |
| --- | --- |
| [Build providers](../../contract/v1/README.md#build-providers) | 两种形式、请求参数、`initialize` 协商、进程隔离和生命周期 |
| [Provider task execution](../../contract/v1/README.md#provider-task-execution) | 任务执行、进度、取消 |
| [Provider test execution](../../contract/v1/README.md#provider-test-execution)、[Test discovery](../../contract/v1/README.md#test-discovery) | 测试执行、报告、测试发现 |
| [Provider run preparation](../../contract/v1/README.md#provider-run-preparation) | 运行准备、可移植启动计划 |
| [Typed run options and service/task preparation](../../contract/v1/README.md#typed-run-options-and-servicetask-preparation) | `launchOptions`、服务与任务准备 |
| [Registry connection debugging](../../contract/v1/README.md#registry-connection-debugging) | 经注册表选择 attach 适配器的连接调试 |
| [Native debug preparation](../../contract/v1/README.md#native-debug-preparation) | 原生调试准备和适配器绑定 |
| [Optional native configuration](../../contract/v1/README.md#optional-native-configuration) | 原生配置的读取和分阶段编辑 |
| [Optional read-only model inputs](../../contract/v1/README.md#optional-read-only-model-inputs) | 生产者与消费者之间的只读模型快照 |
| [Inline provider test reports](../../contract/v1/README.md#inline-provider-test-reports) | 回复里的 `litheTestResults` 报告 |
| [Opaque semantic test selectors](../../contract/v1/README.md#opaque-semantic-test-selectors) | 语义测试选择器 |
| [Declarative project preparation](../../contract/v1/README.md#declarative-project-preparation) | 运行语言启动之前的项目准备任务 |
| [Model and outputs](../../contract/v1/README.md#model-and-outputs) | 提供者结果要符合的模型、诊断、依赖格式 |

### 文档格式

所有格式的版本都是 `0`，见[文档格式版本](../capabilities-and-versions.md#文档格式版本)。

| 格式 | 用在 | 示例 |
| --- | --- | --- |
| `litheBuildModel` | 模型结果 | [`build-plans.json`](../../contract/v1/fixtures/build-systems/build-plans.json) |
| `litheDependencies` | 依赖结果 | 契约正文，见 [Model and outputs](../../contract/v1/README.md#model-and-outputs) |
| `litheDiagnostics` | 输出行里的诊断 | 契约正文，见 [Model and outputs](../../contract/v1/README.md#model-and-outputs) |
| `litheTestResults` | 测试报告、内联报告 | 契约正文，见 [Inline provider test reports](../../contract/v1/README.md#inline-provider-test-reports)；重复用例的耗时求和规则见 [`build-test-durations.json`](../../contract/v1/fixtures/build-providers/build-test-durations.json) |
| `litheTestDiscovery` | 测试发现 | [`build-test-discovery.json`](../../contract/v1/fixtures/build-providers/build-test-discovery.json) |
| `litheBuildConfiguration` | 原生配置的事实 | [`build-configuration.json`](../../contract/v1/fixtures/build-providers/build-configuration.json) |
| `litheConfigurationEdit` | 原生配置编辑的回执 | 契约正文，见 [Optional native configuration](../../contract/v1/README.md#optional-native-configuration) |
| `modelInputs` 快照 | 只读模型输入 | [`build-model-consumer.plugin.json`](../../contract/v1/fixtures/build-systems/build-model-consumer.plugin.json) |
| `launchOptions` 对象 | 服务、任务和连接调试的运行选项；示例文件外层标为 `format: "litheBuildRunOptions"`、`version: 0` | [`build-run-options.json`](../../contract/v1/fixtures/build-providers/build-run-options.json)、[`build-connection-debug-options.json`](../../contract/v1/fixtures/build-providers/build-connection-debug-options.json) |

### 相关指南

- [原生构建、测试与运行准备](../../guides/native-build-workflows.md)
- [为语言插件接入构建、运行与测试](../../guides/build-tool-integration.md)
- [声明式构建系统](build-systems.md)

---

## English

Some build tools expose their project model only through their own API (the Gradle Tooling API, Mix inside the BEAM). Such a plugin declares a build provider in `buildSystems[].provider`: a long-lived LSP base-protocol process of the same kind as a language server, which answers `lithe/build/*` requests. Models, dependencies, tasks, tests, run preparation, debug preparation, and native configuration can all be handed to it. The host still starts and stops the process, draws the UI, and runs the final program.

> **Host support.** The Lithe desktop declares `buildSystems` and `buildProviders` (both version `0`) and does not declare `buildModelInputs`: a plugin whose `requires` lists it (a producer or consumer of read-only model inputs) is not offered for install. The Build window detects build instances, loads provider models, resolves executable candidates, and runs command tasks and provider tasks.
> Build-provider run preparation, tests, debug preparation, model inputs, native configuration, inline test reports, and runtime debugging are not presented on the desktop yet; the same comment above `Declared()` also says the Build window prepares application targets, so read that comment for the current state.
> A host presents the features it implements, and a provider must not depend on a host calling any of them. See [`pluginhost.go`](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go); for a per-capability comparison see the [capability and protocol versions](../capabilities-and-versions.md#capability-versions-requires).
> Questions about the contract go to [Lithe-core-rust Issues](https://github.com/Lithe-IDEA/Lithe-core-rust/issues).

| Item | Value |
| --- | --- |
| Manifest fields | `buildSystems[].provider`, and `source: "provider"` in individual declarations |
| Capability versions | `buildProviders` `0`; every `buildSystems[].provider` needs both `buildSystems: 0` and `buildProviders: 0`, whether it starts its own process or shares a language server; a template-only build plugin omits `buildProviders`. `requires` must list exactly the capabilities the manifest uses; process start is base contract, not a capability, so do not write `requires.serverProcess`. See [versions](../capabilities-and-versions.md#capability-versions-requires) |
| Protocols | `litheBuild` `0`: the host offers `capabilities.experimental.litheBuild: {"versions": [0]}` in `initialize`, and the provider answers `{"version": 0}`. The protocol has no version of its own: it is the version of `buildProviders`; see [process protocol versions](../capabilities-and-versions.md#process-protocol-versions) |
| API IDs | [API-29](../api-catalog.md#apis-usable-now): build, tasks, tests, run preparation |
| Permissions | `process.build`; `languageServer.launch` for an own-process provider; `debug.launch` for native debug preparation |
| Activation events | `onBuildSystem:<id>`; a provider starts on the first build request, which only a user action sends |
| Official SDKs | [Node.js 0.5.0](../../sdk/node/README.md) and [Java 0.5.0](../../sdk/java/README.md) have typed `litheBuild` handlers: model, dependencies, task execution, test execution, test discovery, run preparation, and debug preparation; native configuration (`lithe/build/configuration`, `editConfiguration`) has no typed handler. [Rust 0.4.0](../../sdk/rust/README.md) has no build-provider interface. Released as `sdk-node-v0.5.0`, `sdk-java-v0.5.0`, and `sdk-rust-v0.4.0`; see [official SDKs](../capabilities-and-versions.md#official-sdks) |

### Two forms

The `languageServer` key tells the two forms apart, and mixing them is `plugin.manifest.invalid`. A `provider` that no source uses, or a `provider` source without a provider, is `plugin.manifest.invalid` too.

| Form | Fields | Notes |
| --- | --- | --- |
| Shared | `{"languageServer": "<languageID>", "executesProjectCode", "mayUseNetwork"}` | The plugin must have a language server for that language, and requests go to its session; `requires` also needs `languageServers: 0` and `languages: 0`. The shared form cannot use native configuration |
| Own process | The language-server launch fields (`launch`, `arguments`, `validationArguments`, `runtimeDependencies`, `initializationOptions`) plus `executesProjectCode` and `mayUseNetwork` | Validation, launch templates, settings, and the install review are those of a language server, and it needs the `languageServer.launch` permission; the review lists it as `packagedBuildProvider`, `systemBuildProvider`, or `runtimeBuildProvider` with its `buildSystemID` and the two flags |

Detection never starts a provider: an instance with a `provider` source only reports `modelSource: "provider"`, with no model. The host starts the process with `plugins.declarative.languageServerLaunchPlan` and the `buildSystemID`.

### What a provider can do

Every feature belongs to `buildProviders` `0` and uses the same `litheBuild` protocol. Which of them a session uses depends on the sources the manifest declares and on what the host implements.

| Feature | Declared in the manifest | Methods | Contract section | Example (in `fixtures/build-providers/` unless noted) |
| --- | --- | --- | --- | --- |
| Project model and dependencies | `model.source` and `dependencies.source` set to `provider` | `lithe/build/model`, `lithe/build/dependencies` | [Build providers](../../contract/v1/README.md#build-providers) | `build-provider`, `build-provider-language-server` |
| Task execution | `tasks[].source: "provider"`; `arguments` must be `[]` | `lithe/build/execute`, `lithe/build/progress` | [Provider task execution](../../contract/v1/README.md#provider-task-execution) | `build-provider-execution`, `build-execution-protocol.json` |
| Test execution | `test.source: "provider"`; `arguments` must be `[]`, and `hostIntegration` reports are refused | `lithe/build/test` | [Provider test execution](../../contract/v1/README.md#provider-test-execution) | `build-provider-tests`, `build-test-protocol.json` |
| Test discovery | A provider test declares `discovery: true`, which needs both `suite` and `case` filters | `lithe/build/discoverTests` | [Test discovery](../../contract/v1/README.md#test-discovery) | `build-test-discovery.json` |
| Run preparation | `run[].source: "provider"` with `fromTargets.tags`; `execution` is `application` (default), `service`, or `task` | `lithe/build/prepareRun` | [Provider run preparation](../../contract/v1/README.md#provider-run-preparation) | `build-provider-run-preparation`, `build-run-preparation.json` |
| Typed run options | Service and task preparation carry `launchOptions` (`execution`, `runtimeArguments`, `arguments`, `environment`, optional `entrypoint` and `workingDirectory`) | Extends `lithe/build/prepareRun` | [Typed run options and service/task preparation](../../contract/v1/README.md#typed-run-options-and-servicetask-preparation) | `build-provider-run-options`, `build-run-options.json` |
| Registry connection debugging | A service run declares `debug: {"configurationType": "…"}`, and the host picks an enabled debug adapter that advertises the type and supports `attach` | The same `lithe/build/prepareRun` | [Registry connection debugging](../../contract/v1/README.md#registry-connection-debugging) | `build-connection-debug-options.json` (no manifest example of its own) |
| Native debug preparation | A run declares `debug: {"adapter": "native"}`; `requires` needs `debugAdapters: 0`, the plugin requests `debug.launch`, and the adapter must be one the plugin declares itself | `lithe/build/prepareDebug` | [Native debug preparation](../../contract/v1/README.md#native-debug-preparation) | `build-provider-debug-preparation`, `build-debug-preparation.json` |
| Read-only model inputs | A consumer declares `modelInputs` (at most 8 slots), and `requires` needs `buildModelInputs: 0` and `buildProviders: 0`; a producer declares `modelSharing` | The model, dependency, and plan requests; the snapshot is in `params.modelInputs` | [Optional read-only model inputs](../../contract/v1/README.md#optional-read-only-model-inputs) | `build-systems/build-model-producer`, `build-systems/build-model-consumer` |
| Source-backed native configuration | `configuration: {"source": "provider"}`, own-process providers only; the provider advertises the server feature `litheBuildConfiguration: true` | `lithe/build/configuration`, `lithe/build/editConfiguration` | [Optional native configuration](../../contract/v1/README.md#optional-native-configuration) | `build-provider-configuration`, `build-configuration.json` |
| Inline test reports | `results: {"decoder": "provider"}`; replies carry `testResults` | The same `lithe/build/test` and `execute` | [Inline provider test reports](../../contract/v1/README.md#inline-provider-test-reports) | `build-provider-reports`, `build-test-durations.json` (summing repeated case durations) |
| Opaque semantic test selectors | A provider test declares `selectorKinds` (at most 16 namespaces); requests carry `selectorKind` and `selectors` | The same `lithe/build/test` | [Opaque semantic test selectors](../../contract/v1/README.md#opaque-semantic-test-selectors) | `build-provider-selectors` |
| Declarative project preparation | `preparation: {purpose, sourceRoles}` on a project task; it belongs to `buildSystems`, and a provider task runs through `lithe/build/execute` | The same `lithe/build/execute` | [Declarative project preparation](../../contract/v1/README.md#declarative-project-preparation) | `build-provider-preparation` |

- A provider's results must follow the formats in [Model and outputs](../../contract/v1/README.md#model-and-outputs); the SDKs stamp `format` and `version` on models, dependencies, and test discovery, and a handler that returns another format or version fails.
- A provider rejects selectors, filters, and stale project facts it cannot honor exactly instead of widening the scope to guess; a nonzero exit code from a task or test is a completed tool failure, not a protocol error, a malformed reply is `plugin.build.invalidResult`, and a JSON-RPC error is `plugin.build.failed`.
- A provider never falls back to a host command or a plugin-private function; when the protocol was not negotiated, build methods are refused before they reach a handler.

### Negotiation and isolation

- The host sends `capabilities.experimental.litheBuild: {"versions": [0]}`, and the provider answers the `{"version": 0}` it chose. A provider that answers no version, or one the host did not offer, is not used for build requests; a shared provider still serves its language. Both sides ignore fields they do not know in these objects and in request params and results, so either side can add optional data; only a change the other side must not ignore gives `buildProviders` version `1`, and the protocol moves with it.
- One process per plugin, build system, and workspace. Two plugins never share a process, and the host never relays messages between processes; each speaks JSON-RPC only with the host, over its own stdio. The shared form reuses a language server session of the same plugin only.
- A provider starts on the first build request, which only a user action sends, and stops when its workspace closes, its plugin is disabled, or before its package is replaced, rolled back, or uninstalled. A change of the plugin's settings restarts it on the next request, so its launch arguments always reflect the current values; a provider that exits is started again by the next request.
- Installation refuses a package whose provider launch points at a missing file, a path outside the package or through a symbolic link, or a packaged executable or runtime that is not a regular executable file.
- Its writable state lives in the plugin's state directory for that build system, never in the plugin package or the host's installation.
- Every request carries `buildSystemID`, `root`, `options`, `optionArguments`, and `environment`; `environment` is the selected runtime or tool environment merged with the option contributions (options win), the host does not forward its complete environment, and the provider uses it for every invocation and cache identity. `refresh: true` means the user asked to reload, and the provider must recompute instead of answering from a cache of its own. The optional `tool: {executable}` is the absolute path of the build tool the host resolved from the build system's `invocation`, and the provider looks the tool up itself when it is absent; an explicitly selected toolchain or path that cannot resolve fails before the request is sent.

### Limits

| Item | Limit |
| --- | --- |
| Progress `message` | 4 KiB UTF-8 |
| Progress `output` | 64 KiB UTF-8 per notification |
| Final `output` of a task or test | 32 MiB UTF-8; `exitCode` is a signed 32-bit integer |
| Launch result | At most 1,024 arguments of 64 KiB each; at most 64 environment entries (keys `[A-Za-z_][A-Za-z0-9_]*` of at most 128 bytes, values of at most 64 KiB); the whole JSON at most 32 MiB |
| Test discovery | At most 10,000 items and 32 MiB of UTF-8 JSON |
| Inline test report | At most 10,000 cases and 8 MiB of UTF-8 JSON |
| Semantic selectors | At most 16 `selectorKinds` (120 bytes each); at most 64 selectors per request, 4096 bytes each |
| Native configuration | At most 8 sources, each with at most 64 fields |
| Model inputs | At most 8 slots, 64 instances per source, 2,000 projects and 10,000 targets or content roots per instance, and 8 MiB of resolved JSON |

### Contract sections

| Section | Content |
| --- | --- |
| [Build providers](../../contract/v1/README.md#build-providers) | The two forms, request params, `initialize` negotiation, process isolation and lifecycle |
| [Provider task execution](../../contract/v1/README.md#provider-task-execution) | Task execution, progress, cancellation |
| [Provider test execution](../../contract/v1/README.md#provider-test-execution), [Test discovery](../../contract/v1/README.md#test-discovery) | Test execution, reports, discovery |
| [Provider run preparation](../../contract/v1/README.md#provider-run-preparation) | Run preparation and the portable launch plan |
| [Typed run options and service/task preparation](../../contract/v1/README.md#typed-run-options-and-servicetask-preparation) | `launchOptions`, service and task preparation |
| [Registry connection debugging](../../contract/v1/README.md#registry-connection-debugging) | Connection debugging through an attach adapter chosen from the registry |
| [Native debug preparation](../../contract/v1/README.md#native-debug-preparation) | Native debug preparation and adapter bindings |
| [Optional native configuration](../../contract/v1/README.md#optional-native-configuration) | Reading and staged editing of native configuration |
| [Optional read-only model inputs](../../contract/v1/README.md#optional-read-only-model-inputs) | Read-only model snapshots between a producer and a consumer |
| [Inline provider test reports](../../contract/v1/README.md#inline-provider-test-reports) | `litheTestResults` reports in replies |
| [Opaque semantic test selectors](../../contract/v1/README.md#opaque-semantic-test-selectors) | Semantic test selectors |
| [Declarative project preparation](../../contract/v1/README.md#declarative-project-preparation) | Project preparation tasks that run before a language launch |
| [Model and outputs](../../contract/v1/README.md#model-and-outputs) | The model, diagnostics, and dependency formats a provider result must follow |

### Document formats

Every format has version `0`; see [document format versions](../capabilities-and-versions.md#document-format-versions).

| Format | Used by | Example |
| --- | --- | --- |
| `litheBuildModel` | Model results | [`build-plans.json`](../../contract/v1/fixtures/build-systems/build-plans.json) |
| `litheDependencies` | Dependency results | In the contract, see [Model and outputs](../../contract/v1/README.md#model-and-outputs) |
| `litheDiagnostics` | Diagnostics in output lines | In the contract, see [Model and outputs](../../contract/v1/README.md#model-and-outputs) |
| `litheTestResults` | Test reports and inline reports | In the contract, see [Inline provider test reports](../../contract/v1/README.md#inline-provider-test-reports); the rule for summing repeated case durations is in [`build-test-durations.json`](../../contract/v1/fixtures/build-providers/build-test-durations.json) |
| `litheTestDiscovery` | Test discovery | [`build-test-discovery.json`](../../contract/v1/fixtures/build-providers/build-test-discovery.json) |
| `litheBuildConfiguration` | Native configuration facts | [`build-configuration.json`](../../contract/v1/fixtures/build-providers/build-configuration.json) |
| `litheConfigurationEdit` | The receipt of a native configuration edit | In the contract, see [Optional native configuration](../../contract/v1/README.md#optional-native-configuration) |
| `modelInputs` snapshot | Read-only model inputs | [`build-model-consumer.plugin.json`](../../contract/v1/fixtures/build-systems/build-model-consumer.plugin.json) |
| `launchOptions` object | Run options of services, tasks, and connection debugging; the fixture files wrap it as `format: "litheBuildRunOptions"`, `version: 0` | [`build-run-options.json`](../../contract/v1/fixtures/build-providers/build-run-options.json), [`build-connection-debug-options.json`](../../contract/v1/fixtures/build-providers/build-connection-debug-options.json) |

### Related guides

- [Native build, test, and run preparation](../../guides/native-build-workflows.md)
- [Build, run, and test for language plugins](../../guides/build-tool-integration.md)
- [Declarative build systems](build-systems.md)
