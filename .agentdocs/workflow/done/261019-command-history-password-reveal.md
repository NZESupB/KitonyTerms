# 命令历史与系统认证查看密码

## 范围与约束

本轮只实施原需求第 4、5 项，保留更新、字体和更新代理的现有修改。命令历史放在桌面终端下方，支持调整高度、隐藏、清空和逐条复制，最近 200 条仅保存在各会话内存中。保存密码在 macOS/Windows 系统认证成功后才从 vault 取出，不改变连接时自动认证及 vault 格式。

## 方案与边界

- `kt-core/src/command_history.rs` 在 PTY 收发顺序中观察专用提示符标记，每次提示符只采集一条提交；程序运行期间、备用屏、前导空格、补全/方向键等不确定编辑及超过 4096 字节的输入跳过。UTF-8 退格保留整码点语义。
- Bash/Zsh hook 在用户已有 hook 之后发出提示符标记；bootstrap 初始化只上报目录，不开放采集。shell 集成在连接后独立启用，目录自动同步开关仍只控制目录跟随。其他 shell、嵌套的未集成 shell及容器独立 PTY 不采集；多行粘贴最多收录首条简单提交。
- `FromCore::CommandSubmitted` 仅经普通事件通道传递，Debug 不带命令内容；UI 按已连接会话存储有界历史，不写入配置/同步，关闭会话移除。面板提供标题栏/终端菜单入口、拖动及方向键调整高度、逐条原生剪贴板复制和结果反馈，不自动执行命令。
- `os_auth.rs` 在后台线程执行系统认证，每次新建 macOS LAContext 并使用设备所有者策略；Windows 使用 CredUI、LogonUserW 与当前进程账户 SID 比较，支持拆分 DOMAIN\\user 与 UPN。取消、错误、其他账户及不支持的平台均拒绝查看；Windows 使用当前账户的登录密码，不声明支持 Hello PIN。
- Windows 临时凭据与打包缓冲由 Zeroizing/RAII 清零释放，token 句柄自动关闭；不保存系统账户密码，不记录原始认证输入。
- `Verified` 只能由认证模块构造、不可复制，`Store::reveal_secret` 消费一次该凭据。`Store::has_secret` 只检查存在性，不克隆明文。保留连接参数显式 vault_id。
- 密码查看值与编辑值分开：只读展示不参与保存；结果绑定连接名称、host/user/port、vault ID 和请求代次。关闭、切换目标/页签、编辑输入或 30 秒后清除查看值。组件卸载会取消 UI 后续取密流程，系统认证工作线程仍可能等待系统窗口返回。

## 阶段

- [x] 核对项目约束和实现入口。
- [x] 实现提示符门控采集、事件和会话内历史。
- [x] 实现历史面板、复制、调整高度及双语文案。
- [x] 实现系统认证及弹窗安全状态。
- [x] 完成 Rust 门禁、平台 API 类型检查和 SSR 浏览器检查。
- [x] 记录原生系统/真实 SSH 验收边界并同步长期文档。

## 已执行验证

- `cargo fmt --all -- --check`、`cargo check --workspace --all-targets` 通过。
- `cargo test --workspace -- --test-threads=1`：553 通过、1 项既有忽略。随后对 Clippy 提示作等价简化并复测 kt-core（含回环集成）和 kt-ui，全部通过。
- 默认及 all-features 的 workspace/all-targets Clippy，均以 `-D warnings` 通过。初次 Clippy 的 collapsible_match 已修正；保留失败和修正日志，不依赖忽略规则。
- Windows scratch 工程以 x86_64-pc-windows-msvc 编译实际 os_auth.rs/windows.rs，并验证 Tao 窗口 HWND 接口，通过。它不经过 ring，不表示完整 Windows 应用构建或运行通过。
- 生产 CommandHistoryPanel/ConnectionDialog 的 16 份 SSR 页面，覆盖 3 个视口宽度（360/900/1440）共 48 组组合：517/517 浏览器断言通过，包含主题、双语、溢出、命令文本转义、焦点及认证前不输出保存密码。未模拟系统认证成功。
- CSS 通过 PostCSS 与 css-tree 解析，13 条新增规则无重复属性；深色历史与浅色连接截图已回看。
- `git diff --check`、`git diff --cached --check` 通过。

验证产物位于本会话 scratch：`history-auth-final-verification.json`、`history-auth-browser-results.json`、`ui-fixtures/site/`。浏览器 SSR 夹具只证明 DOM/CSS/焦点契约，不代表生产 Dioxus 事件、系统弹窗、原生剪贴板或真实 SSH 端到端已验证。

## 尚未执行的原生验收

1. Windows：用隔离数据目录保存一条虚构 SSH 密码；点“验证身份并查看”，确认取消、错误密码和其他系统账户均不揭示。输入当前账户正确登录密码后才显示只读值；检查 DOMAIN\\user、Microsoft/域账户的实际凭据提供程序行为。
2. macOS：验证 Touch ID/设备所有者密码回退、取消、系统中断，以及关闭/切换连接后迟到认证不揭示；无可用认证能力时明确失败。
3. 两端：查看值在 30 秒后消失；查看后保存不改原密码，新输入密码仍能保存；换目标、页签和关闭弹窗立即隐藏。认证期间其他 SSH 会话可继续操作。
4. 真实 Bash/Zsh SSH：关闭目录自动同步后输入 pwd/echo，历史仍更新；逐条复制到文本编辑器，调整面板高度和切换会话；sudo/read/嵌套程序的后续输入不收录，vim/top、前导空格和不确定编辑被跳过，超过 200 条淘汰最早记录。

本轮未执行原生系统认证，也未替换已安装应用。此前完整 Windows 交叉构建受本机 Windows SDK 缺失影响，本轮仅完成认证代码的目标类型检查。
