# KitonyTerms 0.2.3 · 正式版

KitonyTerms 0.2.3 是 Go / Wails v3.1.0 / React 架构的正式补丁版本，并设为 GitHub Latest。本版把 SSH 断线恢复改为原终端内按 Enter 重连：保留会话标签页、xterm 实例、滚动历史和未保存文件草稿，仅在后端轮换连接 generation；断开原因、重连状态与失败提示直接写入终端，连接配置缺失时提示打开连接管理。旧代次的输入、输出、尺寸和文件请求不能写入新连接，文件编辑器按稳定会话 ID 保留草稿保护。v0.2.2 及更早 Release 和资产保持原样。

**稳定版自动更新提示**：稳定客户端读取 `/releases/latest` 后会发现本版。这是用户选择发布正式稳定版的结果；全平台的旧正式包升级、生产数据接管、系统授权和回滚尚未完成实机验收。升级前务必完整备份，必要时优先从发布页手动安装。

- Linux x64/ARM64：原生 GTK4 与 WebKitGTK 6.0 桌面程序，TAR.GZ。
- macOS ARM64/Intel：最低 macOS 15.0，DMG；ad-hoc 签名，未做 Apple Developer ID 签名或公证。
- Windows x64/ARM64：便携 ZIP 和 Inno Setup 安装器；未做 Authenticode 签名。
- Android：仅 ARM64，固定发布证书签名 APK/AAB，递增内部构建号；不支持 x86/x86_64。
- iOS：真实 iPhoneOS SDK 链接的 ARM64 无签名 IPA；不能直接安装，侧载需由安装者签名；未验证真机运行。iOS 文件导入/导出仍未提供。

正式身份的桌面程序沿用旧数据目录与单实例锁；开发构建仍使用独立目录。升级前请完整备份 `config.toml`、`known_hosts.toml`、成对的 `secrets.vault`/`secrets.vault.key`、`updates/` 更新事务材料和未上传编辑副本；WebDAV/LAN 同步不是完整备份。手改或扩展配置、未知字段、历史自定义主密码密码库、设备私钥和移动沙盒迁移存在明确限制。不可与旧版同时打开同一数据目录；若更新或恢复事务尚未结束，先按原版本流程完成，不删除 `updates/` 绕过。Android 请覆盖安装以保留数据，不要先卸载。

自动验证覆盖构建、领域回归、发布身份、包格式、签名结构和摘要，不等同实体键盘/输入法、系统认证、真实设备、正式自动升级、最旧支持系统、长连接和性能验收。应用内自动更新还可能涉及保存未上传内容、确认退出/重启、Windows UAC 和安装目录授权；自动测试不能代替这些真实环境操作。

每个平台的 `verification-*.json` 记录本轮验证范围；`update-manifest.json` 与 `SHA256SUMS` 绑定实际上传资产。Release 包含六份构建身份、八个桌面包、三个移动包、八份平台验证记录、更新清单和摘要文件，共 27 项。安装前请按 `SHA256SUMS` 核对下载包摘要。
