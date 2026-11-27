# Agent / Agents

[中文](#中文) · [English](#english)

## 中文

插件可以把用户自己安装、自己登录的 Agent 命令行工具接进 Lithe。插件只声明这个 CLI 是哪个工具、可以用哪几种形式运行、会话文件放在哪里；Core 启动和回收 CLI 进程，插件后端只在 CLI 自己的协议和 Lithe 的会话事件之间翻译。插件拿不到 CLI 的凭据，启动结果不能带环境变量；只有用户的操作能开始会话、发送提示和回答权限请求。

| 项目 | 内容 |
| --- | --- |
| 清单字段 | `agents[]`：`{id, displayName, icon?, tool, native?, acp?, terminal?, update?, sessionSources?}`，每个插件最多 8 个 |
| 能力版本 | `agents` 0；有 Agent 声明 `native` 或 `sessionSources` 时还需要 `backend` 0。见[版本对照](../capabilities-and-versions.md#能力版本requires) |
| API 编号 | API-45 |
| 权限 | `agents.run`（有 Agent 就必须声明）；`agents.native`（有 `native`）；`agents.sessions.read`（有 `sessionSources`）；`agents.usage.read`（读用量汇总，需要后端，没有 Agent 的插件也可以声明）。多声明或少声明都会被拒绝 |
| 激活事件 | 声明了 `native` 或 `sessionSources` 的 Agent 必须有 `onAgent:<id>`，其他 Agent 不能有 |
| 后端启动 | 只有终端形式的 Agent 不需要后端 |
| 安装审查 | 每种运行形式列成一条 `agent` 程序，每个会话来源列成一条 `agentSessions` 程序，用户安装前看到 CLI 会被怎样运行、哪些目录会被读取 |
| Lithe 桌面端 | 接入中，尚未声明 `agents` |

### 运行形式

| 形式 | 谁做什么 |
| --- | --- |
| `native` | Core 调用后端函数拿到命令行（只能是 `{executable: {kind: "tool", tool}, arguments}`，不能带环境变量），自己启动 CLI；CLI 的标准输出经 `lithe/host/agents/didOutput` 交给后端，后端用 `lithe/host/agents/report` 报告会话事件，用 `lithe/host/agents/write` 写 CLI 的标准输入 |
| `terminal` | 宿主在终端里用声明的参数运行 CLI，用的是 CLI 自己的交互界面 |
| `acp` | 版本 0 只校验、列入审查和目录，不运行 Agent Client Protocol 客户端；只提供 ACP 的 Agent 应同时声明 `terminal` |
| `update` | 用户确认命令行后，用声明的参数运行 CLI 的自更新命令 |

工具一律来自插件的 `requiresTools`：宿主发现并探测版本，会话运行的始终是用户装好的那份 CLI，插件不能指定路径。会话只在可信的本地工作区运行，远程工作区按 API-40 拒绝。

### 会话记录与用量

- 用户逐个 Agent 同意后，Core 才读取 `sessionSources` 声明的目录：只读匹配 `include` 的普通文件，不跟随符号链接；后端的 `scan` 函数把新增的行变成记录，`read` 函数把记录的范围变成标题或消息。
- Core 缓存记录，负责列表、全文搜索和统计；缓存不存正文，可以删除重建。
- `lithe/host/agents/usage` 按天、Agent 或模型汇总用户已开启记录的 Agent 的用量，并带上最近的额度窗口。它只给数字和模型名，不含对话正文、标题、会话 ID 或工作目录，适合做跨 Agent 的统计和额度面板。

### 契约章节

| 章节 | 内容 |
| --- | --- |
| [Agents](../../contract/v1/README.md#agents) | 总览和清单示例 |
| [Agent declarations](../../contract/v1/README.md#agent-declarations) | 字段、限制、安装审查列出的程序 |
| [Agent permissions and activation](../../contract/v1/README.md#agent-permissions-and-activation) | 四个权限、`onAgent:<id>`、拒绝码 |
| [Host commands](../../contract/v1/README.md#host-commands) | 宿主调用的 `agents.*` 命令和事件队列 |
| [Native sessions](../../contract/v1/README.md#native-sessions) | 启动流程、通知、宿主方法、会话事件 |
| [Session records](../../contract/v1/README.md#session-records) | 会话来源、`scan` 与 `read` 函数、汇总和拒绝码 |
| [Privacy and logging](../../contract/v1/README.md#privacy-and-logging) | 日志只记 ID 和元数据，不读凭据文件 |
| [Limits of version 0](../../contract/v1/README.md#limits-of-version-0) | 不运行 ACP 客户端，只在可信本地工作区运行 |
| [Rust Core API：Commands](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/contracts/rust-core-api.md#commands) | `agents.*` 与 `agentSessions.*` 命令（Rust Core API 文档） |

### 示例与用例

| 文件 | 内容 |
| --- | --- |
| [`agents/agents.plugin.json`](../../contract/v1/fixtures/agents/agents.plugin.json) | 一个带原生会话、终端形式、更新命令和一个会话来源的 Agent |
| [`agents/terminal-agent.plugin.json`](../../contract/v1/fixtures/agents/terminal-agent.plugin.json) | 只有终端形式、不需要后端的 Agent |
| [`manifest/invalid-manifests.json`](../../contract/v1/fixtures/manifest/invalid-manifests.json) | 以这两个清单为基础的拒绝用例 |

---

## English

A plugin can connect an agent command-line tool that the user installed and signed in to themselves. The plugin declares only which tool the CLI is, the forms it can run in, and where it keeps its session files. Core starts and reaps the CLI process; the plugin backend only translates between the CLI's own protocol and Lithe's session events. The plugin never gets the CLI's credentials, and a launch answer cannot carry environment variables. Only the user's actions start a session, send a prompt, or answer a permission request.

| Item | Value |
| --- | --- |
| Manifest field | `agents[]`: `{id, displayName, icon?, tool, native?, acp?, terminal?, update?, sessionSources?}`, at most 8 per plugin |
| Capability versions | `agents` 0, and also `backend` 0 when an agent declares `native` or `sessionSources`. See [versions](../capabilities-and-versions.md#capability-versions-requires) |
| API IDs | API-45 |
| Permissions | `agents.run` (required whenever there are agents); `agents.native` (with `native`); `agents.sessions.read` (with `sessionSources`); `agents.usage.read` (reads usage totals, needs a backend, and may be declared by a plugin without agents). Declaring one too many or one too few is refused |
| Activation events | An agent that declares `native` or `sessionSources` needs `onAgent:<id>`; other agents may not have one |
| Backend launch | A terminal-only agent needs no backend |
| Install review | Each form is an `agent` program and each session source an `agentSessions` program, so the user sees how the CLI will run and which directories will be read before installing |
| Lithe desktop | Integration in progress; does not declare `agents` yet |

### Forms

| Form | Who does what |
| --- | --- |
| `native` | Core calls a backend function for the command line (only `{executable: {kind: "tool", tool}, arguments}`, no environment variables) and starts the CLI itself. The CLI's standard output reaches the backend through `lithe/host/agents/didOutput`; the backend reports session events with `lithe/host/agents/report` and writes the CLI's standard input with `lithe/host/agents/write` |
| `terminal` | The host runs the CLI with the declared arguments in a terminal: the CLI's own interactive interface |
| `acp` | Version 0 validates it and lists it in the review and the catalog, but runs no Agent Client Protocol client; an agent that offers only ACP should also declare `terminal` |
| `update` | After the user confirms the command line, runs the CLI's self-update command with the declared arguments |

The tool always comes from the plugin's `requiresTools`: the host discovers it and probes its version, so a session always runs the CLI the user installed and the plugin cannot name a path. Sessions run only in trusted local workspaces; remote workspaces are refused as API-40 describes.

### Session records and usage

- Core reads the directories `sessionSources` declares only after the user consents for that agent. It reads only regular files that match `include` and does not follow symbolic links. The backend's `scan` function turns new lines into records, and its `read` function turns recorded ranges into titles or messages.
- Core caches the records and answers lists, full-text search, and statistics. The cache stores no message text and can be deleted and rebuilt.
- `lithe/host/agents/usage` sums the usage of the agents whose records the user enabled, by day, agent, or model, with the latest quota windows. It carries only numbers and model names, never conversation text, titles, session IDs, or working directories, so it suits cross-agent statistics and quota panels.

### Contract sections

| Section | Content |
| --- | --- |
| [Agents](../../contract/v1/README.md#agents) | Overview and manifest example |
| [Agent declarations](../../contract/v1/README.md#agent-declarations) | Fields, limits, and the programs the install review lists |
| [Agent permissions and activation](../../contract/v1/README.md#agent-permissions-and-activation) | The four permissions, `onAgent:<id>`, and refusal codes |
| [Host commands](../../contract/v1/README.md#host-commands) | The `agents.*` commands the host calls and the event queue |
| [Native sessions](../../contract/v1/README.md#native-sessions) | Launch flow, notifications, host methods, session events |
| [Session records](../../contract/v1/README.md#session-records) | Session sources, the `scan` and `read` functions, totals, and refusal codes |
| [Privacy and logging](../../contract/v1/README.md#privacy-and-logging) | Logs carry only IDs and metadata; no credential file is read |
| [Limits of version 0](../../contract/v1/README.md#limits-of-version-0) | No ACP client; trusted local workspaces only |
| [Rust Core API: Commands](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/contracts/rust-core-api.md#commands) | The `agents.*` and `agentSessions.*` commands (Rust Core API document) |

### Examples and cases

| File | Content |
| --- | --- |
| [`agents/agents.plugin.json`](../../contract/v1/fixtures/agents/agents.plugin.json) | An agent with a native session, a terminal form, an update command, and one session source |
| [`agents/terminal-agent.plugin.json`](../../contract/v1/fixtures/agents/terminal-agent.plugin.json) | A terminal-only agent without a backend |
| [`manifest/invalid-manifests.json`](../../contract/v1/fixtures/manifest/invalid-manifests.json) | Rejection cases built on these two manifests |
