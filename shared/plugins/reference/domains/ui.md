# 界面：工具窗口、命令、网页与插件视图 / UI: tool windows, commands, web pages and plugin views

[中文](#中文) · [English](#english)

## 中文

插件可以在工作台里放工具窗口和命令。窗口的外壳（停靠、标题、活动栏、顺序、布局、主题、生命周期）始终归宿主；插件只提供窗口内容：纯文本、包内网页，或插件视图：后端函数返回界面树（宿主控件、树）和画布节点，由宿主用原生控件绘制。同样的插件视图也能放进设置页的设置区域。命令可以打开插件自己的窗口，也可以不打开窗口，只通知后端。

| 项目 | 内容 |
| --- | --- |
| 清单字段 | `ui.toolWindows`、`ui.commands`（可选 `icon`） |
| 能力版本 | `ui` 0；`ui` 1 加上设置区域里的工具选择器（工具行的 `choose`）。插件视图的结果形状见 [`plugin-ui.schema.json`](../../contract/v1/plugin-ui.schema.json)，不另带版本号。见[版本对照](../capabilities-and-versions.md#能力版本requires) |
| 协议 | 网页通过 `functions.*` 调用[插件后端](backend.md)；插件视图由宿主经 `lithe/function/invoke` 按事件调用视图函数，每个结果交给 Core 命令 `plugins.ui.applyResult` 校验；没有窗口的命令通过 `litheFunctions` 0 的 `lithe/host/commands/didRun` 通知后端 |
| API 编号 | API-02、03、13、14、17，见 [API 总表](../api-catalog.md#现在能用的-api) |
| 权限 | `ui.contribute`；网页加 `ui.web`；没有窗口的命令加 `backend.launch`；后端弹出消息用 `window.messages` |
| 激活事件 | 每个窗口 `onView:<window-id>`，每个命令 `onCommand:<command-id>` |
| Lithe 桌面端 | 声明 `ui` 0，呈现文本、网页和插件视图工具窗口，以及打开工具窗口的命令；插件视图用 Viva 原生绘制宿主控件、树和画布。不呈现不属于任何工具窗口的命令：这类包照常安装，但这些命令没有入口 |

### 规则

- **工具窗口**：1–64 个。每个窗口有稳定的 `id`、`title`、初始 `placement`（`left`、`bottom` 或 `right`）、整数 `order`（默认 0），以及且只能有一种内容：`text`（纯文本）、`web`（包内网页）或 `view`（插件视图）。
- **命令**：最多 128 个，`placements` 非空，取值为 `commandPalette`、`toolbar`、`menu`。带 `toolWindowID` 的命令打开同一插件的一个窗口；省略 `toolWindowID` 的命令只通知后端，此时 `toolWindows` 可以为空或省略。
- **网页窗口**：`web.entry` 是包内 `.html` 文件的相对路径，需要 `ui.web` 权限。网页的桥接和生命周期见[网页内容](../../contract/v1/README.md#web-content)。
- **插件视图**：`view.function` 是插件后端的私有函数，清单需要 `backend`、`ui` 0 和 `ui.contribute` 权限。宿主按事件（`open`、`click`、`change`、`select`、`viewport` 等）调用它，为某个工作区显示的视图调用时带着该窗口的工作区上下文，函数返回整个视图或按节点 ID 的补丁。一个视图最多 512 个界面节点、深度 16、2000 个树项和 8 个画布，每个画布最多 2000 个节点；超出预算或违反界面规则的结果整体拒绝，宿主保留上一版视图并显示错误和重试。工具窗口里不能绑定设置，设置区域里不能放树。设置区域里的工具行带 `choose` 时是该工具的选择器，需要 `ui` 1。官方 SDK 都用 `view(id, handler)`（Go 是 `View(id, handler)`）声明视图函数（[Node.js](../../sdk/node/README.md#plugin-views)、[Rust](../../sdk/rust/README.md#plugin-views)、[Go](../../sdk/go/README.md#plugin-views)、[Java](../../sdk/java/README.md#plugin-views)）。
- **没有窗口的命令**：需要 `backend`（清单 `requires`）、`ui.contribute` 和 `backend.launch` 权限，以及 `onCommand:<command-id>` 激活事件。`commands/didRun` 只是事件，不代表命令的业务动作成功。
- 所有 ID 以插件 ID 加点开头。标题最多 256 字节，窗口文本最多 65536 字节，按原样显示，不作为 HTML 或脚本执行。

### 契约章节

| 章节 | 内容 |
| --- | --- |
| [宿主负责的工作台界面](../../contract/v1/README.md#host-owned-workbench-ui) | 工具窗口、命令、ID、限制、宿主负责的部分 |
| [网页内容](../../contract/v1/README.md#web-content) | 包内网页、桥接、状态和生命周期 |
| [插件视图](../../contract/v1/README.md#plugin-views) | 事件、结果与补丁、宿主控件、画布、预算、生命周期与失败 |
| [设置区域](../../contract/v1/README.md#settings-regions) | 设置页上的插件视图和搜索关键词（`settingRegions`），见[设置](settings.md) |
| [不打开窗口的命令](../../contract/v1/README.md#backend-only-commands) | 没有工具窗口的命令与 `commands/didRun` |
| [UI 图标](../../contract/v1/README.md#ui-icons) | 宿主图标 ID（如 `{"host": "database"}`）与自带 SVG/PNG |
| [公共文档、文件和窗口方法](../../contract/v1/README.md#public-document-file-and-window-methods) | 后端弹出的消息 `window/showInformation`、`showWarning`、`showError` |
| [现有社区商店的窗口](../../contract/v1/README.md#existing-community-store) | 给已有社区包增加宿主绘制视图的步骤 |

### 示例与用例

| 文件 | 内容 |
| --- | --- |
| [`manifest/hello-lithe.plugin.json`](../../contract/v1/fixtures/manifest/hello-lithe.plugin.json) | `ui` 0：一个文本窗口和打开它的命令 |
| [`ui/web-panel.plugin.json`](../../contract/v1/fixtures/ui/web-panel.plugin.json) | 两个网页窗口和一个命令 |
| [`ui/view-panel.plugin.json`](../../contract/v1/fixtures/ui/view-panel.plugin.json) | 工具窗口和设置区域里的插件视图 |
| [`ui/view-results.json`](../../contract/v1/fixtures/ui/view-results.json) | `plugins.ui.applyResult` 的接受与拒绝用例：整页视图、补丁、画布颜色、界面规则 |
| [`backend/backend.plugin.json`](../../contract/v1/fixtures/backend/backend.plugin.json) | 网页窗口调用 `backend` 0 |
| [`backend/command-backend.plugin.json`](../../contract/v1/fixtures/backend/command-backend.plugin.json) | 不打开窗口的后端命令 |
| [`plugin-ui.schema.json`](../../contract/v1/plugin-ui.schema.json) | 视图函数结果的 JSON Schema |

清单层面的界面拒绝用例在 [`manifest/invalid-manifests.json`](../../contract/v1/fixtures/manifest/invalid-manifests.json)，视图结果的拒绝用例在 `ui/view-results.json`。

### 相关指南

- [插件后端：工作区上下文与宿主服务](../../guides/plugin-backend-workspace.md)

---

## English

A plugin can place tool windows and commands in the workbench. The window shell (docking, title, activity bar, order, layout, theme, lifecycle) always belongs to the host; the plugin supplies only the content: plain text, a packaged web page, or a plugin view: a backend function returns a UI tree (host controls and trees) and canvas nodes that the host draws with native controls. The same plugin views can also sit in a settings region of a settings page. A command can open one of the plugin's windows, or open none and only notify the backend.

| Item | Value |
| --- | --- |
| Manifest fields | `ui.toolWindows`, `ui.commands` (optional `icon`) |
| Capability versions | `ui` 0; `ui` 1 adds the tool picker in settings regions (`choose` on a tool row). The shape of plugin view results is [`plugin-ui.schema.json`](../../contract/v1/plugin-ui.schema.json), which carries no version of its own. See [versions](../capabilities-and-versions.md#capability-versions-requires) |
| Protocols | Pages call the [plugin backend](backend.md) through `functions.*`; for a plugin view the host calls the view function through `lithe/function/invoke` once per event and passes each result to the Core command `plugins.ui.applyResult`; a command without a window notifies the backend with `lithe/host/commands/didRun` (`litheFunctions` 0) |
| API IDs | API-02, 03, 13, 14, 17; see the [API catalog](../api-catalog.md#apis-usable-now) |
| Permissions | `ui.contribute`; `ui.web` for web pages; `backend.launch` for a command without a window; `window.messages` for messages shown by the backend |
| Activation events | `onView:<window-id>` per window, `onCommand:<command-id>` per command |
| Lithe desktop | Declares `ui` 0 and presents text, web, and plugin view tool windows, and commands that open a tool window; plugin views draw their host controls, trees, and canvases natively with Viva. It does not present commands that belong to no tool window: such packages install, but those commands have no entry point |

### Rules

- **Tool windows**: 1–64. Each has a stable `id`, `title`, initial `placement` (`left`, `bottom`, or `right`), integer `order` (default 0), and exactly one kind of content: `text` (plain text), `web` (a packaged page), or `view` (a plugin view).
- **Commands**: up to 128. `placements` is nonempty and takes `commandPalette`, `toolbar`, or `menu`. A command with `toolWindowID` opens one of the same plugin's windows. A command without `toolWindowID` only notifies the backend; `toolWindows` may then be empty or omitted.
- **Web windows**: `web.entry` is a package-relative `.html` path and requires the `ui.web` permission. The bridge and lifecycle are in [Web content](../../contract/v1/README.md#web-content).
- **Plugin views**: `view.function` is a private function of the plugin's backend; the manifest needs `backend`, `ui` 0, and the `ui.contribute` permission. The host calls it once per event (`open`, `click`, `change`, `select`, `viewport`, and so on), with the window's workspace context when the view is shown for a workspace, and it returns a whole view or patches by node ID. A view holds at most 512 UI nodes, depth 16, 2000 tree items and 8 canvases of at most 2000 nodes each; a result over budget or against a surface rule is refused whole, and the host keeps the last view with an error and Retry. A tool window cannot bind settings, and a settings region cannot show a tree. A tool row with `choose` in a settings region is the tool's picker and needs `ui` 1. All official SDKs declare a view function with `view(id, handler)` (`View(id, handler)` in Go) ([Node.js](../../sdk/node/README.md#plugin-views), [Rust](../../sdk/rust/README.md#plugin-views), [Go](../../sdk/go/README.md#plugin-views), [Java](../../sdk/java/README.md#plugin-views)).
- **Commands without a window**: require `backend` (in `requires`), `ui.contribute`, and `backend.launch`, plus the `onCommand:<command-id>` activation event. `commands/didRun` is an event; it does not show that the command's business action succeeded.
- Every ID starts with the plugin ID and a dot. Titles are at most 256 bytes and window text at most 65536 bytes; text is shown literally and never evaluated as HTML or script.

### Contract sections

| Section | Content |
| --- | --- |
| [Host-owned workbench UI](../../contract/v1/README.md#host-owned-workbench-ui) | Tool windows, commands, IDs, limits, and what the host owns |
| [Web content](../../contract/v1/README.md#web-content) | Packaged pages, bridge, state, and lifecycle |
| [Plugin views](../../contract/v1/README.md#plugin-views) | Events, results and patches, host controls, canvas, budgets, lifecycle and failure |
| [Settings regions](../../contract/v1/README.md#settings-regions) | Plugin views and search keywords on settings pages (`settingRegions`); see [Settings](settings.md) |
| [Backend-only commands](../../contract/v1/README.md#backend-only-commands) | Commands without a tool window, and `commands/didRun` |
| [UI icons](../../contract/v1/README.md#ui-icons) | Host icon IDs (such as `{"host": "database"}`) and packaged SVG/PNG |
| [Public document, file and window methods](../../contract/v1/README.md#public-document-file-and-window-methods) | Backend messages `window/showInformation`, `showWarning`, `showError` |
| [Existing community store](../../contract/v1/README.md#existing-community-store) | Steps for adding a host-rendered view to an existing community package |

### Examples and cases

| File | Content |
| --- | --- |
| [`manifest/hello-lithe.plugin.json`](../../contract/v1/fixtures/manifest/hello-lithe.plugin.json) | `ui` 0: one text window and the command that opens it |
| [`ui/web-panel.plugin.json`](../../contract/v1/fixtures/ui/web-panel.plugin.json) | Two web windows and a command |
| [`ui/view-panel.plugin.json`](../../contract/v1/fixtures/ui/view-panel.plugin.json) | Plugin views in a tool window and a settings region |
| [`ui/view-results.json`](../../contract/v1/fixtures/ui/view-results.json) | Accepted and refused `plugins.ui.applyResult` cases: whole views, patches, canvas colors, surface rules |
| [`backend/backend.plugin.json`](../../contract/v1/fixtures/backend/backend.plugin.json) | A web window calling `backend` 0 |
| [`backend/command-backend.plugin.json`](../../contract/v1/fixtures/backend/command-backend.plugin.json) | A backend command without a window |
| [`plugin-ui.schema.json`](../../contract/v1/plugin-ui.schema.json) | JSON Schema for view function results |

Manifest-level UI rejections are in [`manifest/invalid-manifests.json`](../../contract/v1/fixtures/manifest/invalid-manifests.json); refused view results are in `ui/view-results.json`.

### Related guides

- [Plugin backend: workspace contexts and host services](../../guides/plugin-backend-workspace.md)
