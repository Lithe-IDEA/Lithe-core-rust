# Lithe 插件开发 / Lithe plugin development

[中文](#中文) · [English](#english)

## 中文

写插件、测试插件、发布插件，都在**你自己的仓库**里完成。不需要克隆 [Lithe](https://github.com/1lck/Lithe-IDEA) 主仓库，也不需要给它提 PR。插件开发需要的资料都在这个目录：契约、指南、参考和官方 SDK 都在 [Lithe Core 仓库](https://github.com/Lithe-IDEA/Lithe-core-rust)的 `shared/plugins/` 下。

| 想做什么 | 看哪里 |
| --- | --- |
| 了解插件长什么样 | [插件是什么](#插件是什么) |
| 动手做第一个插件 | [五步做出第一个插件](#五步做出第一个插件) |
| 跟着教程完整做一遍 | [教程](#教程) |
| 写 `plugin.json` 时有提示和校验 | [在编辑器里校验](#在编辑器里校验-pluginjson) |
| 需要自己的后端进程 | [插件后端 SDK](#插件后端-sdk可选) |
| 接入宿主日志、排查插件故障 | [插件接入统一日志](guides/plugin-logging.md) |
| 参考现成插件 | [示例插件](#示例插件) |
| 查 API-01–44 现在能用什么、每种能力和协议的版本、Core 和 Lithe 桌面端支持哪些 | [插件参考](reference/README.md) |
| 查字段的准确含义 | [插件契约](#插件契约) |
| 修改契约、指南或 SDK，在本地验证 | [贡献与维护](#贡献与维护) |

### 插件是什么

一个 Lithe 插件就是一个目录：`plugin.json` 声明插件提供什么，其余文件是它用到的资源，例如语言服务器、脚本和图标。

```text
io.github.<你的用户名>.zig-support/
├── plugin.json
└── …            # plugin.json 引用的资源
```

Lithe 读取并校验 `plugin.json`，然后自己启动其中声明的外部进程。插件代码不会加载进 Lithe 进程，所以同一个插件包不需要为每个 Lithe 版本重新构建。

社区插件必须遵守这几条：

- `id` 用你自己的反向域名，例如 `io.github.<你的用户名>.zig-support`。`dev.lithe` 和 `dev.lithe.*` 是官方保留命名空间。
- `vendor.signatureRequirement` 写 `"none"`。社区插件不带 Lithe 签名，用户安装前会看到完整的程序、权限和来源，并确认。
- 包内路径只用 `/`，不能有 `..`、绝对路径或符号链接。
- 插件包安装后是只读的。缓存和状态使用 Lithe 通过启动模板传入的 `${stateDir}`、`${workspaceStateDir}`；诊断日志使用 SDK 与宿主统一入口。确需原生文件日志时才使用 `${logDir}`，没有本地日志目录的宿主会拒绝这类启动，详见[插件接入统一日志](guides/plugin-logging.md)。

### 五步做出第一个插件

1. **复制模板。** 以 [Lua 插件](https://github.com/Lithe-IDEA/lithe-plugin-lua) 为起点，它是 Lithe Store 的示例和模板。
2. **改 `plugin.json`。** 换成你自己的 ID、名称、语言和语言服务器，并按[下面](#清单里的版本号)的规则写版本号和 `requires`。先按[后面一节](#在编辑器里校验-pluginjson)配置编辑器，写错字段时会直接提示。
3. **在构建阶段准备好所有文件。** 语言服务器、后端都在打包时放进插件包，运行时不下载任何东西。详见 [LSP 插件完整生命周期教程](guides/lsp-plugin-lifecycle.md)。
4. **在 Lithe 里试装。** 开发时可以用“从磁盘安装插件…”选择 zip 或目录（同样要确认安装审查；Lithe 安装的是一份快照，改完要重新安装）。发布前在自己仓库发一个 Release，并在仓库根目录放 `lithe-plugin-index.json`，然后在“管理插件仓库…”里加入你的仓库地址，走一遍用户的真实安装流程。
5. **发布。** 想让所有 Lithe 用户在插件市场看到，按 [Lithe Store 发布指南](https://github.com/Lithe-IDEA/Lithe-store/blob/main/docs/publishing.md) 收录；收录后新版本会被自动追踪。

需要运行、测试或构建工具集成时，再读 [构建、运行与测试接入指南](guides/build-tool-integration.md)。

#### 清单里的版本号

`plugin.json` 里有四处版本相关的字段，含义各不相同：

| 字段 | 写什么 | 含义 |
| --- | --- | --- |
| `schemaVersion` | `0` | 清单的文件格式 |
| `pluginAPIVersion` | `0` | 插件 API 版本，Lithe 据此判断能不能读你的清单 |
| `requires` | 每个用到的能力写 `0` | 你的插件用了哪些能力；Lithe 只在支持这些能力时才提供安装 |
| `version` | 你自己定 | 你的插件自己的版本 |

`requires` 必须**恰好**列出实际用到的能力：漏一个或多列一个，清单都会被拒绝。例如只用声明式构建系统时写 `"requires": { "buildSystems": 0 }`，不要多写。

能力以后可能出新版本。一次改动只给一个能力加一个新版本，没用到它的插件继续写 `0`，继续可以安装。每个能力的内容、Core 的支持表和 Lithe 桌面端是否声明它，见[能力与协议版本对照](reference/capabilities-and-versions.md#能力版本requires)；Lithe 桌面端没有声明的能力，需要它的插件不会被提供安装。

### 在编辑器里校验 plugin.json

契约里的 JSON Schema 地址固定：

| 文件 | Schema |
| --- | --- |
| `plugin.json` | `https://raw.githubusercontent.com/Lithe-IDEA/Lithe-core-rust/main/shared/plugins/contract/v1/plugin-manifest.schema.json` |
| `lithe-plugin-index.json` | `https://raw.githubusercontent.com/Lithe-IDEA/Lithe-core-rust/main/shared/plugins/contract/v1/plugin-index.schema.json` |
| `l10n/<语言>.json`（如 `l10n/zh-Hans.json`） | `https://raw.githubusercontent.com/Lithe-IDEA/Lithe-core-rust/main/shared/plugins/contract/v1/plugin-l10n.schema.json` |

> **不要在 `plugin.json` 里写 `"$schema"`。** Lithe 会拒绝清单中的未知字段，加了以后插件反而装不上。请在编辑器里按文件名关联 Schema。翻译文件 `l10n/*.json` 可以写 `"$schema"`，Lithe 会忽略它。

VS Code：在插件仓库的 `.vscode/settings.json` 中加入

```json
{
  "json.schemas": [
    {
      "fileMatch": ["**/plugin.json"],
      "url": "https://raw.githubusercontent.com/Lithe-IDEA/Lithe-core-rust/main/shared/plugins/contract/v1/plugin-manifest.schema.json"
    },
    {
      "fileMatch": ["**/lithe-plugin-index.json"],
      "url": "https://raw.githubusercontent.com/Lithe-IDEA/Lithe-core-rust/main/shared/plugins/contract/v1/plugin-index.schema.json"
    },
    {
      "fileMatch": ["**/l10n/*.json"],
      "url": "https://raw.githubusercontent.com/Lithe-IDEA/Lithe-core-rust/main/shared/plugins/contract/v1/plugin-l10n.schema.json"
    }
  ]
}
```

JetBrains IDE：设置 → Languages & Frameworks → Schemas and DTDs → JSON Schema Mappings，按上表添加每个 URL 和它的文件模式，例如 `plugin.json`。

Schema 只检查 JSON 结构。ID 引用、权限、激活事件、模板变量这类规则由 Lithe 在安装时检查，规则写在[插件契约](#插件契约)里。

### 插件后端 SDK（可选）

只用语言服务器、运行和测试的插件不需要后端。需要构建后端（`buildProviders`）、插件函数，或给插件自己的网页配一个后端进程（`backend`）时，用官方 SDK 写：协议、取消、进程清理和错误处理由 SDK 负责，你只写处理函数。

| SDK | 版本 | 运行时 | Release | 怎么放进插件 |
| --- | --- | --- | --- | --- |
| [Node.js](sdk/node/README.md) | 0.5.0 | Node.js 20.3+ | [`sdk-node-v0.5.0`](https://github.com/Lithe-IDEA/Lithe-core-rust/releases/tag/sdk-node-v0.5.0)：`lithe-plugin-sdk-node-0.5.0.tgz` 和 `.sha256` | 解压后把 `package/` 复制进插件包，用相对路径导入 |
| [Java](sdk/java/README.md) | 0.5.0 | Java 8+ | [`sdk-java-v0.5.0`](https://github.com/Lithe-IDEA/Lithe-core-rust/releases/tag/sdk-java-v0.5.0)：`lithe-plugin-sdk-java-0.5.0.jar` 和 `.sha256` | shade 进后端 JAR |
| [Rust](sdk/rust/README.md) | 0.4.0 | Rust 1.87+ | [`sdk-rust-v0.4.0`](https://github.com/Lithe-IDEA/Lithe-core-rust/releases/tag/sdk-rust-v0.4.0)：`lithe-plugin-sdk-rust-0.4.0.crate` 和 `.sha256` | 用 Git 依赖锁定标签（`Cargo.lock` 记录 commit），或解压后作为 `path` 依赖；编译成每个平台各一份的插件后端可执行文件 |

Rust 的 Git 依赖这样写：

```toml
lithe-plugin-sdk = { git = "https://github.com/Lithe-IDEA/Lithe-core-rust", tag = "sdk-rust-v0.4.0" }
```

SDK 的源码都在本目录的 `sdk/` 下，发布在 [Core 仓库的 Releases](https://github.com/Lithe-IDEA/Lithe-core-rust/releases) 里（标签 `sdk-node-v<版本>`、`sdk-java-v<版本>`、`sdk-rust-v<版本>`），不在 npm、Maven 或 crates.io 上。Rust SDK 只实现插件后端（`backend`）：插件函数和宿主服务；构建后端请用 Node.js 或 Java SDK，VCS 后端请用 Node.js SDK。各 SDK 的 README 有 “What the Lithe desktop serves” 一节，说明 Lithe 桌面端提供哪些协议；各协议的版本见[进程协议版本](reference/capabilities-and-versions.md#进程协议版本)。以上三个版本说的是版本 `0` 的协议；已退役的 `lithe-plugin-docs` 仓库发布过更早的 SDK，它们说的是旧协议，Lithe 桌面端不接受用它们写的插件，请只使用 Core 仓库发布的版本。

规则只有一条：**在构建时下载，并按你锁定的 SHA-256（或 Git commit）校验；运行时不下载任何东西。** 插件包安装后没有 `npm install` 步骤，也必须能离线运行。

[Maven 插件](https://github.com/Lithe-IDEA/lithe-plugin-maven) 的 `sdk.lock.json` 是锁定 Java SDK 的现成例子。

### 示例插件

| 插件 | 示范什么 |
| --- | --- |
| [Lua](https://github.com/Lithe-IDEA/lithe-plugin-lua) | **模板**；随包附带语言服务器（LuaLS），运行当前文件 |
| [Go](https://github.com/Lithe-IDEA/lithe-plugin-go) | 使用用户自己安装的 gopls，运行 main 包和 `go test` |
| [PHP](https://github.com/Lithe-IDEA/lithe-plugin-php) | 使用用户自己安装的 Intelephense（许可证不允许随包分发），PHPUnit |
| [Gradle](https://github.com/Lithe-IDEA/lithe-plugin-gradle) | 纯声明式构建系统，不需要后端 |
| [Maven](https://github.com/Lithe-IDEA/lithe-plugin-maven) | 用 Java SDK 写的构建后端 |
| [Cargo](https://github.com/Lithe-IDEA/lithe-plugin-cargo) | 用 Node.js SDK 写的构建后端和插件设置 |
| [LLDB](https://github.com/Lithe-IDEA/lithe-plugin-lldb) | 调试适配器的参考实现；用 Xcode 自带的 `lldb-dap` 调试 C、C++、Swift 和 Rust，含 `executable` 查找和调试控制台补全 |

示例插件在各自的仓库里维护。照着做之前，对照它们的 `plugin.json` 里的 `requires` 和[版本对照](reference/capabilities-and-versions.md#能力版本requires)，确认 Lithe 桌面端声明了它们需要的能力。

### 教程

| 教程 | 内容 |
| --- | --- |
| [LSP 插件完整生命周期](guides/lsp-plugin-lifecycle.md) | 从 Lua 模板开始：清单、服务器打包、激活、状态目录、停止、升级与卸载 |
| [插件接入统一日志](guides/plugin-logging.md) | 三 SDK 的安全失败边界、受管诊断通道，以及 `${logDir}` 的本地会话生命周期 |
| [原生构建工作流](guides/native-build-workflows.md) | Node/Java SDK 的构建执行、精确测试、运行准备，以及用真实 Core 做端到端验证 |
| [构建、运行与测试接入](guides/build-tool-integration.md) | 执行提供者示例、工程后端边界，以及声明式构建系统的现状和已发布示例 |
| [编写调试适配器插件](guides/debug-adapter-plugin.md) | 从最小清单到启动方式、先编译再调试的 `executable`，以及调试控制台补全 |
| [插件后端：工作区上下文与宿主服务](guides/plugin-backend-workspace.md) | 免页面启动、工作区上下文、纯后端命令、文档/文件/窗口宿主方法、宿主通知、失败码，以及宿主服务只增不改时后端要遵守的规则 |
| [插件自有的原生终端](guides/native-terminals.md) | `terminal/*`：创建、显示、关闭、重开和退出通知（本机工作区） |

收录和上架的规则由 Lithe Store 维护：[发布社区插件](https://github.com/Lithe-IDEA/Lithe-store/blob/main/docs/publishing.md)讲怎么打包、提交收录和发布新版本，[版本追踪与更新](https://github.com/Lithe-IDEA/Lithe-store/blob/main/docs/versioning-and-updates.md)讲 Lithe 怎么选择可安装的版本。

### 插件参考

[`reference/`](reference/README.md) 按功能整理契约，中英双语：

| 页面 | 内容 |
| --- | --- |
| [插件 API 总表](reference/api-catalog.md) | API-01–44 每一项现在能用的方法、尚未提供的部分和对应功能页 |
| [能力与协议版本对照](reference/capabilities-and-versions.md) | `requires` 能力版本、进程协议版本、文档格式版本，对照 Core 和 Lithe 桌面端支持哪些 |
| [插件协议更新记录](reference/protocol-changelog.md) | 每个协议、能力和 API 是哪天、哪个 PR 改的，以及每个版本首次出现在哪个 PR；只有中文 |
| [功能页](reference/README.md#功能页) | 15 个功能各一页：字段、宿主现状、契约章节、示例与用例 |

### 插件契约

插件契约规定每个字段的含义、校验规则和错误码，完整内容在本目录的 [`contract/v1/`](contract/v1/README.md)，以英文为准：

| 文件 | 内容 |
| --- | --- |
| [`README.md`](contract/v1/README.md) | 字段、版本规则、能力、权限和错误码的完整说明 |
| [`plugin-manifest.schema.json`](contract/v1/plugin-manifest.schema.json) | `plugin.json` 的 JSON Schema |
| [`plugin-index.schema.json`](contract/v1/plugin-index.schema.json) | `lithe-plugin-index.json` 的 JSON Schema |
| [`native-ui.schema.json`](contract/v1/native-ui.schema.json) | `ui` 能力里宿主渲染的原生内容的 JSON Schema |
| [`plugin-l10n.schema.json`](contract/v1/plugin-l10n.schema.json) | 插件翻译文件 `l10n/<语言>.json` 的 JSON Schema |
| [`toolchains/`](contract/v1/toolchains/README.md) | 插件提供的工具链：工具定义、版本管理器提供方、提供方后端协商（`litheToolchains`）、查询与回复校验（含稳定失败码）、固定值（pin）和四层解析。Core 实现这些命令；Lithe 桌面端声明 `toolDefinitions`，未声明 `toolchainProviders` |
| [`fixtures/`](contract/v1/fixtures) | 标准用例，按功能分目录（`manifest`、`languages`、`backend`、`build-systems`、`build-providers`、`marketplace` 等），和 [reference 功能页](reference/README.md#功能页)一一对应：合法和非法的清单、命令和构建规划、插件函数协议、市场索引，以及 Lithe 对每个用例的预期结果 |

fixture 放在 `fixtures/<功能>/` 下，文件按演示的能力命名，例如 `build-systems/build-command-stdout.plugin.json` 演示“解码命令的标准输出”。它们借 Cargo、Go、Gradle、Maven 等真实工具作为输入，但不是这些插件本身；要参考完整插件，看[示例插件](#示例插件)。每个目录的文件和用例数量见对应功能页的“示例与用例”。

契约由 Lithe 的 Rust Core 统一执行。契约、指南、参考、官方 SDK、SDK 端到端测试和脚本都在 Core 仓库的 `shared/plugins/` 下，和执行契约的 Core 代码（仓库根的 `rust/`）在同一个 PR 里修改，所以每个版本的契约、SDK 和 Core 彼此一致，不存在需要同步的副本。本目录以 Core 仓库 `main` 为准；`main` 上的内容可能还没进入已发布的 Lithe 版本，发布插件前以目标版本实际支持为准。

对契约有疑问或建议，到 [Core 仓库提 issue](https://github.com/Lithe-IDEA/Lithe-core-rust/issues)。

### 贡献与维护

修改契约、指南、参考或 SDK 的 PR 提到 [Core 仓库](https://github.com/Lithe-IDEA/Lithe-core-rust)，和对应的 Rust Core 改动放在同一个 PR 里。下面的命令都在 **Core 仓库的根目录**运行；CI 工作流是仓库根的 `.github/workflows/plugins-verify.yml`，每次涉及 `shared/plugins/` 或 `rust/` 的提交都会运行它们。

| 命令 | 作用 |
| --- | --- |
| `node shared/plugins/scripts/verify-docs.mjs` | 离线检查：每份 Schema 的 `$id` 等于它的发布地址 `https://raw.githubusercontent.com/Lithe-IDEA/Lithe-core-rust/main/shared/plugins/contract/v1/<文件>`、Markdown 相对链接都能解析、没有指向已退役的 `lithe-plugin-docs` 仓库的链接 |
| `node --test shared/plugins/scripts/*.test.mjs` | 脚本自身的测试 |

改契约、能力版本或插件函数协议时，同一个 PR 还要更新[插件协议更新记录](reference/protocol-changelog.md)，以及 [`reference/`](reference/README.md) 里受影响的功能页和版本表：它们是手写的对照，不会自动更新。更新记录的做法见该页的“维护”一节。

#### SDK

| 命令 | 作用 |
| --- | --- |
| `node --test shared/plugins/sdk/node/test/*.test.mjs` | Node.js SDK 的单元测试 |
| `npm run --prefix shared/plugins/sdk/node test:types` | Node.js SDK 的 TypeScript 使用方检查；先运行 `npm ci --prefix shared/plugins/sdk/node --ignore-scripts` 安装开发依赖 |
| `mvn -B -ntp -f shared/plugins/sdk/java/pom.xml verify` | Java SDK 的单元测试；加 `-Djvm=<java>` 可指定运行测试的 Java（CI 用 Java 8） |
| `cargo test --locked --manifest-path shared/plugins/sdk/rust/Cargo.toml` | Rust SDK 的单元测试（CI 用 Rust 1.87，另用 stable 跑 `cargo fmt` 和 `cargo clippy`） |
| `node shared/plugins/scripts/sdk-e2e.mjs` | 端到端测试：先构建三个 SDK 的示例后端，再运行 `cargo test -p lithe-core --features plugin-sdk-e2e` 的 `plugin_sdk_node`、`plugin_sdk_java`、`plugin_sdk_rust` 和 `plugin_sdk_build` 四个测试，对象是**当前工作树的 Core**；总是包含原生构建工作流和快捷键导入场景 |

端到端测试需要 Rust、Node.js 20.3+、Java（`LITHE_PLUGIN_SDK_JAVA_RUNTIME` 指定，或 `PATH` 上的 `java`）和 Maven。场景文件在 [`sdk/e2e/`](sdk/e2e)：Node.js 和 Java 跑同一套构建后端场景，Rust 跑插件后端场景（`plugin_sdk/backend.rs`），测试代替宿主应答凭据请求。`LITHE_PLUGIN_SDK_JAVA_SAMPLE`、`LITHE_PLUGIN_SDK_JAVA_NATIVE_SAMPLE` 和 `LITHE_PLUGIN_SDK_RUST_SAMPLE` 可以指定已构建好的示例，跳过构建。

发版：先改版本号（Node.js 改 `shared/plugins/sdk/node/package.json`；Java 运行 `mvn -f shared/plugins/sdk/java/pom.xml versions:set -DnewVersion=<版本> -DgenerateBackupPoms=false`，三个 pom 一起改；Rust 改 `shared/plugins/sdk/rust/Cargo.toml` 的 `workspace.package.version` 并更新 `Cargo.lock`），合并后推送对应标签 `sdk-node-v<版本>`、`sdk-java-v<版本>` 或 `sdk-rust-v<版本>`。仓库根的发版工作流 `.github/workflows/plugins-release-{node,java,rust}-sdk.yml` 由标签触发，检查标签和版本一致，测试通过后把包和 `.sha256` 发到 Core 仓库的 Releases。发版后更新本页和各 SDK README 里的版本号。

### 许可证

[Apache-2.0](LICENSE)

---

## English

Writing, testing, and publishing a plugin all happen in **your own repository**. You never need to clone the [Lithe](https://github.com/1lck/Lithe-IDEA) repository or send it a pull request. Everything a plugin developer needs is in this directory: the contract, guides, reference, and official SDKs all live under `shared/plugins/` in the [Lithe Core repository](https://github.com/Lithe-IDEA/Lithe-core-rust).

| To | Go to |
| --- | --- |
| See what a plugin looks like | [What a plugin is](#what-a-plugin-is) |
| Build your first plugin | [Your first plugin in five steps](#your-first-plugin-in-five-steps) |
| Follow a complete walkthrough | [Guides](#guides) |
| Get completion and validation while writing `plugin.json` | [Validate in your editor](#validate-pluginjson-in-your-editor) |
| Run a backend process of your own | [Plugin backend SDKs](#plugin-backend-sdks-optional) |
| Connect to host logging and diagnose plugin failures | [Plugin logging](guides/plugin-logging.md) |
| Learn from existing plugins | [Example plugins](#example-plugins) |
| See what API-01–44 offer today, the version of each capability and protocol, and what Core and the Lithe desktop client support | [Plugin reference](reference/README.md) |
| Look up the exact meaning of a field | [Plugin contract](#plugin-contract) |
| Change the contract, guides, or SDKs and verify locally | [Contributing and maintenance](#contributing-and-maintenance) |

### What a plugin is

A Lithe plugin is a directory: `plugin.json` declares what the plugin provides, and the other files are the resources it uses, such as language servers, scripts, and icons.

```text
io.github.<your-user-name>.zig-support/
├── plugin.json
└── …            # resources referenced by plugin.json
```

Lithe reads and validates `plugin.json` and then starts the external processes it declares itself. Plugin code is never loaded into the Lithe process, so the same package does not need rebuilding for each Lithe version.

Community plugins must follow these rules:

- `id` uses your own reverse domain name, such as `io.github.<your-user-name>.zig-support`. `dev.lithe` and `dev.lithe.*` are reserved for official plugins.
- `vendor.signatureRequirement` is `"none"`. Community plugins carry no Lithe signature; before installing, users see and confirm the complete programs, permissions, and source.
- Package paths use only `/`, with no `..`, absolute paths, or symbolic links.
- An installed package is read-only. Caches and state go to `${stateDir}` and `${workspaceStateDir}`, which Lithe passes through launch templates; diagnostic logs go through the shared SDK and host path. Use `${logDir}` only when you really need native file logs; a host without a local log directory rejects such launches. See [Plugin logging](guides/plugin-logging.md).

### Your first plugin in five steps

1. **Copy the template.** Start from the [Lua plugin](https://github.com/Lithe-IDEA/lithe-plugin-lua), Lithe Store's example and template.
2. **Edit `plugin.json`.** Use your own ID, name, language, and language server, and write the version numbers and `requires` by the rules [below](#version-numbers-in-the-manifest). Set up your editor as in the [section after that](#validate-pluginjson-in-your-editor) first, so mistyped fields are flagged as you write.
3. **Prepare every file at build time.** Language servers and backends go into the package when you build it; nothing is downloaded at run time. See the [LSP plugin lifecycle guide](guides/lsp-plugin-lifecycle.md).
4. **Try it in Lithe.** During development, use "Install Plugin from Disk…" with a zip or directory (you still confirm the install review; Lithe installs a snapshot, so reinstall after each change). Before publishing, create a Release in your repository, put `lithe-plugin-index.json` at the repository root, and add your repository under "Manage Plugin Repositories…" to walk through the real user install flow.
5. **Publish.** To list the plugin in every Lithe user's marketplace, follow the [Lithe Store publishing guide](https://github.com/Lithe-IDEA/Lithe-store/blob/main/docs/publishing.md); new versions are tracked automatically once listed.

When you need run, test, or build tool integration, read the [build, run, and test integration guide](guides/build-tool-integration.md).

#### Version numbers in the manifest

`plugin.json` has four version-related fields, each with its own meaning:

| Field | Write | Meaning |
| --- | --- | --- |
| `schemaVersion` | `0` | The manifest file format |
| `pluginAPIVersion` | `0` | The plugin API version; Lithe uses it to decide whether it can read your manifest |
| `requires` | `0` for each capability you use | Which capabilities your plugin uses; Lithe offers the plugin for installation only where it supports them |
| `version` | Your choice | Your plugin's own version |

`requires` must list **exactly** the capabilities you use: leaving one out or listing one extra gets the manifest rejected. For example, a plugin that only uses declarative build systems writes `"requires": { "buildSystems": 0 }` and nothing more.

A capability may get a new version later. One change adds a new version to exactly one capability, and a plugin that does not use it keeps writing `0` and stays installable. For what each capability covers, Core's support table, and whether the Lithe desktop client declares it, see [Capability and protocol versions](reference/capabilities-and-versions.md#capability-versions-requires); a plugin that needs a capability the Lithe desktop client does not declare is not offered for installation there.

### Validate plugin.json in your editor

The contract's JSON Schemas have fixed addresses:

| File | Schema |
| --- | --- |
| `plugin.json` | `https://raw.githubusercontent.com/Lithe-IDEA/Lithe-core-rust/main/shared/plugins/contract/v1/plugin-manifest.schema.json` |
| `lithe-plugin-index.json` | `https://raw.githubusercontent.com/Lithe-IDEA/Lithe-core-rust/main/shared/plugins/contract/v1/plugin-index.schema.json` |
| `l10n/<language>.json` (such as `l10n/zh-Hans.json`) | `https://raw.githubusercontent.com/Lithe-IDEA/Lithe-core-rust/main/shared/plugins/contract/v1/plugin-l10n.schema.json` |

> **Do not put `"$schema"` in `plugin.json`.** Lithe refuses unknown manifest fields, so adding it makes the plugin uninstallable. Associate the schema by file name in your editor instead. Translation files `l10n/*.json` may contain `"$schema"`; Lithe ignores it.

VS Code: add this to `.vscode/settings.json` in your plugin repository:

```json
{
  "json.schemas": [
    {
      "fileMatch": ["**/plugin.json"],
      "url": "https://raw.githubusercontent.com/Lithe-IDEA/Lithe-core-rust/main/shared/plugins/contract/v1/plugin-manifest.schema.json"
    },
    {
      "fileMatch": ["**/lithe-plugin-index.json"],
      "url": "https://raw.githubusercontent.com/Lithe-IDEA/Lithe-core-rust/main/shared/plugins/contract/v1/plugin-index.schema.json"
    },
    {
      "fileMatch": ["**/l10n/*.json"],
      "url": "https://raw.githubusercontent.com/Lithe-IDEA/Lithe-core-rust/main/shared/plugins/contract/v1/plugin-l10n.schema.json"
    }
  ]
}
```

JetBrains IDEs: Settings → Languages & Frameworks → Schemas and DTDs → JSON Schema Mappings; add each URL from the table above with its file pattern, such as `plugin.json`.

A schema checks only JSON structure. Rules such as ID references, permissions, activation events, and template variables are checked by Lithe at install time and are written in the [plugin contract](#plugin-contract).

### Plugin backend SDKs (optional)

A plugin that uses only language servers, run, and test needs no backend. When you need a build backend (`buildProviders`), plugin functions, or a backend process for the plugin's own web page (`backend`), write it with an official SDK: the SDK handles the protocol, cancellation, process cleanup, and errors, and you write only the handlers.

| SDK | Version | Runtime | Release | How it goes into the plugin |
| --- | --- | --- | --- | --- |
| [Node.js](sdk/node/README.md) | 0.5.0 | Node.js 20.3+ | [`sdk-node-v0.5.0`](https://github.com/Lithe-IDEA/Lithe-core-rust/releases/tag/sdk-node-v0.5.0): `lithe-plugin-sdk-node-0.5.0.tgz` and `.sha256` | Unpack it, copy `package/` into the plugin package, and import it by relative path |
| [Java](sdk/java/README.md) | 0.5.0 | Java 8+ | [`sdk-java-v0.5.0`](https://github.com/Lithe-IDEA/Lithe-core-rust/releases/tag/sdk-java-v0.5.0): `lithe-plugin-sdk-java-0.5.0.jar` and `.sha256` | Shade it into the backend JAR |
| [Rust](sdk/rust/README.md) | 0.4.0 | Rust 1.87+ | [`sdk-rust-v0.4.0`](https://github.com/Lithe-IDEA/Lithe-core-rust/releases/tag/sdk-rust-v0.4.0): `lithe-plugin-sdk-rust-0.4.0.crate` and `.sha256` | Pin the tag as a Git dependency (`Cargo.lock` records the commit), or unpack it as a `path` dependency; compile one backend executable per platform |

The Rust Git dependency looks like this:

```toml
lithe-plugin-sdk = { git = "https://github.com/Lithe-IDEA/Lithe-core-rust", tag = "sdk-rust-v0.4.0" }
```

All SDK sources live under `sdk/` in this directory and are published in the [Core repository's Releases](https://github.com/Lithe-IDEA/Lithe-core-rust/releases) (tags `sdk-node-v<version>`, `sdk-java-v<version>`, `sdk-rust-v<version>`), not on npm, Maven, or crates.io. The Rust SDK implements only the plugin backend (`backend`): plugin functions and host services; write build backends with the Node.js or Java SDK and VCS backends with the Node.js SDK. Each SDK's README has a "What the Lithe desktop serves" section naming the protocols the Lithe desktop client provides; the version of each protocol is in [Process protocol versions](reference/capabilities-and-versions.md#process-protocol-versions). The three versions above speak the version `0` protocols; the retired `lithe-plugin-docs` repository published earlier SDKs that speak an older protocol, and the Lithe desktop client does not accept plugins written with them, so use only versions released from the Core repository.

There is one rule: **download at build time and verify against the SHA-256 (or Git commit) you pinned; download nothing at run time.** An installed package has no `npm install` step and must run offline.

The [Maven plugin](https://github.com/Lithe-IDEA/lithe-plugin-maven)'s `sdk.lock.json` is a ready example of pinning the Java SDK.

### Example plugins

| Plugin | Demonstrates |
| --- | --- |
| [Lua](https://github.com/Lithe-IDEA/lithe-plugin-lua) | **Template**; a packaged language server (LuaLS) and running the current file |
| [Go](https://github.com/Lithe-IDEA/lithe-plugin-go) | The user's own gopls installation, running the main package and `go test` |
| [PHP](https://github.com/Lithe-IDEA/lithe-plugin-php) | The user's own Intelephense installation (its license forbids bundling), PHPUnit |
| [Gradle](https://github.com/Lithe-IDEA/lithe-plugin-gradle) | A purely declarative build system with no backend |
| [Maven](https://github.com/Lithe-IDEA/lithe-plugin-maven) | A build backend written with the Java SDK |
| [Cargo](https://github.com/Lithe-IDEA/lithe-plugin-cargo) | A build backend and plugin settings written with the Node.js SDK |
| [LLDB](https://github.com/Lithe-IDEA/lithe-plugin-lldb) | Reference debug adapter; debugs C, C++, Swift, and Rust with Xcode's `lldb-dap`, including `executable` lookup and debug console completion |

The example plugins are maintained in their own repositories. Before copying one, compare the `requires` in its `plugin.json` with the [version page](reference/capabilities-and-versions.md#capability-versions-requires) to confirm the Lithe desktop client declares the capabilities it needs.

### Guides

| Guide | Covers |
| --- | --- |
| [LSP plugin lifecycle](guides/lsp-plugin-lifecycle.md) | Starting from the Lua template: manifest, server packaging, activation, state directories, shutdown, upgrade, and uninstall |
| [Plugin logging](guides/plugin-logging.md) | The safe failure boundary of the three SDKs, the managed diagnostics channel, and the local session lifecycle of `${logDir}` |
| [Native build workflows](guides/native-build-workflows.md) | Build execution, exact tests, and run preparation in the Node and Java SDKs, and end-to-end verification against a real Core |
| [Build, run, and test integration](guides/build-tool-integration.md) | Execution provider examples, the project backend boundary, and the current status and published examples of declarative build systems |
| [Writing a debug adapter plugin](guides/debug-adapter-plugin.md) | From a minimal manifest to launch modes, compile-then-debug `executable`, and debug console completion |
| [Plugin backend: workspace contexts and host services](guides/plugin-backend-workspace.md) | Launching without a page, workspace contexts, backend-only commands, document, file, and window host methods, host notifications, failure codes, and the rules a backend follows while host services only grow |
| [Plugin-owned native terminals](guides/native-terminals.md) | `terminal/*`: create, show, close, reopen, and exit notifications (local workspaces) |

Lithe Store maintains the listing rules: [Publishing community plugins](https://github.com/Lithe-IDEA/Lithe-store/blob/main/docs/publishing.md) covers packaging, submitting a listing, and releasing new versions, and [Version tracking and updates](https://github.com/Lithe-IDEA/Lithe-store/blob/main/docs/versioning-and-updates.md) covers how Lithe chooses an installable version.

### Plugin reference

[`reference/`](reference/README.md) organizes the contract by feature, in Chinese and English:

| Page | Covers |
| --- | --- |
| [Plugin API catalog](reference/api-catalog.md) | For each of API-01–44, the methods usable today, what is still missing, and its feature page |
| [Capability and protocol versions](reference/capabilities-and-versions.md) | `requires` capability versions, process protocol versions, and document format versions, compared across Core and the Lithe desktop client |
| [Plugin protocol changelog](reference/protocol-changelog.md) | When, and in which PR, each protocol, capability, and API changed, and the PR that introduced each version; Chinese only |
| [Feature pages](reference/README.md#feature-pages) | One page for each of 15 features: fields, host status, contract sections, examples and cases |

### Plugin contract

The plugin contract defines the meaning, validation rules, and error codes of every field. It lives in [`contract/v1/`](contract/v1/README.md) of this directory, and its English text is normative:

| File | Contents |
| --- | --- |
| [`README.md`](contract/v1/README.md) | Complete description of fields, version rules, capabilities, permissions, and error codes |
| [`plugin-manifest.schema.json`](contract/v1/plugin-manifest.schema.json) | JSON Schema for `plugin.json` |
| [`plugin-index.schema.json`](contract/v1/plugin-index.schema.json) | JSON Schema for `lithe-plugin-index.json` |
| [`native-ui.schema.json`](contract/v1/native-ui.schema.json) | JSON Schema for the host-rendered native content of the `ui` capability |
| [`plugin-l10n.schema.json`](contract/v1/plugin-l10n.schema.json) | JSON Schema for a plugin's translation file `l10n/<language>.json` |
| [`toolchains/`](contract/v1/toolchains/README.md) | Plugin-owned toolchains: tool definitions, version manager providers, provider backend negotiation (`litheToolchains`), query and reply validation (with stable failure codes), pins, and four-layer resolution. Core implements these commands; the Lithe desktop client declares `toolDefinitions` and does not declare `toolchainProviders` |
| [`fixtures/`](contract/v1/fixtures) | Conformance cases in one directory per feature (`manifest`, `languages`, `backend`, `build-systems`, `build-providers`, `marketplace`, and so on), matching the [reference feature pages](reference/README.md#feature-pages): valid and invalid manifests, command and build plans, plugin function protocol, marketplace indexes, and Lithe's expected result for each case |

Fixtures live under `fixtures/<feature>/` and are named for the capability they demonstrate; for example `build-systems/build-command-stdout.plugin.json` demonstrates decoding a command's standard output. They use real tools such as Cargo, Go, Gradle, and Maven as inputs but are not those plugins; for complete plugins, see the [example plugins](#example-plugins). Each feature page's "Examples and cases" section lists its files and case counts.

Lithe's Rust Core enforces the contract. The contract, guides, reference, official SDKs, SDK end-to-end tests, and scripts all live under `shared/plugins/` in the Core repository and change in the same PR as the Core code that enforces the contract (`rust/` at the repository root), so the contract, SDKs, and Core always agree and there is no copy to synchronize. This directory follows the Core repository's `main`; what is on `main` may not be in a released Lithe version yet, so check what your target version supports before publishing.

For questions or suggestions about the contract, [open an issue in the Core repository](https://github.com/Lithe-IDEA/Lithe-core-rust/issues).

### Contributing and maintenance

A PR that changes the contract, guides, reference, or SDKs goes to the [Core repository](https://github.com/Lithe-IDEA/Lithe-core-rust), in the same PR as the matching Rust Core change. Run the commands below at the **root of the Core repository**; CI runs them from `.github/workflows/plugins-verify.yml` at the repository root on every change that touches `shared/plugins/` or `rust/`.

| Command | Purpose |
| --- | --- |
| `node shared/plugins/scripts/verify-docs.mjs` | Offline check: each schema's `$id` equals its published address `https://raw.githubusercontent.com/Lithe-IDEA/Lithe-core-rust/main/shared/plugins/contract/v1/<file>`, Markdown relative links resolve, and no link points to the retired `lithe-plugin-docs` repository |
| `node --test shared/plugins/scripts/*.test.mjs` | Tests for the scripts themselves |

When a change touches the contract, a capability version, or the plugin function protocol, the same PR also updates the [plugin protocol changelog](reference/protocol-changelog.md) and the affected feature pages and version tables in [`reference/`](reference/README.md): they are hand-written comparisons and do not update on their own. The changelog's "Maintenance" section describes how.

#### SDKs

| Command | Purpose |
| --- | --- |
| `node --test shared/plugins/sdk/node/test/*.test.mjs` | Node.js SDK unit tests |
| `npm run --prefix shared/plugins/sdk/node test:types` | Node.js SDK TypeScript consumer check; first run `npm ci --prefix shared/plugins/sdk/node --ignore-scripts` to install the development dependencies |
| `mvn -B -ntp -f shared/plugins/sdk/java/pom.xml verify` | Java SDK unit tests; add `-Djvm=<java>` to choose the Java that runs them (CI uses Java 8) |
| `cargo test --locked --manifest-path shared/plugins/sdk/rust/Cargo.toml` | Rust SDK unit tests (CI uses Rust 1.87, plus stable for `cargo fmt` and `cargo clippy`) |
| `node shared/plugins/scripts/sdk-e2e.mjs` | End-to-end tests: builds the sample backends of the three SDKs, then runs the four tests `plugin_sdk_node`, `plugin_sdk_java`, `plugin_sdk_rust`, and `plugin_sdk_build` of `cargo test -p lithe-core --features plugin-sdk-e2e` against the **Core in the current working tree**; it always includes the native build workflow and shortcut import scenarios |

The end-to-end tests need Rust, Node.js 20.3+, Java (set `LITHE_PLUGIN_SDK_JAVA_RUNTIME`, or have `java` on `PATH`), and Maven. The scenario files are in [`sdk/e2e/`](sdk/e2e): Node.js and Java run the same build backend scenarios, Rust runs the plugin backend scenario (`plugin_sdk/backend.rs`), and the test answers credential requests in place of the host. `LITHE_PLUGIN_SDK_JAVA_SAMPLE`, `LITHE_PLUGIN_SDK_JAVA_NATIVE_SAMPLE`, and `LITHE_PLUGIN_SDK_RUST_SAMPLE` can point at already built samples to skip building them.

Releasing: first bump the version (Node.js in `shared/plugins/sdk/node/package.json`; for Java run `mvn -f shared/plugins/sdk/java/pom.xml versions:set -DnewVersion=<version> -DgenerateBackupPoms=false`, which updates all three poms; for Rust change `workspace.package.version` in `shared/plugins/sdk/rust/Cargo.toml` and update `Cargo.lock`), then after merging push the tag `sdk-node-v<version>`, `sdk-java-v<version>`, or `sdk-rust-v<version>`. The release workflows `.github/workflows/plugins-release-{node,java,rust}-sdk.yml` at the repository root start from the tag, check that it matches the version, and, once tests pass, publish the package and its `.sha256` to the Core repository's Releases. After a release, update the version numbers in this page and in each SDK README.

### License

[Apache-2.0](LICENSE)
