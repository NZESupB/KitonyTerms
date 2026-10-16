# 断线 Enter 重连与左侧文件管理布局

## 目标与依据

按已批准计划 `.pi/plan/修复断线-enter-重连与左侧文件管理布局-20261015-2006.md` 实施；范围限 kt-ui、必要回归测试与代理文档。保留 dist/ 和无关改动。

## 已确认问题

- Closed 清除 snapshot，断线界面使用不支持键盘/焦点的 TerminalPlaceholder。
- 正常 Terminal 的 Enter 只编码为 CR，没有断线状态门禁。
- 文件表格最小 680px、侧栏最大 320px，必然溢出；终端最小 420px 需纳入侧栏上限。

## 实施决策

- 使用状态锁内的 reconnect_disconnected_session 复用 connect_session，只允许断线会话重连。
- 桌面终端和占位页共享保守的焦点恢复策略，避开表单与弹窗；手机继续管理自己的输入桥。
- 完善现有侧栏拖拽，范围 176–720px；上限同时考虑实际运维区宽度和终端 420px 保留空间。
- 文件表头与行共用五列可收缩 Grid，单元格省略号和完整 title。

## 阶段与验证

- [x] 实现状态门禁、Enter 处理、提示与桌面焦点。
- [x] 实现侧栏可用上限、失焦收敛及表格自适应。
- [x] 补充状态、键盘与宽度回归测试。
- [x] 执行 fmt、workspace check/test/clippy、all-features clippy、diff 检查。
- [x] 验证浏览器布局与焦点策略，并记录原生 WebView 验收范围。
- [x] 自行审阅、更新长期约束并归档本任务。

## 验证记录

- `cargo fmt --all -- --check`、`cargo check --workspace --all-targets` 通过。
- `cargo test --workspace` 通过：505 个测试通过，1 项既有忽略；包含 10 项 SSH 回环集成测试和本次新增回归用例。
- 默认及 all-features 的 workspace/all-targets Clippy（`-D warnings`）通过；`git diff --check` 通过。
- 使用实际 CSS、`terminal_focus.js`、`sidebar_layout.js` 的独立浏览器夹具，在正常时钟的无界面 Chrome 下 40 项检查通过：浅/深色与 176/220/320/480/720px 宽度、五列对齐和横向溢出、长文本与状态文本、真实 RAF 焦点恢复、弹窗/输入/inert 保护、尺寸/失焦上报和清理后重新挂载。
- 虚拟时钟下 RAF 相关检查未通过，正常时钟复测全部通过；夹具与日志均在会话 scratch，无项目临时测试文件。
- 原生 Dioxus WebView 的实际 Enter 到 SSH 连接、实际鼠标拖拽和移动软键盘未端到端操作；Rust 状态/键盘单元测试、SSH 回环测试及浏览器脚本验证不能替代该原生验收。
