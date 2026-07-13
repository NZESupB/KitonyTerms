# 代理文档索引

## 技术治理

`index.md` - 项目代理文档索引，记录文档读取场景、测试验证要求与全局重要记忆。
`architecture.md` - KitonyTerms 整体架构、crate 职责、core/UI 协议、GUI 模块边界与 UI 抽离约定；修改跨模块逻辑、UI 编排、core 通信或持久化边界时必读。
`maintenance.md` - 长期维护规程，记录功能更新影响清单、轻量回归套件与季度治理核对；修改功能、协议、UI 状态或持久化语义前必读。
`governance/rust.md` - Rust workspace 的开发、测试与安全审计基线，修改 Rust 代码时必读。

## 当前任务文档

无。

## 已归档完成任务文档

workflow/done/260713-restore-terminal-monitor-regressions.md - 恢复备份分支中的终端输入自动回底、硬盘监控、网络上下行纵排及磁盘总量解析，并补齐防回归测试。

workflow/done/260713-code-review-remediation-plan.md - 全量代码审查修复：完成认证并发、SFTP 数据完整性与请求关联、单实例/持久化、Host Key 队列、终端协议、代理边界、快捷键、菜单、README 与 CI/RustSec 治理。

workflow/done/260713-full-code-review.md - 全量代码审查：覆盖 core、配置与密钥、UI、应用入口、CI 和完整 Rust 门禁；当前结论为存在阻断项，不建议通过审批。

workflow/done/260705-security-audit.md - 安全审计：应用托管 vault 独立密钥、敏感日志脱敏、SFTP 临时文件权限、外部编辑命令解析与 HTTP CONNECT 输入校验。

workflow/done/260705-fix-terminal-wheel-scroll.md - 终端 scrollback 修复：滚轮向上回看历史、core 快照正确映射 alacritty 历史行坐标。

## 已归档完成任务摘要

- 稳定连接基线：补齐连接、会话生命周期与错误收敛的早期方案。
- README 第四阶段：同步功能里程碑、README 状态与功能声明。
- SFTP 文件管理：沉淀右键菜单、外部编辑菜单、保存确认对话和回传策略。
- 架构审查：确认项目适合继续维护，指出 UI 主组件过大、通道背压、vault 解锁、known_hosts 安全语义与认证能力缺口。
- 功能性问题优化(`260628-functional-optimizations`)：终端键位/尺寸、监控延迟与占用、主题入口、文件管理、服务器分组、SSH 密码保存与密钥登录。
- 架构演进框架(`260627-architecture-evolution`)：早期入口能力对齐、Monitor 闭环、UI 拆分、安全策略与背压治理计划。
- 统一优化路线图(`260628-implementation-roadmap`)：阶段 1~7 完成——安全、并发、认证、UI 模块化、文档收敛与长期维护规程。
- 界面与菜单体验修复批次(`260629-polish-menu-terminal-auth`/`260629-menu-polish-followup`/`260630-urgent-connection-ui-polish`)：macOS 系统菜单与设置入口、认证弹窗密码保存、TCP 延迟显示与高延迟颜色、监控色块、浅色主题、应用内顶栏移除与右键编辑入口等体验打磨。
- 连接对话框与编辑器设置：会话/代理使用左侧条件渲染选项卡，编辑器通过 PATH/macOS app/环境变量探测并以下拉选择，既有自定义命令必须保留。
- 移动端历史实验曾处理 Android 应用私有目录和稳定 APK 签名；当前代码与 CI 已不再提供移动端构建，不能把该历史方案视为现行能力。

## 测试与验证要求

- Rust 代码变更后至少运行 `cargo fmt --all -- --check`、`cargo check --workspace --all-targets`、`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`。
- 若涉及 UI 行为、终端渲染、SSH/SFTP 交互或密钥处理，应补充对应 crate 的单元测试或集成测试。
- 如仅进行代码审查且未改动业务代码，可运行只读检查或现有测试来辅助判断。

## 全局重要记忆

- 项目为 Rust workspace，按职责拆分为核心协议与会话、配置解析、密钥存储、UI 与应用入口等 crate。
- UI 中接收 `Arc<Mutex<AppState>>`、`Arc<Store>` 或大量 `Signal` 的重状态入口优先使用普通函数返回 `Element`；仅展示型、props 可自然比较的单元使用 Dioxus `#[component]`。
- 主工作台子布局应优先接收 `app_logic.rs` 中的轻量 selector 视图（如 SFTP、Monitor、状态栏、会话标签），避免直接传递完整 `SessionState`。
- 每次功能更新前先按 `maintenance.md` 填写影响清单；新增 `app.rs` 之外模块逻辑时优先补纯逻辑单测，再接入渲染或副作用。
- Store 启动时自动打开或创建应用托管加密 vault；当前安装会生成独立 `secrets.vault.key` 作为本机自动密码库密钥，旧固定密钥 vault 会原地迁移，旧主密码 vault 无法自动打开时备份为 `secrets.vault.legacy*` 后重建新 vault。
- Monitor 延迟优先 TCP connect 当前 SSH `host:port`，失败时回退 SSH 心跳；UI 中延迟合并到网络标题展示并用颜色分级提示高延迟。
- SSH 支持 TCP 级代理（`kt_config::ProxyConfig`：Direct/System/Socks5/Http）：`crates/kt-core/src/ssh/proxy.rs` 经代理建立到目标的 `TcpStream` 后交给 `russh::client::connect_stream` 握手，`connect_direct` 统一分派，`Direct`/System 未解析出代理时回退直连。System 读取 `ALL_PROXY/HTTPS_PROXY/HTTP_PROXY/SOCKS_PROXY`（大小写各一），只接受 `socks5/socks5h/socks/http` scheme；`https://` proxy 与未知 scheme 明确失败，HTTP CONNECT 的 IPv6 authority 使用 `[host]:port`。代理与 ProxyJump 组合时代理仅作用于最外层 TCP，目标段走 direct-tcpip。代理凭证不接入 vault，仅以 username+空密码尝试认证。
- 终端当前工作目录通过 OSC 7 获取：`session.rs::parse_osc7_cwd` 扫描 PTY 原始字节解析 `ESC]7;file://host/path`，发 `FromCore::Cwd` 写入 `SessionState.terminal_cwd`，供 SFTP 侧「跟随终端目录」按钮使用；无 shell 集成时为空。反向的 SFTP→终端用 `sidebar.rs::cd_command_for_path` 生成单引号安全的 `cd` 命令发送到终端。
- SFTP 外部编辑支持自定义编辑器：`AppSettings.default_editor`（默认编辑器命令，`{file}` 占位）与 `AppSettings.editors: Vec<EditorEntry>`（右键"打开方式"列表）。`external_edit.rs::open_local_file_with` + `build_editor_command` 解析命令模板，`ExternalEdit.editor_command` 贯穿下载→打开链路，缺省回退系统默认程序。设置 UI 用 `external_edit.rs::detect_editors`（PATH + macOS `.app` bundle + Linux/Windows 候选，按名去重）与 `env_editor_command`（`$VISUAL`/`$EDITOR`）下拉选择编辑器，不再让用户手填命令；既有非空命令以「自定义」option 保留不丢失。
- SFTP 外部编辑临时目录应保持本机私有权限；Unix 下目录使用 `0700`，下载目标文件使用 `0600`。
- 所有 SFTP 请求由 UI 分配 `SftpRequestId`；请求级 Listing/Progress/Done/Error 必须携带并按 ID 消费，Stopped/Closed 保持会话级语义。迟到列表和旧超时不得覆盖或终止新请求，同路径外部编辑任务不得按 path/op 猜测关联。
- SFTP 覆盖传输必须先写同目录唯一临时文件再原子 rename；远端不支持覆盖 rename 时安全失败并保留原文件，不允许先删除正式文件。
- GUI 通过数据目录 `kitonyterms.lock` 保证单实例；Config/KnownHosts 使用唯一临时文件原子替换，Config/vault 更新失败必须回滚内存状态。
- Host Key 待确认项使用 host/port/fingerprint 去重队列；用户操作只移除精确项并只处理匹配 host/port（含 ProxyJump）的会话。新信任落盘失败不得接受，可信 key 的 last_seen 保存失败仍允许连接并向状态栏告警。
- GUI 状态栏只展示需要用户注意的核心信息（错误、阻断性状态、重要迁移/初始化提示、正在进行的文件同步等）；不要把 host key 信任成功、一次允许成功、密码保存成功这类成功/过程/调试性质提示写入状态栏。
- 终端行号/时间戳 gutter：`AppSettings.show_line_numbers/show_timestamps`，`terminal.rs` 在 surface 左内边距带内绝对定位 gutter（resize 脚本按 padding 自动扣减，PTY 列数不受影响）；时间戳为尽力而为，用 `Rc<RefCell>` 跨帧记录每行内容签名与首见时刻。
- 终端处于 scrollback 历史视口时，任何非空用户输入都必须先恢复到实时底部并立即渲染；空输入或已经在底部时不增加 revision。修改 `SessionCmd::Input` 或 `TermEngine::scroll_to_bottom` 时必须保留对应回归测试。
- 系统监控固定包含 CPU、内存、硬盘、负载、网络五项；硬盘展示 `/` 根挂载点并使用 `df` 的总块数字段，网络下行/上行在同一卡片中纵向显示，loading/placeholder 结构必须同步。
- Windows/macOS/Linux 都会显示 Dioxus Desktop 原生菜单栏，必须通过 `desktop_menu.rs::app_menu` 显式覆盖默认 Window/Edit 菜单并保持一致；菜单内必须包含 Undo/Redo/Cut/Copy/Paste/SelectAll 预定义项，确保 WebView 聚焦输入框能正确处理编辑快捷键。
- 原生菜单启动时优先使用 `config.toml` 保存的界面语言，配置缺失或损坏时回退系统语言；运行时切换语言暂不重建原生菜单。
- CI 双轨：`.github/workflows/release.yml`（v* tag→正式 Release）与 `alpha.yml`（分支 push→滚动更新固定 `alpha` tag 的 Alpha 预发布），触发条件互斥；两者共用同一套 6 平台 build matrix 与打包逻辑，产物架构命名统一用 `x64` / `aarch64`（Rust target triple 仍使用标准 `x86_64-*` / `aarch64-*`），改打包流程需同步两个文件。
- Alpha 保留任意分支 push 触发，所有分支共享固定 concurrency group，后触发构建取消旧构建；Release RustSec 扫描阻断，Alpha RustSec 仅告警，workflow 默认权限只读、发布 job 单独授予 contents:write。
