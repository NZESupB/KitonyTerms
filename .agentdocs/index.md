# KitonyTerms 代理文档索引

本目录(`.agentdocs/`)仅面向 AI 代理,沉淀项目架构与执行约束,避免重复遍历代码。所有文档均为中文。

## 技术治理

- **项目形态**:Rust workspace(`resolver = 2`),纯 Rust SSH 终端,GUI 用 Dioxus Desktop(基于 WebView)。
- **crate 划分**:`kt-config`(配置/会话)、`kt-secrets`(加密 vault)、`kt-core`(SSH + SFTP + 终端引擎,无 UI 依赖)、`kt-ui`(Dioxus 组件库)、`kt-app`(桌面应用入口,二进制 `kitonyterms`)。
- **依赖管理**:版本集中在根 `Cargo.toml` 的 `[workspace.dependencies]`,各 crate 用 `xxx.workspace = true` 引用;新增依赖用 `cargo add` 并回填工作区。
- **构建/校验**(回传或提交前必须全绿):
  - `cargo check --workspace`
  - `cargo clippy --workspace`(不得引入新告警)
  - `cargo test --workspace`
  - 格式:对**改动文件**执行 `rustfmt --edition 2021 <file>`。
- **测试约定**:使用 Rust 内置 `#[test]`,不引入第三方测试框架;新增纯逻辑应补单元测试。
- **发布**:推送 `v*` 标签触发 [.github/workflows/release.yml](../.github/workflows/release.yml),构建 6 端产物(Linux/macOS/Windows × x86_64/aarch64)并创建 GitHub Release。

## 架构文档

- [`architecture.md`](architecture.md) — 整体架构:crate 职责、core 的 UI⇄core 消息协议、SSH/SFTP 层、GUI 面板与主题。**修改任意模块前必读**。

## 当前任务文档

- [`workflow/260625-stable-connection-baseline.md`](workflow/260625-stable-connection-baseline.md) — 稳定连接基线排查与 CLI/GUI 降级路径，修改应用启动、连接、SFTP 或监控失败处理时必读。

## 已完成任务摘要

- **Dioxus 重写(2026-06-22)**: UI 从 egui 切换到 Dioxus Desktop，保留 `kt-core`/`kt-config`/`kt-secrets`，新增 `kt-ui` 组件库与 `kt-app` 入口；Terminal、SFTP、Monitor、ConnectionDialog 独立组件化。
- **综合工作台重构(2026-06-24)**: 主页面转为左侧会话导航、中央终端工作台、辅助文件/监控区域与底部状态栏的信息架构；优先保证终端工作区面积。
- **连接后崩溃排查(2026-06-25)**: 降低连接成功后自动打开 SFTP/监控双通道的风险，减少高频 Render 日志，终端渲染增加边界兜底。
- **连接后工作台体验优化(2026-06-25)**: SFTP/监控按会话状态联动，SFTP 与监控需要避免无限 loading，并优先复用现有 `StartMonitor`、`SftpRequest` 协议。
- **参考图界面还原(2026-06-26)**: Dioxus 主界面改为深色 macOS 风格工作台：顶部窗口栏、窄导航、资源管理器、中央终端、右侧 SFTP、底部监控卡片和状态栏；样式集中在 `kt-ui/src/assets/app.css`。
- **界面压缩与 SFTP 修复(2026-06-26)**: 资源管理器与底部监控区收紧；SFTP 读取链路补齐投递失败错误、打开超时、独立 SSH fallback、目录读取超时与 UI 本地看门狗。详见 [`workflow/done/260626-tighten-ui-fix-sftp.md`](workflow/done/260626-tighten-ui-fix-sftp.md)。
- **SFTP 反复重连修复(2026-06-26)**: 修正 Dioxus effect 订阅路径导致的重复 `List` 投递；SFTP 自动加载只执行一次，全局状态同步前先比较差异，同一路径 loading 时跳过重复请求。
- **工作台布局调整(2026-06-26)**: 保留系统原生标题栏，移除应用内重复标题栏；资源管理器与 SFTP 默认更窄，并支持拖动分隔条调整宽度；监控、渲染和键盘输入等高频日志降为 `debug`。

## 全局重要记忆

- **UI 与 core 完全经 channel 通信**:所有阻塞/异步 SSH/SFTP I/O 都在 `kt-core` 的 tokio 运行时;UI 只发 `ToCore`、收 `FromCore`,并按 `GridSnapshot` 重绘。
- **SFTP 复用 SSH 会话**:经 `SshShell::open_sftp` 在同一 russh 会话上开 `sftp` 子系统。
- **SFTP 请求必须闭环**:UI 发起 SFTP 请求后必须在成功、失败或超时中收敛;core 需要对投递失败、打开失败、读取超时返回 `SftpError`,并保留独立 SSH fallback。
- **SFTP UI 生命周期约束**:Dioxus `use_effect` 会订阅 effect 内读取的 Signal;SFTP 自动加载不得读取会被同步循环写入的 `current_path`,全局状态同步到本地 Signal 前必须先比较差异。
- **机密与会话分离**:机密(密码/口令)只存 `kt-secrets` 加密 vault;会话(host/port/user/auth)明文存 `config.toml`。
- **布局设计**:参考 FinalShell + WindTerm,左侧会话列表 + 中央终端/SFTP + 右侧监控(可折叠)。
- **Dioxus UI 样式集中管理**:主界面长期采用深色工作台风格,新增视觉结构优先复用 `crates/kt-ui/src/assets/app.css` 的 class,避免在组件内继续扩散大段 inline style。
- **默认日志不落盘**:`kt-app` 只配置 `tracing_subscriber::fmt()` 输出到启动终端,没有文件 appender;监控/渲染/输入等高频日志应保持 `debug` 级别。
- **主机密钥目前 TOFU**:`AcceptAllVerifier` 信任所有主机密钥。
