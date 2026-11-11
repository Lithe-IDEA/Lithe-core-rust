# 原生构建、测试与运行准备 / Native build, test, and run preparation

[中文](#中文) · [English](#english)

## 中文

插件可以只提供工作区后端，不创建网页。用户从原生工具窗口执行构建、发现与运行测试、准备应用启动；宿主继续管理工作区、取消、输出和应用进程。

这些入口都属于 `buildProviders`（版本 `0`）：清单要求 `requires.buildProviders: 0`，进程协议 `litheBuild` 的版本就是 `buildProviders` 的版本。Node.js SDK 0.5.0 和 Java SDK 0.5.0 实现了全部入口；Rust SDK 0.4.0 没有构建提供者接口。能力定义见[构建提供者](../reference/domains/build-providers.md)，字段和错误码以[契约](../contract/v1/README.md#build-providers)为准。

> **宿主支持。** Lithe 桌面端声明 `buildSystems` 和 `buildProviders`：构建窗口发现构建实例、加载提供者模型、解析可执行文件候选并运行任务。构建提供者的运行准备、测试、调试准备、model inputs、原生配置、内联测试报告和运行时调试，桌面端尚未呈现，见
> [`pluginhost.go`](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go) 中 `Declared()` 上方的注释。下文的入口因此由 Core 与 SDK 的端到端测试验证，插件作者在桌面端界面里暂时看不到测试、运行和调试这几类入口的效果。

### 选择入口

| 工作 | Node.js SDK | Java SDK | 清单里的声明 |
| --- | --- | --- | --- |
| 执行有界任务 | `backend.build.execute` | `onExecute` | 任务 `source: "provider"` |
| 运行精确测试 | `backend.build.test` | `onTest` | `test.source: "provider"` |
| 发现可运行的用例 | `backend.build.discoverTests` | `onDiscoverTests` | `test.discovery: true` |
| 准备应用启动 | `backend.build.prepareRun` | `onPrepareRun` | `run[].source: "provider"` |
| 准备原生程序调试 | `backend.build.prepareDebug` | `onPrepareDebug` | `run[].debug: {"adapter": "native"}` |

安装时检查清单的能力版本；启动时 SDK 只在注册了构建处理器、且宿主提供了 `litheBuild` 版本 `0` 时才协商构建协议。没有协商成功时，构建方法在进入处理器之前被拒绝；不会改发私有函数，也不会展开成另一条宿主命令。Node.js 用 `backend.buildVersion` 查看结果（协商到为 `0`，否则为 `null`），Java 用 `backend.negotiated().buildVersion()`。

处理器必须在 `listen` 前注册。模型、依赖和测试发现回复由 SDK 补齐对应的 `format` 和 `version: 0`；执行回复是 `{ exitCode, output }`，启动准备回复是完整的可移植计划。Core 继续检查选择器、工作区路径和大小限制。

### 所有权与清理

插件从其选定的上游引擎读取当前工程、测试和运行事实。发现结果中的每个用例都必须能通过同一套件、用例、工程与目标选择器精确执行；处理器重新核对选择，不把失效用例当成“运行全部”。不要在宿主或 SDK 里再扫描 Java 注解、拼 classpath 或解释 Maven 模型。

构建与测试处理器使用调用上下文的 `run` 启动工具，取消或调用结束会结束它拥有的工具树。工具正常返回非零码仍是执行结果；异常属于协议或准备失败。不要启动脱离调用的进程，或用一条私有函数代替这些领域入口。

运行准备先等待上游同步和必要构建，再返回 `executable`、原始 `arguments` 数组、工作区相对 `workingDirectory` 和 `environment`。`executable.kind` 只能是 `command`（宿主解析的裸命令名）或 `toolchain`（宿主工具链标识）。处理器不启动长期运行的应用；宿主校验后启动、停止并显示应用输出。构建失败、取消或旧结果失效时不能启动应用。

执行与测试请求的后端拒绝由 Core 报告为 `plugin.build.failed`。运行准备的 RPC 错误保留通用 LSP `serverError` 分类，不是带退出码的执行结果，也不使用私有函数的 `pluginErrorCode`。宿主在两种失败下都不能启动应用或改走旧命令。

执行、测试、运行与调试准备上下文携带 Core 分配的 `operationID`，进度自动带上它。Node.js 使用 `context.progress({ message, percentage, output })`；Java 使用 `context.progress(message, percentage, output)`。消息上限 4096 UTF-8 字节，单次输出上限 64 KiB，百分比限制在 0–100；取消或结束后的报告被丢弃。模型、依赖和发现不提供执行进度。

状态写入宿主提供的可写目录；安装后的插件包只读。`prepareRun` 的结果不能变成跨重启缓存的 classpath 或命令：宿主保存入口选择器，重启重新准备。

### 可运行示例

两套示例使用相同的虚构构建引擎，工程事实来自工作区的 `lithe-sample.json`。这是协议教学用的引擎，不是真实的 Maven 或 Java 构建。

- [Node.js 示例](../sdk/node/examples/sample-native-build/backend/main.mjs)及其[清单](../sdk/node/examples/sample-native-build/plugin.json)，清单要求 `buildProviders`、`buildSystems` 和 `debugAdapters`（版本都是 `0`）。开发时从源码引入 SDK；分发时将 `src/` 放入 `backend/vendor/`，把导入改为 `./vendor/index.mjs`。端到端测试按这种结构打包验证。
- [Java 示例](../sdk/java/examples/sample-native-build/src/main/java/org/example/samplenativebuild/SampleNativeBuild.java)及其[清单](../sdk/java/examples/sample-native-build/plugin.json)。在仓库根运行 `mvn -B -ntp -f shared/plugins/sdk/java/pom.xml package`，将 `shared/plugins/sdk/java/examples/sample-native-build/target/sample-native-build.jar` 放到包内 `backend/`。SDK 已 shade 进同一 JAR。

两个 SDK 的完整用法见各自 README：[Node.js](../sdk/node/README.md#native-build-workflows)，[Java](../sdk/java/README.md)。

示例工程包含构建命令、精确用例及应用参数，例如：

```json
{
  "projects": [{
    "name": "app", "path": "app",
    "build": ["node", "-e", "console.log('built')"],
    "tests": [{ "suite": "Suite", "case": "exact case",
      "command": ["node", "-e", "console.log('passed')", "--"] }],
    "run": { "command": "node",
      "arguments": ["-e", "console.log(process.argv.slice(1))", "--"] }
  }]
}
```

模型的路径相对构建实例，后续请求的工程和目标选择器相对整个工作区；示例显式转换这两种表示，不把嵌套实例当成根工程。

`app` 目录需存在，测试工具需可执行。示例重新读取配置、核对工程、目标和测试，并解析真实路径以拒绝工作区外目录；实际插件应调用其上游工程引擎，而非复制这份虚构引擎。

### 验证

下面的命令都在仓库根运行：

```sh
node --test shared/plugins/sdk/node/test/*.test.mjs
npm ci --prefix shared/plugins/sdk/node --ignore-scripts
npm run --prefix shared/plugins/sdk/node test:types
mvn -B -ntp -f shared/plugins/sdk/java/pom.xml verify
node --test shared/plugins/scripts/*.test.mjs

# 端到端：对当前工作树的 Core 运行，总是包含原生构建工作流。
node shared/plugins/scripts/sdk-e2e.mjs
```

`sdk-e2e.mjs` 先构建三个 SDK 的示例后端，再用 `cargo test -p lithe-core --features plugin-sdk-e2e` 运行 `plugin_sdk_node`、`plugin_sdk_java`、`plugin_sdk_rust` 和 `plugin_sdk_build`，被测对象是当前工作树里的 Core，所以改 Core 与改 SDK 或契约可以在同一个 PR 里一起验证。运行需要 Rust、Node.js 20.3 或更高版本、Java（用 `LITHE_PLUGIN_SDK_JAVA_RUNTIME` 指定，或放在 `PATH` 上的 `java`）和 Maven。

这些测试通过真实 Core 规划与解码，覆盖构建非零退出码、精确筛选、运行前构建、准备计划、进度、取消后的工具清理、并行工作区隔离，以及包含 SDK 和 JAR 的完整包文件不变。它们不代替原生界面、真实 Maven/JDT 工程或桌面端实机验证。

若复用预构建的示例，分别设置 `LITHE_PLUGIN_SDK_JAVA_SAMPLE`、`LITHE_PLUGIN_SDK_JAVA_NATIVE_SAMPLE` 和 `LITHE_PLUGIN_SDK_RUST_SAMPLE`。Java 示例用 JDK 21 编译，产物面向 Java 8；可用 `LITHE_PLUGIN_SDK_JAVA_RUNTIME` 指定实际运行时。

调试适配器、长期服务和跨插件协作有各自的宿主契约；这些处理器不开放跨包私有函数或反向 `lithe/*` 调用。

### 原生程序调试准备

运行声明加 `debug: {"adapter":"native"}`，同一插件提供 `native` 适配器；清单要求 `buildProviders: 0` 与 `debugAdapters: 0`，并声明 `debug.launch` 权限。适配器声明 launch 的进程和 transport，使用 program、cwd、args、env 的目标绑定。不要用跨包 ID，也不要在后端返回 DAP 进程、端口或任意调试参数。

`prepareDebug` 先完成构建，再返回 `{program, arguments, workingDirectory, environment}`。程序是当前实例内的已有文件，路径从工作区起算；宿主校验路径和可执行权限，再复用现有的调试适配器注册表。取消、错误和过期结果都不能启动调试。Node.js 和 Java 的 sample-native-build 从虚构引擎的 `run.debugProgram` 读取程序，参数使用当前请求的 userArguments；示例的 `example-dap` 是占位适配器命令，真实插件应打包符合其许可的成熟适配器。

端到端示例验证两个 SDK 的真实进程、运行前构建、原生调试准备前构建、Core 路径解码、进度及失败不返回目标。它不启动真实调试器，也不代表 Java/JDT 或跨插件协作已经完成。

---

## English

A plugin can provide only a workspace backend and create no web page. Users build, discover and run tests, and prepare application launches from native tool windows; the host keeps managing the workspace, cancellation, output, and application processes.

These entry points all belong to `buildProviders` (version `0`): the manifest requires `requires.buildProviders: 0`, and the version of the `litheBuild` process protocol is the version of `buildProviders`. Node.js SDK 0.5.0 and Java SDK 0.5.0 implement every entry point; Rust SDK 0.4.0 has no build-provider interface. For the capability see [Build providers](../reference/domains/build-providers.md); the [contract](../contract/v1/README.md#build-providers) is normative for fields and error codes.

> **Host support.** The Lithe desktop declares `buildSystems` and `buildProviders`: the Build window detects build instances, loads provider models, resolves executable candidates, and runs tasks. Build-provider run preparation, tests, debug preparation, model inputs, native configuration, inline test reports, and runtime debugging are not presented on the desktop yet; see the comment above `Declared()` in
> [`pluginhost.go`](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go). The entry points below are therefore verified by the Core and SDK end-to-end tests, and a plugin author cannot yet see the effect of the test, run, and debug entry points in the desktop UI.

### Choosing an entry point

| Work | Node.js SDK | Java SDK | Manifest declaration |
| --- | --- | --- | --- |
| Execute a bounded task | `backend.build.execute` | `onExecute` | task `source: "provider"` |
| Run exact tests | `backend.build.test` | `onTest` | `test.source: "provider"` |
| Discover runnable cases | `backend.build.discoverTests` | `onDiscoverTests` | `test.discovery: true` |
| Prepare an application launch | `backend.build.prepareRun` | `onPrepareRun` | `run[].source: "provider"` |
| Prepare native program debugging | `backend.build.prepareDebug` | `onPrepareDebug` | `run[].debug: {"adapter": "native"}` |

The manifest's capability versions are checked at install time; at startup the SDK negotiates the build protocol only when build handlers are registered and the host offers `litheBuild` version `0`. Without a negotiated protocol, a build method is rejected before it reaches the handler; it is never rerouted to a private function or expanded into a different host command. Read the result with `backend.buildVersion` in Node.js (`0` when negotiated, otherwise `null`) or `backend.negotiated().buildVersion()` in Java.

Register handlers before `listen`. For model, dependency, and test discovery replies, the SDK fills in the matching `format` and `version: 0`; an execution reply is `{ exitCode, output }`, and a launch preparation reply is a complete portable plan. Core still checks selectors, workspace paths, and size limits.

### Ownership and cleanup

The plugin reads the current project, test, and run facts from the upstream engine it chose. Every case in a discovery result must be exactly runnable through the same suite, case, project, and target selectors; the handler checks the selection again and never treats a stale case as "run all". Do not scan Java annotations, assemble a classpath, or interpret a Maven model in the host or SDK.

Build and test handlers start tools through the call context's `run`; cancellation or the end of the call ends the tool tree it owns. A tool that returns a non-zero code normally is still an execution result; an exception is a protocol or preparation failure. Do not start processes that outlive the call, and do not replace these domain entry points with a private function.

Run preparation first waits for upstream sync and any required build, then returns `executable`, the raw `arguments` array, a workspace-relative `workingDirectory`, and `environment`. `executable.kind` is only `command` (a bare command name the host resolves) or `toolchain` (a host toolchain identifier). The handler does not start a long-running application; the host validates the plan, then starts and stops the application and shows its output. A build failure, cancellation, or stale result must not start the application.

Core reports a backend rejection of an execute or test request as `plugin.build.failed`. An RPC error from run preparation keeps the generic LSP `serverError` classification; it is not an execution result with an exit code and does not use the private-function `pluginErrorCode`. In both failures the host must not start the application or fall back to an old command.

Execute, test, run preparation, and debug preparation contexts carry an `operationID` assigned by Core, and progress carries it automatically. Node.js uses `context.progress({ message, percentage, output })`; Java uses `context.progress(message, percentage, output)`. Messages are limited to 4096 UTF-8 bytes, each output chunk to 64 KiB, and percentages are clamped to 0–100; reports after cancellation or completion are dropped. Model, dependency, and discovery requests have no execution progress.

State goes into the writable directories the host provides; an installed plugin package is read-only. A `prepareRun` result must not become a classpath or command cached across restarts: the host stores the entry selector and prepares again after a restart.

### Runnable examples

Both examples use the same fictional build engine, whose project facts come from `lithe-sample.json` in the workspace. It is an engine for teaching the protocol, not a real Maven or Java build.

- The [Node.js example](../sdk/node/examples/sample-native-build/backend/main.mjs) and its [manifest](../sdk/node/examples/sample-native-build/plugin.json); the manifest requires `buildProviders`, `buildSystems`, and `debugAdapters` (all version `0`). During development it imports the SDK from source; for distribution, put `src/` into `backend/vendor/` and change the import to `./vendor/index.mjs`. The end-to-end tests package and verify that layout.
- The [Java example](../sdk/java/examples/sample-native-build/src/main/java/org/example/samplenativebuild/SampleNativeBuild.java) and its [manifest](../sdk/java/examples/sample-native-build/plugin.json). From the repository root run `mvn -B -ntp -f shared/plugins/sdk/java/pom.xml package` and put `shared/plugins/sdk/java/examples/sample-native-build/target/sample-native-build.jar` into the package's `backend/`. The SDK is already shaded into the same JAR.

Each SDK's README has its full usage: [Node.js](../sdk/node/README.md#native-build-workflows), [Java](../sdk/java/README.md).

The example project contains a build command, exact cases, and application arguments, for example:

```json
{
  "projects": [{
    "name": "app", "path": "app",
    "build": ["node", "-e", "console.log('built')"],
    "tests": [{ "suite": "Suite", "case": "exact case",
      "command": ["node", "-e", "console.log('passed')", "--"] }],
    "run": { "command": "node",
      "arguments": ["-e", "console.log(process.argv.slice(1))", "--"] }
  }]
}
```

Model paths are relative to the build instance, while project and target selectors in later requests are relative to the whole workspace; the example converts between the two explicitly and never treats a nested instance as the root project.

The `app` directory must exist and the test tool must be executable. The example rereads its configuration, checks the project, target, and test, and resolves real paths to reject directories outside the workspace. A real plugin should call its upstream project engine instead of copying this fictional one.

### Verification

Run these from the repository root:

```sh
node --test shared/plugins/sdk/node/test/*.test.mjs
npm ci --prefix shared/plugins/sdk/node --ignore-scripts
npm run --prefix shared/plugins/sdk/node test:types
mvn -B -ntp -f shared/plugins/sdk/java/pom.xml verify
node --test shared/plugins/scripts/*.test.mjs

# End to end: runs against this working tree's Core and always includes the native build workflows.
node shared/plugins/scripts/sdk-e2e.mjs
```

`sdk-e2e.mjs` first builds the three SDKs' sample backends, then runs `plugin_sdk_node`, `plugin_sdk_java`, `plugin_sdk_rust`, and `plugin_sdk_build` with `cargo test -p lithe-core --features plugin-sdk-e2e`. The code under test is the Core in the working tree, so a change to Core and a change to an SDK or the contract are verified together in one PR. It needs Rust, Node.js 20.3 or newer, Java (set with `LITHE_PLUGIN_SDK_JAVA_RUNTIME`, or `java` on `PATH`), and Maven.

These tests plan and decode through a real Core and cover a non-zero build exit code, exact filtering, build before run, preparation plans, progress, tool cleanup after cancellation, isolation between parallel workspaces, and an unchanged complete package including the SDK and JAR. They do not replace verification of the native UI, real Maven/JDT projects, or the desktop on a real machine.

To reuse prebuilt examples, set `LITHE_PLUGIN_SDK_JAVA_SAMPLE`, `LITHE_PLUGIN_SDK_JAVA_NATIVE_SAMPLE`, and `LITHE_PLUGIN_SDK_RUST_SAMPLE`. The Java examples compile with JDK 21 and target Java 8; `LITHE_PLUGIN_SDK_JAVA_RUNTIME` selects the runtime that actually runs them.

Debug adapters, long-running services, and cross-plugin collaboration have host contracts of their own; these handlers open no cross-package private functions or reverse `lithe/*` calls.

### Native program debug preparation

Add `debug: {"adapter":"native"}` to the run declaration and provide a `native` adapter from the same plugin; the manifest requires `buildProviders: 0` and `debugAdapters: 0` and declares the `debug.launch` permission. The adapter declares the launch process and transport and uses the target bindings for program, cwd, args, and env. Do not use a cross-package ID, and do not return a DAP process, port, or arbitrary debug arguments from the backend.

`prepareDebug` finishes the build first and then returns `{program, arguments, workingDirectory, environment}`. The program is an existing file inside the current instance, with a path relative to the workspace; the host checks the path and execute permission and then reuses the existing debug adapter registry. Cancellation, errors, and stale results must not start debugging. The Node.js and Java sample-native-build examples read the program from the fictional engine's `run.debugProgram` and use the current request's userArguments as arguments. The examples' `example-dap` is a placeholder adapter command; a real plugin should package a mature adapter whose license allows it.

The end-to-end example verifies real processes for both SDKs, build before run, build before native debug preparation, Core path decoding, progress, and that failures return no target. It does not start a real debugger and does not mean Java/JDT or cross-plugin collaboration is done.
