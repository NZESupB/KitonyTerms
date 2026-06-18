# KitonyTerms

[English](README.md) | **中文**

一个轻量、跨平台的 SSH 客户端 —— [WindTerm](https://github.com/kingToolbox/WindTerm) 的精神继任者,用 **Rust** 编写,采用**原生 GPU 渲染**(不使用 Electron、不使用 WebView)。

> **目标:** 启动快、内存占用低、原生体验,在 macOS / Windows / Linux 上都是一个小巧的二进制。

## 当前状态

阶段一(核心引擎)与阶段二(可用 GUI)已**功能完成**。阶段三的打磨与高级特性仍在进行。

| Crate | 职责 | 测试 |
|-------|------|------|
| `kt-secrets` | 主密码保险库:Argon2id + XChaCha20-Poly1305,密码加密落盘(SSH 密码、私钥口令) | 6 ✅ |
| `kt-config` | TOML 会话档案与应用设置 + `~/.ssh/config` 解析/合并 | 9 ✅ |
| `kt-core` | SSH 客户端(`russh`)+ 终端引擎(`alacritty_terminal`)+ 会话编排。**无 UI 依赖。** | 13 ✅ |
| `kt-app` | egui/eframe + wgpu(Metal/Vulkan/DX12)GUI:标签页、终端渲染、输入、连接对话框、会话持久化、主密码解锁 | — |

**28 个测试通过;`clippy` 干净。** 核心最关键的验证是一个**进程内往返集成测试**([`kt-core/tests/roundtrip.rs`](crates/kt-core/tests/roundtrip.rs)):它在回环地址上起一个真实的 `russh` SSH 服务端,让真实的 `SessionManager` 跑通整条链路:`连接 → 密码认证 → PTY → 通道数据 → TermEngine → GridSnapshot`,断言服务端输出与回显的按键确实落进了渲染网格。GUI 在 macOS 上通过 wgpu Metal 后端启动,并用 egui 的 painter 绘制网格。

### 当前可用功能

- 通过对话框连接 SSH 主机(密码 / 公钥 / 选择 agent)
- GPU 加速的终端视图渲染交互式 PTY shell
- 多标签并发会话;每个标签独立 resize、scrollback、字号缩放
- 真彩色、加粗/下划线/删除线、块状/竖线/下划线光标
- `~/.ssh/config` 查找 + 默认私钥文件(headless 示例)
- **会话持久化**:连接成功后可保存会话到 `config.toml`,密码加密存入 vault
- **主密码解锁**:首次运行设置主密码,之后启动时解锁;可跳过
- **侧栏会话列表**:点选重连(自动回填已保存密码)、删除会话

### 尚未接通

- 真正的 `known_hosts` 信任库(GUI 当前首用即信任,TOFU)。
- 握手过程中的异步认证弹窗(GUI 的密码是连接前在对话框收集的)。
- SFTP、分屏、ssh-agent 转发、ProxyJump、触发器/高亮。

## 架构

```
kt-app (egui/eframe + wgpu)         ← UI 线程,立即模式渲染
   │  ToCore(输入/resize)   ▲ FromCore(渲染快照/事件)
   ▼  走 channel             │
kt-core (tokio 运行时,后台)
   ├─ ssh/      russh:连接、认证(密码/公钥/交互式)、PTY shell
   ├─ term/     alacritty_terminal 封装 → 可跨线程的 GridSnapshot(已解析为 RGB)
   └─ session   SessionManager:每会话一个 task,UI⇄core 消息协议
        │                         │
   kt-config             kt-secrets
   (TOML + ssh_config)   (Argon2id + XChaCha20-Poly1305 保险库)
```

终端**引擎**(VT 解析、网格、scrollback)与**渲染**完全解耦:核心产出一份不可变的 `GridSnapshot`,颜色已解析为 24-bit RGB,因此渲染器无需依赖 `alacritty_terminal`。alacritty 的公开 API(明确*不保证*稳定)被完全隔离在 `kt-core/src/term/` 内。

### 会话与机密的存储

- **会话**(`SessionProfile`:host/port/user/auth/…)**非机密**,明文存于 `config.toml`。
- **机密**(密码、私钥口令)按 vault id(`user@host:port`)存入加密 vault,永不明文落盘。
- 启动时 vault 处于**锁定**状态,直到在解锁对话框输入主密码;跳过解锁仍可连接,仅无法读写保存的密码。

## 技术栈

- **SSH:** [`russh`](https://crates.io/crates/russh) 0.61(纯 Rust、异步)
- **终端后端:** [`alacritty_terminal`](https://crates.io/crates/alacritty_terminal) 0.26(锁定版本)
- **GUI/GPU:** `eframe`/`egui` + `wgpu`
- **异步:** `tokio`
- **加密:** `argon2`、`chacha20poly1305`、`zeroize`
- **配置:** `serde` + `toml`、`directories`

## 构建与运行

需要 Rust 工具链(stable,1.85+)。

```bash
# 运行全部测试
cargo test

# 启动 GUI
cargo run -p kt-app
#   首次运行:设置主密码(可跳过)
#   点 ➕ 新建 → 填 host / user / 认证 → 连接
#   勾选"保存会话"可把会话写入侧栏,密码加密存入 vault
#   侧栏点会话名 → 重连(自动回填密码)
#   点终端聚焦后输入;鼠标滚轮翻 scrollback;A+/A− 缩放

# 试一下 headless SSH 客户端(在终端里跑通整个核心链路)
cargo run -p kt-core --example headless -- user@host
#   认证:依次尝试 ~/.ssh/config + 默认密钥、交互式、密码
#   退出:Ctrl-]
```

## 路线图

- [x] **阶段一** —— 核心引擎(SSH + 终端 + 会话),端到端验证
- [x] **阶段二** —— GUI:wgpu 终端渲染、输入、连接对话框、多标签
- [x] **阶段三(部分)** —— 会话持久化(TOML + vault)+ 主密码解锁 + 侧栏会话列表
- [ ] **后续** —— `known_hosts` 信任库、分屏、SFTP 面板、ssh-agent、ProxyJump、触发器/高亮

## 许可证

Apache-2.0
