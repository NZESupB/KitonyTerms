# 代理文档索引

## 技术治理

`index.md` - 项目代理文档索引，记录文档读取场景、测试验证要求与全局重要记忆。
`architecture.md` - KitonyTerms 整体架构、crate 职责、core/UI 协议、GUI 模块边界与 UI 抽离约定；修改跨模块逻辑、UI 编排、core 通信或持久化边界时必读。
`maintenance.md` - 长期维护规程，记录功能更新影响清单、轻量回归套件与季度治理核对；修改功能、协议、UI 状态或持久化语义前必读。
`governance/rust.md` - Rust workspace 的开发、测试与安全审计基线，修改 Rust 代码时必读。

## 当前任务文档

`workflow/260820-mobile-phone-ui.md` - 手机端独立 UI（设备判定、手机 Shell、软键盘输入、触屏交互替代）；修改移动端界面时必读。
`workflow/260823-fix-mobile-build-number-permissions.md` - 修复 Actions 构建号 Git ref 写入 403，改为只读时间槽分配；修改移动 CI 构建号时必读。
`workflow/260823-fix-dioxus-objcopy-loader.md` - 修复 Dioxus 移动 release 的 rust-objcopy/LLVM 动态库加载；修改移动打包脚本或 workflow 时必读。

## 已归档完成任务摘要

- SSH 登录信息与编辑器诊断修复（`260825-fix-ssh-login-banner`）：shell integration 不再用时间窗口吞整个 PTY 流，改为完成 OSC 标记 + 精确回显过滤，MOTD/Last login 与 stderr 始终可见，异常/用户输入时原样冲刷缓存；新增竞态 roundtrip。附件中的 8 条 `scanner.rs` 警告确认是 rust-analyzer 旧快照，真实 workspace check/test/clippy 均通过。
- 局域网 v2 配对与扫码修复（`260824-fix-lan-pairing-and-scanner`）：旧 v1/32 位 hex/8 位短码全部废弃，只接受 26 位 Crockford Base32 高熵秘密；删除全局认证失败销毁分享，ACK/TTL 完成后 UI 自动回收；扫码改为 8 FPS Base64 灰度帧并显式释放摄像头，移动产物校验 CAMERA/NSCameraUsageDescription。
- 终端与文件管理目录双向同步重构(`260818-shell-cwd-sync`)：终端→文件管理改为「每连接一次的 shell 集成注入 + 输入推断兜底」，注入输出由完成标记过滤且不得丢登录信息；文件管理→终端收敛到唯一写入点并加 Ctrl+U 清行、回显自擦除、备用屏拒绝；开关持久化到 `AppSettings.sftp_auto_sync`。
- Rust/Dioxus 升级与 Android 验证（`260821-rust-dioxus-android-validation`）：当次以 Rust 1.98.0、Dioxus 0.7.10 完成验证；当前版本策略已改为跟随最新稳定版。dev/test profile 限制依赖调试符号并关闭增量；Android API 35 debug APK 已在 `ktdbg` 模拟器通过 ADB 启动，edge-to-edge、同步入口和 native 加载已验证；临时 Cargo target、Android 专用中间产物及旧 incremental 合计回收约 17.6 GiB。
- Rust/Dioxus 滚动稳定版策略（`260821-follow-latest-stable-toolchain`）：Rust 工具链与 CI 使用 `stable` 且不声明数字 MSRV；Dioxus crate 使用开放稳定版本范围，CLI 安装不传 `--version`；`Cargo.lock` 继续提交为当次全量门禁验证快照，依赖升级通过 `cargo update` 显式完成。移动打包契约会阻止数字版本固定回归。
- 移动端沉浸式与配置同步（`260821-mobile-immersive-sync`）：生成工程 edge-to-edge 注入、非机密 Config 的局域网/WebDAV 同步及 UI 入口已完成；验证细节见上一条归档任务。
- 局域网同步协议加固（`260821-harden-lan-sync`）：LAN 配对码改为本地派生 HMAC/AEAD 密钥而不经网络发送，配置正文使用 ChaCha20Poly1305 加密；Store 原子落盘后显式 ACK 才消费分享，发送失败可回滚；连接并发、HTTP 头与处理时间有界，地址发现支持 IPv4/IPv6 接口枚举。
- 窗口交互与 SFTP 自动同步：侧栏折叠按钮居中并使用缓动动画；桌面设置遮罩保留标题栏拖动；文件管理加入会话级自动同步的早期版本（仅单向可用，已被上一条取代）。
- 稳定连接基线：补齐连接、会话生命周期与错误收敛的早期方案。
- 连接、SFTP 同步与一体化顶栏：core 按内部代次回收自然结束任务；断开会话可直接重连并清除运行时残留；SFTP 主动以 OSC 7 同步 `$PWD`；桌面端改为应用内窗口控制，不再创建系统标题栏或原生菜单；macOS 控制置左，终端 CJK 双宽字符按两列渲染。
- README 第四阶段：同步功能里程碑、README 状态与功能声明。
- SFTP 文件管理：沉淀右键菜单、外部编辑菜单、保存确认对话和回传策略。
- 架构审查：确认项目适合继续维护，指出 UI 主组件过大、通道背压、vault 解锁、known_hosts 安全语义与认证能力缺口。
- 功能性问题优化(`260628-functional-optimizations`)：终端键位/尺寸、监控延迟与占用、主题入口、文件管理、服务器分组、SSH 密码保存与密钥登录。
- 架构演进框架(`260627-architecture-evolution`)：早期入口能力对齐、Monitor 闭环、UI 拆分、安全策略与背压治理计划。
- 统一优化路线图(`260628-implementation-roadmap`)：阶段 1~7 完成——安全、并发、认证、UI 模块化、文档收敛与长期维护规程。
- 界面与菜单体验修复批次(`260629-polish-menu-terminal-auth`/`260629-menu-polish-followup`/`260630-urgent-connection-ui-polish`)：macOS 系统菜单与设置入口、认证弹窗密码保存、TCP 延迟显示与高延迟颜色、监控色块、浅色主题、应用内顶栏移除与右键编辑入口等体验打磨。
- 连接对话框与编辑器设置：会话/代理使用左侧条件渲染选项卡，编辑器通过 PATH/macOS app/环境变量探测并以下拉选择，既有自定义命令必须保留。
- Rust 工具链使用 `stable` 通道且不声明数字 MSRV；Dioxus crate 允许解析最新非预发布版本，Dioxus CLI 安装时不传 `--version`。`Cargo.lock` 仍必须提交，用于记录当次完整验证过的依赖快照；升级依赖使用 `cargo update`。`Dioxus.toml` 固定 Android application ID 与 iOS Bundle ID 为 `com.kitonyterms.app`；Android 配置与 vault 必须位于应用私有 `files/config`、`files/data`，不得回退到依赖 `$HOME` 的桌面路径。
- 手机与平板由 `kt-ui/src/device.rs` 在**运行时**按视口短边判定（阈值 600 CSS px），不能用 `target_os` 区分。手机走 `components/phone_shell/`（顶栏 + 全屏视图 + 底部四标签），平板与桌面复用 `main_shell`。两套 Shell 共用 `ShellArgs`、在 `app.rs` 同一层条件渲染，因此 `render_main_shell` 与 `render_phone_shell` **都必须保持无 hook**，局部状态一律下沉到 `#[component]`，跨 Shell 的 `phone_tab`/`phone_sheet` 在 `app.rs` 无条件创建。
- 手机端终端输入必须挂真实可聚焦的 `textarea`（聚焦 `div[tabindex]` 唤不起软键盘），字符走 `input` 事件、IME 组合期间不得取值清空，功能键走 `keydown`；字节序列一律复用 `terminal.rs` 的 `terminal_input_for_key_name` / `terminal_input_for_text`，不得另写一套 escape 序列。软键盘遮挡量由 `visualViewport` 直接写 `--kt-keyboard-inset`。
- 手机端不提供分屏与 SFTP 外部编辑/打开方式：`open_with_system_default` 在 Android/iOS 会调用 `xdg-open`，必然失败。文件的编辑走**内嵌编辑器**（全平台可用，见下条）。触屏交互用行尾/顶栏 `⋮` 动作面板替代右键菜单，SFTP 单击（而非双击）进目录。
- SFTP 内嵌编辑器（`components/inline_editor.rs`，全平台）：显式点保存后回传，保存成功即关闭。超过 `INLINE_EDIT_MAX_BYTES`（1 MiB）的文件按目录列表 size 在**下载前**拒绝、读取时再复查一次；非 UTF-8 内容按二进制拒绝。**回传失败必须保留编辑器与本地临时文件且不更新 `original`**，否则用户的编辑会丢、重试也判定不出「有改动」。与外部编辑共用 `state_controller` 的同一个 250ms 循环（`EditSignals`）。
- `--kt-keyboard-inset`（软键盘遮挡量）只由 `device.rs` 的移动端常驻 eval 写入。终端键位条与内嵌编辑器都按它收缩，两者不会同时挂载，放在任一方都会漏。
- rsx 的**组件 prop 位置不支持 `if` 表达式**（元素属性位置支持）。`Icon { name: if cond {"a"} else {"b"} }` 会报出指向整个 `rsx!` 块的 `expected &str, found String`，必须先算好再传。
- `kt-ui`/`kt-app` 的 `phone-preview` 特性让桌面端也按视口短边判定设备类型，用于在开发机上预览手机 Shell（`cargo run -p kt-app --features phone-preview` 后把窗口缩窄）；正式构建不启用。本机 Rust Android target、Dioxus CLI、OpenJDK 17、Android API 35/Build Tools 35.0.0/NDK 27.2.12479018 与 `ktdbg` AVD 均已验证；debug APK 已通过 ADB 安装启动，系统栏 edge-to-edge 与 WebDAV/LAN 设置入口已做模拟器 smoke test。
- 移动端/SFTP 体验：SFTP 跟随终端目录后路径输入框必须同步；监控固定收敛在底栏五项紧凑视图，阻断性状态使用右下角浮层。**竖屏纵向布局那一版已被手机独立 Shell 取代**，见上文手机端条目。
- 桌面顶栏布局：会话标签属于应用顶栏且保持紧凑固定宽度并隐藏滚动条，左侧服务器/SFTP 区可由顶栏折叠并随时恢复且使用宽度过渡，设置只保留顶栏带文字入口；分屏从终端右键菜单进入；顶栏和带 `title` 的图标使用即时 CSS tooltip。
- UI 体验修正：终端保留可横向查看长日志的宽度并支持历史视口方向键/横向滚轮，SFTP 外部编辑回传框改为不透明样式，外部编辑通知自动消退，桌面品牌和底部连接状态文本移除。

## 测试与验证要求

- Rust 代码变更后至少运行 `cargo fmt --all -- --check`、`cargo check --workspace --all-targets`、`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`。
- 若涉及 UI 行为、终端渲染、SSH/SFTP 交互或密钥处理，应补充对应 crate 的单元测试或集成测试。
- 如仅进行代码审查且未改动业务代码，可运行只读检查或现有测试来辅助判断。

## 全局重要记忆

- 项目为 Rust workspace，按职责拆分为核心协议与会话、配置解析、密钥存储、UI 与应用入口等 crate。
- 配置同步由 `kt-sync` 承担：只同步 `kt_config::Config`，不触碰 vault、vault key、known_hosts、锁文件或运行时状态；WebDAV 通过完整 URL 与 ETag 条件写避免并发覆盖。局域网仅支持 v2 和 26 位 Crockford Base32 配对秘密，不兼容 v1、32 位 hex 或 8 位短码；秘密不经网络发送，使用 HMAC nonce 请求认证与 ChaCha20Poly1305 加密载荷，认证失败不得全局销毁分享。UI 必须先经 Store 的 snapshot/replace 原子边界成功落盘，再发送 ACK 消费分享，并在 ACK/TTL 结束后回收分享状态。连接处理必须保留并发上限、超时、失败回滚与 IPv4/IPv6 接口枚举。
- UI 中接收 `Arc<Mutex<AppState>>`、`Arc<Store>` 或大量 `Signal` 的重状态入口优先使用普通函数返回 `Element`；仅展示型、props 可自然比较的单元使用 Dioxus `#[component]`。
- Dioxus Desktop 会把布尔 HTML 属性写成 `attr="false"`；对 `inert`、`hidden` 等按属性存在与否生效的布尔属性，false 必须传 `None` 让渲染器移除属性，不能直接传 bool。
- 主工作台子布局应优先接收 `app_logic.rs` 中的轻量 selector 视图（如 SFTP、Monitor、状态栏、会话标签），避免直接传递完整 `SessionState`。
- 每次功能更新前先按 `maintenance.md` 填写影响清单；新增 `app.rs` 之外模块逻辑时优先补纯逻辑单测，再接入渲染或副作用。
- Store 启动时自动打开或创建应用托管加密 vault；当前安装会生成独立 `secrets.vault.key` 作为本机自动密码库密钥，旧固定密钥 vault 会原地迁移，旧主密码 vault 无法自动打开时备份为 `secrets.vault.legacy*` 后重建新 vault。
- Monitor 延迟优先 TCP connect 当前 SSH `host:port`，失败时回退 SSH 心跳；UI 中延迟合并到网络标题展示并用颜色分级提示高延迟。
- SSH 支持 TCP 级代理（`kt_config::ProxyConfig`：Direct/System/Socks5/Http）：`crates/kt-core/src/ssh/proxy.rs` 经代理建立到目标的 `TcpStream` 后交给 `russh::client::connect_stream` 握手，`connect_direct` 统一分派，`Direct`/System 未解析出代理时回退直连。System 读取 `ALL_PROXY/HTTPS_PROXY/HTTP_PROXY/SOCKS_PROXY`（大小写各一），只接受 `socks5/socks5h/socks/http` scheme；`https://` proxy 与未知 scheme 明确失败，HTTP CONNECT 的 IPv6 authority 使用 `[host]:port`。代理与 ProxyJump 组合时代理仅作用于最外层 TCP，目标段走 direct-tcpip。代理凭证不接入 vault，仅以 username+空密码尝试认证。
- 终端与文件管理的目录同步按可靠性分层，开关持久化在 `AppSettings.sftp_auto_sync`。**终端→文件管理**：首选每连接一次的 shell 集成注入（`ToCore::SetupShellIntegration` → `kt_core::shell_integration::BOOTSTRAP_COMMAND`），让远端 shell 自己在每次 prompt 前发 OSC 7，此后跟随过程零命令；bootstrap 末尾发不可见完成标记，`BootstrapOutputFilter` 只过滤已识别的命令回显与执行段，标记前的 MOTD/Last login 必须正常进入 `TermEngine`，stderr 永不参与过滤，超时、超限或用户输入时原样冲刷未确认数据。注入失效（`sudo -i`、`su`、`docker exec`、受限 shell）时退回 UI 输入推断，只识别 `cd <path>` / `cd` / `cd ~` / `cd ~/x` / `cd -` / `pushd <path>` / `popd`，依赖 `remote_home`（首个 `.` 列表 canonicalize 得到）、`terminal_prev_cwd`、`terminal_dir_stack`；别名、函数、脚本、子 shell、`~user`、失败命令一律不猜。**文件管理→终端**：SSH 改不了运行中 shell 的 cwd，只能写 PTY，全部经 `AppState::send_terminal_cd` 这一个写入点，命令为 `Ctrl+U` + 前导空格 + 前置 `printf '\033[A\r\033[J'` 擦除回显 + 单引号转义的 `cd --`；擦除必须排在 `cd` 前面，`cd` 失败的报错才可见。
- 注入命令必须**追加**而不是覆盖用户的 `PROMPT_COMMAND`（含 bash 5.1+ 数组形态）、`precmd_functions`、`HISTCONTROL`；zsh 专用语法要包在 `eval` 里，否则 dash 会整行解析失败。改这段命令后必须跑 `cargo test -p kt-core shell_integration`——那里用本机 `sh/bash/zsh/dash/ksh` 做真实语法与行为校验，纯字符串断言挡不住这类失效。
- 任何向 PTY 写 shell 命令的功能都必须先检查 `GridSnapshot.alt_screen`：终端在跑 vim/top/less 时写入会被那个程序当按键吃掉。手动同步按钮报 `TerminalCdBlocked::AltScreen`（走 i18n），后台自动同步静默跳过、不打断用户。
- 已知限制：远端路径含形如 `%3A` 的合法百分号转义时，OSC 7 解码会误还原（多数 shell 不做 URL 编码，而 fish 会）。后果只是 SFTP 跟随到错误路径并显示列表失败，不涉及安全。
- SFTP 外部编辑支持自定义编辑器：`AppSettings.default_editor`（默认编辑器命令，`{file}` 占位）与 `AppSettings.editors: Vec<EditorEntry>`（右键"打开方式"列表）。`external_edit.rs::open_local_file_with` + `build_editor_command` 解析命令模板，`ExternalEdit.editor_command` 贯穿下载→打开链路，缺省回退系统默认程序。设置 UI 用 `external_edit.rs::detect_editors`（PATH + macOS `.app` bundle + Linux/Windows 候选，按名去重）与 `env_editor_command`（`$VISUAL`/`$EDITOR`）下拉选择编辑器，不再让用户手填命令；既有非空命令以「自定义」option 保留不丢失。
- SFTP 外部编辑临时目录应保持本机私有权限；Unix 下目录使用 `0700`，下载目标文件使用 `0600`。
- 所有 SFTP 请求由 UI 分配 `SftpRequestId`；请求级 Listing/Progress/Done/Error 必须携带并按 ID 消费，Stopped/Closed 保持会话级语义。迟到列表和旧超时不得覆盖或终止新请求，同路径外部编辑任务不得按 path/op 猜测关联。
- SFTP 覆盖传输必须先写同目录唯一临时文件再提交，不允许先删除正式文件。提交顺序为：直接 rename（新建文件与支持 POSIX 覆盖语义的服务器一次成功）→ 目标已存在且直接 rename 失败时，把原文件改名成同目录备份、提交临时文件、删除备份，任一步失败立即把备份改回原名。OpenSSH 的 `SSH_FXP_RENAME` 在目标已存在时必然失败（`link()` 返回 `EEXIST`），因此不能只依赖单次 rename，否则所有覆盖上传（含外部编辑回传）全部失效。覆盖上传前先 `metadata` 探测目标：是目录直接拒绝，是文件则记录权限并在提交前 `setstat` 回写，避免临时文件的默认 0644 覆盖掉原权限（`FileAttributes::empty()` 只带 permissions，`default()` 会带 uid/gid 0 导致 chown 报错）。
- 终端粘贴在桌面端必须走原生剪贴板（`kt-ui/src/clipboard.rs` 的 `arboard`），不得改回 `navigator.clipboard.readText()`：WKWebView/WebView2/WebKitGTK 读剪贴板都要求系统级粘贴确认，macOS 上表现为点完「粘贴」后还要再点一次系统 Paste 按钮。原生读取失败才回退 WebView 剪贴板；移动端没有原生实现，仍只走 WebView。`arboard` 以 target 条件依赖形式只在 windows/linux/macos 引入（默认特性关闭，避免拉入 `image`），Android/iOS 依赖图中不得出现。
- GUI 通过数据目录 `kitonyterms.lock` 保证单实例；Config/KnownHosts 使用唯一临时文件原子替换，Config/vault 更新失败必须回滚内存状态。
- Host Key 待确认项使用 host/port/fingerprint 去重队列；用户操作只移除精确项并只处理匹配 host/port（含 ProxyJump）的会话。新信任落盘失败不得接受，可信 key 的 last_seen 保存失败仍允许连接并向状态栏告警。
- GUI 状态栏只展示需要用户注意的核心信息（错误、阻断性状态、重要迁移/初始化提示、正在进行的文件同步等）；不要把 host key 信任成功、一次允许成功、密码保存成功这类成功/过程/调试性质提示写入状态栏。
- 终端行号/时间戳 gutter：`AppSettings.show_line_numbers/show_timestamps`，`terminal.rs` 在 surface 左内边距带内绝对定位 gutter（resize 脚本按 padding 自动扣减，PTY 列数不受影响）；时间戳为尽力而为，用 `Rc<RefCell>` 跨帧记录每行内容签名与首见时刻。行号为包含 scrollback 历史的绝对行号：`GridSnapshot.history_size` + `first_visible_line_number()` 计算视口首行行号，滚动回看历史时行号随之减小；自动换行续行显示 `-` 且不消耗逻辑行号。正文行、gutter 行和 resize 计算必须使用同一实际行高与 surface 内边距。
- 移动端入口禁止读取 `std::env::args*`：Dioxus 的 Android/iOS 胶水层通过 `dlsym("main")` 以无参函数指针调用入口，argc/argv 为未初始化垃圾值，读取即 SIGSEGV 闪退（`kt-app::startup_command` 已按平台隔离，移动端固定走 GUI）。Android API 35 debug smoke 已确认 native `libmain.so` 加载成功且无启动崩溃。
- CI 移动端 APK 内不含 dx 前端资源（重跑 `gradlew assembleRelease` 不经过 dx 资源注入）；kt-ui 的 `app.css` 通过 `include_str!` 内嵌，不依赖 APK assets，新增前端静态资源时不得依赖 `asset!` 路径在移动端可用。
- 终端处于 scrollback 历史视口时，任何非空用户输入都必须先恢复到实时底部并立即渲染；空输入或已经在底部时不增加 revision。修改 `SessionCmd::Input` 或 `TermEngine::scroll_to_bottom` 时必须保留对应回归测试。
- 系统监控固定包含 CPU、内存、硬盘、负载、网络五项；硬盘展示 `/` 根挂载点并使用 `df` 的总块数字段，网络下行/上行在同一卡片中纵向显示，loading/placeholder 结构必须同步。
- Windows/macOS/Linux 使用 `WindowBuilder::with_decorations(false)`，不创建原生菜单栏；`main_shell/desktop_titlebar.rs` 提供应用内品牌栏、拖动/双击最大化、设置、最小化、最大化和关闭控制。macOS 窗口控制按关闭、最小化、最大化置于顶栏左侧，Windows/Linux 置于右侧；顶栏仅在桌面 target 编译，移动端继续使用 safe-area 布局。
- 终端快照的 `CellAttrs::wide` 表示双宽字符左格，UI 必须使用 `width: 2ch`；`wide_spacer` 是后继占位格，必须使用 `width: 0` 并跳过文本绘制，避免 CJK 字符与后续列重叠。
- 终端快照的 `SnapshotCell::default_fg` 表示未显式着色的默认前景色；UI 使用 `--terminal-default-fg` 随应用主题调整此类文字。ANSI、256 色、truecolor、反色、DIM 或 OSC 覆盖颜色必须保持原始 RGB，禁止用 CSS `!important` 全量覆盖终端单元格前景色。
- CI 双轨：`.github/workflows/release.yml`（v* tag→正式 Release）与 `alpha.yml`（仅 main push→滚动更新固定 `alpha` tag 的 Alpha 预发布），触发条件互斥；两者共用桌面 6 平台 matrix，并将 Android/iOS `aarch64` 拆为独立 job。产物架构命名统一用 `x64` / `aarch64`（Rust target triple 仍使用标准 `x86_64-*` / `aarch64-*`）。Android 签名集中在 `.github/scripts/package-android-apk.sh`，iOS 未签名封装与校验集中在 `package-ios-ipa.sh`。
- 仅 Android job 绑定受保护的 `mobile-signing` Environment 并读取 `ANDROID_*` Secrets；iOS job 不得绑定签名 Environment，也不得读取 Android 或历史 iOS 签名 Secrets。Environment deployment policy 仅允许 `main` 与正式 `v*` tag，仓库还应保护主分支与 `v*` tag 创建权限。Release RustSec 扫描阻断，Alpha RustSec 仅告警；workflow 默认权限只读，仅发布 job 单独授予 `contents: write`，构建号分配器保持只读。
- Android Alpha/Release 必须共用同一个 PKCS#12 keystore，并用 `ANDROID_CERT_SHA256` 门禁签名证书；签名 Secrets 缺失或身份不匹配时失败，不允许降级发布 debug 或临时签名 APK。新版 Android Gradle Plugin 可能缩短 APK 内资源路径，launcher 图标应按 `aapt dump badging` 的 `application-icon-*` 声明复验实际 ZIP 条目，不得硬编码 `res/mipmap-*/ic_launcher.png`。iOS 只发布不含 provisioning profile 与代码签名残留的未签名 IPA，用户必须自行重签后安装，项目 CI 不保证 iOS 覆盖更新连续性。
- 两条 workflow 共用 `mobile-build-number` concurrency group；分配器不再写 GitHub ref，而是在锁内按 UTC 秒分配，使用历史值 `1,787,238,032` 作为切换下限，并将值限制在 Android 上限 `2,100,000,000` 内。锁会保持到候选秒结束，避免正常 runner 时钟下的同秒重复。
- 无持久共享状态时，构建号只能在正常时钟与 concurrency 语义下保持分配顺序单调；时钟回拨、pending 任务替换或 workflow 重建可能破坏绝对唯一/递增保证。若必须恢复任意并发、重跑和故障场景下的严格保证，应引入外部原子计数器或具有写权限的 GitHub App/PAT。Android 覆盖更新使用更高 `versionCode`；iOS 用户重签时必须保持同一 Team/application identifier、Bundle ID 与兼容 Entitlements，并使用不低于已安装包的版本和 build number。
- 正式 `v*` tag 的三段式版本必须与对应提交的 workspace `Cargo.toml` 一致，避免 Android `versionName`、iOS `CFBundleShortVersionString` 与 Release tag 漂移。
- Dioxus 0.7.10 的移动 release strip 会调用 host Rust 的 `rust-objcopy`；Android/iOS workflow 必须安装 `llvm-tools-preview`，打包脚本在调用 `dx` 前 source `.github/scripts/prepare-rust-objcopy.sh`，按宿主系统设置 `DYLD_LIBRARY_PATH`/`LD_LIBRARY_PATH` 并预检 `rust-objcopy --version`，否则 macOS 可能因 Dioxus 仅设置 `LD_LIBRARY_PATH` 而找不到 `libLLVM.dylib`。
- 滚动 Alpha 先把完整资产上传到唯一草稿 Release，再隐藏旧 Release、移动固定 `alpha` tag、公开新 Release 并复验；失败时反向恢复旧 tag/Release。GitHub 不允许 prerelease 标记为 Latest，因此保持 `make_latest: false`；“置顶”仅指 tag 指向最新成功 Alpha、该 Alpha 在发布时是最新预发布，不保证后续正式 Release 发布后仍永久排在列表第一。
