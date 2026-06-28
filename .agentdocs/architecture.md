# KitonyTerms 整体架构

修改任意模块前必读。本文沉淀 crate 职责、UI⇄core 消息协议、SSH/SFTP 层与 GUI 结构。

## crate 划分与依赖方向

```
kt-app (Dioxus Desktop 入口) ──▶ kt-ui ──▶ kt-core ──▶ kt-config
                                 └──────▶ kt-config    kt-secrets(被 kt-ui Store 用于 vault)
                                 └──────▶ kt-secrets
kt-core ──▶ kt-config        (kt-core 无 UI 依赖,可 headless 跑/测)
```

- **kt-config**:UI 无关、可序列化。`ConnectParams`(host/port/user/auth/vault_id/proxy_jump/forward_agent)、`AuthMethod`(Password/PublicKey/KeyboardInteractive/Agent)、`KnownHosts`、`SessionProfile`、`AppSettings`、`Config`(TOML)、`Paths`(跨平台目录:`config.toml`、`secrets.vault`、`known_hosts.toml`)、`~/.ssh/config` 合并。`effective_vault_id()` = `user@host:port`。
- **kt-secrets**:主密码加密 vault。Argon2id 派生密钥(每库随机盐)+ ChaCha20Poly1305。`Vault::create/open/set/get/remove/save`。空密码可正常派生(无长度校验)。
- **kt-core**:SSH 连接、SFTP、终端引擎,见下。
- **kt-ui**:Dioxus 组件库,持有主界面、终端、SFTP、监控、连接弹窗与 Store 桥接。
- **kt-app**:Dioxus Desktop 启动入口,二进制 `kitonyterms`,见下。当前入口能力为 GUI-only:无参数或 `--gui` 启动 GUI,`--help` 输出用法;`--safe`、`--system-ssh`、`--show-log`、`--list` 等历史稳定终端/降级入口不在当前代码中提供。

## kt-core:UI⇄core 消息协议(核心)

文件:[crates/kt-core/src/session.rs](../crates/kt-core/src/session.rs)

`SessionManager` 持有一个多线程 tokio 运行时,每个会话一个 task。调用方(GUI / headless 示例)**只**通过两条 channel 通信:

- `ToCore`(UI→core):`Connect{id,params,pty}`、`Input{id,data}`、`Resize{id,cols,rows}`、`Scroll{id,delta}`、`Sftp{id,req}`、`StartMonitor{id}`、`Disconnect{id}`。
- `FromCore`(core→UI):`Connected`、`Render{snapshot}`、`Title`、`Bell`、`SftpListing{path,entries}`、`SftpProgress{name,transferred,total}`、`SftpDone{op,path}`、`SftpError{message}`、`Monitor{stats}`、`MonitorStopped`、`MonitorError{message}`、`Closed{error}`。

要点:
- `SessionManager::spawn(verifier, auth_factory)` 启动 `core_loop`,后者按 `id` 把命令路由到各 `SessionTask`。
- `SessionTask::run` 是一个 `select!` 循环:一边收 `SessionCmd`(由 `ToCore` 转来),一边 `shell.next_message()` 取远端输出喂给 `TermEngine`,变化时发 `Render`。
- **扩展能力的标准做法**:加 `ToCore`/`FromCore` 变体 + `SessionCmd` 变体 + `core_loop` 路由 + `SessionTask` 处理。新增 `FromCore` 变体后,记得给 UI 的 `pump_core_events`(穷举匹配)和 headless 示例(有 `Some(_)=>{}` 兜底)补齐。
- **辅助能力闭环原则**:SFTP、Monitor 等辅助能力必须在成功、失败、超时或会话关闭时收敛;core 路由失败和子通道打开失败要返回对应 `*Error` 事件,UI state 保存 `loading/error/data`。
- **SSH 建连闭环原则**:初始连接不能只给 TCP/握手设超时,完整 `connect→auth→request_pty→request_shell` 链路必须有总超时;失败或超时必须返回 `Closed{error}`,不得让 UI 长期停留在连接中。
- `AuthProvider`(密码/口令/keyboard-interactive)由工厂按会话创建;GUI 实现读预先填好的机密,不做握手期阻塞弹窗。

## kt-core:SSH 层

文件:[crates/kt-core/src/ssh/mod.rs](../crates/kt-core/src/ssh/mod.rs)、`ssh/handler.rs`

- `SshShell`(持有 `russh::client::Handle` 与 PTY shell `Channel`):`open()`(connect→auth→request_pty→request_shell)、`write/resize/next_message/disconnect`。
- 认证:按 `params.auth` 顺序尝试 password / publickey / keyboard-interactive / agent。ssh-agent 不可用、公钥文件不可用或 key 认证失败时应继续后续认证方式,避免 `~/.ssh/config` 中的默认 `IdentityFile` 或 agent 环境破坏密码 fallback。`AuthProvider::password` 必须按实际 `user@host:port` 请求密码,以支持 ProxyJump 和非 22 端口。
- 主机密钥:GUI 使用持久化 `KnownHostsVerifier`,未知主机首次写入 `known_hosts.toml`,已知主机指纹变化时拒绝连接;测试和显式 opt-in 才使用 `AcceptAllVerifier`。
- ProxyJump: `ConnectParams.proxy_jump` 支持单跳 `[user@]host[:port]`;core 先认证跳板,再通过 `channel_open_direct_tcpip` 建立目标 SSH 握手,并保留跳板 handle 直到目标连接结束。
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

## kt-ui / kt-app:GUI

文件:[crates/kt-app/src/main.rs](../crates/kt-app/src/main.rs)、[crates/kt-ui/src/components/app.rs](../crates/kt-ui/src/components/app.rs)

- `kt-app` 负责解析最小入口参数、初始化日志、创建 Dioxus Desktop 窗口并 `launch(App)`；业务界面在 `kt-ui`。当前支持无参数或 `--gui` 启动 GUI、`--help` 查看用法；旧 `--safe`、`--system-ssh`、`--show-log`、`--list` 会明确报错，避免文档中曾存在但代码不存在的能力被误用。
- `App` 通过全局 `Store` 与 `AppState` 懒初始化 `SessionManager`。UI 每 16ms 泵送 `FromCore`，每 100ms 从 `AppState.sessions` 同步会话列表。
- **主界面结构**:系统原生标题栏 + 左侧边栏(分组连接树、SFTP 表格、设置入口) + 中央终端工作区 + 底部系统监控横条 + 状态栏。样式集中在 [app.css](../crates/kt-ui/src/assets/app.css)。
- **终端渲染**:[terminal.rs](../crates/kt-ui/src/components/terminal.rs) 使用 `GridSnapshot` 渲染 HTML 行列，并把键盘、滚轮输入转成 `ToCore::Input`/`Scroll`。
- **分屏与触发器高亮**:终端工具栏可切换水平/垂直双视图,当前为同一 session 的本地双视图;`AppSettings.trigger_highlights` 提供行级文本触发器,由 [terminal.rs](../crates/kt-ui/src/components/terminal.rs) 做大小写不敏感匹配并加高亮 class。
- **SFTP 面板**:[sftp.rs](../crates/kt-ui/src/components/sftp.rs) 发送 `ToCore::Sftp(List)`，并从全局 `SessionState` 同步 `sftp_path/sftp_entries/sftp_loading/sftp_error/sftp_progress`。连接成功后的自动加载由 `AppState` 触发；同步全局状态到本地 signal 前必须比较差异，避免 effect 订阅与定时同步造成重复请求或重连循环。外部编辑器流程在 [app.rs](../crates/kt-ui/src/components/app.rs) 中处理，必须下载完成后打开本地临时文件，监听本地保存后弹窗选择回传策略，回传进度放到底部状态栏。
- **资源监控**:[state.rs](../crates/kt-ui/src/state.rs) 收到 `Connected` 后自动发送 `StartMonitor` 并进入 `monitor_loading`;core 成功采样返回 `Monitor`,失败/超时返回 `MonitorError`;正常通道关闭返回 `MonitorStopped` 清理等待态,不展示为错误。监控子任务退出后会通知会话重置启动状态,允许后续重新 `StartMonitor`。[monitor.rs](../crates/kt-ui/src/components/monitor.rs) 只展示 `monitor_loading`、`monitor_error` 与 `monitor` 三态。
- **连接失败展示**:`FromCore::Closed{error}` 必须写入 `SessionState.connection_error`;终端占位、状态栏和会话状态点都要把错误会话显示为失败/断开,不得继续使用 connecting 文案或黄色连接中状态。
- **持久化**:[store.rs](../crates/kt-ui/src/store.rs) 桥接 `kt-config`(会话明文)与 `kt-secrets`(机密)。保存连接后按 `effective_vault_id()` 写入 vault 中的密码。
