# KitonyTerms 长期维护规程

修改功能、扩展协议、调整 UI 状态或更新持久化语义前必读。本文用于把路线图阶段 7 固化为可重复执行的维护流程。

## 功能更新影响清单

每次立项或开始实现前，先在任务文档中复制并填写以下清单；无法判断的项按“有影响”处理，先查代码再动手。

### 1. 通信协议
- 是否新增或变更 `ToCore`、`FromCore`、`SessionCmd`。
- 是否需要同步更新 `core_loop` 路由、`SessionTask` 处理、UI `pump_core_events` 穷尽匹配和 headless 示例兜底。
- 是否改变通道容量、投递策略或 `Render` 合并策略。

### 2. 状态机
- 是否新增 `SessionState`、`AppState` 或 Store 字段。
- 是否定义了 loading、pending、done、error、stopped、cancelled 的收敛路径。
- 会话关闭、连接失败、认证取消、SFTP/Monitor 停止时是否能清理等待态。

### 3. 持久化
- 是否读写 `config.toml`、`known_hosts.toml`、`secrets.vault`。
- 是否改变旧数据兼容逻辑、默认值或迁移策略。
- secret 值是否仍只进入 vault，不落入 `config.toml` 或日志。

### 4. 安全与失败闭环
- 主机密钥、认证、vault 自动初始化/旧库备份、secret 写入是否有明确失败反馈。
- 成功、失败、超时、取消、停止是否都能回到可预测状态。
- 是否避免使用 `AcceptAllVerifier` 进入 GUI 默认路径。

### 5. UI 与体验
- 是否新增弹窗、按钮、菜单、状态栏文案或设置项。
- 可见文案是否接入 `crates/kt-ui/src/i18n/`。
- 是否通过 `app_logic.rs` selector 或纯逻辑函数限制重状态扩散。

### 6. 验证与文档
- 是否补充单元测试或集成测试，且测试覆盖新增失败路径。
- 是否需要更新 `README.md`、`README.zh-CN.md`、`.agentdocs/architecture.md` 或当前任务文档。
- 是否通过必跑门禁：`cargo fmt --all -- --check`、`cargo check --workspace --all-targets`、`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`。

## 开发顺序

- 跨模块功能先写任务文档，拆成可独立验收的阶段；每阶段完成后更新 TODO 和自审记录。
- 新增 `app.rs` 之外模块逻辑时，优先先写纯逻辑单测，再接入渲染或副作用。
- UI 子布局优先接收 `app_logic.rs` 的轻量视图模型；只有展示型、props 可自然比较的单元才使用 Dioxus `#[component]`。
- `App` 保持主编排职责；事件泵、会话同步、host key prompt、外部编辑副作用继续放在 `state_controller`。
- core capability 按 `Request -> Loading/Pending -> Data/Done/Error/Stopped` 闭环设计，SFTP、Monitor、后续端口转发等能力保持同一生命周期语言。

## 轻量回归套件

完整门禁仍以 `cargo test --workspace` 为准。日常改动可先按影响范围运行以下轻量回归点，最后再跑完整门禁。

| 场景 | 推荐命令 | 覆盖重点 |
|---|---|---|
| 启动与 CLI 参数 | `cargo test -p kt-app` | 无参数/`--gui` 启动、`--help`、已移除入口明确失败、应用图标 |
| 连接失败闭环 | `cargo test -p kt-ui session_close_records_connection_error`；`cargo test -p kt-core ssh_open_timeout_turns_pending_connect_into_error`；`cargo test -p kt-core connection_generation_only_matches_current_session` | `Closed(error)` 写入 UI 错误状态，SSH 打开总超时不会卡住，旧连接认证答案不会路由到当前连接 |
| SFTP 失败与重试 | `cargo test -p kt-core sftp_request_for_missing_session_returns_error`；`cargo test -p kt-ui sftp_stopped_clears_loading_and_progress_without_overwriting_error`；`cargo test -p kt-ui stale_sftp_done_and_error_are_ignored_without_pending_request`；`cargo test -p kt-ui close_clears_sftp_outcomes_and_pending_requests`；`cargo test -p kt-ui auto_sftp_load_only_when_connected_and_idle` | 缺失会话返回错误、停止事件清理 loading/progress、无 pending 的迟到终态被忽略、关闭清空请求队列与历史结果、自动加载不重复触发 |
| SFTP 覆盖上传提交 | `cargo test -p kt-core upload_commit` | 服务器拒绝覆盖 rename 时用备份轮换完成覆盖、支持覆盖时只 rename 一次、提交失败把原文件改回原名、新建目标不做备份轮换 |
| 终端复制粘贴 | `cargo test -p kt-ui clipboard`；`cargo test -p kt-ui terminal_paste_input_normalizes_line_endings`；`cargo test -p kt-ui terminal_clipboard_shortcuts_follow_desktop_platform_conventions` | 空剪贴板不算错误、其余失败保留原始描述、粘贴换行归一化、平台快捷键映射 |
| Monitor 停机 | `cargo test -p kt-core monitor_request_for_missing_session_returns_error`；`cargo test -p kt-ui monitor_stopped_clears_loading_without_overwriting_error`；`cargo test -p kt-ui session_close_clears_monitor_pending_and_error_state` | 缺失会话返回错误、正常停止不覆盖旧错误、会话关闭清理等待态 |
| 远程运维只读查询 | `cargo test -p kt-core remote_ops`；`cargo test -p kt-core monitor`；`cargo test -p kt-ui components::operations`；`cargo test -p kt-ui operations_result_is_identity_checked_and_failure_keeps_old_snapshot` | systemd/ps/ss/Docker 解析、EOF 后退出状态、输出异常、监控字段、自动刷新退避、旧快照与请求 ID 校验 |
| 远程运维回环与隐私 | `cargo test -p kt-core --test roundtrip readonly_operations_use_allowlisted_exec_and_keep_stderr_out_of_events`；`cargo test -p kt-core --test roundtrip container_pty_is_isolated_validated_and_rendered`；`cargo test -p kt-core protocol_debug_does_not_include_remote_payloads`；`cargo test -p kt-core to_core_debug_redacts_sensitive_payloads` | 真实 loopback SSH exec/PTY、固定 allowlist、stderr 隔离、容器输入/resize/关闭、恶意容器 ID 拒绝、协议 Debug 脱敏 |
| Vault 自动加密存储 | `cargo test -p kt-ui missing_vault_is_created_and_unlocked_automatically`；`cargo test -p kt-ui app_managed_vault_reopens_and_reads_saved_password`；`cargo test -p kt-ui legacy_master_password_vault_is_backed_up_and_replaced`；`cargo test -p kt-ui legacy_vault_backup_path_does_not_overwrite_existing_backup`；`cargo test -p kt-ui store_auth_provider` | 自动创建/打开 vault、保存后重启可读、旧主密码 vault 有备份与状态提示、备份不覆盖、认证 provider 可直接读取已保存密码 |
| Host key 安全语义 | `cargo test -p kt-ui host_key_check_does_not_persist_unknown_before_trust`；`cargo test -p kt-ui temporary_host_key_allowance_is_consumed_once_without_persisting`；`cargo test -p kt-config known_hosts_check_requires_explicit_trust_for_unknown_host` | 未确认不持久化、仅允许一次不落盘、未知主机需显式信任 |

## 季度治理核对

每季度或 release 前执行一次：

1. 重新阅读 `.agentdocs/index.md`、`.agentdocs/architecture.md`、本文件和根 `AGENTS.md`。
2. 对照实际代码检查 CLI、UI 模块边界、core 协议、vault、known_hosts、SFTP、Monitor 描述是否仍准确。
3. 若 README 的功能声明、测试数量、命令清单与实现不一致，立即同步中英文 README。
4. 检查 `.agentdocs/workflow/`：完成任务移动到 `workflow/done/`；若 `done/` 超过 5 个文件，合并长期记忆到 `index.md` 后清理。
5. 跑完整门禁并在当前任务文档记录结果。
