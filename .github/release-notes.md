# KitonyTerms 0.1.19 · Wails preview

修复从旧架构升级后，服务器同时提供多种 SSH 主机密钥时误报「主机密钥变更」的问题。连接会在有界超时内重新协商其他现代密钥类型，直到找到已保存的原指纹；真正的密钥变更仍会拒绝连接，不修改或清空信任记录。ProxyJump 连接也使用同样的校验，并为每次目标重试重新建立独占跳板连接。

本版继续使用 Go / Wails v3.0.0 / React。现有 v0.1.17 稳定版与 v0.1.18 预发布资产保留，本版不设为 Latest。稳定版自动更新不会发现本预发布，请从本发布页手动下载安装。

- Linux x64/ARM64：原生 GTK4 与 WebKitGTK 6.0 桌面程序，TAR.GZ。
- macOS ARM64/Intel：最低 macOS 15.0，DMG；ad-hoc 签名，未做 Apple Developer ID 签名或公证。
- Windows x64/ARM64：便携 ZIP 和 Inno Setup 安装器；未做 Authenticode 签名。
- Android：仅 ARM64，固定发布证书签名 APK/AAB，递增内部构建号。
- iOS：真实 iPhoneOS SDK 链接的 ARM64 无签名 IPA；不能直接安装，侧载需由安装者签名；未验证真机运行。

正式身份的桌面程序沿用旧数据目录与单实例锁；开发构建仍使用独立目录。升级前请完整备份配置、vault/key 和未上传编辑副本；不可与旧版同时打开同一数据目录。Android 请覆盖安装以保留数据，不要先卸载。

自动验证覆盖构建、领域回归、发布身份、包格式和摘要，不等同实体键盘/输入法、系统认证、真实设备、正式自动升级、长连接和性能验收。iOS 文件导入/导出仍未提供。

每个平台的 `verification-*.json` 记录本轮验证范围；`update-manifest.json` 与 `SHA256SUMS` 绑定实际上传资产。
