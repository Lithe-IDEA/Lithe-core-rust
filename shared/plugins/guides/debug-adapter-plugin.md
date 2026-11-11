# 编写调试适配器插件 / Writing a debug adapter plugin

[中文](#中文) · [English](#english)

## 中文

这篇指南面向想让 Lithe 调试某种语言或运行时的插件作者。调试适配器插件只需要一份 `plugin.json`：声明一个遵循
[DAP（Debug Adapter Protocol）](https://microsoft.github.io/debug-adapter-protocol/)的进程怎么启动，Lithe 负责断点、
单步、变量、调用栈、调试控制台这些界面和会话管理。我们以官方的
[LLDB 插件](https://github.com/Lithe-IDEA/lithe-plugin-lldb)为例，它用 Xcode 自带的 `lldb-dap` 调试 C、C++、Objective-C、Rust 和 Swift。

字段的权威定义在[契约](../contract/v1/README.md#debug-adapters)，这里只讲怎么用、为什么这样设计。发布步骤见
[发布社区插件](https://github.com/Lithe-IDEA/Lithe-store/blob/main/docs/publishing.md)。

> **宿主支持。** 调试适配器由 Lithe 桌面端实现，支持 stdio 和插件语言服务器内的回环 TCP 两种传输。宿主提供哪些能力，以
> 桌面端的[能力声明](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go)（`Declared()` 上方注释）为准；
> 能力版本见[版本说明](../reference/capabilities-and-versions.md)。

### 1. 插件和宿主各自负责什么

| 对象 | 插件作者负责 | Lithe 负责 |
| --- | --- | --- |
| 调试适配器 | 选型、许可、固定版本、启动入口 | 启动进程、DAP 通信、会话停止 |
| 要调试的程序 | 说明怎么产生它（构建命令、编译参数） | 在设置里显示实际找到的路径，把它交给适配器 |
| 调试界面 | 不需要写 | 断点、变量、调用栈、调试控制台与补全 |
| 权限 | 声明 `debug.launch` | 安装时让用户确认 |
| 激活 | 声明 `onDebug:<适配器 id>` | 调试会话需要时才启动适配器 |

插件不加载进 Lithe 进程。清单是纯数据：没有解析器、没有工作流、没有宿主集成，也不能传 `initializationOptions`，写了会被拒绝。

### 2. 最小清单

```json
{
  "schemaVersion": 0,
  "id": "com.example.demo-debug",
  "displayName": "Demo Debugger",
  "version": "0.1.0",
  "pluginAPIVersion": 0,
  "requires": { "debugAdapters": 0, "languages": 0 },
  "vendor": { "id": "com.example", "displayName": "Example", "signatureRequirement": "none" },
  "permissions": ["debug.launch"],
  "activationEvents": ["onDebug:demo"],
  "languages": [
    { "id": "demo", "displayName": "Demo", "fileExtensions": ["demo"] }
  ],
  "debugAdapters": [
    {
      "id": "demo",
      "displayName": "Demo",
      "requests": ["launch"],
      "selectors": [{ "targetKinds": ["file"], "languageIDs": ["demo"] }],
      "configurations": {
        "launch": {
          "arguments": { "stopOnEntry": false },
          "targetBindings": { "program": "target.path", "cwd": "target.workingDirectory" }
        }
      },
      "launch": {
        "process": { "launch": { "kind": "systemCommand", "commands": ["demo-dap"] } },
        "transport": "stdio"
      }
    }
  ]
}
```

要点：

- `pluginAPIVersion` 和 `schemaVersion` 都是 `0`。`requires` 必须逐项列出实际用到的能力，每项都写 `0`：使用 `debugAdapters` 时列出它；声明 `languages` 时列出 `languages`；由后端发起调试时还要列出 `backend`，见第 6 节。
- 每个适配器都要有一条 `onDebug:<id>` 激活事件，并声明 `debug.launch` 权限。适配器列出 `attach` 时才需要（也只能声明）`debug.attach`。
- `selectors` 决定 Lithe 在哪些目标上自动选这个适配器：`targetKinds` 是目标种类，`languageIDs` 是语言。不写 `selectors` 表示只能被用户显式选择。
- `languages[]` 声明适配器服务的语言。如果这个语言已经由别的插件提供（例如 Rust），你的声明只会给它增加“可以调试”的能力，并并入你声明的文件扩展名；语言名、语言标识和语言服务器不变，装上调试器也不会重启正在运行的语言服务器。
- `targetBindings` 把调试目标的事实填进适配器的启动参数：`program` ← 目标路径，`cwd` ← 工作目录，`processId` ← 进程 ID。只填 `arguments` 里没写的字段，不会覆盖你显式写出的默认值。目标缺少绑定要读的事实时，Lithe 会拒绝这次启动并说明缺什么。
- `configurationTypes` 声明适配器对应的 `launch.json` `type`，用于导入用户已有的 VS Code 风格配置。

### 3. 怎么启动适配器

`launch.process` 有三种常见写法：

| 写法 | 适用场景 |
| --- | --- |
| `{"launch": {"kind": "packaged", "path": …}}` | 适配器许可允许分发、你能为每个平台提供构建；可执行文件随插件包安装 |
| `{"launch": {"kind": "systemCommand", "commands": ["lldb-dap"]}}` | 适配器已经装在用户机器上（例如随 Xcode 命令行工具提供）；宿主用自己的工具发现解析命令名。Lithe 桌面端目前只查 PATH，依赖项目工具链或用户覆盖的启动会被拒绝 |
| `{"launch": {"kind": "runSelectedRuntime", "tool": "<工具定义 ID>"}}` | 适配器是 Python / Java / Node.js 等程序，用用户选定的运行时启动；`tool` 必须是清单顶层 `requiresTools` 列出的工具定义 ID |

`runSelectedRuntime.tool` 的规则见契约的 [Tool references](../contract/v1/README.md#tool-references)。

`launch.transport` 是 `stdio`，或者 TCP（`tcpAnnounced`：适配器在输出里打印端口；`tcpPort`：固定端口）。TCP 只允许回环地址。

插件包和 Lithe 的安装目录运行时是只读的，适配器不能往里面写日志、索引或状态，否则会破坏代码签名和增量更新。宿主会把适配器的状态放在它自己的可写目录下，不放进插件包或安装目录。

### 4. 调试要先编译的语言：`executable`

DAP 适配器大多直接接受“要运行的程序”，但 C、C++、Swift 这类语言的用户打开的是**源文件**，真正要调试的是编译产物。
`configurations.launch.executable` 就是为这种情况设计的：

```json
"configurations": {
  "launch": {
    "targetBindings": { "program": "target.executable", "cwd": "target.workingDirectory" },
    "executable": {
      "setting": "program",
      "candidates": [
        "${fileDirectory}/${fileBasenameNoExtension}",
        "${workspaceFolder}/${fileBasenameNoExtension}",
        "${workspaceFolder}/build/${fileBasenameNoExtension}"
      ]
    }
  }
}
```

- `setting` 是你在 `settings[]` 里声明的一个 `file` 类型设置的 id。用户在设置页里选了路径，就用它。
- 用户没选时，Lithe 依次查找 `candidates`（最多 8 个，每个必须以 `${workspaceFolder}/` 或 `${fileDirectory}/` 开头，可以用 `${fileBasenameNoExtension}`，不能含 `..`），用找到的第一个可运行文件。
- 找到的路径会**回显在这个设置里**，用户能看到实际会调试哪个文件。
- 用户选的路径如果不是可运行文件，是错误，**不会**悄悄回落到候选路径。
- `executable` 和 `targetBindings.program: "target.executable"` 必须一起声明，缺一不可。

设置的 `description` 是教用户怎么产生这个程序的地方，写清楚构建命令（例如 `clang -g main.c -o main`）和适配器本身的安装方式。LLDB 插件的 `program` 设置就是这样写的。设置的写法见契约的 `settings` 一节，`scope: "project"`、`effect: "nextRun"` 适合这类“下次运行生效”的路径。

### 5. 调试控制台：提示符、补全和 `console`

Lithe 的调试控制台有输入行和补全弹窗。补全来自适配器的 DAP `completions` 请求；`console` 补充两件事：

- `commandPrefix`：1 到 8 个字符、不含空格。有些适配器把“光秃秃的单词”当表达式（lldb-dap 就是这样），补全第一个词时会混进 `long` 之类的语言关键字。声明前缀（lldb-dap 用反引号 `` ` ``）后，Lithe 只在向适配器请求补全时把它加在输入前面，并把适配器的答案移回去，用户看不到它。
- `commands[]`：适配器不支持 `completions`，或者它的实时答案缺一些命令时，列出调试器命令（`name`，可选的一行 `description`）。名字按原样插入，不能重复，不要在名字里补空格；说明按原样显示，不翻译。

两者至少声明一个。适配器能实时补全时，不要把整个命令集再抄一遍。

### 6. 适配器行为声明与由后端发起调试

宿主从不按 ID 认识某个适配器：适配器特有的行为写在清单里，共享的调试引擎照着执行。`features` 和 `defaults` 是 `debugAdapters` 条目的可选字段，不需要额外的版本。

- `features.hotCodeReplace`：`request` 是宿主发给适配器、让它把改动的代码应用到运行中的被调试程序的 DAP 命令，声明了就表示支持热代码替换；
  `event` 是其 `message` 在调试控制台里显示为进度的 DAP 事件；`changedField`、`errorField` 是响应体里已替换单元的列表和适配器在成功响应里报告的失败信息。
- `features.stepFilters.argument`：适配器接收步进过滤器所用的启动参数名；用户配置里已经设置的同名参数保持不变。
- `defaults.stepFilters`：用户没有配置过滤器时的默认值（`classNameFilters`、`skipSynthetics`、`skipStaticInitializers`、`skipConstructors`、`hideFilteredStackFrames`）。
- 名字写错或使用契约未定义的字段是 `plugin.manifest.invalid`；两者都不声明时，适配器按默认行为运行。完整字段和限制以契约的
  [Adapter features and defaults](../contract/v1/README.md#adapter-features-and-defaults)为准。

插件有后端（`requires.backend`）时，可以由后端决定启动什么调试会话，会话仍由宿主拥有（进程、控制台、停止按钮、会话结束）。方法名前缀是 `lithe/host/`：

| 方法 | API | 权限 | 用途 |
| --- | --- | --- | --- |
| `debug/start` | API-30 | `debug.start` | 在本插件自己声明的适配器上启动 `launch` 或 `attach` 会话，返回 `sessionID`；需要已声明的 `debugAdapters` 条目 |
| `run/start` 的 `debug` 字段 | API-23 | `run.start` | 把一次宿主拥有的运行变成调试会话 |
| `run/stop` | API-23 | `run.start` | 按 `sessionID`（或 `runID`）停止本插件启动的调试会话或运行 |

- `debug/start` 的参数是 `context, label, adapter, request, configuration`。`adapter` 只能是本插件的适配器（本地 ID 或 `<插件 ID>/<本地 ID>`），
  别的插件的适配器是 `plugin.host.notFound`；`request` 必须是该适配器声明过的；`configuration` 是不超过 256 KiB 的 JSON 对象，
  作为 DAP 的 launch 或 attach 参数叠在适配器声明的默认值之上，`noDebug` 和以 `__` 开头的键属于宿主，会被拒绝。适配器本身照常声明 `debug.launch`（`attach` 还要 `debug.attach`）。
- `run/start` 的 `debug` 是 `{configurationType, target}`。`target` 为 `{kind: "connection", port}` 时，进程在 `port` 上等调试器，宿主等它出现后挂接适配器；
  为 `{kind: "launch"}` 时由适配器自己启动进程，程序、参数、目录和环境变量经 `targetBindings` 取自 `process`。
  `process.executable` 只能是清单 `requiresTools` 列出的工具定义 ID，或插件包内的文件。运行结束时宿主发 `run/didExit` 通知。

这三个方法能否调用，以宿主 `getCapabilities` 的返回为准。Lithe 桌面端的能力声明注释把 run 与 debug 服务列为尚未注册，目前不要假定它们可用。
官方 Node、Java、Rust SDK 还没有 `debug/start`、`run/start`、`run/stop` 的封装，需要时可以用 SDK 的通用宿主请求，以完整方法名调用，
例如 Node SDK 的 `backend.host.request(method, params?)`，见 [Host services](../sdk/node/README.md#host-services)。

细节见[调试适配器](../reference/domains/debug-adapters.md)和契约的
[Run and debug sessions](../contract/v1/README.md#run-and-debug-sessions)。
示例：[`debug-adapters/debug-adapter-features.plugin.json`](../contract/v1/fixtures/debug-adapters/debug-adapter-features.plugin.json)、
[`debug-adapters/debug-start.json`](../contract/v1/fixtures/debug-adapters/debug-start.json)、
[`run/run-start.json`](../contract/v1/fixtures/run/run-start.json)。

### 7. 本地验证

1. 对照契约示例 [`contract/v1/fixtures/debug-adapters/debug-adapter.plugin.json`](../contract/v1/fixtures/debug-adapters/debug-adapter.plugin.json)，并用[编辑器 Schema](../README.md#在编辑器里校验-pluginjson) 检查字段。
2. [`contract/v1/fixtures/debug-adapters/debug-adapter-rejections.json`](../contract/v1/fixtures/debug-adapters/debug-adapter-rejections.json) 列出每种拒绝的稳定错误码、出错字段和报告它的层（`schema`、`coreManifest`、`corePlan`、`coreInstall`），写测试时可以直接对照。
3. 在 Lithe 桌面端安装插件，打开一个目标文件，设置断点并启动调试。适配器被拒绝请求时，Lithe 会显示适配器自己的解释，所以适配器返回的错误文字要写得让用户能看懂。
4. 在设置页确认 `executable` 找到的路径被回显，再把它改成一个不存在的文件，确认得到的是错误而不是悄悄换文件。

### 8. 安装后是否需要重启

只贡献调试适配器（以及它声明的语言和设置）的插件，在支持热激活的 Lithe 里安装、更新、回退、卸载、启用、停用后**不需要重启**。是否免重启由宿主按它实际能在运行期登记和撤回的贡献类型决定，不是 `plugin.json` 里的字段，所以插件作者不需要也不能声明。带有其他贡献（语言服务器、构建系统、界面等）的插件仍然在重启后生效。

### 9. 发布

调试适配器插件使用基线 0 的契约，宿主提供了 `debugAdapters` 才会提供安装。

- 插件声明的能力，或所用 SDK 要求的协议（例如 `litheLogging`），还没进入 Lithe 稳定版时，把 GitHub Release 标为**预发布**。Lithe Store 的自动追踪只收正式 Release，预发布需要提交一个手工的索引条目，写法参考官方插件已有的预发布索引条目。
- 稳定版支持插件需要的全部能力和协议后，把 Release 转为正式版，追踪器会自动提交后续版本的索引更新。
- 发布后不要替换或删除资产；索引固定了它的 SHA-256。

### 10. 当前边界

- 调试适配器的宿主实现是 Lithe 桌面端。
- 宿主自己的调试入口对插件适配器只发起 `launch` 请求。`attach` 可以在清单里声明：宿主在构建提供者服务运行的连接调试中选用它（见契约的 [Registry connection debugging](../contract/v1/README.md#registry-connection-debugging)），插件后端也可以用 `debug/start` 或 `run/start` 的 `debug` 字段发起（见第 6 节）；用户手动填写主机和端口的远程 attach 以宿主实际呈现为准，不要依赖。
- 适配器声明是纯数据，没有自定义界面、没有宿主集成点；需要调试之外的能力（构建、测试、运行配置）请另用对应的贡献类型。

---

## English

This guide is for plugin authors who want Lithe to debug a language or runtime. A debug adapter plugin needs only a `plugin.json`: it declares how to start a process that speaks the
[Debug Adapter Protocol (DAP)](https://microsoft.github.io/debug-adapter-protocol/), and Lithe provides the breakpoint, stepping, variable, call stack, and debug console UI and manages the session. The running example is the official
[LLDB plugin](https://github.com/Lithe-IDEA/lithe-plugin-lldb), which debugs C, C++, Objective-C, Rust, and Swift with the `lldb-dap` that ships with Xcode.

The normative field definitions are in the [contract](../contract/v1/README.md#debug-adapters); this guide covers only how to use them and why they are designed this way. For publishing steps, see
[Publishing community plugins](https://github.com/Lithe-IDEA/Lithe-store/blob/main/docs/publishing.md).

> **Host support.** Debug adapters are implemented by the Lithe desktop app, over stdio and over loopback TCP inside a plugin's language server. Which capabilities the host offers is decided by its
> [capability declaration](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go) (the comment above `Declared()`); for capability versions, see the [version notes](../reference/capabilities-and-versions.md).

### 1. What the plugin and the host each own

| Item | The plugin author owns | Lithe owns |
| --- | --- | --- |
| Debug adapter | Choosing it, its license, the pinned version, the launch entry | Starting the process, DAP communication, stopping the session |
| Program to debug | Explaining how it is produced (build command, compiler flags) | Showing the path actually found in settings and passing it to the adapter |
| Debugging UI | Nothing to write | Breakpoints, variables, call stack, debug console and completion |
| Permissions | Declaring `debug.launch` | Asking the user to confirm at install time |
| Activation | Declaring `onDebug:<adapter id>` | Starting the adapter only when a debug session needs it |

Plugins are not loaded into the Lithe process. The manifest is pure data: no resolvers, no workflows, no host integrations, and no `initializationOptions`; writing one is rejected.

### 2. A minimal manifest

```json
{
  "schemaVersion": 0,
  "id": "com.example.demo-debug",
  "displayName": "Demo Debugger",
  "version": "0.1.0",
  "pluginAPIVersion": 0,
  "requires": { "debugAdapters": 0, "languages": 0 },
  "vendor": { "id": "com.example", "displayName": "Example", "signatureRequirement": "none" },
  "permissions": ["debug.launch"],
  "activationEvents": ["onDebug:demo"],
  "languages": [
    { "id": "demo", "displayName": "Demo", "fileExtensions": ["demo"] }
  ],
  "debugAdapters": [
    {
      "id": "demo",
      "displayName": "Demo",
      "requests": ["launch"],
      "selectors": [{ "targetKinds": ["file"], "languageIDs": ["demo"] }],
      "configurations": {
        "launch": {
          "arguments": { "stopOnEntry": false },
          "targetBindings": { "program": "target.path", "cwd": "target.workingDirectory" }
        }
      },
      "launch": {
        "process": { "launch": { "kind": "systemCommand", "commands": ["demo-dap"] } },
        "transport": "stdio"
      }
    }
  ]
}
```

Key points:

- `pluginAPIVersion` and `schemaVersion` are both `0`. `requires` must list exactly the capabilities you use, each at `0`: list `debugAdapters` for the adapters, `languages` when you declare `languages`, and `backend` when the backend starts debug sessions (see section 6).
- Every adapter needs an `onDebug:<id>` activation event and the `debug.launch` permission. `debug.attach` is needed (and allowed) only when the adapter lists `attach`.
- `selectors` decide on which targets Lithe picks this adapter automatically: `targetKinds` are target kinds and `languageIDs` are languages. Without `selectors`, the adapter can only be chosen explicitly by the user.
- `languages[]` declares the languages the adapter serves. If another plugin already provides the language (for example Rust), your declaration only adds the ability to debug it and merges in the file extensions you declare; the language name, language ID, and language server stay the same, and installing the debugger does not restart a running language server.
- `targetBindings` fills facts about the debug target into the adapter's launch arguments: `program` ← the target path, `cwd` ← the working directory, `processId` ← the process ID. Only fields missing from `arguments` are filled; defaults you wrote explicitly are never overwritten. When the target lacks a fact a binding reads, Lithe refuses the launch and says what is missing.
- `configurationTypes` declares the `launch.json` `type` values the adapter matches, used to import the user's existing VS Code-style configurations.

### 3. Launching the adapter

`launch.process` has three common forms:

| Form | When to use it |
| --- | --- |
| `{"launch": {"kind": "packaged", "path": …}}` | The adapter's license allows distribution and you can provide a build for each platform; the executable installs with the plugin package |
| `{"launch": {"kind": "systemCommand", "commands": ["lldb-dap"]}}` | The adapter is already installed on the user's machine (for example with the Xcode command line tools); the host resolves the command name with its own tool discovery. The Lithe desktop app currently searches only PATH, and a launch that depends on a project toolchain or a user override is refused |
| `{"launch": {"kind": "runSelectedRuntime", "tool": "<tool-definition ID>"}}` | The adapter is a Python, Java, or Node.js program started with the runtime the user selected; `tool` must be a tool-definition ID listed in the top-level `requiresTools` |

For the rules on `runSelectedRuntime.tool`, see [Tool references](../contract/v1/README.md#tool-references) in the contract.

`launch.transport` is `stdio` or TCP (`tcpAnnounced`: the adapter prints its port in its output; `tcpPort`: a fixed port). TCP allows only loopback addresses.

The plugin package and the Lithe install directory are read-only at run time. An adapter must not write logs, indexes, or state there, or it breaks code signing and delta updates. The host keeps the adapter's state in its own writable directory, never in the plugin package or the install directory.

### 4. Languages that compile before debugging: `executable`

Most DAP adapters take "the program to run" directly, but in languages such as C, C++, and Swift the user opens a **source file** while the thing to debug is the compiled output.
`configurations.launch.executable` is designed for this case:

```json
"configurations": {
  "launch": {
    "targetBindings": { "program": "target.executable", "cwd": "target.workingDirectory" },
    "executable": {
      "setting": "program",
      "candidates": [
        "${fileDirectory}/${fileBasenameNoExtension}",
        "${workspaceFolder}/${fileBasenameNoExtension}",
        "${workspaceFolder}/build/${fileBasenameNoExtension}"
      ]
    }
  }
}
```

- `setting` is the ID of a `file` setting you declare in `settings[]`. When the user picks a path on the settings page, that path is used.
- When the user has picked nothing, Lithe tries `candidates` in order (at most 8; each must start with `${workspaceFolder}/` or `${fileDirectory}/`, may use `${fileBasenameNoExtension}`, and must not contain `..`) and uses the first runnable file it finds.
- The path it finds is **shown back in that setting**, so the user can see which file will actually be debugged.
- A user-picked path that is not a runnable file is an error; Lithe does **not** quietly fall back to the candidates.
- `executable` and `targetBindings.program: "target.executable"` must be declared together.

The setting's `description` is where you teach the user how to produce the program: give the build command (for example `clang -g main.c -o main`) and how to install the adapter itself. The LLDB plugin's `program` setting is written this way. For writing settings, see the contract's `settings` section; `scope: "project"` and `effect: "nextRun"` suit a path like this that takes effect on the next run.

### 5. The debug console: prompt, completion, and `console`

Lithe's debug console has an input line and a completion popup. Completions come from the adapter's DAP `completions` request; `console` adds two things:

- `commandPrefix`: 1 to 8 characters with no spaces. Some adapters treat a bare word as an expression (lldb-dap does), so completing the first word mixes in language keywords such as `long`. With a declared prefix (lldb-dap uses a backtick `` ` ``), Lithe adds it in front of the input only when asking the adapter for completions and moves the adapter's answers back, so the user never sees it.
- `commands[]`: when the adapter does not support `completions`, or its live answers miss some commands, list the debugger commands (`name`, with an optional one-line `description`). Names are inserted as written, must be unique, and must not have padding spaces; descriptions are shown as written and are not translated.

Declare at least one of the two. When the adapter completes live, do not copy its whole command set as well.

### 6. Adapter behavior declarations and backend-started debugging

A host never recognizes an adapter by its ID: what is specific to an adapter is written in the manifest, and the shared debug engine follows it. `features` and `defaults` are optional fields of a `debugAdapters` entry and need no version of their own.

- `features.hotCodeReplace`: `request` is the DAP command the host sends to have the adapter apply changed code to the running debuggee, and declaring it means the adapter supports hot code replace;
  `event` is a DAP event whose `message` the Debug Console shows as progress; `changedField` and `errorField` are the response-body fields for the list of replaced units and for a failure the adapter reports in a successful response.
- `features.stepFilters.argument`: the launch argument through which the adapter receives stepping filters; an argument the user's configuration already sets is kept.
- `defaults.stepFilters`: the filters used when the user configured none (`classNameFilters`, `skipSynthetics`, `skipStaticInitializers`, `skipConstructors`, `hideFilteredStackFrames`).
- A malformed name, or a field the contract does not define, is `plugin.manifest.invalid`; when neither is declared, the adapter runs with default behavior. For the full fields and limits, go by
  [Adapter features and defaults](../contract/v1/README.md#adapter-features-and-defaults) in the contract.

A plugin with a backend (`requires.backend`) can decide which debug session to start, while the host still owns the session (the process, the console, the Stop button, and the end of the session). The method prefix is `lithe/host/`:

| Method | API | Permission | Purpose |
| --- | --- | --- | --- |
| `debug/start` | API-30 | `debug.start` | Start a `launch` or `attach` session on an adapter the plugin declares itself and return a `sessionID`; needs a declared `debugAdapters` entry |
| `debug` field of `run/start` | API-23 | `run.start` | Turn a host-owned run into a debug session |
| `run/stop` | API-23 | `run.start` | Stop a debug session (by `sessionID`) or a run (by `runID`) the plugin started |

- `debug/start` takes `context, label, adapter, request, configuration`. `adapter` must be one of the plugin's own adapters (its local ID or `<plugin ID>/<local ID>`);
  an adapter of another plugin is `plugin.host.notFound`. `request` must be one the adapter declares. `configuration` is a JSON object of at most 256 KiB that is passed to the adapter as the DAP launch or attach arguments
  on top of its declared defaults; `noDebug` and keys starting with `__` belong to the host and are refused. The adapter itself still declares `debug.launch` (plus `debug.attach` for `attach`).
- The `debug` field of `run/start` is `{configurationType, target}`. With `target` as `{kind: "connection", port}`, the process listens for a debugger on `port` and the host waits for it and attaches the adapter;
  with `{kind: "launch"}`, the adapter starts the process itself and takes the program, arguments, directory, and environment from `process` through its `targetBindings`.
  `process.executable` can only be a tool-definition ID listed in the manifest's `requiresTools`, or a file inside the plugin package. The host sends the `run/didExit` notification when the run ends.

Whether these three methods can be called is decided by what the host returns from `getCapabilities`. The Lithe desktop app's capability declaration comment lists the run and debug services as not yet registered, so do not assume they are available.
The official Node, Java, and Rust SDKs do not wrap `debug/start`, `run/start`, or `run/stop` yet; where needed, call them with the SDK's generic host request using the full method name,
for example `backend.host.request(method, params?)` in the Node SDK, see [Host services](../sdk/node/README.md#host-services).

For details, see [Debug adapters](../reference/domains/debug-adapters.md) and
[Run and debug sessions](../contract/v1/README.md#run-and-debug-sessions) in the contract.
Examples: [`debug-adapters/debug-adapter-features.plugin.json`](../contract/v1/fixtures/debug-adapters/debug-adapter-features.plugin.json),
[`debug-adapters/debug-start.json`](../contract/v1/fixtures/debug-adapters/debug-start.json), and
[`run/run-start.json`](../contract/v1/fixtures/run/run-start.json).

### 7. Local verification

1. Compare with the contract example [`contract/v1/fixtures/debug-adapters/debug-adapter.plugin.json`](../contract/v1/fixtures/debug-adapters/debug-adapter.plugin.json) and check fields with the [editor schema](../README.md#validate-pluginjson-in-your-editor).
2. [`contract/v1/fixtures/debug-adapters/debug-adapter-rejections.json`](../contract/v1/fixtures/debug-adapters/debug-adapter-rejections.json) lists, for each rejection, the stable error code, the offending field, and the layer that reports it (`schema`, `coreManifest`, `corePlan`, `coreInstall`); compare against it directly when writing tests.
3. Install the plugin in the Lithe desktop app, open a target file, set a breakpoint, and start debugging. When the adapter rejects a request, Lithe shows the adapter's own explanation, so write the adapter's error text for users to understand.
4. On the settings page, confirm that the path `executable` found is shown, then change it to a file that does not exist and confirm you get an error rather than a quietly swapped file.

### 8. Whether installing needs a restart

A plugin that contributes only debug adapters (and the languages and settings it declares) needs **no restart** after install, update, rollback, uninstall, enable, or disable in a Lithe that supports hot activation. The host decides this from the contribution types it can actually register and withdraw at run time; it is not a `plugin.json` field, so plugin authors neither need to nor can declare it. Plugins with other contributions (language servers, build systems, UI, and so on) still take effect after a restart.

### 9. Publishing

A debug adapter plugin uses the baseline-0 contract, and the marketplace offers it for installation only on a host that provides `debugAdapters`.

- Mark your GitHub Release as a **prerelease** while a capability the plugin declares, or a protocol its SDK requires (for example `litheLogging`), is not yet in a stable Lithe release. Lithe Store's automatic tracking only picks up full Releases, so a prerelease needs a hand-written index entry; follow the existing prerelease index entries of the official plugins.
- Once a stable release supports every capability and protocol the plugin needs, convert the Release to a full release, and the tracker submits index updates for later versions automatically.
- Do not replace or delete assets after publishing; the index pins their SHA-256.

### 10. Current limits

- The host implementation of debug adapters is the Lithe desktop app.
- The host's own debug entry issues only `launch` requests to plugin adapters. `attach` can be declared in the manifest: the host chooses it for connection debugging of build-provider service runs (see [Registry connection debugging](../contract/v1/README.md#registry-connection-debugging) in the contract), and a plugin backend can start it with `debug/start` or the `debug` field of `run/start` (see section 6). Remote attach with a user-entered host and port follows what the host actually presents; do not rely on it.
- An adapter declaration is pure data, with no custom UI and no host integration points; for capabilities beyond debugging (build, test, run configurations), use the matching contribution types.
