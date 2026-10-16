# 代理文档索引

## 长期文档

- [architecture.md](architecture.md)：crate 边界、协议、安全与 UI 约束；修改跨模块逻辑前读取。
- [maintenance.md](maintenance.md)：现有验证命令及文档归档方式。
- [governance/rust.md](governance/rust.md)：Rust 开发与安全基线。
- [README 中文版](../README.zh-CN.md)：运行、存储和发布入口。

## 当前任务

- [发布 v0.1.5](workflow/261016-release-v015.md)：版本与本地门禁已通过，待提交、推送 dev 与标签并核验 GitHub Release。
- [手机 UI](workflow/260820-mobile-phone-ui.md)：实现入口与尚未闭环的移动输入验收。
- [设置拖动、编辑缓存与下载](workflow/260926-settings-editor-download.md)：实现及协议测试完成；v0.1.1 已验证跨平台构建，真实桌面交互待验收。

## 最近归档

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
