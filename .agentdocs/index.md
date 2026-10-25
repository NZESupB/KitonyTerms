# 代理文档索引

## 长期文档

- [architecture.md](architecture.md)：crate 边界、协议、安全与 UI 约束；修改跨模块逻辑前读取。
- [maintenance.md](maintenance.md)：现有验证命令及文档归档方式。
- [governance/rust.md](governance/rust.md)：Rust 开发与安全基线。
- [README 中文版](../README.zh-CN.md)：运行、存储和发布入口。

## 当前任务

- [发布 v0.1.17 并合并 main](workflow/261025-release-v0117.md)：无新增功能的补丁版本重打包、全平台产物验收及 main 同步。
- [手机 UI](workflow/260820-mobile-phone-ui.md)：实现入口与尚未闭环的移动输入验收。
- [设置拖动、编辑缓存与下载](workflow/260926-settings-editor-download.md)：实现及协议测试完成；v0.1.1 已验证跨平台构建，真实桌面交互待验收。
- [界面字体与更新代理](workflow/261016-ui-font-and-update-proxy.md)：界面字体族/界面字号缩放与更新代理已实现并通过门禁；真实代理链路与非 100% 缩放的逐屏核对待人工验收。

## 最近归档

- [发布 v0.1.16 并合并 main](workflow/done/261024-release-v0116.md)：发布提交 23f4e31；修复 v0.1.15 审计依赖阻断后，流水线 40092527348 的 10 项任务全部成功，12 个附件设为 Latest，43 项实际下载校验与 5 项 macOS 包检查通过；main 快进纳入 dev 并同步远端。Rust 615 通过/1 忽略，Node 8 项、Windows 原生 61/6 项通过；原生 Vim/Docker/WebView 等实机验收仍待对应环境。
- [动态终端刷新](workflow/done/261024-terminal-render-batching.md)：8ms/16ms 输出合帧、同步更新超时、背压重试/关闭尾帧及 16ms UI 快照同步完成；Rust 串行 615 项通过/1 忽略、Node 8 项、浏览器动态 70 项/选区 267 项通过。默认并发仍触发既有单实例锁失败，独立测试通过；未改字体/TERM、未替换本机应用，原生 Vim/Docker/WebView 待实机验收。
- [发布 v0.1.14](workflow/done/261022-release-v0114.md)：终端拖选回复通道修复已发布，提交 042a370；流水线 39738473053 的 10 项任务全部成功，12 个附件设为 Latest，43 项实际下载校验和 5 项 macOS 包检查通过。Rust 606 项/Node 8 项及 Windows 原生 61/6 项测试通过；本机应用未替换，原生交互与连续更新仍待实机验收。
- [终端选区桥生命周期](workflow/done/261022-terminal-selection-bridge-lifecycle.md)：修复 eval 提前返回关闭回复通道；8 项桥接回归、267 项真实通道源码/真实网格浏览器断言通过。Rust 串行 606 通过/1 忽略；首次默认并发单实例锁测试失败、单独运行通过，未修改该无关模块。原生 WebView/系统剪贴板与手机手柄仍待实机验收。
- [发布 v0.1.13](workflow/done/261021-release-v0113.md)：终端行号与模型选区已升版发布，提交 90e1078；流水线 39631343005 的 10 项任务全部成功，12 个资产设为 Latest，实际下载的 43 项校验与 5 项 macOS 包检查通过，Windows 原生更新器 61 项/helper 6 项测试通过。本机应用未替换，原生剪贴板/触摸与连续更新仍待实机验收。
- [终端行号与模型选区](workflow/done/261021-terminal-selection.md)：行号留白/分割线、缓冲区选区、跨屏拖拽与统一复制已实现；工作区 606 测试通过/1 忽略，258 项浏览器断言通过。原生 WebView 菜单/系统剪贴板与手机触摸仍待实机验收。
- [发布 v0.1.12](workflow/done/261021-release-v0112.md)：发布提交 0d860f5，dev 与新标签已推送；流水线 39527210267 的十项任务全部成功，12 个资产发布并设为 Latest。41 项实际下载/身份/大小/摘要核验与 5 项 macOS 包检查通过；Windows 原生更新器 61 项、helper 6 项测试通过。v0.1.11 审计失败标签保留，本机应用未替换。
- [Windows 更新与通知主题](workflow/done/261020-windows-update-theme.md)：事务路径及 Shell/安装器传参修正、安装日志与阶段错误、通知主题已实现；工作区 593 测试通过/1 忽略，154 项浏览器断言及 Windows 安装模块隔离类型检查通过。v0.1.12 已补充 Windows 原生 61/6 项测试与正式构建证据，连续升级/UAC/WebView 待验收。
- [发布 v0.1.10](workflow/done/261020-release-v0110.md)：发布提交 0e29d60，dev 与新标签已推送；流水线 39390766230 的十项任务全部成功，12 个上传资产发布并设为 Latest。25 项实际下载/身份/大小/摘要核验及 5 项 macOS 包/签名/二进制身份检查通过；本机应用未替换。
- [文件夹下载进度](workflow/done/261020-folder-download-progress.md)：人类可读大小、文件文案及批次整体进度完成；工作区 589 测试通过／1 默认忽略（OpenSSH 专项另行通过），2329 项 SSR 浏览器断言通过。原生 WebView 下载及系统选择器交互待验收。
- [发布 v0.1.9](workflow/done/261020-release-v019.md)：最终发布提交 9ac26ae，用户授权重建未发布标签后流水线 39361411242 的十项任务全部成功；十二个资产设为 Latest，实际下载/身份/大小/摘要二十五项核验通过。Windows 原生更新器 57 项与辅助程序 4 项测试通过，本机应用未替换。
- [界面字体与文案精简](workflow/done/261019-ui-font-copy-cleanup.md)：界面字体名转义与平台回退、列宽按钮及重复提示清理完成；工作区 584 测试通过/1 忽略，88 份 SSR 夹具、2494 项浏览器断言通过。目标字体原生观感、Windows 跨屏 DPI 与真实持久化交互待实机验收。
- [更新交互与 Windows 权限](workflow/done/261019-update-permissions.md)：确认页已移除，写句柄刷盘、原用户目录事务、受限提权与恢复已实现。工作区测试 581 通过/1 忽略，274 项浏览器断言与 Windows 安装模块类型检查通过；v0.1.9 已通过 Windows 原生更新器/辅助程序测试及正式构建，UAC/ACL/占用实机验收仍待对应环境。
- [发布 v0.1.8](workflow/done/261019-release-v018.md)：发布提交 03edaf0；dev 与标签已推送。流水线 39229110227 的 10 项任务全部成功，12 个上传产物发布并设为 Latest；25 项身份/大小/摘要核验通过。Windows 原生构建成功，认证交互、真实 Docker 和 WebView 文件操作仍待实机验收。
- [认证、文件列宽与运维导航](workflow/done/261019-auth-columns-docker-navigation.md)：四项实现与适用自动检查完成；工作区 569 测试通过/1 忽略，289 项 SSR 浏览器断言及 Windows 认证 API 类型检查通过。完整 Windows 检查受缺失 SDK 阻塞；原生认证、WebView 文件操作、真实 SSH/Docker 对照和性能待实机验收。
- [发布 v0.1.7](workflow/done/261019-release-v017.md)：发布提交 0be0f5a；升级 zip 9.0.5 修复 v0.1.6 审计阻塞。流水线 39196336172 的 10 项任务全部成功，12 个资产发布并设为 Latest；桌面身份、大小与 SHA-256 摘要一致。本机应用未替换，原生系统认证/SSH/代理/重启安装待实机验收。
- [命令历史与系统认证查看密码](workflow/done/261019-command-history-password-reveal.md)：第 4、5 项实现及自动检查完成；工作区测试 553 通过/1 忽略，517 项 SSR 浏览器检查和 Windows 认证 API 类型检查通过；原生系统认证、真实 SSH 与原生剪贴板仍待实机验收。
- [发布 v0.1.5](workflow/done/261016-release-v015.md)：发布提交 49be49b；dev 与标签已推送，流水线 38862414531 的 10 项任务全部成功。12 个资产发布并设为 Latest，桌面构建身份、大小和 SHA-256 摘要一致；本机应用未替换。
- [全局视觉统一](workflow/done/261016-ui-consistency.md)：共用控件、主题、设置/弹窗、工作区及手机端表面已统一；既有浏览器检查见归档。v0.1.5 已修复 macOS 回环 HTTP 测试 socket 模式，512 项串行及默认并发 Rust 测试通过；原生键鼠/SSH/软键盘仍待验收。
- [更新界面](workflow/done/261016-update-ui.md)：状态卡片、发布日志、真实下载进度、主题及确认交互已实现；Rust 门禁与 915 项浏览器检查通过，原生 WebView 下载/终端/重启端到端待验收。
- [发布 v0.1.4 与本机更新](workflow/done/261016-release-v014.md)：dev 与标签已推送，12 个产物发布成功；本机 ARM64 正式版已更新并通过身份和签名校验，原生交互端到端待验收。
- [断线重连与文件侧栏](workflow/done/261015-reconnect-sidebar-layout.md)：Enter 状态门禁、桌面焦点和侧栏自适应已实现；Rust 门禁与 40 项浏览器检查通过，原生键鼠端到端操作待验收。
- [发布产物与 Node.js 24](workflow/done/261012-release-artifact-scope-node24.md)：macOS ARM64 DMG、Windows AMD64 ZIP/安装器与四目标更新清单；本机门禁通过，新远端流水线未触发。
- [桌面自动更新](workflow/done/261012-desktop-auto-update.md)：后台服务、确认重启安装与六目标发布清单已实现；本机门禁和 macOS 签名包事务通过，Windows/Linux 原生安装及三端 GUI 验收待对应环境。
- [SSH 监控延迟](workflow/done/261012-ssh-monitor-latency.md)：延迟统一为带序号的 SSH 心跳 RTT，沿用会话路径，超时为未知；自动检查通过，真实 TUN 环境待验收。
- [编辑器与紧凑布局](workflow/done/260908-editor-open-density.md)
- [运维能力探测与取消](workflow/done/260907-operations-capabilities-cancel.md)
- [设置、主题与字体](workflow/done/260907-fix-settings-theme-fonts.md)
- [文件上传与 SSH 界面](workflow/done/260907-fix-file-ssh-ui.md)
- [运维与界面优化](workflow/done/260905-operations-ui-optimization.md)

归档记录实现出处和验证入口，不以历史勾选或测试数量证明当前版本已通过验收。技术约束集中于架构文档，索引不重复记录。
