# 快捷键导入 / Shortcut import

[中文](#中文) · [English](#english)

## 中文

插件可以把其他编辑器的快捷键配置转换成 Lithe 的快捷键。插件声明 1–8 个导入提供者，说明怎样识别某个编辑器的配置目录、要读哪些文件；用户在导入窗口里选中目录后，宿主只把声明过的文件按块交给插件后端，后端返回规范化的映射，由 Core 对照宿主的动作目录校验。插件拿不到绝对路径，也读不到用户没选中的目录。

| 项目 | 内容 |
| --- | --- |
| 清单字段 | `keymapImportProviders[]`：`{id, displayName, recognition, inputs}` |
| 能力版本 | `keymapImportProviders` 0，同时需要 `backend` 0。见[版本对照](../capabilities-and-versions.md#能力版本requires) |
| 协议 | `litheKeymapImport` 0：Core 只向声明了提供者的插件会话提供 `experimental.litheKeymapImport: {versions: [0], providers}`，后端回答 `{version: 0, providers}`（非空、无重复的子集），见[进程协议版本](../capabilities-and-versions.md#进程协议版本) |
| API 编号 | 不在 API-01–44 内（安装和导入层面的契约），见 [API 总表](../api-catalog.md#计划中的-api)下方的说明 |
| 权限 | `backend.launch`、`keymaps.provide`、`keymaps.readSelectedSources` |
| 激活事件 | 没有；宿主只在用户发起导入时启动后端 |
| 后端启动 | 包内可执行文件或包内运行时，不能依赖需要发现的运行时 |
| Lithe 桌面端 | 未声明 `keymapImportProviders`，也没有读取导入源的实现，安装时拒绝需要它的包 |
| 官方 SDK | [Node](../../sdk/node/README.md#shortcut-import-providers) 0.5.0、[Java](../../sdk/java/README.md#shortcut-import-providers) 0.5.0、[Rust](../../sdk/rust/README.md#shortcut-import-providers) 0.4.0 均实现 `litheKeymapImport` 0 |

### 声明与读取

| 部分 | 规则 |
| --- | --- |
| `id` | 插件内唯一，小写字母开头，只含小写 ASCII 字母、数字和连字符，最多 64 字节 |
| `recognition` | 1–16 个互不相同的只看元数据的探针：`{kind: "file" \| "directory", path}` 匹配所选目录下的条目，`{kind: "root", base: "userConfig", components}` 匹配平台用户配置目录（macOS 的 Application Support、Windows 的 Roaming AppData）下的确切路径。`components` 为 1–8 个安全的字面名称，最后一段可以是 `*`（只匹配一层目录）。识别不授权读内容，也不向插件暴露绝对路径 |
| `inputs` | 1–16 个互不相同的选择器：`{kind: "file", path}` 选一个相对文件；`{kind: "directory", path, extension}` 选该目录下指定小写扩展名的直接子文件，不递归，最多 64 个匹配（最多扫描 4096 个目录条目）。路径最长 256 字符，不含空段、`.`、`..`、反斜杠、冒号、通配符和控制字符 |
| 读取限制 | 拒绝符号链接和离开所选目录的路径；单个文件最多 1 MiB，一次快照最多 8 MiB，每块最多 128 KiB |
| 宿主方法 | `lithe/host/keymapImport/listInputs` 返回 `{files: [{id, path, byteCount}]}`；`lithe/host/keymapImport/readChunk` 按 `fileID`、`offset`、`maximumBytes`（1–131072）返回 base64 的 `{data}`。两者都带 `snapshotID` 与 `providerID`；错误不暴露绝对路径 |
| 导入请求 | `lithe/keymap/import` 只带 `{providerID, snapshotID}`；快照是宿主持有的不透明引用，导入或会话结束时撤销，同一提供者的新快照会撤销旧快照，捕获后被替换或修改的文件会被拒绝 |

提供者负责理解源格式，不能把有条件或多键序列的规则悄悄变成全局快捷键。

### 契约章节

| 章节 | 内容 |
| --- | --- |
| [Shortcut import providers (Core foundation)](../../contract/v1/README.md#shortcut-import-providers-core-foundation) | 清单字段、识别、输入、协商、宿主读取方法 |
| [Normalized shortcut import validation](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/contracts/rust-core-api.md#normalized-shortcut-import-validation) | Core 命令、结果校验规则、限制和示例（Rust Core API 文档） |

### 示例与用例

| 文件 | 内容 |
| --- | --- |
| [`keymap-import/keymap-import-provider.plugin.json`](../../contract/v1/fixtures/keymap-import/keymap-import-provider.plugin.json) | 两个提供者：`first` 用文件探针和两个 `userConfig` 根探针（含 `*`）、读一个文件；`second` 用目录探针、读一个文件加一个目录下的 `.xml` |
| [Lithe-IDEA/lithe-plugin-keymap-import](https://github.com/Lithe-IDEA/lithe-plugin-keymap-import) | 官方快捷键导入插件（预发布） |

### 相关指南

- [插件后端：工作区上下文与宿主服务](../../guides/plugin-backend-workspace.md)

---

## English

A plugin can convert another editor's shortcut configuration into Lithe shortcuts. The plugin declares one to eight import providers that say how to recognize an editor's configuration directory and which files to read. After the user selects a directory in the import window, the host passes only the declared files, in chunks, to the plugin backend; the backend returns normalized mappings, which Core validates against the host's action catalog. The plugin never sees absolute paths and cannot read a directory the user did not select.

| Item | Value |
| --- | --- |
| Manifest field | `keymapImportProviders[]`: `{id, displayName, recognition, inputs}` |
| Capability versions | `keymapImportProviders` 0, together with `backend` 0. See [versions](../capabilities-and-versions.md#capability-versions-requires) |
| Protocols | `litheKeymapImport` 0: Core offers `experimental.litheKeymapImport: {versions: [0], providers}` only to a plugin session that declares providers, and the backend answers `{version: 0, providers}` (a nonempty, duplicate-free subset); see [process protocol versions](../capabilities-and-versions.md#process-protocol-versions) |
| API IDs | Outside API-01–44 (an installation and import contract); see the note under the [API catalog](../api-catalog.md#planned-apis) |
| Permissions | `backend.launch`, `keymaps.provide`, `keymaps.readSelectedSources` |
| Activation events | None; the host starts the backend only when the user starts an import |
| Backend launch | A packaged executable or packaged runtime, without discovered runtime dependencies |
| Lithe desktop | Does not declare `keymapImportProviders` and has no import source reader; packages that require it are refused before installation |
| Official SDKs | [Node](../../sdk/node/README.md#shortcut-import-providers) 0.5.0, [Java](../../sdk/java/README.md#shortcut-import-providers) 0.5.0, and [Rust](../../sdk/rust/README.md#shortcut-import-providers) 0.4.0 implement `litheKeymapImport` 0 |

### Declaration and reading

| Part | Rules |
| --- | --- |
| `id` | Unique in the plugin; starts with a lowercase letter; lowercase ASCII letters, digits, and hyphens only; at most 64 bytes |
| `recognition` | 1–16 distinct metadata-only probes: `{kind: "file" \| "directory", path}` matches an entry beneath the selected directory, and `{kind: "root", base: "userConfig", components}` matches an exact path beneath the platform's user configuration directory (macOS Application Support, Windows roaming AppData). `components` are 1–8 safe literal names, and the last may be `*`, matching exactly one directory component. Recognition never authorizes reading contents and never exposes an absolute path to the provider |
| `inputs` | 1–16 distinct selectors: `{kind: "file", path}` selects one relative file; `{kind: "directory", path, extension}` selects that directory's immediate children with the given lowercase extension, never recursively, at most 64 matches (at most 4096 directory entries scanned). Paths are at most 256 characters and contain no empty, `.` or `..` segments, backslashes, colons, wildcards, or control characters |
| Read limits | Symlinks and paths leaving the selected directory are refused; at most 1 MiB per file, 8 MiB per snapshot, and 128 KiB per chunk |
| Host methods | `lithe/host/keymapImport/listInputs` returns `{files: [{id, path, byteCount}]}`; `lithe/host/keymapImport/readChunk` takes `fileID`, `offset`, and `maximumBytes` (1–131072) and returns base64 `{data}`. Both take `snapshotID` and `providerID`; errors never disclose absolute paths |
| Import request | `lithe/keymap/import` carries only `{providerID, snapshotID}`; the snapshot is an opaque host-owned reference, revoked when the import or session ends. A new snapshot for the same provider revokes the previous one, and files replaced or changed after capture are refused |

Providers own the source formats and must not silently promote conditional or multi-stroke rules to global shortcuts.

### Contract sections

| Section | Content |
| --- | --- |
| [Shortcut import providers (Core foundation)](../../contract/v1/README.md#shortcut-import-providers-core-foundation) | Manifest fields, recognition, inputs, negotiation, host read methods |
| [Normalized shortcut import validation](https://github.com/Lithe-IDEA/Lithe-core-rust/blob/main/rust/contracts/rust-core-api.md#normalized-shortcut-import-validation) | Core commands, result validation, limits, and fixtures (the Rust Core API document) |

### Examples and cases

| File | Content |
| --- | --- |
| [`keymap-import/keymap-import-provider.plugin.json`](../../contract/v1/fixtures/keymap-import/keymap-import-provider.plugin.json) | Two providers: `first` uses a file probe and two `userConfig` root probes (one with `*`) and reads one file; `second` uses a directory probe and reads one file plus the `.xml` files of a directory |
| [Lithe-IDEA/lithe-plugin-keymap-import](https://github.com/Lithe-IDEA/lithe-plugin-keymap-import) | The official shortcut import plugin (prerelease) |

### Related guides

- [Plugin backend: workspace contexts and host services](../../guides/plugin-backend-workspace.md)
