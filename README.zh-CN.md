# KitonyTerms

[English](README.md) | **中文**

KitonyTerms 是一个用 **Rust** 与 [Dioxus Desktop](https://dioxuslabs.com/)
构建的跨平台 SSH 桌面客户端。SSH、终端模拟、SFTP、监控、配置和机密存储
都保留在 Rust crate 中；桌面界面通过系统原生 WebView 栈渲染。

## 当前形态

- **主应用：** GUI-only 桌面二进制 `kitonyterms`，由 `kt-app` 提供。
- **支持平台：** macOS / Windows / Linux 的 `x64` 与 `aarch64` 发布产物。
  不构建 32 位产物。
- **核心引擎：** `kt-core` 中实现纯 Rust SSH 客户端、终端网格、SFTP 任务和远端监控，
  不依赖 UI。
- **界面：** Dioxus 0.7 desktop，系统窗口、左侧连接/SFTP 侧栏、中央终端工作区、
  监控横条、状态栏、弹窗与设置面板。
- **验证：** workspace 每个 crate 都有单元测试或集成测试覆盖；clippy 以
  `-D warnings` 执行。

## 已实现能力

- SSH 终端会话：支持密码、公钥、keyboard-interactive、ssh-agent/Pageant 认证。
- 侧栏保存会话与分组：支持重连、编辑、复制、删除，并可合并 `~/.ssh/config`。
- 主机密钥信任流程：使用 `known_hosts.toml` 记录指纹；未知或变化的主机密钥需要确认，
  支持“仅允许一次”和“信任此主机”。
- 本机加密机密保险库：密码与私钥口令加密存储，UI 启动时自动打开。
- 单跳 `ProxyJump`、TCP 级代理（`Direct`、`System`、`SOCKS5`、`HTTP CONNECT`）
  和可选 agent forwarding。
- 终端渲染：RGB 颜色、常见文本属性、光标样式、scrollback、分屏、触发器高亮、
  可选行号和可选时间戳。
- SFTP 文件浏览：列表、上传、下载、创建目录、删除、重命名、远端路径导航、跟随终端目录，
  以及通过本地编辑器编辑远端文件后回传。
- 编辑器设置：默认编辑器选择和右键“打开方式”条目。
- 远端 CPU、内存、磁盘、网络、负载、运行时长和延迟监控。
- 浅色/深色主题与中文/英文界面语言设置。

## 当前边界

- `kt-app` 主二进制只提供 GUI 入口：无参数、`--gui`、`--help`。
  历史上的 `--safe`、`--system-ssh`、`--show-log`、`--list` 会明确报错。
- 尚未实现多跳 `ProxyJump` 链。
- 触发器规则暂不可在 UI 中编辑，完整语法高亮尚未实现。
- 发布包目前未代码签名 / 未 Apple 公证，因此 macOS Gatekeeper 与 Windows SmartScreen
  可能需要用户手动确认。
- headless 客户端仅作为 `kt-core` 示例用于调试核心管线，不是主要产品入口。

## 快速开始

需要 Rust stable 1.85+。

### Linux 依赖

Ubuntu/Debian：

```bash
sudo apt install libwebkit2gtk-4.1-dev \
  libgtk-3-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  libxdo-dev \
  libssl-dev \
  pkg-config
```

macOS 和 Windows 本地开发不需要额外系统依赖。

### 启动应用

```bash
cargo run -p kt-app
```

入口检查：

```bash
cargo run -p kt-app -- --gui
cargo run -p kt-app -- --help
```

在 UI 中，从侧栏创建连接，选择认证方式后连接；如需复用连接，可保存会话。
保存的密码和私钥口令会进入加密保险库，不会写入 `config.toml`。

## 开发者地图

```text
kt-app
  Dioxus Desktop 入口、窗口/图标/菜单设置、最小 CLI 参数处理

kt-ui
  Dioxus 组件、AppState/Store 桥接、终端工作区、SFTP 侧栏、
  监控 UI、弹窗、设置、主机密钥/认证提示

kt-core
  SessionManager、russh 连接/认证、PTY shell、终端引擎、
  SFTP worker、远端监控、UI <-> core 消息协议

kt-config
  配置路径、TOML 模型、会话、应用设置、known_hosts、ssh_config 合并

kt-secrets
  Argon2id + XChaCha20-Poly1305 本机机密保险库
```

关键边界是 `kt-core`：它负责协议和终端行为，并且不依赖 UI。
`kt-ui` 只通过 `ToCore` / `FromCore` 消息与它通信，并渲染 selector 风格的轻量视图模型。

## 存储模型

- `config.toml`：非机密的会话配置与应用设置。
- `known_hosts.toml`：受信任主机密钥指纹与最近访问元数据。
- `secrets.vault`：加密存储密码与私钥口令。
- `secrets.vault.key`：当前安装独立生成的本机密码库密钥；应与 vault 一样保持私有。
- 旧固定密钥密码库会在启动时迁移到当前安装独立密钥。
- 无法自动打开的旧主密码保险库会备份为 `secrets.vault.legacy*`；
  新机密会继续写入新建的加密保险库。

机密值不应写入配置文件或日志。

## 验证

维护者门禁：

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

workspace 测试覆盖应用入口、配置与机密存储、SSH/终端/SFTP 核心行为、
UI 状态流转及纯 UI 逻辑。测试数会随覆盖持续增长，因此 README 不再维护
容易过期的固定统计。

核心集成测试
[`crates/kt-core/tests/roundtrip.rs`](crates/kt-core/tests/roundtrip.rs)
会在回环地址启动真实的进程内 `russh` 服务端，验证完整路径：
连接、密码认证、PTY、shell 数据、`TermEngine` 和 `GridSnapshot`。

## 发布自动化

GitHub Actions 有两条打包流程：

- `.github/workflows/release.yml`：`v*` 标签只有在阻断式 RustSec 依赖扫描通过后，
  才会创建正式 GitHub Release。
- `.github/workflows/alpha.yml`：任意分支 push 都可更新滚动 `alpha` 预发布。
  所有分支共享一个并发组，新 push 会取消旧构建；发布正文会记录来源分支与提交。
  RustSec 扫描只告警，不阻断 Alpha 发布。

两条 workflow 共用同一套六平台矩阵和产物命名：
Linux/macOS/Windows x `x64`/`aarch64`。Rust target triple 仍使用标准名称，
例如 `x86_64-pc-windows-msvc`。

## 路线图快照

- [x] 核心 SSH + 终端引擎
- [x] Dioxus desktop GUI
- [x] 会话持久化、加密保险库、SFTP、监控
- [x] 主机密钥信任流程、分屏、ssh-agent、ProxyJump、触发器高亮
- [x] UI 模块化与 selector 驱动的主界面面板
- [x] Release/Alpha 打包与维护治理
- [ ] 多跳 ProxyJump、可编辑触发器规则、更完整的终端高亮

## 许可证

Apache-2.0
