# 代理文档索引

## 技术治理
`index.md` - 项目代理文档索引，记录文档读取场景、测试验证要求与全局重要记忆。
`architecture.md` - KitonyTerms 整体架构、crate 职责、core/UI 协议、GUI 模块边界与 UI 抽离约定；修改跨模块逻辑、UI 编排、core 通信或持久化边界时必读。
`maintenance.md` - 长期维护规程，记录功能更新影响清单、轻量回归套件与季度治理核对；修改功能、协议、UI 状态或持久化语义前必读。

## 当前任务文档
`workflow/260628-implementation-roadmap.md` - 统一优化路线图：cover 安全、并发、性能、UI 模块化与文档治理，支持分阶段落地。

## 已归档完成任务文档
`workflow/done/260627-architecture-evolution.md` - 架构演进框架：记录早期入口能力对齐、Monitor 闭环、UI 拆分、安全策略与背压治理计划。

## 已归档完成任务摘要
- 稳定连接基线：补齐连接、会话生命周期与错误收敛的早期方案。
- README 第四阶段：同步功能里程碑、README 状态与功能声明。
- SFTP 文件管理：沉淀右键菜单、外部编辑菜单、保存确认对话和回传策略。
- 架构审查：确认项目适合继续维护，指出 UI 主组件过大、通道背压、vault 解锁、known_hosts 安全语义与认证能力缺口。

## 测试与验证要求
- Rust 代码变更后至少运行 `cargo fmt --all -- --check`、`cargo check --workspace --all-targets`、`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`。
- 若涉及 UI 行为、终端渲染、SSH/SFTP 交互或密钥处理，应补充对应 crate 的单元测试或集成测试。
- 如仅进行代码审查且未改动业务代码，可运行只读检查或现有测试来辅助判断。

## 全局重要记忆
- 项目为 Rust workspace，按职责拆分为核心协议与会话、配置解析、密钥存储、UI 与应用入口等 crate。
- UI 中接收 `Arc<Mutex<AppState>>`、`Arc<Store>` 或大量 `Signal` 的重状态入口优先使用普通函数返回 `Element`；仅展示型、props 可自然比较的单元使用 Dioxus `#[component]`。
- 主工作台子布局应优先接收 `app_logic.rs` 中的轻量 selector 视图（如 SFTP、Monitor、状态栏、会话标签），避免直接传递完整 `SessionState`。
- 每次功能更新前先按 `maintenance.md` 填写影响清单；新增 `app.rs` 之外模块逻辑时优先补纯逻辑单测，再接入渲染或副作用。
