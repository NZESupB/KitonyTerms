# 维护与验证

## 变更检查

按本会话项目指令执行：限定任务范围，保留正式回归测试；涉及跨模块变更时检查协议路由、状态收敛、持久化失败、安全边界及 UI/i18n。技术边界见[架构](architecture.md)。

Rust 变更的现有门禁：

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

涉及安全、网络、认证、外部命令或并发时，另执行适用的 `cargo clippy --workspace --all-targets --all-features -- -D warnings`。使用项目工具链；失败需区分代码问题与环境限制。

## 定向验证入口

下列命令用于定位回归，不替代适用的完整门禁，也不表示已经执行通过。

| 场景 | 命令与源码 |
|---|---|
| 配置与旧 TOML 兼容 | `cargo test -p kt-config`；[配置测试](../crates/kt-config/src/lib.rs) |
| 编辑器菜单 | `cargo test -p kt-ui editor_menu`；[编辑器测试](../crates/kt-ui/src/components/external_edit.rs) |
| SFTP 覆盖提交 | `cargo test -p kt-core upload_commit`；[提交测试](../crates/kt-core/src/sftp.rs) |
| shell 注入与输出过滤 | `cargo test -p kt-core shell_integration`；[shell 测试](../crates/kt-core/src/shell_integration.rs) |
| SSH、运维取消与容器 PTY | `cargo test -p kt-core --test roundtrip`；[回环测试](../crates/kt-core/tests/roundtrip.rs) |
| vault、信任与落盘 | `cargo test -p kt-ui store::tests`；[Store 测试](../crates/kt-ui/src/store.rs) |
| 移动打包 | `cargo test -p kt-app --test mobile_packaging_contract`；[契约测试](../crates/kt-app/tests/mobile_packaging_contract.rs) |

修改 shell 脚本时对改动文件执行 `bash -n`；文档变更检查本地链接和 `git diff --check`，包含暂存修改时检查 `git diff --cached --check`。

## 文档

依据本会话最新项目指令：内部文档存于 .agentdocs；复杂任务保留必要目标、决策与验证状态。完成才归档，超过五份时先提炼有效内容再删除旧文档，保留最新归档。未验收事项不得因清理而标记完成。

已跟踪范围由 [.gitignore](../.gitignore) 确定。索引仅维护入口；不重复保存无法对应代码、配置或验证证据的历史总结。
