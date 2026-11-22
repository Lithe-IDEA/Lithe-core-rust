# 参与 Rust Core 开发

本页规定平台相关代码怎么写、平台测试放在哪里、CI 怎样只运行受影响的 crate，以及固定的 Rust 版本怎么升级。插件契约和 SDK 的规则见 [`shared/plugins/README.md`](shared/plugins/README.md)。

## 平台差异写成数据

- 路径比较、Windows 保留文件名、路径斜杠、Unix 权限位、batch 参数转义这类平台规则放在 [`lithe-platform`](rust/crates/platform)，以 `Platform { Unix, Windows }` 参数表达。生产代码用 `Platform::HOST` 取本机规则。
- `cfg` 只用在必须调用操作系统接口的地方，例如 Windows job object、Unix 进程组、权限位系统调用。
- 规则写成数据后，任何主机都能分别传入 `Platform::Unix` 和 `Platform::Windows` 测试两套规则，不需要真的有那台系统。

## 平台测试放在哪里

按下面的顺序判断，命中第一条就停：

1. **能写成 `Platform` 参数的规则**：在规则所在的 crate 里对两个平台各测一次，不加 `cfg`。这类测试在所有主机上都运行。
2. **结果取决于真实操作系统的行为**（进程树、job object、权限位、batch 启动、可执行文件查找等）：放进 [`rust/crates/platform-tests/tests/platform/`](rust/crates/platform-tests/tests/platform) 里对应主题的模块。需要被启动的程序时，给 `lithe-platform-fixture` 增加一种模式。不要把这类测试写在被测 crate 里，也不要在其他 crate 里加 `#[cfg(windows)]`、`#[cfg(target_os = "macos")]` 之类的测试。
3. **只借用 Unix 才有的夹具**（符号链接、权限位）来验证本 crate 自己的逻辑，并且 Linux 上会运行的 `#[cfg(unix)]` 测试：可以留在本 crate。如果 Windows 上应有另一种结果，那个结果按第 2 条写进 `lithe-platform-tests`。
4. **测试里的 `cfg!`** 只能把夹具改成本机格式，例如 Windows 绝对路径要带盘符；不能用它为不同系统选择不同的预期结果。预期随系统变化的断言属于第 2 条。
5. **例外**：crate 的集成测试必须启动本包的可执行文件时，因为 `CARGO_BIN_EXE_*` 只对同一个包可见，测试无法搬走。这种 crate 在自己的 `Cargo.toml` 声明主机。目前只有 [`lithe-remote`](rust/crates/remote)：它的服务端运行在 Linux 和 macOS 主机上。新增例外要在 PR 里说明为什么不能放进 `lithe-platform-tests`。

[`scripts/rust-core/verify-platform-tests.mjs`](scripts/rust-core/verify-platform-tests.mjs) 检查的是：有没有测试在所属 crate 的每台 CI 主机上都被 `cfg` 编译掉，或者被 `cfg_attr(…, ignore)` 忽略。这样的测试在 CI 里永远不会运行，检查会失败。CI 的 `Select crates` 作业每次都运行这项检查。加 `--list` 会列出所有只在部分主机上运行的测试。第 3、4 条要看测试的意图，脚本判断不了，由评审把关。

## 声明 CI 主机

```toml
[package.metadata.lithe-ci]
hosts = ["macos", "windows"]   # 除 Linux 外，还要在这些主机上运行测试
test-features = ["server"]     # 测试需要的 feature，在每台主机上都启用
```

`hosts` 只接受 `macos` 和 `windows`；每个 crate 都会在 Linux 上测试，不必声明。目前声明主机的 crate 有 `lithe-platform`、`lithe-platform-tests` 和 `lithe-remote`。声明主机后，每个影响到该 crate 的 PR 都会多占用 macOS 或 Windows runner，所以只能用于上一节允许的情况。

## CI 怎样只运行受影响的 crate

[`.github/workflows/rust-core.yml`](.github/workflows/rust-core.yml) 由四个作业组成：

- **Select crates**：先运行 CI 脚本的测试和上面的放置检查，再由 [`affected-crates.mjs`](scripts/rust-core/affected-crates.mjs) 根据 PR 改动的文件选出 crate。选中的是文件所在的 crate，加上所有直接或间接依赖它的 crate（各种依赖都算，包括 dev-dependency）。另外几类文件的处理是：
  - 会选中全部 crate：`rust/` 下不属于某个 crate 的文件（workspace 清单、`Cargo.lock`、`fixtures/`、`contracts/`、`logging/`）、`shared/plugins/contract/`、`scripts/rust-core/`、`scripts/logging/`、`rust-toolchain.toml`，以及工作流文件本身。
  - `shared/plugins/sdk/e2e/` 只选中 `lithe-core`。
  - 文档、SDK 等其他文件不选中任何 crate。
- **Linux**：对选中的 crate 依次运行格式检查（全部 crate，耗时很短）、日志事件目录检查、注释规范（文本检查覆盖全部 crate，Rustdoc 只构建选中的 crate）、启用全部 feature 的 clippy、针对 `x86_64-pc-windows-msvc` 的 clippy（只用默认 feature），最后是启用 `test-features` 的测试。
- **macOS / Windows**：只测试选中、并且声明了对应主机的 crate。Draft PR 不运行这一步；PR 标记为 ready 并且 Linux 通过之后才运行。
- **Rust Core gate**：汇总上面三个作业。作业被跳过算通过，其余结果都算失败。工作流不设路径过滤，所以每个 PR 都会报告这个检查，分支规则可以把它设为必过。

手动运行工作流（workflow_dispatch）会检查全部 crate。CI 只缓存下载的依赖，不缓存构建产物。

## 工具链

仓库根目录的 [`rust-toolchain.toml`](rust-toolchain.toml) 固定 Rust 版本，本地和 CI 用同一个版本，新版本 clippy 新增的 lint 只会在主动升级时出现，不会让无关的 PR 失败。

- 在仓库里运行 `cargo` 时，rustup 自动安装并使用这个版本，不需要切换默认工具链。Homebrew 等不经过 rustup 安装的 Rust 不读这个文件，请改用 rustup。
- 额外的编译目标装在固定版本上：在仓库目录里运行 `rustup target add x86_64-pc-windows-msvc`，再运行下面的 Windows clippy。
- 升级时在同一个 PR 里修改 `rust-toolchain.toml` 的 `channel` 和 `.github/workflows` 里所有 `dtolnay/rust-toolchain@<版本>`（`@1.87` 除外），并修复新版本报出的问题。两处必须写同一个版本字符串，例如都写 `1.100.0`：rustup 把 `1.100` 和 `1.100.0` 当成两个工具链，写得不一致时 CI 会另装一份没有 clippy、rustfmt 和 Windows 目标的工具链。
- Lithe-IDEA 构建 Core 时使用 Lithe-IDEA 仓库根目录自己的 `rust-toolchain.toml`，Core 升级合入后，Lithe-IDEA 要同步升级到同一版本。
- Rust 插件 SDK（`shared/plugins/sdk/rust`）的最低版本 `rust-version = "1.87"` 不随固定版本变化，CI 用 `cargo +1.87` 测试它。

## 本地命令

```sh
node scripts/rust-core/affected-crates.mjs                # 相对 origin/main 的改动，CI 会运行哪些 crate
node scripts/rust-core/verify-platform-tests.mjs --list   # 放置检查，并列出只在部分主机运行的测试
node --test scripts/rust-core/*.test.mjs                  # CI 脚本自己的测试
cargo test --manifest-path rust/Cargo.toml -p lithe-platform -p lithe-platform-tests   # 在 macOS 或 Windows 上运行平台测试
```

手边没有 Windows 时，至少运行一次 `LIBSQLITE3_SYS_USE_PKG_CONFIG=1 cargo clippy --manifest-path rust/Cargo.toml --workspace --all-targets --target x86_64-pc-windows-msvc -- -D warnings`，确认 Windows 代码能编译。环境变量让 SQLite 改为链接系统库、不编译自带的 C 源码：编译它需要 MSVC 工具，macOS 和 Linux 上没有；clippy 只检查不链接，不受影响。

---

# Contributing to Rust Core (English)

This page sets how platform code is written, where platform tests go, how CI runs only the crates a change affects, and how the pinned Rust release is upgraded. The plugin contract and SDK rules live in [`shared/plugins/README.md`](shared/plugins/README.md).

## Platform differences are data

- Platform rules, such as path comparison, reserved Windows file names, path separators, Unix permission bits and batch argument quoting, live in [`lithe-platform`](rust/crates/platform) and take a `Platform { Unix, Windows }` argument. Production code passes `Platform::HOST`.
- `cfg` is only for calls into the operating system, such as Windows job objects, Unix process groups and permission system calls.
- Because the rules are data, any host tests both sets by passing `Platform::Unix` and `Platform::Windows`.

## Where platform tests go

Take the first rule that applies:

1. **A rule that takes a `Platform`**: test it for both platforms in the crate that owns it, without `cfg`. Every host runs these tests.
2. **A result that depends on the real operating system** (process trees, job objects, permission bits, batch launches, executable lookup and the like): add it to the module for that subject under [`rust/crates/platform-tests/tests/platform/`](rust/crates/platform-tests/tests/platform). When a test needs a program to start, add a mode to `lithe-platform-fixture`. Do not write such tests in the crate under test, and do not add `#[cfg(windows)]`, `#[cfg(target_os = "macos")]` or similar tests to any other crate.
3. **A `#[cfg(unix)]` test that only borrows a Unix fixture** (a symbolic link, permission bits) to check the crate's own logic, and that runs on Linux: it may stay in its crate. If Windows should behave differently, that result goes into `lithe-platform-tests` under rule 2.
4. **`cfg!` in a test** may only convert a fixture to the host's form, for example give a Windows absolute path its drive. It must not choose a different expected result per system; such assertions fall under rule 2.
5. **Exception**: when a crate's integration tests must start the crate's own executable, they cannot move, because `CARGO_BIN_EXE_*` is visible only within the package. Such a crate declares its hosts in its own `Cargo.toml`. Today only [`lithe-remote`](rust/crates/remote) does, because its server runs on Linux and macOS hosts. A new exception explains in its pull request why the tests cannot go into `lithe-platform-tests`.

[`scripts/rust-core/verify-platform-tests.mjs`](scripts/rust-core/verify-platform-tests.mjs) fails when a test is compiled out by `cfg`, or ignored by `cfg_attr(…, ignore)`, on every CI host of its crate, because CI would never run it. The `Select crates` job runs it on every change. `--list` also prints every test that runs on only some hosts. Rules 3 and 4 are about a test's intent, which the script cannot judge; review enforces them.

## Declaring CI hosts

```toml
[package.metadata.lithe-ci]
hosts = ["macos", "windows"]   # hosts that also run the crate's tests, besides Linux
test-features = ["server"]     # features the tests need, enabled on every host
```

`hosts` accepts `macos` and `windows`; every crate is tested on Linux without declaring it. Today `lithe-platform`, `lithe-platform-tests` and `lithe-remote` declare hosts. A declaration makes every pull request that affects the crate use a macOS or Windows runner, so it is only for the cases the previous section allows.

## How CI runs only the affected crates

[`.github/workflows/rust-core.yml`](.github/workflows/rust-core.yml) has four jobs:

- **Select crates** tests the CI scripts, runs the placement check above, and lets [`affected-crates.mjs`](scripts/rust-core/affected-crates.mjs) choose crates from the files the pull request changes: the crate each file belongs to, plus every crate that depends on it directly or indirectly, through any kind of dependency, dev-dependencies included. Other files are handled as follows:
  - These select every crate: files under `rust/` outside any crate (the workspace manifest, `Cargo.lock`, `fixtures/`, `contracts/`, `logging/`), `shared/plugins/contract/`, `scripts/rust-core/`, `scripts/logging/`, `rust-toolchain.toml` and the workflow itself.
  - `shared/plugins/sdk/e2e/` selects `lithe-core` only.
  - Documentation, the SDKs and other files select no crate.
- **Linux** runs, for the selected crates: the formatting check (every crate; it is quick), the log event catalog check, the comment rules (the textual checks cover every crate, Rustdoc builds the selected ones), clippy with every feature, clippy for `x86_64-pc-windows-msvc` with default features, and the tests with their `test-features`.
- **macOS / Windows** test the selected crates that declare that host. Draft pull requests skip them; they run once the pull request is ready for review and Linux passed.
- **Rust Core gate** sums up the three jobs above. A skipped job passes; any other result fails. The workflow has no path filter, so every pull request reports this check and branch rules can require it.

A manual run (workflow_dispatch) checks every crate. CI caches downloaded dependencies only, not build output.

## Toolchain

[`rust-toolchain.toml`](rust-toolchain.toml) at the repository root pins the Rust release. Local builds and CI use the same release, so lints added by a new clippy show up only when the pin is upgraded, never in an unrelated pull request.

- When `cargo` runs in this repository, rustup installs and uses that release; there is no need to change the default toolchain. Rust installed without rustup, for example by Homebrew, ignores the file; use rustup instead.
- Extra targets are added to the pinned release: run `rustup target add x86_64-pc-windows-msvc` in the repository before the Windows clippy below.
- An upgrade changes, in one pull request, the `channel` in `rust-toolchain.toml` and every `dtolnay/rust-toolchain@<version>` in `.github/workflows` except `@1.87`, and fixes what the new release reports. Both places must spell the version the same way, for example `1.100.0` in each: rustup treats `1.100` and `1.100.0` as two toolchains, and a mismatch makes CI install a second one without clippy, rustfmt or the Windows target.
- Lithe-IDEA builds Core with the `rust-toolchain.toml` at its own repository root. Once a Core upgrade is merged, Lithe-IDEA moves to the same release.
- The Rust plugin SDK (`shared/plugins/sdk/rust`) keeps its minimum `rust-version = "1.87"` whatever the pin is; CI tests it with `cargo +1.87`.

## Local commands

```sh
node scripts/rust-core/affected-crates.mjs                # which crates CI runs for the changes since origin/main
node scripts/rust-core/verify-platform-tests.mjs --list   # the placement check, listing tests that run on some hosts only
node --test scripts/rust-core/*.test.mjs                  # the CI scripts' own tests
cargo test --manifest-path rust/Cargo.toml -p lithe-platform -p lithe-platform-tests   # platform tests on macOS or Windows
```

Without a Windows host, at least run `LIBSQLITE3_SYS_USE_PKG_CONFIG=1 cargo clippy --manifest-path rust/Cargo.toml --workspace --all-targets --target x86_64-pc-windows-msvc -- -D warnings` to confirm the Windows code compiles. The variable makes SQLite link a system library instead of compiling its bundled C source, which needs the MSVC tools that macOS and Linux do not have; clippy only checks and never links, so this does not matter.
