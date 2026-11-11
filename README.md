# Lithe Rust Core

Lithe 的跨平台应用内核，以及插件契约、插件开发资料和官方插件 SDK 的唯一来源。

| 目录 | 内容 |
| --- | --- |
| [`rust/`](rust) | Rust Core 各 crate，以及 Core 自己的接口契约 `rust/contracts/`、测试数据 `rust/fixtures/` 和日志事件目录 `rust/logging/` |
| [`shared/plugins/`](shared/plugins) | 插件契约 `contract/v1/`、开发指南 `guides/`、参考 `reference/`、官方 SDK `sdk/{node,java,rust}` 和它们的端到端测试 |
| [`scripts/rust-core/`](scripts/rust-core) | Rust Core 注释规范检查（模块文档、英文注释、`unsafe` 安全说明、导出项 Rustdoc） |

**插件开发者**从 [`shared/plugins/README.md`](shared/plugins/README.md) 开始，不需要克隆 Lithe 应用仓库。

## 常用命令

```sh
cargo test --manifest-path rust/Cargo.toml --workspace   # Core 测试
scripts/rust-core/verify-rust-core-comments.sh           # 注释规范
node shared/plugins/scripts/verify-docs.mjs              # 契约 $id 与文档链接
node shared/plugins/scripts/sdk-e2e.mjs                  # 三个 SDK 对当前 Core 的端到端测试
```

修改 `shared/plugins/contract/v1/`、`rust/lithe-core/src/plugins/capabilities.rs` 或
`rust/lithe-core/src/lsp/interface/plugin_function.rs` 的 PR，要在同一个 PR 里更新
[协议更新记录](shared/plugins/reference/protocol-changelog.md)。

---

# Lithe Rust Core (English)

Lithe's cross-platform application core, and the single source of the plugin contract, the plugin developer documentation and the official plugin SDKs.

| Directory | Contents |
| --- | --- |
| [`rust/`](rust) | The Rust Core crates, plus Core's own interface contracts `rust/contracts/`, test data `rust/fixtures/` and logging event catalog `rust/logging/` |
| [`shared/plugins/`](shared/plugins) | The plugin contract `contract/v1/`, guides `guides/`, reference `reference/`, the official SDKs `sdk/{node,java,rust}` and their end-to-end tests |
| [`scripts/rust-core/`](scripts/rust-core) | The Rust Core comment rules check (module docs, English comments, `unsafe` safety sections, Rustdoc on exported items) |

**Plugin authors** start at [`shared/plugins/README.md`](shared/plugins/README.md) and never need to clone the Lithe application repository.

A pull request that changes `shared/plugins/contract/v1/`, `rust/lithe-core/src/plugins/capabilities.rs` or `rust/lithe-core/src/lsp/interface/plugin_function.rs` updates the [protocol changelog](shared/plugins/reference/protocol-changelog.md) in the same pull request.
