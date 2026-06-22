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

无进行中任务。

## 全局重要记忆

- **UI 与 core 完全经 channel 通信**:所有阻塞/异步 SSH/SFTP I/O 都在 `kt-core` 的 tokio 运行时;UI 只发 `ToCore`、收 `FromCore`,并按 `GridSnapshot` 重绘。
- **SFTP 复用 SSH 会话**:经 `SshShell::open_sftp` 在同一 russh 会话上开 `sftp` 子系统。
- **机密与会话分离**:机密(密码/口令)只存 `kt-secrets` 加密 vault;会话(host/port/user/auth)明文存 `config.toml`。
- **布局设计**:参考 FinalShell + WindTerm,左侧会话列表 + 中央终端/SFTP + 右侧监控(可折叠)。
- **主机密钥目前 TOFU**:`AcceptAllVerifier` 信任所有主机密钥。
