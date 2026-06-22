# KitonyTerms 整体架构

修改任意模块前必读。本文沉淀 crate 职责、UI⇄core 消息协议、SSH/SFTP 层与 GUI 结构。

## crate 划分与依赖方向

```
kt-app (GUI: eframe/egui)  ──depends──▶ kt-core ──▶ kt-config
   └─────────────────────────────────▶ kt-config        kt-secrets(被 kt-app 用于 vault)
   └─────────────────────────────────▶ kt-secrets
kt-core ──▶ kt-config        (kt-core 无 UI 依赖,可 headless 跑/测)
```

- **kt-config**:UI 无关、可序列化。`ConnectParams`(host/port/user/auth/vault_id)、`AuthMethod`(Password/PublicKey/KeyboardInteractive/Agent)、`SessionProfile`、`AppSettings`、`Config`(TOML)、`Paths`(跨平台目录:`config.toml`、`secrets.vault`、`known_hosts.toml`)、`~/.ssh/config` 合并。`effective_vault_id()` = `user@host:port`。
- **kt-secrets**:主密码加密 vault。Argon2id 派生密钥(每库随机盐)+ ChaCha20Poly1305。`Vault::create/open/set/get/remove/save`。空密码可正常派生(无长度校验)。
- **kt-core**:SSH 连接、SFTP、终端引擎,见下。
- **kt-app**:eframe/egui GUI,二进制 `kitonyterms`,见下。

## kt-core:UI⇄core 消息协议(核心)

文件:[crates/kt-core/src/session.rs](../crates/kt-core/src/session.rs)

`SessionManager` 持有一个多线程 tokio 运行时,每个会话一个 task。调用方(GUI / headless 示例)**只**通过两条 channel 通信:

- `ToCore`(UI→core):`Connect{id,params,pty}`、`Input{id,data}`、`Resize{id,cols,rows}`、`Scroll{id,delta}`、`Sftp{id,req}`、`Disconnect{id}`。
- `FromCore`(core→UI):`Connected`、`Render{snapshot}`、`Title`、`Bell`、`SftpListing{path,entries}`、`SftpProgress{name,transferred,total}`、`SftpDone{op}`、`SftpError{message}`、`Closed{error}`。

要点:
- `SessionManager::spawn(verifier, auth_factory)` 启动 `core_loop`,后者按 `id` 把命令路由到各 `SessionTask`。
- `SessionTask::run` 是一个 `select!` 循环:一边收 `SessionCmd`(由 `ToCore` 转来),一边 `shell.next_message()` 取远端输出喂给 `TermEngine`,变化时发 `Render`。
- **扩展能力的标准做法**:加 `ToCore`/`FromCore` 变体 + `SessionCmd` 变体 + `core_loop` 路由 + `SessionTask` 处理。新增 `FromCore` 变体后,记得给 UI 的 `pump_core_events`(穷举匹配)和 headless 示例(有 `Some(_)=>{}` 兜底)补齐。
- `AuthProvider`(密码/口令/keyboard-interactive)由工厂按会话创建;GUI 实现读预先填好的机密,不做握手期阻塞弹窗。

## kt-core:SSH 层

文件:[crates/kt-core/src/ssh/mod.rs](../crates/kt-core/src/ssh/mod.rs)、`ssh/handler.rs`

- `SshShell`(持有 `russh::client::Handle` 与 PTY shell `Channel`):`open()`(connect→auth→request_pty→request_shell)、`write/resize/next_message/disconnect`。
- 认证:按 `params.auth` 顺序尝试 password / publickey / keyboard-interactive(Agent 暂跳过)。
- 主机密钥:`AcceptAllVerifier`(TOFU,信任所有)。`known_hosts` 持久化未实现。
- `open_sftp(&self) -> SftpSession`:在**同一 handle** 上 `channel_open_session` → `request_subsystem(true,"sftp")` → `russh_sftp::client::SftpSession::new(channel.into_stream())`。返回独立拥有通道流的会话,可 move 进子任务;底层 TCP 由 `SshShell` 的 handle 维持。

## kt-core:SFTP 子任务

文件:[crates/kt-core/src/sftp.rs](../crates/kt-core/src/sftp.rs)

- `SessionTask` 首次收到 `SessionCmd::Sftp` 时**惰性** `open_sftp`,把 `SftpSession` move 进 `tokio::spawn(sftp_task(...))`,并保存其命令 sender;后续请求转发给该子任务。
- `sftp_task` 拥有独立 mpsc 与 `FromCore` 发送端,**串行**处理请求,故大文件传输不阻塞 shell `select` 循环。
- 请求类型 `SftpRequest`:`List`(先 `canonicalize` 成绝对路径再 `read_dir`,目录优先 + 名称不分大小写排序)、`Download`/`Upload`(用 `File` 的 tokio `AsyncRead`/`AsyncWrite` 分块拷贝,按 `PROGRESS_STEP` 节流上报进度)、`Mkdir`/`Remove`(按 `is_dir` 选 `remove_dir`/`remove_file`)/`Rename`。
- `SftpEntry`(name/is_dir/size/modified/permissions)是 core 内中立类型,**不向 UI 暴露** russh-sftp 类型。
- 依赖:`russh-sftp`(传输无关,基于流);`tokio` 启用 `fs` 特性用于本地异步文件。

## kt-core:终端引擎

文件:`crates/kt-core/src/term/`(`mod.rs`/`color.rs`/`snapshot.rs`)

- `TermEngine` 包装 `alacritty_terminal`,产出 `GridSnapshot`(行列单元格 + 光标 + 颜色),`advance(bytes)` 喂入输出,`resize/scroll`,`take_events()` 取 Bell/Title 等。

## kt-app:GUI

文件:[crates/kt-app/src/app.rs](../crates/kt-app/src/app.rs) 等

- `KitonyApp` 持有 `SessionManager`、`Store`、`tabs: Vec<Tab>`、连接表单、启动态。`eframe::App::update` 每帧:`handle_start_dialog`(未解锁前不进主界面)→ `pump_core_events`(排空 `FromCore` 到各 tab)→ 连接对话框 → 面板渲染。
- **面板顺序**(egui 要求 CentralPanel 最后):左 `side_panel`(会话列表)→ 顶 `top_bar`(标签 + 字号 + SFTP 开关)→ 底 `status_bar`(连接状态/尺寸/字号/vault 锁)→ 右 `sftp_panel`(SFTP 抽屉)→ 中 `central`(终端)。
- `Tab` 持有 `id/title/snapshot/view/status/last_grid/sftp`。终端渲染在 [terminal_view.rs](../crates/kt-app/src/terminal_view.rs)(egui `Painter` 直接画,GPU 经 wgpu)。
- **持久化**:[store.rs](../crates/kt-app/src/store.rs) 桥接 `kt-config`(会话明文)与 `kt-secrets`(机密)。锁定模型:启动 vault 未解锁;空密码 vault 静默解锁;未解锁仍可连接但不读写机密。解锁/设置见 [unlock_dialog.rs](../crates/kt-app/src/unlock_dialog.rs)(允许空主密码)。
- **主题**:[theme.rs](../crates/kt-app/src/theme.rs) 启动时设一次 `egui::Visuals`(Tokyo Night `Palette`),各面板自动继承;勿散落硬编码颜色。
- **SFTP 面板**:[sftp_panel.rs](../crates/kt-app/src/sftp_panel.rs) `SftpState::show(ctx) -> Vec<SftpRequest>`,app 给请求补当前会话 `id` 后发 `ToCore::Sftp`。支持浏览/上级/刷新/上传/下载/新建目录/删除(二次确认)/重命名;远端路径用 `join_path`/`parent_path` 拼接;`📁 SFTP` 开关首次打开发 `List{"."}`。
- 其他:`fonts.rs`(CJK 回退字体)、`input.rs`(egui 按键→字节)、`connect_dialog.rs`(新建连接,`rfd` 选私钥)。
