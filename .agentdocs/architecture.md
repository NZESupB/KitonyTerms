# 架构与关键约束

## 模块边界

[Workspace](../Cargo.toml) 包含六个 crate：

| 模块 | 职责 |
|---|---|
| [kt-app](../crates/kt-app/src/main.rs) | GUI 入口与平台启动 |
| [kt-ui](../crates/kt-ui/src/components/app.rs) | 界面编排、AppState、Store |
| [kt-core](../crates/kt-core/src/session.rs) | SSH、SFTP、终端、监控与运维协议；不依赖 UI |
| [kt-config](../crates/kt-config/src/lib.rs) | 配置、路径、会话与 known_hosts |
| [kt-secrets](../crates/kt-secrets/src/lib.rs) | Argon2id + XChaCha20Poly1305 vault |
| [kt-sync](../crates/kt-sync/src/lib.rs) | WebDAV/LAN 非机密配置同步 |

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

## SFTP 与编辑

依据：[SFTP](../crates/kt-core/src/sftp.rs)、[内置编辑器](../crates/kt-ui/src/components/inline_editor.rs)、[外部编辑](../crates/kt-ui/src/components/external_edit.rs)。

- 下载先写私有临时文件；上传保留目标权限，经同目录临时文件提交。
- 覆盖 rename 失败时备份原文件再替换，提交失败尝试恢复备份；不得先删除正式文件。
- 内置编辑限制 UTF-8、1 MiB，下载前和读取时均校验；上传失败保留编辑内容与临时文件，允许重试。
- 外部编辑按请求 ID 推进下载/回传；Unix 临时目录/文件保持 0700/0600。
- 桌面打开方式合并探测结果与用户配置；手机使用内置编辑器。

## 终端与目录同步

依据：[shell integration](../crates/kt-core/src/shell_integration.rs)、[终端引擎](../crates/kt-core/src/term/)、[终端 UI](../crates/kt-ui/src/components/terminal.rs)。

- OSC 7 上报远端目录；bootstrap 追加 shell hook，保留用户已有配置。
- 输出过滤仅处理已识别的 bootstrap 数据；登录信息和 stderr 保留，超时、超限或用户输入时冲刷未确认缓存。
- 文件管理切目录统一经 `AppState::send_terminal_cd`；检查备用屏，转义路径，先清行/擦回显再执行 cd。
- 输入推断仅作有限兜底，不猜测任意脚本或别名；百分号路径解码仍需注意兼容性。
- 历史视口收到非空输入先恢复实时底部；双宽字符保留占位语义，显式 ANSI 色不被主题覆盖。
- 桌面粘贴优先[原生剪贴板](../crates/kt-ui/src/clipboard.rs)，失败再回退 WebView。

## 运维

依据：[请求与执行](../crates/kt-core/src/remote_ops.rs)、[运维 UI](../crates/kt-ui/src/components/operations.rs)、[回环测试](../crates/kt-core/tests/roundtrip.rs)。

- 使用固定命令和校验后的参数，经独立 exec 通道运行；输出与执行时间有上限，不污染宿主 PTY，不记录原始 payload。
- 类型化管理动作支持 sudo 挑战；当前协议不是 Prepare/Commit 两阶段协议，也没有进程启动身份校验来防止 PID 复用。
- 能力探测按连接懒加载；负面结果允许手动重试。proc/proc-net 可读性不等于已实现对应列表后端。
- 取消、断线、切换和重连收敛在途请求；失败保留最近成功快照，迟到结果不能覆盖新请求。
- Docker 容器终端独立拥有 SSH PTY、TermEngine 和 ExecId；宿主断线时回收。

## UI 与移动端

依据：[app.rs](../crates/kt-ui/src/components/app.rs)、[selector](../crates/kt-ui/src/components/app_logic.rs)、[设备判定](../crates/kt-ui/src/device.rs)、[键盘桥](../crates/kt-ui/src/components/phone_shell/keyboard.rs)。

- app 编排，state_controller 处理事件与副作用；展示组件优先接收轻量 selector。
- 手机以视口短边 600 CSS px 分界，两套 Shell 共用状态；Shell 渲染函数保持无 hook，局部 hook 下沉组件。
- 软键盘使用真实 textarea，IME 组合期间不清空输入；键序列复用终端映射，键盘高度由 device 写入统一 CSS 变量。
- 桌面固定紧凑布局；旧 density 字段由配置兼容读取并在保存时移除。
- [主题](../crates/kt-ui/src/components/theme.rs)与[样式](../crates/kt-ui/src/assets/app.css)使用变量；默认终端色随主题，显式颜色保留。
- 手机输入的待验收场景见[当前任务](workflow/260820-mobile-phone-ui.md)。

## 配置同步与打包

依据：[同步实现](../crates/kt-sync/src/lib.rs)、[扫码](../crates/kt-ui/src/components/scanner.rs)、[移动打包契约](../crates/kt-app/tests/mobile_packaging_contract.rs)。

- WebDAV 采用完整 URL 和 ETag 条件写；LAN v2 使用 26 位秘密、HMAC 请求认证与 ChaCha20Poly1305 载荷加密。
- 导入经 Store 原子落盘后 ACK；连接、头缓冲、处理时间和重放缓存有界，失败可回滚，认证失败不全局销毁分享。
- 扫码使用限频灰度帧，卸载时释放摄像头；移动产物声明相机权限。
- [启动逻辑](../crates/kt-app/src/main.rs)隔离移动端命令行参数读取；[app.rs](../crates/kt-ui/src/components/app.rs)内嵌 CSS。
- [工具链](../rust-toolchain.toml)跟随 stable，[Cargo.lock](../Cargo.lock)记录依赖快照，升级显式更新锁文件。
- 发布、签名、构建号和 objcopy 的操作入口统一见 [README](../README.zh-CN.md#构建与发布)，具体配置以 workflow 和脚本为准。
