# 终端选区桥生命周期

## 目标与依据

执行已批准计划 `.pi/plan/修复终端拖拽失效-保持选区桥存活并验证跨屏复制-20261022-1422.md`。恢复鼠标拖拽，保留 Alacritty 缓冲区选区与文本提取，不修改选区坐标协议或复制语义。

已确认：Dioxus Desktop 0.7.10 在 eval 返回时关闭回复通道；选区脚本同步返回导致首个 begin 的回复无法到达，后续 extend/copy 被有序队列阻塞。原生选择已被 preventDefault 和 CSS 禁用。旧浏览器夹具没有模拟通道关闭行为。

## 实施阶段

- [x] 添加生命周期契约回归并验证旧实现失败：8 项中 6 项因 eval 提前结束失败，2 项缺少节点的退出测试通过。
- [x] 用返回的 Promise 保持 eval 存活，缺少 DOM 与 cleanup 时结束；生产改动仅在 `terminal_selection.js`。
- [x] 验证 Node 回归、真实 Dioxus 通道、生产 DOM/真实 TermEngine 浏览器场景与 Rust 门禁；原生实机边界见下。
- [x] 审查差异，更新架构/维护入口和索引，归档本任务。

## 验证结果与入口

- `node --test crates/kt-ui/tests/terminal_selection_bridge.cjs`：8 项通过，覆盖有序 begin/extend/copy、立即复制、第二次选择、清理与指针捕获释放、缺少节点、相同/不同 bridgeKey 重装、迟到回复、触摸和外部选择不被拦截。
- `node --check crates/kt-ui/src/assets/terminal_selection.js`、`cargo fmt --all -- --check`、`cargo check --workspace --all-targets`、普通及全特性 Clippy（`-D warnings`）通过。
- `cargo test -p kt-core term::selection`：12 项通过；`cargo test -p kt-ui owner_survives_rerenders`：1 项通过。
- 首次 `cargo test --workspace` 失败：`single_instance::tests::second_lock_is_rejected_until_first_lock_is_dropped` 在释放后的再次获取锁断言失败。该测试使用独立临时目录，与已运行应用无同路径锁竞争；当时并发运行其他子进程测试，未改动该无关模块。该用例单独运行通过，随后 `cargo test --workspace -- --test-threads=1` 完整通过（606 通过、1 项既有忽略）。不能把默认并发全量记为通过，具体并发根因本轮未证明。
- Chrome：267 项断言通过、0 失败、控制台无错误。复用当前生产 Terminal SSR/样式/脚本与真实 TermEngine，加载锁定 Dioxus 的真实 `native_eval.js`，按 AsyncFunction 完成后 close 的契约运行，回复经 `window.getQuery(id).rustSend()`。跨七屏正反向、途中变向、滚轮/自动滚动、释放后滚动、行节点替换、立即复制、重复反向选择、双/三击、触摸端点、横向滚动、焦点、过期快照，以及缩进/内部空格/空行/Tab/NBSP/中文/组合字符/Emoji/软硬换行均通过。
- 证据位于本会话 scratch `/Users/nzesupb/.pi-desktop/scratch/6010fcb6-d585-4580-9904-92a8169bca40`：`selection-bridge-before.log`、`selection-verification.json`、`selection-gate-*.log`、`terminal-browser-results.json`、`terminal-browser-run.log`。夹具服务及 worker 已关闭。

## 验证边界

Node 的最小 DOM 与 Chrome 中的真实通道源码不等同真实桌面 WebView 端到端。未替换/重启本机已安装应用，重新构建的原生 WebView、系统剪贴板、macOS 菜单与手机触摸手柄仍待实机验收。宿主/容器路由与 owner 切换由现有 Rust 回归覆盖，浏览器夹具的原生状态适配不声称为真实 SSH 会话切换验证。

保留 scrollback 淘汰与列数变化时的引擎既有边界。正式回归测试保留，无新依赖；临时工具和日志在 scratch，无 Git 暂存、提交、推送或发布。
