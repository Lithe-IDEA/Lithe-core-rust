# 包信任 / Package trust

[中文](#中文) · [English](#english)

## 中文

这一页说明插件包的目录规则、清单里的签名要求，以及 `packageTrust` 能力。签名要求决定宿主用什么证据接受一个包：发布者签名、宿主自身安装目录的代码签名，或者社区包的用户确认。签名和信任由宿主验证，Core 校验清单里的声明是否合法。

| 项目 | 内容 |
| --- | --- |
| 清单字段 | `vendor.id`、`vendor.displayName`、`vendor.signatureRequirement`；官方副本可以再加 `vendor.bundledSignatureRequirement` |
| 能力 | `packageTrust` 0，只用于 `vendor.bundledSignatureRequirement`。见[能力与协议版本](../capabilities-and-versions.md) |
| 签名文件 | 包根目录的 `lithe-plugin-signature.json`，Ed25519 签名，覆盖包里其他所有文件 |
| API 编号 | 不在 API-01–44 内（安装和打包层面的契约），见 [API 总表](../api-catalog.md) |
| 权限、激活事件 | 没有 |
| 宿主信任的密钥 | 由宿主决定；宿主固定自己官方插件 ID 的签名要求，社区包使用 `none` |
| Lithe 桌面端 | 未声明 `packageTrust`：要求它的包不会被提供安装，见[宿主现状](#宿主现状) |
| 官方 SDK | 与 SDK 无关；签名属于打包和发布环节 |

### 包目录规则

| 规则 | 内容 |
| --- | --- |
| 根目录 | `plugin.json`、`lithe-plugin-signature.json`（需要时）和清单引用的资源 |
| 路径 | `plugin.json` 里的每个路径都相对包根目录，用 `/` 分隔，不能含 `..`、开头的 `/`、反斜杠或 NUL |
| 符号链接 | 包里不能有符号链接 |
| 只读 | 宿主把已安装的包当作只读；插件的可写状态放在宿主给的状态目录，不能写回包目录 |

### 三种签名要求

| `signatureRequirement` | 适用于 | 宿主怎样接受 |
| --- | --- | --- |
| `publisherPackage` | 官方插件和其他由宿主信任的发布者签名的包 | 签名文件必须有效，并由宿主信任的密钥签名。开发构建可以把未签名的包从磁盘装成本地开发包（`localDevelopment`），发布构建拒绝 |
| `sameTeamAsHost` | 随宿主安装包一起发布的包 | 只认宿主自身已签名安装目录里的副本，由宿主的代码签名封存 |
| `none` | 社区包 | 不要求 Lithe 签名。从第三方市场安装时先用索引里的 SHA-256 校验压缩包；从本地 ZIP 或目录安装时先复制私有快照。两条路都要用户确认安装审查。`dev.lithe` 和 `dev.lithe.*` 不能使用（`plugin.manifest.invalid`）。包里带了签名文件仍要验证，无效签名不会被忽略 |

宿主固定自己官方插件 ID 的签名要求，包不能把自己降级。从磁盘选文件不能绕过这个固定值、命名空间限制、发布者密钥检查，也不能绕过 `sameTeamAsHost` 要求的安装封存。社区 `none` 包在开发构建和发布构建里都走自己的确认流程，不会被当成开发包。

### `packageTrust`：随宿主发布的官方副本

官方插件可以同时声明：

- `requires.packageTrust: 0`
- `vendor.signatureRequirement: "publisherPackage"`
- `vendor.bundledSignatureRequirement: "sameTeamAsHost"`

只有官方命名空间的 ID 可以这样声明，主要求和随宿主发布时的要求都必须与宿主固定的官方目录一致。只有实际路径在宿主封存的安装目录内、来源为 `bundled` 的副本可以不带发布者签名；单独安装、导入或回退的副本仍然需要发布者签名。只要带了签名就会验证，封存目录里也一样。这条声明本身不授予任何信任。

没有在 `hostCapabilities` 里声明 `packageTrust: 0` 的宿主，在下载前就过滤掉这样的版本，并拒绝它的清单（`plugin.api.unsupported`，`field` 为 `requires.packageTrust`）。没有这条声明的清单得不到随宿主发布的免签名待遇。

### 签名文件

`lithe-plugin-signature.json` 的字段：

| 字段 | 内容 |
| --- | --- |
| `schemaVersion` | `1`（签名文件自己的格式，不是清单的 `schemaVersion`） |
| `algorithm` | `"ed25519"` |
| `keyID` | 签名密钥的 ID，只能用小写字母、数字、`.`、`-`、`_` |
| `pluginID`、`pluginVersion` | 必须与 `plugin.json` 的 `id`、`version` 一致 |
| `files` | 包里除签名文件以外每个文件的相对路径到 SHA-256（小写十六进制）的映射；文件集合必须与包里的实际文件完全一致 |
| `signature` | 对规范负载的 Ed25519 签名，base64，64 字节 |

规范负载是 UTF-8 文本，每行以 `\n` 结尾：

```text
schemaVersion=1
algorithm=ed25519
keyID=<keyID>
pluginID=<token(pluginID)>
pluginVersion=<token(pluginVersion)>
file=<token(path)>=<sha256>      # 每个文件一行，按路径排序
```

`token` 是 UTF-8 字节的 base64url 编码，去掉末尾的 `=`。没有发布者签名的包，宿主可以记录内容摘要：把每个文件写成 `token(path)=<sha256>\n`，按路径排序后拼接，再取 SHA-256，之后用它判断包文件有没有被改动。确切字节以 [`signature-vectors.json`](../../contract/v1/fixtures/package-trust/signature-vectors.json) 为准。

### 宿主现状

| 宿主 | 现状 |
| --- | --- |
| Lithe 桌面端 | 未声明 `packageTrust`。要求它的插件（例如带 `vendor.bundledSignatureRequirement` 的官方副本）不会被提供安装，`validate` 和 `installReview` 也拒绝它们。其他宿主能力见桌面端的[能力清单](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go) |

### 契约章节

| 章节 | 内容 |
| --- | --- |
| [Package layout](../../contract/v1/README.md#package-layout) | 包目录、路径和符号链接规则 |
| [Trust](../../contract/v1/README.md#trust) | 三种签名要求、`packageTrust`、官方 ID 固定和开发包规则 |
| [Capabilities](../../contract/v1/README.md#capabilities) | `packageTrust` 在能力表里的位置 |
| [Host capabilities](../../contract/v1/README.md#host-capabilities) | 宿主用 `hostCapabilities` 声明能力，`validate` 和 `installReview` 据此拒绝 |
| [Trust and install review](../../contract/v1/README.md#trust-and-install-review) | 社区包的安装审查，以及本地磁盘安装的私有快照 |

### 示例与用例

| 文件 | 内容 |
| --- | --- |
| [`package-trust/publisher-bundled.plugin.json`](../../contract/v1/fixtures/package-trust/publisher-bundled.plugin.json) | 官方语言服务器插件，`requires` 含 `packageTrust: 0`，同时声明 `publisherPackage` 和 `bundledSignatureRequirement: "sameTeamAsHost"` |
| [`package-trust/signature-vectors.json`](../../contract/v1/fixtures/package-trust/signature-vectors.json) | 跨平台签名向量：RFC 8032 测试密钥（不能签真实的包）、含非 ASCII 文件名的固定包、规范负载、确定性签名和内容摘要。CryptoKit 这类带随机数的签名器要用验证代替字节比对 |

### 相关指南

- [从零构建一个完整生命周期的 LSP 插件](../../guides/lsp-plugin-lifecycle.md)：社区包的信任、签名和发布

---

## English

This page covers the package layout, the signature requirement in the manifest, and the `packageTrust` capability. The signature requirement decides what evidence a host uses to accept a package: a publisher signature, the code signature of the host's own installation, or the user's confirmation for a community package. Hosts verify signatures and trust; Core validates that the manifest declarations are legal.

| Item | Details |
| --- | --- |
| Manifest fields | `vendor.id`, `vendor.displayName`, `vendor.signatureRequirement`; an official copy may add `vendor.bundledSignatureRequirement` |
| Capability | `packageTrust` 0, only for `vendor.bundledSignatureRequirement`. See [capabilities and protocol versions](../capabilities-and-versions.md) |
| Signature file | `lithe-plugin-signature.json` at the package root, an Ed25519 signature over every other file in the package |
| API number | Outside API-01–44 (an installation and packaging contract); see the [API catalog](../api-catalog.md) |
| Permissions, activation events | None |
| Keys hosts trust | The host decides; it pins the signature requirement of its official plugin IDs, and community packages use `none` |
| Lithe desktop | Does not declare `packageTrust`: packages that require it are not offered for installation. See [Host status](#host-status) |
| Official SDKs | Not SDK-related; signing is part of packaging and release |

### Package layout

| Rule | Details |
| --- | --- |
| Root | `plugin.json`, `lithe-plugin-signature.json` (when required), and the resources the manifest references |
| Paths | Every path in `plugin.json` is relative to the package root, uses `/`, and may not contain `..`, a leading `/`, a backslash, or NUL |
| Symbolic links | Packages may not contain symbolic links |
| Read-only | Hosts treat an installed package as read-only; a plugin's writable state goes in the state directory the host provides, never back into the package |

### The three signature requirements

| `signatureRequirement` | For | How a host accepts it |
| --- | --- | --- |
| `publisherPackage` | Official plugins and other packages signed by a publisher the host trusts | The signature file must be valid and signed by a key the host trusts. Development builds may install an unsigned package from disk as a local development package (`localDevelopment`); release builds refuse it |
| `sameTeamAsHost` | Packages shipped inside the host's installation | Honored only for a copy inside the host's own signed installation, sealed by the host's code signature |
| `none` | Community packages | No Lithe signature required. From a third-party marketplace the archive is first checked against the index SHA-256; from a local ZIP or directory a private snapshot is taken first. Both paths require the user to confirm the install review. `dev.lithe` and `dev.lithe.*` may not use it (`plugin.manifest.invalid`). A signature file that is present is still verified; an invalid signature is never ignored |

A host pins the signature requirement of its official plugin IDs so a package cannot downgrade itself. Selecting a file from disk never overrides that pin, the namespace restrictions, the publisher-key checks, or the installation seal `sameTeamAsHost` requires. Community `none` packages use their own approval path in both development and release builds and are never treated as development packages.

### `packageTrust`: official copies shipped with the host

An official plugin may declare all of:

- `requires.packageTrust: 0`
- `vendor.signatureRequirement: "publisherPackage"`
- `vendor.bundledSignatureRequirement: "sameTeamAsHost"`

Only official-namespace IDs may do so, and both the primary and the bundled requirement must match the host's pinned official catalog. Only a copy whose actual path is inside the host's sealed installation and whose origin is `bundled` may omit the publisher signature; every independently installed, imported, or rolled-back copy still needs it. A present signature is always verified, inside the seal too. The declaration alone grants no trust.

A host that does not declare `packageTrust: 0` in `hostCapabilities` filters such a version before download and refuses its manifest (`plugin.api.unsupported`, `field` `requires.packageTrust`). A manifest without the declaration gets no bundled exemption.

### Signature file

Fields of `lithe-plugin-signature.json`:

| Field | Details |
| --- | --- |
| `schemaVersion` | `1` (the signature file's own format, not the manifest's `schemaVersion`) |
| `algorithm` | `"ed25519"` |
| `keyID` | ID of the signing key, limited to lowercase letters, digits, `.`, `-`, and `_` |
| `pluginID`, `pluginVersion` | Must equal `id` and `version` in `plugin.json` |
| `files` | Map from the relative path of every file except the signature file to its lowercase hex SHA-256; the set must equal the package's actual files exactly |
| `signature` | Base64 Ed25519 signature of the canonical payload, 64 bytes |

The canonical payload is UTF-8 text with every line ending in `\n`:

```text
schemaVersion=1
algorithm=ed25519
keyID=<keyID>
pluginID=<token(pluginID)>
pluginVersion=<token(pluginVersion)>
file=<token(path)>=<sha256>      # one line per file, sorted by path
```

`token` is the base64url encoding of the UTF-8 bytes with trailing `=` removed. For a package without a publisher signature, a host may record a content digest: each file becomes `token(path)=<sha256>\n`, the lines are sorted by path and concatenated, and the result is hashed with SHA-256; the host then uses it to tell whether the package files changed. [`signature-vectors.json`](../../contract/v1/fixtures/package-trust/signature-vectors.json) is authoritative for the exact bytes.

### Host status

| Host | Status |
| --- | --- |
| Lithe desktop | Does not declare `packageTrust`. Plugins that require it (for example an official copy with `vendor.bundledSignatureRequirement`) are not offered for installation, and `validate` and `installReview` refuse them. The rest of the host's capabilities are listed in the desktop [capability list](https://github.com/1lck/Lithe-IDEA/blob/preview/desktop/internal/pluginhost/pluginhost.go) |

### Contract sections

| Section | Covers |
| --- | --- |
| [Package layout](../../contract/v1/README.md#package-layout) | Package root, path, and symbolic-link rules |
| [Trust](../../contract/v1/README.md#trust) | The three signature requirements, `packageTrust`, official ID pinning, and development packages |
| [Capabilities](../../contract/v1/README.md#capabilities) | Where `packageTrust` sits in the capability table |
| [Host capabilities](../../contract/v1/README.md#host-capabilities) | A host declares capabilities with `hostCapabilities`; `validate` and `installReview` refuse by it |
| [Trust and install review](../../contract/v1/README.md#trust-and-install-review) | The install review of community packages, and the private snapshot of a local disk install |

### Examples and cases

| File | Contents |
| --- | --- |
| [`package-trust/publisher-bundled.plugin.json`](../../contract/v1/fixtures/package-trust/publisher-bundled.plugin.json) | An official language-server plugin whose `requires` includes `packageTrust: 0`, declaring both `publisherPackage` and `bundledSignatureRequirement: "sameTeamAsHost"` |
| [`package-trust/signature-vectors.json`](../../contract/v1/fixtures/package-trust/signature-vectors.json) | Cross-platform signature vectors: the RFC 8032 test key (never for real packages), a fixed package with non-ASCII file names, the canonical payload, a deterministic signature, and the content digest. Signers that add randomness, such as CryptoKit, check by verification instead of byte comparison |

### Related guides

- [Building a complete-lifecycle LSP plugin](../../guides/lsp-plugin-lifecycle.md): community package trust, signing, and release
