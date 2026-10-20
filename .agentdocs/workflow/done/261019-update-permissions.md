# 更新交互与 Windows 权限链路

## 目标与范围

依据已批准方案 `.pi/plan/移除更新确认页并修复-windows-更新权限链路-20261019-1916.md`：移除更新确认页；修正只读句柄刷盘；补齐便携版受保护目录的暂存、UAC、恢复与错误传播；增加 Windows 回归入口。不执行 Git 写操作或发布。

## 实施阶段

- [x] 移除确认页与独占样式、脚本、文案，保留安全收尾和原位状态。
- [x] 修正同步复制；实现兼容旧 journal 的用户目录事务及目标同卷替换。
- [x] 补齐提权参数、校验、结果传递及恢复，保留普通权限 GUI 重启。
- [x] 增加正式回归和 Windows 流水线检查。
- [x] 完成本机门禁、Windows API 类型检查及自查，同步架构与验证说明。

## 已落地的约束

- 普通协调进程持有 helper/安装互斥；旧 GUI 退出且 commit 后才修改目标。按钮直接进入原有安全流程，保留重复点击、未完成工作、握手后复查及 SSH 断开保护，主界面 `inert` 仍使用 `then_some(true)`。
- 更新目标、辅助程序、包、暂存及备份均校验身份；管理员入口显式接收原用户事务目录和 journal 摘要，不重新发现管理员账户目录。拒绝 reparse point，固定输入文件和目录句柄。
- Windows 新事务暂存及备份位于原用户 updates；旧 journal 缺少 storage 时继续按 Legacy 解析。文件刷盘使用写句柄；跨卷载荷先复制到目标目录唯一临时文件，再同卷替换。
- 只有本次目标的权限错误可进入一次 UAC 重试；共享/锁/映射占用、明确只读目标和辅助目录错误不产生无效提权标记。原始系统错误保留在错误链中。
- UAC 取消与文件占用分别报告；未确认管理员子进程退出不并发恢复。普通协调进程统一恢复；失败保留 journal、备份及具体原因，不能报告为恢复成功。原目标未变化时无需再次授权。
- 管理员入口仅替换/恢复文件，普通进程重开 GUI；只有匹配的 committed 事务经 GUI 启动确认才清理备份与提权/失败提示。普通启动不清除失败标记。

## 已执行验证

宿主为 macOS，以下均有实际执行结果：

- `cargo test --locked -p kt-update`：59 通过；最后一次 Windows 分类变更后再次通过。
- `cargo test --locked -p kt-app --bin kitonyterms update_launcher`：7 通过；最后一次只读目标/路径范围回归补充后再次通过。
- `cargo test --locked -p kt-ui software_update`：7 通过。
- 工作区格式、all-targets check、test、Clippy、all-features Clippy 均退出 0；工作区测试 581 通过、1 忽略。最后一次辅助程序修正后，格式及 all-features Clippy 再次通过。
- 清单契约 `python3 -B .github/scripts/test-update-manifest.py`：8 通过。
- 生产 `UpdateView` SSR fixture：16 种状态组合，在 360/900 宽度检查 274/274 断言通过。覆盖中英文、深浅色、就绪/提权/准备/失败、旧对话框 DOM 缺失、按钮焦点、无横向溢出、reduced-motion 和无未捕获错误；不代替原生事件与安装行为验收。
- CSS 语法及重复声明检查通过；813 个顶层节点。
- 实际安装模块及测试的隔离 Windows x64 API 类型检查通过；仅证明类型与 feature 契约，不是完整应用的原生构建或执行。
- `git diff --check`、`git diff --cached --check` 通过。发布 workflow 增加 Windows 模块及 helper 测试，位于发布身份注入之前，PowerShell 显式传播失败退出码；本次未触发远端流水线。

## 验证边界与迁移

- 完整 Windows 交叉检查在 `ring` 编译阶段因缺少 Windows SDK 头文件 `assert.h` 失败；没有把该检查记录为通过。
- Windows 10/11 原生构建、真实 UAC 接受/取消、另一管理员凭据、Program Files ACL、C/D 跨盘、空格/中文路径、EXE 共享/映射占用、只读/写保护、故障恢复及普通权限 GUI 重启仍待 Windows 环境执行。验证入口与场景见 [维护文档](../../maintenance.md)。
- 已发布旧版运行自身更新器；若在辅助副本刷盘阶段失败，需要手动安装一次修复版本。随后使用两个带有效发布身份的隔离构建验收连续更新；默认 development 构建不能用于真实更新验收。
- 没有替换本机真实安装应用，没有提交、推送或发布。本任务实现与可用自动检查已完成，原生验收按上述边界继续跟踪。
