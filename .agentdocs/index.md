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
- **界面压缩与 SFTP 修复(2026-06-26)**: 资源管理器与底部监控区收紧；SFTP 读取链路补齐投递失败错误、打开超时、独立 SSH fallback、目录读取超时与 UI 本地看门狗。
- **SFTP 反复重连修复(2026-06-26)**: 修正 Dioxus effect 订阅路径导致的重复 `List` 投递；SFTP 自动加载只执行一次，全局状态同步前先比较差异，同一路径 loading 时跳过重复请求。
- **工作台布局调整(2026-06-26)**: 保留系统原生标题栏，移除应用内重复标题栏；资源管理器与 SFTP 默认更窄，并支持拖动分隔条调整宽度；监控、渲染和键盘输入等高频日志降为 `debug`。
- **Windows 语言与启动体验(2026-06-27)**: Windows 二进制声明 GUI 子系统以隐藏运行时 cmd 窗口；Inno Setup 安装包配置中英文语言并按系统 UI 语言检测；应用新增 `AppSettings.language` 与设置面板语言切换。
- **集中 i18n 重构(2026-06-27)**: UI 多语言文案集中到 `crates/kt-ui/src/i18n/`，由 `mod.rs` 定义结构与语言选择入口，`zh_cn.rs`、`en.rs` 按语言维护完整文案。
- **界面布局精简优化(2026-06-27)**: 移除 nav-rail 与顶部命令栏，左侧改为分组连接树 + SFTP 目录 + 设置按钮的紧凑侧边栏；SFTP 独立面板从主布局移除（组件保留）；状态栏精简为 UTF-8 / 尺寸 / 延迟；连接按 `SessionProfile.group` 字段自动分组。
- **左侧分组、SFTP 与监控优化(2026-06-27)**: 分组持久化并支持新建/重命名/删除，连接弹窗可选择分组；右键菜单仅在有上下文功能的服务器与分组上显示，服务器项仅显示名称并支持编辑/删除/复制；左侧 SFTP 显示路径、返回/刷新与远端条目，分组区和 SFTP 区可纵向拖动；监控使用真实 CPU 核心数、采样延迟与百分比填充条，状态栏移除硬编码 UTF-8/尺寸/延迟。
- **SFTP 文件管理器增强(2026-06-27)**: 左侧 SFTP 改为可横向/纵向滚动的表格，显示名称、修改时间、大小、权限、用户/用户组；连接成功后自动加载目录，SFTP 变更操作完成后自动刷新；条目与空白区域右键菜单按上下文提供打开目录、刷新、复制路径/名称、新建目录、重命名、删除等入口，暂未接平台文件选择器的上传/下载/外部编辑器项保持禁用。
- **SFTP 编辑器回传与菜单布局(2026-06-27)**: 左侧分组区与 SFTP 区默认对半分；右键菜单渲染后按窗口空间自动向上/向左修正，并限制最大高度；SFTP 完成事件携带操作路径，外部编辑器流程支持下载到临时文件、系统默认编辑器打开、待回传条中选择回传/放弃/重新打开，上传完成后自动移除待回传项。
- **SFTP 外部编辑保存检测(2026-06-27)**: macOS 默认编辑器打开改为下载完成后调用 `/usr/bin/open` 传入绝对路径；移除常驻外部编辑回传条；本地文件保存后弹窗选择仅本次回传、本次打开期间自动同步或不回传，回传状态与百分比显示在底部状态栏；SFTP 右键菜单清理无闭环占位项，并修复文件间连续右键菜单无法切换的问题。

## 全局重要记忆

- **UI 与 core 完全经 channel 通信**:所有阻塞/异步 SSH/SFTP I/O 都在 `kt-core` 的 tokio 运行时;UI 只发 `ToCore`、收 `FromCore`,并按 `GridSnapshot` 重绘。
- **SFTP 复用 SSH 会话**:经 `SshShell::open_sftp` 在同一 russh 会话上开 `sftp` 子系统。
- **SFTP 请求必须闭环**:UI 发起 SFTP 请求后必须在成功、失败或超时中收敛;core 需要对投递失败、打开失败、读取超时返回 `SftpError`,并保留独立 SSH fallback。
- **SFTP UI 生命周期约束**:Dioxus `use_effect` 会订阅 effect 内读取的 Signal;SFTP 自动加载不得读取会被同步循环写入的 `current_path`,全局状态同步到本地 Signal 前必须先比较差异。
- **SFTP 自动加载**:连接成功后由 `AppState` 触发初始 `SftpRequest::List`，不要只依赖组件挂载 effect；`Upload/Mkdir/Remove/Rename` 完成后应刷新当前目录。
- **SFTP 外部编辑器流程**:外部编辑应走 `Download -> 本地临时文件 -> 系统默认编辑器 -> 监听本地保存 -> 用户选择回传策略 -> Upload`；不得在未收到 `SftpDone{op,path}` 前打开本地文件；不得使用常驻回传条，保存询问用弹窗，回传进度与结果放在底部状态栏。
- **机密与会话分离**:机密(密码/口令)只存 `kt-secrets` 加密 vault;会话(host/port/user/auth)明文存 `config.toml`。
- **布局设计**:左侧边栏（分组连接树 + SFTP 目录条目 + 底部设置按钮）+ 中央终端工作区 + 底部系统监控横条 + 精简状态栏。无独立图标导航条(nav-rail)，无顶部命令栏。
- **Dioxus UI 样式集中管理**:主界面长期采用深色工作台风格,新增视觉结构优先复用 `crates/kt-ui/src/assets/app.css` 的 class,避免在组件内继续扩散大段 inline style。
- **UI 图标统一入口**:界面内品牌标识与常用线性图标统一复用 `crates/kt-ui/src/components/icons.rs` 的 `AppLogo` / `Icon` 组件,样式集中在 `app.css`;新增图标按钮必须保留 `title` 说明。
- **外部应用图标统一资产目录**:应用窗口与平台外壳图标统一放在 `crates/kt-app/assets/`;运行时读取 `app-icon.png`,macOS 复制 `macos/KitonyTerms.icns`,Windows 使用 `windows/kitonyterms.ico`,Linux 使用 `linux/hicolor/` 与 `linux/kitonyterms.desktop`。release/debug 都必须直接引用这些入仓资产,不得在 CI 中重新绘制或临时生成品牌图形；外部图标视觉必须从 `kt-ui` 的 `AppLogo` 派生，避免应用内外品牌标识不一致。
- **Windows 安装包语言文件**:GitHub Actions 的 Chocolatey Inno Setup 环境不保证预装 `Languages\ChineseSimplified.isl`;release workflow 必须自行准备简体中文 `.isl`,并通过 `compiler:Default.isl,<repo-file>.isl` 兜底,避免 Windows 打包因缺少本机语言文件失败。
- **默认日志不落盘**:`kt-app` 只配置 `tracing_subscriber::fmt()` 输出到启动终端,没有文件 appender;监控/渲染/输入等高频日志应保持 `debug` 级别。
- **应用语言配置**:`AppSettings.language` 持久化 UI 语言，默认按系统环境推断；新增可见 UI 文案时必须接入 `crates/kt-ui/src/i18n/`，按语言文件维护，避免在组件内硬编码多语言文本。
- **主机密钥目前 TOFU**:`AcceptAllVerifier` 信任所有主机密钥。
