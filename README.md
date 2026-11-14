# Lithe Rust Core

Lithe 的跨平台应用内核，以及插件契约、插件开发资料和官方插件 SDK 的唯一来源。

| 目录 | 内容 |
| --- | --- |
| [`rust/crates/`](rust/crates) | Rust Core 的 crate，每个子目录是一个 crate，目录名是包名去掉 `lithe-` 前缀 |
| [`rust/contracts/`](rust/contracts) | Core 命令的接口契约和 schema |
| [`rust/fixtures/`](rust/fixtures) | Core 命令的测试数据，Core 测试和桌面端测试共用 |
| [`rust/logging/`](rust/logging) | 日志事件目录 |
| [`shared/plugins/`](shared/plugins) | 插件契约 `contract/v1/`、开发指南 `guides/`、参考 `reference/`、官方 SDK `sdk/{node,java,rust}` 和它们的端到端测试 |
| [`scripts/rust-core/`](scripts/rust-core) | Rust Core 注释规范检查（模块文档、英文注释、`unsafe` 安全说明、导出项 Rustdoc） |
| [`scripts/logging/`](scripts/logging) | 日志事件目录生成器，从 `rust/logging/events/` 生成 `rust/crates/diagnostics/src/catalog_generated.rs` |

**插件开发者**从 [`shared/plugins/README.md`](shared/plugins/README.md) 开始，不需要克隆 Lithe 应用仓库。

## 常用命令

```sh
cargo test --manifest-path rust/Cargo.toml --workspace   # Core 测试
scripts/rust-core/verify-rust-core-comments.sh           # 注释规范
node scripts/logging/generate-log-events.mjs --check     # 日志事件目录与生成表一致
node shared/plugins/scripts/verify-docs.mjs              # 契约 $id 与文档链接
node shared/plugins/scripts/sdk-e2e.mjs                  # 三个 SDK 对当前 Core 的端到端测试
```

修改 `shared/plugins/contract/v1/`、`rust/crates/plugin-model/src/capabilities.rs` 或
`rust/crates/plugin-rpc/src/` 的 PR，要在同一个 PR 里更新
[协议更新记录](shared/plugins/reference/protocol-changelog.md)。

---

# Lithe Rust Core (English)

Lithe's cross-platform application core, and the single source of the plugin contract, the plugin developer documentation and the official plugin SDKs.

| Directory | Contents |
| --- | --- |
| [`rust/crates/`](rust/crates) | The Rust Core crates; each directory is one crate, named after its package without the `lithe-` prefix |
| [`rust/contracts/`](rust/contracts) | Interface contracts and schemas for Core commands |
| [`rust/fixtures/`](rust/fixtures) | Test data for Core commands, shared by the Core and desktop tests |
| [`rust/logging/`](rust/logging) | The logging event catalog |
| [`shared/plugins/`](shared/plugins) | The plugin contract `contract/v1/`, guides `guides/`, reference `reference/`, the official SDKs `sdk/{node,java,rust}` and their end-to-end tests |
| [`scripts/rust-core/`](scripts/rust-core) | The Rust Core comment rules check (module docs, English comments, `unsafe` safety sections, Rustdoc on exported items) |
| [`scripts/logging/`](scripts/logging) | The logging event catalog generator, which turns `rust/logging/events/` into `rust/crates/diagnostics/src/catalog_generated.rs` |

**Plugin authors** start at [`shared/plugins/README.md`](shared/plugins/README.md) and never need to clone the Lithe application repository.

A pull request that changes `shared/plugins/contract/v1/`, `rust/crates/plugin-model/src/capabilities.rs` or `rust/crates/plugin-rpc/src/` updates the [protocol changelog](shared/plugins/reference/protocol-changelog.md) in the same pull request.
