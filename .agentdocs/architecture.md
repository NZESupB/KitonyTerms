# KitonyTerms 整体架构

修改任意模块前必读。本文沉淀 crate 职责、UI⇄core 消息协议、SSH/SFTP 层与 GUI 结构。
进行功能更新前还必须阅读 [maintenance.md](maintenance.md)，先填写影响清单并选择对应轻量回归套件。

## crate 划分与依赖方向

```
kt-app (Dioxus desktop/mobile 入口) ──▶ kt-ui ──▶ kt-core ──▶ kt-config
                                        └──────▶ kt-config    kt-secrets(被 kt-ui Store 用于 vault)
                                        └──────▶ kt-secrets
                                        └──────▶ kt-sync ───▶ kt-config
kt-core ──▶ kt-config        (kt-core 无 UI 依赖,可 headless 跑/测)
```

- **kt-config**:UI 无关、可序列化。`ConnectParams`(host/port/user/auth/vault_id/proxy_jump/proxy/forward_agent)、`AuthMethod`(Password/PublicKey/KeyboardInteractive/Agent)、`ProxyConfig`(Direct/System/Socks5/Http，TCP 层代理，独立于 SSH 跳板 `proxy_jump`)、`KnownHosts`、`SessionProfile`、`AppSettings`(含 language/font/theme/scrollback/cursor/use_ssh_config/trigger_highlights/default_editor/editors/show_line_numbers/show_timestamps/sftp_auto_sync)、`EditorEntry`(打开方式命令模板)、`Config`(TOML)、`Paths`(跨平台目录:`config.toml`、`secrets.vault`、`known_hosts.toml`、`kitonyterms.lock`)、`~/.ssh/config` 合并。Android 的 `Paths` 通过 JNI `Context.getFilesDir()` 使用应用私有 `files/config` 与 `files/data`，其他平台继续使用 `ProjectDirs`。`effective_vault_id()` = `user@host:port`。Config 与 KnownHosts 保存均使用同目录唯一临时文件原子替换，禁止固定临时文件名和原地截断。
- **kt-secrets**:主密码加密 vault。Argon2id 派生密钥(每库随机盐)+ ChaCha20Poly1305。`Vault::create/open/set/get/remove/save`。UI Store 不暴露主密码流程，而是使用应用托管固定保护因子自动打开/创建本机 vault。
- **kt-core**:SSH 连接、SFTP、终端引擎,见下。
- **kt-ui**:Dioxus 组件库,持有主界面、终端、SFTP、监控、连接弹窗与 Store 桥接。
- **kt-sync**:只同步非机密 `kt_config::Config` 的独立传输层。提供 WebDAV GET/PUT（完整 URL、禁重定向、ETag 条件写、1 MiB 上限）与局域网一次性分享；LAN 协议不发送配对码，使用 HMAC 认证带随机 nonce 的 GET/ACK 请求，以配对码派生的 ChaCha20Poly1305 密钥加密配置，客户端解密并落盘后显式 ACK 才消费分享。服务枚举非 loopback IPv4/IPv6 接口、限制并发连接数和单连接处理时间；不接触 vault、vault key、known_hosts、锁文件或运行时状态。
- **kt-app**:Dioxus desktop/mobile 启动入口,二进制 `kitonyterms`,见下。当前入口能力为 GUI-only:无参数或 `--gui` 启动 GUI,`--help` 输出用法;`--safe`、`--system-ssh`、`--show-log`、`--list` 等历史稳定终端/降级入口不在当前代码中提供。

## kt-core:UI⇄core 消息协议(核心)

文件:[crates/kt-core/src/session.rs](../crates/kt-core/src/session.rs)

`SessionManager` 持有一个多线程 tokio 运行时,每个会话一个 task。调用方(GUI / headless 示例)**只**通过两条 channel 通信:

- `ToCore`(UI→core):`Connect{id,params,pty}`、`Input{id,data}`、`Resize{id,cols,rows}`、`Scroll{id,delta}`、`Sftp{id,request_id,req}`、`StartMonitor{id}`、`SetupShellIntegration{id}`、`AuthResponse{id,response}`、`Disconnect{id}`。
- `FromCore`(core→UI):`Connected`、`Render{snapshot}`、`Title`、`Cwd{path}`、`Bell`、`SftpListing{request_id,path,entries}`、`SftpProgress{request_id,name,transferred,total}`、`SftpDone{request_id,op,path}`、`SftpError{request_id,message}`、`SftpStopped`、`Monitor{stats}`、`MonitorStopped`、`MonitorError{message}`、`AuthChallenge{id,challenge}`、`HostKeyPending{id}`、`Closed{error}`。SFTP 请求级事件必须回传 UI 分配的 `SftpRequestId`；`SftpStopped` 与 `Closed` 保持会话级语义。`Cwd` 由 `session.rs` 扫描 PTY 原始字节解析 OSC 7(`ESC]7;file://host/path`)得到,写入 `SessionState.terminal_cwd`,供 SFTP「跟随终端目录」使用。

要点:
- `SessionManager::spawn(verifier, auth_factory)` 启动 `core_loop`,后者按 `id` 把命令路由到各 `SessionTask`。每次 `Connect` 都分配内部递增代次；任务结束仅在其代次仍为当前句柄时回收，避免旧任务迟到结束误删重连后的句柄。
- `SessionTask::run` 是一个 `select!` 循环:一边收 `SessionCmd`(由 `ToCore` 转来),一边 `shell.next_message()` 取远端输出喂给 `TermEngine`,变化时发 `Render`。
- `ToCore` 与 `FromCore` 边界通道为有界队列(当前容量各 2048)。GUI 侧 `SessionManager::send` 使用 `try_send`,饱和时返回 `false` 并记录日志;headless stdin 线程使用 `blocking_send`,避免交互输入被轻易丢弃。
- `SessionManager::try_recv` 会在 UI 接收侧合并 `Render` 事件:普通事件 FIFO 保留,同一 session 的多帧 `Render` 只保留最新 `GridSnapshot`。UI 应通过 `try_recv` 泵事件,不要绕过 manager 直接消费 core 输出通道。
- core→UI 普通事件使用有界通道的 async `send().await` 形成背压;`Render` 使用 `try_send`,队列满时允许丢弃当前帧,因为下一帧会覆盖显示状态。
- **扩展能力的标准做法**:加 `ToCore`/`FromCore` 变体 + `SessionCmd` 变体 + `core_loop` 路由 + `SessionTask` 处理。新增 `FromCore` 变体后,记得给 UI 的 `pump_core_events`(穷举匹配)和 headless 示例(有 `Some(_)=>{}` 兜底)补齐。
- **辅助能力闭环原则**:SFTP、Monitor 等辅助能力必须在成功、失败、超时或会话关闭时收敛;core 路由失败和子通道打开失败要返回对应 `*Error` 事件,子任务正常停止返回 `*Stopped` 事件,UI state 保存 `loading/error/data`。唯一例外是 `SetupShellIntegration`:它没有 loading/pending 状态,失效时 UI 侧的输入推断兜底仍然工作,因此路由失败只记日志,不新增 `FromCore` 错误事件,以免把尽力而为的增强写成用户可见故障。
- **SSH 建连闭环原则**:初始连接不能只给 TCP/握手设超时,完整 `connect→auth→request_pty→request_shell` 链路必须有总超时;失败或超时必须返回 `Closed{error}`,不得让 UI 长期停留在连接中。
- `AuthProvider`(密码/口令/keyboard-interactive)由工厂按会话创建;session 层会用 `InteractiveAuthProvider` 包装 GUI provider。GUI provider 先读 vault 中已有密码或 `key:{key_path}` 私钥口令;缺失时 core 发 `AuthChallenge` 给 UI,UI 弹窗采集后用 `AuthResponse` 回传。认证等待期间 `SessionState.auth_challenge` 非空,状态栏显示“等待认证”。同步等待认证答案必须放入 Tokio `block_in_place`，避免多个认证弹窗耗尽 runtime worker；认证答案仍通过独立响应通道回到认证流程,不要混入终端 `Input`。

## kt-core:SSH 层

文件:[crates/kt-core/src/ssh/mod.rs](../crates/kt-core/src/ssh/mod.rs)、`ssh/handler.rs`

- `SshShell`(持有 `russh::client::Handle` 与 PTY shell `Channel`):`open()`(connect→auth→request_pty→request_shell)、`write/resize/next_message/disconnect`。
- 认证:按 `params.auth` 顺序尝试 password / publickey / keyboard-interactive / agent。ssh-agent 不可用、公钥文件不可用或 key 认证失败时应继续后续认证方式,避免 `~/.ssh/config` 中的默认 `IdentityFile` 或 agent 环境破坏密码 fallback。`AuthProvider::password` 必须按实际 `user@host:port` 请求密码,以支持 ProxyJump 和非 22 端口。GUI 认证缺口统一走 `AuthChallenge`/`AuthResponse`:password 返回单个隐藏输入,加密私钥返回私钥口令输入,keyboard-interactive 按服务端 prompts 逐项采集。
- 主机密钥:GUI 使用持久化 `KnownHostsVerifier`。未知主机或已知主机指纹变化时,verifier 把 `PendingHostKey` 放入按 host/port/fingerprint 去重的队列并拒绝本次握手;core 将 russh 的 `UnknownKey`/`KeyChanged` 映射为主机密钥待确认,先发 `HostKeyPending{id}` 再用 `Closed{error}` 收敛任务。UI 收到 `HostKeyPending` 后设置 `SessionState.host_key_pending`,不得把随后的 host-key 拒绝当普通连接失败展示。信任、仅允许一次或取消只移除精确匹配的队列项，并只重连/清理目标 host/port（含匹配 ProxyJump）的会话；其他未知主机提示继续排队。Store 在单个 `Mutex<KnownHosts>` 内串行校验与保存：新信任必须落盘成功后才能接受，可信 key 的 `last_seen` 保存失败仍允许连接并通过状态栏告警。测试和显式 opt-in 才使用 `AcceptAllVerifier`。
- ProxyJump: `ConnectParams.proxy_jump` 支持单跳 `[user@]host[:port]`;core 先认证跳板,再通过 `channel_open_direct_tcpip` 建立目标 SSH 握手,并保留跳板 handle 直到目标连接结束。
- TCP 层代理: `ConnectParams.proxy`(`ssh/proxy.rs`)在 SSH 握手前建立经代理的 TCP 流,再交给 `client::connect_stream`。`System` 解析环境变量代理 URL(ALL_PROXY/HTTPS_PROXY/HTTP_PROXY 等);`Socks5` 走 `tokio-socks`;`Http` 仅支持 `http://` 明文 CONNECT，`https://` proxy 与未知 scheme 会明确报错，不能静默降级。HTTP CONNECT 的 IPv6 authority 必须使用 `[host]:port`。与 ProxyJump 组合时代理作用于最外层(连接跳板机那段),目标段仍走 direct-tcpip。代理凭证不入 vault。
- ssh-agent: `AuthMethod::Agent` 会读取本机 ssh-agent/Pageant identities 逐个尝试公钥认证;`ConnectParams.forward_agent` 会在 shell channel 上请求 agent forwarding。
- `open_sftp(&self) -> SftpSession`:在**同一 handle** 上 `channel_open_session` → `request_subsystem(true,"sftp")` → `russh_sftp::client::SftpSession::new(channel.into_stream())`。返回独立拥有通道流的会话,可 move 进子任务;底层 TCP 由 `SshShell` 的 handle 维持。

## kt-core:SFTP 子任务

文件:[crates/kt-core/src/sftp.rs](../crates/kt-core/src/sftp.rs)

- `SessionTask` 首次收到 `SessionCmd::Sftp` 时**惰性** `open_sftp`,把 `SftpSession` move 进 `tokio::spawn(sftp_task(...))`,并保存其命令 sender;后续请求转发给该子任务。
- SFTP 打开采用两段式:先复用当前 SSH 会话开 `sftp` 子系统(8 秒超时),失败后自动新建独立 SSH 连接承载 SFTP(20 秒超时),并把两段失败原因合并为 `SftpError` 返回 UI。
- `sftp_task` 拥有独立 mpsc 与 `FromCore` 发送端,**串行**处理请求,故大文件传输不阻塞 shell `select` 循环。
- 请求类型 `SftpRequest`:`List`(先 `canonicalize` 成绝对路径再 `read_dir`,目录优先 + 名称不分大小写排序;快速操作 12 秒超时)、`Download`/`Upload`(用 `File` 的 tokio `AsyncRead`/`AsyncWrite` 分块拷贝,按 `PROGRESS_STEP` 节流上报进度)、`Mkdir`/`Remove`(按 `is_dir` 选 `remove_dir`/`remove_file`)/`Rename`。下载先写目标同目录私有唯一临时文件，`flush + sync_all` 后原子替换。
- 上传先 `metadata` 探测目标（目录直接拒绝，普通文件记下权限），写远端同目录排他唯一临时文件，`shutdown` 后按原权限 `setstat`，再由 `commit_uploaded_temp` 提交：先直接 rename；目标已存在而 rename 被拒时，改名原文件为同目录备份 → 提交临时文件 → 删除备份，提交失败立刻把备份改回原名。OpenSSH 的 `SSH_FXP_RENAME` 对已存在目标必然失败（内部 `link()` 得到 `EEXIST`），单次 rename 会让所有覆盖上传失效；`russh-sftp` 2.3 未暴露 `posix-rename@openssh.com`（`SftpSession` 不公开内部 `RawSftpSession`），故用备份轮换换取兼容性，代价是提交期间有极短的目标缺失窗口。提交策略通过私有 trait `RemoteCommitOps` 抽象，单测用假远端分别模拟“拒绝覆盖”“支持覆盖”“提交失败需回滚”三类服务器行为。任何失败路径都保留原文件并清理临时文件，禁止“先删旧文件”的非原子降级。
- `SftpEntry`(name/is_dir/size/modified/permissions/user/group/uid/gid)是 core 内中立类型,**不向 UI 暴露** russh-sftp 类型。
- 依赖:`russh-sftp`(传输无关,基于流);`tokio` 启用 `fs` 特性用于本地异步文件。

## kt-core:shell 集成(终端目录上报)

文件:[crates/kt-core/src/shell_integration.rs](../crates/kt-core/src/shell_integration.rs)

SSH 协议既读不到也改不了一个已在运行的交互 shell 的工作目录。要让「终端里切目录 → 文件管理跟随」实时可靠,唯一可行的做法是让远端 shell 自己在每次 prompt 前发 OSC 7,而 Debian/Ubuntu/RHEL 的默认 bash 与 Linux 上的 zsh 都不发。

- `BOOTSTRAP_COMMAND` 是每个连接注入一次的常量命令:定义 `__kt_cwd` 用 `printf '%s'` **传参**上报 `$PWD`(路径里的 `%` 不会被当成格式说明符),bash 侧按标量/数组两种形态前置追加到 `PROMPT_COMMAND`,zsh 侧追加 `precmd_functions`,并给 `HISTCONTROL`/`hist_ignore_space` 追加 ignorespace；命令结束时必须发送 `BOOTSTRAP_DONE_MARKER`，供输出过滤器确定边界。**一律追加,不得覆盖用户配置**;zsh 专用的 `+=(...)`、`setopt` 必须留在 `eval` 里,否则 dash 之类会整行解析失败。改动这段命令后必须跑 `shell_integration` 的 shell 执行测试(用本机 `sh/bash/zsh/dash/ksh` 做真实语法与行为校验)。
- `change_directory_command` 构造文件管理→终端方向的 `cd`:前置 `\x15`(Ctrl+U)清掉用户可能输入到一半的命令行,前导空格配合 ignorespace 不进 history,`printf '\033[A\r\033[J'` 擦除本行回显且**必须排在 `cd` 之前**,这样 `cd` 失败的报错落在被清除的位置上仍然可见。路径按 `'\''` 收敛单引号。
- `BootstrapOutputFilter` 只隐藏本次 bootstrap 已识别的命令回显、执行输出和完成标记，不能按时间丢弃整个 PTY 流。命令回显前到达的 MOTD、Last login、shell banner 必须立即进入 `TermEngine`；`ExtendedData`/stderr 始终可见。过滤器有 3 秒硬期限与 64 KiB 上限，标记缺失、输出异常或用户开始输入时必须原样冲刷未确认缓存，数据保留优先于注入不可见。
- 注入是**尽力而为的增强**:shell 不支持、被 `sudo -i`/`su`/`docker exec` 换掉进程、语法在受限 shell 下失败,都只会让这条路径失效并静默退回 UI 侧的输入推断,不产生用户可见错误。



文件:`crates/kt-core/src/term/`(`mod.rs`/`color.rs`/`snapshot.rs`)

- `TermEngine` 包装 `alacritty_terminal`,产出 `GridSnapshot`(行列单元格 + 光标 + 颜色),`advance(bytes)` 喂入输出,`resize/scroll`,`take_events()` 取 Bell/Title 等。
- `GridSnapshot.alt_screen` 反映终端是否处于备用屏(vim/top/less)。任何向 PTY 写 shell 命令的功能都必须先看这个标志:备用屏下写入会被那个全屏程序当按键消费,既改不了目录还会破坏用户正在编辑的内容。
- scrollback 方向契约：`ToCore::Scroll`/`TermEngine::scroll` 中正数表示进入历史，负数表示回到底部；WebView 滚轮 `deltaY < 0`（向上滚）应转换为正数。`alacritty_terminal` 的 `display_iter` 使用包含历史行的终端坐标，构建 `GridSnapshot` 时必须用 `point_to_viewport(display_offset, point)` 转成可见视口坐标，不能直接丢弃负 line。
- 用户在历史视口开始输入非空终端数据时，`SessionTask` 必须先调用 `TermEngine::scroll_to_bottom` 并发送一次 Render，再把输入写入远端 shell；空输入或已经位于实时底部时不得增加 revision 或产生无效渲染。该行为保证 `docker logs`、`docker ps` 等长输出后下一条命令立即回到当前提示符。
- `GridSnapshot` 中的单元格颜色是 core 层解析后的最终显示色:反色、DIM 等属性在快照生成时完成颜色计算,UI 不应再次反转前景/背景。终端字符必须以普通文本节点渲染,不得使用 HTML 注入式渲染,避免 `<`、`&` 等字符破坏 DOM。终端 cell 的 inline style 必须显式写出可跨帧变化属性的默认值(如 `background: transparent`、`text-decoration: none`、`opacity: 1`),避免 WebView/Dioxus 样式 diff 后残留备用屏程序的色块。

## kt-ui / kt-app:GUI

文件:[crates/kt-app/src/main.rs](../crates/kt-app/src/main.rs)、[crates/kt-ui/src/components/app.rs](../crates/kt-ui/src/components/app.rs)

- `kt-app` 负责解析最小入口参数并初始化日志。桌面端获取数据目录 `kitonyterms.lock` 排他锁、创建无系统装饰窗口与图标后用 Dioxus Desktop `launch(App)`；Android/iOS 跳过桌面单实例与原生对话框，改用 Dioxus mobile launcher。第二个桌面实例获取锁失败时显示随系统语言变化的原生提示并退出。当前支持无参数或 `--gui` 启动 GUI、`--help` 查看用法；旧 `--safe`、`--system-ssh`、`--show-log`、`--list` 会明确报错。
- `App` 通过全局 `Store` 与 `AppState` 懒初始化 `SessionManager`。UI 每 16ms 泵送 `FromCore`，每 100ms 从 `AppState.sessions` 同步会话列表。
- **SFTP 内嵌编辑器**:[inline_editor.rs](../crates/kt-ui/src/components/inline_editor.rs) 在应用内直接编辑远端文本文件,全平台可用,入口为桌面右键菜单的「编辑」与手机动作面板的「编辑」。与外部编辑的分工:外部编辑把文件交给系统编辑器并监听本地 mtime;内嵌编辑器把文件读进应用内编辑框,由用户**显式点保存**后回传,保存成功即关闭。Android/iOS 没有可用的外部编辑器链路,内嵌编辑器是手机上唯一的编辑路径。状态机 `sync_inline_edit` 与 `sync_external_edits` 同构:按 `request_id` 精确消费请求级完成/失败,清空 `request_id` 保证 `Load` 只派发一次;两者共用 `state_controller` 的同一个 250ms 循环(`EditSignals`),因为都靠 `sftp_completions`/`sftp_failures` 推进。同一时刻只允许一个内嵌编辑会话(全屏模态),状态是 `Option<InlineEdit>` 而非 `Vec`。**准入限制**:编辑框一次性把全文读进内存,因此超过 `INLINE_EDIT_MAX_BYTES`(1 MiB)的文件在**下载前**就按目录列表里的 size 拒绝,读取阶段再复查一次(列表 size 可能过期);非 UTF-8 内容按二进制拒绝,不渲染成乱码。**失败语义**:下载失败清理临时文件并只能关闭;**回传失败必须保留编辑器与临时文件**,且不更新 `original`,这样「有未保存改动」的判定与重试都仍然成立。回传复用既有 SFTP Upload 链路,因此覆盖写仍是 kt-core 的「同目录唯一临时文件 + 提交」原子语义。
- **软键盘遮挡量的唯一所有者**:`--kt-keyboard-inset` 只由 [device.rs](../crates/kt-ui/src/device.rs) 的移动端常驻 eval 写入(`visualViewport` 监听)。终端键位条与内嵌编辑器都要按它收缩,而两者不会同时挂载,放在任一方都会漏(内嵌编辑器打开时终端桥卸载会把 inset 清零,导致保存按钮被键盘盖住)。
- **主界面结构**:桌面为应用内顶栏(`main_shell/desktop_titlebar.rs`，拖动、双击最大化、紧凑会话标签、左侧资源区折叠、唯一设置入口和窗口控制) + 可折叠左侧边栏(分组连接树、SFTP 表格) + 中央终端工作区 + 含紧凑五项监控的底部状态栏；折叠侧栏保持节点挂载，以宽度和分隔条过渡收起，顶栏按钮始终可恢复，且必须清除进行中的拖拽状态。桌面端会话标签只渲染在顶栏，工作区中的标签栏仅供未渲染桌面顶栏的移动端使用；分屏入口统一位于终端右键菜单，标签栏不承载分屏按钮。顶栏图标使用 CSS `data-tooltip` 或 `title` 的即时 tooltip，禁止仅依赖 WebView 的延迟原生 tooltip。需要用户注意的同步、错误或迁移状态以右下角浮层展示，连接就绪与空工作台引导不得作为通知。样式集中在 [app.css](../crates/kt-ui/src/assets/app.css)。[app.rs](../crates/kt-ui/src/components/app.rs) 是主编排组件,保留全局信号、上下文菜单、弹窗和跨模块动作;[state_controller.rs](../crates/kt-ui/src/components/state_controller.rs) 负责事件泵、会话列表同步、主机密钥提示同步与编辑副作用;[main_shell.rs](../crates/kt-ui/src/components/main_shell.rs) 负责主工作台外层调度,其子模块 `main_shell/sidebar_panel.rs`、`main_shell/workbench_panel.rs`、`main_shell/status_bar.rs` 分别承接连接/SFTP 侧边栏、终端工作区、含监控的底部状态栏;安全认证对话框、外部编辑状态机、侧边栏/SFTP 右键菜单、连接/分组/命名对话框已拆到独立模块;[app_logic.rs](../crates/kt-ui/src/components/app_logic.rs) 保存分组归并、会话状态初始化、SSH config 合并、连接状态 selector 等纯逻辑;[app_runtime.rs](../crates/kt-ui/src/components/app_runtime.rs) 保存 Store-backed AuthProvider 与 KnownHostsVerifier。后续深拆目标是更细粒度 selector 与 `state_controller` 集成断言。
- **手机端独立 Shell**:`target_os` 分不开手机与平板，改由 [device.rs](../crates/kt-ui/src/device.rs) 在**运行时**按视口短边判定(`device_class_for_min_side`，阈值 600 CSS px，对齐 Android `sw600dp`)。桌面 target 恒为 `Desktop` 且不起 eval；Android/iOS 起一次常驻 eval 监听 `resize`/`orientationchange`，旋转与折叠屏展开都能跟随。手机走 [phone_shell/](../crates/kt-ui/src/components/phone_shell/)(顶栏 + 全屏视图 + 底部四标签:服务器/终端/文件/监控)，**平板与桌面继续复用 `main_shell`**。两套 Shell 共用 `ShellArgs` 入参，由 `app.rs` 在同一层条件渲染，因此 `render_main_shell` 与 `render_phone_shell` **都必须保持无 hook**——设备类型会随旋转切换，任何一侧在这一层调用 hook 都会打乱 Dioxus 的 hook 顺序；需要局部状态的部分一律下沉到 `#[component]`，跨 Shell 的 `phone_tab`/`phone_sheet` 信号在 `app.rs` 无条件创建。手机端交互替代:行尾/顶栏 `⋮` 打开底部动作面板(`phone_shell/action_sheet.rs`)取代右键菜单;SFTP 单击(而非双击)进入目录;不提供分隔条拖拽、分屏与 hover tooltip。手机端**不提供 SFTP 外部编辑与打开方式**:`external_edit.rs::open_with_system_default` 在非 macOS/Windows 分支调用 `xdg-open`，Android/iOS 上必然失败，不存在可用的「外部编辑器 + 回传」链路。
- **手机端终端输入**:桌面终端是 `div[tabindex] + onkeydown`，在 WebView 里聚焦 `div` **不会唤起软键盘**，因此手机端必须挂一个真实可聚焦的 `textarea`(`phone_shell/keyboard.rs`，视觉不可见但不能用 `display:none`/`visibility:hidden`)。常驻 eval 把事件回传 Rust:字符走 `input` 事件(GBoard 对字符只给 `keyCode 229`，keydown 里拿不到内容)，取值后清空，**不能用 Dioxus 受控 `value`**;IME 组合期间(`compositionstart`~`compositionend`)不得取值与清空，否则中文/日文上屏丢字，`compositionend` 后浏览器补发的 `input` 读到空串因此不会重复发送;`keydown` 只拦 Enter/Backspace/Tab/Escape/方向键等软键盘会真实派发的键。字节序列一律复用 `terminal.rs` 的 `terminal_input_for_key`(经 `terminal_input_for_key_name` 做 web key 名映射)与 `terminal_input_for_text`，禁止另写一套 escape 序列。键位条提供 Esc/Tab/Ctrl/Alt/方向键/Home/End/PgUp/PgDn 等，Ctrl 与 Alt 为**粘滞修饰键**，只作用于下一个按键或软键盘字符的第一个字符，消费后自动熄灭。软键盘遮挡量由 `visualViewport` 在 JS 侧直接写入 `--kt-keyboard-inset`，`.phone-shell` 据此收缩，终端自己的 `ResizeObserver` 会因此重算 PTY 行数，不需要额外往返。
- **selector 边界**:`app_logic.rs` 中的 `SessionTabView / ActiveSftpView / ActiveMonitorView / StatusBarSessionView / ActiveTerminalView` 是主工作台的轻量视图模型。SFTP、Monitor、状态栏和会话标签不应直接依赖完整 `SessionState`;终端区域可以通过 `ActiveTerminalView` 持有 `GridSnapshot`,但不要为了比较或 memo 强行给大快照引入伪等价语义。`state_controller::resolve_active_session_id` 统一处理 active session 缺失、过期和空列表,会话列表同步时按 `SessionId` 排序以保持 UI 顺序稳定。
- **UI 抽离约定**:接收 `Arc<Mutex<AppState>>`、`Arc<Store>`、大量 `Signal` 或闭包的重状态入口优先使用普通函数返回 `Element`,不要默认写成 Dioxus `#[component]`;只有 props 天然适合 `PartialEq`、边界清晰且可复用的展示单元才使用组件。这样避免为了通过 props 派生而给运行时对象引入伪等价语义。
- **终端渲染**:[terminal.rs](../crates/kt-ui/src/components/terminal.rs) 使用 `GridSnapshot` 渲染 HTML 行列，并把键盘、滚轮输入转成 `ToCore::Input`/`Scroll`。`CellAttrs::wide` 左格必须按 `2ch` 渲染，`wide_spacer` 后继格必须保持 `0` 宽度并跳过文本绘制。
- **终端复制粘贴**:复制读取 DOM 选区后走 WebView `navigator.clipboard.writeText`(带 `execCommand` 回退)；粘贴在桌面端必须调用 [clipboard.rs](../crates/kt-ui/src/clipboard.rs) 的 `read_text()`(`arboard` 原生剪贴板)，因为 WebView 的 `navigator.clipboard.readText()` 会触发系统粘贴确认(macOS 上要额外点一次系统 Paste 按钮)。原生读取失败才回退到 WebView 剪贴板，移动端只有 WebView 一条路径；粘贴文本统一由 `terminal_paste_input` 归一化换行后发 `ToCore::Input`。
- **会话标题边界**:`SessionState.title` 是用户保存的服务器/会话名称,用于标签、侧边栏高亮与状态栏;远端 OSC title/ResetTitle 事件不得覆盖它。若后续需要展示远端窗口标题,应新增独立字段。
- **分屏与触发器高亮**:终端工具栏可切换水平/垂直双视图,当前为同一 session 的本地双视图;`AppSettings.trigger_highlights` 提供行级文本触发器,由 [terminal.rs](../crates/kt-ui/src/components/terminal.rs) 做大小写不敏感匹配并加高亮 class。
- **SFTP 面板**:[sftp.rs](../crates/kt-ui/src/components/sftp.rs) 通过 `AppState::send_sftp_request` 分配 `SftpRequestId` 并发送请求，从全局 `SessionState` 同步 `sftp_path/sftp_entries/sftp_loading/sftp_error/sftp_progress`。目录列表和超时只接受当前 request ID，迟到结果不得覆盖新目录。近期完成/失败事件保存在有界队列，外部编辑器按 request ID 精确消费；同路径任务彼此隔离，下载失败清理临时文件，上传失败保留本地文件并回到可重试状态，会话关闭收敛全部进行中任务。`SftpStopped` 清理 loading/progress 但不覆盖已有错误。
- **目录自动同步**:开关持久化在 `AppSettings.sftp_auto_sync`，用户勾一次之后新建会话直接沿用。两个方向按可靠性分层：
  - **终端→文件管理**(跟随过程零命令)：连接就绪或开关打开时发一次 `ToCore::SetupShellIntegration` 注入 OSC 7 上报 hook（每次连接最多一次，`SessionState.shell_integration_requested` 把关，重连后重置），之后 `cd`、`pushd`、脚本内切目录、子 shell 退出都会自然上报。注入失效时退回 UI 侧输入推断：`state.rs::parse_directory_intent` 只识别能可靠还原的形式（`cd <path>`、`cd`、`cd ~`、`cd ~/x`、`cd -`、`pushd <path>`、`popd`），依赖 `remote_home`（首个 `.` 列表 canonicalize 得到）、`terminal_prev_cwd`（OLDPWD）与 `terminal_dir_stack`。别名、函数、脚本、子 shell、`~user`、失败的命令一律不猜；`pushd` 只在目标真能解析时才压栈，避免本地栈与远端漂移。用户刚提交的推断目标在收到相同 OSC 7 确认前优先于旧目录事件。
  - **文件管理→终端**(必须写 PTY)：`AppState::send_terminal_cd` 是自动同步与手动同步按钮**共用的唯一写入点**，统一做连接检查、备用屏拒绝、`shell_integration::change_directory_command` 转义与待确认目标清理。备用屏下手动按钮报 `TerminalCdBlocked::AltScreen`（i18n 文案），自动同步则静默跳过——不该为了后台跟随打断正在用 vim 的用户。
- **资源监控**:[state.rs](../crates/kt-ui/src/state.rs) 收到 `Connected` 后自动发送 `StartMonitor` 并进入 `monitor_loading`;core 成功采样返回 `Monitor`,失败/超时返回 `MonitorError`;正常通道关闭返回 `MonitorStopped` 清理等待态,不展示为错误。监控子任务退出后会通知会话重置启动状态,允许后续重新 `StartMonitor`。延迟采样优先 TCP connect 当前会话 SSH `host:port`,失败时回退到已连接 SSH monitor 通道心跳,不得阻塞资源采样。磁盘采集使用 `df -P -k` 的 `1024-blocks` 总量字段，不得用 used+available 推算；UI 优先展示 `/` 根挂载点，缺失时安全降级为 `--`。Monitor 固定展示 CPU、内存、硬盘、负载、网络五张卡片，网络下行与上行在卡片内纵向排列；loading 和空工作台占位必须保持相同卡片结构。
- **连接失败展示**:`FromCore::Closed{error}` 必须写入 `SessionState.connection_error`；无错误的远端关闭也写入“SSH 会话已断开”。关闭和 `AppState::connect_session` 都必须清除终端快照、目录、SFTP 请求/列表与监控瞬态数据，保留会话标签并显示重连操作；终端占位、状态栏和会话状态点都要把断开会话显示为失败/断开,不得继续使用 connecting 文案或黄色连接中状态。
- **持久化**:[store.rs](../crates/kt-ui/src/store.rs) 桥接 `kt-config`(会话明文)与 `kt-secrets`(机密)。Config 更新采用 clone-save-swap，保存失败不污染内存；vault 使用 `set_and_save`，保存失败恢复旧值与 dirty 状态。Store 启动时自动打开或创建应用托管 vault,保存连接后按 `effective_vault_id()` 写入密码;Store-backed `AuthProvider` 重连时直接读取 `user@host:port` 或配置的 `vault_id`。旧主密码 vault 无法自动打开时会备份为 `secrets.vault.legacy` 并创建新的托管 vault,状态栏提示旧保存密码暂不可用;若初始化/备份失败则保持 `VaultState::Locked` 并让读写返回明确错误。secret 值不得写入 `config.toml` 或日志。
- **配置同步**:[kt-sync](../crates/kt-sync/src/lib.rs) 只序列化 `Config` envelope；UI 必须经 `Store::config_snapshot` / `replace_config_snapshot` 获取和应用快照，落盘成功后才替换内存。WebDAV 密码不进入 `AppSettings`、vault 或同步载荷；下载得到的 ETag 与完整资源 URL 绑定，后续上传用 `If-Match`，没有已知版本时只允许 `If-None-Match: *` 首次创建。局域网分享前台有效，配对码不进 URL、HTTP 请求、日志或持久化；GET 只交付 AEAD 密文，客户端成功解密与落盘后发送认证 ACK，ACK 响应交付成功或 TTL 到期才失效。发送/确认失败必须回滚到可重试状态；活跃连接有硬上限和超时，地址发现必须支持 IPv4/IPv6 接口枚举。
