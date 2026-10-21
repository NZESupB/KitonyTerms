# Windows 更新与通知主题

## 目标与依据

按批准计划 `.pi/plan/修复-windows-应用更新循环与通知主题同步-20261020-2317.md` 修复 KitonyTerms 更新安装链路与通知主题；范围不包含 Git 写操作、发布或替换真实安装。

## 根因与决策

- 通知判断 `light`，上游传入 `default-light`，导致浅色 CSS 不生效。通知直接复用主窗口 `theme_class`，保留 App 唯一的系统主题监听。
- helper 规范化 updates，服务原先保留普通路径；journal 暂存父目录按字符串比较，Windows 扩展路径与普通路径不一致会阻断 GUI 确认及后续更新。服务创建目录后保存规范路径；校验时规范化两个父目录，兼容已有记录，保留阶段、安装身份与路径分量检查。
- Windows 外部调用不再直接使用扩展路径：`windows_path::external` 复用 dunce 的安全分量检查，处理盘符与 UNC，保留中文/空格；无法等价转换、设备路径或超长路径显式失败。转换仅用于 Shell 文件路径及 Inno `/DIR`、`/LOG` 参数，事务与摘要校验仍用规范路径。这是外部调用兼容性修正，尚无 Windows 实机证据证明其是用户所遇循环的唯一原因。
- 安装器日志为 `updates/update-install.log`；helper 错误上下文区分等待旧进程、安装执行及安装后身份验证。helper 必须收到 commit 并确认旧进程退出后才能安装；失败仍由持锁协调者回滚，新 GUI 验证成功后才清理备份。

## 实施阶段

- [x] 统一事务根目录及已有 journal 校验，补充生命周期回归。
- [x] 修复外部安装路径、添加安装日志与阶段错误，补充 helper 保护回归。
- [x] 通知复用主窗口主题 class，验证渲染与切换。
- [x] 执行 Rust 门禁、Windows 类型检查和自查。
- [x] 同步架构、维护入口与索引，归档实现记录。

## 验证证据

宿主为 macOS，实际执行结果：

- `cargo fmt --all -- --check`、`cargo check --locked --workspace --all-targets`、`cargo test --locked --workspace`、两组 all-targets Clippy（含 all-features，`-D warnings`）全部退出 0。工作区测试 593 通过、1 默认忽略；未将忽略用例记为执行通过。
- 定向：kt-update 60 通过，kt-app update_launcher 9 通过，kt-ui software_update 8 通过。新回归涵盖普通 GUI 目录与规范 helper 目录的确认/恢复/回滚，以及受控父进程退出、commit、abort 与等待超时。原有归档安装回归改为从规范 helper 目录 apply，再从普通 GUI 目录 recover。
- scratch 独立 crate 渲染生产通知组件及子组件，使用当前 CSS、生产主题函数与系统监听脚本；8 份 SSR 夹具在 360/900 宽度检查 154/154 断言通过。覆盖中英文、就绪/准备状态、背景/文字/强调色、按钮禁用和焦点、inert 外显示、边界布局；通过 Chrome 媒体模拟 light → dark → light，验证同一通知节点颜色更新与显式主题不被覆盖。深浅色截图已在浏览器面板查看。
- 浏览器 SSR 的主题消息适配仅用于 DOM/CSS 和监听核验，不代表原生 Dioxus 事件往返或 Windows WebView 端到端验收。
- `cargo check --locked -p kt-update --tests --target x86_64-pc-windows-msvc` 失败于 ring 原生构建，缺少 Windows SDK `assert.h`，未记为通过。
- scratch 复制当前实际 install.rs 及 elevation/regression/windows_path 子模块，网络、归档和服务使用类型适配，`cargo check --offline --tests --target x86_64-pc-windows-msvc` 退出 0；包括新增 Windows 测试的类型检查，不执行测试，也不等价完整应用构建。
- 额外的 scratch Windows 严格 Clippy 失败：既有代码两处 `needless_return` 与一处 Windows 清除只读属性的 `permissions_set_readonly_false`；对应语句不在本次修改中，未扩展修改范围。工作区两组宿主 Clippy 均通过。
- `git diff --check` 与 `git diff --cached --check` 通过；最终文件与文档链接复核完成。

原始日志与截图保存在本会话 scratch：`verification-current.json`、`workspace-tests.log`、`windows-full-check.log`、`windows-isolated-final.log`、`windows-isolated-clippy.log`、`browser-results.json`、`notice-default-light.png` 和 `notice-default-dark.png`。临时验证工具未写入产品目录。

## 原生验收边界

- 当前没有 Windows 运行器，尚未执行两个有效发布身份构建的安装版/便携版连续升级、真实 UAC 接受/取消、跨盘、真实 UNC、文件占用与 Windows WebView 系统主题切换。
- Windows 回归已纳入现有 kt-update/helper 测试入口，现有两条发布 workflow 会在身份注入前运行这些测试；本任务未触发远端流水线。
- 原生验收入口及日志定位见 [维护说明](../../maintenance.md)。代码实施与可用自动检查完成；真实 Windows 现象是否全部消除仍须上述原生验收确认。
