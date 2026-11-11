# 插件设置 / Plugin settings

[中文](#中文) · [English](#english)

## 中文

插件在清单里声明用户可以修改的选项。宿主负责显示控件、保存值、区分全局和项目两层，并把生效值传给 Core；Core 合并后展开到启动计划、`initializationOptions` 或语言服务器的 `workspaceConfiguration`。插件不写设置页代码，宿主也不需要认识插件。

| 项目 | 内容 |
| --- | --- |
| 清单字段 | `settings`、`settingActions` |
| 能力 | `settings`，版本 `0`。`workspaceConfiguration` 的展开和推送见[语言](languages.md) |
| 协议 | 没有专用协议；`settingActions` 调用的是插件自己的[插件函数](backend.md) |
| API 编号 | [API-19](../api-catalog.md#现在能用的-api)：声明式设置。运行期读取、查看来源、修改设置和变化事件仍在计划中 |
| 权限 | 不需要额外权限。设置不会改变运行的程序，所以不进入安装审查的程序列表 |
| 激活事件 | 不需要。值按设置的 `effect` 在下一次启动、模型重载、下一次运行或 `immediate` 推送时生效 |
| 桌面端 | 已声明 `settings`：按语言和构建系统分页、分组、全部控件类型、项目层、生效方式和页面按钮都已呈现 |

### 设置类型

| `type` | 字段 | 规则 |
| --- | --- | --- |
| `integer` | `minimum`、`maximum`、`unit`（可选） | `minimum` 和 `maximum` 必填 |
| `enum` | `options`，每项 `{value, title}` | 1–16 个选项；值只含字母、数字、`.`、`_`、`-` |
| `boolean` | — | 值是 `true` 或 `false` |
| `multiSelect` | `options`，每项 `{value, title}` | 1–32 个选项；值是互不相同的选项值数组 |
| `text` | `pattern`（必填）、`maxLength`（可选）、`placeholder`（可选） | `pattern` 必须匹配整个值；`maxLength` 默认 256，最多 1024 个 Unicode 标量 |
| `file`、`directory` | `placeholder`（可选） | 绝对路径，最多 1024 个 Unicode 标量；空值表示“未设置” |

所有类型的 `title`、`description` 都是宿主显示的文字。`text`、`file` 和 `directory` 的值不以 `-` 开头，也不含控制字符。`default` 的 JSON 类型必须符合 `type`，并且被这个声明接受。

```json
"settings": [
  { "id": "heapMB", "type": "integer", "title": "Maximum memory",
    "default": 2048, "minimum": 512, "maximum": 8192, "unit": "MB" },
  { "id": "logLevel", "type": "enum", "title": "Log level", "default": "info",
    "options": [{ "value": "info", "title": "Information" }, { "value": "debug", "title": "Debug" }] },
  { "id": "telemetry", "type": "boolean", "title": "Send usage data", "default": false }
]
```

因为每个值都是数字、布尔值或已声明的选项值，设置永远不能注入额外参数，也不能改变运行的包内文件。存储的值如果已经不被声明接受（例如更新后缩小了范围），会被默认值替换，启动不会因此失败。未声明的 ID 的值被忽略。

### 放置、层级与生效方式

设置还可以声明它显示在哪里、什么时候生效。宿主只提供通用的注册点：每种语言和构建系统一个页面、分组、每种类型一个控件、项目/全局切换和按钮。页面上出现什么，由清单决定。

```json
"settings": [
  { "id": "dialyzer", "type": "boolean", "title": "Dialyzer", "default": true,
    "scope": "project", "effect": "immediate", "page": { "languageID": "elixir" } },
  { "id": "mixEnv", "type": "text", "title": "MIX_ENV", "default": "dev", "pattern": "[a-z]+",
    "scope": "project", "page": { "languageID": "elixir" }, "section": "Server" }
],
"settingActions": [
  { "id": "rebuildPlt", "title": "Rebuild PLT", "function": "dialyzer.rebuild",
    "page": { "languageID": "elixir" }, "confirm": true }
]
```

| 字段 | 含义 |
| --- | --- |
| `scope` | `global`（默认）：所有项目共用一个值。`project`：打开的项目可以覆盖它 |
| `effect` | 修改何时生效。`restart`（默认）：插件进程下一次启动。`immediate`：通过 `workspaceConfiguration` 推送给运行中的语言服务器。`reload`：下一次构建模型重载，宿主会重载插件的构建模型。`nextRun`：下一次运行或构建命令 |
| `page` | `{ "languageID": … }` 或 `{ "buildSystemID": … }`：设置显示在哪个语言或构建系统的设置页上，与插件自己的页面并列。任何插件都可以往页面上加内容；只要有已启用的插件为它提供内容，宿主就显示该页面 |
| `section` | 设置在页面上所属分组的标题 |

`settingActions` 最多 16 个，按唯一的 `id` 排序。每个按钮显示在插件自己的页面和它声明的 `page` 上；点击后调用本插件的插件函数 `function`，参数为空，由本插件为当前项目运行的提供者进程执行（见[插件函数](backend.md)）。`confirm: true` 让宿主先询问用户。宿主只负责按钮本身，不知道函数做什么。声明按钮也算使用了 `settings`。

由 Core 在校验时检查的放置和使用规则：

- `immediate` 设置只能出现在语言服务器的 `workspaceConfiguration` 里，它从不进入命令行或 `initializationOptions`，所以修改它从不需要新进程。
- `text`、`file`、`directory` 设置只能是一个完整的模板值，或某个 `name=` 参数的值，不能是较长参数的一部分。

### 两层保存与生效顺序

宿主为每个插件的每个设置保存一个全局值，并把项目值保存在项目的 `.lithe/plugin-settings.json` 里（格式 `{pluginID: {settingID: value}}`，适合通过版本控制共享）。宿主把两者都作为 `settings` 和 `projectSettings` 传给 Core。Core 按这个顺序合并：项目值（只对 `scope: "project"` 有效）、全局值、默认值；存储的值如果不被声明接受，就跳过。

`plugins.declarative.resolveSettings` 返回每个生效值及其 `source`（`project`、`global` 或 `default`）。

### 契约章节

| 章节 | 内容 |
| --- | --- |
| [Plugin settings](../../contract/v1/README.md#plugin-settings) | 设置的字段、类型、模板 `${setting:<id>}` 和使用规则 |
| [Placement, scope, effect, and page buttons](../../contract/v1/README.md#placement-scope-effect-and-page-buttons) | 放置位置、层级、生效时机、页面按钮和校验规则 |
| [Workspace configuration](../../contract/v1/README.md#workspace-configuration) | `workspaceConfiguration` 的展开和推送 |
| [Executable resolution](../../contract/v1/README.md#executable-resolution) | `setting` 类型的可执行文件候选：引用全局 `file` 设置选择工具路径、候选顺序和严格选择 |
| [Launch templates](../../contract/v1/README.md#launch-templates) | 设置值在模板里的替换规则 |

### 示例与用例

| 文件 | 内容 |
| --- | --- |
| [`settings/settings.plugin.json`](../../contract/v1/fixtures/settings/settings.plugin.json) | 放在 Elixir 语言页和 Mix 构建系统页上的设置、`immediate` 设置、`settingActions` 按钮 |
| [`languages/node-runtime.plugin.json`](../../contract/v1/fixtures/languages/node-runtime.plugin.json) | 带插件设置的语言服务器（`node` 运行时） |
| [`build-systems/build-tool-path.plugin.json`](../../contract/v1/fixtures/build-systems/build-tool-path.plugin.json) | 用 `file` 设置选择构建工具的可执行文件候选 |
| [`debug-adapters/debug-adapter.plugin.json`](../../contract/v1/fixtures/debug-adapters/debug-adapter.plugin.json) | 调试适配器用 `file` 设置（`nextRun`）选择要调试的程序 |

设置相关的拒绝用例在 [`manifest/invalid-manifests.json`](../../contract/v1/fixtures/manifest/invalid-manifests.json)。

### 相关指南

- [从零构建一个完整生命周期的 LSP 插件](../../guides/lsp-plugin-lifecycle.md)
- [为语言插件接入构建、运行与测试](../../guides/build-tool-integration.md)

---

## English

A plugin declares, in its manifest, the options a user may change. The host draws the controls, stores the values, keeps the global and project layers apart, and passes the effective values to Core. Core merges them and expands them into launch plans, `initializationOptions`, or a language server's `workspaceConfiguration`. The plugin writes no settings-page code, and the host never needs to know the plugin.

| Item | Value |
| --- | --- |
| Manifest fields | `settings`, `settingActions` |
| Capability | `settings`, version `0`. Expanding and pushing `workspaceConfiguration` is described in [Languages](languages.md) |
| Protocol | None of its own; `settingActions` call the plugin's own [plugin functions](backend.md) |
| API IDs | [API-19](../api-catalog.md#apis-usable-now): declarative settings. Runtime reads, source inspection, writes, and change events are still planned |
| Permissions | None. Settings never change which program runs, so they are not in the install review's program list |
| Activation | Not needed. Values take effect according to the setting's `effect`: at the next start, a model reload, the next run, or an `immediate` push |
| Desktop | Declares `settings`: pages per language and build system, sections, every control type, the project layer, effects, and page buttons are presented |

### Setting types

| `type` | Fields | Rule |
| --- | --- | --- |
| `integer` | `minimum`, `maximum`, `unit` (optional) | `minimum` and `maximum` are required |
| `enum` | `options`, each `{value, title}` | One to 16 options; values contain only letters, digits, `.`, `_`, `-` |
| `boolean` | — | The value is `true` or `false` |
| `multiSelect` | `options`, each `{value, title}` | One to 32 options; the value is an array of distinct option values |
| `text` | `pattern` (required), `maxLength` (optional), `placeholder` (optional) | `pattern` matches the whole value; `maxLength` defaults to 256 and is at most 1024 Unicode scalars |
| `file`, `directory` | `placeholder` (optional) | An absolute path of at most 1024 Unicode scalars, or empty for "not set" |

`title` and `description` are text the host shows. Text and paths never start with `-` and contain no control characters. `default` must have the JSON type of `type` and must be accepted by the declaration.

```json
"settings": [
  { "id": "heapMB", "type": "integer", "title": "Maximum memory",
    "default": 2048, "minimum": 512, "maximum": 8192, "unit": "MB" },
  { "id": "logLevel", "type": "enum", "title": "Log level", "default": "info",
    "options": [{ "value": "info", "title": "Information" }, { "value": "debug", "title": "Debug" }] },
  { "id": "telemetry", "type": "boolean", "title": "Send usage data", "default": false }
]
```

Because every value is a number, a boolean, or one of the declared option values, a setting can never inject extra arguments or change which package file runs. A stored value the declaration no longer accepts (for example after an update narrowed a range) is replaced by the default, and a launch never fails for it. Values for undeclared IDs are ignored.

### Placement, scope, effect, and page buttons

A setting may also say where it is shown and when a change takes effect. The host offers only generic registration points: a page per language and per build system, sections, one control per setting type, a project/global switch, and buttons. What appears there comes from the manifest.

```json
"settings": [
  { "id": "dialyzer", "type": "boolean", "title": "Dialyzer", "default": true,
    "scope": "project", "effect": "immediate", "page": { "languageID": "elixir" } },
  { "id": "mixEnv", "type": "text", "title": "MIX_ENV", "default": "dev", "pattern": "[a-z]+",
    "scope": "project", "page": { "languageID": "elixir" }, "section": "Server" }
],
"settingActions": [
  { "id": "rebuildPlt", "title": "Rebuild PLT", "function": "dialyzer.rebuild",
    "page": { "languageID": "elixir" }, "confirm": true }
]
```

| Field | Meaning |
| --- | --- |
| `scope` | `global` (default): one value for every project. `project`: the open project may override it |
| `effect` | When a change takes effect. `restart` (default): the next start of the plugin's processes. `immediate`: pushed to the running language server through `workspaceConfiguration`. `reload`: the next build-model reload, which the host performs for the plugin's build models. `nextRun`: the next run or build command |
| `page` | `{ "languageID": … }` or `{ "buildSystemID": … }`: the settings page that shows the setting beside the plugin's own page. Any plugin may add to a page; the host shows the page while an enabled plugin contributes to it |
| `section` | Heading the setting is grouped under on its page |

`settingActions` (at most 16, sorted by unique `id`) are buttons the host draws on the plugin's page and on their `page`. Each invokes the plugin function `function` of the same plugin with empty params, served by the plugin's own provider process for the open project (see [plugin functions](backend.md)). `confirm: true` makes the host ask first. The host owns the button and knows nothing about what the function does. Declaring actions counts as using `settings`.

Placement and use rules that Core checks at validation:

- An `immediate` setting may appear only in a language server's `workspaceConfiguration`. It never reaches a command line or `initializationOptions`, so a change never needs a new process.
- A `text`, `file`, or `directory` setting is a whole template value, or the value of one `name=` argument; it is never part of a longer argument.

### Two layers and their order

The host stores one global value per plugin and setting, and a project value in the project's `.lithe/plugin-settings.json` (`{pluginID: {settingID: value}}`, meant to be shared through version control). It sends both as `settings` and `projectSettings`. Core merges them in this order: project (only for `scope: "project"`), then global, then default, skipping a stored value the declaration no longer accepts.

`plugins.declarative.resolveSettings` returns each effective value with its `source` (`project`, `global`, or `default`).

### Contract sections

| Section | Content |
| --- | --- |
| [Plugin settings](../../contract/v1/README.md#plugin-settings) | Setting fields, types, the `${setting:<id>}` template, and use rules |
| [Placement, scope, effect, and page buttons](../../contract/v1/README.md#placement-scope-effect-and-page-buttons) | Placement, layers, effect timing, page buttons, and validation rules |
| [Workspace configuration](../../contract/v1/README.md#workspace-configuration) | Expanding and pushing `workspaceConfiguration` |
| [Executable resolution](../../contract/v1/README.md#executable-resolution) | `setting` executable candidates: a global `file` setting selects the tool path, candidate order, and strict selection |
| [Launch templates](../../contract/v1/README.md#launch-templates) | How setting values are substituted in templates |

### Examples and cases

| File | Content |
| --- | --- |
| [`settings/settings.plugin.json`](../../contract/v1/fixtures/settings/settings.plugin.json) | Settings on the Elixir language page and the Mix build-system page, an `immediate` setting, and `settingActions` buttons |
| [`languages/node-runtime.plugin.json`](../../contract/v1/fixtures/languages/node-runtime.plugin.json) | A language server with plugin settings (`node` runtime) |
| [`build-systems/build-tool-path.plugin.json`](../../contract/v1/fixtures/build-systems/build-tool-path.plugin.json) | A `file` setting selects the build tool's executable candidate |
| [`debug-adapters/debug-adapter.plugin.json`](../../contract/v1/fixtures/debug-adapters/debug-adapter.plugin.json) | A debug adapter uses a `file` setting (`nextRun`) to pick the program to debug |

Settings rejections are in [`manifest/invalid-manifests.json`](../../contract/v1/fixtures/manifest/invalid-manifests.json).

### Related guides

- [Building an LSP plugin end to end](../../guides/lsp-plugin-lifecycle.md)
- [Build, run, and test for language plugins](../../guides/build-tool-integration.md)
