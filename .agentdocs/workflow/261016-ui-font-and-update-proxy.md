# 界面字体配置与在线更新代理

## 目标

1. 在既有终端字体配置之外，增加软件界面（UI）字体族自定义与界面字号缩放：配置项定义、持久化、变更后即时生效。
2. 为在线更新增加代理配置（系统代理 / 直连 / HTTP / HTTPS / SOCKS5），使检查更新与下载更新包都走代理，并妥善处理连接超时与异常状态。

## 已确认决策

| 议题 | 结论 |
|---|---|
| 界面字体范围 | 字体族 + 界面字号（用户选定）：字体族走 CSS 变量，字号走 `--kt-text-*` 尺寸令牌 |
| 更新「系统代理」语义 | 环境变量（`ALL_PROXY`/`HTTPS_PROXY`/`HTTP_PROXY`/`NO_PROXY`），与 `kt-core` 的 TCP 代理 `System` 一致；不启用 reqwest `system-proxy`（否则 WebDAV 同步会被动改道，并引入平台专有依赖） |
| 代理凭证 | 仅用户名、密码留空（沿用 `ssh/proxy.rs` 先例）；需要密码时用 System 模式并在环境变量 URL 中携带，凭证不落盘 |

## 关键实现与理由

- **界面字体族/缩放注入两个渲染根**（`app.rs` 的 `.kt-window` 与软件更新弹层 `.software-update-layer`）：`font-family` 是继承的*计算值*，只在祖先声明无法被内联到子树的变量影响；两处统一走 `theme::interface_root_style`，避免漂移。
- **界面文字尺寸令牌化**：`app.css` 原本 174 处 `font-size: Npx`，其中 166 处改成 `var(--kt-text-N)`（令牌 `calc(Npx * var(--kt-ui-scale, 1))`，在 `:root`、`.kt-window`、`.software-update-layer` 三处声明），2 处 iOS 保护改成 `max(16px, var(--kt-text-16))`，6 处终端区（`.terminal-surface`/`.terminal-gutter`/`.terminal-placeholder`）保留 px——那里字号由终端设置内联决定且行高按 `em` 绑定，不随界面缩放。终端列数由 JS 探针实测，与缩放无关。
- **守卫测试** `kt-ui/src/style_contract.rs`：除白名单外不得出现 px 字号，并校验令牌在三个根齐全，防止后续新增文字漏掉缩放。
- **更新代理数据层** `kt-update/src/proxy.rs`：`UpdateProxy` 五种模式；System 不改动 reqwest 默认（自动读环境变量），Direct 用 `no_proxy()`，Http/Https 用 `Proxy::all` + `basic_auth(user, "")`，Socks5 用 `socks5h`（域名交由代理解析）并把用户名放在 URL userinfo（reqwest 的 SOCKS5 认证只认 userinfo）。新增依赖仅 `url`（编码用户名）与 reqwest `socks` 特性（无新依赖）。
- **服务层**（`kt-update/src/lib.rs`）：`Preferences` 增加 `proxy`（设备本机偏好，不参与配置同步）；`client` 改为 `Mutex<ReleaseClient>`，改代理时重建客户端并同时取消在途检查/下载；`State.generation` 让过期任务（被取消或代理已替换）的写回失效，避免旧结果覆盖新状态。改代理失败不落盘、不改内存；启动读到非法代理回退 System 并把原因写进快照 `message`（坏配置不应阻断启动，但也不能静默）。
- **失败分类** `describe_failure`：407 → 代理认证失败、超时、无法连接更新服务器或代理、重定向被拒、服务器错误状态，显式代理时追加「请检查代理地址与端口」，并保留底层错误文本。

## 阶段与 TODO

1. [x] kt-config：`ui_font_family` / `ui_font_scale` 字段、归一辅助、向后兼容与字号归一测试
2. [x] kt-ui fonts.rs：抽出 `sanitize_font_family`，新增 `css_ui_font_family`（未配置返回 `None`）
3. [x] app.css：令牌块、两个根的字体族、166 处 px→令牌、3 处 `max(16px, …)` 保护
4. [x] app.rs / theme.rs：`interface_root_style` 统一注入，更新弹层新增 `ui_font` / `ui_scale` props
5. [x] settings.rs + i18n：通用分类新增「界面字体」「界面字号缩放」两行与中英文案
6. [x] style_contract 守卫测试
7. [x] kt-update：proxy.rs、client.rs 统一构建路径、Cargo（`url`、reqwest `socks`）
8. [x] kt-update：`Preferences.proxy`、client Mutex、generation 隔离、`preferences()` 重写、失败分类、非法配置回退
9. [x] kt-update 测试：HTTP 代理转发（绝对形式请求行）、SOCKS5 隧道与用户名、407、超时、改代理取消并丢弃结果、非法代理拒绝与回退
10. [x] 更新界面：`UpdateProxyCard` + `on_proxy` 接线 + i18n + CSS
11. [x] 文档：architecture.md、maintenance.md、本文件与索引

## 验证状态

已执行并通过（本机 macOS ARM64）：

- `cargo fmt --all -- --check`
- `cargo check --workspace --all-targets`
- `cargo test --workspace`（535 项通过，1 项既有 ignore）
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- 定向：`cargo test -p kt-update proxy`（13 项）、`cargo test -p kt-config ui_font`、`cargo test -p kt-ui style_contract`
- 缩放机制核对（scratch 中临时夹具 + 无头 Chrome，不入库）：85%/100%/140% 下 `.settings-row p` 为 10.2/12/16.8px、`.settings-row strong` 11.05/13/18.2px，`.terminal-surface` 恒为 15px；所探针的容器无裁切溢出；手机夹具在 85% 下输入框仍为 16px（iOS 自动放大保护未被破坏）。

尚未验收（需人工在原生界面/真实代理下执行）：

- 桌面设置→通用切换界面字体与 85%/100%/140% 缩放后，逐屏核对主界面、设置面板、连接/安全弹窗、SFTP、运维、内置编辑器、更新页与更新弹层无溢出或换行异常，并确认重启后保留。
- 手机/窄窗口下界面缩放不破坏 44px 触控行与 16px 输入保护。
- 用真实代理（如 mitmproxy 或 `ssh -D`）验证 HTTP/HTTPS/SOCKS5 三模式下检查更新与下载更新包均走代理；错误代理与断网时显示超时/代理提示并可重试；检查或下载进行中改代理会被取消并在 5 秒内重新检查。
