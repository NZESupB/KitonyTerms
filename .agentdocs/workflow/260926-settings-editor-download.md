# 设置拖动、编辑缓存与 SFTP 下载

批准快照：`.pi/plan/设置拖动-编辑器缓存清理与-sftp-下载优化计划-20260926-1508.md`。

## 实施边界

桌面单个文件或目录下载使用系统选择器；同名目录合并与文件覆盖需明确确认；拒绝符号链接和特殊文件。保留失败的编辑内容供重试，不把连接切换视为用户放弃编辑。

## 阶段

- [x] 设置打开时保留标题栏命中层级，设置头部提供专用拖动区。
- [x] 统一编辑副本删除、空目录回收、路径唯一性、显式结束与空闲副本卸载清理。
- [x] 实现递归下载、桌面右键入口、系统选择器、当前文件进度与覆盖确认。
- [x] 定向测试、完整门禁与差异自查。
- [ ] 真实桌面：设置打开/关闭时拖动，输入与关闭按钮，系统保存/目录选择器交互。
- [ ] Windows/Linux 桌面交互回归（构建侧已由 v0.1.1 发布流水线覆盖，见下）。

## 验证

本次执行通过：

- `cargo fmt --all -- --check`
- `cargo check --workspace --all-targets`
- `cargo test --workspace`（包括 SSH roundtrip；OpenSSH 专项默认 ignored，另行显式执行）
- `cargo test -p kt-core recursive_download_against_openssh -- --ignored`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `git diff --check`

发布流水线（`v0.1.1` = `fdfb69b`）各平台构建全部成功：Linux x64/aarch64、macOS x64/aarch64、Windows x64/aarch64、Android aarch64 与 iOS 未签名 IPA 均产出产物，RustSec audit 通过。该结果只作跨平台构建证据；真实窗口拖动与系统选择器交互仍未验收。

真实 OpenSSH 专项覆盖中文文件、单文件下载、嵌套目录、空目录、零字节文件、未确认覆盖拒绝、确认覆盖及远端符号链接拒绝。本地路径测试覆盖链接逃逸拒绝和新目标不可覆盖；编辑测试验证实际文件删除、空目录回收、重复清理及保存失败保留。

## 验收限制

computer-use 技能明确禁止自动化终端类应用，因此未通过该插件操作本应用。不可将上述代码检查标为真实窗口拖动验收。

外部编辑经系统默认程序启动时无法可靠检测第三方编辑器某个标签页关闭，新增应用内“结束外部编辑”入口作为明确的清理边界。正在传输及保存失败的副本不在组件卸载时强删。异常退出和第三方继续写入不保证即时清理。
