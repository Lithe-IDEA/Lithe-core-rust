# 编辑器、文档与预览 / Editor, documents, and previews

[中文](#中文) · [English](#english)

## 中文

插件后端通过插件函数协议读取打开的文档和选区、做版本化编辑、打开和定位文件、打开 HTML 预览或外部浏览器，并在编辑器里注册自己的动作和语言功能 Provider（定义、悬浮、补全等），还能把没有对应文件的文档（例如反编译出的库类源码）按 URI scheme 交给宿主打开。这些都是宿主方法，由 Rust Core 按权限校验、宿主执行；插件不直接操作编辑器。

| 项目 | 内容 |
| --- | --- |
| 清单字段 | 没有专用字段；用 `backend` 加下面的权限 |
| 能力 | `requires.backend: 0`，见[能力与协议版本](../capabilities-and-versions.md) |
| 协议 | `litheFunctions`，版本 `0` |
| API 编号 | API-09、10、12、15、16、25、27，见 [API 总表](../api-catalog.md) |
| 权限 | `documents.read`、`editor.edit`、`editor.open`、`preview.open`、`browser.open`、`editor.actions`（同时需要 `documents.read`）、`editor.providers`（同时需要 `documents.read`）、`documents.contentProvider`（只授予 `documents/registerContentProvider`，不授予读写文档） |
| 激活事件 | 与后端相同：`onWorkspaceOpen`、`onCommand:<id>` 或网页调用 |
| Lithe 桌面端 | 已声明 `backend`；各方法是否可用由后端用 `getCapabilities` 在运行时发现，见[宿主现状](#宿主现状) |

### 方法对照

| 方法 | 权限 | API | 说明 |
| --- | --- | --- | --- |
| `documents/list`、`documents/getSnapshot`、`editor/getActive`、`editor/getSelections` | `documents.read` | API-09 | 包含未保存状态、版本、编码、换行符；快照最多 128 KiB |
| `editor/applyEdits`、`editor/replaceSelections`、`editor/insertText` | `editor.edit` | API-10 | 校验文档版本；输入法组字中和只读文档会被拒绝；一批编辑是一次撤销 |
| `editor/open`、`editor/reveal` | `editor.open` | API-15 | 返回 `documentID` 和版本；传入范围时只把光标放到起点 |
| `preview/openHTML` | `preview.open` | API-16 | `editorAndPreview` 或 `previewOnly` |
| `browser/openExternal` | `browser.open` | API-16 | HTTP(S) 地址或已保存的 HTML |
| `editor/registerActions` | `editor.actions` + `documents.read` | API-27 | 先用 `getCapabilities` 发现，再按工作区上下文注册；动作绑定普通函数和文件范围，不依赖 LSP、构建或运行。动作可以带多个图标、最多 4 个 `alternatives` 和点击结果 `reveal`，可用图标由宿主在 `services.editor.options.actionIcons` 中声明 |
| `editor/registerProviders` | `editor.providers` + `documents.read` | API-25 | 先用 `getCapabilities` 发现方法和 `services.editor.options.providerTypes`，再按工作区上下文注册；把 `definition`、`hover`、`completion`、`codeLens`、`foldingRange`、`semanticTokens`、`documentLink` 这 7 种类型绑定到普通函数，不在 `providerTypes` 中的类型返回 `unsupportedOption`；语言服务器的结果优先，插件结果只补充；预算由宿主决定 |

所有方法都要带 `context: {contextID, generation}`；除 `documents/registerContentProvider` 外，远程工作区上下文返回 `plugin.host.authorityUnsupported`。`documents/registerContentProvider` 见下一节。

### 编辑器动作与语言功能 Provider

| 项目 | 规则 |
| --- | --- |
| 动作形状 | 后端的采集函数返回 `{actions: [{id, range, title, icon, invoke}]}`：位置是 1 起算的 UTF-16，范围半开且不能切开代理对；至多 1000 个动作、序列化后至多 1 MiB，格式错误的结果整体被拒绝；同一行的动作共用一个菜单 |
| 图标 | `icon` 是宿主声明的展示标记，图标只表示一张图，不表示操作：测试图标不会让动作变成测试，宿主也不会因为图标启动任何东西 |
| 采集与点击 | 采集只读，不能发网络请求或执行业务；编辑会立即让旧动作失效并取消旧的采集；点击时宿主再次校验文档、上下文、会话和注册。点击要运行或调试代码，应在 `getCapabilities` 列出 `run/start`、`debug/start` 后通过这两个方法发起，让运行出现在宿主的 Run 和 Debug 窗口里，见[调试适配器](debug-adapters.md) |
| 动作校验 | 共享的 Core 命令 `plugins.editorActions.decode`，请求带 `version`（必须为 `0`）、`kind`（`registration`、`actions` 或 `invokeResult`）、宿主声明的 `icons` 和 `sourceBytes` |
| Provider 合并 | 语言服务器的结果在先：悬浮段落和补全项跟在语言服务器之后，定义把插件位置加到服务器的位置上，代码镜头、折叠范围和链接直接加入，语义标记只用于语言服务器没有语义标记的文件；合并由 Core 命令 `plugins.editorProviders.decode`（`kind: "mergeDefinitions"`）完成，所以每个宿主合并结果一致 |
| 预算 | 悬浮和补全 400 ms，定义 1500 ms，其余 2000 ms；迟到的或属于旧文档版本的答案被无声丢弃；注册不能自定预算 |
| 没有这些方法的宿主 | `getCapabilities` 没有列出 `editor/registerActions` 或 `editor/registerProviders` 时宿主不呈现对应功能，插件仍保留自己的命令和窗口入口；不认识 `editor.actions` 或 `editor.providers` 权限的宿主拒绝安装该包，不会悄悄授予 |

### 内容 Provider（API-12）

语言服务器可能给出没有文件的文档（例如库类的反编译源码）。插件把这些文档当作一个 URI scheme 提供，用 `documents/registerContentProvider`，权限 `documents.contentProvider`（需要 `backend`）。

| 方法 | 权限 | API | 参数 | 结果 |
| --- | --- | --- | --- | --- |
| `documents/registerContentProvider` | `documents.contentProvider` | API-12 | `{context, scheme, function}` | `{subscriptionID}` |

- 注册只记录绑定，不会调用后端；`function` 必须已经在当前会话里注册。用已有的 `subscriptions/dispose` 撤销。
- `scheme` 是 1 到 64 个小写 ASCII 字母、数字、`+`、`-` 或 `.`，以字母开头。宿主或平台自己解析的 scheme 会被拒绝：`about`、`blob`、`data`、`file`、`ftp`、`http`、`https`、`javascript`、`lithe`、`mailto`、`untitled`、`ws`、`wss`。scheme 不合法、保留或 `function` 不是函数 ID，返回 `plugin.host.invalidParams`。
- 每个工作区窗口里每个 scheme 只能有一个 Provider；再注册同一个 scheme 返回 `plugin.host.preconditionFailed`。
- 打开 URI 带这个 scheme 的文档时，宿主用工作区 `context` 和 `{uri}` 通过 `lithe/function/invoke` 调用该函数。函数返回字符串或 `{content: string}`，最多 1 MiB，文档以只读方式打开；其他返回、失败或超时（宿主等 30 秒）都会向用户显示错误，不打开任何文档。
- Provider 是文本的唯一来源：文档关闭后宿主不保留副本，重新打开时再问一次。会话结束或上下文关闭会撤销 Provider。
- 只有 `documents.contentProvider` 授予这个方法；聚合服务名 `documents` 和其他授权都不会放行，返回 `plugin.host.permissionDenied`（`-32010`）。

### 宿主现状

| 宿主 | 现状 |
| --- | --- |
| Lithe 桌面端 | 声明 `backend`。后端能调用的方法以 `getCapabilities` 的返回为准，它只列出权限允许、且宿主注册了处理器的方法。桌面端提供文档、编辑器读写和打开定位、`browser/openExternal`（仅 HTTP 和 HTTPS）、内容 Provider、编辑器动作和语言功能 Provider；不提供 `preview/openHTML`。能力清单见[桌面端源码](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go) |

### 契约章节

| 章节 | 内容 |
| --- | --- |
| [Public document, file and window methods](../../contract/v1/README.md#public-document-file-and-window-methods) | 文档、编辑、打开、预览、浏览器方法的参数、限制和失败码 |
| [Independent editor actions](../../contract/v1/README.md#independent-editor-actions) | `editor/registerActions` 的注册、范围、调用和撤回，以及图标、`alternatives` 和点击结果 |
| [Editor language-feature providers](../../contract/v1/README.md#editor-language-feature-providers) | `editor/registerProviders` 的注册、选择器、结果形状、预算和合并规则 |
| [Content providers](../../contract/v1/README.md#content-providers) | `documents/registerContentProvider` 的 scheme 规则、调用、只读文档和撤回 |
| [Workspace contexts and capability discovery](../../contract/v1/README.md#workspace-contexts-and-capability-discovery) | `contextID`/`generation` 与 `getCapabilities` |
| [Outcomes and error codes](../../contract/v1/README.md#outcomes-and-error-codes) | 结果与错误码 |

### 示例与用例

| 文件 | 内容 |
| --- | --- |
| [`editor/editor-actions.plugin.json`](../../contract/v1/fixtures/editor/editor-actions.plugin.json) | `backend` 加 `editor.actions`、`documents.read`：`onWorkspaceOpen` 启动后注册编辑器动作 |
| [`editor/editor-actions.json`](../../contract/v1/fixtures/editor/editor-actions.json) | `plugins.editorActions.decode` 的 8 个 Core 用例：宿主声明的图标和源码上限、`alternatives`、点击结果 `reveal`，以及整个结果被拒绝的情形 |
| [`editor/editor-providers.plugin.json`](../../contract/v1/fixtures/editor/editor-providers.plugin.json) | `backend` 加 `editor.providers`、`documents.read`：`onWorkspaceOpen` 启动后注册编辑器语言功能 Provider |
| [`editor/editor-providers.json`](../../contract/v1/fixtures/editor/editor-providers.json) | 注册与结果的 Core 用例：6 个注册用例（选择器和类型校验、`unsupportedOption`）和 11 个结果用例（7 种类型的结果形状与被丢弃的结果） |
| [`backend/command-backend.plugin.json`](../../contract/v1/fixtures/backend/command-backend.plugin.json) | 无工具窗口的 `ui` 命令读取文档并编辑 |
| [`backend/function-protocol.json`](../../contract/v1/fixtures/backend/function-protocol.json) | `editor/applyEdits`、`editor/registerActions` 的标准请求与回复，以及 `versionConflict`、`readOnly` 等宿主失败码；还有 `documents/registerContentProvider` 的用例：需要 `documents.contentProvider` 授权、聚合名 `documents` 不够、保留 scheme、大写或数字开头的 scheme、非法函数名、缺少 `context` |

### 相关指南

- [插件后端：工作区上下文与宿主服务](../../guides/plugin-backend-workspace.md)

---

## English

Through the plugin-function protocol, a plugin backend reads open documents and selections, applies versioned edits, opens and reveals files, opens an HTML preview or an external browser, and registers its own editor actions and language-feature providers (definition, hover, completion, and so on). It can also serve documents that have no file (such as the decompiled source of a library class) to the host as a URI scheme. These are host methods: Rust Core checks permissions and the host executes them. The plugin never drives the editor directly.

| Item | Value |
| --- | --- |
| Manifest field | None of its own; use `backend` with the permissions below |
| Capability | `requires.backend: 0`; see [capabilities and protocol versions](../capabilities-and-versions.md) |
| Protocol | `litheFunctions`, version `0` |
| API IDs | API-09, 10, 12, 15, 16, 25, 27; see the [API catalog](../api-catalog.md) |
| Permissions | `documents.read`, `editor.edit`, `editor.open`, `preview.open`, `browser.open`, `editor.actions` (also needs `documents.read`), `editor.providers` (also needs `documents.read`), `documents.contentProvider` (grants only `documents/registerContentProvider`, not reading or editing documents) |
| Activation | As for the backend: `onWorkspaceOpen`, `onCommand:<id>`, or a page call |
| Lithe desktop | Declares `backend`; a backend finds which methods are usable with `getCapabilities` at runtime. See [Host status](#host-status) |

### Methods compared

| Method | Permission | API | Notes |
| --- | --- | --- | --- |
| `documents/list`, `documents/getSnapshot`, `editor/getActive`, `editor/getSelections` | `documents.read` | API-09 | Includes unsaved state, version, encoding, line endings; snapshots up to 128 KiB |
| `editor/applyEdits`, `editor/replaceSelections`, `editor/insertText` | `editor.edit` | API-10 | Checks the document version; refused during IME composition and on read-only documents; one batch is one undo step |
| `editor/open`, `editor/reveal` | `editor.open` | API-15 | Returns `documentID` and version; a range only places the caret at its start |
| `preview/openHTML` | `preview.open` | API-16 | `editorAndPreview` or `previewOnly` |
| `browser/openExternal` | `browser.open` | API-16 | An HTTP(S) URL or saved HTML |
| `editor/registerActions` | `editor.actions` + `documents.read` | API-27 | Discover it with `getCapabilities`, then register per workspace context; actions bind ordinary functions and file scopes, independent of LSP, build, or run. An action can carry multiple icons, up to 4 `alternatives`, and a click result `reveal`; the host declares the usable icons in `services.editor.options.actionIcons` |
| `editor/registerProviders` | `editor.providers` + `documents.read` | API-25 | Discover the method and `services.editor.options.providerTypes` with `getCapabilities`, then register per workspace context; binds the 7 types `definition`, `hover`, `completion`, `codeLens`, `foldingRange`, `semanticTokens`, and `documentLink` to ordinary functions, and a type outside `providerTypes` returns `unsupportedOption`; language-server results come first and plugin results only add to them; the host sets the budgets |

Every method takes `context: {contextID, generation}`; except for `documents/registerContentProvider`, a remote workspace context returns `plugin.host.authorityUnsupported`. `documents/registerContentProvider` is described in the next section.

### Editor actions and language-feature providers

| Topic | Rule |
| --- | --- |
| Action shape | The backend's collection function returns `{actions: [{id, range, title, icon, invoke}]}`: positions are one-based UTF-16, ranges are half-open and cannot split a surrogate pair; at most 1000 actions and 1 MiB serialized, and a malformed answer is refused whole; actions on the same line share one menu |
| Icons | `icon` is a presentation token the host declares; an icon names a picture, not an operation: a test icon does not make the action a test, and the host never starts anything because of an icon |
| Collection and click | Collection is read-only, with no network request or business execution; an edit invalidates the old actions immediately and cancels older collection; a click makes the host validate document, context, session, and registration again. A click that runs or debugs code should use `run/start` and `debug/start` once `getCapabilities` lists them, so the run appears in the host's Run and Debug windows; see [Debug adapters](debug-adapters.md) |
| Action validation | The shared Core command `plugins.editorActions.decode`; its request carries `version` (must be `0`), `kind` (`registration`, `actions`, or `invokeResult`), and the host-declared `icons` and `sourceBytes` |
| Provider merging | Language-server results come first: hover sections and completion items follow the server's, definitions add plugin locations to the server's, code lenses, folding ranges, and links are added, and semantic tokens apply only to files whose language server has none; the merge is the Core command `plugins.editorProviders.decode` with `kind: "mergeDefinitions"`, so every host merges alike |
| Budgets | Hover and completion 400 ms, definition 1500 ms, the rest 2000 ms; a late answer, or one for an older document version, is dropped without an error; a registration cannot set its own budget |
| A host without these methods | A host whose `getCapabilities` does not list `editor/registerActions` or `editor/registerProviders` presents no such feature, and the plugin keeps its own command and window entry points; a host that does not recognize the `editor.actions` or `editor.providers` permission refuses the package instead of granting it silently |

### Content providers (API-12)

A language server may hand out documents that have no file (such as the decompiled source of a library class). The plugin serves them as a URI scheme with `documents/registerContentProvider`, granted by `documents.contentProvider` (needs `backend`).

| Method | Permission | API | Params | Result |
| --- | --- | --- | --- | --- |
| `documents/registerContentProvider` | `documents.contentProvider` | API-12 | `{context, scheme, function}` | `{subscriptionID}` |

- Registration only records the binding and never calls the backend; `function` must already be registered on the session. Withdraw it with the existing `subscriptions/dispose`.
- `scheme` is 1 to 64 lowercase ASCII letters, digits, `+`, `-`, or `.`, and starts with a letter. The schemes the host or the platform resolves are refused: `about`, `blob`, `data`, `file`, `ftp`, `http`, `https`, `javascript`, `lithe`, `mailto`, `untitled`, `ws`, and `wss`. A malformed or reserved scheme, or a `function` that is not a function ID, returns `plugin.host.invalidParams`.
- A scheme has one provider per workspace window; registering it again returns `plugin.host.preconditionFailed`.
- When a document whose URI has that scheme is opened, the host calls the function through `lithe/function/invoke` with the workspace `context` and `{uri}`. The function answers a string, or `{content: string}`, of at most 1 MiB, and the document opens read-only. Any other answer, a failure, or a timeout (the host waits 30 seconds) shows the user an error and opens nothing.
- The provider is the only source of the text: the host keeps no copy once the document closes and asks again when it is reopened. Ending the session or closing the context withdraws the provider.
- Only `documents.contentProvider` grants this method; the aggregate service name `documents` and other grants do not, and return `plugin.host.permissionDenied` (`-32010`).

### Host status

| Host | Status |
| --- | --- |
| Lithe desktop | Declares `backend`. The methods a backend can call are whatever `getCapabilities` returns, which lists only methods the permissions allow and the host has registered a handler for. The desktop provides documents, editor reads and edits, open and reveal, `browser/openExternal` (HTTP and HTTPS only), content providers, editor actions, and language-feature providers; it does not provide `preview/openHTML`. The capability list is in the [desktop source](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go) |

### Contract sections

| Section | Content |
| --- | --- |
| [Public document, file and window methods](../../contract/v1/README.md#public-document-file-and-window-methods) | Parameters, limits, and failure codes for document, edit, open, preview, and browser methods |
| [Independent editor actions](../../contract/v1/README.md#independent-editor-actions) | Registering, scoping, invoking, and withdrawing `editor/registerActions`, plus icons, `alternatives`, and click results |
| [Editor language-feature providers](../../contract/v1/README.md#editor-language-feature-providers) | Registering `editor/registerProviders`, selectors, result shapes, budgets, and merge rules |
| [Content providers](../../contract/v1/README.md#content-providers) | Scheme rules, invoking, read-only documents, and withdrawing for `documents/registerContentProvider` |
| [Workspace contexts and capability discovery](../../contract/v1/README.md#workspace-contexts-and-capability-discovery) | `contextID`/`generation` and `getCapabilities` |
| [Outcomes and error codes](../../contract/v1/README.md#outcomes-and-error-codes) | Outcomes and error codes |

### Examples and cases

| File | Content |
| --- | --- |
| [`editor/editor-actions.plugin.json`](../../contract/v1/fixtures/editor/editor-actions.plugin.json) | `backend` with `editor.actions` and `documents.read`: registers editor actions after `onWorkspaceOpen` |
| [`editor/editor-actions.json`](../../contract/v1/fixtures/editor/editor-actions.json) | 8 Core cases for `plugins.editorActions.decode`: the host-declared icons and source limit, `alternatives`, click-result `reveal`, and results refused whole |
| [`editor/editor-providers.plugin.json`](../../contract/v1/fixtures/editor/editor-providers.plugin.json) | `backend` with `editor.providers` and `documents.read`: registers editor language-feature providers after `onWorkspaceOpen` |
| [`editor/editor-providers.json`](../../contract/v1/fixtures/editor/editor-providers.json) | Core cases for registrations and results: 6 registration cases (selector and type validation, `unsupportedOption`) and 11 result cases (result shapes for the 7 types, and dropped results) |
| [`backend/command-backend.plugin.json`](../../contract/v1/fixtures/backend/command-backend.plugin.json) | A `ui` command without a tool window that reads and edits documents |
| [`backend/function-protocol.json`](../../contract/v1/fixtures/backend/function-protocol.json) | Canonical requests and replies for `editor/applyEdits` and `editor/registerActions`, plus host failure codes such as `versionConflict` and `readOnly`; also the `documents/registerContentProvider` cases: it needs the `documents.contentProvider` grant, the aggregate `documents` name is not enough, reserved schemes, an uppercase or digit-first scheme, a malformed function name, and a missing `context` |

### Related guides

- [Plugin backend: workspace contexts and host services](../../guides/plugin-backend-workspace.md)
