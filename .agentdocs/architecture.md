# KitonyTerms 整体架构

修改任意模块前必读。本文沉淀 crate 职责、UI⇄core 消息协议、SSH/SFTP 层与 GUI 结构。
进行功能更新前还必须阅读 [maintenance.md](maintenance.md)，先填写影响清单并选择对应轻量回归套件。

## crate 划分与依赖方向

```
kt-app (Dioxus Desktop 入口) ──▶ kt-ui ──▶ kt-core ──▶ kt-config
                                 └──────▶ kt-config    kt-secrets(被 kt-ui Store 用于 vault)
                                 └──────▶ kt-secrets
kt-core ──▶ kt-config        (kt-core 无 UI 依赖,可 headless 跑/测)
```

- **kt-config**:UI 无关、可序列化。`ConnectParams`(host/port/user/auth/vault_id/proxy_jump/proxy/forward_agent)、`AuthMethod`(Password/PublicKey/KeyboardInteractive/Agent)、`ProxyConfig`(Direct/System/Socks5/Http，TCP 层代理，独立于 SSH 跳板 `proxy_jump`)、`KnownHosts`、`SessionProfile`、`AppSettings`(含 language/font/theme/scrollback/cursor/use_ssh_config/trigger_highlights/default_editor/editors/show_line_numbers/show_timestamps)、`EditorEntry`(打开方式命令模板)、`Config`(TOML)、`Paths`(跨平台目录:`config.toml`、`secrets.vault`、`known_hosts.toml`)、`~/.ssh/config` 合并。`effective_vault_id()` = `user@host:port`。
- **kt-secrets**:主密码加密 vault。Argon2id 派生密钥(每库随机盐)+ ChaCha20Poly1305。`Vault::create/open/set/get/remove/save`。UI Store 不暴露主密码流程，而是使用应用托管固定保护因子自动打开/创建本机 vault。
- **kt-core**:SSH 连接、SFTP、终端引擎,见下。
- **kt-ui**:Dioxus 组件库,持有主界面、终端、SFTP、监控、连接弹窗与 Store 桥接。
- **kt-app**:Dioxus Desktop 启动入口,二进制 `kitonyterms`,见下。当前入口能力为 GUI-only:无参数或 `--gui` 启动 GUI,`--help` 输出用法;`--safe`、`--system-ssh`、`--show-log`、`--list` 等历史稳定终端/降级入口不在当前代码中提供。

## kt-core:UI⇄core 消息协议(核心)

文件:[crates/kt-core/src/session.rs](../crates/kt-core/src/session.rs)

`SessionManager` 持有一个多线程 tokio 运行时,每个会话一个 task。调用方(GUI / headless 示例)**只**通过两条 channel 通信:

- `ToCore`(UI→core):`Connect{id,params,pty}`、`Input{id,data}`、`Resize{id,cols,rows}`、`Scroll{id,delta}`、`Sftp{id,req}`、`StartMonitor{id}`、`AuthResponse{id,response}`、`Disconnect{id}`。
- `FromCore`(core→UI):`Connected`、`Render{snapshot}`、`Title`、`Cwd{path}`、`Bell`、`SftpListing{path,entries}`、`SftpProgress{name,transferred,total}`、`SftpDone{op,path}`、`SftpError{message}`、`SftpStopped`、`Monitor{stats}`、`MonitorStopped`、`MonitorError{message}`、`AuthChallenge{id,challenge}`、`HostKeyPending{id}`、`Closed{error}`。其中 `Cwd` 由 `session.rs` 扫描 PTY 原始字节解析 OSC 7(`ESC]7;file://host/path`)得到,写入 `SessionState.terminal_cwd`,供 SFTP「跟随终端目录」使用。

要点:
- `SessionManager::spawn(verifier, auth_factory)` 启动 `core_loop`,后者按 `id` 把命令路由到各 `SessionTask`。
- `SessionTask::run` 是一个 `select!` 循环:一边收 `SessionCmd`(由 `ToCore` 转来),一边 `shell.next_message()` 取远端输出喂给 `TermEngine`,变化时发 `Render`。
- `ToCore` 与 `FromCore` 边界通道为有界队列(当前容量各 2048)。GUI 侧 `SessionManager::send` 使用 `try_send`,饱和时返回 `false` 并记录日志;headless stdin 线程使用 `blocking_send`,避免交互输入被轻易丢弃。
- `SessionManager::try_recv` 会在 UI 接收侧合并 `Render` 事件:普通事件 FIFO 保留,同一 session 的多帧 `Render` 只保留最新 `GridSnapshot`。UI 应通过 `try_recv` 泵事件,不要绕过 manager 直接消费 core 输出通道。
- core→UI 普通事件使用有界通道的 async `send().await` 形成背压;`Render` 使用 `try_send`,队列满时允许丢弃当前帧,因为下一帧会覆盖显示状态。
- **扩展能力的标准做法**:加 `ToCore`/`FromCore` 变体 + `SessionCmd` 变体 + `core_loop` 路由 + `SessionTask` 处理。新增 `FromCore` 变体后,记得给 UI 的 `pump_core_events`(穷举匹配)和 headless 示例(有 `Some(_)=>{}` 兜底)补齐。
- **辅助能力闭环原则**:SFTP、Monitor 等辅助能力必须在成功、失败、超时或会话关闭时收敛;core 路由失败和子通道打开失败要返回对应 `*Error` 事件,子任务正常停止返回 `*Stopped` 事件,UI state 保存 `loading/error/data`。
- **SSH 建连闭环原则**:初始连接不能只给 TCP/握手设超时,完整 `connect→auth→request_pty→request_shell` 链路必须有总超时;失败或超时必须返回 `Closed{error}`,不得让 UI 长期停留在连接中。
- `AuthProvider`(密码/口令/keyboard-interactive)由工厂按会话创建;session 层会用 `InteractiveAuthProvider` 包装 GUI provider。GUI provider 先读 vault 中已有密码或 `key:{key_path}` 私钥口令;缺失时 core 发 `AuthChallenge` 给 UI,UI 弹窗采集后用 `AuthResponse` 回传。认证等待期间 `SessionState.auth_challenge` 非空,状态栏显示“等待认证”。认证挑战通过独立响应通道回到认证流程,不要把认证答案混入终端 `Input`。

## kt-core:SSH 层

文件:[crates/kt-core/src/ssh/mod.rs](../crates/kt-core/src/ssh/mod.rs)、`ssh/handler.rs`

- `SshShell`(持有 `russh::client::Handle` 与 PTY shell `Channel`):`open()`(connect→auth→request_pty→request_shell)、`write/resize/next_message/disconnect`。
- 认证:按 `params.auth` 顺序尝试 password / publickey / keyboard-interactive / agent。ssh-agent 不可用、公钥文件不可用或 key 认证失败时应继续后续认证方式,避免 `~/.ssh/config` 中的默认 `IdentityFile` 或 agent 环境破坏密码 fallback。`AuthProvider::password` 必须按实际 `user@host:port` 请求密码,以支持 ProxyJump 和非 22 端口。GUI 认证缺口统一走 `AuthChallenge`/`AuthResponse`:password 返回单个隐藏输入,加密私钥返回私钥口令输入,keyboard-interactive 按服务端 prompts 逐项采集。
- 主机密钥:GUI 使用持久化 `KnownHostsVerifier`。未知主机或已知主机指纹变化时,verifier 记录 `PendingHostKey` 并拒绝本次握手;core 将 russh 的 `UnknownKey`/`KeyChanged` 映射为主机密钥待确认,先发 `HostKeyPending{id}` 再用 `Closed{error}` 收敛任务。UI 收到 `HostKeyPending` 后设置 `SessionState.host_key_pending`,不得把随后的 host-key 拒绝当普通连接失败展示。UI 弹窗展示主机、已保存指纹与本次指纹,用户可选择“仅允许一次”(内存态,下次连接消费后失效,不写入 `known_hosts.toml`)或“信任此主机”(持久写入/更新 `known_hosts.toml`)。由于 russh 主机密钥 verifier 为同步回调,本次握手会结束;UI 在用户确认后通过 `SessionState.connect_params` 和 `SessionState.pty` 自动重新发起同一会话连接。测试和显式 opt-in 才使用 `AcceptAllVerifier`。`Trusted` 与 `NewlyTrusted` 都要保存 `known_hosts.toml`,确保 `last_seen_unix` 可追踪。
- ProxyJump: `ConnectParams.proxy_jump` 支持单跳 `[user@]host[:port]`;core 先认证跳板,再通过 `channel_open_direct_tcpip` 建立目标 SSH 握手,并保留跳板 handle 直到目标连接结束。
- TCP 层代理: `ConnectParams.proxy`(`ssh/proxy.rs`)在 SSH 握手前建立经代理的 TCP 流,再交给 `client::connect_stream`。`System` 解析环境变量代理 URL(ALL_PROXY/HTTPS_PROXY/HTTP_PROXY 等);`Socks5` 走 `tokio-socks`;`Http` 手写 CONNECT 请求/响应解析。与 ProxyJump 组合时代理作用于最外层(连接跳板机那段),目标段仍走 direct-tcpip。代理凭证不入 vault。
- ssh-agent: `AuthMethod::Agent` 会读取本机 ssh-agent/Pageant identities 逐个尝试公钥认证;`ConnectParams.forward_agent` 会在 shell channel 上请求 agent forwarding。
- `open_sftp(&self) -> SftpSession`:在**同一 handle** 上 `channel_open_session` → `request_subsystem(true,"sftp")` → `russh_sftp::client::SftpSession::new(channel.into_stream())`。返回独立拥有通道流的会话,可 move 进子任务;底层 TCP 由 `SshShell` 的 handle 维持。

## kt-core:SFTP 子任务

文件:[crates/kt-core/src/sftp.rs](../crates/kt-core/src/sftp.rs)

- `SessionTask` 首次收到 `SessionCmd::Sftp` 时**惰性** `open_sftp`,把 `SftpSession` move 进 `tokio::spawn(sftp_task(...))`,并保存其命令 sender;后续请求转发给该子任务。
- SFTP 打开采用两段式:先复用当前 SSH 会话开 `sftp` 子系统(8 秒超时),失败后自动新建独立 SSH 连接承载 SFTP(20 秒超时),并把两段失败原因合并为 `SftpError` 返回 UI。
- `sftp_task` 拥有独立 mpsc 与 `FromCore` 发送端,**串行**处理请求,故大文件传输不阻塞 shell `select` 循环。
- 请求类型 `SftpRequest`:`List`(先 `canonicalize` 成绝对路径再 `read_dir`,目录优先 + 名称不分大小写排序;快速操作 12 秒超时)、`Download`/`Upload`(用 `File` 的 tokio `AsyncRead`/`AsyncWrite` 分块拷贝,按 `PROGRESS_STEP` 节流上报进度)、`Mkdir`/`Remove`(按 `is_dir` 选 `remove_dir`/`remove_file`)/`Rename`。
- `SftpEntry`(name/is_dir/size/modified/permissions/user/group/uid/gid)是 core 内中立类型,**不向 UI 暴露** russh-sftp 类型。
- 依赖:`russh-sftp`(传输无关,基于流);`tokio` 启用 `fs` 特性用于本地异步文件。

## kt-core:终端引擎

文件:`crates/kt-core/src/term/`(`mod.rs`/`color.rs`/`snapshot.rs`)

- `TermEngine` 包装 `alacritty_terminal`,产出 `GridSnapshot`(行列单元格 + 光标 + 颜色),`advance(bytes)` 喂入输出,`resize/scroll`,`take_events()` 取 Bell/Title 等。
- `GridSnapshot` 中的单元格颜色是 core 层解析后的最终显示色:反色、DIM 等属性在快照生成时完成颜色计算,UI 不应再次反转前景/背景。终端字符必须以普通文本节点渲染,不得使用 HTML 注入式渲染,避免 `<`、`&` 等字符破坏 DOM。终端 cell 的 inline style 必须显式写出可跨帧变化属性的默认值(如 `background: transparent`、`text-decoration: none`、`opacity: 1`),避免 WebView/Dioxus 样式 diff 后残留备用屏程序的色块。

## kt-ui / kt-app:GUI

文件:[crates/kt-app/src/main.rs](../crates/kt-app/src/main.rs)、[crates/kt-ui/src/components/app.rs](../crates/kt-ui/src/components/app.rs)

- `kt-app` 负责解析最小入口参数、初始化日志、创建 Dioxus Desktop 窗口并 `launch(App)`；业务界面在 `kt-ui`。当前支持无参数或 `--gui` 启动 GUI、`--help` 查看用法；旧 `--safe`、`--system-ssh`、`--show-log`、`--list` 会明确报错，避免文档中曾存在但代码不存在的能力被误用。
- `App` 通过全局 `Store` 与 `AppState` 懒初始化 `SessionManager`。UI 每 16ms 泵送 `FromCore`，每 100ms 从 `AppState.sessions` 同步会话列表。
- **主界面结构**:系统原生标题栏 + 左侧边栏(分组连接树、SFTP 表格、设置入口) + 中央终端工作区 + 底部系统监控横条 + 状态栏。样式集中在 [app.css](../crates/kt-ui/src/assets/app.css)。[app.rs](../crates/kt-ui/src/components/app.rs) 是主编排组件,保留全局信号、上下文菜单、弹窗和跨模块动作;[state_controller.rs](../crates/kt-ui/src/components/state_controller.rs) 负责事件泵、会话列表同步、主机密钥提示同步与外部编辑副作用;[main_shell.rs](../crates/kt-ui/src/components/main_shell.rs) 负责主工作台外层调度,其子模块 `main_shell/sidebar_panel.rs`、`main_shell/workbench_panel.rs`、`main_shell/status_bar.rs` 分别承接连接/SFTP 侧边栏、终端与监控工作区、底部状态栏;安全认证对话框、外部编辑状态机、侧边栏/SFTP 右键菜单、连接/分组/命名对话框已拆到独立模块;[app_logic.rs](../crates/kt-ui/src/components/app_logic.rs) 保存分组归并、会话状态初始化、SSH config 合并、连接状态 selector 等纯逻辑;[app_runtime.rs](../crates/kt-ui/src/components/app_runtime.rs) 保存 Store-backed AuthProvider 与 KnownHostsVerifier。后续深拆目标是更细粒度 selector 与 `state_controller` 集成断言。
- **selector 边界**:`app_logic.rs` 中的 `SessionTabView / ActiveSftpView / ActiveMonitorView / StatusBarSessionView / ActiveTerminalView` 是主工作台的轻量视图模型。SFTP、Monitor、状态栏和会话标签不应直接依赖完整 `SessionState`;终端区域可以通过 `ActiveTerminalView` 持有 `GridSnapshot`,但不要为了比较或 memo 强行给大快照引入伪等价语义。`state_controller::resolve_active_session_id` 统一处理 active session 缺失、过期和空列表,会话列表同步时按 `SessionId` 排序以保持 UI 顺序稳定。
- **UI 抽离约定**:接收 `Arc<Mutex<AppState>>`、`Arc<Store>`、大量 `Signal` 或闭包的重状态入口优先使用普通函数返回 `Element`,不要默认写成 Dioxus `#[component]`;只有 props 天然适合 `PartialEq`、边界清晰且可复用的展示单元才使用组件。这样避免为了通过 props 派生而给运行时对象引入伪等价语义。
- **终端渲染**:[terminal.rs](../crates/kt-ui/src/components/terminal.rs) 使用 `GridSnapshot` 渲染 HTML 行列，并把键盘、滚轮输入转成 `ToCore::Input`/`Scroll`。
- **会话标题边界**:`SessionState.title` 是用户保存的服务器/会话名称,用于标签、侧边栏高亮与状态栏;远端 OSC title/ResetTitle 事件不得覆盖它。若后续需要展示远端窗口标题,应新增独立字段。
- **分屏与触发器高亮**:终端工具栏可切换水平/垂直双视图,当前为同一 session 的本地双视图;`AppSettings.trigger_highlights` 提供行级文本触发器,由 [terminal.rs](../crates/kt-ui/src/components/terminal.rs) 做大小写不敏感匹配并加高亮 class。
- **SFTP 面板**:[sftp.rs](../crates/kt-ui/src/components/sftp.rs) 发送 `ToCore::Sftp(List)`，并从全局 `SessionState` 同步 `sftp_path/sftp_entries/sftp_loading/sftp_error/sftp_progress`。连接成功后的自动加载由 `AppState` 触发；`SftpStopped` 清理 loading/progress 但不覆盖已有错误。同步全局状态到本地 signal 前必须比较差异，避免 effect 订阅与定时同步造成重复请求或重连循环。侧边栏 SFTP 树、条目格式化和右键菜单在 [sidebar.rs](../crates/kt-ui/src/components/sidebar.rs)。外部编辑器状态机、临时文件命名、打开本地编辑器与状态栏文案在 [external_edit.rs](../crates/kt-ui/src/components/external_edit.rs);App 只负责触发下载/上传和弹出保存确认。
- **资源监控**:[state.rs](../crates/kt-ui/src/state.rs) 收到 `Connected` 后自动发送 `StartMonitor` 并进入 `monitor_loading`;core 成功采样返回 `Monitor`,失败/超时返回 `MonitorError`;正常通道关闭返回 `MonitorStopped` 清理等待态,不展示为错误。监控子任务退出后会通知会话重置启动状态,允许后续重新 `StartMonitor`。延迟采样优先 TCP connect 当前会话 SSH `host:port`,失败时回退到已连接 SSH monitor 通道心跳,不得阻塞资源采样。[monitor.rs](../crates/kt-ui/src/components/monitor.rs) 只展示 `monitor_loading`、`monitor_error` 与 `monitor` 三态。
- **连接失败展示**:`FromCore::Closed{error}` 必须写入 `SessionState.connection_error`;终端占位、状态栏和会话状态点都要把错误会话显示为失败/断开,不得继续使用 connecting 文案或黄色连接中状态。
- **持久化**:[store.rs](../crates/kt-ui/src/store.rs) 桥接 `kt-config`(会话明文)与 `kt-secrets`(机密)。Store 启动时自动打开或创建应用托管 vault,保存连接后按 `effective_vault_id()` 写入密码;Store-backed `AuthProvider` 重连时直接读取 `user@host:port` 或配置的 `vault_id`。旧主密码 vault 无法自动打开时会备份为 `secrets.vault.legacy` 并创建新的托管 vault,状态栏提示旧保存密码暂不可用;若初始化/备份失败则保持 `VaultState::Locked` 并让读写返回明确错误。secret 值不得写入 `config.toml` 或日志。
