# Agent / Agents

[中文](#中文) · [English](#english)

## 中文

插件可以把用户自己安装、自己登录的 Agent 命令行工具接进 Lithe。插件只声明这个 CLI 是哪个工具、可以用哪几种形式运行、会话文件放在哪里；Core 启动和回收 CLI 进程，插件后端只在 CLI 自己的协议和 Lithe 的会话事件之间翻译。插件拿不到 CLI 的凭据，启动结果不能带环境变量；只有用户的操作能开始会话、发送提示、回答权限请求和修改会话设置。

| 项目 | 内容 |
| --- | --- |
| 清单字段 | `agents[]`：`{id, displayName, icon?, tool, native?, acp?, terminal?, update?, sessionSources?, configuration?}`，每个插件最多 8 个 |
| 能力版本 | `agents` 0；有 Agent 声明 `native`、`sessionSources` 或带 `read` 的 `configuration` 时还需要 `backend` 0。`configuration`、`agents.configuration` 和 `native.imageTypes` 是 `agents` 0 内的补充，不认识它们的 Core 拒绝这样的清单。见[版本对照](../capabilities-and-versions.md#能力版本requires) |
| API 编号 | API-45 |
| 权限 | `agents.run`（有 Agent 就必须声明）；`agents.native`（有 `native`）；`agents.sessions.read`（有 `sessionSources`）；`agents.configuration`（有 `configuration`）；`agents.usage.read`（读用量汇总，需要后端，没有 Agent 的插件也可以声明）。多声明或少声明都会被拒绝 |
| 激活事件 | 声明了 `native`、`sessionSources` 或带 `read` 的 `configuration` 的 Agent 必须有 `onAgent:<id>`，其他 Agent 不能有 |
| 后端启动 | 只有终端形式的 Agent 不需要后端 |
| 官方 SDK | [Rust](../../sdk/rust/README.md#native-agent-sessions)、[Go](../../sdk/go/README.md#native-agent-sessions)、[Node.js](../../sdk/node/README.md#native-agent-sessions)、[Java](../../sdk/java/README.md#native-agent-sessions) 的 `main` 源码都有原生会话接口（`onAgentSessions`，Rust 是 `on_agent_sessions`）和 `agents` 宿主服务（`write`、`report`、`usage`），随各 SDK 的下一次发布提供；已发布的 SDK 版本没有。`configuration` 的 `read` 和 `edit` 是普通插件函数，任何 SDK 版本都能注册，见各 SDK 的 Agent configuration functions 一节 |
| 安装审查 | 每种运行形式列成一条 `agent` 程序，每个会话来源列成一条 `agentSessions` 程序，声明了 `configuration` 的 Agent 列成一条 `agentConfiguration` 程序，用户安装前看到 CLI 会被怎样运行、哪些目录和文件会被读取或修改 |
| Lithe 桌面端 | 接入中，尚未声明 `agents` |

### 运行形式

| 形式 | 谁做什么 |
| --- | --- |
| `native` | Core 调用后端函数拿到命令行（只能是 `{executable: {kind: "tool", tool}, arguments}`，不能带环境变量），自己启动 CLI；CLI 的标准输出经 `lithe/host/agents/didOutput` 交给后端，后端用 `lithe/host/agents/report` 报告会话事件，用 `lithe/host/agents/write` 写 CLI 的标准输入；后端报告的 `configOptions`（模型、推理等级、模式）由用户在宿主里切换，经 `lithe/host/agents/didSetConfigOption` 交回后端；声明了 `native.imageTypes` 的 Agent 可以收到带图片的提示，Core 检查每张图片的类型和大小后，把 `{mimeType, path}` 经 `lithe/host/agents/didPrompt` 的 `images` 交给后端，后端按 CLI 自己的格式传给它 |
| `terminal` | 宿主在终端里用声明的参数运行 CLI，用的是 CLI 自己的交互界面 |
| `acp` | 版本 0 只校验、列入审查和目录，不运行 Agent Client Protocol 客户端；只提供 ACP 的 Agent 应同时声明 `terminal` |
| `update` | 用户确认命令行后，用声明的参数运行 CLI 的自更新命令 |

工具一律来自插件的 `requiresTools`：宿主发现并探测版本，会话运行的始终是用户装好的那份 CLI，插件不能指定路径。会话只在可信的本地工作区运行，远程工作区按 API-40 拒绝。

### 会话记录与用量

- 用户逐个 Agent 同意后，Core 才读取 `sessionSources` 声明的目录：只读匹配 `include` 的普通文件，不跟随符号链接；后端的 `scan` 函数把新增的行变成记录，`read` 函数把记录的范围变成标题或消息。
- Core 缓存记录，负责列表、全文搜索和统计；缓存不存正文，可以删除重建。
- `lithe/host/agents/usage` 按天、Agent 或模型汇总用户已开启记录的 Agent 的用量，并带上最近的额度窗口。它只给数字和模型名，不含对话正文、标题、会话 ID 或工作目录，适合做跨 Agent 的统计和额度面板。
- `agentSessions.calls` 按时间倒序分页列出 `scan` 报告的每一次调用，带所属 Agent 和调用本身（模型、令牌数等），以及 Core 算出的费用、缓存命中率和速度，供用量页的请求列表和详情使用；不含正文。

### 资源库：指令、技能、MCP 服务器与 Hooks

宿主的 Agent 资源库把指令、技能、MCP 服务器和 Hooks 存一份，用户按 Agent 开关后，Core 写进每个 CLI 自己的配置。插件只声明 `configuration`：

- `instructions`、`skills`：CLI 读取的用户指令文件和用户技能目录。Core 自己读写它们：跟随资源库的 Agent，指令文件整份写成当前启用的那一套指令加上该 Agent 的附加文字；技能以符号链接放进技能目录，资源库外的技能只显示、不改。
- `files` 和 `read`、`edit`：保存 MCP 服务器和 Hooks 的 CLI 配置文件。只有后端认识它们的格式：`read` 报告文件里有哪些服务器和 Hooks（环境变量和请求头只给名称），`edit` 只改 Core 给的暂存副本，并保留文件里其他内容；Core 校验后用原子替换写回，文件在此期间被改动就拒绝。
- CLI 自己装的内容（比如 CLI 插件带来的 MCP 服务器、Hooks 和技能）标为 `origin: "agent"`，宿主只显示、不改。
- 宿主按插件声明的 `displayName` 和 `icon` 显示每个 Agent，不按名字特殊处理。

所有改动都只能由用户的操作发起；Core 只读写声明的路径；插件没有获得 `agents.configuration` 授权时，它的 Agent 以 `mayConfigure: false` 列出，Core 什么都不读、不改。命令见 Rust Core API 的 `agentLibrary.*`。

### 契约章节

| 章节 | 内容 |
| --- | --- |
| [Agents](../../contract/v1/README.md#agents) | 总览和清单示例 |
| [Agent declarations](../../contract/v1/README.md#agent-declarations) | 字段、限制、安装审查列出的程序 |
| [Agent permissions and activation](../../contract/v1/README.md#agent-permissions-and-activation) | 五个权限、`onAgent:<id>`、拒绝码 |
| [Host commands](../../contract/v1/README.md#host-commands) | 宿主调用的 `agents.*` 命令和事件队列 |
| [Native sessions](../../contract/v1/README.md#native-sessions) | 启动流程、通知、宿主方法、会话事件 |
| [Session records](../../contract/v1/README.md#session-records) | 会话来源、`scan` 与 `read` 函数、汇总和拒绝码 |
| [Agent configuration](../../contract/v1/README.md#agent-configuration) | `configuration` 声明、资源库的存储和命令、`read` 与 `edit` 函数、限制和拒绝码 |
| [Privacy and logging](../../contract/v1/README.md#privacy-and-logging) | 日志只记 ID 和元数据，不读凭据文件 |
| [Limits of version 0](../../contract/v1/README.md#limits-of-version-0) | 不运行 ACP 客户端，只在可信本地工作区运行 |
| [Rust Core API：Commands](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/contracts/rust-core-api.md#commands) | `agents.*`、`agentSessions.*` 与 `agentLibrary.*` 命令（Rust Core API 文档） |

### 示例与用例

| 文件 | 内容 |
| --- | --- |
| [`agents/agents.plugin.json`](../../contract/v1/fixtures/agents/agents.plugin.json) | 一个带原生会话、终端形式、更新命令、一个会话来源和一份资源库配置声明的 Agent |
| [`agents/terminal-agent.plugin.json`](../../contract/v1/fixtures/agents/terminal-agent.plugin.json) | 只有终端形式、不需要后端的 Agent |
| [`manifest/invalid-manifests.json`](../../contract/v1/fixtures/manifest/invalid-manifests.json) | 以这两个清单为基础的拒绝用例 |
| [`agents/native-session.json`](../../contract/v1/fixtures/agents/native-session.json) | 原生会话通知和每个会话的处理器依次收到的调用、启动或提示失败时报告的 `turnEnded`、`agents` 宿主方法的请求；四个 SDK 的测试都用它 |

---

## English

A plugin can connect an agent command-line tool that the user installed and signed in to themselves. The plugin declares only which tool the CLI is, the forms it can run in, and where it keeps its session files. Core starts and reaps the CLI process; the plugin backend only translates between the CLI's own protocol and Lithe's session events. The plugin never gets the CLI's credentials, and a launch answer cannot carry environment variables. Only the user's actions start a session, send a prompt, answer a permission request, or change a session setting.

| Item | Value |
| --- | --- |
| Manifest field | `agents[]`: `{id, displayName, icon?, tool, native?, acp?, terminal?, update?, sessionSources?, configuration?}`, at most 8 per plugin |
| Capability versions | `agents` 0, and also `backend` 0 when an agent declares `native`, `sessionSources` or a `configuration` with `read`. `configuration`, `agents.configuration` and `native.imageTypes` were added within `agents` 0; a Core that does not know them refuses such a manifest. See [versions](../capabilities-and-versions.md#capability-versions-requires) |
| API IDs | API-45 |
| Permissions | `agents.run` (required whenever there are agents); `agents.native` (with `native`); `agents.sessions.read` (with `sessionSources`); `agents.configuration` (with `configuration`); `agents.usage.read` (reads usage totals, needs a backend, and may be declared by a plugin without agents). Declaring one too many or one too few is refused |
| Activation events | An agent that declares `native`, `sessionSources` or a `configuration` with `read` needs `onAgent:<id>`; other agents may not have one |
| Backend launch | A terminal-only agent needs no backend |
| Official SDKs | The [Rust](../../sdk/rust/README.md#native-agent-sessions), [Go](../../sdk/go/README.md#native-agent-sessions), [Node.js](../../sdk/node/README.md#native-agent-sessions), and [Java](../../sdk/java/README.md#native-agent-sessions) SDK sources on `main` have the native session interface (`onAgentSessions`, `on_agent_sessions` in Rust) and the `agents` host service (`write`, `report`, `usage`), shipping in each SDK's next release; released SDK versions do not. The `read` and `edit` of `configuration` are ordinary plugin functions any SDK version registers; see each SDK's Agent configuration functions section |
| Install review | Each form is an `agent` program, each session source an `agentSessions` program, and each agent that declares `configuration` an `agentConfiguration` program, so the user sees how the CLI will run and which directories and files will be read or changed before installing |
| Lithe desktop | Integration in progress; does not declare `agents` yet |

### Forms

| Form | Who does what |
| --- | --- |
| `native` | Core calls a backend function for the command line (only `{executable: {kind: "tool", tool}, arguments}`, no environment variables) and starts the CLI itself. The CLI's standard output reaches the backend through `lithe/host/agents/didOutput`; the backend reports session events with `lithe/host/agents/report` and writes the CLI's standard input with `lithe/host/agents/write`. The user switches the `configOptions` the backend reports (model, reasoning level, mode) in the host, and the choice reaches the backend through `lithe/host/agents/didSetConfigOption`. An agent that declares `native.imageTypes` can take prompts with images: Core checks each image's type and size, then hands `{mimeType, path}` to the backend in the `images` of `lithe/host/agents/didPrompt`, and the backend passes them to the CLI in its own form |
| `terminal` | The host runs the CLI with the declared arguments in a terminal: the CLI's own interactive interface |
| `acp` | Version 0 validates it and lists it in the review and the catalog, but runs no Agent Client Protocol client; an agent that offers only ACP should also declare `terminal` |
| `update` | After the user confirms the command line, runs the CLI's self-update command with the declared arguments |

The tool always comes from the plugin's `requiresTools`: the host discovers it and probes its version, so a session always runs the CLI the user installed and the plugin cannot name a path. Sessions run only in trusted local workspaces; remote workspaces are refused as API-40 describes.

### Session records and usage

- Core reads the directories `sessionSources` declares only after the user consents for that agent. It reads only regular files that match `include` and does not follow symbolic links. The backend's `scan` function turns new lines into records, and its `read` function turns recorded ranges into titles or messages.
- Core caches the records and answers lists, full-text search, and statistics. The cache stores no message text and can be deleted and rebuilt.
- `lithe/host/agents/usage` sums the usage of the agents whose records the user enabled, by day, agent, or model, with the latest quota windows. It carries only numbers and model names, never conversation text, titles, session IDs, or working directories, so it suits cross-agent statistics and quota panels.
- `agentSessions.calls` pages through every call `scan` reported, most recent first, with its agent and the call itself (model, tokens and so on), plus the cost, cache hit rate and speed Core derives, for the request list and details of a usage page. It carries no message text.

### The library: instructions, skills, MCP servers and hooks

The host's agent library keeps one copy of instructions, skills, MCP servers and hooks; once the user turns them on per agent, Core writes them into each CLI's own configuration. The plugin only declares `configuration`:

- `instructions` and `skills`: the user instruction file and the user skills directory the CLI reads. Core reads and writes them itself: for an agent that follows the library it writes the whole instruction file as the active instruction set followed by that agent's extra text, and it puts skills into the skills directory as symbolic links. Skills outside the library are shown and never changed.
- `files` with `read` and `edit`: the CLI configuration files that hold MCP servers and hooks. Only the backend knows their format. `read` reports the servers and hooks a file holds (names only for environment variables and headers), and `edit` changes only the staged copy Core gives it and keeps everything else in the file. Core checks the copy and replaces the file atomically, refusing when the file changed meanwhile.
- What the CLI installed itself, such as the MCP servers, hooks and skills a CLI plugin brings, has `origin: "agent"`; the host shows it and never changes it.
- The host shows each agent by the `displayName` and `icon` its plugin declares and never special-cases agents by name.

Every change comes only from a user action. Core reads and writes only the declared paths. An agent whose plugin is not granted `agents.configuration` is listed with `mayConfigure: false`, and Core reads and changes nothing of it. The commands are the `agentLibrary.*` commands of the Rust Core API.

### Contract sections

| Section | Content |
| --- | --- |
| [Agents](../../contract/v1/README.md#agents) | Overview and manifest example |
| [Agent declarations](../../contract/v1/README.md#agent-declarations) | Fields, limits, and the programs the install review lists |
| [Agent permissions and activation](../../contract/v1/README.md#agent-permissions-and-activation) | The five permissions, `onAgent:<id>`, and refusal codes |
| [Host commands](../../contract/v1/README.md#host-commands) | The `agents.*` commands the host calls and the event queue |
| [Native sessions](../../contract/v1/README.md#native-sessions) | Launch flow, notifications, host methods, session events |
| [Session records](../../contract/v1/README.md#session-records) | Session sources, the `scan` and `read` functions, totals, and refusal codes |
| [Agent configuration](../../contract/v1/README.md#agent-configuration) | The `configuration` declaration, the library's storage and commands, the `read` and `edit` functions, limits, and refusal codes |
| [Privacy and logging](../../contract/v1/README.md#privacy-and-logging) | Logs carry only IDs and metadata; no credential file is read |
| [Limits of version 0](../../contract/v1/README.md#limits-of-version-0) | No ACP client; trusted local workspaces only |
| [Rust Core API: Commands](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/contracts/rust-core-api.md#commands) | The `agents.*`, `agentSessions.*`, and `agentLibrary.*` commands (Rust Core API document) |

### Examples and cases

| File | Content |
| --- | --- |
| [`agents/agents.plugin.json`](../../contract/v1/fixtures/agents/agents.plugin.json) | An agent with a native session, a terminal form, an update command, one session source, and a library configuration declaration |
| [`agents/terminal-agent.plugin.json`](../../contract/v1/fixtures/agents/terminal-agent.plugin.json) | A terminal-only agent without a backend |
| [`manifest/invalid-manifests.json`](../../contract/v1/fixtures/manifest/invalid-manifests.json) | Rejection cases built on these two manifests |
| [`agents/native-session.json`](../../contract/v1/fixtures/agents/native-session.json) | Native-session notifications with the calls each session's handler receives, the `turnEnded` reports of failed starts and prompts, and the `agents` host method requests; all four SDKs test against it |
