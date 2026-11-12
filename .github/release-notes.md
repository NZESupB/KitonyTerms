# KitonyTerms 0.2.1 · 正式版

KitonyTerms 0.2.1 是 Go / Wails v3.1.0 / React 架构的正式补丁版本，并设为 GitHub Latest。本版将 Wails CLI、Go 模块与前端 runtime 同步固定为 3.1.0；针对 WKWebView 的本地字体测量差异，让 xterm 构建统一使用自带 DOM 字宽度量，终端在容器可测量且字体加载完成后才打开或更新渲染器，并拒绝隐藏状态与过期字体请求的迟到布局结果；新增不改动布局尺寸的控件交互反馈，且保留减少动态效果支持。对外应用、窗口及 Windows / Apple 版本资源的显示名统一为 `KitonyTerms`。v0.2.0 及更早 Release 和资产保持原样。

**稳定版自动更新提示**：稳定客户端读取 `/releases/latest` 后会发现本版。这是用户选择发布正式稳定版的结果；全平台的旧正式包升级、生产数据接管、系统授权和回滚尚未完成实机验收。升级前务必完整备份，必要时优先从发布页手动安装。

- Linux x64/ARM64：原生 GTK4 与 WebKitGTK 6.0 桌面程序，TAR.GZ。
- macOS ARM64/Intel：最低 macOS 15.0，DMG；ad-hoc 签名，未做 Apple Developer ID 签名或公证。
- Windows x64/ARM64：便携 ZIP 和 Inno Setup 安装器；未做 Authenticode 签名。
- Android：仅 ARM64，固定发布证书签名 APK/AAB，递增内部构建号；不支持 x86/x86_64。
- iOS：真实 iPhoneOS SDK 链接的 ARM64 无签名 IPA；不能直接安装，侧载需由安装者签名；未验证真机运行。iOS 文件导入/导出仍未提供。

正式身份的桌面程序沿用旧数据目录与单实例锁；开发构建仍使用独立目录。升级前请完整备份 `config.toml`、`known_hosts.toml`、成对的 `secrets.vault`/`secrets.vault.key`、`updates/` 更新事务材料和未上传编辑副本；WebDAV/LAN 同步不是完整备份。手改或扩展配置、未知字段、历史自定义主密码密码库、设备私钥和移动沙盒迁移存在明确限制。不可与旧版同时打开同一数据目录；若更新或恢复事务尚未结束，先按原版本流程完成，不删除 `updates/` 绕过。Android 请覆盖安装以保留数据，不要先卸载。

自动验证覆盖构建、领域回归、发布身份、包格式、签名结构和摘要，不等同实体键盘/输入法、系统认证、真实设备、正式自动升级、最旧支持系统、长连接和性能验收。应用内自动更新还可能涉及保存未上传内容、确认退出/重启、Windows UAC 和安装目录授权；自动测试不能代替这些真实环境操作。

每个平台的 `verification-*.json` 记录本轮验证范围；`update-manifest.json` 与 `SHA256SUMS` 绑定实际上传资产。Release 包含六份构建身份、八个桌面包、三个移动包、八份平台验证记录、更新清单和摘要文件，共 27 项。安装前请按 `SHA256SUMS` 核对下载包摘要。
