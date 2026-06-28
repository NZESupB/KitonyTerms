# KitonyTerms

[English](README.md) | **中文**

一个轻量、跨平台的 SSH 终端客户端，用 **Rust** 编写，采用 [Dioxus](https://dioxuslabs.com/) 构建——一个利用系统原生 WebView 作为 UI 层的现代桌面框架，同时保持 SSH/终端逻辑为纯 Rust 实现。

> **目标：** 快速启动、低内存占用、原生系统集成，在 macOS / Windows / Linux (x86_64 + aarch64) 上都是一个小巧的二进制文件。

## 当前状态

**功能可用** —— 核心 SSH 引擎、终端模拟、远端系统监控、SFTP 文件管理、主机密钥确认和交互式认证提示均已实现。UI 基于 Dioxus 0.7 desktop 构建。

| Crate | 职责 | 测试 |
|-------|------|------|
| `kt-secrets` | 主密码保险库：Argon2id + XChaCha20-Poly1305 加密存储机密（SSH 密码、私钥口令） | 6 ✅ |
| `kt-config` | TOML 会话配置与应用设置 + `~/.ssh/config` 解析/合并 | 20 ✅ |
| `kt-core` | SSH 客户端 (`russh`) + 终端引擎 (`alacritty_terminal`) + 会话编排 + SFTP 支持 + 远端系统监控。**无 UI 依赖。** | 29 ✅ |
| `kt-ui` | Dioxus 组件与 UI 状态：终端工作台、对话框、SFTP 树/操作、系统监控、selector 驱动的主界面 | 45 ✅ |
| `kt-app` | 主二进制：GUI-only Dioxus desktop 入口、CLI 参数校验、应用图标集成 | 8 ✅ |

**108 个测试通过；`clippy` 干净。** 核心验证是一个**进程内往返集成测试**（[`kt-core/tests/roundtrip.rs`](crates/kt-core/tests/roundtrip.rs)）：在回环地址上启动真实的 `russh` SSH 服务端，驱动完整的 `SessionManager` 路径：`连接 → 密码认证 → PTY → shell → 通道数据 → TermEngine → GridSnapshot`，断言服务端输出和回显的按键确实落入渲染网格。

### 当前功能

- **SSH 终端**：通过密码 / 公钥 / 交互式键盘认证连接
- **多会话与分屏**：标签页界面，每个会话独立 scrollback 和 resize，终端区支持水平/垂直双视图
- **终端特性**：真彩色、加粗/斜体/下划线/删除线、块状/竖线/下划线光标
- **交互式认证提示**：保存的机密缺失时，可在握手过程中采集密码、私钥口令和 keyboard-interactive 输入
- **会话持久化**：保存连接到 `config.toml`；密码和私钥口令加密存入主密码保险库
- **主密码保险库**：保险库解锁是显式操作；跳过解锁仍可连接，但禁用已保存机密的读写
- **SFTP 面板**：浏览远程文件系统、上传/下载文件、创建目录、删除/重命名，并支持通过本地编辑器编辑远端文件后回传
- **系统监控**：实时展示远端 CPU、内存、网络、磁盘、负载、运行时长和延迟摘要
- **SSH 配置集成**：读取 `~/.ssh/config` 获取主机别名、默认设置和单跳 `ProxyJump`
- **主机密钥信任库**：持久化 `known_hosts.toml`；未知或变化的指纹需要用户确认，可选择“仅允许一次”或“信任此主机”
- **ssh-agent**：支持本机 ssh-agent/Pageant 公钥认证，并可在 shell 会话中请求 agent forwarding
- **触发器高亮**：终端行文本按内置触发器规则进行高亮

### 尚未实现

- 多跳 ProxyJump 链
- 可编辑触发器规则和完整语法高亮
- 当前二进制不提供 `--safe`、`--system-ssh`、`--show-log`、`--list` 等非 GUI 降级入口

## 架构

```
kt-app (Dioxus desktop 二进制)
   ├─ kt-ui (Dioxus 组件)
   │    └─ 终端 / 对话框 / SFTP / 监控视图
   └─ kt-core (tokio 运行时，后台)
       ├─ ssh/      russh：连接、认证、PTY shell、SFTP 子系统
       ├─ term/     alacritty_terminal 封装 → GridSnapshot（已解析 RGB）
       ├─ monitor/  远端系统资源监控（CPU、内存、磁盘、网络）
       ├─ sftp/     SFTP 任务：列表/上传/下载/创建目录/删除/重命名
       └─ session/  SessionManager：每会话一个 task，UI⇄core 消息协议
            │                         │
       kt-config             kt-secrets
       (TOML + ssh_config)   (Argon2id + XChaCha20 保险库)
```

终端**引擎**（VT 解析、网格、scrollback）与**渲染**完全解耦：核心产出不可变的 `GridSnapshot`，颜色已解析为 24-bit RGB，因此渲染器无需依赖 `alacritty_terminal`。`alacritty_terminal` API（明确*不保证*稳定）完全隔离在 `kt-core/src/term/` 内。

### 会话与机密存储

- **会话**（`SessionProfile`：host/port/user/auth/…）为**非机密**，明文存储在 `config.toml` 中。
- **机密**（密码、私钥口令）按 vault id（`user@host:port`）索引并加密存入保险库，永不明文落盘。
- **主机密钥**（host/port/fingerprint）存储在 `known_hosts.toml`，用于检测远端主机密钥变化。
- 启动时保险库处于**锁定**状态，直到在解锁对话框输入主密码；跳过解锁仍可连接，但禁用已保存机密的读写。

## 技术栈

- **SSH：** [`russh`](https://crates.io/crates/russh) 0.61（纯 Rust、异步）
- **终端后端：** [`alacritty_terminal`](https://crates.io/crates/alacritty_terminal) 0.26（锁定版本）
- **UI 框架：** [Dioxus](https://dioxuslabs.com/) 0.7 desktop（wry + tao + 原生 WebView）
- **异步运行时：** `tokio`
- **加密：** `argon2`、`chacha20poly1305`、`zeroize`
- **配置：** `serde` + `toml`、`directories`

## 构建与运行

需要 Rust 工具链（stable，1.85+）和平台特定依赖：

### Linux (Ubuntu/Debian)
```bash
sudo apt install libwebkit2gtk-4.1-dev \
  libgtk-3-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  libxdo-dev \
  libssl-dev \
  pkg-config
```

### macOS
无需额外依赖 —— 使用系统 WebKit。

### Windows
无需额外依赖。

### 构建与运行
```bash
# 运行全部测试
cargo test --workspace

# 维护者使用的质量门禁
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# 启动 GUI
cargo run -p kt-app
# 或显式指定 GUI 入口 / 查看当前入口用法
cargo run -p kt-app -- --gui
cargo run -p kt-app -- --help
# 已移除入口会明确失败：--safe、--system-ssh、--show-log、--list
#   首次运行：设置主密码（可跳过）
#   点击 ➕ 新建 → 输入 host / user / 认证 → 连接
#   勾选"保存会话"以持久化；密码加密存入保险库
#   点击侧栏会话以重连（密码自动填充）
#   终端视图：点击聚焦，输入；鼠标滚轮滚动；Cmd/Ctrl +/− 缩放

# 试用 headless SSH 客户端（在终端中执行完整核心管道）
cargo run -p kt-core --example headless -- user@host
#   认证：尝试 ~/.ssh/config + 默认密钥，然后交互式键盘认证，最后密码
#   退出：Ctrl-]
```

## 路线图

- [x] **阶段一** —— 核心引擎（SSH + 终端 + 会话），端到端验证
- [x] **阶段二** —— Dioxus desktop UI：终端渲染、输入、连接对话框、多标签
- [x] **阶段三** —— 会话持久化（TOML + 保险库）、主密码、SFTP 面板、系统监控
- [x] **阶段四** —— `known_hosts` 信任库、分屏、ssh-agent 转发、ProxyJump、触发器/高亮
- [x] **阶段五** —— UI 模块化：状态控制器、主界面拆分、selector 驱动的 SFTP/监控/状态栏视图
- [x] **阶段六** —— 文档与工程收敛：README/架构/QA 路径/release note
- [x] **阶段七** —— 维护治理：影响清单、回归套件、季度架构核对

## 许可证

Apache-2.0
