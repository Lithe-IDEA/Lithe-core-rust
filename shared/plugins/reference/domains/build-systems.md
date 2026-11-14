# 声明式构建系统 / Declarative build systems

[中文](#中文) · [English](#english)

## 中文

`buildSystems` 用数据描述一个项目级构建工具（Cargo、Go、Gradle、Maven 等）：怎么识别项目根目录、找哪个可执行文件、怎么得到项目模型、有哪些任务和运行入口、哪些文件变化要重新同步。所有构建工具共用同一套模型和命令：Core 校验声明、检测实例、解码模型、展开每一条命令行，宿主只执行 Core 规划好的进程并把输出交回。所以社区插件加一个构建工具不需要改宿主，宿主代码也不按构建系统 ID 分支。模型需要程序计算时，再加一个[构建提供者](build-providers.md)。

> **宿主支持。** Lithe 桌面端声明 `buildSystems` 和 `buildProviders`（版本都是 `0`）：构建窗口发现构建实例、加载提供者模型、解析 Core 返回的有序可执行文件候选，并运行命令任务和提供者任务。桌面端不声明 `buildModelInputs`、`executionProviders` 和 `toolchainProviders`：`requires` 里有其中任何一个的插件不会被提供安装，所以 `modelSharing` 和 `modelInputs` 在桌面端用不上。
> 构建系统的实例级选项（`scope: "instance"`）和项目任务的 `preparation`，桌面端尚未呈现：插件照常安装，这部分没有界面，也不会被调用。构建提供者的运行准备、测试、调试准备、模型输入、原生配置、内联测试报告和运行时调试同样尚未呈现，见[构建提供者](build-providers.md)。
> 宿主只呈现它实现的功能，插件不得依赖宿主一定会调用其中任何一个。该注释还写到系统命令的工具发现目前只读 `PATH`，依赖项目工具链或用户覆盖的计划会被拒绝，它是否也约束可执行文件候选里的 `setting` 和 `toolchain` 来源，注释没有说明；依赖这两种来源前，先在桌面端实际试一遍。说明见 [`pluginhost.go`](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go)；各能力的对照见[能力与协议版本对照](../capabilities-and-versions.md#能力版本requires)。
> 对契约有疑问，请到 [Lithe-core-rust 的 Issues](https://github.com/Lithe-IDEA/Lithe-core-rust/issues) 反馈。

| 项目 | 内容 |
| --- | --- |
| 清单字段 | `buildSystems[]`：`id`、`displayName`、`icon`、`detection`、`invocation`、`options`、`model`、`provider`、`tasks`、`run`、`test`、`diagnostics`、`dependencies`、`sync`、`layout`、`hostIntegration`，以及可选的 `modelSharing`、`modelInputs`（见下文）和 `configuration`（原生配置，见[构建提供者](build-providers.md)） |
| 能力版本 | `buildSystems` `0`。不带 `provider` 的构建系统只需要它；带 `provider` 还要 `buildProviders: 0`；`modelSharing`、`modelInputs` 另需 `buildModelInputs: 0`（消费者还需要 `buildProviders: 0`）；可执行文件候选用 `setting` 来源时还要 `settings: 0`（见 `build-tool-path`）。可执行文件解析本身是基础契约，不是能力，不需要在 `requires` 里声明。`requires` 必须恰好列出清单用到的能力，见[版本对照](../capabilities-and-versions.md#能力版本requires) |
| 协议 | 没有进程协议；`model.source: "provider"` 时走 [`litheBuild`](build-providers.md) |
| API 编号 | [API-29](../api-catalog.md#现在能用的-api)：构建、任务、测试、运行准备 |
| 权限 | `process.build` |
| 激活事件 | `onBuildSystem:<id>` |
| 宿主支持 | Lithe 桌面端：见上面的说明 |

### 一份声明由什么组成

构建系统、任务和运行入口的 ID 以小写字母开头，只用小写字母、数字和连字符；选项 ID 用 camelCase，和设置 ID 一样。声明里的路径都是相对路径，用 `/`，不能越出各自的基准目录（否则是 `plugin.path.invalid`）。完整字段表见[契约](../../contract/v1/README.md#build-systems)。

| 字段 | 决定什么 |
| --- | --- |
| `detection` | 用 `markers` 里的文件名识别候选根目录。Core 最多向下找 6 层，跳过已知的缓存和构建输出目录，以及被 Git 忽略的目录（规则来自工作区内途经的 `.gitignore`，以及根目录在工作区内的仓库的 `info/exclude`；不读工作区根以上的目录、`core.excludesFile` 和索引，所以被忽略目录下即使有已跟踪的清单文件也不算候选）。检测还会跳过工作区的忽略路径：这是用户在构建选项里编辑的 `.gitignore` 规则，存放在团队或本机的构建选项中，默认是所有名字以 `.` 开头的目录（如 `.claude/worktrees`），用户可以再添加，也可以用 `!` 放回某个目录；`nesting` 决定深层候选并入外层实例（`outermost`，如 Gradle、Maven）还是各自独立（`separate`，默认，如 Cargo）；`priority` 决定多个插件声明同一个 ID 时谁优先 |
| `invocation` | 可执行文件候选（`executables`，按偏好排序）、`globalArguments`、版本探测和 `minimumVersion` |
| `runtimeDependencies` | 构建工具依赖的运行时。`node`、`go` 由宿主管理；其他运行时要么命名同 ID 的[工具定义](toolchains.md)，要么自己声明 `executables` 和版本探测 |
| `options` | 用户选择的构建维度：`boolean`、`singleSelect`、`multiSelect`、`text`、`file`、`directory`；每个选项在有值时贡献 `arguments` 和 `environment` |
| `model` | 怎么得到项目模型，见下表 |
| `provider` | 没有命令能打印模型时，由[构建提供者](build-providers.md)回答模型和依赖请求 |
| `tasks`、`run`、`test` | 任务目录、从模型目标或任务生成的运行入口、测试的范围、过滤器和结果文件 |
| `diagnostics`、`dependencies` | 诊断的行匹配或 `litheDiagnostics`；依赖列表或依赖树的来源 |
| `sync`、`layout` | 哪些文件变化会让模型过期；每个项目要作为 `excluded` 内容根加入的目录 |

### 模型来源

| `model.source` | 怎么得到模型 | 示例 |
| --- | --- | --- |
| `static` | 读取项目里的 `toml` 文件，检测时完成，不启动进程 | `build-static-file`（Cargo） |
| `command`（标准输出） | 运行命令，解码标准输出（`jsonStream` 或规范格式） | `build-command-stdout`（Go） |
| `command`（输出文件） | 运行命令，解码它写到 `${outputFile}` 的文件 | `build-command-output-file`（Gradle） |
| `provider` | 由插件的构建提供者回答 `lithe/build/model` | [构建提供者](build-providers.md) |
| `hostIntegration` | 保留字段：任何 ID 都返回 `plugin.integration.unsupported`。构建系统用声明式契约或规范格式 | — |

`command` 来源要声明 `executesProjectCode` 和 `mayUseNetwork`。命令只在宿主按请求启动时运行，静态文件在检测时读取。`map` 只负责选取值，不是转换语言：map 表达不了的（其他文件格式、从工具读出的任务、显式的父子关系、计算出的标签、依赖树）由插件自己的命令或构建提供者输出规范格式 `litheBuildModel`、`litheDependencies`、`litheTestResults` 或 `litheDiagnostics`（`version` 都是 `0`）。不符合解码器的输出是 `plugin.build.invalidOutput`。

构建系统的 `hostIntegration` 和语言服务器的 `hostIntegration` 都是保留字段：Core 不实现任何宿主集成，任何 ID 都返回 `plugin.integration.unsupported`，见[语言服务器的宿主集成](../../contract/v1/README.md#language-server-host-integrations)。

### 选项、可执行文件与准备任务

- **选项范围。** 选项可以声明 `scope: "instance"`，按检测到的每个根目录分别保存：改动或重置 `a` 不会影响 `b`。省略范围（或 `"workspace"`）使用工作区和构建系统存储。显式的 `false`、`""`、`[]` 仍然是用户保存的值。实例值不能覆盖工作区选项，工作区值也不能悄悄覆盖实例选项。示例：`build-instance-options`。
- **可执行文件解析。** `invocation.executables` 按偏好顺序列出来源：`projectFile`（项目里的包装脚本，总是运行项目代码）、`environment`、`command`（`PATH` 上的命令名）、`runtime`、`setting`（引用声明为 `file` 的全局设置）和 `toolchain`（工具定义 ID）。可以用 `id`、`priority`、`insertBefore`、`insertAfter` 调整同一个候选集内的顺序。用户配置的路径失败是终止性的，不会回退到后面的候选或提供者自己的查找。示例：`build-tool-path`（`setting`）、`build-toolchains`（`toolchain`）。规则见[可执行文件解析](../../contract/v1/README.md#executable-resolution)。
- **展示。** 每个启用的构建系统有自己的工具窗口，由宿主按模型绘制；`options[].presentation` 把布尔选项放到工具栏（`toolbar`，需要 `icon`）或把多选选项放成树顶部的清单（`tree`），`tasks[].category` 给任务归类。这些字段不改变任何计划。
- **项目准备任务。** 项目任务可以声明 `preparation: { purpose, sourceRoles }`，按用途（如 `run`）和内容根角色绑定运行前要执行的任务；宿主从当前模型选出最深的所属项目和内容根，要求恰好一个匹配的声明，不猜任务名，也不回退到约定的任务 ID。示例：`build-providers/build-provider-preparation`。
- **只读模型输入。** 生产者用 `modelSharing: { options: [...] }` 公开规范化的项目、目标和内容根（以及明确列出的普通选项的有效值），消费者用 `modelInputs: [{ pluginID, buildSystemID }]` 读取，最多 8 个槽位。缺失、被禁用或不公开的生产者只会让该槽位 `unavailable`，从不被自动安装或激活；一个生产者不能同时是消费者。示例：`build-model-producer`、`build-model-consumer`。

### 契约章节

| 章节 | 内容 |
| --- | --- |
| [Build systems](../../contract/v1/README.md#build-systems) | 字段表：识别、可执行文件、运行时依赖、选项、模型、任务、运行、测试、诊断、依赖、同步、布局，以及实例级选项 |
| [Presentation](../../contract/v1/README.md#presentation) | 宿主绘制的构建窗口，插件不需要自带界面 |
| [Templates](../../contract/v1/README.md#templates) | 命令行模板变量 |
| [Model and outputs](../../contract/v1/README.md#model-and-outputs) | 统一模型、诊断、测试结果和依赖的格式 |
| [Optional read-only model inputs](../../contract/v1/README.md#optional-read-only-model-inputs) | 插件之间共享只读模型 |
| [Declarative project preparation](../../contract/v1/README.md#declarative-project-preparation) | 运行前准备任务 |
| [Executable resolution](../../contract/v1/README.md#executable-resolution) | 可执行文件候选、排序、严格选择 |
| [Host integrations](../../contract/v1/README.md#host-integrations) | 构建系统的宿主集成（保留字段，Core 不实现） |
| [Commands](../../contract/v1/README.md#commands) | `build.*` Core 命令：检测、规划、解码、同步指纹 |

### 示例与用例

示例以 Cargo、Go、Gradle 为真实输入，但不是那些插件本身。

| 文件 | 内容 |
| --- | --- |
| [`build-systems/build-static-file.plugin.json`](../../contract/v1/fixtures/build-systems/build-static-file.plugin.json) | `static` 模型（Cargo） |
| [`build-systems/build-command-stdout.plugin.json`](../../contract/v1/fixtures/build-systems/build-command-stdout.plugin.json) | 命令标准输出模型（Go） |
| [`build-systems/build-command-output-file.plugin.json`](../../contract/v1/fixtures/build-systems/build-command-output-file.plugin.json) | 命令输出文件模型（Gradle） |
| [`build-systems/build-instance-options.plugin.json`](../../contract/v1/fixtures/build-systems/build-instance-options.plugin.json) | 按实例保存的选项 |
| [`build-systems/build-tool-path.plugin.json`](../../contract/v1/fixtures/build-systems/build-tool-path.plugin.json) | 用 `file` 设置选择构建工具 |
| [`build-systems/build-toolchains.plugin.json`](../../contract/v1/fixtures/build-systems/build-toolchains.plugin.json) | 可执行文件候选和运行时依赖用 `toolchain` 来源 |
| [`build-systems/build-model-producer.plugin.json`](../../contract/v1/fixtures/build-systems/build-model-producer.plugin.json) | 公开模型的生产者 |
| [`build-systems/build-model-consumer.plugin.json`](../../contract/v1/fixtures/build-systems/build-model-consumer.plugin.json) | 读取模型的消费者（带构建提供者） |
| [`build-systems/build-plans.json`](../../contract/v1/fixtures/build-systems/build-plans.json) | `build.*` 标准答案：每个宿主从上面的声明得到的计划和解码结果 |
| [`build-providers/build-provider-preparation.plugin.json`](../../contract/v1/fixtures/build-providers/build-provider-preparation.plugin.json) | 运行前准备任务 |

构建相关的拒绝用例在 [`manifest/invalid-manifests.json`](../../contract/v1/fixtures/manifest/invalid-manifests.json)。

### 相关指南

- [为语言插件接入构建、运行与测试](../../guides/build-tool-integration.md)
- [原生构建、测试与运行准备](../../guides/native-build-workflows.md)

---

## English

`buildSystems` describes a project-level build tool (Cargo, Go, Gradle, Maven, …) as data: how to recognize a project root, which executable to use, how to get the project model, which tasks and run entries exist, and which file changes require a re-sync. Every build tool shares the same model and commands: Core validates the declaration, detects instances, decodes the model, and expands every command line; the host only runs the processes Core planned and passes their output back. A community plugin therefore adds a build tool without any host change, and no host code branches on a build-system ID. When the model must be computed by a program, add a [build provider](build-providers.md).

> **Host support.** The Lithe desktop declares `buildSystems` and `buildProviders` (both version `0`): the Build window detects build instances, loads provider models, resolves the ordered executable candidates Core returns, and runs command and provider tasks. The desktop does not declare `buildModelInputs`, `executionProviders` or `toolchainProviders`: a plugin whose `requires` lists any of them is not offered for installation, so `modelSharing` and `modelInputs` are unusable there.
> Build-system instance options (`scope: "instance"`) and project-task `preparation` are not presented on the desktop yet: the plugin installs, and that part has no surface and is never called. Build-provider run preparation, tests, debug preparation, model inputs, native configuration, inline test reports and runtime debug are not presented either; see [Build providers](build-providers.md).
> A host presents the features it implements; a plugin must not depend on a host calling any of them. The same comment says tool discovery for system commands reads `PATH` only and that a plan depending on a project toolchain or a user override is refused. It does not say whether that also limits the `setting` and `toolchain` sources of executable candidates, so try those two on the desktop before relying on them. See [`pluginhost.go`](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go); the per-capability comparison is in [Capabilities and protocol versions](../capabilities-and-versions.md#capability-versions-requires).
> Questions about the contract go to [Lithe-core-rust Issues](https://github.com/Lithe-IDEA/Lithe-core-rust/issues).

| Item | Value |
| --- | --- |
| Manifest fields | `buildSystems[]`: `id`, `displayName`, `icon`, `detection`, `invocation`, `options`, `model`, `provider`, `tasks`, `run`, `test`, `diagnostics`, `dependencies`, `sync`, `layout`, `hostIntegration`, plus the optional `modelSharing`, `modelInputs` (below) and `configuration` (native configuration, see [Build providers](build-providers.md)) |
| Capability versions | `buildSystems` `0`. A build system without `provider` needs only it; with `provider` it also needs `buildProviders: 0`; `modelSharing` and `modelInputs` also need `buildModelInputs: 0` (a consumer also needs `buildProviders: 0`); a `setting` executable source also needs `settings: 0` (see `build-tool-path`). Executable resolution itself is base contract, not a capability, and is not declared in `requires`. `requires` must list exactly the capabilities the manifest uses; see [versions](../capabilities-and-versions.md#capability-versions-requires) |
| Protocols | No process protocol; `model.source: "provider"` uses [`litheBuild`](build-providers.md) |
| API IDs | [API-29](../api-catalog.md#apis-usable-now): build, tasks, tests, run preparation |
| Permissions | `process.build` |
| Activation events | `onBuildSystem:<id>` |
| Host support | The Lithe desktop: see the note above |

### What a declaration contains

IDs of build systems, tasks, and run entries start with a lowercase letter and use lowercase letters, digits, and hyphens; option IDs are camelCase like setting IDs. Paths in a declaration are relative, use `/`, and stay inside their base directory (`plugin.path.invalid` otherwise). The full field table is in the [contract](../../contract/v1/README.md#build-systems).

| Field | What it decides |
| --- | --- |
| `detection` | Candidate roots are directories holding one of the `markers` file names. Core searches at most 6 levels down and skips known caches and build outputs, plus directories Git ignores (rules from the `.gitignore` files inside the workspace and the `info/exclude` of each repository rooted there; directories above the workspace root, `core.excludesFile` and the index are not read, so a tracked manifest below an ignored directory is no candidate). Detection also skips the workspace's ignored paths, `.gitignore` patterns the user edits in the team or machine-local Build options: by default every directory whose name starts with a dot, such as `.claude/worktrees`, and the user can add more or bring one back with `!`; `nesting` folds deeper candidates into the outer instance (`outermost`, such as Gradle and Maven) or keeps each one (`separate`, the default, such as Cargo); `priority` decides between plugins declaring the same ID |
| `invocation` | Executable candidates (`executables`, in preference order), `globalArguments`, the version probe, and `minimumVersion` |
| `runtimeDependencies` | Runtimes the build tool needs. `node` and `go` are host-managed; any other runtime either names the [tool definition](toolchains.md) with the same ID or declares its own `executables` and version probe |
| `options` | Build dimensions the user chooses: `boolean`, `singleSelect`, `multiSelect`, `text`, `file`, `directory`; each contributes `arguments` and `environment` when it has a value |
| `model` | How the project model is obtained; see the table below |
| `provider` | The [build provider](build-providers.md) that answers model and dependency requests when no command prints them |
| `tasks`, `run`, `test` | The task catalog, run entries generated from model targets or tasks, and the scopes, filters, and result files of tests |
| `diagnostics`, `dependencies` | Line matchers or `litheDiagnostics` for diagnostics; the source of the dependency list or tree |
| `sync`, `layout` | Which file changes make the model stale; directories added to every project as `excluded` content roots |

### Model sources

| `model.source` | How the model is obtained | Example |
| --- | --- | --- |
| `static` | Read a `toml` file in the project, at detection time, with no process | `build-static-file` (Cargo) |
| `command` (stdout) | Run a command and decode its standard output (`jsonStream` or a canonical format) | `build-command-stdout` (Go) |
| `command` (output file) | Run a command and decode the file it writes to `${outputFile}` | `build-command-output-file` (Gradle) |
| `provider` | The plugin's build provider answers `lithe/build/model` | [Build providers](build-providers.md) |
| `hostIntegration` | Reserved: every ID returns `plugin.integration.unsupported`. Build systems use the declarative contract or a canonical format | — |

A `command` source declares `executesProjectCode` and `mayUseNetwork`. A command runs only when the host starts it on request; a static file is read during detection. A `map` only selects values and is not a transformation language: whatever a map cannot express (other file formats, tasks read from the tool, explicit parent links, computed tags, dependency trees) is the plugin's own command or build provider printing the canonical `litheBuildModel`, `litheDependencies`, `litheTestResults`, or `litheDiagnostics` (all `version` `0`). Output that does not fit its decoder is `plugin.build.invalidOutput`.

A build system's `hostIntegration` and a language server's `hostIntegration` are both reserved: Core implements no host integration, and every ID returns `plugin.integration.unsupported`; see [language server host integrations](../../contract/v1/README.md#language-server-host-integrations).

### Options, executables, and preparation tasks

- **Option scope.** An option may declare `scope: "instance"`, stored separately per detected root: changing or resetting `a` cannot change `b`. Omitting the scope (or `"workspace"`) uses the workspace and build-system storage. Explicit `false`, `""`, and `[]` remain stored user values. Instance values cannot override workspace options, and workspace values cannot silently override an instance option. Example: `build-instance-options`.
- **Executable resolution.** `invocation.executables` lists sources in preference order: `projectFile` (a wrapper inside the project; always runs project code), `environment`, `command` (a bare name on `PATH`), `runtime`, `setting` (references a declared global `file` setting), and `toolchain` (a tool-definition ID). `id`, `priority`, `insertBefore`, and `insertAfter` reorder candidates within one candidate set. A configured path that fails is terminal and never falls back to later candidates or to the provider's own lookup. Examples: `build-tool-path` (`setting`) and `build-toolchains` (`toolchain`). The rules are in [Executable resolution](../../contract/v1/README.md#executable-resolution).
- **Presentation.** Every enabled build system gets a tool window of its own, drawn by the host from the model; `options[].presentation` puts a boolean option on the toolbar (`toolbar`, needs an `icon`) or a multiSelect option as a checklist at the top of the tree (`tree`), and `tasks[].category` files a task under headings. None of these fields changes a plan.
- **Project preparation tasks.** A project task may declare `preparation: { purpose, sourceRoles }`, binding the task to run first by purpose (such as `run`) and content-root role. The host selects the deepest owning project and content root from the current model, requires exactly one matching declaration, never guesses a task name, and never falls back to a conventional task ID. Example: `build-providers/build-provider-preparation`.
- **Read-only model inputs.** A producer publishes normalized projects, targets, and content roots (and the effective values of explicitly listed ordinary options) with `modelSharing: { options: [...] }`; a consumer reads them with `modelInputs: [{ pluginID, buildSystemID }]`, up to 8 slots. A missing, disabled, or non-public producer only makes that slot `unavailable` and is never installed or activated automatically; a producer cannot also be a consumer. Examples: `build-model-producer` and `build-model-consumer`.

### Contract sections

| Section | Content |
| --- | --- |
| [Build systems](../../contract/v1/README.md#build-systems) | The field table: detection, executables, runtime dependencies, options, model, tasks, run, tests, diagnostics, dependencies, sync, layout, and instance options |
| [Presentation](../../contract/v1/README.md#presentation) | The host-drawn build window; the plugin brings no UI |
| [Templates](../../contract/v1/README.md#templates) | Command-line template variables |
| [Model and outputs](../../contract/v1/README.md#model-and-outputs) | The shared model, diagnostics, test results, and dependency formats |
| [Optional read-only model inputs](../../contract/v1/README.md#optional-read-only-model-inputs) | Sharing read-only models between plugins |
| [Declarative project preparation](../../contract/v1/README.md#declarative-project-preparation) | Tasks that run before a launch |
| [Executable resolution](../../contract/v1/README.md#executable-resolution) | Executable candidates, ordering, strict selection |
| [Host integrations](../../contract/v1/README.md#host-integrations) | Host integrations for build systems (reserved; Core implements none) |
| [Commands](../../contract/v1/README.md#commands) | `build.*` Core commands: detection, plans, decoding, sync fingerprints |

### Examples and cases

The examples use Cargo, Go, and Gradle as realistic inputs; they are not those plugins.

| File | Content |
| --- | --- |
| [`build-systems/build-static-file.plugin.json`](../../contract/v1/fixtures/build-systems/build-static-file.plugin.json) | `static` model (Cargo) |
| [`build-systems/build-command-stdout.plugin.json`](../../contract/v1/fixtures/build-systems/build-command-stdout.plugin.json) | Command stdout model (Go) |
| [`build-systems/build-command-output-file.plugin.json`](../../contract/v1/fixtures/build-systems/build-command-output-file.plugin.json) | Command output-file model (Gradle) |
| [`build-systems/build-instance-options.plugin.json`](../../contract/v1/fixtures/build-systems/build-instance-options.plugin.json) | Per-instance options |
| [`build-systems/build-tool-path.plugin.json`](../../contract/v1/fixtures/build-systems/build-tool-path.plugin.json) | A `file` setting selects the build tool |
| [`build-systems/build-toolchains.plugin.json`](../../contract/v1/fixtures/build-systems/build-toolchains.plugin.json) | Executable candidates and a runtime dependency with `toolchain` sources |
| [`build-systems/build-model-producer.plugin.json`](../../contract/v1/fixtures/build-systems/build-model-producer.plugin.json) | A model producer |
| [`build-systems/build-model-consumer.plugin.json`](../../contract/v1/fixtures/build-systems/build-model-consumer.plugin.json) | A model consumer (with a build provider) |
| [`build-systems/build-plans.json`](../../contract/v1/fixtures/build-systems/build-plans.json) | Golden `build.*` results: the plans and decoded outputs every host gets from the declarations above |
| [`build-providers/build-provider-preparation.plugin.json`](../../contract/v1/fixtures/build-providers/build-provider-preparation.plugin.json) | Tasks that run before a launch |

Build rejections are in [`manifest/invalid-manifests.json`](../../contract/v1/fixtures/manifest/invalid-manifests.json).

### Related guides

- [Build, run, and test for language plugins](../../guides/build-tool-integration.md)
- [Native build, test, and run preparation](../../guides/native-build-workflows.md)
