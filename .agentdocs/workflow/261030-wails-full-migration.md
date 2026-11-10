# 全平台 Wails 迁移

## 目标与授权

用户明确选择全应用Go/Wails/React迁移。2026-11-10追加授权继续直到项目Rust退出并上传GitHub打包，允许本任务提交/推送dev和独立v0.1.18预发布；保留v0.1.17稳定版、main、旧tag、用户发布文档及未上传副本。下方各带日期阶段的旧权限/入口说明为历史状态，当前以[退出发布任务](done/261110-remove-rust-github-package.md)为准。

最终方案与功能/平台矩阵：[docs/dioxus-to-wails-v3.md](../../docs/dioxus-to-wails-v3.md)。运行、构建与复核入口：[app/README.md](../../app/README.md)。

## 技术边界

- 固定 Wails v3.0.0；移动端实验能力不能以编译替代原生验收。
- Go 领域层不依赖 Wails；Go 管理持久化。前端快照携带十进制字符串版本，事件可用查询恢复。
- 兼容TOML、KTVT v1 vault、known_hosts；开发数据独立，发行身份沿用旧ProjectDirs与锁，回归只用临时根，不试写已安装数据。
- 终端字节有序、有界、可重读；主机密钥必须确认当前挑战且成功保存。
- 同步不含 vault/key/known_hosts；接收先持久化再 ACK；桌面更新事务通过原生兼容验收后才能接管。
- 项目Cargo/Dioxus/crates与旧`app/tools/interop`已退出；公开冻结夹具继续验证Go兼容性，历史跨语言证据保留。
- SSH config 自动身份候选不存在时不阻断其它认证；显式私钥错误必须返回；别名展开前固定 vault ID。
- 用户新增约束：桌面端页面布局以迁移前 Rust/Dioxus 为基准，导航、侧栏、面板位置/尺寸及工作区层级保持一致；迁移版现有简化页面不作为重新设计依据。新增功能沿用原面板、菜单与弹窗。

## 当前优先级：先通过目标平台编译（2026-11-09）

用户追加要求先保证目标平台编译，允许其他平台使用桩实现或条件编译，暂缓其交互和兼容性验收。用户未另行指定平台，本轮按宿主及默认构建目标 macOS ARM64 推进；这不关闭全平台迁移或 P5/P6。

- [x] 核对默认 Taskfile、production/cgo 参数及固定 Wails 3.0.0；CLI 不在常规 PATH，使用已有私有 CLI 的命令级 PATH，无需安装工具。
- [x] 实际构建前端生产资源与 macOS ARM64 原生可执行文件，保留 12.0 部署目标及只读模块依赖；无代码编译错误，无需新增平台桩。没有调用会额外 tidy/install/重建图标的默认依赖任务。
- [x] Go 格式、production 标签全模块 vet、生成绑定无漂移、前端格式/类型/lint/47 项现有测试通过；实际 ARM64 成品只读 --update-info 退出0、无业务数据创建，未启动 GUI。
- [x] README 补充可复现编译命令、环境限制与临时处理方式；保留已有候选产物和历史冻结。

新输出为会话 scratch 的 `compile-first-macos-arm64-20261109/kitonyterms`，16,491,842 字节，SHA-256 `04334b57610b6faf6638480977e988f3019c52588f1f2a8edc774bb48f177aa1`；同目录 build-results/check-results/verification JSON 与逐项日志记录实际结果。此为无正式发布身份的 development/0.1.17/build0，production 标签不等于正式发行。

现阶段保留重复 objc、Go 对象13.0/链接目标12.0警告，后续补最低系统运行证据。其他平台先保留现有条件分支：Linux 原生 CGO 工具链与 Docker 缺失、iOS 无完整 Xcode/SDK，不能靠业务桩补齐原生链接环境；待环境具备后再做对应原生编译。已知历史 Linux CGO=0 桌面编译失败不改写为成功，本轮不扩展其它平台行为验收。

## 可重复编译入口阶段（2026-11-10）

沿用编译优先要求，将上轮临时命令收敛为工程内脚本与 Wails task。默认宿主桌面；macOS/Linux desktop 要求原生 CGO，Windows desktop 沿用 CGO=0；server 必须显式选择，原生前置条件不满足时不自动降级。固定 Wails 3.0.0 与只读模块，不安装依赖或工具，不变更打包/签名/正式入口。

- [x] 实现 `app/scripts/compile.sh` 与 `compile` task，复用生成绑定/前端生产构建与既有平台参数；task 环境变量保持输出参数字面值，`check.sh` 纳入 Bash 语法检查。
- [x] macOS ARM64 桌面原生编译、macOS amd64/Windows amd64与ARM64桌面交叉编译、Linux amd64/arm64显式server模式全部通过。仅macOS ARM64执行只读身份命令，未启动GUI，其余五份只编译。
- [x] 11项命令行为通过：help、非法平台/架构/模式/多余参数、无Linux原生环境、CLI/Go模块版本拒绝、注入前端/Go编译错误传播及task字面参数。Go格式/production vet、绑定无漂移和前端格式/类型/lint/47测试通过；本轮未重跑全量Go race。
- [x] 更新 README/平台矩阵，复核锁定输入、用户保留材料与旧候选；整体迁移仍保留开放。

本轮独立记录位于scratch `compile-entry-20261110/`：matrix-progress/artifacts、additional-progress/additional-artifacts、checks、failure-path-results与逐项日志/exit。macOS ARM64字节及摘要与11月9日原生编译一致（16,491,842字节/04334b57610b6faf6638480977e988f3019c52588f1f2a8edc774bb48f177aa1），其余产物身份均为开发构建；Windows普通编译不注入候选版本资源。只读身份查询无业务数据，业务源码/生成绑定/依赖版本/原候选保持，原冻结不改写。

Linux server使用已有Wails显式编译标签作为无GUI占位，不能关闭Linux原生桌面CGO问题；iOS SDK/文件、实体输入、系统认证、正式签名/旧包升级/数据接管、稳定性与P5/P6仍待各自条件。本轮只执行编译、成品回读和定向检查，未启动VM、模拟器、业务GUI或新的网络服务。

## 当前 Android 源码编译刷新（2026-11-10）

桌面入口完成后继续刷新当前依赖下的Android产物。只在全新私有副本重建overlay/双ABI，不覆盖原APK/JNI库或历史证据；沿用SDK36/NDK28.2.13676358/私有JDK21与Gradle8.13缓存，离线构建。调试签名使用本轮新建测试密钥，不读取用户keystore，不启动或安装应用。

- [x] 复制195份/2,282,065字节源码、原生和前端构建输入；排除缓存、旧JNI、测试provider与原绝对路径overlay。
- [x] 在副本生成overlay并实际编译ARM64/x86_64共享库，使用android,debug/cgo/只读模块。
- [x] 离线lintDebug/testDebugUnitTest/assembleDebug成功，48个Gradle任务实际执行；lint为0错误/20警告，单元任务NO-SOURCE。APK包名/独立调试签名、双ABI16KiB对齐与包内库字节一致通过。
- [x] 195份原/副本输入、原APK/JNI/overlay、候选与用户保留项摘要复验通过；本轮临时调试keystore、签名init与错误注入脚本已删除。新包仅构建，未沿用旧APK的设备验收结论。

新APK为scratch `compile-entry-20261110/kitonyterms-current-debug.apk`，52,320,404字节，SHA-256 `235b5752cfff30a1f3b8a358de738ab787cf2a7438e620825b0265f830ad8d46`；独立包名com.kitonyterms.app.wailsdev/0.1.17-wailsdev。apksigner实际验证v2/单签名及本轮测试证书；zipalign通过16KiB，两个ELF各4段LOAD为0x4000，包内库与编译源字节完全一致。原生依赖仅liblog/libdl/libc系统库，新旧APK均863条目且名称集合相同，无存储权限或测试provider。

`android-build-progress.json`/`android-verification.json`/`android-package-content-comparison.json`/`cleanup-and-review.json`及逐项日志记录本轮结果。此为195份当前构建输入的新包，双ABI均未安装/运行；11月3日旧APK的SAF/生命周期设备证据继续独立保留。11月10日复核仍无Docker及iPhoneOS/Simulator SDK，Linux原生GUI和iOS链接继续阻塞；正式签名、旧包升级、实体输入及整体P5/P6未关闭。

## 后续实施顺序

- [x] SFTP 目录上传、显式覆盖策略、可查询进度与协商取消；取消后临时文件限时清理，回环 race 通过。
- [x] 类型化运维、能力探测、sudo、独立容器终端与可见页面采集调度。
  - 固定管理请求、能力缓存/重试、一次性sudo、输出上限与等待回收已验证；容器PTY按代次/ExecId独立寻址，抽屉隐藏保留、整页恢复、显式关闭及宿主断线回收。Docker按当前筛选分页采集，快慢分槽、64项批次、TTL/退避、手动抢占、隐藏取消和失败保留快照已接线。
- [ ] 原生密码查看、全部平台文件/剪贴板与更新事务兼容。macOS文件选择往返已通过；进程内Cocoa菜单/Cmd+Q/performClose、输入框剪贴板及远程未保存保护已补验，实体输入与其它平台原生GUI仍待执行。
- [ ] 按原桌面布局接线，继续移动与平台验收；通过全部门槛后执行 P6。

## 阶段与 TODO

- [x] P0：共享 app/、工具链固定、DTO、生成绑定和独立开发目录。
- [x] P1 已实现：严格配置/版本冲突、失败回滚、主机信任、vault 与旧格式备份迁移；Rust/Go TOML/vault 回写通过。
- [x] P2 已实现：SSH/PTTY/ProxyJump、agent 接口、有界输出重放、输入恢复去重、SFTP 文本/文件动作/目录批次上传下载、显式上传覆盖、可查询进度和取消；协商/临时清理回环 race 通过。
- [x] P2 shell：OSC7目录/提示符集成、受限内存历史、原面板/菜单及有归属的剪贴板粘贴已实现，race与真实SSH浏览器通过。
- [x] P2 外部编辑：私有副本/保存检测、一次与自动回传、冲突/失败保留、显式结束与退出保护已接线；race与真实SSH浏览器通过。
- [x] P2 本机专项：完全停滞SFTP/真实TCP写背压、单跳取消与独立系统agent认证/转发/回收通过；修复跳板停读时内层Close卡住。
- [ ] P2 剩余：Windows系统OpenSSH服务互通、真实桌面编辑器GUI与其余平台系统选择器验收；私有管道agent已在ARM64原生/amd64仿真通过。拒绝通道关闭的远端必要时断开 SSH 回收，不能承诺任意恶意对端下始终保留连接。
- [x] P3 已实现：原桌面基础分栏已接线（36px 标题栏含会话标签、默认220px侧栏内上下连接/SFTP、中央终端、48px运维工具栏/抽屉、58px底栏）；手机导航、多会话终端、文件编辑保留、右键菜单、设置冲突与同步/扫码界面。
- [ ] P3 剩余：实体键盘/系统IME、终端剪贴板及其它平台原生GUI、手机选区/键盘完整行为；macOS进程内Cocoa输入/菜单剪贴板/三种关闭路由和未保存保护见11月4日证据。设置分类/外观/终端选项、中英资源及系统字体枚举/原位选择器已实现；Windows GDI 7项已实际通过，Linux原生枚举与各端字体观感仍待对应环境，更新分类已接线，开发构建禁用。
- [x] P4 已实现：WebDAV ETag、LAN v2 与真实双向 Rust/Go 互通、纯 Go QR；Android 日志脱敏；原58px底栏五组监控与独立SSH心跳。
- [ ] P4 剩余：系统认证原生GUI、桌面更新正式签名/旧版升级及Windows授权/占用验收。认证与安装发现/准备/提交/取消/恢复已实现，本机领域和隔离产品事务验证通过，不能以合成响应和编译替代系统验收。
- [ ] P5：全部平台原生打包、性能、正式签名、旧版覆盖升级/数据接管与回滚、移动真机验收。
- [x] P6：源码/生产入口与Rust依赖退出完成；完整回归、GitHub原生多平台打包及v0.1.18实际27项下载核验见[退出发布任务](done/261110-remove-rust-github-package.md)。正式升级/数据接管、系统认证、实体交互、真机和性能继续开放。

## 当前验证证据（更新至2026-11-02）

- SFTP/布局检查点 `continued-final-check.log` 全部通过：CLI/Go/runtime同为v3.0.0；gofmt、vet、internal race、绑定漂移、前端format/typecheck/lint/8项测试/build。当时生成绑定38 methods /29 models /1 typed event。
- 运维检查点 `operations-check.log`：统一门禁通过，绑定42 methods /41 models，前端10项测试、生产JS约596kB；真实SSH race覆盖能力缓存、EOF后迟到失败、分块sudo提示/一次性答案及凭据不回显、输出超限、协商取消/超时与SFTP继续可用。
- 运维实际浏览器：126服务按100/26分页、本地筛选、详情返回及关闭重开保留筛选；目标确认、sudo提交后动作回读、取消挑战；Docker容器/镜像/Compose、234.5%CPU与内存、进程和网络列表通过。1024×768下展开抽屉仍为360px/48px栏/420px终端，关闭后侧栏220px、SFTP320px、标题36px/底栏58px，无页面溢出。最初分步sudo验收等待未收敛，直接提交当前挑战可结束；紧凑时序完整表单复测只提交一次且成功，后台与单测均通过，未把首轮计为成功。
  当时使用的19246运维server与19250夹具已停止；旧SFTP夹具19249仍为本次独立测试服务。该检查点不代表原生桌面或移动验收。
- 容器/采集最终检查点 `container-final-check.log`：统一门禁通过，绑定48 methods /44 models /1 typed event；前端14项测试，生产JS606.86kB。真实SSH race覆盖独立二进制/中文输入、顺序去重与重放、EOF后状态42、协商取消/输出背压、SFTP继续可用、宿主断线回收，以及空ExecId只读发现/自然结束恢复/显式关闭不再发现。
- 当前源码server+真实SSH浏览器：CPU234.5%/内存、可写层0B/rootfs/镜像逻辑大小独立展示；xterm中文输入及27×41尺寸回读；抽屉隐藏重开/完整页面刷新均恢复同一ExecId和输入序号，显式关闭后刷新不再恢复，新启动使用新ExecId，自然退出状态42及宿主关闭回收通过。慢采集在手动刷新前取消；隐藏后全部查询active=0，连续2.5秒计数不变；最终手动刷新窗口仅一次清单查询。
- 1024×768同源HTTP验收视口：标题36px/底栏58px/栏48px；抽屉360px、侧栏191px，关闭后侧栏220px，页面无横向溢出。本地测试转发仅将响应CSP的frame-src改为self以允许嵌入，产品CSP/外层布局源码未改；该证据不代表原生WebView或安全策略验收。当前server19246、SSH19251、无载荷计数19252/临时转发19253均使用session scratch与独立开发数据。
- 退出保护检查点 `quit-final-check.log`：统一门禁通过，生成52 methods /46 models；退出控制器64路并发/单次清理/旧ID隔离、真实SSH活动统计与shutdown归零race通过。最后一次仅前端取消竞态修正另经 `quit-frontend-final.log` 完整format/typecheck/lint/14测试/build通过，JS609.41kB。
- 原生接线共用Controller：Wails `ShouldQuit`/`WindowClosing`与标题栏都创建同一请求；取消不动会话，确认后拒绝新业务调用、停止分享、等待Manager回收再放行。浏览器实测真实未保存中文文件提示、取消保留内容/连接/同一容器PTY、确认后server进程实际退出且夹具全部active=0；最终构建受控延迟复测“取消到后端后再次关闭”恢复新请求ID。退出组件复用Modal，外层CSS未改，最终36/220/48/58px与无溢出复验通过。
- 退出检查点当时源码官方darwin:build及server构建通过，二进制位于scratch `KitonyTerms-Quit-Native-Final`；保留macOS13对象/12目标链接警告。未启动该轮原生GUI验收；computer-use技能明确禁止自动操作终端应用，不能把server退出或编译计为系统关闭/菜单退出验收。
- shell/历史最终检查点 `shell-clipboard-final-check.log`：统一门禁全部通过，309包/1服务/58方法/48模型/1事件；前端14测试，JS617.97kB。真实Bash标量/数组、Zsh hook顺序、过滤恢复、有界解析、隐私/Unicode/序号与真实SSH目录/历史回环race通过。
- 真实SSH浏览器：bootstrap隐藏且保留登录输出、200px历史面板、前导空格/密码输入排除、清空迟到隔离、按准确会话ID隔离、关闭面板/收起侧栏停止查询、刷新恢复；OSC7目录跟随服从开关，中文/单引号路径同步、备用屏自动跳过/手动提示、内部cd不进历史通过。58方法最终产物复测复制零输入、原生服务失败回退、双失败提示、UTF-8字节上限、切换离开再回来及关闭后迟到粘贴均被丢弃；只替换浏览器接口，未访问系统剪贴板，finally已恢复。
- 最终server与官方macOS构建位于scratch `KitonyTerms-Shell-Clipboard-Final` / `KitonyTerms-Shell-Clipboard-Native-Final`；证据 `shell-clipboard-final-evidence.json`。最终36/220/48/58px、历史200px且无横向溢出。固定Wails runtime忽略原生剪贴板成功标志，故新增检查bool的服务包装；原生剪贴板GUI仍待验收。
- 外部编辑器检查点 `external-edit-final-check.log`：统一Go format/vet/internal race、绑定漂移、前端format/typecheck/lint/14测试/build通过，310包/1服务/62方法/49模型/1事件；最后仅状态文案修正另经 `external-edit-frontend-final.log` 全前端门禁通过，JS623.28kB。7项外部编辑race与1项真实SSH二进制回环覆盖32MiB边界内二进制、权限、冲突、代次、保存中再次修改、失败不循环重试、符号链接拒绝及自然断开/取消/清理。
- 真实SSH浏览器+实际受控编辑器进程：文件右键/打开方式、0600副本/0700目录、保存提示、一次上传内容回读及清理、连续两次自动保存/原子替换、旧代次拒绝、取消退出保留监听、结束不上传/保留副本、关闭记录不删副本通过；临时编辑器配置已恢复。保留副本再次磁盘校验通过，外部活动归零。最终外层36/220/48/58px且无溢出；真实用户编辑器GUI未执行。
- 最终当前源码产物为scratch `KitonyTerms-External-Edit-Verified-Final` / `KitonyTerms-External-Edit-Native-Verified-Final`，官方macOS构建通过，仍保留13对象/12部署目标警告。已正常退出旧server并启动最终产物，浏览器确认资产`index-BwqwIArD.js`、配置恢复/外部活动0及36/220/48/58px，无溢出。证据为`external-edit-final-evidence.json`。`external-edit-cross-compile.log` 中externaledit/session测试包 Windows amd64、Linux amd64/arm64均编译通过，未在目标系统执行；不代表完整应用或GUI验收。
- 底栏监控检查点`monitor-final-check.log`：统一Go format/vet/internal race、生成绑定漂移、前端format/type/lint/16测试/build通过；311包/1服务/64方法/54模型/1事件，JS628.06kB。4项解析和5项会话监控测试（含真实SSH）覆盖未知/零、大整数、guest/重置/网卡增删、共享订阅、租约、失败保留、心跳超时/旧序号、取消、协商及正文/stderr上限。
- 实际SSH浏览器：原CPU/内存/根磁盘/负载/网络五组指标恢复；902ms慢采样时心跳1ms、失败过期标记与成功快照保留、隐藏后零查询且远端monitor/sample归零并计数稳定、可见恢复、准确会话ID切换/迟到旧响应丢弃、未知无障碍语义、整页刷新同会话恢复通过。退出保护确认后server实际退出，夹具所有活动归零。最终资产`index-BwhbYz2a.js`、36/220/48/58px、无横向溢出复验通过；临时fetch/可见性/测试容器缩放均恢复。
- 最终当前源码产物为scratch `KitonyTerms-Monitor-Final` / `KitonyTerms-Monitor-Native-Final`；官方macOS构建通过，仍保留13对象/12部署目标警告。最终server已正常重启并验证新资产，证据`monitor-final-evidence.json`。`monitor-cross-compile.log`的monitor/session测试包Windows amd64、Linux amd64/arm64共6个二进制编译通过，未在目标系统执行，不代表完整应用/GUI/真实生产代理TUN验收。
- `bash app/scripts/interop.sh`：测试工具 Rust fmt/Clippy(-D warnings)/build；Go race；Rust 读取 Go 回写 TOML/vault；双向 LAN 持久化/ACK；WebDAV Go 创建→Rust 条件更新→Go 回读/旧 ETag 拒绝，均通过。
- 实际 Wails server + TCP SSH/内存 SFTP：配置保存、信任确认、终端 ready、文件读取、新建/重命名/递归删除、中文编辑保存回读通过；无横向溢出。
- 早期SFTP/基础分栏检查点：1024×768测得36px标题栏/220px侧栏/320px文件区/48px运维工具栏/58px底栏；运维展开为360px抽屉、侧栏191px、中央终端420px；分隔条调整、侧栏收起/展开、文件区内部横滚及六列、新建目录/编辑回读均通过。当时命令历史和底栏监控尚缺，后续已按上文补齐；完整设置与全部界面等价验收仍未完成。
- 官方 macOS Taskfile 构建通过；最终隔离数据目录原生二进制存活 3 秒，正常终止退出 0。仍有 Go 对象 macOS13.0 与目标12.0 的链接警告，最低系统版本未验收。
- Android ARM64/x86_64 库及 debug APK 构建；实测两种 ELF LOAD 为0x4000，ZIP 16 KiB 对齐通过；独立包覆盖安装。缺少正式签名的间接 build 与越界 build number 均被拒绝。
- Android 实际 WebView 曾验证 title/IPC、412px 无溢出、底部52px导航；合成敏感标记经JS接口/console输出后未出现在logcat。最后一次仅追加异常日志类别的 Java 修改已编译/打包/覆盖安装/进程启动通过，但当时 CDP 未就绪，未重复页面与标记验收。
- 相机曾观察到 live track；权限拒绝/关闭与后台释放尚未验收，UIAutomator/CDP后续检查超时。不能计为扫码原生验收通过。
- 当前monitor/session领域测试包与历史externaledit包交叉编译证据见上文；历史APK v2调试签名与ZIP对齐校验摘要见正式迁移文档，早于本轮源码，不代表当前移动产物验收。
- 完整 Xcode 缺失，活动目录为 `/Library/Developer/CommandLineTools`；iOS链接、签名、模拟器/真机未执行。iOS手工打包的签名/build number/IPA门禁仍待专项核对。
- 无正式移动签名、旧版升级/回滚或物理设备证据；当前Vite单JS680.71kB告警保留。

## 验证约定

当前按改动运行`app/scripts/check.sh`与发布契约测试；兼容改动使用公开冻结夹具和Go领域回归，旧`interop.sh`不再存在。下方旧跨语言执行记录均为历史证据；构建、原生运行、签名、旧版升级和真机输入分别验收。

## 外部编辑器实施

- [x] Go私有副本、文件变更检测、一次/自动上传、失败保留与显式结束。
- [x] 原文件菜单/状态/选择弹窗接线，退出活动保护。
- [x] 真实SSH回环与浏览器状态验收；实际受控进程启动通过，用户编辑器GUI仍待原生验收。

## 底栏监控实施

- [x] 对照原监控采集、SSH心跳RTT和底栏结构，保留58px外层布局。
- [x] 实现有界采样、速率差分、代次隔离和可见性/退出清理。
- [x] 运行对应race、浏览器与构建检查，更新平台矩阵。

## 后续界面与原生能力

- [x] 对照原设置分类补齐通用/终端/编辑器入口和前端中英资源，沿用同步面板；开发版更新状态如实展示。
- [ ] 系统认证GUI、桌面更新正式签名/原生升级验收；产品安装动作与可恢复安全收尾已接线。
- [ ] 保持原桌面布局，继续移动输入/导出/后台及P5平台验收。

完整目标仍未完成，本任务保留在当前任务索引，不归档。后续按剩余P2/P3/P4功能继续，具备对应环境后关闭P5，全部通过才实施P6。

## 设置与语言实施

- [x] 对照原`settings.rs`和Go Settings确认通用/终端/编辑器/同步/更新分类与字段；设置校验补齐有限数值、语言/光标/颜色和有界字体/关键字/编辑器，模型/存储/同步race通过。
- [x] 单个界面字体序列化/强调色前景及3项测试通过；根节点和Portal共用，终端字体独立。
- [x] 恢复桌面760px弹窗/168px分类栏、手机列表→详情；SettingsWriter按字段基线串行合并最新配置，版本冲突有限重试，同字段冲突/保存失败保留草稿并等待显式操作，保存中阻止关闭。
- [x] 行号/时间戳/换行/关键字标注使用xterm公共缓冲和marker，本地有界、独立普通/备用屏、不进入复制；前端应用文案显式资源化，远端/用户内容保持原样。
- [x] `settings-final-check.log`统一门禁通过：311包/1服务/64方法/54模型/1事件、26前端测试，JS `index-BmjFOZZa.js` 659.23kB；`settings-interop.log`实际Rust/Go配置/vault/LAN/WebDAV互通通过。
- [x] `settings-final-evidence.json`区分首检查点和最终构建：真实配置保存/字段并发/同字段冲突/单次失败显式重试、中文特殊字体/140%缩放/Portal一致、编辑器配置、终端ANSI/长行复制/备用屏时间戳/裁剪、手机412px分类与16px输入通过；最终英文文件/运维/监控/退出、配置快照反映目录开关、语言切换不重发在途运维、取消退出保留连接通过。基线设置和临时fetch已恢复，36/220/48/58px无溢出。
- [x] 最终server和官方macOS产物为scratch `KitonyTerms-Settings-Final` / `KitonyTerms-Settings-Native-Final`；编译通过，未执行原生GUI/真机验收，macOS13对象/12目标警告保留。原始及验收数据中未上传副本内容/0600权限已复核。

## 系统认证查看密码实施

- [x] Go `osauth`/`secretview`和仅索引存在性查询；认证不缓存且禁止并发，读取vault前后复核取消/退出/完整目标身份。macOS每次独立LAContext；Windows窗口绑定Hello，只有明确不可用才回退，显式账户入口使用实际LogonUserW与当前SID匹配，缓冲区清理；server/Linux/移动端拒绝查看。
- [x] 原连接弹窗局部只读字段；查看值与新编辑值分离，30秒、隐藏、关闭、编辑、身份变化或退出清除，旧代次结果丢弃；中英提示已接线。
- [x] `secret-final-check.log`统一门禁通过：313包/1服务/66方法/56模型/1事件，29前端测试；osauth 2项与secretview 4项测试含认证并发/取消/失败、每次重新认证、伪造目标及vault读取等待期间变化。最终JS `index-BdTwVt6D.js` 665.28kB/gzip193.32kB，CSS28.22kB。
- [x] `secret-view-evidence.json`：真实server只返回存在性并拒绝明文查看；合成响应验证只读/编辑值为空、目标往返/取消/关闭重开/页面隐藏后迟到丢弃、失败仅一次、查看后保存0次凭据写入、实际约30.9秒自动隐藏、英文提示。配置/transport/visibility恢复，36/220/48/58px无溢出。测试未读取用户系统凭据、未触发系统认证。
- [x] 当前源码最终server/官方macOS构建通过，scratch `KitonyTerms-Secret-Final` / `KitonyTerms-Secret-Native-Final`；osauth/secretview Windows amd64、Linux amd64/arm64共6领域测试二进制编译通过（`secret-cross-compile-evidence.json`），未执行目标二进制。macOS13对象/12目标与重复objc库警告保留。
- [ ] macOS/Windows系统认证弹窗、Hello配置/取消/账户密码/锁定及完整原生交互验收；Windows账户对话框为同步调用，取消请求只保证丢弃结果，门禁在调用返回前不释放。可选独立审阅超时停止，无报告，不计作通过。

## 桌面更新迁移实施

目标：保持原官方仓库、schema1清单、stable/alpha身份、设备本机preferences、资产ID/大小/SHA-256及取消语义；正式入口与安装接管仍受P5/P6门禁约束。

1. 先实现Go查询/代理/有界下载/缓存与后台状态，当前开发构建禁用更新；下载完成只标记downloaded，不代表已解包或允许安装。
2. 在原设置更新分类接线真实状态与本机偏好，补齐中英提示与关闭/切换时草稿保护，保留桌面外层布局。
3. 之后实施归档/平台身份验证、journal/备份/握手/替换回滚与启动确认，再执行对应原生验收；不以任意命令或下载成功替代安装事务。

- [x] 核对Rust metadata/client/download/proxy与后台状态、既有清单生成脚本和平台边界。
- [x] 实现Go构建身份/发布协议、固定源HTTP与代理、偏好原子保存和任务/缓存。
- [x] 接入设置更新分类、退出取消与局部轮询/保存冲突保护。
- [x] 本地HTTP/HTTPS CONNECT/SOCKS5/远程DNS/空密码用户名认证、TLS拒绝、取消/大小/摘要/缓存/限流/并发race及前端/统一门禁、隔离浏览器和构建验收。
- [x] 私有ZIP/TAR/DMG归档准备、平台头/签名/构建身份验证，旧task/journal只读解析与未完成事务保护；产品安装IPC已在后续阶段接线。
- [x] 内部安装引擎的备份、三阶段journal、目标卷替换、回滚、显式确认及安装锁；临时签名DMG完整替换/资源链接恢复与确认测试通过，未触碰已安装应用。
- [x] 内部helper副本/身份/PID摘要握手、等待旧进程退出和commit/abort、锁内协调与重启前原摘要校验；真实测试子进程覆盖就绪/取消/错误握手/提前退出与崩溃锁释放，产品命令已在后续阶段分流。
- [x] 恢复重启内部协议、正式helper CLI分流、GUI前应用/安装锁、服务退出后释放及页面显式确认；本机真实进程与隔离server/browser通过，原生GUI/正式升级尚未验收。
- [x] 安装发现/安装器包选择、产品IPC/GUI安装动作和既有安全退出/未保存所有权接线；隔离应用副本实际产品helper事务和浏览器通过。
- [ ] Windows实际UAC/ACL/占用、原生GUI与正式签名升级验收。

- 更新协议：固定nzesupb/KitonyTerms与资产ID；schema1、stable/alpha、完整commit/run/attempt/target绑定；开发构建仅有只读构建身份。响应/文件/超时/跳转有界；本机preferences与config/sync隔离，失败保存保持旧任务和内存，状态sequence与偏好revision分别为十进制字符串。
- `update-ui-evidence.json`分首版与最终修复证据：真实偏好保存/配置隔离、草稿跨分类/语言保留、关闭保护/并发冲突显式恢复通过；合成响应验证失败保留、保存中关闭/切换、隐藏零轮询/单链恢复/迟到丢弃、纯文本说明及99%→100%进度。首版同轮双击取消发现2次请求，修复后最终资产`index-C6rd9buo.js`实测仅1次。412px表单16px/44px；最终36/220/48/58px、无横向溢出。配置摘要及原本不存在的偏好文件已恢复，临时fetch/visibility/confirm均恢复。
- `update-archive-final-check.log`统一门禁通过：319包/1服务/71方法/64模型/1事件，34前端测试，更新领域28项顶层测试；JS680.71kB/gzip198.08kB、CSS29.22kB。真实DMG验证挂载/有界复制/包内链接、构建不匹配/签名损坏/越界链接拒绝及成功失败清理；`update-archive-dmg-recheck.jsonl`和`update-journal-final.jsonl`记录对应race结果。曾发现/修正macOS临时目录`/var`与`/private/var`比较问题，失败首轮不计为通过。
- 最终server/官方macOS产物为scratch `KitonyTerms-Update-Archive-Final` / `KitonyTerms-Update-Archive-Native-Final`；`update-archive-build-evidence.json`记录SHA-256、两产物真实`--update-info`/辅助安装入口拒绝且不创建数据目录，以及Windows amd64、Linux amd64/arm64共3个更新领域测试包编译与摘要，目标二进制未执行。保留编辑副本内容/0600权限复核通过；13对象/12目标和重复objc库警告仍在，原生GUI及正式签名验收未执行。

## 安装引擎与helper内部协议检查点（2026-11-02）

- `install.go`、`install_files.go`及平台适配实现持锁事务；同目录/用户目录布局沿用旧字段，文件刷盘后再写下一阶段。macOS包复用DMG的路径/链接限额与codesign并原子交换；文件在目标卷复制校验再替换。无已验证备份且目标变化时保留journal并失败，原目标未变时可回收旧文件事务；恢复清理可重入。
- `install_helper.go`实现内部普通协调器：自身/构建/任务/旧目标/更新包校验，真实子PID+摘要握手，ready不授权安装，必须旧进程退出+有效commit且无abort；持有helper/installation/应用锁执行，失败先恢复，重启前复核旧摘要。已有journal阻止新helper，不能误回滚其他事务。仅精确匹配的committed事务通过函数级confirm清理备份；尚未绑定原生GUI就绪。
- 新增17项顶层安装/协议测试，总更新领域45项。六个故障边界、四个独立进程崩溃点、跨进程锁/退出释放、损坏备份/目标/记录/暂存链接、取消/错误身份、旧字段确认及ENOENT传播通过；真实临时ad-hoc签名DMG完整安装/原资源和链接回滚/签名/确认通过。helper测试宿主注入已知current身份，只证明内部协议，不代表产品CLI/GUI接线或正式旧版升级。
- `update-install-final-check.log`统一Go格式/vet/internal race、绑定漂移、前端格式/类型/lint/34测试/build通过；319包/71方法/64模型/1事件不变，更新包106.199秒。原60秒门禁在新增串行原生测试后超时，失败记录保留为`update-install-first-full-check.log`；脚本总预算改为180秒，各操作短超时仍保留。
- 当前源码server与官方macOS构建为scratch `KitonyTerms-Update-Install-Final` / `KitonyTerms-Update-Install-Native-Final`；`update-install-build-evidence.json`记录五个产物摘要、两产物实际只读身份和安装命令拒绝/零数据目录创建、用户文档/编辑副本0600/偏好原本不存在的保持。三个Windows amd64、Linux amd64/arm64更新测试包仅编译，未执行目标二进制。
- 前端源码未改，生产资产仍`index-C6rd9buo.js`/`index-CMw-OlSE.css`，本轮未重跑浏览器或原生GUI。19256仍运行上一归档检查点的隔离server，并非本轮新产物；不得混同证据。保留macOS13对象/12目标、重复objc与Vite单chunk告警。
- 当前安装CLI仍拒绝、`installAvailable=false`；服务和GUI尚未持有安装引擎的锁或调用恢复。下一步继续Windows安装器/UAC/受限管理员入口与恢复重启、原生启动锁/就绪确认和退出安全接线，再启用对应入口；P0–P6总体未完成，生产Rust入口保留。

## Windows安装与恢复接线阶段（2026-11-02）

- [x] 核对Windows规范路径/外部路径、注册安装目录、受限管理员入口和普通协调器契约。
- [x] 实现仅绑定journal摘要的replace/restore、目录与输入固定、Shell/COM及安装器内部适配；占用/只读不提权，超时等待实际退出。
- [x] 接入内部安装/恢复协调，补充路径、篡改、结果身份及等待策略回归；产品入口启用仍依赖恢复重启和GUI就绪接线。
- [x] 运行本机race/统一门禁及Windows/Linux目标编译，分别记录实际执行与仅编译证据；更新文档及后续待办。

- `install_elevation.go`、Windows入口/句柄/Shell及注册安装器适配复用原事务引擎；管理员只替换/恢复，不执行新载荷、不启动GUI。runas前固定并复验helper/安装器，输入及原始父目录固定、拒绝reparse/hardlink，日志预建后允许写入但禁止换名；只对实际非只读目标的权限失败提权一次。普通协调器持续持锁等待管理员实际退出，取消/超时/等待API错误不允许并发恢复。
- Windows最终路径由文件句柄规范化，旧普通/扩展形式journal两侧规范比较；Shell/Inno安全边界单点转换并拒绝设备/危险/超长路径，保留中文、空格和UNC。旧Rust注册版staged摘要属于旧EXE时保留精确构建校验，新Go安装后记录新EXE摘要。
- 新增7项本机顶层回归，总updater52项；路径/参数、状态、过期结果、构建/helper变化、三种等待失败与真实子进程持锁/退出回收、旧摘要兼容通过。另有5项Windows专用句柄/日志/hardlink/reparse/结构布局/路径形式测试，本轮只编译，未执行。
- `update-windows-final-check.log`完整统一门禁退出0，updater112.310秒；319包/71方法/64模型/1事件、34前端测试及JS/CSS资产不变。`update-windows-integration-race.jsonl`记录定向事务与新增协议race通过（24.163秒）。Windows交叉vet和Windows amd64、Linux amd64/arm64测试包编译通过，不代表目标运行。
- server与官方macOS产物为scratch `KitonyTerms-Update-Windows-Adapter-Final` / `KitonyTerms-Update-Windows-Adapter-Native-Final`，`update-windows-build-evidence.json`记录5产物摘要、55冻结源码、10次实际CLI检查及用户文档/0600编辑副本/偏好不存在的保持。两产物只读身份正常，四个安装辅助命令仍拒绝且未创建数据目录；未替换已安装应用，未启动原生GUI或重跑浏览器，19256仍为历史归档检查点server。
- Windows实际UAC/ACL/占用/安装器及日志共享行为仍待原生环境；本阶段完成内部适配，产品命令尚未分流。下一步实施`--relaunch-update`的恢复记录/自身与PID摘要握手/旧进程退出/应用锁/普通权限重开，再接原生启动锁、GUI就绪确认与安全退出所有权。P0–P6总体未完成，任务不归档。

## 恢复重启与启动确认接线（2026-11-02）

- [x] `install_relaunch.go`严格读取Rust四字段数组，绑定固定helper/旧构建/摘要、原任务与journal原文；父子共用PID/摘要握手、父进程退出+commit/abort，锁内复验与普通权限重开，失败保留恢复材料。
- [x] `desktop.go`及进程入口在GUI前取得应用锁/安装锁，拒绝活跃/外来/损坏记录。中断事务转交旧helper，正常committed不清备份；Wails服务全部关闭后才释放锁。移动端沿用原生命周期。
- [x] 生成`ConfirmUpdateStartup`绑定；Info/Config/初始Sessions全部成功后，React下一帧发送一次确认，取消/退出拒绝迟到结果，失败显示已有错误且不自动重试。确认严格匹配启动时journal原文摘要。
- [x] 新增12项顶层回归，updater本机64项；真实进程涵盖ready/父进程存活/commit/abort/错误握手/等待后篡改/损坏备份与释放锁后重开。`update-relaunch-first.jsonl`首次协议race通过82.699秒；合并重复身份夹具后，`update-desktop-first.jsonl`6项启动race通过12.760秒。
- [x] `update-relaunch-final-check.log`完整统一门禁退出0，updater177.748秒；319包/72方法/64模型/1事件、34前端测试。资产`index-hUhx258R.js`681.29kB/gzip198.29kB、CSS不变。Windows交叉vet与Windows amd64/Linux amd64/arm64测试包只编译，未执行目标二进制。
- [x] 当前server与官方macOS构建通过，`update-relaunch-build-evidence.json`含65冻结源码、6产物摘要与10次开发产物只读/拒绝命令。额外公开正式身份夹具实际验证CLI ready/PID摘要/取消/缺少commit/错误摘要/多余参数；不启动GUI，不替换安装，非正式发布。默认开发构建仍拒绝4个安装辅助入口且不创建数据目录。
- [x] `update-relaunch-ui-evidence.json`：实际隔离server，延迟Sessions及配置失败均0确认，恢复后1次，正常加载1次；合成确认失败保留错误/不重试、移动标记0次、离开待加载页面后迟到0次。正常工作台36/220/48/58px、无溢出。首次转发未保留Host造成Forbidden，不计通过，修正测试转发后重测；产品来源/CSP未改。React子代理超时停止且无报告/改动，不计证据，接线与最终验证由主代理完成。
- [x] 实际安装发现/包选择、产品安装动作与既有退出/未保存保护已在下一阶段完成，符合条件时动态开放`installAvailable`。原生GUI启动/菜单退出、Windows UAC/ACL/安装器及正式签名/旧版升级仍未验收，生产入口保留Rust，迁移任务不归档。

## 安装发现与产品安装动作实施计划（2026-11-02）

- [x] 第一阶段：复用规范路径与注册表校验，发现实际app/便携/注册安装；区分镜像/Translocation、不可写、Windows授权与不可恢复错误。更新服务绑定桌面锁owner和同一root/构建，使用实际安装类型选择归档或安装器；开发构建不写探测安装目录。新增6项本机race通过1.707秒，相关安装/授权回归通过1.719秒，全模块vet/Windows交叉vet及测试包编译通过；生成requiresElevation绑定，前端类型检查与34项测试通过。本阶段尚未执行完整门禁、当前产物或原生运行，安装按钮仍禁用。
- [x] 第二阶段：复用Desktop.engine串行准备并重探测实际安装/权限；Prepare/ready与Commit分离，未授权失败精确Abort并回收helper。退出控制器区分install/quit，在途保护与安装准入原子互斥；Manager.Drain按五类真实worker等待，超时/失败恢复准入，提交前不永久关闭业务。
- [x] 第三阶段：生成75方法绑定；原更新卡片接入直接重启安装/Windows授权、独立状态层、同步布尔保护、双次安全检查、取消等待和刷新恢复。主界面与业务Portal禁用，失败恢复，显式中英资源，移动端与development禁用；桌面布局保持。
- [x] 第四阶段：定向race、真实helper及隔离签名应用副本产品事务、完整Go/绑定/前端门禁、当前server/macOS构建、Windows交叉vet及三目标测试包编译、20个浏览器场景和保留项核验完成；只编译/合成/原生未验收分别记录，未接管正式数据或替换本机安装。

## 产品安装接线验证（2026-11-02）

- `update-installation-full-check.log`完整统一门禁退出0：Go格式/vet/全部internal race、绑定漂移、前端格式/类型/lint/44测试/build；319包/1服务/75方法/64模型/1事件，updater本机78项顶层测试196.368秒。脚本包预算240秒，各真实操作短超时保留。最后仅前端刷新提交失败恢复/过期错误清理修正，另经`update-installation-frontend-final.log`全前端门禁；最终`index-eTc-2HZt.js`692.53kB/gzip201.97kB、CSS29.28kB。
- 新增可恢复Drain、退出安装所有权和保护操作计数、Desktop/Service准备/提交/回收；针对性race涵盖 removed-session worker、旧release、200轮业务/安装准入争用、64路单次提交、并发关闭、真实helper退出/握手丢失、外来task/journal保留、失败材料隐私及真实签名DMG准备/取消/损坏包恢复。10项前端协议测试覆盖同帧双击、迟到ID取消、未知所有权、回收重试、提交响应丢失与刷新失败恢复。
- `update-installation-product-e2e-evidence.json`：scratch overlay驱动仅注入已下载候选，使用两个公开正式身份的真实产品server副本及临时签名.app/真实DMG。实际产品helper完成取消回收、再次准备/提交、父进程仍在时目标不变/无journal、退出后替换及新server重开；新签名/资源链接/精确身份通过，旧备份/journal在真实React页面确认前保留、确认后清理。夹具非发布、非正式旧版升级，未启动原生GUI或替换已安装应用。首轮后台任务容器回收派生server的问题经驱动保活和整套重跑解决。
- `update-installation-ui-evidence.json`：最终资产20个浏览器场景通过，直接点击无二次确认/重复提交、root和Portal禁用、提前取消等精确ID及实际回收、失败/显式重试、后到保护保留草稿、隐藏取消、提交不确定保持等待、刷新遗留请求、刷新提交失败重新加载初始数据、中英授权/状态、普通退出在途操作计数、移动/开发禁用；仅最后产品页面确认及开发禁用为真实服务结果，其余按文件标明合成协议/配置。最终36/220/48/58px、无横向溢出。
- 当前server/官方macOS最终产物为scratch `KitonyTerms-Update-Integration-Final` / `KitonyTerms-Update-Integration-Native-Final`；Windows交叉vet、Windows amd64/Linux amd64/arm64 updater测试包编译通过，未执行目标二进制。macOS13对象/12目标、重复objc及Vite单chunk告警保留。原生系统认证/剪贴板/GUI退出安装、Windows授权/安装器及正式签名/旧版升级仍待P5，P0–P6任务继续，不归档。
- `update-installation-final-evidence.json`冻结251份源码与8产物摘要，10次实际开发产物CLI只读/拒绝检查零数据目录创建；124个文档本地链接/空白/脚本语法/diff检查通过。本轮两个隔离server经RequestQuit/ConfirmQuit正常退出，4把应用/安装锁实际重获；用户发布文档SHA-256、两个未上传副本内容/0600、原偏好不存在和旧server PID65386/归档产物身份保持。

## 系统字体枚举与选择器实施（2026-11-02）

目标：恢复原界面/终端字体下拉框及未知已保存值，保持布局、单字体CSS转义和字段串行保存语义。字体清单只属于设置局部状态，不进入配置或同步。

- [x] 第一阶段：核对原fontdb行为与平台官方接口；macOS CoreText、Windows GDI、Linux有界fc-list，不引入新依赖。移动端沿用七项回退，禁用cgo/不支持平台与枚举失败明确区分来源。
- [x] 第二阶段：实现Go字体目录/DTO/IPC并生成绑定；恢复两个原位select，异步结果按组件生命周期归属，保留未知字体名与显式重试。
- [x] 第三阶段：归一化/排序/限额/失败与取消回归、本机真实CoreText、目标编译、统一门禁、当前构建和隔离HTTP浏览器布局/保存验收；分别记录原生GUI与目标运行的未验收项。

接口依据：Apple的CTFontManagerCopyAvailableFontFamilyNames返回可见族名；微软EnumFontFamiliesExW使用DEFAULT_CHARSET、空FaceName及零PitchAndFamily；Fontconfig格式使用family[0]保留单族名，避免按逗号拆分合法名称。可能等宽仅沿用原名称排序启发式，不作为实际安装或字宽证明。

- `fonts-full-check.log`完整统一门禁通过：320包/1服务/76方法/65模型/1事件，47前端测试；字体race 4.853秒，更新器回归196.194秒。定向`fonts-targeted.log`另覆盖CoreText四路并发、记录/输出限额、分类错误与取消、真实子进程回收及本机Fontconfig；实际CoreText252族（含回退），本机Fontconfig2811条记录只证明macOS上的适配器，不能计作Linux原生验收。
- 当前官方macOS、CGO_ENABLED=1 server和CGO_ENABLED=0 server产物为scratch `KitonyTerms-Fonts-Native-Final` / `KitonyTerms-Fonts-CoreText-Final` / `KitonyTerms-Fonts-Fallback-Final`。实际IPC分别返回system/252与fallback/unavailable/7；Windows交叉vet、Windows amd64/Linux amd64/arm64字体测试包编译通过，目标二进制未执行。移动只沿用预设列表，未重新构建当前APK/iOS。macOS部署目标、重复objc和Vite单chunk告警保留，JS694.51kB/gzip202.48kB，CSS29.28kB。
- `fonts-ui-evidence.json`的20个最终资产浏览器场景区分实际服务/保存与合成故障：原位select、未知单族/旧终端栈、分类与语言零重复请求、失败/显式重试、关闭后迟到丢弃、特殊名称实际持久化/CSS与Portal一致、系统默认清除、手机16px/44px均通过。最终36/220/48/58px、设置760px/168px无溢出；配置已恢复，两服务通过退出协议正常结束，浏览器返回保留的19257。原生字体观感与Windows/Linux枚举仍待对应环境；本阶段React子代理停止前未交付，主线程实现并验证，不计委派为证据。
- `fonts-final-evidence.json`/`fonts-frozen-source.json`冻结267份源码和6产物，3次开发产物只读CLI均零数据目录创建，126个文档链接及脚本/diff检查通过；CSS/appearance、依赖锁文件和更新源码与上阶段一致。用户发布文档SHA、两份未上传副本内容/0600、原更新偏好不存在、旧server19256/PID65386保持，两个临时服务的4把锁实际重获。

## P2 完全停滞远端与 Unix 原生 agent（2026-11-03）

限定范围：以真实回环 SSH 和独立临时系统 ssh-agent 验证认证、转发、ProxyJump 归属及取消回收；不访问用户 agent/密钥，不改生产连接、桌面布局或入口。完全停读写的远端可以触发宿主传输关闭，合作远端已有的通道取消/连接复用仍须通过。

- [x] 第一阶段：核对 session、通道、agent 与跳板连接的所有权，确定真实停滞和实际退出的观测点。
- [x] 第二阶段：补齐未回应 OpenChannel、停读写/写背压及隔离原生 agent 回归；仅修复已复现的生命周期缺陷。
- [x] 第三阶段：定向 race、相关包及统一门禁、目标测试包编译；更新矩阵和来源明确的证据。Windows 命名管道、原生 GUI 和移动实机不计作本机 Unix 验收。

- 首次真实TCP背压回归复现ProxyJump取消卡住，直连同条件通过：内层Close仍向已停读跳板发送通道关闭，无法解除写锁。新增jumpConn先关闭独占跳板传输，再关闭内层通道；dialJump复用negotiation等待实际拨号完成，取消不留下DialContext内部后台拨号。
- `p2-targeted-final.log`定向race连续两轮34.306秒通过。新增5项顶层/12个子场景覆盖SFTP OpenChannel/子系统/版本完全停滞、直连与跳板TCP写背压、跳板开通道与内层握手Drain、系统agent仅认证/实际转发签名/只向目标转发、认证与转发挂起socket回收；同时复验既有SFTP取消后复用及正常跳板。
- 原生agent通过私有短目录启动系统ssh-agent -D，密钥仅内存生成并通过独立socket添加，实际签名由公钥验证；临时进程Kill/Wait和目录清理由测试收尾。挂起agent为故障夹具，不能混作系统agent正常运行证据。首次缺少公钥认证声明和版本阶段未等INIT的失败日志保留在p2-agent-first.log/p2-first-regressions.log，最终回归修正观测点后通过。
- `p2-full-check.log`统一Go/绑定/前端门禁通过：session race 50.359秒、updater 196.146秒，320包/76方法/65模型/1事件与47前端测试保持。Windows交叉vet session/ipc及Windows amd64/Linux amd64/arm64三个session测试包编译通过，未执行目标二进制。
- 当前官方macOS开发构建与CGO_ENABLED=0 server重建通过，scratch `KitonyTerms-P2-Native-Final` / `KitonyTerms-P2-Server-Final`；后者按既有规则使用字体回退。前端资产仍index-B3OGbTiM.js / index-DwXO_pTh.css；无本轮GUI/浏览器、实际升级或移动验收，macOS部署目标/重复objc和Vite单chunk告警沿用。字体/安装历史产物和证据按各自源码冻结，不能将旧HTTP验收改称新产物已运行。
- `p2-final-evidence.json` / `p2-frozen-source.json`冻结271份源码、5产物、2次实际只读开发CLI与127文档链接。与字体检查点比较，仅dial.go/roundtrip_test.go变化及4个session新文件；全部前端/生成绑定/依赖/入口/update源码保持。用户发布文档SHA、两个未上传副本内容/0600、原偏好不存在及原19256/PID65386身份均保持。总体迁移仍未完成，不归档。

## Android 当前源码调试包与模拟器复验（2026-11-03）

限定范围：SDK36/NDK28.2/私有JDK21，现有ktdbg AVD以read-only/no-snapshot启动，独立com.kitonyterms.app.wailsdev调试包；保留既有数据，不卸载原应用，不改正式签名或发布入口。当前源码重新构建ARM64/x86_64共享库及debug APK，核对签名/摘要/16KiB，再按实际可观察的WebView/生命周期行为记录证据。模拟器不代替真机、正式升级、完整IME或原生终端验收。

- [x] 核对隔离工具链、AVD与当前APK构建输入。
- [x] 重新打包并检查v2调试签名、ELF/ZIP对齐；仅向只读模拟器覆盖安装独立调试包。
- [x] 验证实际启动与可观察WebView/权限/退出行为，记录未覆盖项及完整清理。

- 首轮当前源码debug APK固定为scratch android-current-debug.apk，SHA-256为815a0d5817e83256111b2eea32c87b08dd4fb9d7e8a75527529fc74388256dc2；ARM64/x86_64共享库均重新构建。该包实际WebView验证移动IPC、字体预设、权限拒绝/视频帧/关闭与后台释放，不能将此前仅启动记录当完整验收。
- 系统权限窗口暴露模板名Wails App，现改为KitonyTerms。首次Android lint有5处错误：模板定位缺少权限、旧onBackPressed及相机硬件声明。定位为未使用的模板能力，保留JNI入口明确unavailable，不增加定位权限；相机/自动对焦为可选，返回迁移至OnBackPressedDispatcher。根页面BACK实际销毁Activity并残留Go进程，重开退出；进一步改为根任务moveTaskToBack(false)，有WebView历史仍优先返回。
- 最终APK为scratch android-current-final.apk，SHA-256为d4570e41c01e092460bcd5bad77260b205b6e25186a375b9110551f1227fcb3b。v2调试签名/1签名者、ZIP16KiB通过；两ABI最终SO与首轮完全一致，各4个ELF LOAD=0x4000。APK包名com.kitonyterms.app.wailsdev/versionCode1/versionName0.1.17-wailsdev，设备base.apk摘要与最终文件一致。正式签名、x86_64运行不在此证据范围。
- 最终lintDebug/assembleDebug通过，0错误/21警告；testDebugUnitTest为NO-SOURCE，不能计为单元测试通过。三个前期脚本失败分别为遗留权限标记、冷启动上下文未就绪、根BACK销毁后的CDP超时；日志和部分结果保留，未计为通过。最后实际MAIN/LAUNCHER冷启动1118ms，verify-android-current-final.mjs完整16场景通过：真实移动IPC/当前资产、七项字体和原位控件/16px44px/412px无溢出、更新IPC拒绝、产品名权限窗口、系统拒绝零轨道、等待授权时关闭后迟到轨道释放、480×640真实视频帧/关闭、HOME释放/恢复暂停/重开、原生BACK返回历史、根BACK隐藏并保持同一PID/任务、恢复后IPC和移动退出拒绝、force-stop后进程不存在。后者是系统强制停止，不是优雅退出证明。
- 当前Go/前端/生成绑定与P2冻结的271源码摘要一致，未重复运行无变化的Go/前端全门禁；本轮增加Android宿主/资源输入冻结。原36/220/48/58px布局源码未改。只读模拟器emulator-5560和本轮ADB转发19268已清理，5560/5561/19268无监听，不保存userdata/快照；原19256/19257及未上传副本保留。
- 当前模拟器使用emulated相机，未通过实体二维码识别；真实扫码、Android16预测返回手势、完整IME/选区、后台SSH/导出、系统销毁Activity后的同进程重建、正式签名/旧版覆盖升级及真机仍待验收。最终证据见scratch android-current-final-runtime-evidence.json、android-current-cleanup-evidence.json及android-current-final-evidence.json；完整迁移不归档。

## Android Activity 重建与进程级后端（2026-11-03）

目标：Activity 销毁/重建不等同进程级Go服务退出。固定Wails3.0.0的nativeInit每次启动main、nativeShutdown关闭单例应用，因此由应用宿主仅初始化一次并安全重新绑定Activity/WebView；不修改依赖、生产入口或桌面布局。旧界面请求/回调必须按归属隔离，系统选择器/对话框和监听实际回收，不保存输入或凭据。

- [x] 确认固定JNI初始化、全局引用及Activity销毁的所有权缺陷。
- [x] 实现进程级桥接、界面解绑/重绑、旧任务回调隔离与原生UI资源清理。
- [x] lint/构建和真实Activity销毁/重建回归，确认同PID后端与新WebView/IPC正常。
- [x] 冻结本阶段证据、更新迁移矩阵并清理本轮只读模拟器。

- WailsApplication拥有桥接，Manifest注册Application；nativeInit每进程一次，Activity.onDestroy关闭旧JS接口、取消原生UI请求、解绑并销毁WebView，不调用nativeShutdown。应用Context承担存储/包查询，界面任务捕获并复核owner/view；不修改Wails依赖/Go/前端/生成绑定。初版static桥接触发StaticFieldLeak，改用Application归属而未抑制lint。
- android-rebind-product-final-build.log独立产品门禁退出0：lintDebug/assembleDebug通过，0错误/21警告，testDebugUnitTest仍NO-SOURCE。当前android-rebind-final.apk SHA-256为dd53e303dc231e3042f21ecf670fd4b39eafcb6756d30ee1ad0646bcfec701d4，与干净重打包及设备验收摘要一致；v2/单签名者、ZIP16KiB通过，两ABI共享库与前检查点相同。
- android-rebind-instrumentation-evidence.json记录两轮各28项通过：实际recreate/finish/reopen、同PID/桥接、旧Activity/WebView/JS归属拒绝、键盘监听释放、解绑字段空、解绑期间后端可读及新页面IPC/字体/配置正常。真实127.0.0.1分享在多次重建中保持同一资源并于结尾停止；真实Question/OpenFile在Activity销毁时取消，等待中的Go runtime请求实际返回。临时test源码和runner DSL只在scratch，未新增产品测试入口。
- android-rebind-final-runtime-evidence.json的16项页面/相机/返回回归全部重跑通过；android-rebind-repeat-evidence.json的两轮各3次am start -W -R 3及8项检查通过，新页面保持同PID/分享/配置，初始化日志每进程一次。不能将上一APK的通过改称本轮执行。
- 临时runner过晚配置、辅助测试方法结构错误和always_finish_activities自动销毁尝试失败均保留原日志；前两项修正后测试构建/两轮运行通过，后一项未建立系统自动销毁证据，原值null已恢复。已验证范围为recreate/finish/reopen和系统显式finish/relaunch，不等同内存压力、进程死亡、GC无泄漏或全部JNI竞态。
- android-rebind-cleanup-evidence.json记录只清理本轮emulator-5560/ktdbg、准确ADB转发和设备测试文件；force-stop独立debug包后PID消失、crash buffer无异常、开发设置恢复，5560/5561/19268释放。不保存userdata/快照、不卸载原应用、不影响保留桌面服务。完整P0–P6仍继续，不归档。
- android-rebind-final-evidence.json/android-rebind-frozen-source.json冻结307份输入（P2的271份未变+36份Android）、3产物及130个文档链接；与前Android检查点比较只有4份宿主/Manifest变化和新增Application。两ABI再次ELF核验通过，发布文档SHA、两个未上传副本内容/0600、原偏好不存在及19256/PID65386和19257保持。

## Android 后台进程终止与冷恢复验收（2026-11-03）

限定范围：同一当前debug APK、ktdbg read-only/no-snapshot，系统am kill只终止后台独立调试包；不以Activity重绑结果替代进程死亡证据，不修改正式数据。验证新PID/新Go初始化、磁盘配置保持、内存分享/保护状态清空和旧WebView草稿不恢复。系统命令终止不等同真实低内存杀进程、后台SSH持续性或真机验收。

- [x] 实际后台终止进程并冷启动，连续两轮验证持久化/内存资源边界。
- [x] 核对当前包摘要与初始化/异常记录，清理准确forward和只读模拟器。
- [x] 更新矩阵、冻结本阶段证据及新的继续迁移交接。

- android-process-runtime-evidence.json/日志退出0，共20项通过：两轮HOME后am kill实际旧PID消失，冷启动生成新PID/新WebView，新Go桥接各初始化一次。保存外观设置后完整配置保持，进程重新生成revision；内存分享、退出保护状态和未提交连接草稿不恢复，实际Info/Fonts/Config/Sessions IPC正常。只改变独立调试实例的外观值，最后完整恢复原配置，不记录配置内容、配对码或用户输入。
- 本轮产品源码与android-rebind-frozen-source.json的307份输入完全一致，当前APK及设备摘要仍dd53e303dc231e3042f21ecf670fd4b39eafcb6756d30ee1ad0646bcfec701d4；没有新增产品测试入口或重新声明Go/前端全门禁执行。冷恢复与同进程重建证据分开，系统am kill不是低内存或后台SSH持续性验收。
- android-process-cleanup-evidence.json：开发设置null未改动、独立配置恢复，force-stop当前debug进程后PID不存在、当前crash buffer无异常；准确ADB forward和emulator-5560/ktdbg已退出，5560/5561/19268无监听，不保存userdata/快照。移动导入/批量导出IPC仍明确拒绝，属于后续实现项；对应平台GUI/签名/真机与P5/P6继续开放。
- android-process-final-evidence.json/android-process-frozen-source.json冻结307份未变产品输入、20项实际检查/两次COLD启动（770ms/842ms）及130文档链接；源码/当前APK/清理/发布文档SHA/两个未上传副本0600/原偏好不存在/保留19256与19257全部复核通过。新交接为scratch continue-after-android-lifecycle.md，优先于continue-after-android.md；下一实施项为移动文件导入/导出，原桌面布局和Rust生产入口保持。

## 移动文件导入/导出适配实施（2026-11-03）

目标：开放原上传/下载入口的Android文件/目录批次操作，复用Go预检/原子上传/任务进度与取消，桌面路径和布局不变。固定Wails的AndroidOpenFile只复制缓存且忽略目录，SaveFile明确不支持，因此新增应用自有JNI/SAF适配，不修改依赖。iOS无对应宿主/SDK，继续明确未适配。

- [x] 核对原Rust桌面下载/原位菜单、现有Go任务与Android选择器、SAF官方契约：OPEN_DOCUMENT/TREE/CREATE_DOCUMENT按用户授权，不增加存储权限；Android11+目录目标受限，不能承诺Download根目录可选。
- [x] Go私有暂存与平台接口、Android应用级JNI桥/SAF读写、名称/类型/数量/深度/体积边界、准确取消/Activity解绑和实际worker收尾。
- [x] 接回原上传/下载及批次任务；只有完成文档导出才成功，不把私有暂存下载完成当导出完成；本地草稿与应用文案规则保持。
- [x] Go/race/绑定/前端门禁、双ABI当前APK和Android文件/目录/权限/取消/冲突/重建回归；真实系统选择器30项与注入provider20项分开记证据。
- [x] 清理准确测试数据/服务/模拟器，核对发布文档与未上传副本/保留桌面服务，更新矩阵；完整P5/P6继续开放。

实现边界：Go持有私有暂存目录和请求ID，JNI只在Application级对象上注册一次；用户URI/文件内容不进入前端全局状态、日志或同步。暂存仅限本次目录，目录导入保留空目录并拒绝不安全名称/虚拟文件/循环；导出拒绝已有目标而不静默覆盖，失败保留已完成的外部文件并回收当前未完成文档，清理失败必须显式暴露。取消/解绑关闭正在读写的描述符及选择器，Go等待实际结束；传输任务收尾包含暂存清理，不能提前成功或归零worker。

共享层已实现：mobile-files请求目录0700/文件0600、16个活动暂存上限、1GiB/50000项/64层限制与启动时仅回收本模块遗留；JNI独立Application注册、UTF-8 byte[]及Activity归属撤销，原生描述符非阻塞轮询取消并等待实际结束。SAF目录保留根名与空目录，拒绝链接/虚拟文档/大小写及Unicode冲突；导出无覆盖、名称复验、当前未完成文档清理及SHA-256回读验证，暂存释放在任务终态前。原位入口按mobile_files能力开放Android，iOS明确未适配，外部编辑仍桌面专属。

首轮自动检查：mobile-files-go-second.log定向overlay/race通过（4个暂存、4个任务顶层场景；宿主文件系统合并名称导致2个子场景明确跳过）；真实SSH回环验证导出/清理前非终态、取消与worker、预检/实际字节限额、准入失败资源归属。mobile-files-full-check.log退出0，322包/76方法/65模型、47前端测试、全部internal race与绑定漂移通过；JS index-Cc62zd-7.js 695.25kB，CSS index-DwXO_pTh.css不变。Android首轮API检查暴露Os.fcntlInt/API30及isChildDocument/API29，改为自有JNI非阻塞和tree作用域校验后lint通过；testDebugUnitTest仍NO-SOURCE。最终原生复验和当前门禁见下文，旧生命周期APK不计作新文件能力证据。

原生文件验收（2026-11-03）：设备暴露两项真实兼容缺陷：Root.Chmod调用fchmodat2遭API35 seccomp终止，改为Root打开的目录句柄Chmod；私有暂存硬链接提交被拒绝，改为显式独占暂存上下文的O_EXCL写入/刷盘/关闭，整批完成才交给导出，桌面原硬链接提交保持。单文件选择另修复未通过空文档校验就纳入删除清理的问题，并拒绝系统自动改名。首崩溃、linkat拒绝及临时驱动/夹具失败均保留。

mobile-files-native-final-full.log通过21项真实系统检查：非BMP文件/目录导入、真实SSH上传、SAF批量导出与内容相等、已有目标不覆盖、选择器取消、销毁重开/SSH保持、暂存/能力/保护计数清零和无溢出。mobile-files-native-single-first.log通过9项：空目录上传保持、实际CREATE_DOCUMENT保存、自动改名拒绝/清理与原文件保持。另以设备外部文件SHA-256确认两份导出内容及空目录，非沙盒复制成功。

mobile-files-native-faults-final-second.log通过20项独立注入检查：合成跨进程provider提供大小写/NFC冲突、不安全名、超1GiB元数据、虚拟文档及已有非空文档；无效项在打开前拒绝，已有文档无删除调用。挂起可靠管道在途时保护计数仍大于零；Activity销毁后实际IPC返回，provider按内核peer-close事件回收写端，暂存/能力/保护计数归零，原SSH仍可用。这些不是用户通过系统选择器选取任意云provider的证据。宿主APFS及模拟外部存储均合并等价名称，原2个宿主跳过项保持如实记录，由注入元数据专项补验。

最终共享门禁mobile-files-current-full-check-second.log退出0：322包/76方法/65模型、47前端测试、全部internal race（session52.498秒/update194.569秒）、绑定漂移/格式/类型/lint/build通过；私有暂存专项overlay/race为4个暂存+5个任务顶层检查。当前双ABI APK SHA-256 d761c68be6359f0f66a036da02f67cf38ab077ccbc3fd5082630cf5aa665da05，73357024字节，与设备一致；v2/单签名/ZIP16KiB/两ABI各4段LOAD0x4000通过，产品不包含测试provider/认证资产且无新增存储权限。x86_64仅编译与包验证；正式签名、真机/任意provider、后台SSH/IME/真实低内存与P5/P6继续开放。

mobile-files-runtime-evidence.json/mobile-files-final-package.json记录当前设备/包与50项检查，mobile-files-cleanup-evidence.json记录准确回收：专用Downloads子目录、debug/provider进程、19270/PID83675 fixture和emulator-5560/ktdbg均结束，无本轮ADB forward；5560/5561/19270释放，不保存userdata/快照。临时认证资产及含该资产的测试APK/压缩副本已删除，保留产品APK/测试源码/日志。开发设置null不变，独立配置在每阶段恢复，发布文档SHA、两份0600未上传文件、偏好不存在及原19256/PID65386、19257服务均复核。当前产品输入与证据摘要由mobile-files-frozen-source.json/mobile-files-final-evidence.json冻结；本阶段结束，整体迁移任务不归档。

## macOS原生桌面补验（2026-11-03）

目标：在独立bundle/数据目录中补齐当前源码原生构建、页面关闭/系统应用退出保护及可操作的原生文件交互。只操作非终端界面，不自动化系统认证；不替换已安装应用，不接管正式数据。控制插件仍枚举为0应用/0窗口；Cocoa/CoreGraphics确认实际窗口存在，改用仅匹配本轮PID、可执行路径、Bundle ID和新鲜控件身份的scratch AX助手。

- [x] 固定当前源码构建macOS ARM64开发产物，独立bundle/数据目录启动并核对窗口身份。
- [x] 验证页面关闭/系统应用退出及取消、选择器在途保护、原生文件往返和字体保存/恢复。
- [x] 复核异常动作与源码接线；未确认产品缺陷，保留原实现，键盘/剪贴板等不计通过。
- [x] 回收本轮实例，复核保留数据/服务，冻结产物/实际检查/受限项并更新矩阵。

官方`wails3 task darwin:build ARCH=arm64`构建退出0，raw Mach-O为16491842字节，SHA-256 `076b4dd9da59bff47773f6eb85e471a7de1b53a326a07b7b1c5dbb76838267c1`。独立`com.kitonyterms.app.wailsdev.macos`应用包使用ad-hoc签名，strict验证通过；签名后可执行文件摘要为`f0a04c86806a0e9b016babc8d4fe76be90d1b08e107bf5bbee6171f7c36da341`，与raw不同属于签名修改。实际窗口14440为1199×799，设置分类可打开；没有修改产品布局或Wails依赖。

退出：前台页面关闭与NSRunningApplication系统退出请求均实际显示保护弹窗，取消后同一实例恢复。后台页面已有请求在重新激活时才显示，因此初期AX菜单/CG按键返回成功不单独证明菜单或Cmd+Q。空闲实例95055和新建一条活动SSH会话的实例97735均经产品确认正常结束，后一轮154ms；应用/安装/helper三把锁均实际释放。原生WindowClosing hook、独立Cmd+Q和菜单派发仍未确认，不能用这些应用退出结果替代。

文件：原有SFTP界面经真实NSOpenPanel选择文件/目录，连接的是临时公钥认证、回环监听的Go SSH/内存SFTP夹具；不是前端响应注入，也不是生产OpenSSH验收。非BMP文件上传、目录根名/子文件/空目录上传、两根批量下载、实际本地字节/SHA-256、重复目标拒绝且无临时残留通过。选择器打开时protectedOperations=1且退出按钮禁用；取消后实际返回并归零。目录选择和下载均使用OpenPanel，不计NSSavePanel证明。

字体：原生下拉253项（系统默认加252个目录项），选择Menlo实际写入隔离TOML，再选择系统默认清除字段。该结果证明原生控件/保存链路，不证明Hinting、实际等宽或跨屏DPI观感。

未通过与测试限制：WebView文本框AX赋值及合成键盘未形成有效编辑，剪贴板复制/粘贴匹配失败；最后两轮明确返回剪贴板已恢复，原值仅存内存，没有输出。只读探测确认AX/事件投递权限，主窗口Main=true但Focused=false；Wails实现允许Key/Main，尚不能定位为产品缺陷。原生键盘、剪贴板、未保存编辑提示和系统认证仍待继续。夹具旧会话曾以缺少exit status结束，第一次上传因此失败，重连后的完整上传/下载通过；不计长连接稳定性。夹具停止后网络已释放但worker等待未收敛，准确PID97705最终SIGKILL清理，不计夹具优雅退出，产品正常退出证据独立保留。plist解析、焦点/菜单、脚本预期、旧助手编译及Python缺少tomllib等失败均保留，不计通过。

315份产品输入与`mobile-files-frozen-source.json`全部一致，沿用该精确源码已通过的`mobile-files-current-full-check-second.log`完整门禁（322包/76方法/65模型、47前端测试和全部internal race）；本轮只重建原生包和执行上述GUI检查，没有重跑完整门禁。台账校验通过：13项原生子集检查、4份包文件和132个文档链接。隔离配置恢复、临时私钥/唯一夹具信任记录移除、19272释放；两份0600未上传副本、缺省更新偏好、发布文档摘要和19256/19257保留服务均保持。冻结入口为scratch `desktop-native-final-evidence.json`与`desktop-native-frozen-source.json`。完整迁移任务不归档，Rust生产入口保持。

## macOS进程内原生事件补验（2026-11-04）

目标：继续定位上一轮外部AX/合成按键未形成编辑的问题。在同一个未改产品源码的隔离开发包中加载仅scratch存在的临时Objective-C测试库，观察NSApp/key window/first responder，通过受限的Cocoa菜单与原生事件操作测试输入框。测试库只接受私有Unix socket上的固定命令；不开放任意JS、终端输入或系统认证操作，不输出剪贴板/用户内容。原生产入口、已安装应用和Wails依赖保持。测试库注入、原生菜单/按键事件与物理键盘/系统IME证据分别记录。

- [x] 建立独占数据/控制socket与受限测试库，核对原生焦点及产品二进制身份。
- [x] 验证原生输入、菜单复制粘贴、窗口关闭/菜单退出及取消恢复，按实际支持范围记录。
- [x] 原生远程未保存警告/取消保留/保存回读通过；未确认产品缺陷，保留源码和既有精确源码完整门禁。
- [x] 产品确认退出/夹具正常停机，回收本轮socket/私钥/信任/配置，保存原始失败及新证据，更新矩阵与交接。

执行结果：scratch测试库精确绑定隔离包路径/Bundle ID，控制目录0700/socket0600，固定命令无任意参数。第一次测试库premain调用unsetenv干扰Go executable-path初始化，保留失败库/日志，移除该测试工具行为后正常启动；产品与Wails未改。315份产品输入和签名后可执行文件摘要始终保持。显式激活/key window/first responder后，NSApp原生ASCII事件与NSTextInputClient中文/非BMP输入匹配，并观察可信WebView事件；该焦点设置本身属于注入，不能证明自动启动焦点、物理按键或系统IME。

剪贴板：原生Edit菜单SelectAll/Copy，已知中文标记实际进入系统pasteboard；原生清空再Paste回输入框，可信copy/paste各1次。原剪贴板全部项目/类型/数据有界保存在内存，完成后逐项匹配恢复，原值没有输出或写盘。新建连接表单只用于已知标记，未保存连接或凭据。

远程编辑：独立新公钥和19273回环Go SSH/内存SFTP夹具，原文件列表打开hello📄.txt，经NSTextInputClient改为已知25字节草稿。菜单Cmd+Q、NSApp Cmd+Q事件和正常NSWindow.performClose分别触发“1个远端文件尚未保存”；每轮取消后草稿/保存按钮保持，独立SFTP快照证明远端仍为原内容。原保存按钮提交后SFTP回读SHA-256为`a5b684b325296a67564a94a836ade91cf251bd50ce8a3642ab73686eca8acb79`，保存后退出不再显示dirty警告。未操作终端或系统认证弹窗。

退出/清理：首个输入实例58045和最终活动会话实例59445均经产品确认实际结束；最终轮确认后约159ms，应用/安装/helper三锁实际重获。后台performClose后当时未显示弹窗，激活后观察到待处理请求，该失败/限制保留；不重复归因为产品缺陷。新夹具59393正常SIGTERM停止，修正夹具关闭传输后再等待通道的清理顺序，所有worker结束，无SIGKILL；11月3日旧夹具超时记录不变。夹具首次因空实例尚无磁盘config.toml而失败，保留日志，初始化独立配置后重跑全部场景。原配置缺失状态已恢复，私钥/唯一信任/socket已清理；剪贴板、发布文档、两个0600未上传副本、原偏好缺失状态和19256/19257保留服务均复核保持。

证据冻结：scratch `native-events-final-evidence.json` / `native-events-frozen-source.json`，独立记录注入库源码/二进制、原始事件快照、成功摘要、历史失败和清理。完整Go/绑定/47项前端门禁沿用相同315份输入已通过的`mobile-files-current-full-check-second.log`，本轮未机械重跑；新Objective-C库与Go夹具均实际编译，产品源码/布局/依赖未变。实体键盘/系统IME、终端原生剪贴板、系统认证、正式升级、代表性远端长连接及Windows/Linux/iOS/移动剩余P5/P6继续开放，整体任务不归档，生产Rust入口保留。

## 已批准规划的M0/M1实施（2026-11-04）

用户已批准P0–P6规划并要求执行。基准排期为三角色配置下12个有效工作周及2周缓冲，条件是首周取得各平台/完整Xcode/签名/真机；实际当前仅由本地代理推进已具备环境的工作，不将建议人员视为已到位。M0基线与资源→M1数据兼容/恢复→P2会话与iOS文件补差、P3实体输入、P4原生能力/包装并行→P5五平台/升级/性能→获准P6切换和Rust退出。正式发布、Git写操作和生产数据接管仍不在本轮操作范围。

本批目标：将单文件互通扩展为完整config/data目录的离线快照与恢复到新目录，保护加密vault/key、known_hosts、更新材料、空目录、未知文件和未上传编辑副本。新增独立Go工具，不增加自动发现正式目录、业务IPC或启动接管入口；现有产品UI/绑定/布局不变。源目录显式指定，必须有可取得的应用锁，已有更新锁也须全部取得；不创建源锁/不改源内容。目标只允许全新目录，不覆盖/合并；失败材料保留且没有完成清单，校验失败不可恢复。清单和备份目录私有，不输出机密、用户内容或完整清单。

实施阶段：

- [x] M0本机盘点：核对工具链/SDK/设备/签名、现有315份冻结输入及本批资源阻塞；不代表五平台环境就绪。
- [x] M1实现：有界完整目录扫描、源锁、快照复制/摘要验证、恢复新目录、显式取消与失败保留。
- [x] M1本机验证：真实Rust/Go兼容、损坏/冲突/并发/取消/链接拒绝、未上传副本保持，统一Go/绑定/前端门禁；目标平台原生检查另列。
- [x] 本批收尾：冻结323份输入/5产物，13项真实CLI及136个文档链接校验通过，用户保留项保持；矩阵和后续依赖已更新，整体任务仍不归档。

M0资源初查：本机Darwin/arm64，Go1.27.3/Node24.22.1及scratch私有Wails3.0.0可用；现有315份产品输入逐项一致。完整Xcode/iPhoneOS/模拟器SDK不可用，有效codesigning身份0，在线Android设备0；SDK36已在，Docker CLI/daemon不可用，本机不能提供Windows/Linux原生验收。原始只读环境台账为scratch `plan-m0-environment.json`。当前不盲写无法链接复验的iOS适配，不把交叉编译当原生运行；优先完成不依赖这些资源的M1。

实现落点：新增`internal/datasnapshot/{snapshot,walk,permissions_*}.go`及`tools/data-snapshot`独立CLI，复用现有flock/文件名策略，不增加依赖或业务IPC。只接受显式绝对路径和已有应用锁；config/data可同目录或独立，不允许嵌套/目标重叠。完整普通文件/空目录保留，运行锁排除；限制8GiB/50000项/64层/16MiB清单，拒绝链接/特殊项/名称及大小写/NFC冲突。源前后清单、锁和目录身份与目标SHA-256回读均须一致，文件/目录刷盘后提交完成清单；临时`.partial`使提交前取消不发布正式标记，已知目录刷盘失败退回未完成名并暴露错误。恢复只到新目录，不覆盖后来新增数据。

权限：Unix目录和可执行文件0700、普通文件0600，不保存原ACL/owner/xattr。Windows对空目标固定身份和句柄，先设置受保护、仅当前用户且可继承的DACL，再回读单ACE/权限/SID；无法设置或确认则在复制前失败。Windows vet通过，包含两个Windows专用ACL测试的测试包编译通过，当前没有Windows原生执行证据。Linux同样只有工具编译。备份包含vault与key，应作为敏感材料保存；清单不认证来源。`pendingUpdate`仅检测task/journal/relaunch存在，原始绝对路径不改写/不执行，不能据此认为新路径可运行更新恢复。

最终本机证据：`plan-m1-targeted-complete.log`共12项顶层测试通过（含CLI参数），实际旧Rust读取恢复并经Go回写后的公开TOML/vault通过；另`plan-m1-interop-complete.log/.exit=0`完整验证storage、目录恢复、双向LAN与WebDAV。`plan-m1-cli-evidence.json`含13项真实子进程检查，三种父进程系统锁均阻止备份，完整字节/空目录/可执行状态恢复、源全部字节/权限保持、不覆盖新数据、篡改预检和错误隐私通过，公开夹具已回收。首次CLI夹具父目录默认0755与恢复规范0700的预期不一致，修正夹具期望后重跑，失败日志保留。

`plan-m1-full-check-complete.log/.exit=0`是所有源码修改后的完整门禁：Go格式/vet/全部internal及工具race、绑定漂移、前端格式/类型/lint/47测试/build通过，session50.504秒/updater199.780秒。绑定/界面/JS695.25kB及CSS均保持，既有npm配置与单chunk告警保留。实际macOS CLI和Windows amd64/Linux amd64/arm64 CLI已重建，另Windows ACL测试包只编译；五份产物摘要在`plan-m1-final-evidence.json`。本批8个新增源码/测试文件，旧315份产品输入仅两份检查脚本变化，最终冻结323份，历史Android/macOS台账不覆盖。

本批不能关闭完整M1/P1或P5/P6：原位置恢复、正式数据接管、真实更新journal恢复、旧正式包覆盖升级、跨平台ACL/系统交互、签名/设备/性能仍待对应环境和后续事务。资源依赖不变：完整Xcode/iOS SDK、有效签名、Windows/Linux原生主机、Docker与移动真机均缺失。后续可先准备与旧注册安装契约一致的Windows Inno打包，但须与实际Windows/UAC/正式签名升级验收分开；Rust生产入口继续保留。

## Windows Inno打包准备（2026-11-04）

范围：新增显式候选包暂存/编译工具与Wails任务，保留正式Rust workflow；不安装应用、不写HKLM、不运行UAC/认证、不签名或发布。旧契约为AppId=KitonyTerms、管理员安装、默认32位安装登记视图、kitonyterms-installation.txt/unins000.exe；Windows目标仅amd64。继续使用默认32位安装模式以兼容旧登记，不因PE为64位而改变安装模式。

阶段：

- [x] 核对旧workflow、Go/Rust安装发现和构建身份；明确NSIS/MSIX不能作为旧Inno更新包。
- [x] 实现声明身份/真实PE的暂存、仅EXE便携ZIP和Inno模板；编译入口只在Windows开放。
- [x] 实际Windows PE交叉构建与本机暂存/拒绝边界、格式/vet/race/绑定门禁；未计为Windows运行。
- [x] 冻结327份输入/5份编译产物/5份暂存材料，142个文档链接和用户保留项通过核验；ISCC/Windows/UAC/签名/旧包升级仍为原生阻塞。

落点：`tools/windows-package/{main.go,main_test.go,inno/project.iss}`及`windows:package:inno:{prepare,verify}`/`windows:package:inno`任务。输入均显式绝对路径，目标只到新目录；PE必须amd64/GUI、Go模块KitonyTerms/GOOS/GOARCH/production且非server。便携ZIP只含EXE，安装标记另存payload；模板沿用AppId/管理员/旧32位登记模式、禁止自动关闭重启和skipifsilent。准备与校验只读取候选程序，不启动GUI/系统认证/安装器；编译只在Windows，先实际--update-info精确比对完整声明身份，再调用显式ISCC，产物/材料再验后才写compiled.json，签名仍未验证。

身份检查纠正：首轮尝试从Go BuildInfo的ldflags校验-X，但实际trimpath PE没有该字段；已核对本机Go pkg.go对应实现并保留`plan-inno-targeted-first.log`。最终清单固定identityStatus=declared，prepare/verify返回prepared/runtimeIdentityVerified=false；不通过字节搜索猜测变量绑定。运行身份须由Windows实际查询确认，本机不计为通过。早期编辑行号落位在编译前读回修正，首次编译通过；无产品源码修正。

当前证据：实际Wails Windows x64候选PE以公开测试身份0.1.18/build602/commit c×40交叉构建，另保留无注入的开发PE，均未执行，不是正式发行。`plan-inno-targeted-second.log`含7项回归及1项子进程助手全部通过/无跳过，27.515秒；验证真实PE暂存、仅EXE ZIP、原文件保持、已有目录、五类篡改、无效声明身份、参数/非PE/FIFO、输出限额与已就绪子进程超时回收。Windows vet/测试包及Windows工具编译通过，目标未运行。

`plan-inno-cli-evidence.json`含8项实际Wails任务/CLI/独立ZIP检查：中文/空格目录prepare/verify、declared标记、实际复制及ZIP SHA一致、标记隔离与旧模板契约、已有材料不覆盖、CLI及真实Wails编译任务均在macOS拒绝且没有新产物。`plan-inno-full-check.log/.exit=0`统一门禁通过：全部internal/tools race及绑定/47前端测试，session52.547秒/updater195.082秒/windows-package30.468秒；UI与资产不变，既有告警保留。正式Rust workflow/Cargo/Dioxus/入口read-only diff均保持。

冻结范围为M1的323份输入，加3份新文件及既有Windows Taskfile，共327份；M1范围内只有check.sh变化，历史M1/Android/macOS证据不改写。最终证据入口为scratch `plan-inno-final-evidence.json`与`plan-inno-frozen-source.json`。本批没有ISCC/Windows原生执行或setup.exe；英文模板的中文资源、完整Windows资源/WebView2、HKLM/UAC/安装卸载/旧包升级和正式签名/发布清单均待P5及对应环境。现有Xcode/iOS/签名/原生主机/真机/Docker资源阻塞保持，实体输入/认证及性能仍未全验；整体P0–P6继续保留，不能据候选暂存切换正式入口。

## Windows候选版本资源与隔离构建（2026-11-09）

范围：补齐Windows候选PE中的图标、双语版本字段和manifest/权限/DPI声明，新增显式候选构建及成品资源回读入口，复用Wails CLI使用的winres v0.3.1。不切换正式workflow、默认安装模式或生产数据，不签名/发布/安装，不把交叉构建及静态资源校验算作Windows原生运行。

方案：BuildInfo作为公开构建身份单一输入；Windows四段数字版本逐段不得超过65535，完整build/attempt/commit保留字符串而不截断。初次overlay实构建失败：Go虽发现虚拟syso，链接器仍打开原物理路径，失败日志/材料保留。现改为在全新私有目录复制明确构建输入（Go模块/根Go文件/internal/前端dist/资源模板，排除测试、node_modules、开发数据及Git），最多64MiB/5000项/64层/单文件16MiB，拒绝链接；资源对象只写私有副本。构建后复验原输入和副本摘要，成品资源回读通过后才写成功记录；失败副本及有界编译日志保留。已有根syso或输出重叠导致歧义时拒绝，不修改原工程。现有模板占位公司/描述字段同步改为真实产品字段。

- [x] 确认资源接口、overlay失败原因、隔离副本方案和当前冻结基线。
- [x] 实现双语资源、显式候选构建/回读与任务入口。
- [x] 实际带资源PE构建、篡改/权限/DPI/版本限制/隔离回归及最终统一门禁。
- [x] 冻结本批输入/产物/证据、保留项核验及迁移矩阵更新；P5/P6仍开放。

当前实现：新增candidate/resource_bounds/隔离输入及回归文件、独立旧产品candidate.ico，模板字段真实化；直接依赖winres v0.3.1，与Wails CLI一致，新增resize/x/image间接依赖。候选资源保留原XML回退，en-US/zh-CN字段包含完整公开身份；验证只证明静态一致，runtimeIdentityVerified始终false。Go环境固定windows/amd64/CGO=0/GOWORK=off/GOFLAGS空，modreadonly/production/trimpath，子进程5分钟/输出256KiB/实际等待，失败保留日志和副本。

实构建：公开0.1.18/build603/commit d×40候选PE为18,720,768字节，SHA-256为`7510dd86452732f84cd5c20ee121cdec99bce0b9690c44ac0fde21e48c31d366`；157份私有构建输入和10项资源回读一致。最终17项工具顶层测试（含1项子进程助手）无跳过，通过29.673秒；8项新增资源/隔离定向测试另通过3.147秒。独立13项PE/CLI/ZIP/隔离检查确认原ICO图像字节、双语数值/完整身份、原XML权限/DPI、完整输入/产物SHA、内容篡改/身份变化/已有目标/重叠拒绝、Inno仅EXE ZIP/标记隔离及非Windows编译拒绝。自查确认winres按目录RVA而非段名读取，现将目录RVA/尺寸与有界.rsrc绑定并拒绝重复地址；三类实际PE头篡改在库读取前拒绝。首轮独立脚本将manifest默认0409误记为0，修正验收预期后完整重跑，首日志保留；不是产品缺陷。

最终门禁`windows-resources-full-check-final.log/.exit=0`：目录/段绑定修复后的全部Go格式/vet/internal+tools race、绑定漂移、前端格式/类型/lint/47测试/build通过，session50.086秒/update203.859秒/windows-package30.667秒；第一轮完整门禁另行保留。JS/CSS仍为index-Cc62zd-7.js/index-DwXO_pTh.css；本批产品运行/界面源码未变。Windows全模块vet及工具/测试包仅编译，macOS完整应用直接Go构建及只读--update-info零数据创建通过；该编译不是官方包或GUI验收，当前对象macOS26.5/链接11.0警告、npm配置和单chunk警告保留。Windows运行身份/GUI/ISCC/中文安装语言/WebView2/签名升级与整体P5/P6继续开放，Rust生产workflow/入口保留。

保留核验：本轮首次只读审计时历史PID65386/52913已经不存在，19256/19257无监听且HTTP不能连接；未执行停止、重启或清理，不沿用历史“服务仍运行”的结论。发布文档SHA、两个0600未上传副本字节和原偏好不存在保持；历史冻结/结果独立保留。临时篡改CLI目录已移除，新的公开候选/隔离副本/Inno暂存和失败overlay材料保留，不安装、不签名、不发布或接管数据。

冻结入口为scratch `windows-resources-frozen-source.json` / `windows-resources-final-evidence.json`，最终collector退出0：335份输入、5份编译产物、17项工具测试、13项独立检查和148文档链接。相对327份Inno基线仅Taskfile/Go依赖两文件/工具main变化，另增加8份资源/源码冻结项；40份历史结果/冻结、旧Inno日志与5产物、用户保留材料均逐项保持。新交接为scratch `continue-after-windows-resources.md`；本批结束，整体迁移任务保留在当前索引，不归档。

## 整体迁移继续：macOS候选打包与平台阻塞（2026-11-09）

用户再次要求继续完成整体迁移。当前335份冻结输入/用户发布文档保持；Linux完整桌面CGO=0实构建失败于Wails原生pointer定义，活动主机为Darwin/arm64且Docker CLI不存在，不能用server/stub或交叉领域测试冒充Linux桌面包。Linux CGO/sysroot或原生主机条件未具备时，继续本机可验证的macOS候选链，整体P5/P6不提前关闭。

本批方案：复用有界私有构建输入复制，避免重复实现；macOS候选以公开BuildInfo单一输入构建ARM64 production，明确12.0声明与最低系统实际兼容尚未验证的边界。旧产品ICNS作为独立候选资源，plist补齐真实产品字段和正确布尔类型，版本/完整身份与实际--update-info回读一致。只在全新输出中准备.app/DMG并验证包内身份、资源/字节、测试签名及只读挂载/卸载；不安装或启动业务GUI、不访问生产数据、不正式签名/公证/发布。默认Rust workflow及其它既有产物保持。

- [x] 核对剩余矩阵、335份冻结基线和Linux实际构建阻塞，选择本机可验证的独立工作。
- [x] 提取共享构建输入复制并接线macOS候选身份、plist、图标与显式任务。
- [x] 真实候选.app/DMG与身份/资源/挂载回读、篡改/取消/边界回归、受影响Windows检查及统一门禁。
- [x] 冻结本批证据、恢复/清理仅本批资源、保留历史及用户改动，更新整体平台门禁；P5/P6继续按实际条件推进。

实现：新增 [共享复制器](../../app/internal/buildinputs/inputs.go)，Windows保留原包装与回归；64MiB/5000项/64层/单文件16MiB、固定根/ReadDir128、前后身份/SHA与0700/0600，额外资源为调用方固定build相对路径，规范化父目录拒绝别名重叠。新增 [macOS工具](../../app/tools/macos-package/main.go) 和 [Darwin任务](../../app/build/darwin/Taskfile.yml)，不运行默认删除旧包的任务。旧ICNS单独复制，SHA-256为`47170eb1a03f32bab617843e2df2a299c2b97ef2c0e0fe37b74054922521fe55`；默认plist的占位产品信息/字符串布尔修正，候选plist由单一BuildInfo生成并保留完整公开JSON，不截断build/attempt。

实构建：公开stable0.1.18/build604/attempt1/commit e×40，157份私有输入、8项产物摘要。ARM64程序16,396,096字节，SHA-256为`7d6c1f9cb2cf1ee1a7f6c65de34d56ae6d2c9bbc8d1d66f142594c52832fc1ac`；DMG 7,772,926字节，SHA-256为`6972bcd91c4285cb4b8881749c6efa3238710f9a88750ec64d23a64bfd8c1541`。实际Mach-O为macOS/ARM64/12.0声明/SDK26.5.0；仅本地ad-hoc strict签名，直接和只读DMG内--update-info精确身份一致且未创建隔离业务目录，包四文件摘要一致并实际卸载。未启动业务GUI、未安装/正式签名/公证/发布。

自查收紧标准库读取前的Mach-O加载命令/段/符号/重定位范围与累计符号名限额；包目录使用固定白名单和ReadDir16，避免无界枚举。最初syscall.MNT_RDONLY编译失败改用项目已有x/sys/unix，未增加依赖。最终28项顶层race无跳过：共享复制2/macOS工具9/Windows工具17（含1助手），覆盖完整大身份、plist类型、错误参数/重叠/链接/输入上限、真实子进程输出/超时/取消、失败材料/不发成功记录、真实Mach-O/包篡改与只读镜像失败/取消/外来目录保留。另15项独立Python plistlib/Mach-O/系统签名/真实CLI/实际只读DMG回读卸载检查通过，临时篡改材料已回收。

共享提取后实际重建Windows候选：158份输入/10资源回读，依旧static verified/runtime false；Windows全模块vet和两工具/两测试包交叉编译通过，Windows载荷/ISCC未执行。最终`macos-candidate-full-check-final.log/.exit=0`：Go格式/vet/全部internal+tools race、绑定漂移、前端格式/类型/lint/47测试/build通过；session51.264秒/updater201.133秒/macOS工具13.905秒/Windows工具31.494秒。原布局、绑定和前端资产保持。Go对象13.0链接12.0、重复objc、npm minimum-release-age和Vite单chunk警告保留；12.0声明不证明最低系统实机兼容。

冻结为scratch `macos-candidate-frozen-source.json` / `macos-candidate-final-evidence.json`：349份输入、8份编译产物及实际DMG、28项定向测试/15项独立检查；相对335基线仅Darwin plist/Taskfile/Windows复制包装变化，另14份新增资源和源码。82份历史证据、旧Windows/Inno产物及两份0600未上传副本/发布文档逐项保持，preferences仍不存在，原app根无syso；旧19256/19257仍无监听，未停止或重启。交接为scratch `continue-after-macos-candidate.md`。

整体迁移在本历史检查点未完成：Linux需要真实CGO/sysroot或原生主机/Docker，iOS活动SDK仍缺失且文件适配未完成；Windows原生GUI/agent/认证/ISCC/UAC/ACL/升级、macOS实体输入/认证/最低系统/正式包升级、Android真机/更多provider/后台/低内存/x86/实体扫码、签名/数据接管/恢复/性能及P5/P6继续开放。该段记录的是2026-11-09前的历史状态；当前项目Rust入口已退出，GitHub发布收尾见261110任务。

## Windows 11 ARM64 隔离执行验收（2026-11-09）

发现本机既有Parallels Windows 11 VM，原状态suspended；恢复后以当前用户执行，客体ARM64/10.0.26200.0，无Go/ISCC。此前M0“无Windows执行环境”已被本轮证据更新。ARM64原生执行与amd64的x64仿真分开记录，未据此关闭原生amd64硬件、GUI/DPI/认证/安装升级门禁。

- [x] 记录原状态、当前用户执行/架构及工具；不使用密码或用户agent。
- [x] 执行DACL、GDI字体、适用快照与更新器测试，修复真实失败并完成整包复测。
- [x] 重建最新源码Windows/macOS候选，验证运行身份/零数据查询及独立暂存摘要，完成最新统一门禁。
- [x] 确认自有子进程结束、删除带归属标记的客体目录、停止传输服务并恢复原挂起状态。

传输：Host Shared Folders原为关闭，设置未改；共享路径失败、长EncodedCommand/EOF等待失败均保留为夹具问题，不计产品验收。最终使用短固定stdin脚本入口加临时令牌/文件名白名单HTTP，每份公开二进制/夹具CreateNew并逐项SHA-256回读。测试名触发Windows安装器启发式后，只给scratch测试副本嵌入asInvoker清单，实际PE回读且全部非资源段字节相同；未提权或改产品manifest。

实测修复：原目录/日志零访问句柄不能约束删除共享，改GENERIC_READ并验证日志仍可写、路径不能删除/换名；无扩展名payload经os/exec会改选邻近.exe，改os.StartProcess显式绝对应用路径，保留输出限制/取消Kill+Wait及管道读取加入。映像退出后Root.RemoveAll首次返回5、103ms重试成功的诊断保留；回收增加3秒内拒绝/占用等待，每轮复查SameFile，身份变化保留材料。一次后续完整包在verified回滚时短暂拒绝误入提权，20轮独立诊断未稳定复现；替换将access_denied纳入原3秒等待，新增真实只读解除/持续只读/取消回归通过，未运行真实UAC。

最终执行：ARM64 DACL 2项、GDI 7项、适用快照10项全部通过；Windows更新专项8通过/1跳过。amd64更新器在x64仿真下完整78通过/5跳过/0失败，另1个stage_link子项跳过。链接权限仅确切ERROR_PRIVILEGE_NOT_HELD(1314)跳过，别的错误仍失败；Unix权限夹具跳过，不把这些分支记为Windows安全验收。客体快照未执行Rust回读，既有macOS真实Rust互通证据独立保留。

最终公开测试身份：Windows0.1.18/build609/4×40 commit，161份复制输入，EXE 18,732,544字节/SHA-256 `8b4b509742296cf13c150df8f94829edb56da5f0dc89345c65007b86ad8ba98b`；实际完整--update-info/零数据、中文空格prepare/verify、仅EXE ZIP/标记、已有输出与缺少ISCC拒绝通过。客体与主机独立生成5份材料摘要一致；候选/prepare静态记录仍保留declared/runtime未验证，实际运行证据另存，未生成setup.exe。

macOS0.1.18/build610/5×40 commit，160份复制输入；可执行文件16,396,096字节/SHA-256 `4808150ce94843bc4b7f1ad26e2546efbad17273b6698e6ba659fd6b54e6afd1`，DMG 7,772,817字节/SHA-256 `87430220b650b17f0289f084e0e48692c87278779483d9025a085588ea54a40e`。本地ad-hoc签名、实际身份/零数据及只读DMG回读/卸载通过。最新`windows-native-release-full-check.log/.exit=0`覆盖全部Go/vet/internal+tools race、绑定漂移、前端格式/类型/lint/47测试/build；前端源码/布局/JS/CSS保持。最低macOS12实际运行、正式签名与升级未验收，既有链接/单chunk警告保持。

证据入口为scratch `windows-native-final-evidence.json` / `windows-native-frozen-source.json`，新收集器校验349历史输入加3个平台适配、7份实际执行产物、候选全部文件/原输入与副本、最终退出码及旧证据摘要；旧冻结和中间失败/产物不改写。`windows-native-cleanup-final.json`记录客体自有进程均结束、未强杀、准确目录删除，临时HTTP进程退出0且端口释放，VM身份核对后恢复suspended，共享设置不变；发布文档/两份0600未上传副本/偏好原缺失和工程根无syso已核验。

本轮完成Windows隔离执行阶段，整体迁移继续：Windows原生amd64硬件/GUI/DPI/agent/认证/ISCC/UAC、Linux原生CGO/sysroot、iOS完整SDK/文件适配、各端实体输入/真机/最低系统、正式签名升级、原位置数据恢复/接管和性能/长稳仍开放。生产Rust入口保留，无Git写操作、正式签名/发布、已安装应用替换或生产数据接管；整体任务不归档。

## Windows agent 可取消连接与私有管道验收（2026-11-09）

继续已批准迁移，优先关闭可在既有ARM64 VM独立验证的Windows agent缺口。现有实现直接os.OpenFile系统管道，只有打开前取消检查；Wails固定版本已声明go-winio v0.6.2，源码对比v0.6.4补齐监听器关闭竞态。方案采用固定go-winio v0.6.4的DialPipeContext/net.Conn，不自建管道I/O、句柄、超时/关闭层；产品仍默认Windows OpenSSH固定管道。

每个Manager私有持有agent连接器，产品构造时固定默认实现，测试在连接前注入专属管道；无新增IPC、环境开关或全局可变连接器。认证/转发/跳板仅目标/Drain复用现有跨平台流程，Windows夹具使用仅当前用户DACL、随机新管道和内存生成密钥，不能访问系统agent/用户密钥/正式连接。原桌面布局/生产入口不变。

- [x] 固定成熟管道依赖并接线有界可取消拨号，复用认证/转发/取消测试流程。
- [x] 私有管道覆盖精确Win32错误、忙碌连接取消、读写/期限/Close与实际资源回收。
- [x] 执行Windows ARM64原生与amd64仿真专项、受影响回归和最新统一门禁，编译不替代执行。
- [x] 冻结新证据、清理本轮资源并恢复VM原挂起；历史352输入冻结和用户材料保持。

Windows OpenSSH系统服务互通、原生amd64硬件、GUI/系统认证/UAC/ISCC及整体P5/P6仍需各自验收；生成密钥的agent协议夹具不证明系统服务或正式生产连接。

依赖明确固定go-winio v0.6.4，其最低依赖使x/sys从v0.48.0升至v0.49.0；Wails保持v3.0.0。首轮两种架构各4项agent顶层测试/11个子项无跳过通过。扩展回归首次各9通过/1失败，DACL 2项通过；失败为SFTP取消后恢复夹具将本机Windows盘符路径当作远端路径，改读已配置的远端工作目录“.”，断言仍检查取消后可重开且SSH保持。服务就绪前驱动启动的失败与修正前日志均保留，不计产品验收。

最终ARM64原生与amd64仿真各10项顶层/18子项通过、0跳过/失败，包含agent四项及Drain/SFTP/背压六项；DACL 2项通过，升级依赖后的完整amd64更新器78通过/5跳过/0失败，另stage_link子项因1314跳过。`windows-agent-full-check.log/.exit=0`的Go/vet/internal+tools race、绑定漂移及前端47测试/build通过；之后只有两处测试远端路径字面量修正，`windows-agent-session-final-build.log/.exit=0`另证macOS定向race 8.830秒、两种Windows重新编译及实际10项回归通过。未新增前端或业务测试开关，页面与资产保持。

新公开测试候选Windows0.1.18/build611/6×40 commit，161份输入，EXE 18,784,256字节/SHA-256 `4468ab6b6d6c5bae1e9b08408e213152489595546ae06b7e2c6cf913c245852b`；实际完整身份/零数据、中文空格暂存/ZIP/标记/已有输出/缺少ISCC拒绝、5份主客体独立摘要一致。macOS/build612/7×40 commit，160份输入，可执行文件16,396,096字节/SHA-256 `ab507a58dd8f4617ce820b2e1d84650913ddc739a20018127628a924ac9b6cf1`，DMG 7,772,474字节/SHA-256 `bc42e31f1d98a3c19cb666a5bf2cc30b65ada90ae679e8d0ae85fb4f0500e4be`；本地ad-hoc签名/身份/零数据/实际只读镜像回读和卸载通过，正式签名与最低系统仍未验收。

scratch `windows-agent-cleanup-final.json`确认自有进程结束/未强杀、归属目录删除、两轮HTTP服务退出0/端口释放、准确VM恢复suspended及原共享设置保持；发布文档/两份0600未上传副本/偏好原缺失与原工程无syso核对通过。新冻结为`windows-agent-frozen-source.json`/`windows-agent-final-evidence.json`：替换旧Unix单文件测试为共享/平台夹具，明确记录移除路径与新增文件；旧352冻结、609/610与更早证据不改写。交接为`continue-after-windows-agent.md`。本阶段完成，整体任务/P5/P6不归档，无Git写操作、发布、系统agent访问或生产接管。

## Rust/Go 更新记录与恢复互通（2026-11-09）

继续已批准迁移，在独立验证工具中接入真实kt-update的Journal/InstallTask序列化、recover/rollback及installation.lock，不给产品增加测试入口。只使用新建带归属标记的夹具根、固定install/kitonyterms和updates子目录，生成公开字节/测试程序；无自动发现、原位置接管、生产更新或正式签名。

- [x] 核对Rust/Go公开字段、缺省值、两种布局、阶段与安装锁契约。
- [x] 补充受目录归属约束的Rust工具命令，验证Rust→Go与Go→Rust的记录和真实恢复、确认/拒绝与跨进程锁。
- [x] 执行真实互通、Rust工具格式/clippy/build、Go race及统一门禁；明确本机函数/进程证据和正式升级/helper握手的边界。
- [x] 记录结果、冻结新输入与证据，保留354输入及既有候选/用户材料；整体P5/P6继续开放。

实施顺序：先让真实Rust类型读写和恢复受控目录，再由Go测试调用它；Rust→Go覆盖legacy/user_directory及旧缺省字段，Go→Rust使用真实Go安装引擎落盘的prepared/backed_up/committed边界。确认前保留、错误路径/坏备份拒绝和恢复幂等分别断言；跨语言读取不等同正式发布升级或完整helper握手。

首轮6顶层/24子项真实race通过；自检后归属标记先校验普通文件/确切长度再有界读取，补充链接/超长标记拒绝与取消原因断言。最终`update-interop-complete.log/.exit=0`的6顶层/26子项、Rust fmt/clippy/locked构建及原配置/vault/目录恢复/LAN/WebDAV全流程通过，更新专项19.897秒。离线解析初次附带降级四个既有间接依赖，已以precise恢复原combine/redox_users/libredox/redox_syscall版本；原锁定版本全部保持，仅增加kt-update所需依赖/特性，原生产Cargo.lock未改。

统一门禁首轮因本轮长TMPDIR使已有agent Unix socket超出平台路径上限，session夹具invalid argument，日志保留；移除该覆盖后`update-interop-full-check-final.log/.exit=0`完整重跑通过：Go格式/vet/全部internal+tools race、322包/76方法/65模型绑定无漂移、前端格式/类型/lint/47测试/build。独立编译的macOS race测试程序已实际执行6/26无跳过通过，Windows amd64/Linux amd64+arm64测试包仅编译；本轮没有Windows/Linux跨语言执行结论。

新冻结为scratch `update-interop-frozen-source.json`/`update-interop-final-evidence.json`，385份输入：沿用354份基线，仅interop.sh改变，另纳入31份Go集成测试、独立Rust工具、根清单/锁/工具链和四个真实crate的源码/构建脚本；2份实际执行产物及3份仅编译产物分别记录。原生产Cargo.lock与Rust入口/发布流程保持，独立验证锁新增26包且未移除既有锁定版本；完整helper协议、task/准备载荷最终回收、bundle/安装器、原生GUI确认和正式旧包升级未据此关闭。

Windows611/macOS612的161/160份生产输入及副本逐项保持，无需重建；前端源码/绑定/JS/CSS、既有历史冻结/日志/候选与用户发布文档/两份0600未上传副本保持。`update-interop-cleanup-final.json`记录本轮已知工具进程退出、空临时目录回收、互通临时目录不存在；VM本轮未启动，只读复核仍suspended、原共享设置保持。交接为`continue-after-update-interop.md`。本轮实现与验证完成，整体任务/P5/P6保留开放，不归档、不切换生产入口，无Git写操作、安装或发布。

## 原 Rust helper 与 Go 发起端隔离互通（2026-11-09）

上一轮385输入冻结保持。本轮复用原`kt-app/update_launcher.rs`和`single_instance.rs`，在独立Rust验证二进制中只适配数据目录发现：仅允许新归属夹具根内的固定updates/update-helper及install/kitonyterms，拒绝链接/重解析点、外来路径和已有journal，不读取系统应用目录。Go测试使用真实spawnApplyHelper与原Rust内部命令；不复制握手算法、不增加产品入口。

- [x] 核对原Rust路径发现、构建/摘要/任务绑定及Go发起端限制，确定独立验证边界。
- [x] 实现受限目录适配与就绪/取消/父进程存活、错误身份/坏包/缺失归属回归。
- [x] 实际执行macOS ARM64互通、Rust工具检查和统一门禁；只对执行范围作结论。
- [x] 更新矩阵、冻结新源码/证据并核对用户材料、候选与资源保持。

本轮只验证Go发起→原Rust helper的准备/握手/取消方向，commit后仍须父进程退出；不验证安装成功、完整双向helper接力、原Rust系统目录发现、bundle/GUI重启或正式旧包升级。

实际结果：原helper 2顶层/5子项通过；首轮完整互通/门禁通过后，自检标准cargo test发现独立工具缺少tempfile开发依赖，`helper-interop-cargo-test-first.log/.exit=101`保留。补齐开发依赖后，最新`helper-interop-complete-final.log/.exit=0`的Rust fmt/clippy/test/locked构建、原launcher与单实例锁10项Rust单元测试、配置/vault/目录/LAN/WebDAV及更新8顶层/31子项全部通过，0跳过/失败，更新专项19.651秒。`helper-interop-full-check-final.log/.exit=0`完整Go/vet/internal+tools race、322包/76方法/65模型绑定无漂移及前端格式/类型/lint/47测试/build通过，更新包230.776秒；保存的macOS race二进制另以`artifact-execution-final`实际执行2/5通过。旧轮次和npm/Vite警告保留，页面资产保持。

新增仅验证工具的macOS libc直接依赖和tempfile开发依赖，独立锁无包新增/移除或版本变化，仅验证包依赖列表改变；原launcher/单实例锁以path模块直接编译，rustfmt显式跳过其原文件，生产Rust/根锁/发布流程未改。新冻结为scratch `helper-interop-frozen-source.json`/`helper-interop-final-evidence.json`，389输入=385基线+4文件（helper工具、Darwin回归与两份原Rust源），基线仅Cargo清单/锁、共享fixture校验与interop.sh四项变化；与最终门禁启动前`helper-interop-tested-source-final.json`一致。3份显式保存且实际执行的产物分别为通用Rust工具、受限原helper与Go race测试，未增加其它目标执行结论。

`helper-interop-cleanup-verified-final.json`在最终补验后复核已知工具/夹具进程退出、interop/check临时目录不存在，VM本轮未启动且仍suspended/共享保持；Windows611/macOS612的161/160份生产输入/副本、旧候选/历史证据、前端与用户保留材料均保持。交接为`continue-after-helper-interop.md`。本轮完成，整体迁移及P5/P6仍开放，不归档整体任务、不切换生产入口，无Git写操作、安装或发布。

## 平台范围收敛与本机运行验证（2026-11-10）

用户新策略取代旧平台门槛：Android仅保留ARM64，移除x86/x86_64构建和模拟器适配；macOS停止12–14兼容，当前部署下限收敛为15.0，宿主27.0执行单列；iOS仅交付不经App Store的无签名设备IPA配置，无开发者账号不作为打包前提，实际设备安装仍需侧载签名。本机缺完整Xcode/iOS SDK，真实iOS链接/可运行IPA不能声明通过。iOS原生文件功能及真机不再作为本轮打包配置验收项。

阶段计划：

- [x] 读取现有方案、平台入口和本机环境，保存用户材料与生产入口摘要。
- [x] 移除Android x86目标和Gradle打包路径，统一macOS部署/包声明/候选校验，新增iOS构建号/设备IPA预检与回读。
- [x] 实际构建并运行macOS ARM64、Android ARM64及既有Windows ARM64 VM可验子集，记录基本功能、架构与失败。
- [x] 完成相关自动检查、自审、资源回收和平台矩阵；整体迁移按未满足的生产门槛继续开放。

约束：只读现有依赖，不安装系统工具，不改生产Rust/CI，不接管正式数据；原候选/历史冻结不改写。Android使用新的独立测试实例和只读AVD，Windows测试后恢复挂起。Linux无原生CGO/GUI环境，iOS无SDK，明确跳过其原生运行；server和模拟器/仿真结果分别记录。

本阶段结果：

- macOS ARM64当前production/cgo构建、15.0加载声明、614公开测试候选本地签名/完整身份/零数据/真实DMG只读回读通过。宿主27.0的实际WKWebView核对当前资产、1200×800无溢出、Info/Config/Sessions IPC、252系统字体、设置保存/版本变化/恢复、连接表单及产品退出弹窗/确认后退出0。原生探针/DOM动作不代表实体键盘或系统认证。Intel macOS当前minos15.0交叉编译通过；Rosetta探测Bad CPU type，跳过运行。Docker模板已声明15.0，五段shell语法通过，容器未执行。
- Android使用195份当前私有副本输入，ARM64 Go共享库及离线lint/testDebugUnitTest/assembleDebug/bundleDebug通过，lint 0错误/20警告，单元任务NO-SOURCE。APK 28,671,268字节/SHA-256 `ecc0028908c9894cf0c9686275967a23c5faa6e7da90257271134220417947a8`；AAB 15,540,913字节/SHA-256 `ea028a5a6a7901323b66e099bedb74945adc4708861f6ec36b0d6c9162f50622`，均仅arm64-v8a，APK v2测试签名/ZIP16KiB/ELF四段LOAD0x4000与包内字节通过。独立`.wailsdev.policy`在只读API35模拟器13项通过，覆盖冷启动/当前资产/真实IPC/布局/配置/连接表单/设置控件/HOME/BACK/同PID恢复/安装摘要/force-stop；未重跑历史SAF/相机全套，不计真机、后台SSH或API36。
- Windows两架构当前PE构建与真实只读身份通过；既有ARM64客体内原生ARM64及x64仿真各2字体/6存储/4私有agent顶层与11子项无失败/跳过通过。ARM64实际窗口与WebView2进程/环境创建确认，但调试端点超时，GUI表单/IPC未验收。缺少Go/ISCC，未安装工具或修改共享/权限；客体仅本轮归属目录已删除、进程归零，VM恢复suspended。
- iOS新增独立无签名设备IPA入口，构建号1–2100000000、arm64/LC_BUILD_VERSION设备平台/plist/非空资源/签名排除/Payload字节回读；SDK路径按cgo参数转义。26项合成结构/拒绝检查通过，实际task明确因无iPhoneOS SDK提前失败且不创建输出；未生成实际应用IPA，不以合成夹具替代。无开发者账号不阻止该配置，设备侧载仍需签名；Linux无CGO/GUI/Docker、Intel macOS无Rosetta分别跳过。

统一`check-final.log/.exit=0`：Go格式/vet、internal/tools race（session50.640秒、update199.860秒、macOS候选工具13.448秒）、322包/76方法/65模型绑定无漂移、前端格式/类型/lint/47测试/build。最后iOS脚本局部修正补语法/26项结构及实际SDK拒绝检查；后续仅Docker/文档变化，不重复无关领域测试。前端/绑定/依赖和布局保持。

驱动问题保留：Android副本首缺build/Taskfile、短visibility等待、Windows GUI PE立即读取LASTEXITCODE/参数拆分/缺少Rust夹具、macOS plist格式自动探测/空AX树映射、premain修改环境及重复导入runtime。修正对应驱动后分项重跑；macOS最终固定fetch复用现有IPC并确认退出0，早期准确强制清理记录不改写。WindowsGUI端点超时仍是验证限制，不称产品白屏或已通过。

本轮资源回收：独立Android进程已force-stop、准确19278转发及只读模拟器退出，5560/5561/19278无监听；临时测试签名密钥和Gradle注入脚本删除。当前macOS实例及私有socket已结束，最后一轮正常退出0，早期强制回收另记；Windows本轮目录/进程回收并恢复VM挂起。证据保存于scratch `platform-policy-20261110/`；原Windows611/macOS612候选、历史冻结与用户保留材料不重写，最终摘要核对另记。整体迁移/P5/P6继续开放，不归档整体任务，不切换生产入口。

收尾核对：受保护80份输入摘要未变，Windows611/macOS612的7/8份产物与161/160份私有构建输入全部保持；174个当前本地文档链接通过，关键VM共享字段与原基线一致。两个未上传副本继续保留，临时签名材料/转发/进程已回收。本阶段检查完成；真实iOS链接、Windows GUI、Linux桌面、Intel运行及整体生产迁移门槛分别保留，不将本阶段完成解释为全迁移完成。
