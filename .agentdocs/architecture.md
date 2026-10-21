# 架构与关键约束

## 模块边界

[Workspace](../Cargo.toml) 包含七个 crate：

| 模块 | 职责 |
|---|---|
| [kt-app](../crates/kt-app/src/main.rs) | GUI 入口与平台启动 |
| [kt-ui](../crates/kt-ui/src/components/app.rs) | 界面编排、AppState、Store |
| [kt-core](../crates/kt-core/src/session.rs) | SSH、SFTP、终端、监控与运维协议；不依赖 UI |
| [kt-config](../crates/kt-config/src/lib.rs) | 配置、路径、会话与 known_hosts |
| [kt-secrets](../crates/kt-secrets/src/lib.rs) | Argon2id + XChaCha20Poly1305 vault |
| [kt-sync](../crates/kt-sync/src/lib.rs) | WebDAV/LAN 非机密配置同步 |
| [kt-update](../crates/kt-update/src/lib.rs) | 桌面发布查询、后台下载、归档安全与安装事务；不依赖 UI/core |

## 会话与协议

依据：[session.rs](../crates/kt-core/src/session.rs)、[UI 状态](../crates/kt-ui/src/state.rs)、[事件泵](../crates/kt-ui/src/components/state_controller.rs)。

- UI 经 `ToCore`/`FromCore` 与 core 通信；扩展消息时同步路由、任务处理和 UI 消费。
- 连接代次隔离旧事件及认证答案；SFTP、运维、容器终端分别匹配 `SftpRequestId`、`OperationId`、`ExecId`，不得按路径猜测请求归属。
- 通道有界；GUI 投递失败必须处理。普通事件保序，终端 Render 可合并。
- 连接失败、超时、取消与关闭需清理 pending，避免迟到事件覆盖新连接。
- [SSH](../crates/kt-core/src/ssh/mod.rs) 支持单跳 ProxyJump；[TCP 代理](../crates/kt-core/src/ssh/proxy.rs)作用于最外层连接。

## 存储与信任

依据：[Store](../crates/kt-ui/src/store.rs)、[配置](../crates/kt-config/src/lib.rs)、[vault](../crates/kt-secrets/src/lib.rs)。

- Config/KnownHosts 使用临时文件提交；保存失败不得保留错误的内存更新。
- vault 使用每安装独立的 `secrets.vault.key`；旧固定密钥迁移，不能打开的旧主密码 vault 先备份再重建。
- 主机密钥按 host/port/fingerprint 去重并精确确认；首次信任必须落盘成功，不能因保存失败接受未知 key。
- Android 使用应用私有路径。配置同步不包含 vault、vault key、known_hosts 或运行时状态。
- 保存密码的 UI 查看路径由 [os_auth.rs](../crates/kt-ui/src/os_auth.rs) 先进行系统认证，再消费不可自行构造、不可复制的 `Verified` 调用 `Store::reveal_secret`；存在性检查仅用 `has_secret`，不克隆明文。macOS 每次新建 LAContext 使用设备所有者策略；Windows 优先探测 Hello，经 HWND 绑定的 `IUserConsentVerifierInterop` 验证，接口明确不支持或未配置时走账户密码；取消/认证失败不自动再弹密码窗口，界面提供独立账户密码入口。密码路径按当前 token SID 预填账户，经 LogonUserW 成功后再比较 SID；空密码由系统实际验证，不能据为空放行。凭据清零并分类提示错误，日志仅阶段/错误码。Linux/移动端拒绝查看，连接时自动 SSH 认证及 vault 格式沿用原实现。
- [密码字段](../crates/kt-ui/src/components/password_field.rs)将只读查看值与编辑值隔离，查看值不参与保存；异步结果绑定连接名称、host/user/port、vault ID 与请求代次。关闭弹窗、切换目标/页签、编辑输入或 30 秒超时清除查看值；原生系统弹窗和取消行为仍需实机验收。

## SFTP 与编辑

依据：[SFTP](../crates/kt-core/src/sftp.rs)、[内置编辑器](../crates/kt-ui/src/components/inline_editor.rs)、[外部编辑](../crates/kt-ui/src/components/external_edit.rs)。

- 下载先写私有临时文件；上传保留目标权限，经同目录临时文件提交。
- 覆盖 rename 失败时备份原文件再替换，提交失败尝试恢复备份；不得先删除正式文件。
- 内置编辑限制 UTF-8、1 MiB，下载前和读取时均校验；上传失败保留编辑内容与临时文件，允许重试。
- 内置编辑器用非受控 textarea（`initial_value` 播种、`key` 保证换文件时重建）：改回受控 `value` 会在事件往返期间回写 DOM，把光标与滚动顶到文末。
- 外部编辑按请求 ID 推进下载/回传；Unix 临时目录/文件保持 0700/0600。
- 桌面打开方式合并探测结果与用户配置；手机使用内置编辑器。
- 桌面下载入口位于右键菜单，`sftp_download.rs` 使用系统选择器并按请求 ID 观察终态；`SftpRequest::DownloadBatch` 支持单文件与目录，core 先校验清单再传输，拒绝链接、特殊文件及本地名称冲突，保留空目录。`SftpProgress` 保留当前文件的字节字段，并通过可选 `SftpBatchProgress` 携带 core 维护的累计字节与可选总量；扫描清单时统计普通文件大小，未知大小、总量溢出或传输期间大小改变后总量为 `None`，已知零字节为 `Some(0)`。UI 展示文件名与人类可读总大小，底部进度按批次字节计算，成功事件后才显示 100%，失败保留已完成文件。
- 编辑副本删除统一使用 `cleanup_edit_file`，只回收该副本及空的会话目录；外部编辑器关闭无法通用检测，应用内“结束外部编辑”明确结束监听，保存失败仍保留副本。

## 终端与目录同步

依据：[shell integration](../crates/kt-core/src/shell_integration.rs)、[终端引擎](../crates/kt-core/src/term/)、[终端 UI](../crates/kt-ui/src/components/terminal.rs)。

- OSC 7 上报远端目录；bootstrap 追加 shell hook，保留用户已有配置。
- 输出过滤仅处理已识别的 bootstrap 数据；登录信息和 stderr 保留，超时、超限或用户输入时冲刷未确认缓存。
- [命令历史采集](../crates/kt-core/src/command_history.rs)在 core PTY 收发顺序中工作：Bash/Zsh 提示符 hook 在用户已有 hook 之后发出专用标记，bootstrap 初始化不开放采集，每次提示符最多记录一条提交；无标记、备用屏、前导空格、控制编辑/补全及超过 4096 字节的输入跳过。shell 集成独立于目录自动同步开关，目录跟随仍服从开关。仅宿主 SSH 会话采集，容器独立 PTY 不采集。
- [历史面板](../crates/kt-ui/src/components/command_history.rs)从会话状态展示最近 200 条内存记录；支持标题栏/终端菜单切换、拖动或方向键调整高度、逐条原生剪贴板复制与清空，不写入配置或同步载荷，不提供自动执行。
- 文件管理切目录统一经 `AppState::send_terminal_cd`；检查备用屏，转义路径，先清行/擦回显再执行 cd。
- 终端列数由视口宽度驱动：`terminal_wrap` 开启时下限 20 列、长行在视口内折行；关闭时保留更宽终端（桌面 160、窄屏 80）供横向查看长行。
- 输入推断仅作有限兜底，不猜测任意脚本或别名；百分号路径解码仍需注意兼容性。
- 历史视口收到非空输入先恢复实时底部；双宽字符保留占位语义，显式 ANSI 色不被主题覆盖。
- 桌面粘贴优先[原生剪贴板](../crates/kt-ui/src/clipboard.rs)，失败再回退 WebView。
- [模型选区](../crates/kt-core/src/term/selection.rs)保存在 Alacritty 缓冲区，快照仅携带带实例归属的可见列区间。`terminal_selection.js` 捕获鼠标指针并将视口行、列、半格、`display_offset` 和 `revision` 经有序命令送入宿主/容器终端；输出或尺寸变化使旧坐标失效，前端等待新帧重采样，普通滚动不丢弃锚点。复制使用 `selection_to_string()`，不读取 DOM 字符串、不全局 trim 或替换 NBSP；保留软/硬换行、组合字符和宽字符语义。范围以仍保留的 scrollback 为边界，列数变化遵循引擎清除选区。终端目标切换往返须生成新 owner，旧请求只能清理自己的选区；快捷键、右键、手机菜单与原生 `copy` 事件共用路径，桌面优先原生剪贴板。
- gutter 内容列宽由 `terminal.rs` 的常量统一下发，`app.css` 共用变量计算内容宽度、左右留白、边线与正文起点；resize 继续扣除实际计算 padding。辅助 gutter 不可选且 `aria-hidden`。
- 桌面断线 Enter 复用 `AppState::reconnect_disconnected_session`，在状态锁内检查并切入连接中；认证/主机密钥确认阶段和重复/IME 按键不发起重连。`terminal_focus.js` 共享桌面终端与占位页的焦点恢复策略，避开输入框、编辑器、弹窗和 inert 区域；手机继续由输入桥管理焦点。

## 运维

依据：[请求与执行](../crates/kt-core/src/remote_ops.rs)、[运维 UI](../crates/kt-ui/src/components/operations.rs)、[回环测试](../crates/kt-core/tests/roundtrip.rs)。

- 使用固定命令和校验后的参数，经独立 exec 通道运行；输出与执行时间有上限，不污染宿主 PTY，不记录原始 payload。
- 类型化管理动作支持 sudo 挑战；当前协议不是 Prepare/Commit 两阶段协议，也没有进程启动身份校验来防止 PID 复用。
- 能力探测按连接懒加载；负面结果允许手动重试。proc/proc-net 可读性不等于已实现对应列表后端。
- 取消、断线、切换和重连收敛在途请求；失败保留最近成功快照，迟到结果不能覆盖新请求。
- [operations_state.rs](../crates/kt-ui/src/operations_state.rs) 按会话保存当前页面、父列表上下文及 `OperationId → OperationKey`；列表、详情、动作与监控分槽，结果只能回写本槽，不能改变页面。每领域保留当前一个详情，返回只清理离开的详情；动作成功标记相关列表待刷新，包含仍在途的旧列表。滚动与焦点桥只写父列表位置，清理时不读取新详情的滚动。
- Docker 清单约 10 秒、CPU/内存约 2 秒、容器大小初始 60 秒 TTL（慢查询最多延至 300 秒）。仅可见页面采集，runtime 不重叠，storage/image 共用单个慢查询且每批最多 64 个 ID；前台请求优先。失败按 2/4/8/16/30 秒退避，权限/能力/解析/输出超限/取消等等待显式重试。大小命令仅投影必要字段，容器存储与 image ID 缓存分开，删除清单清理缓存且拒绝迟到数据恢复已删除容器。镜像逻辑大小、可写层及 rootfs 不相加当作独占磁盘，CPU 允许超过 100%，缺失值用 `--` 并保留过期快照。
- Docker 容器终端独立拥有 SSH PTY、TermEngine 和 ExecId；宿主断线时回收。

## UI 与移动端

依据：[app.rs](../crates/kt-ui/src/components/app.rs)、[selector](../crates/kt-ui/src/components/app_logic.rs)、[设备判定](../crates/kt-ui/src/device.rs)、[键盘桥](../crates/kt-ui/src/components/phone_shell/keyboard.rs)。

- app 编排，state_controller 处理事件与副作用；展示组件优先接收轻量 selector。
- 手机以视口短边 600 CSS px 分界，两套 Shell 共用状态；Shell 渲染函数保持无 hook，局部 hook 下沉组件。
- 软键盘使用真实 textarea，IME 组合期间不清空输入；键序列复用终端映射，键盘高度由 device 写入统一 CSS 变量。
- 软键盘输入框用固定 DOM id 且桥接脚本常驻，目标会话每次渲染同步到本地 `Cell`：组件在会话切换时不重建，靠 `key` 无法改目标。
- 桌面固定紧凑布局；旧 density 字段由配置兼容读取并在保存时移除。
- 桌面侧栏默认 220px、可拖拽范围 176–720px；`sidebar_width_limit` 以主内容区、运维区和分隔条实测宽度计算上限，优先为终端保留 420px。`sidebar_layout.js` 观察尺寸并上报窗口失焦，切换设备或卸载时清理监听。文件表头与行共用六列（名称/修改时间/类型/大小/权限/所有者组）；`SftpColumns` 集中布局与归一化，`None` 自适应，手动宽度以 `AppSettings.sftp_column_widths` 原子保存且只更新最新配置中的该字段。窄侧栏在文件区域内部横滚，最小列宽 `[96,110,64,64,78,90]`、每列最大 2048 CSS px。Pointer/键盘调整、Esc/失焦取消及双击恢复自适应共用 `sftp_columns.js`，按实例注册清理以支持 DOM 已移除时卸载；界面不再提供独立恢复按钮，保存失败才显示状态行。长文本省略并提供 title；手机单列布局沿用原实现。
- [主题](../crates/kt-ui/src/components/theme.rs)与[样式](../crates/kt-ui/src/assets/app.css)使用变量；默认终端色随主题，显式颜色保留。
- 视觉共用规则集中于 app.css：设置、连接/安全弹窗、内置编辑器与通知共用按钮和表单表面；终端、文件 Grid 和运维保留紧凑尺寸。依赖其他 CSS 变量的强调色、菜单和悬停 token 在 `:root`、`.kt-window`、`.software-update-layer` 同时声明，确保在当前主题/强调色根节点重新计算，不能只依赖 :root 的已计算继承值。主要按钮复用 `accent_foreground` 计算的前景。
- 设置内容区以具名 container query 在 480px 以下堆叠表单，640px 以下桌面设置导航横向换行。手机普通输入（含 URL）保持 16px/44px；连接/安全弹窗随键盘 inset 收缩，正文滚动与底部操作区分离。全局可见焦点不覆盖终端输入桥，新增悬停仅用于精确指针，减少动态效果时禁用过渡。
- 跟随当前会话的组件状态用 `use_reactive(session_id)` 驱动，轮询另以代次收敛旧任务；Dioxus 的 `key` 只在条件渲染/迭代器产生的 Fragment 层比较，普通子节点位的 `key` 不触发重建。
- 界面字体族与界面字号缩放经内联 `--kt-ui-font` / `--kt-ui-scale` 注入 `.kt-window` 与 `.software-update-layer` 两个渲染根（更新弹层在 `.kt-window` 之外，两处必须用同一份拼装）；界面文字尺寸统一用 `--kt-text-10..22` 令牌（在这两个根与 `:root` 声明），终端区（`.terminal-surface`/`.terminal-gutter`/`.terminal-placeholder`）与手机 iOS 16px 输入保护保持原始 px。终端字号仍只由终端设置决定，不随界面缩放；新增界面文字不得写死 px，由 `kt-ui/src/style_contract.rs` 守卫测试约束。
- 界面选择器保存单个字体族，`fonts::css_ui_font_family` 以 CSS 字符串转义完整名称并附加 `var(--kt-ui-font-default)`；通用字体族保留关键字语义。终端继续使用独立的字体栈处理，不能用界面单字体序列化替换。界面保留原生抗锯齿与 `font-optical-sizing: auto`；浏览器 DPR 模拟只能证明匹配与布局，不能等同原生 Hinting、ClearType 或跨屏 DPI 的视觉验收。
- 手机输入的待验收场景见[当前任务](workflow/260820-mobile-phone-ui.md)。

## 配置同步与打包

依据：[同步实现](../crates/kt-sync/src/lib.rs)、[扫码](../crates/kt-ui/src/components/scanner.rs)、[移动打包契约](../crates/kt-app/tests/mobile_packaging_contract.rs)。

- WebDAV 采用完整 URL 和 ETag 条件写；LAN v2 使用 26 位秘密、HMAC 请求认证与 ChaCha20Poly1305 载荷加密。
- 导入经 Store 原子落盘后 ACK；连接、头缓冲、处理时间和重放缓存有界，失败可回滚，认证失败不全局销毁分享。
- 扫码使用限频灰度帧，卸载时释放摄像头；移动产物声明相机权限。
- [启动逻辑](../crates/kt-app/src/main.rs)隔离移动端命令行参数读取；[app.rs](../crates/kt-ui/src/components/app.rs)内嵌 CSS。
- Dioxus 0.7.10 的原生解释器遗漏了 `inert` 布尔属性，传入 `false` 会保留 `inert="false"` 并禁用整棵界面。根节点使用 `update_restarting().then_some(true)`：正常及准备失败后省略属性，准备更新重启时才设置。SSR 输出不能代替原生 DOM 属性添加与移除链路的验证。
- 桌面窗口无系统装饰；macOS 的 Cmd+C/V/A 由应用菜单派发 `copy:`/`paste:`/`selectAll:`，因此 `desktop_config` 必须在设置无装饰窗口之前注册 Edit 菜单，否则连接对话框、设置与内置编辑器的输入框都无法复制粘贴。
- [工具链](../rust-toolchain.toml)跟随 stable，[Cargo.lock](../Cargo.lock)记录依赖快照，升级显式更新锁文件。
- 发布、签名、构建号和 objcopy 的操作入口统一见 [README](../README.zh-CN.md#构建与发布)，具体配置以 workflow 和脚本为准。
- 桌面发布范围为 macOS ARM64 DMG、Windows AMD64 ZIP/安装器、Linux x64/aarch64 TAR.GZ；两条 workflow 的 JavaScript Actions 均使用 Node.js 24，Rust 工具链 Action 为 composite。

## 桌面更新

依据：[后台服务](../crates/kt-update/src/lib.rs)、[安装事务](../crates/kt-update/src/install.rs)、[辅助入口](../crates/kt-app/src/update_launcher.rs)、[更新界面](../crates/kt-ui/src/components/software_update.rs)。

- kt-app 仅在桌面分流内部辅助命令并注入 UpdateContext；GUI 持有单实例锁和安装锁，helper 锁阻止 GUI 介入替换。正常更新和恢复重启都先通过 PID/摘要握手，再退出旧进程。
- kt-update 自有两线程 Tokio runtime，检查和下载不占用 SSH 事件通道。状态互斥避免重复操作，取消任务收敛后才能启动下一任务。
- 当前构建嵌入完整 target、渠道、Cargo 版本、commit、GitHub run ID/attempt。正式版按 SemVer，Alpha 按 run ID/attempt 比较；本地缺失身份则开发构建禁用更新。
- 更新源固定为官方仓库，清单和包绑定 GitHub 资产 ID；缓存用大小/SHA-256 验证。updates/preferences.json 独立于 config.toml，不参与配置同步。
- 更新正文、标签与发布时间从同一次 Release 查询携带至 Available；正文按 UTF-8 边界限 64KiB。UI 用 pulldown-cmark 事件构建受控 Dioxus 节点，原始 HTML 显示为文字、图片仅保留替代文字、链接仅 HTTP/HTTPS；无日志不影响更新包。
- 检查取消为 Cancelled 中性终态，保留上次成功检查时间与失败计数并将自动检查延后 6 小时；手动重试仍可立即执行。下载取消保留 Available 以便重试。
- 更新展示位于 software_update/view.rs；点击“重启并安装”直接进入安全收尾，不再显示应用内确认页。SSH 断开和 UAC 提示在原卡片/通知中展示，准备时禁用重复操作并原位显示真实阶段。通知继续在主界面 inert 之外同步主题、强调色、字体和缩放。
- 更新通知与主窗口复用 `theme_class`；上游解析后的浅色名称为 `default-light`，不能单独比较 `light`。系统监听仍只在 App 创建，通知接收解析后的外观。
- ZIP/TAR 路径、重复项、链接、体积和二进制平台必须校验。归档路径在调用 Path::components 前拒绝原始反斜杠，避免 Windows 先拆分分隔符导致逐段校验漏检；统一只接受正斜杠归档路径。macOS 更新包使用 DMG，经 hdiutil 校验并只读挂载到 updates 下固定私有挂载点，仅复制 KitonyTerms.app，沿用路径/链接/体积限制；成功与失败均卸载，重试先回收遗留挂载。复制后以 codesign 验证签名和 bundle ID。ZIP 中的 AppleDouble 载体校验标识与体积后丢弃，不得作为普通资源加入签名包。
- 安装先写 journal，保留旧版本备份；替换后执行 `--update-info` 验证精确身份。只有匹配的 committed 事务在 GUI 启动确认后清理备份及提权/失败标记，普通启动不清除失败提权提示。失败由持有互斥的普通协调进程统一恢复，目标未改动时直接保留原版；恢复失败保留 journal 与备份。
- Windows 的新事务使用 `TransactionStorage::UserDirectory`，暂存与备份在原用户 updates 下；字段缺省按 Legacy 解析已有记录。`copy_file_synced` 用新建写句柄复制、刷盘再设置权限，不能用只读 File::open 刷盘。普通便携版直接替换，受保护便携版经 runas 请求 UAC；安装版经已登记目录的 Inno Setup。目标卷内临时复制后同卷替换，支持跨盘。只有目标写入权限错误可提权一次，32/33/1224 及写探测识别的共享冲突按占用处理，明确只读的目标不重试 UAC；主程序失败不改名让位，辅助副本可让位且恢复失败显式报告。
- Windows 授权集中于 install/elevation.rs：显式传递原用户事务目录与 journal 摘要，管理员入口重新校验自身、构建、目标、阶段和载荷/备份；拒绝 reparse point 并固定目录及输入文件句柄。管理员入口仅替换/恢复，结果绑定操作与摘要；普通进程继续负责 UI 重启。Shell 调用初始化 COM，保留 Win32 错误，区分取消、启动、等待、超时、非零退出；超时仍持有锁等待实际退出，不能并发回滚。
- 更新服务在创建 updates 后保存规范目录，journal 校验按规范化的暂存父目录比较，兼容 GUI 普通路径与 helper 扩展路径。Shell/安装器边界用 `windows_path::external` 安全简化盘符或 UNC 路径；不改变事务/摘要校验中的路径。无法安全转换或超过传统路径长度时明确失败；Inno Setup 固定 `/DIR` 并写入 `updates/update-install.log`，错误保留安装执行与安装后身份验证阶段。
- App 提供内置/外部编辑、SFTP pending、下载选择器及同步状态保护；准备期间根界面 inert，握手后再次检查，用户点击后取消运维并断开连接。Android/iOS 不渲染更新入口。
- 更新代理属于设备本机偏好（updates/preferences.json，不参与配置同步）：System 读环境变量代理、Direct 强制直连、HTTP/HTTPS/SOCKS5 显式代理只在 URL 中带用户名且密码留空（与 core 的 TCP 代理一致，代理凭证不落盘，需要密码时用 System 加环境变量凭证）。改代理先构建新客户端再落盘（失败不留内存更新），并取消在途检查/下载、以 `generation` 丢弃过期写回；启动读到非法代理时回退 System 并把原因写进快照 message，避免坏配置阻断启动。检查与下载共用同一客户端，失败按 407 / 超时 / 连接 / 重定向 / 状态分类后再附底层原因。
