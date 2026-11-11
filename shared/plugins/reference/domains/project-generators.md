# 项目生成器 / Project generators

[中文](#中文) · [English](#english)

## 中文

插件可以在宿主的"新建项目"窗口里加入口：某种语言的普通项目，或框架、脚手架生成器。窗口归宿主：入口列表和搜索、项目名称和位置、"创建 Git 仓库"选项、空项目、"创建"按钮。生成器只声明自己的字段、结果需要的语言和构建系统、生成项目的步骤。Core 计算表单和生成计划，宿主执行，宿主代码不按生成器 ID 分支。

> **宿主支持。** Lithe 桌面端声明 `projectGenerators`（版本 `0`）：新建项目窗口列出生成器、通过 Core 计算它们的表单、用共享的工具发现为 `tool` 字段列出安装，并在暂存目录里执行生成计划。宿主源码 `Declared()` 上方注释的"尚未呈现"清单里没有项目生成器。
> 对契约有疑问，请到 [Lithe-core-rust 的 Issues](https://github.com/Lithe-IDEA/Lithe-core-rust/issues) 反馈；说明见 [`pluginhost.go`](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go)。

| 项目 | 内容 |
| --- | --- |
| 清单字段 | `projectGenerators[]`：`id`、`displayName`、`section`、`order`、`icon`、`fields`、`requirements`、`steps`、`openFiles` |
| 能力版本 | `projectGenerators` `0`。`requires` 必须恰好列出清单用到的能力；契约夹具里的生成器插件只需要这一项。见[版本对照](../capabilities-and-versions.md#能力版本requires) |
| 协议 | 没有进程协议；生成器只是数据，由 Core 命令计算 |
| API 编号 | [API-33](../api-catalog.md#现在能用的-api)：项目生成器 |
| 权限 | `project.generate` |
| 激活事件 | 不需要 |
| 宿主支持 | Lithe 桌面端：见上面的说明 |

### 字段、步骤与命令

| 部分 | 内容 |
| --- | --- |
| `section` | `project` 放在"新建项目"下（某种语言的普通项目），`generator` 放在"生成器"下（框架或脚手架）；按分区、`order`（0–1000，默认 100）、名称排序 |
| `fields` | 生成器的输入，按声明顺序绘制和计算；字段只能读取排在它前面的字段 |
| 字段类型 | `text`（`default` 模板、`pattern`、`maxLength`、`placeholder`、`optional`、`maximumVersion`）、`enum`（1–16 个选项，`presentation` 可选 `segmented` 或 `menu`）、`boolean`、`multiSelect`（1–128 个选项，可分组）、`tool`（工具定义的一个已有安装，按主目录记录，见下文）；其他类型是 `plugin.manifest.invalid` |
| 通用属性 | 每个字段都有 `id`（lowerCamelCase）、`title`、`description`、`advanced`（放进折叠的"高级设置"）和 `visibleWhen`；隐藏字段不校验，模板和条件里视为不存在 |
| `maximumVersion` | 一个读取前面字段的模板，给出取值允许的最高版本，按点分数字比较。常见写法是 `"${field.jdk.major}"`：目标 Java 版本不能超过所选 JDK。展开为空串（还没选工具）时不检查 |
| 条件 | `when`、`visibleWhen`：`equals`、`notEquals`、`oneOf`；单个条件或必须全部成立的列表；隐藏字段上的条件为假 |
| 清单模板 | `${project.name}`、`${field.<id>}`、`${field.<id>.version}`、`${field.<id>.major}`、`${field.<id>.executable}`（后三个只用于 `tool` 字段：所选安装的版本、主版本号、可执行文件路径，用在别的字段上校验清单时就被拒绝），修饰符 `path`、`package`、`lower` |
| 模板文件 | 用 `{{ … }}`，因为 POM、Gradle 脚本里本来就有 `${…}`；只有 `{{` 后面紧跟 `project.` 或 `field.` 才是变量；`dot-` 开头的路径段变成点文件（`dot-gitignore` → `.gitignore`）；模板目录里的隐藏文件被跳过，符号链接被拒绝 |
| `files` 步骤 | 把包内模板目录写进项目，保留可执行位；最多 1024 个文件、单个 1 MiB、合计 8 MiB |
| `command` 步骤 | 像构建工具一样解析可执行文件，在暂存目录里运行；`timeoutSeconds` 默认 600、最多 1800；`collect` 把子目录内容上移到项目根；展开为空串的参数被省略 |
| `archive` 步骤 | 下载 zip 并用 `project.extractArchive` 解压；必须是 HTTPS（`localhost`、`127.0.0.1`、`[::1]` 可用 HTTP），不带凭据；最多 20,000 个条目、解压后 512 MiB；创建前宿主会显示服务器地址 |
| Core 命令 | `project.generatorCatalog`、`project.generatorForm`、`project.generationPlan`、`project.checkName`、`project.extractArchive` |

文本值不会以 `-` 开头，也不含控制字符，所以一个值不会变成命令行选项或拆开一行。项目名称是一个目录名：非空、最多 128 个字符、不含 `/`、`\`、`:` 和控制字符、不是 `.` 或 `..`、不以 `-` 开头、首尾没有空格。

### `tool` 字段

`tool` 字段让用户从任意插件在 [`toolDefinitions`](toolchains.md) 里声明的工具中选一个已有安装，例如 Python 解释器、Node.js 或 JDK。生成器不指名任何宿主功能或语言：宿主通过通用的工具发现和用户的工具设置列出安装，和工具自己的设置页一样。

| 属性 | 含义 |
| --- | --- |
| `tool` | 必填。工具定义 ID：恰好由一个已启用插件定义的短 ID（如 `compiler`），或 `<pluginID>:<id>`。没有封闭列表；没人声明的工具没有安装，几个已启用插件都定义的短 ID 也没有，因为它不属于任何一个 |
| `minimumVersion` | 可选，一到三个点分数字。低于它的安装不可选。只允许用在 `tool` 字段上 |
| `pinAs` | 可选。`project` 把所选安装保存为新项目自己的工具选择（和工具设置页编辑的是同一份按目录的固定），项目因此用这里选的安装构建和运行；不写则选择只供步骤使用 |
| `optional` | 可选。没有可用安装时，可选字段保持为空且不留固定；否则表单报 `required`。低于 `minimumVersion` 的安装不算可用 |

`tool` 字段没有 `default`，初始值由宿主选择：Core 在宿主列出的安装里，优先保留用户的选择（如果它仍在列表中），否则选最新版本（版本相同时取宿主列表里靠前的）。版本按发布版本比较，末尾的 0 不计：`3.11`、`3.11.0`、`3.11.0.0` 相等。低于 `minimumVersion` 的安装在字段的 `installations` 里被标出，宿主据此把它显示为禁用。所选安装低于 `minimumVersion` 时表单报 `toolTooOld`；但用户没设置的可选字段保持为空，不会去选一个过旧的安装。

### 表单、计划与命令

- `form` 是 `{projectName, values, edited, tools}`。`values` 把字段 ID 映射到值；文本值只有 ID 在 `edited` 里才算用户填写，所以派生出来的默认值会继续跟随它读取的字段。`tools` 把每个 `tool` 字段 ID 映射到宿主为该工具找到的安装，按宿主偏好排序，形如 `[{home, executable, version}]`；字段的 `values` 项是用户所选安装的 `home`。
- 结果里的 `tool` 字段带 `installations: [{home, executable, version, meetsMinimum}]`，宿主从 Core 的回答绘制选择器，`value` 是 Core 选中的安装。字段的 `problem` 是 `required`、`tooLong`、`invalidFormat`、`unsupportedValue`、`toolTooOld`、`versionTooHigh` 之一，宿主按代码本地化。
- 计划的步骤是 `{kind: "writeFiles", files: [{path, contents, executable}]}`、`{kind: "command", command, arguments, environment, collect, mayUseNetwork, timeoutSeconds}` 和 `{kind: "archive", url, host, format: "zip"}`。`toolPins` 列出每个带 `pinAs` 且有安装的可见 `tool` 字段所选的工具、`home` 和 `executable`，宿主把它们存为项目的固定。
- 目录条目的 `tools` 列出每个 `tool` 字段对应工具的声明插件及其启用状态，和 `requirements` 一样：字段没有安装时，宿主可以提供启用其中一个，或打开市场。Core 只列出声明；定义是否可信、工具是否已安装，由宿主的工具注册表和发现决定。
- `requirements` 是创建出的项目需要的 `languageID` 或 `buildSystemID`（可带 `when`）。目录会列出提供它们的已安装插件及其启用状态，宿主据此提供启用或打开市场；它只是提示，不阻止创建。

步骤在暂存目录里按顺序执行，结束后宿主把它移到最终位置（目标已存在时拒绝）、把 `toolPins` 存为项目的工具选择（和工具设置页用的是同一个存储）、按需创建 Git 仓库并打开项目；`openFiles` 里的文件在创建后由宿主打开。失败或取消会删掉暂存目录，不留下任何东西（`plugin.project.generationFailed`）。

### 契约章节

| 章节 | 内容 |
| --- | --- |
| [Project generators](../../contract/v1/README.md#project-generators) | 宿主和生成器各自负责的部分、清单字段 |
| [Fields](../../contract/v1/README.md#fields) | 字段类型、属性、`maximumVersion`、项目名称规则 |
| [Tool fields](../../contract/v1/README.md#tool-fields) | `tool` 字段的属性、初始选择、最低版本 |
| [Conditions](../../contract/v1/README.md#conditions) | `when`、`visibleWhen` |
| [Templates](../../contract/v1/README.md#templates-1) | 清单模板、模板文件、点文件 |
| [Steps](../../contract/v1/README.md#steps) | 三种步骤和限制 |
| [Commands](../../contract/v1/README.md#commands-1) | 五个 Core 命令、表单与计划格式、失败清理 |

### 示例与用例

| 文件 | 内容 |
| --- | --- |
| [`project-generators/project-generator.plugin.json`](../../contract/v1/fixtures/project-generators/project-generator.plugin.json) | 三个生成器：`app`（`project` 分区，含 `java` 工具字段，按构建系统条件写模板文件）、`archetype`（运行脚手架命令）、`initializr`（从可配置服务器下载压缩包） |
| [`project-generators/project-plans.json`](../../contract/v1/fixtures/project-generators/project-plans.json) | `project.*` 命令的标准答案：表单默认值与派生、计划写模板、工具过旧、运行命令、下载压缩包、拒绝非 HTTPS 服务器、项目名称检查；其中的工具字段用例用 `manifest` 指定 `project-generator-tool-field.plugin.json`（选最新安装、保留用户选择、低于最低版本、没有安装、展开并固定、可选字段不固定、末尾 0 视为同一版本等） |
| [`project-generators/project-generator-tool-field.plugin.json`](../../contract/v1/fixtures/project-generators/project-generator-tool-field.plugin.json) | 两个用 `tool` 字段的生成器：`python-app`（`project` 分区，必填 `python` 工具字段，最低 3.11、固定到项目，用 `${field.python.executable}` 运行 `uv venv`）、`node-app`（`generator` 分区，可选 `node` 工具字段，最低 18.0.0、固定到项目） |
| [`project-generators/templates/`](../../contract/v1/fixtures/project-generators/templates/) | 模板目录：`app/README.md`、`app/dot-gitignore`、`main/Main.java`（Java 示例）和 `python-app` 用的 `tool-field/README.md`；`.gitattributes` 关闭换行转换，保证模板逐字节输出 |

生成器的清单拒绝用例在 [`manifest/invalid-manifests.json`](../../contract/v1/fixtures/manifest/invalid-manifests.json) 里，都和工具字段有关：`tool` 缺失或不是工具定义 ID、`minimumVersion` 不是点分数字、`pinAs` 取值未知、工具字段带 `default`、非工具字段用 `.executable`、非工具字段带 `tool` 属性。计划层面的拒绝（工具过旧、非 HTTPS 服务器、名称带斜杠）在 `project-plans.json` 里。

---

## English

A plugin can add entries to the host's New Project window: a plain project of a language, or a framework or scaffolding generator. The window belongs to the host: the entry list and its search, the project name and location, the "create Git repository" option, the empty project, and the Create button. A generator declares only its own fields, the languages and build systems the result needs, and the steps that produce it. Core computes the form and the generation plan, the host executes it, and no host code branches on a generator ID.

> **Host support.** The Lithe desktop declares `projectGenerators` (version `0`): the New Project window lists generators, evaluates their forms through Core, lists installations for `tool` fields through the shared tool discovery, and runs the generation plan in a staging directory. The "not presented yet" list in the comment above `Declared()` in the host source does not include project generators.
> Questions about the contract go to [Lithe-core-rust Issues](https://github.com/Lithe-IDEA/Lithe-core-rust/issues); see [`pluginhost.go`](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go) for the host's own description.

| Item | Value |
| --- | --- |
| Manifest field | `projectGenerators[]`: `id`, `displayName`, `section`, `order`, `icon`, `fields`, `requirements`, `steps`, `openFiles` |
| Capability versions | `projectGenerators` `0`. `requires` must list exactly the capabilities the manifest uses; the generator plugins in the contract fixtures need only this one. See [versions](../capabilities-and-versions.md#capability-versions-requires) |
| Protocols | No process protocol; a generator is data that Core commands evaluate |
| API IDs | [API-33](../api-catalog.md#apis-usable-now): project generators |
| Permissions | `project.generate` |
| Activation events | None needed |
| Host support | The Lithe desktop: see the note above |

### Fields, steps, and commands

| Part | Content |
| --- | --- |
| `section` | `project` lists the entry under "New Project" (a plain project of a language); `generator` lists it under "Generators" (a framework or scaffolding tool); sorted by section, `order` (0–1000, default 100), then name |
| `fields` | The generator's inputs, drawn and evaluated in order; a field may read only fields declared before it |
| Field types | `text` (`default` template, `pattern`, `maxLength`, `placeholder`, `optional`, `maximumVersion`), `enum` (1–16 options, `presentation` of `segmented` or `menu`), `boolean`, `multiSelect` (1–128 options, optionally grouped), `tool` (an installation of a tool definition, recorded by home directory; see below); any other type is `plugin.manifest.invalid` |
| Common properties | Every field takes `id` (lowerCamelCase), `title`, `description`, `advanced` (listed under the collapsed "Advanced Settings"), and `visibleWhen`; a hidden field is not validated and reads as absent in templates and conditions |
| `maximumVersion` | A template over earlier fields naming the highest version the value may be, compared numerically by dotted components. The usual form is `"${field.jdk.major}"`: a target Java release must not exceed the selected JDK. While the bound expands to an empty string (no tool chosen yet) nothing is checked |
| Conditions | `when` and `visibleWhen`: `equals`, `notEquals`, `oneOf`; one condition or a list that must all hold; a condition on a hidden field is false |
| Manifest templates | `${project.name}`, `${field.<id>}`, `${field.<id>.version}`, `${field.<id>.major}`, `${field.<id>.executable}` (the last three for `tool` fields only: the chosen installation's version, major number, and executable path; using them on another field is rejected when the manifest is validated), with the modifiers `path`, `package`, and `lower` |
| Template files | Use `{{ … }}`, because POMs and Gradle scripts already contain `${…}`; only `{{` followed by `project.` or `field.` is a variable; a path segment starting with `dot-` becomes a dotfile (`dot-gitignore` → `.gitignore`); hidden files in a template directory are skipped and symbolic links are refused |
| `files` step | Writes a packaged template directory into the project, keeping the executable bit; at most 1024 files, 1 MiB each, 8 MiB in total |
| `command` step | Resolves the executable like a build tool and runs it in the staging directory; `timeoutSeconds` defaults to 600, at most 1800; `collect` moves a subdirectory's contents up into the project root; arguments that expand to an empty string are left out |
| `archive` step | Downloads a zip and extracts it with `project.extractArchive`; HTTPS only (HTTP allowed for `localhost`, `127.0.0.1`, `[::1]`), no credentials; at most 20,000 entries and 512 MiB extracted; the host shows the server before creating |
| Core commands | `project.generatorCatalog`, `project.generatorForm`, `project.generationPlan`, `project.checkName`, `project.extractArchive` |

Text values never start with `-` and never contain control characters, so a value cannot become an option or split a line. A project name is one directory name: not empty, at most 128 characters, no `/`, `\`, `:`, or control characters, not `.` or `..`, no leading `-`, and no surrounding spaces.

### `tool` fields

A `tool` field lets the user pick one installation of any tool that a plugin declares in [`toolDefinitions`](toolchains.md), such as a Python interpreter, a Node.js runtime, or a JDK. The generator names no host feature and no language: the host lists installations through its generic tool discovery and the user's tool settings, exactly as the tool's own settings page does.

| Property | Meaning |
| --- | --- |
| `tool` | Required. A tool definition ID: a short ID such as `compiler` defined by exactly one enabled plugin, or `<pluginID>:<id>`. There is no closed list; a tool nobody declares has no installations, and neither has a short ID several enabled plugins define, because it belongs to none of them |
| `minimumVersion` | Optional, one to three dotted numbers. An installation below it is not selectable. Allowed only on a `tool` field |
| `pinAs` | Optional. `project` saves the selection as the created project's own tool choice (the same per-directory pin the tool's settings page edits), so the project builds and runs with the installation picked here. Without it the selection only feeds the steps |
| `optional` | Optional. Without a usable installation an optional field stays empty and leaves no pin; otherwise the form reports `required`. An installation below `minimumVersion` is not usable |

A tool field has no `default`; the host chooses the initial value. Core picks, among the installations the host listed, the user's choice if it is still listed and otherwise the newest version (the host's first among equal versions). Versions compare as releases, so trailing zeros do not count: `3.11`, `3.11.0`, and `3.11.0.0` are equal. An installation below `minimumVersion` is marked in the field's `installations`, so the host can show it disabled. When the selection is below `minimumVersion` the form reports `toolTooOld`, except that an optional field the user has not set stays empty rather than picking an installation that is too old.

### Forms, plans, and commands

- `form` is `{projectName, values, edited, tools}`. `values` maps field IDs to values; a text value counts as user-entered only when its ID is in `edited`, so derived defaults keep following the fields they read. `tools` maps each `tool` field ID to the installations the host found for that tool, in the order the host prefers them, as `[{home, executable, version}]`; the field's `values` entry is the `home` of the user's choice.
- A tool field in the result carries `installations: [{home, executable, version, meetsMinimum}]`, so the host draws its picker from Core's answer, and its `value` is the installation Core selected. A field `problem` is one of `required`, `tooLong`, `invalidFormat`, `unsupportedValue`, `toolTooOld`, and `versionTooHigh`; hosts localize by code.
- Plan steps are `{kind: "writeFiles", files: [{path, contents, executable}]}`, `{kind: "command", command, arguments, environment, collect, mayUseNetwork, timeoutSeconds}`, and `{kind: "archive", url, host, format: "zip"}`. `toolPins` lists, for each visible tool field with `pinAs` and an installation, the tool and the chosen `home` and `executable` that the host saves as the project's pin.
- A catalog entry's `tools` lists, for each tool field, the plugins that declare the field's tool and whether they are enabled, in the same way as `requirements`: when the field has no installation the host can offer to enable one, or open the marketplace. Core only names declarations; whether a definition is trusted, and whether the tool is installed, is decided by the host's tool registry and discovery.
- `requirements` are the `languageID` or `buildSystemID` (optionally with `when`) the created project needs. The catalog lists the installed plugins that provide each, with their enabled state, so the host can offer to enable one or open the marketplace; a requirement only prompts and never blocks creation.

Steps run in order in a staging directory. Afterwards the host moves it to its final location (refusing an existing one), saves the `toolPins` as the project's tool choices (the same store the tool settings page edits), creates the Git repository when asked, and opens the project; the files in `openFiles` are opened by the host after creation. A failure or cancellation removes the staging directory and leaves nothing behind (`plugin.project.generationFailed`).

### Contract sections

| Section | Content |
| --- | --- |
| [Project generators](../../contract/v1/README.md#project-generators) | What the host and the generator each own; manifest fields |
| [Fields](../../contract/v1/README.md#fields) | Field types, properties, `maximumVersion`, project name rules |
| [Tool fields](../../contract/v1/README.md#tool-fields) | `tool` field properties, the initial selection, minimum versions |
| [Conditions](../../contract/v1/README.md#conditions) | `when` and `visibleWhen` |
| [Templates](../../contract/v1/README.md#templates-1) | Manifest templates, template files, dotfiles |
| [Steps](../../contract/v1/README.md#steps) | The three step kinds and their limits |
| [Commands](../../contract/v1/README.md#commands-1) | The five Core commands, form and plan shapes, cleanup on failure |

### Examples and cases

| File | Content |
| --- | --- |
| [`project-generators/project-generator.plugin.json`](../../contract/v1/fixtures/project-generators/project-generator.plugin.json) | Three generators: `app` (the `project` section, with a `java` tool field, writing template files conditioned on the build system), `archetype` (running a scaffolding command), `initializr` (downloading an archive from a configurable server) |
| [`project-generators/project-plans.json`](../../contract/v1/fixtures/project-generators/project-plans.json) | Golden answers for `project.*` command cases: form defaults and derived values, a plan writing templates, a tool that is too old, running a command, downloading an archive, refusing a non-HTTPS server, project name checks; the tool field cases name `project-generator-tool-field.plugin.json` in `manifest` (the newest installation, keeping the user's choice, below the minimum, no installation, expanding and pinning, an optional field left unpinned, trailing zeros as the same release, and more) |
| [`project-generators/project-generator-tool-field.plugin.json`](../../contract/v1/fixtures/project-generators/project-generator-tool-field.plugin.json) | Two generators with `tool` fields: `python-app` (the `project` section; a required `python` tool field, at least 3.11, pinned to the project, running `uv venv` with `${field.python.executable}`) and `node-app` (the `generator` section; an optional `node` tool field, at least 18.0.0, pinned to the project) |
| [`project-generators/templates/`](../../contract/v1/fixtures/project-generators/templates/) | The template directories: `app/README.md`, `app/dot-gitignore`, `main/Main.java` (a Java example), plus `tool-field/README.md` for `python-app`; `.gitattributes` disables line-ending conversion so templates are emitted byte for byte |

Generator manifest refusals are in [`manifest/invalid-manifests.json`](../../contract/v1/fixtures/manifest/invalid-manifests.json), all about tool fields: a missing `tool` or one that is not a tool definition ID, a `minimumVersion` that is not dotted numbers, an unknown `pinAs`, a `default` on a tool field, `.executable` on a non-tool field, and a `tool` property on a non-tool field. Plan-level refusals (a tool that is too old, a non-HTTPS server, a name with a slash) are in `project-plans.json`.
