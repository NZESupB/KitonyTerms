# 终端行号与模型选区

## 目标与边界

修复 gutter 与正文相贴、占位空格进入复制、视口 DOM 复用导致跨屏选区丢失。沿用 Dioxus + alacritty_terminal 0.26.0；仅覆盖仍在 scrollback 中的内容。列数变化遵循引擎行为清除选区，不新增原始 PTY 输出记录。

## 实施阶段

- [x] 确认布局、DOM 复用、复制入口及项目约束。
- [x] 核心保存带实例归属的选区；宿主/容器命令同路由，复制通过 oneshot 保序返回。
- [x] JS 指针捕获、拖拽滚动、复制与触摸选区桥；gutter 统一几何与分割线。
- [x] 核心回归、Rust 门禁、生产 DOM/CSS 浏览器夹具与收尾。

## 实现决策

- 选区锚点存于 Alacritty，读取 selection_to_string，保留软/硬换行与 Unicode 语义；不再从 DOM 字符串累计选区，不全局替换 NBSP 或 trim。
- 选区坐标携带快照版本和 display_offset，内容变化使旧坐标失效，由前端等待新帧重采样。
- gutter 内容宽度仍由 Rust 常量统一下发，CSS 统一计算左右留白、边线和正文间距。
- 快捷键、右键、手机菜单与原生 copy 事件共用模型复制。手机触摸滚动不由鼠标逻辑接管。
- owner 使用组件内非响应式缓存，在目标变化时同步生成新 nonce；目标切换往返不复用旧 owner，普通帧更新保持稳定。
- PointerEvent.detail 在 Chrome 的 pointerdown 中不能提供连续点击次数，脚本显式记录间隔/位置以保留双击选词与三击选行。复制完成仅在终端/菜单仍持有焦点或菜单被移除后恢复焦点，不抢占用户已切到的输入框。

## 验证状态

- `cargo fmt --all -- --check`、`cargo check --workspace --all-targets`、`cargo test --workspace`、普通与全特性 `cargo clippy --workspace --all-targets -- -D warnings`、JS 语法和 diff 检查全部通过。工作区共 606 项通过、1 项既有忽略；新增核心选区回归 12 项、Dioxus owner 生命周期回归 1 项，现有 SSH 回环宿主/容器测试补充模型复制与失效路由验证。
- Chrome 258 项断言通过：48 组 gutter/主题/字体/字号配置、七屏以上正反向选择、滚轮扩展、横向命中、立即复制、软硬换行与触摸宽字符映射、双击/三击、焦点恢复和过期快照重采样。夹具复用生产 Terminal DOM/CSS/JS 与真实 TermEngine；原生状态/选区生命周期适配只在 scratch，不代表生产 WebView 端到端验收。
- 初次单命令 workspace 测试因工具 60 秒预算中止，最终后台完整测试约 70 秒成功；首轮浏览器的双击选词失败修正后通过。owner 测试初版错误驱动 ROOT 包装层，改为 ScopeId::APP 后完成实际生命周期回归。
- 验证证据位于会话 scratch：`terminal-verification.json`、`terminal-gate-*.log`、`terminal-browser-results.json`、`test-terminal-browser.cjs`、`terminal-light.png`、`terminal-dark.png`。
- 剩余实机范围：macOS 原生 Edit 菜单、真实系统剪贴板和手机长按/手柄选择；浏览器不替代这些验收。内置面板当前未能显示会话，视觉证据为 Chrome 几何/计算样式断言及生成截图，不声称人工逐像素核验。
