# KitonyTerms：Dioxus/Rust → Wails v3 / Go / React 最终迁移方案

基准日期：2026-10-30。范围包括 Windows、macOS、Linux、Android、iOS 的整个应用。共享工程为 `app/`，Wails CLI、Go 模块和前端 runtime 固定为 v3.0.0；移动构建入口已具备，但五平台最终验收尚未全部完成。

2026-11-10用户授权项目Rust退出并上传GitHub打包。当前唯一生产工程为`app/`的Go/Wails/React，项目Cargo/Dioxus/crates及旧Rust互通工具已退役，系统Rust不卸载。完整命令见[app/README.md](../app/README.md)；v0.1.18已由GitHub原生CI打包并发布为独立预发布，27项实际下载核验完成；v0.1.17稳定版保持。

当前平台范围（2026-11-10，优先于下方历史检查点）：Android仅保留ARM64，x86/x86_64构建、打包和运行验收撤销；NDK宿主工具架构独立处理。macOS最低15.0，停止12–14兼容；compile、Darwin任务、plist、候选Mach-O校验与Docker模板同步声明15.0，旧候选不重写。iOS本轮仅维护非App Store的无签名设备IPA配置，不以开发者账号、正式签名分发或iOS文件功能作为门槛；完整Xcode/iPhoneOS SDK仍是实际设备链接前提，设备安装另需侧载签名。

本机可执行结果与跳过原因集中在P5矩阵。Windows amd64为ARM64客体内x64仿真，Intel macOS未运行；模拟器/仿真和注入操作均不等同原生硬件验收。正式数据升级、系统认证、实体输入和性能仍开放，Rust退出不代表这些验收完成。

## 1. 最终架构

```text
React / TypeScript / xterm.js
  ├─ 桌面工作台与手机导航
  ├─ Radix 对话框、文件编辑、运维、同步、相机
  └─ Wails 生成绑定、局部输入状态、可取消请求
                       ↓
app/internal/ipc       生命周期、系统选择器、类型化方法与通知
                       ↓
  model       DTO、配置模型、校验
  storage     TOML、known_hosts、KTVT vault、原子提交与版本冲突
  session     SSH、ProxyJump、代理、PTY、输出重放、SFTP、exec
  syncconfig  旧版 envelope、WebDAV ETag、LAN v2
  lifecycle   退出请求归属、取消/确认与清理完成后放行
  qrcode      纯 Go QR 编解码与有界灰度帧
  platform    私有目录与平台边界
                       ↓
Wails 平台宿主 / Go SSH 与 SFTP / 系统原生适配
```

Go 领域层不依赖 React，也不以 Wails 事件充当持久化事实源。前端只维护快照和临时交互；Go 负责配置、vault、信任、连接生命周期与提交结果。原生功能通过 Go/cgo、Java/JNI 或 Objective-C 实现；Rust FFI、WASM、sidecar 不计作 Rust 退出。

旧`app/tools/interop`及`interop.sh`已退出；公开TOML/vault、同步envelope和更新task/journal冻结为Go测试夹具，当前检查不调用Rust。下方带日期的旧Rust验证、候选和入口描述均为历史检查点，以本节当前状态为准。

## 2. 不变契约

| 领域 | 契约 |
|---|---|
| 数据 | 沿用TOML、已知主机及KTVT v1；开发数据独立，发行身份使用旧ProjectDirs目录与锁；所有回归只使用临时根，真实旧数据接管仍待验收 |
| 状态 | `ConfigSnapshot.revision` 为十进制字符串，避免 JS 大整数精度损失；保存比较期望版本，失败不更新内存 |
| 会话 | 每次连接具有 `id + generation`；认证答案、文件结果和终端请求不得跨代次回写 |
| 事件 | `config:changed` 携带快照，`sessions:changed` 触发重新查询；恢复前台与定时查询弥补事件遗漏 |
| 认证 | 主机指纹必须经当前挑战确认且信任落盘成功；变更密钥拒绝连接 |
| 机密 | 密码/口令只在局部输入和 Go 认证/vault 内短暂使用，不进全局 store、通知、日志或同步载荷 |
| 更新 | 保持归档校验、构建身份、journal、备份、握手、替换/回滚及启动确认语义，通过原生验收后接管 |
| 桌面布局 | 以原 Rust/Dioxus 为基准：36px标题栏/会话标签、默认220px侧栏内连接与SFTP、中央终端、48px运维工具栏/抽屉、58px底栏；新增能力复用原入口 |
| 移动 | 真实 textarea/IME，safe-area 与键盘高度单一归属；相机/监听/请求在关闭与后台释放；真机单独验收 |

## 3. 按阶段实施与验收

### P0：工程与契约

受影响文件：`app/go.mod`、`app/frontend/package.json`、`app/main.go`、`app/native_*.go`、`app/build/`、`app/internal/model/`。

已完成共享Go模块、React工程、Wails生命周期与绑定生成。移动文件检查点（2026-11-03）为322 packages /1 service /76 methods /65 models /1 typed event，47前端测试。开发目录独立，Android debug使用`com.kitonyterms.app.wailsdev`。

验收：`bash app/scripts/check.sh` 校验三处版本、编译脚本Bash语法、绑定漂移、Go与前端检查。最近完整统一门禁已通过；本轮新增入口的定向检查与编译结果如下。

编译优先入口（2026-11-10）：新增 `app/scripts/compile.sh` 与 `wails3 task compile`，只读使用锁定依赖，默认宿主desktop，显式MODE=server才构建无GUI服务；原生前置条件缺失时明确失败。macOS ARM64原生桌面、macOS amd64/Windows两架构桌面交叉编译、Linux两架构server共6分支实际通过；仅macOS ARM64执行只读身份命令且无业务数据，其余只编译。11项命令行为检查、Go格式/vet、绑定无漂移及前端47测试通过，本轮未重跑全量Go race。业务源码/依赖/布局/原候选保持，正式分发及原生GUI验收继续按P5/P6处理。

### P1：配置、vault 与信任

受影响文件：`app/internal/model/config.go`、`app/internal/storage/`、`app/internal/platform/`。

配置使用严格 TOML 解码、归一化、数字字符串版本和原子提交；主机信任必须成功持久化。vault 保持 KTVT v1、Argon2id 和 XChaCha20-Poly1305；先尝试安装密钥，再尝试受支持的旧固定密钥。旧 vault 迁移前保存原始加密备份，无法解密或备份冲突时保留原件并报错。

验收：存储race覆盖版本冲突、写失败内存回滚、未知字段、信任持久化/变化拒绝、vault篡改/写失败与旧格式迁移。历史真实Rust回读证据保留；当前由公开冻结夹具和Go回写回归验证，不再运行`interop.sh`。完整旧数据目录接管与升级回滚仍待P5。

已批准计划的M1补差（2026-11-04）：[独立目录工具](../app/tools/data-snapshot/main.go) 基于 [datasnapshot](../app/internal/datasnapshot/snapshot.go) 接受显式绝对config/data，源应用及已有更新锁必须可取得，不创建源锁。全字节快照含vault/key、known_hosts、更新材料、未知文件、空目录及未上传副本；限制8GiB/50000节点/64层/16MiB清单，拒绝链接/特殊文件/不安全名称及归一化冲突。源重验、目标SHA-256回读与刷盘后才发布清单；恢复只写全新目录，失败材料保留，不覆盖后来新增的数据。Unix0700/0600；Windows写内容前设置并回读受保护当前用户DACL。

本机race覆盖锁竞争/缺失/替换、损坏/额外/缺失条目、取消/源变化、路径重叠/已有目标、链接/体积/深度/清单类型及错误隐私，另验证完成清单发布前取消。公开Rust目录夹具经恢复、Go配置/vault回写后，实际旧Rust读取通过；13项真实CLI进程检查验证系统锁竞争、完整字节/空目录/可执行位、源保持、拒绝覆盖与篡改预检。2026-11-09在Windows 11 ARM64实际通过2项DACL拒绝/子项继承及10项适用快照测试，该客体夹具未执行Rust回读；Linux仍仅编译。`pendingUpdate`仅是材料存在标记；更新绝对路径原样保留而不执行，清单不证明来源或更新恢复语义，原位置恢复/正式旧包升级与生产接管仍在P5/P6。

### P2：SSH、终端与文件

受影响文件：`app/internal/session/{manager,dial,jump,agent_*,output,files,transfer,file_actions,download_batch,file_context}.go`、`app/internal/ipc/{backend,files}.go`。

已实现密码、私钥/口令、交互认证、agent 接口、单跳 ProxyJump、TCP 代理、PTY、输入序号去重与恢复。终端字节经 Base64 跨 IPC，每块不超过 32768 字节；最近输出有界保留 1 MiB，未 ACK 的输出施加背压，历史截断显式返回 reset。前端须在 xterm 解析完成后 ACK。

SSH config 采用严格解析和多个 IdentityFile，自动发现的缺失身份不阻断密码；显式私钥错误保留。HostName 展开前固定 vault ID，避免别名展开改变密码索引。

已实现目录读取、1 MiB UTF-8 编辑/摘要冲突检测、同目录临时提交与权限保留、新建目录、无覆盖重命名、递归删除、目录批次上传下载。批次先预检（50000项/64层），拒绝链接、特殊文件与名称冲突，保留空目录。上传默认拒绝同名，显式覆盖才合并目录/替换普通文件并保留目标权限；传输期间检查源文件与目标状态。下载保持不覆盖，失败保留已完成文件。

传输任务由Go按会话保存，最多保留32条，每会话一个在途任务；可查询状态、当前文件、文件数和十进制字节进度，显式取消。未知总量为空字符串，成功前不显示100%。文件锁、初次SFTP子系统/版本协商均可取消；本地管道解除读等待，正常远端只回收文件通道，拒绝关闭的恶意远端必要时断开SSH。取消后使用独立限时通道回收本次临时文件，失败明确保留位置。

验收：真实TCP SSH/SFTP回环race覆盖目录上传/下载、空目录、默认冲突拒绝、显式覆盖/权限、任务查询与代次/限流/取消、协商卡住取消后重开、取消临时清理。当前通过。

Android移动文件由应用自有JNI/SAF适配接入原上传/下载入口：私有暂存0700/文件0600、16请求、1GiB/50000项/64层，URI只在Java内存；不新增存储权限、不持久化授权。保留目录根名/空目录，拒绝虚拟文档、类型/名称/归一化冲突。移动私有批次以O_EXCL写入/刷盘/关闭，整批成功后才导出；桌面仍使用原硬链接不覆盖提交。任务exporting覆盖真实外部写入和SHA-256回读，释放暂存后才终态，取消/解绑等待实际worker/描述符，onStop不取消选择器。已有外部目标/自动改名明确拒绝，仅清理本次未完成且归属已确认的文档；iOS文件适配仍未实现。

P2本机专项（2026-11-03）：真实TCP停读写覆盖SFTP OpenChannel/子系统/版本与直连/跳板写背压，复现并修复ProxyJump内层Close卡住。跳板由当前会话独占，关闭内层先断开跳板传输；拨号取消等待实际后台操作完成。跳板开通道/内层版本握手的Drain、隔离系统ssh-agent认证/转发签名/只向目标转发，以及挂起agent socket清理均通过。定向race连续两轮34.306秒，统一session race50.359秒；全部internal/绑定/47项前端门禁及当前macOS/server构建通过，前端资产不变。Windows交叉vet与Windows amd64/Linux amd64/arm64 session测试包仅编译；Windows原生命名管道及原生GUI仍待对应环境。

已实现Bash/Zsh提示符hook、OSC7目录、受限200条内存命令历史、原历史面板/菜单及剪贴板路径；历史仅宿主SSH采集，初始化/提示符门控，排除密码、前导空格、备用屏、控制编辑/补全与超限输入。目录自动跟随服从开关，使用实际OSC7；内部cd清行/转义且不进历史，备用屏自动跳过、手动提示。真实shell/SSH race及浏览器会话隔离、清空迟到、刷新恢复通过。

外部编辑由Go管理私有副本（目录0700/文件0600），支持32MiB以内普通二进制文件，最多16个活动/32条总记录；按sessionId+generation+editId隔离。合并本机探测、环境与既有编辑器配置；先解析模板再替换文件参数，不经shell。元数据稳定后读入并校验，支持上传一次后结束或后续保存自动上传；远端摘要冲突拒绝覆盖，上传失败停止自动重试，保存期间的新修改继续待处理，显式结束/断线/退出保留未上传副本。只清理本记录与空私有目录，关闭保留记录不删除其副本。移动端继续内置编辑器。

`external-edit-final-check.log`统一门禁与7项外部编辑race/1项真实SSH二进制回环通过；最后状态文案修正经`external-edit-frontend-final.log`全前端门禁通过。真实SSH浏览器与实际受控编辑器进程验证菜单/打开方式、一次与连续自动保存、原子替换、旧代次拒绝、取消退出继续监听、结束保留与关闭记录不删除，临时配置已恢复。用户桌面编辑器GUI和系统选择器仍需验收。停滞远端与agent的本机专项见上述P2检查点，不代替任意网络/Windows/原生环境验收。

### P3：桌面与手机界面

受影响文件：`app/frontend/src/{App,style}`、`components/{TerminalView,FileBrowser,Dialogs,SettingsPanel,SyncPanel,Scanner}`、`lib/`。

已实现连接CRUD、多会话终端、按会话保留文件编辑器、dirty关闭保护、文件右键菜单/多选/传输进度、设置版本冲突、类型化运维/取消/sudo弹窗、同步与相机界面。桌面恢复原基础分栏，文件六列共用宽度，分隔条可拖动或键盘调整；手机保持底部导航。退出保护、200px历史面板、58px底栏五组监控及原设置分类均已接线。

设置与语言检查点（2026-11-01）：桌面760px弹窗/168px分类栏、手机列表→详情；字段基线串行合并最新配置，有限版本重试，同字段冲突与失败保留草稿，明确重载/覆盖/重试。根节点与Portal统一单个UI字体/85–140%缩放/强调色，终端字号独立。xterm公共缓冲/marker实现本地有界行号/时间戳和字面关键词背景，不污染ANSI或复制。前端应用文案使用显式中英资源，远端输出/文件名/用户内容不翻译。

`settings-final-check.log`统一门禁通过：64方法/54模型、26前端测试、JS659.23kB；`settings-interop.log`真实Rust/Go互通通过。`settings-final-evidence.json`区分首版与最终产物：真实保存并发/冲突/单次失败重试、特殊中文字体/140%缩放/Portal、编辑器、终端复制/备用屏/裁剪、412px手机布局及最终英文文件/运维/监控/退出、快照更新与语言切换不重复请求通过；配置和临时替身已恢复。最终server/官方macOS构建通过，36/220/48/58px、无溢出。

系统字体目录及原位选择器已实现：macOS CoreText、Windows GDI、Linux有界Fontconfig，移动端七项预设；macOS无cgo/枚举失败明确回退。未知已保存单族名与终端栈完整保留，名单仅设置局部持有。当前统一门禁47项前端测试、全部internal race与绑定漂移通过；实际CoreText252族/无cgo七项回退，20个隔离HTTP浏览器场景覆盖保存/重试/迟到/特殊名/语言与手机控件，36/220/48/58px和设置760px/168px无改变。Windows/Linux字体测试包仅编译，未在目标运行；原生观感仍待实机。最终server与官方macOS构建通过，JS694.51kB，单chunk告警保留。

尚未完成：实体键盘/系统IME与终端剪贴板实机、其余原生桌面交互、手机选区/键盘行为和真实硬件验收。macOS进程内Cocoa输入、菜单/Cmd+Q/performClose、输入框系统剪贴板及远程未保存保护已补验；注入方式和11月3日实际选择器/字体证据分别见P5。更新分类已接线查询/下载/安装和本机偏好，开发构建保持禁用；安装实现与验收边界见P4。

### P4：运维、同步、原生能力与更新

受影响文件：`app/internal/syncconfig/`、`app/internal/qrcode/`、`app/internal/ipc/{sync,qr}.go`、平台宿主。

同步已实现旧 `kitonyterms.config` v1 envelope（1 MiB），WebDAV 无重定向/HTTPS 凭据/ETag 条件写，LAN v2 HMAC + ChaCha20-Poly1305、26 位配对码、重放拒绝和提交后 ACK。分享失败不破坏原配置，接收保存失败不消费分享。

QR 已使用纯 Go 编解码；相机帧为 16..480 px 的有界灰度图，界面串行采样并限频。Android 桥接不记录原始 IPC、控制台内容、URL/查询参数，错误只保留阶段/类别。最终 APK 的合成敏感标记未出现在 logcat。

验收：历史真实Rust↔Go LAN持久化/ACK及WebDAV条件更新/旧ETag冲突通过；当前冻结旧envelope并保留Go协议与持久化回归，不再调用Rust。QR roundtrip/畸形帧测试保留。
运维已实现固定命令白名单与参数校验、服务/进程/网络列表和详情、Docker五种资源列表/详情/日志/手动用量、服务/进程/Docker管理动作、连接内能力缓存/重新探测、一次性sudo挑战及取消。进程只读comm，Docker/服务详情限制投影；输出最多8MiB/错误64KiB，请求/通道协商/挑战均可取消，取消接口等待回收。失败保留最近成功列表，列表/详情分槽，App按会话缓存筛选和位置；分页每屏100条。真实SSH race和浏览器服务分页/筛选/详情/sudo/取消及Docker/进程/网络交互通过。

容器终端已实现独立SSH PTY/ExecId/1MiB字节流和有序输入，每宿主一个；固定`docker exec -it <校验ID> /bin/sh`，抽屉隐藏保留，空ExecId仅供只读发现，整页刷新恢复，显式关闭等待回收且不再发现，宿主断线回收。Docker仅在可见列表采集：按当前筛选/100项分页选取大小目标，清单10秒/runtime2秒，storage/image共用慢槽且批次≤64，大小TTL60–300秒，失败2/4/8/16/30秒退避或等待显式重试；前台请求先等待后台取消，失败保留快照，镜像/可写层/rootfs与未知/已知零分开。手动清单成功记录同一新鲜期，避免立即重复查询。

容器/采集检查点统一门禁通过（当时48方法/44模型、前端14项）；真实SSH race覆盖容器输出/输入重放、状态42、背压/协商取消、发现/关闭/宿主回收。HTTP浏览器实测中文输入/尺寸回读、隐藏重开/完整刷新恢复同一ExecId、关闭不恢复/新建新ExecId、自然结束和宿主关闭；慢采集在手动刷新前取消，隐藏后活跃查询归零且计数稳定，手动刷新仅一次清单查询。1024×768外层尺寸无改变。桌面验收使用仅允许同源嵌入的临时响应转发，不修改产品CSP，不代表原生或安全策略验收。


退出保护由`internal/lifecycle.Controller`管理当前一次请求；Wails `ShouldQuit`/`WindowClosing`与标题栏共用，重复关闭复用ID，旧请求不能取消/批准新请求，并发确认只启动一次清理。前端现有Modal结合未保存文件、Go活动快照与分享状态提示；确认后拒绝新业务调用、停止分享与外部编辑监听、等待Manager shutdown，再放行退出。退出检查点当时生成52方法/46模型，race/14项前端测试/构建通过；后续已纳入外部编辑活动与未上传副本保留。浏览器已验证未保存中文内容警告、取消保留编辑/连接/容器、确认后server实际退出和活动归零、取消后再次关闭的竞态恢复。macOS当前原生补验范围与限制见P5，不以server退出替代。

剪贴板服务显式检查原生读写成功标志，失败回退WebView。异步粘贴绑定终端对象、活动代次、会话/容器身份、可见性及退出状态；1MiB按UTF-8字节校验。58方法最终server浏览器故障注入验证正常/回退粘贴各一次、复制零输入、双失败提示、超限拒绝、切换离开再回来及卸载后迟到结果丢弃。测试接口在finally恢复，未读写系统剪贴板；不代表原生剪贴板验收。`shell-clipboard-final-check.log`统一检查通过，最终官方macOS/server构建通过，布局36/220/48/58px、历史200px且无横向溢出。

监控由`internal/monitor`解析固定七段命令，`session/monitor.go`在现有SSH路径上使用独立持久非交互`sh`：每轮先测带序号心跳（4秒上限），超时为未知，再采样资源（12秒上限），成功后间隔2秒。单行64KiB、整帧1MiB、stderr64KiB；不污染宿主PTY或历史。CPU/每核和网络按差分计算，首次、计数器回退及网卡增删时速率未知；guest不重复累计，整数溢出拒绝，缺失字段不伪造零。字节/速率/运行秒数用十进制字符串。每宿主最多一个采集任务，最多8个独立订阅，5秒租期；可见底栏每秒续期，切换/隐藏/卸载释放本订阅，失联客户端到期回收，断线/退出收尾。失败按2–30秒退避并保留最近成功快照，重建通道同时重建差分基线。

监控检查点`monitor-final-check.log`统一门禁通过（64方法/54模型，前端16测试，JS628.06kB）；4项纯解析和5项会话监控测试含真实SSH，覆盖心跳超时/旧序号、慢采样、共享归属、失败保留、租期、取消、协商和输出上限。真实SSH浏览器验证五组指标、902ms慢采样时1ms心跳、失败过期标记、隐藏零查询/零采集、迟到旧响应丢弃、未知无障碍语义、整页刷新恢复，以及退出后server实际结束/活动归零。最终资产`index-BwhbYz2a.js`与36/220/48/58px、无横向溢出复验通过；临时替身均恢复。最终server和官方macOS产物已重建，证据`monitor-final-evidence.json`。这些测试不替代原生GUI、真实生产远端/代理TUN或移动验收。

系统认证查看凭据已接线：`internal/osauth`不缓存成功且禁止并发；macOS每次独立LAContext，Windows窗口绑定Hello，明确不可用才回退，显式账户入口经实际LogonUserW和当前用户SID匹配。server/Linux/移动端拒绝查看。`internal/secretview`在读取vault前后复核取消、退出和完整配置身份，存在性仅查询索引。原连接弹窗局部只读显示与新密码分离，30秒/隐藏/关闭/编辑/目标变化清除，迟到结果拒绝。

`secret-final-check.log`统一门禁通过：313包/66方法/56模型、29前端测试，Go认证/读取边界含vault等待期间变化的race通过；JS665.28kB。`secret-view-evidence.json`区分真实server拒绝查看与合成认证响应：取消/目标往返/关闭重开/隐藏后迟到丢弃、失败一次、查看后保存零凭据写入、实际约30.9秒隐藏和英文提示通过。最终server与macOS官方构建通过（scratch `KitonyTerms-Secret-Final` / `KitonyTerms-Secret-Native-Final`），未执行系统认证弹窗；Windows同步账户对话框需实际返回才释放认证门禁，取消请求只保证丢弃结果。

桌面更新查询/代理/下载检查点（2026-11-02）：固定官方仓库与资产ID，严格schema1/渠道/target/commit/run/attempt/大小/SHA-256，元数据/包/时间/跳转均有界。System/Direct/HTTP/HTTPS/SOCKS5共用查询下载客户端，显式代理仅保存用户名且密码为空；设备本机preferences不进入config或同步，原子保存失败保持旧任务/内存，成功切换等待旧任务回收且保留服务器限流时间。状态序号与偏好版本独立，开发构建禁用，下载完成仍`installAvailable=false`。

原设置更新分类使用局部草稿、显式冲突恢复/重试和关闭保护；仅可见分类轮询状态，隐藏停止查询并拒绝迟到结果。发行说明纯文本，完成前进度上限99%。`update-ui-evidence.json`区分真实服务保存/配置隔离和合成下载响应，覆盖分类/语言切换保留、失败/冲突、保存中关闭、隐藏零查询/单链恢复、纯文本/进度及412px表单。首版取消重复提交已修复，最终产物同轮双击仅1次请求；36/220/48/58px无溢出，基线配置/偏好及临时替身均恢复。

[归档准备](../app/internal/update/archive.go)已实现ZIP/TAR的原始路径/Unicode及大小写冲突/链接/特殊文件/CRC与gzip尾/数量深度展开限额。macOS使用真实hdiutil只读挂载，复制指定应用包和有界包内相对链接，检查codesign、bundle ID与精确`--update-info`。真实临时签名DMG的成功及构建/签名/越界链接失败清理已通过race；`/var`与`/private/var`比较问题已修正。[安装记录解析](../app/internal/update/journal.go)兼容Rust字段及legacy/user_directory，非法或未完成事务不自动覆盖。

`update-archive-final-check.log`统一门禁通过，更新领域28项顶层测试、34前端测试，JS `index-C6rd9buo.js` 680.71kB。最终server与官方macOS构建通过，`update-archive-build-evidence.json`记录摘要、真实只读构建命令及拒绝安装辅助入口；Windows amd64/Linux amd64/arm64共3个更新领域测试包仅编译，未在目标系统执行。临时应用签名与镜像测试不等同正式分发签名或升级验收。

[安装引擎](../app/internal/update/install.go)与[内部helper](../app/internal/update/install_helper.go)已实现持锁的三阶段journal、备份、目标卷校验替换、回滚及函数级显式确认；macOS签名应用包原子交换。helper校验自身/任务/旧目标/包和真实子PID+摘要，ready后仍须旧进程退出及有效commit、无abort，已有journal阻止新任务；失败由持锁协调器恢复，重启前再验旧摘要。新增17项安装/协议顶层测试，总更新领域45项；六个故障边界、四个真实进程崩溃点、锁退出释放、损坏/取消/旧字段兼容及临时签名DMG整包安装/资源链接回滚/确认通过。`update-install-final-check.log`完整门禁通过（更新包106.199秒），319包/71方法/64模型、34前端测试与页面资产不变；60秒旧总预算超时记录保留，总预算已改180秒，操作短超时仍在。当前server/macOS与三目标测试包构建和摘要见`update-install-build-evidence.json`，目标测试包未执行。

[Windows内部适配](../app/internal/update/install_elevation_windows.go)已实现HKLM注册目录的Inno执行、固定DIR/日志及禁止自行关闭/重启，受限replace/restore绑定原用户目录和journal原文摘要，固定目录/输入及启动文件、拒绝reparse/hardlink；管理员不运行新载荷或重启GUI。只有目标权限失败且旧摘要不变才提权一次，取消/超时仍等实际子进程退出后由普通持锁进程恢复。普通/扩展Windows路径两侧按句柄规范化，旧Rust注册版staged摘要兼容保留。新增7项本机回归后updater52项顶层测试、完整统一门禁通过（112.310秒）；Windows专用新增5项仅编译，三目标测试包未执行。`update-windows-build-evidence.json`记录55源码摘要、server/macOS/三目标产物及10次真实只读/拒绝命令检查；前端资产与绑定不变，本轮未重跑浏览器或原生GUI。

[恢复重启](../app/internal/update/install_relaunch.go)与[桌面启动守卫](../app/internal/update/desktop.go)已接线：严格兼容Rust四字段记录，绑定固定helper/旧构建/摘要及journal原文；共用PID握手、父进程退出+commit/abort，持锁复验，普通权限恢复/重开。GUI前取得应用锁和安装锁，拒绝活跃/外来/损坏记录，服务关闭后才释放；正常committed保留备份，React在Info/Config/初始Sessions成功并渲染后显式确认，journal变化/取消保留材料。正式身份helper CLI已分流，开发构建仍拒绝；产品安装接线见下文。

恢复启动检查点完整统一门禁通过（updater177.748秒，本机64项顶层测试），72方法/64模型/34前端测试；当前server/macOS与三目标测试包构建通过，Windows/Linux测试二进制未执行。`update-relaunch-build-evidence.json`记录65源码摘要、6产物及实际CLI检查，额外正式身份测试夹具验证真实分流/握手和拒绝，未启动GUI或替换安装。`update-relaunch-ui-evidence.json`实际隔离浏览器验证加载完成后单次确认、初始失败/延迟不确认、失败不自动重试、移动标记/离开页面0次及36/220/48/58px无溢出；JS681.29kB。测试转发首轮Host遗漏导致Forbidden，修正后重测，产品策略未改。

[安装发现](../app/internal/update/installation.go)与[产品IPC](../app/internal/ipc/update_install.go)已接线：更新服务绑定Desktop锁owner，实际Windows注册状态选择安装器或便携包，准备前重查目录/权限。Prepare→前端第二次安全检查→Commit/Abort按同一请求ID推进；退出控制器原子计数在途文件/保存/同步/认证操作，Manager.Drain等待真实worker回收并允许失败后重新连接。最终commit成功与原生退出许可原子绑定，准备失败不永久关闭业务服务。仅满足安装条件的downloaded候选动态开放`installAvailable`。

原更新卡片提供“重启并安装”和Windows授权说明，无二次安装确认。草稿/文件/对话框保护只登记布尔元数据；准备期间主界面与Portal禁用，取消/隐藏/卸载等精确ID及真实回收。未知提交结果保持等待，不重复提交或取消；刷新未提交意图精确回收，刷新提交失败后重载初始后端数据。普通退出在原生选择器/保存/同步结束前不能确认。

安装接线统一门禁通过：updater本机78项顶层测试、196.368秒，75方法/64模型/44前端测试；最后仅前端恢复修正另经全前端门禁，最终JS692.53kB。`update-installation-product-e2e-evidence.json`记录真实产品helper在隔离签名.app副本上取消/提交/等待父进程退出、DMG替换、新server重开，旧备份/journal在真实页面确认前保留、确认后清理。测试使用公开正式身份夹具及临时ad-hoc签名，非发布或正式旧版升级。`update-installation-ui-evidence.json`的20场景区分真实页面确认与合成协议故障，36/220/48/58px无溢出。

当前server/官方macOS构建及Windows交叉vet通过；三个Windows/Linux updater测试包只编译未执行。系统认证原生交互、原生GUI安装/退出、Windows安装器/UAC/ACL/占用和正式签名/旧版升级验收仍未完成。生产Rust入口保留，不能以内部测试、合成UI或目标编译替代平台验收。

Windows Inno候选准备（2026-11-04）：[独立工具](../app/tools/windows-package/main.go) 与 [显式任务](../app/build/windows/Taskfile.yml) 已接线prepare/verify/compile，保留原正式Rust workflow。真实amd64/GUI PE及Go production目标静态检查后，生成仅EXE的便携ZIP与单独安装标记，沿用AppId、管理员/旧32位登记模式、禁止自动关闭重启及静默不自启。模板当前仅英文，正式语言/资源/WebView2/签名与安装升级仍待Windows验收。

声明身份固定写为declared，静态暂存返回runtimeIdentityVerified=false；Go trimpath不保留ldflags，未把静态读取误计为运行身份。Windows compile将实际查询载荷--update-info并精确比较身份，再调用显式ISCC，成功才生成compiled.json；本机不会执行该路径。实际Wails PE已交叉构建并在macOS通过7项回归（另1项子进程助手）和8项真实任务/CLI/独立ZIP检查，Windows工具/测试包编译通过，目标未执行。最终`plan-inno-full-check.log/.exit=0`统一Go/全部internal与tools race/绑定/47前端测试门禁通过，界面源码和资产保持；未生成setup.exe，不能替代Windows安装器、HKLM/UAC/原生升级或正式发布验收。

Windows候选版本资源（2026-11-09）：[候选构建](../app/tools/windows-package/candidate.go) 以公开BuildInfo单一输入，固定winres v0.3.1/Wails v3.0.0；旧产品ICO的独立副本、en-US/zh-CN版本字段、原asInvoker/uiAccess=false/CommonControls6/DPI回退XML一起写入实际PE。数字版本各段≤65535，完整构建号/attempt/commit以字符串保留。初次Go overlay因链接器仍打开原syso物理路径而失败，失败材料保留；现使用 [有界私有源码副本](../app/tools/windows-package/candidate_inputs.go)，64MiB/5000项/64层/单文件16MiB，拒绝链接和输出重叠，不复制测试/开发数据/node_modules/Git、不写原工程syso。

实际0.1.18/build603公开候选PE为18,720,768字节，157份隔离输入/10项资源；构建后原输入与副本SHA、图标/版本/manifest回读一致才写成功记录。17项工具顶层race测试（含1项助手）无跳过，13项独立PE解析/真实CLI/原ICO字节/ZIP/隔离与篡改检查通过，资源目录RVA/尺寸绑定及重复段地址在资源库读取前拒绝；当前Inno暂存保持声明身份，Windows载荷/ISCC未执行、没有setup.exe。最终统一Go/internal+tools race/绑定/47前端测试及构建通过，布局/资产保持，Windows全模块vet及工具/测试包仅编译，macOS完整应用编译和只读身份零数据创建通过。静态资源不证明实际运行身份、来源或正式签名，原生GUI/WebView2/HKLM/UAC/中文安装语言/签名升级仍需对应环境。

macOS候选打包（2026-11-09）：[独立工具](../app/tools/macos-package/main.go) 和显式Darwin任务以公开BuildInfo单一输入，复用 [共享有界复制器](../app/internal/buildinputs/inputs.go)，生成全新私有.app/DMG。保留旧应用路径/Bundle ID和独立旧ICNS；plist产品字段/布尔类型修正，三段版本与完整构建身份分开。实际Mach-O ARM64/12.0声明/SDK26.5.0和Go production/cgo/trimpath/Wails v3.0.0通过；加载命令、符号和重定位表在标准库读取前有界校验。

实际公开0.1.18/build604候选含157份复制输入/8项产物摘要，可执行文件16,396,096字节、DMG 7,772,926字节。本地ad-hoc strict签名、直接和镜像内--update-info完整身份/零业务数据创建、DMG实际只读挂载/四文件回读/卸载已通过；取消/失败同样等待清理，外来目录不接管。共享复制器/macOS/Windows工具共28项顶层race无跳过（2+9+17），新的Windows候选158输入/10资源复验通过，Windows工具/测试包仍只编译。未启动业务GUI、未安装/正式签名/公证/发布；macOS12实际兼容和完整旧版升级仍开放。

本批最终统一Go/vet/全部internal+tools race、绑定漂移及47项前端测试/构建通过，资产保持；另15项独立plistlib/Mach-O/系统签名/真实CLI/只读镜像检查通过。Go对象13.0链接12.0警告保留，不能把部署声明当作最低系统兼容证据。新冻结`macos-candidate-frozen-source.json`/`macos-candidate-final-evidence.json`记录349份输入/8份编译产物及实际DMG；82份历史证据和用户保留材料逐项保持，未重写旧冻结或切换生产入口。

Windows实际执行补验（2026-11-09）：恢复既有Parallels Windows 11 ARM64 VM（10.0.26200.0），以当前用户执行隔离夹具，无Go/ISCC且未安装工具或改变权限/共享设置。ARM64实际通过DACL拒绝/子项继承2项、GDI字体7项、适用快照10项和最终更新专项8通过/1跳过。amd64完整更新包在x64仿真下78通过/5跳过/0失败，另1个stage_link子项跳过；账户缺少链接权限1314及Unix权限夹具均明确跳过，不将其计为安全分支验收或原生amd64硬件证据。

真实失败定位并修复：目录/日志固定句柄必须请求读取才能约束删除共享；无扩展名payload用显式绝对应用路径直接启动，防止改选邻近.exe，保留有界输出/取消及实际进程与管道等待；映像退出后的暂存回收复查SameFile并限时等待拒绝/占用，替换的短暂拒绝也在原3秒预算内等待后再判断提权，永久只读/取消保持材料。中间完整包失败、20轮未重现的诊断及最终通过分别保留，不把诊断通过当作已稳定复现。

最新源码重建公开测试候选Windows0.1.18/build609和macOS/build610，分别161/160份复制输入。Windows实际完整--update-info/零数据、中文空格路径prepare/verify、仅EXE的ZIP/安装标记、已有输出/缺少ISCC拒绝通过；主机和客体独立生成5份暂存材料摘要一致。静态候选/prepare仍保留declared及runtimeIdentityVerified=false，运行证据独立存放；没有setup.exe。macOS本地ad-hoc签名、完整身份/零数据及实际只读镜像回读/卸载通过。最新统一Go/vet/internal+tools race、绑定漂移与前端格式/类型/lint/47测试/build通过；布局/资产不变。旧349输入冻结不改写，本批平台适配与新证据另存；本阶段不关闭正式签名/升级、系统认证、最低系统、数据接管/恢复/性能及P5/P6。

Windows agent补验（2026-11-09）：[命名管道接入](../app/internal/session/agent_windows.go)固定go-winio v0.6.4/x/sys v0.49.0，Wails仍v3.0.0；10秒有界DialPipeContext/net.Conn复用成熟库的I/O、期限与Close。Manager私有连接器在构造时确定，产品默认OpenSSH管道不变，无新增IPC、环境覆盖或全局开关。随机当前用户管道/生成密钥夹具复用认证、转发签名、仅向跳板目标转发及挂起请求Drain；最终ARM64原生与amd64仿真各10项顶层/18子项无跳过通过，另DACL 2项通过。SFTP恢复夹具首次误用Windows本机盘符路径，修正为远端工作目录后两架构通过；旧失败保留。

依赖升级后的完整amd64更新器仍为78通过/5跳过/0失败，另stage_link子项跳过；统一Go/vet/internal+tools race、绑定/前端47测试/build通过，之后仅上述测试路径修正另经macOS定向race与Windows重编译/实际复测。新公开测试候选Windows611/macOS612分别161/160份复制输入：Windows完整身份/零数据、中文空格暂存/ZIP/标记/拒绝及5份主客体摘要一致；macOS本地ad-hoc签名、真实只读DMG回读/卸载通过。前端/布局不变，旧352输入与609/610证据保留；本轮另存windows-agent-frozen-source.json/windows-agent-final-evidence.json。私有协议夹具不证明Windows系统OpenSSH服务互通，也不关闭原生amd64硬件、认证、安装升级或P5/P6。

Rust/Go更新恢复历史互通（2026-11-09）：旧工具`app/tools/interop/src/update_records.rs`接入真实kt-update记录/恢复/锁，原集成测试仅运行归属夹具，实际6顶层/26子项通过。旧工具已退役；当前[Go兼容测试](../app/internal/update/interop_test.go)读取其真实冻结的legacy/user_directory/旧缺省字段。历史恢复/跨进程锁证据及Go恢复/helper回归保留，不能将历史计数当成本轮Rust执行。

这些是普通文件夹具和本机恢复引擎/进程证据，user_directory布局在macOS验证不等同Windows原生执行；没有正式旧包覆盖升级、bundle交换/签名、GUI启动确认、安装器/UAC或完整跨语言helper握手结论。task与准备载荷的最终回收仍属helper协调。最新统一Go/vet/internal+tools race、绑定漂移与前端47测试/build通过；首轮长TMPDIR造成agent夹具socket路径超限，改回默认临时目录重跑成功，原日志保留。Windows611/macOS612的161/160份生产输入逐字节保持，既有候选及354冻结不改写；本阶段另冻385份输入，含Go测试、互通工具及其本仓库Rust依赖源码/构建脚本，2份实际执行与3份仅编译产物分别记录。

原Rust helper历史补验（2026-11-09）：旧`app/tools/interop/src/update_helper.rs`直接编译原launcher/单实例锁，限制新归属夹具。Go真实spawnApplyHelper通过2顶层/5子项的就绪、存活父进程等待、abort和拒绝；仅为Go发起→Rust helper取消方向，不代表完整成功安装或正式旧包升级。该Rust工具现已退役，历史输入和产物保持。

该补验的完整`interop.sh`在macOS ARM64实际通过8顶层/31子项（6/26记录恢复+2/5原helper），Rust fmt/clippy/test/locked构建及原launcher/单实例锁10项单元测试与配置/vault/完整目录/LAN/WebDAV一起通过；补齐独立工具开发依赖后的最终统一Go/vet/internal+tools race、绑定漂移及前端47测试/build通过。新冻结389输入含两份直接编译的原Rust源码，与最终门禁前摘要相同，385历史冻结不改写；Windows611/macOS612生产输入、原桌面布局/前端资产与用户保留材料保持。

### P5：平台与升级验收

| 平台/领域 | 当前证据 | 尚缺 |
|---|---|---|
| macOS ARM64，最低15.0 | 11月10日production/cgo整应用构建通过；新614候选.app/DMG的15.0声明、SDK26.5、ad-hoc签名、完整身份/零业务数据和只读镜像回读通过。宿主27.0的实际WKWebView通过Info/Config/Sessions IPC、252系统字体、设置保存/版本变化/恢复、新建连接表单、1200×800无溢出及产品确认退出0 | 当前15/26系统未执行；实体输入/系统IME/终端剪贴板、系统认证、正式签名/升级及长连接/性能仍需独立验收。测试通过固定原生探针和DOM动作，不算实体输入 |
| macOS Intel，最低15.0 | 当前整应用交叉编译和Mach-O minos15.0回读通过 | 本机`arch -x86_64 /usr/bin/true`返回Bad CPU type，Rosetta不可用；跳过目标身份和GUI运行，不要求恢复12–14支持 |
| Windows ARM64 / amd64 | 当前两架构整应用PE构建、实际--update-info、各2字体/6存储/4私有SSH agent顶层及11 agent子项通过，无失败/跳过；ARM64为原生客体，amd64为x64仿真。ARM64另确认实际窗口和WebView2进程/Environment created successfully | WebView2调试端点超时，GUI表单/IPC未验收；不将创建窗口计为页面功能通过。原生amd64硬件、DPI/系统OpenSSH/Hello/ISCC/UAC、链接权限分支、正式升级及性能仍开放 |
| Linux amd64/arm64 | 既有显式production,server/CGO=0两架构交叉编译保留；本轮跳过原生桌面构建与运行 | 无Linux原生CGO/sysroot/pkg-config、Docker/Podman及GUI环境；历史无cgo桌面构建失败仍保留，server不替代桌面验收 |
| Android ARM64 | 当前APK 28,671,268字节、AAB 15,540,913字节构建通过，均仅arm64-v8a；v2测试签名/ZIP16KiB/四段LOAD0x4000和包内字节通过。离线lint 0错误/20警告，单元任务NO-SOURCE。API35只读模拟器13项通过：冷启动、当前资产/真实Info/Config/Sessions IPC、412px布局/字体、配置保存恢复、连接表单、16px/44px设置、HOME/BACK、同PID恢复、安装摘要和force-stop | 本轮未重复历史SAF/相机全套，API36、真机/更多provider/后台SSH/IME/低内存/实体扫码、正式签名与旧版升级仍待验收 |
| Android x86/x86_64 | 已撤销支持；task明确拒绝、Gradle ABI过滤、旧fat/all-archs名仅为ARM64兼容别名 | 不再安排构建或运行，不作为阻塞 |
| iOS设备IPA配置 | 新`ios:package:ipa`/`scripts/ios_ipa.py`独立构建号、真机arm64/平台/plist/资源/无签名Payload及回读入口；26项合成结构/拒绝检查通过；实际task在创建输出前明确拒绝缺失SDK | 当前仅CommandLineTools，无iPhoneOS SDK，真实链接/实际应用IPA未生成；合成IPA不可运行。后续需完整Xcode执行构建和侧载签名，App Store/开发者账号及iOS文件不作为本轮门槛 |
| Rust/Go更新记录与恢复 | 历史macOS ARM64真实kt-update/Go引擎6顶层26子项及原Rust helper取消方向2顶层5子项保留 | 成功安装与完整双向helper接力、正式旧包升级、Windows/Linux跨语言执行仍未关闭 |

2026-11-10 GitHub发布实证：工作流 `42807234257` / attempt 1 对应提交 `d3e8f686814c55cb97ff95e005f2c6dacdebe31f`，11项任务全部成功；`v0.1.18` Release ID `440301750` 为 prerelease，Latest仍是`v0.1.17`。从Release实际下载的27项（117,274,203字节）与GitHub资产摘要、大小、`SHA256SUMS`、更新清单和本次构建身份逐项匹配。Linux/Windows/macOS六个桌面目标均在原生runner打包与测试；两份macOS DMG只读挂载检查、签名、15.0最低系统和包资源验证通过并卸载。Android ARM64 APK/AAB签名及同证书、16 KiB对齐通过；无签名iOS arm64设备IPA在真实Xcode runner链接并通过包身份检查。稳定`v0.1.17`的12项资产及发布元数据未变，`main`/旧标签引用保持。原生CI和包检查不证明正式旧版升级/生产数据接管、系统认证、实体输入、真机运行、最低macOS 15系统运行或性能长稳；这些验收仍开放。

本轮统一`check.sh`退出0：Go格式/vet、全部internal/tools race（update199.860秒、session50.640秒）、614候选实际测试、绑定无漂移、前端格式/类型/lint/47测试/build。最后iOS资源/带空格SDK路径修正另经Python语法、26项结构检查和实际SDK拒绝复验；Docker五段shell仅语法通过，未执行容器构建。原桌面布局、前端/绑定/依赖与当前Go/Wails生产入口保持。

问题记录：Windows首轮GUI PE退出码读取、未引用PowerShell测试参数及缺少Rust测试夹具属于驱动问题，修正后身份/领域测试通过；Android首轮固定300ms visibility断言改为系统前台状态核对后通过。macOS空AX树不能证明白屏；原生WKWebView探针实际确认页面正常，早期premain环境修改及重复导入runtime曾干扰驱动，改为不修改环境和现有IPC后页面/确认退出通过。各首轮失败及纠正后结果独立保留于scratch `platform-policy-20261110/`；没有据此改写历史证据或声称全部平台通过。

macOS原生补验（2026-11-03）：当前ARM64 raw产物16491842字节/SHA-256 `076b4dd9da59bff47773f6eb85e471a7de1b53a326a07b7b1c5dbb76838267c1`，独立Bundle ID `com.kitonyterms.app.wailsdev.macos`；ad-hoc签名后可执行文件摘要`f0a04c86806a0e9b016babc8d4fe76be90d1b08e107bf5bbee6171f7c36da341`。CoreGraphics/AX实际窗口1199×799，原设置和文件入口可用；插件枚举失败后使用仅匹配本轮实例的临时AX助手。真实NSOpenPanel配合回环Go SSH/内存SFTP夹具，完成非BMP文件/目录/空目录上传下载、实际文件摘要、同名目标拒绝和在途选择器退出保护；不计NSSavePanel、生产OpenSSH或长连接稳定性。原生字体下拉253项，Menlo持久化后恢复默认。空闲及一条活动会话均经产品确认实际退出并释放三锁，活动轮154ms。

AX赋值与合成键盘没有形成有效编辑，剪贴板复制/粘贴匹配失败，最后两轮返回原剪贴板已恢复；不计键盘、原生未保存保护或独立Cmd+Q/菜单派发。系统WindowClosing hook和系统认证未执行，未确认产品缺陷，源码/布局/依赖均未改。临时夹具停机worker等待超时，网络释放后准确强制清理，不能与产品正常退出混为一项。315份产品输入与上一完整门禁全部相同，完整Go/绑定/47前端门禁结果沿用该精确源码，非本轮重跑。隔离配置/私钥/信任记录和实例已回收，证据为scratch `desktop-native-final-evidence.json`；正式签名、升级、其它原生平台及P5/P6继续开放。

macOS进程内事件补验（2026-11-04）：在上述同一未改产品包中加载scratch内临时Objective-C库，私有0700控制目录/0600 Unix socket只接受固定操作名，无任意脚本、终端输入或系统认证命令。显式激活并核对key window/first responder后，NSApp.sendEvent输入ASCII、NSTextInputClient输入中文/非BMP，WebView可信input事件及值匹配。原生Edit菜单SelectAll/Copy/Paste往返匹配，剪贴板全部项目/类型/数据在有界内存备份后逐项恢复，原内容未输出或落盘。实体键盘、系统IME、自动初始焦点和终端复制仍未验收。

新公钥SSH/内存SFTP夹具仅监听127.0.0.1:19273，固定原文件入口打开已知文本。原生菜单Cmd+Q、NSApp Cmd+Q事件和正常NSWindow.performClose分别产生远程未保存警告；三轮取消均保留草稿且远端摘要不变。原保存按钮实际提交，独立SFTP回读25字节/SHA-256 `a5b684b325296a67564a94a836ade91cf251bd50ce8a3642ab73686eca8acb79`，保存后退出警告消失。活动产品实例确认退出约159ms、三锁重获；新夹具正常退出/worker全部结束，旧夹具强制清理历史保持。后台关闭弹窗激活后才可观察、测试库早期修改环境导致Go初始化失败、空配置夹具失败等原始记录保留，未确认为产品缺陷。315份产品输入/包摘要保持，完整门禁沿用精确源码结果而未重跑；配置原缺失状态/剪贴板/私钥/信任/socket已恢复或清理。新证据为scratch `native-events-final-evidence.json`，与11月3日报告分开冻结；P5/P6及生产Rust入口状态不变。

Android当前复验（2026-11-03）：修复模板应用名、根页面BACK默认销毁Activity导致同进程重开失败，改用OnBackPressedDispatcher并在根任务moveTaskToBack(false)；未使用的模板定位明确unavailable，不申请新权限，相机为可选硬件。最终APK SHA-256为 `d4570e41c01e092460bcd5bad77260b205b6e25186a375b9110551f1227fcb3b`，MAIN/LAUNCHER冷启动1118ms。lintDebug/assembleDebug通过，0错误/21警告；单元测试任务NO-SOURCE。16个最终实际场景覆盖移动IPC/字体/更新限制、412px布局、相机系统拒绝、迟到授权/关闭/后台轨道释放、恢复暂停与重开、原生BACK历史/根任务保持同一PID、恢复后IPC及系统force-stop。force-stop不代表优雅退出，模拟器相机视频帧不代表实体扫码。首轮脚本失败与根BACK复现保留，最终ADB转发和read-only/no-snapshot模拟器均已退出；不代替移动实机或正式签名验收。

Android进程/Activity分离（2026-11-03）：WailsApplication持有一次初始化的桥接，Activity销毁释放WebView/监听/原生UI而不关闭Go服务；旧页面与异步回调按归属隔离，不持久化用户输入。当前APK SHA-256为 `dd53e303dc231e3042f21ecf670fd4b39eafcb6756d30ee1ad0646bcfec701d4`；独立产品lint/打包通过，0错误/21警告，单元测试任务仍NO-SOURCE；两ABI共享库与上一检查点相同。scratch临时instrumentation两轮各28项通过，实际recreate/finish/reopen保持同PID、同桥接和真实127.0.0.1分享；原生Question/OpenFile在销毁时实际取消并释放等待中的Go请求。当前APK另通过16项页面/相机/返回回归，以及两轮am start -W -R 3的8项检查。always_finish_activities尝试未建立系统自动销毁证据，失败结果保留且设置恢复；上述通过不证明内存压力/进程死亡、GC或全部JNI竞态。模拟器/转发已清理，无正式签名/实体扫码/真机结论。

Android后台进程冷恢复（2026-11-03）：同一APK在新的ktdbg只读实例执行两轮HOME→am kill→启动，20项实际检查通过。旧PID确实消失，新PID/新页面/新后端只初始化一次；已保存的完整配置保持、revision重新生成；原分享/保护请求和未提交连接草稿不恢复，移动Info/Fonts/Config IPC可用。只调整独立调试配置的外观值供持久化核对，最后完整恢复，开发设置从未改动。准确forward、debug进程和只读模拟器已清理，不保存userdata/快照。系统命令杀进程不是低内存系统行为或后台SSH持续性证明；移动导入/批量导出仍为明确拒绝的待实现适配。

Android文件阶段（2026-11-03）：当前APK SHA-256 `d761c68be6359f0f66a036da02f67cf38ab077ccbc3fd5082630cf5aa665da05`，73357024字节；设备安装包一致，双ABI各4段LOAD0x4000、ZIP16KiB、v2单签名与无新增存储权限通过，未包含测试provider/认证资产。API35原生验收修复Root.Chmod触发fchmodat2/seccomp崩溃和暂存硬链接拒绝，分别改为目录句柄权限与独占暂存写入；另修复单文件未通过校验就纳入删除清理，并拒绝自动改名。

当前文件APK通过21项真实系统批次检查及9项CREATE_DOCUMENT/空目录检查，真实SSH和外部文件摘要相同，目标冲突/自动改名/选择器取消及Activity销毁后清理通过。另20项合成跨进程provider与注入选择结果覆盖大小写/NFC/非法名/超限/虚拟文档拒绝、已有非空文档不删除、可靠管道实际关闭与原SSH保持；这些不是任意云provider或系统UI选择该夹具的证据。共享门禁322包/76方法/65模型、47前端测试、全部internal race及绑定漂移通过；当前原生产品lint/assemble通过，0错误/21警告，单元任务NO-SOURCE。x86_64未执行、正式签名/真机/后台SSH/IME/低内存与完整P5/P6继续开放。

此前仅修改Java异常日志类别后的APK已完成编译/覆盖安装/进程启动、16KiB ZIP对齐与v2调试签名校验；当时CDP未就绪，未重复页面/合成标记验收。该历史APK的SHA-256为 `29b4bffb6e5e085a40192ea4d535deaa5ae963dcb8dfab1c66b4bdc364fa756f`，不代表本轮新增传输与布局源码已重新打包。

Android 固定 SDK36 / NDK28.2.13676358 / Gradle8.13 / JDK21；发布构建要求正式签名与 `MOBILE_BUILD_NUMBER`，当前仅ARM64。安装使用保留数据的覆盖方式，不以卸载应用清理验收失败。iOS设备IPA使用独立无签名入口，构建号1–2100000000，缺少SDK提前失败；设备安装/侧载签名与归档生成分开验收。

### P6：退出 Rust 与切换正式入口

2026-11-10用户新增授权后，项目构建入口切换和Rust退出独立推进；不把尚缺的实机验收描述为通过。

1. 当前生产源码/检查/CI为Go/Wails v3.0.0/React，六桌面目标、Android ARM64及无签名设备IPA；项目无Rust FFI/WASM/sidecar。
2. Cargo/Dioxus/crates/旧Rust验证工具移出当前工程；历史tag和本地冻结保持，公开格式夹具由Go回归。
3. 发行版沿用旧ProjectDirs与锁，开发和显式临时根隔离；真实生产数据接管、旧包覆盖升级与回滚仍待实机演练。
4. v0.1.18版本与构建身份、27项资产集合、大小/SHA和最低系统已由发布链及实际下载核验；结果见本节2026-11-10发布实证，保留既有稳定版。
5. P5实体输入、系统认证、真机、最低系统、长期连接与性能继续开放，不因移除Rust而关闭。

## 4. 运行与复核入口

```bash
# 仓库根目录；wails3 已在 PATH 中
bash app/scripts/check.sh
python3 -B .github/scripts/test-update-manifest.py

# 仅重新生成绑定
cd app
wails3 generate bindings -ts -i
```

关键测试：`internal/storage/{storage,interop}_test.go`、`internal/session/{output,roundtrip,file_actions,file_context,sftp_open,upload_batch,ssh_config,shell,edit_files}_test.go`、`internal/externaledit/manager_test.go`、`internal/shellintegration/shell_test.go`、`internal/syncconfig/{sync,interop}_test.go`、`internal/qrcode/qr_test.go`、`frontend/src/lib/*.test.ts`。

本次未执行 Git 写操作、外部发布或正式数据接管。既有发布任务文档中的用户改动继续保留。
