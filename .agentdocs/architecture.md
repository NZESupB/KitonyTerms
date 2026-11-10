# 架构与关键约束

## 生产边界

唯一工程为[app/go.mod](../app/go.mod)，Go/Wails v3.0.0与React18/xterm.js/Zustand。入口[main.go](../app/main.go)区分桌面/移动，桌面[native_desktop.go](../app/native_desktop.go)在GUI前处理更新内部命令、取得数据锁和恢复守卫。旧Rust/Cargo/Dioxus工作区已退出；历史源码以旧tag为准，当前构建/验证不需要Rust。

| 模块 | 职责 |
|---|---|
| [ipc](../app/internal/ipc/backend.go) | Wails边界、生命周期与业务准入 |
| [session](../app/internal/session/manager.go) | SSH/agent/ProxyJump、PTY、SFTP及独立操作通道 |
| [storage](../app/internal/storage/config.go) | TOML、vault/key、known_hosts与原子保存 |
| [remoteops](../app/internal/remoteops/commands.go)、[monitor](../app/internal/monitor/parse.go) | 有界固定命令、解析和监控 |
| [syncconfig](../app/internal/syncconfig/envelope.go) | WebDAV和LAN v2，仅非机密配置 |
| [update](../app/internal/update/desktop.go) | 固定官方源、归档、事务/helper/恢复与安装身份 |
| [datasnapshot](../app/internal/datasnapshot/snapshot.go) | 显式离线新目录快照/校验/恢复 |
| [frontend](../app/frontend/src/App.tsx) | 界面编排，展示状态与局部草稿分离 |

## 会话、取消和数据

- 所有事件以连接generation和request/operation/exec ID归属，迟到结果不能覆盖新请求。通道有界、终端输出有界重放；宿主PTY、容器PTY、SFTP与操作exec分别拥有生命周期。
- 单跳ProxyJump按会话独占外层传输；取消等待真实OpenChannel结束，关闭先断外层再关转发通道，避免写背压互等。正常SFTP取消保留宿主SSH，完全停滞时允许回收连接。
- Windows agent固定go-winio v0.6.4/x/sys v0.49.0，10秒上限与context；Manager构造时私有连接器，产品仍使用OpenSSH命名管道。测试仅随机当前用户管道/临时socket/生成密钥，不访问用户agent，不提供运行时覆写入口。
- Config/KnownHosts先落盘再更新内存；主机首次信任必须保存成功。vault为Argon2id+XChaCha20Poly1305 KTVT格式，每安装独立key；旧固定key自动迁移，不能打开的旧主密码vault先备份再重建。
- [platform路径](../app/internal/platform/paths_desktop.go)按已验证的构建身份选择生产/开发。生产严格沿用旧ProjectDirs：macOS配置/数据同为Application Support/com.kitony.KitonyTerms；Windows两者均Roaming/kitony/KitonyTerms下config/data；Linux小写kitonyterms遵循XDG。无身份开发隔离；显式DATA_DIR使用根下config/data；回归不触碰真实生产数据。
- 机密查看经[secretview](../app/internal/secretview/reader.go)串行认证，读取前后复核context、完整目标身份和退出；系统认证不缓存成功。macOS新LAContext，Windows绑定HWND的Hello/明确不支持时账户密码+LogonUser/SID校验；取消/认证失败不自动降级。同步系统对话框实际返回前不释放门禁。Linux/server/移动端明确拒绝。
- 前端凭据查看只留局部内存，与编辑/保存分离，30秒或身份/可见性/退出变化即清除。配置同步不携带vault、key、known_hosts、更新偏好、运行时状态或编辑副本。

## 文件、终端与运维

- 下载先私有暂存；上传同目录临时文件提交并保留权限，替换失败尝试恢复备份，不先删正式文件。内置编辑限UTF-8/1MiB，失败保留内容与副本。批次先校验清单，拒绝链接/特殊文件/名称碰撞，保留空目录，总量不确定不得伪造100%。
- 外部编辑以请求ID绑定下载/监听/回传，结束仅回收该副本与空会话目录，未上传材料保留。Unix暂存0700/0600。
- Android SAF仅使用授权URI，无新增存储权限；Java内存保留URI，私有暂存有界。导出实际关闭并SHA回读后才成功；取消/Activity解绑等待描述符与原生请求结束。API35使用句柄Chmod和O_EXCL私有提交，避开fchmodat2/seccomp与禁硬链接；只删除归属明确且本次未完成的外部文件。
- xterm.js负责终端缓冲/显示/选区，禁止从DOM字符串推断内容；OSC7目录和命令历史由有界流解析，保留用户shell hook，历史只内存最近200条、不自动执行。宿主/容器终端分别重放、resize、清理和代次收敛。
- 运维固定命令+校验参数，独立有界exec，不污染PTY；失败保留旧快照，当前详情和父列表上下文独立。Docker采集仅可见页，10秒清单/2秒指标/60–300秒大小TTL；慢查询单worker、每批≤64ID、前台优先，错误指数退避，取消/权限/解析待显式重试。镜像/rootfs/可写层不相加为独占大小。

## UI与移动生命周期

- 保持36px标题、默认220px侧栏、48px操作栏、58px页脚、200px历史；设置760px/168px分类。手机16px输入/44px控件、列表详情导航，safe-area与软键盘独立处理。
- `frontend/bindings/`只由Wails生成；IPC DTO在model，设置/编辑/同步/更新代理草稿留局部，保存使用revision并在失败时保留草稿。主题/字体/界面缩放和终端字号各自拥有单一入口。
- [fonts](../app/internal/fonts/catalog.go)返回本机名单和来源/原因：CoreText、同步GDI独占回调、固定argv有界fc-list；设置局部按实例加载/取消，未知字体原样保留。UI单字体安全CSS引用与终端字体栈分别序列化。
- Android Application持有once-only Go初始化，Activity只绑定/解绑WebView；后台不关闭Go服务，BACK先历史后moveTaskToBack。旧实例和队列按身份拒绝；解绑释放传感器/原生请求，恢复重读Info/Config/Sessions。进程死亡后内存分享、草稿与保护任务不恢复，配置revision重建。
- 相机仅应用来源视频请求；隐藏/关闭/迟到权限必须结束轨道，定位明确unavailable。iOS文件交换仍ErrUnavailable。

## 更新、快照与发布

- BuildInfo仅构建时注入，固定官方仓库，开发禁用更新；stable按SemVer，alpha按run/attempt。版本来源frontend/package.json；六桌面target明确，移动端无桌面更新入口。
- ZIP/TAR/DMG同句柄复验摘要，原始反斜杠/路径穿越、重复/Unicode碰撞、链接/特殊文件和展开体积有界。DMG只读挂载，包内相对链接延后验证，codesign/bundle ID/精确update-info一致，成功失败都回收挂载。
- task/journal/relaunch严格兼容旧字段/缺省storage，坏记录不覆盖。prepared/backed_up/committed在锁内刷盘推进，保留旧备份，匹配路径/构建的页面Info/Config/Sessions渲染确认后才清理；开发不可接管正式journal。
- helper绑定自身路径/构建/摘要、任务、原目标和包，PID/摘要ready仅就绪；必须原进程退出且commit才替换，abort优先。恢复与替换同一持锁协调器负责，失败保留材料。Prepare/Commit/Abort绑定ID，取消等待真实任务回收，提交响应不明不重试或猜测取消。
- Windows固定目录/输入句柄拒绝reparse/hardlink，Shell/Inno边界安全简化路径，内部仍规范路径比对。32/33/1224占用按预算重试、不提权；明确只读不提权；仅首次创建权限失败允许一次runas。管理员只replace/restore，不执行新载荷或启动GUI，普通owner等待实际退出后重开。
- [Drain](../app/internal/session/drain.go)真实worker计数，超时恢复准入，成功release仅本轮有效。准备时所有文件/保存/选择器/认证操作先加入同一准入门禁，原生调用未返不提前释放。
- 快照仅显式绝对源、新目标，取得已有应用/helper/安装锁但不补源锁；完整普通字节/空目录/未知/vault/key/编辑/更新材料，运行锁不复制。8GiB/50000项/64层，源前后/目标摘要核对刷盘。Unix0700/0600，Windows写前当前用户受保护DACL回读；不认证来源，不执行旧journal绝对路径，不覆盖恢复。
- [发布workflow](../.github/workflows/release.yml)固定Go1.27.3/Node24/Wails3.0.0，不安装Rust。六桌面原生runner、Android仅ARM64 SDK36/NDK28.2/JDK21、macOS最低15，iOS完整SDK链接无签名IPA。CLI、模块和runtime不得单独升级。
- Windows资源和macOS包工具在新私有输入副本构建，buildinputs固定Root/边界/前后摘要，资源回读不等于运行；发布另确认实际update-info。macOS ad-hoc/signature/DMG挂载回读，不宣称正式签名公证；Windows ISCC实际执行，不宣称Authenticode。
- 六份构建记录、八桌面包、三移动包、八verification记录、manifest和SHA256SUMS共27项严格集合；一致commit/run/attempt与包大小/SHA后才创建独立预发布，禁止覆盖稳定Release、旧tag或无关用户材料。构建结果与GUI/设备/升级/性能验收分开。

旧跨语言8顶层/31子项等历史证据保留；当前固定夹具记录格式和Go恢复/helper/锁回归不能被描述为每次仍执行Rust。实体输入/系统认证/真实升级/最旧系统/性能与长期稳定性开放项见[迁移矩阵](../docs/dioxus-to-wails-v3.md)。
