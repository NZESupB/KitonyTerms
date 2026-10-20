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

发布流水线的额外门禁：`release.yml` 在打包前执行 `cargo audit` 且失败即阻断发布（`alpha.yml` 同样扫描但只告警）。豁免集中声明在仓库根目录 `.cargo/audit.toml`，当前仅豁免上游无修复版本的 `RUSTSEC-2023-0071`（rsa，经 russh/ssh-key 引入）；新增豁免需要说明依据，并在上游修复后移除。

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
| 命令历史采集与提示符顺序 | `cargo test -p kt-core command_history`、`cargo test -p kt-core history_prompt_marker`、`cargo test -p kt-ui command_history`；[采集器](../crates/kt-core/src/command_history.rs) |
| 保存密码查看的代次隔离与存在性 | `cargo test -p kt-ui password_field`、`cargo test -p kt-ui secret_presence`；[字段状态](../crates/kt-ui/src/components/password_field.rs)、[系统认证](../crates/kt-ui/src/os_auth.rs) |
| 移动打包 | `cargo test -p kt-app --test mobile_packaging_contract`；[契约测试](../crates/kt-app/tests/mobile_packaging_contract.rs) |
| 桌面后台更新与安装事务 | `cargo test -p kt-update`；[本地 HTTP](../crates/kt-update/src/tests.rs)、[归档](../crates/kt-update/src/archive.rs)、[安装](../crates/kt-update/src/install.rs) |
| 更新辅助握手、身份探测与锁竞争 | `cargo test -p kt-app update_launcher`；[辅助程序](../crates/kt-app/src/update_launcher.rs) |
| 更新直接重启与安全保护 | `cargo test -p kt-ui software_update`；[界面](../crates/kt-ui/src/components/software_update.rs) |
| 桌面清单与发布契约 | `python3 -B .github/scripts/test-update-manifest.py`；`bash -n .github/scripts/publish-alpha.sh` |
| 界面字体与缩放令牌 | `cargo test -p kt-ui components::fonts`（字体名、CSS 转义、平台回退）、`cargo test -p kt-ui style_contract`、`cargo test -p kt-config ui_font`；[字体处理](../crates/kt-ui/src/components/fonts.rs)、[样式不变量](../crates/kt-ui/src/style_contract.rs)、[配置字段](../crates/kt-config/src/lib.rs) |
| 更新代理（含 HTTP/SOCKS5 隧道与失败分类） | `cargo test -p kt-update proxy`；[代理实现](../crates/kt-update/src/proxy.rs)、[本地代理与隧道](../crates/kt-update/src/tests.rs) |

修改 shell 脚本时对改动文件执行 `bash -n`；文档变更检查本地链接和 `git diff --check`，包含暂存修改时检查 `git diff --cached --check`。

## 文档

依据本会话最新项目指令：内部文档存于 .agentdocs；复杂任务保留必要目标、决策与验证状态。完成才归档，超过五份时先提炼有效内容再删除旧文档，保留最新归档。未验收事项不得因清理而标记完成。

已跟踪范围由 [.gitignore](../.gitignore) 确定。索引仅维护入口；不重复保存无法对应代码、配置或验证证据的历史总结。

## 桌面更新原生验收

发布清单须包含 macOS ARM64、Windows AMD64、Linux x64/aarch64 四个 target；macOS 仅 DMG，Windows 同时包含便携 ZIP 与安装器。发布契约测试固定该范围及 Node.js 24 Action 版本。Alpha 使用 GitHub run ID/attempt，正式 tag 必须匹配 Cargo 版本。使用隔离应用副本和数据目录验收，不替换开发环境应用。

- Windows：用户目录便携版不提权；Program Files 下便携版与安装版 UAC 接受/取消；标准账户使用另一管理员凭据仍访问原用户事务；跨 C/D 盘及空格/中文路径；32/33/1224 和错误 5 掩盖的占用不循环提权，只读文件/写保护/ACL 错误保留原因；安装路径不漂移，GUI 由原普通进程重开；替换后身份失败及运行中 EXE 中断恢复；恢复取消保留 journal/备份；单实例竞争。两种主题点击均无二次确认，准备失败恢复交互。
- macOS：签名 `.app` 的整包交换/回滚、UI 启动确认后清理备份、DMG/App Translocation/不可写目录错误；测试包含真实 DMG 的只读挂载、签名包更新/回滚、损坏镜像及越界链接清理和遗留挂载回收。原生镜像测试按完整生命周期互斥执行，镜像创建也使用有超时与输出上限的 hdiutil 调用；不代替 GUI 验收。
- Linux：实际二进制路径和执行权限、只读目录、旧进程等待、单实例竞争、恢复和重启。
- 三端：编辑器/外部编辑/SFTP pending/同步阻止重启，后台下载期间终端正常工作，准备失败应用保持运行；断网、限流、Alpha 资产切换可重试。

跨 target 的 API 类型检查仅证明类型契约；缺少 SDK、链接器或原生运行器时必须单独记录，不能等同完整构建/安装通过。

### Windows 更新验证入口

在 Windows 开发终端先运行 `cargo test --locked -p kt-update --target x86_64-pc-windows-msvc`、`cargo test --locked -p kt-app --bin kitonyterms update_launcher --target x86_64-pc-windows-msvc` 和 `cargo test --locked -p kt-ui software_update`。两条发布 workflow 在注入发布身份前执行前两组测试，避免现有 development 身份测试被正式/Alpha 元数据改变。

端到端使用两个具有有效发布身份的隔离构建，开发默认构建禁用更新。一次手动安装修复版可绕过已发布旧更新器自身的刷盘错误，再验收连续自动更新。不在开发机真实安装目录进行故障注入。

核对 updates 下的 `error.json`、`journal.json`、`elevated-result.json`：错误应包含失败阶段/路径和系统原因；取消且原目标未改动时旧版可重开，成功 GUI 确认后事务/备份清理。需要定位时用 Process Monitor 按应用/辅助进程及目标路径筛选 CreateFile、FlushBuffersFile、重命名结果，用任务管理器检查重开 GUI 的权限；仅把实际执行的原生场景记为通过。超时的管理员进程仍运行时，协调进程会保留锁等待其退出，应检查该进程而非手动删除记录。
