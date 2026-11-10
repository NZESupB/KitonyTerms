# KitonyTerms 0.1.18 · Wails preview

本预发布将项目构建入口切换为 Go / Wails v3.0.0 / React，移除项目内 Rust、Cargo 和 Dioxus 构建要求。现有 v0.1.17 稳定版与历史资产保留，本版不设为 Latest。

- Linux x64/ARM64：原生 GTK4 与 WebKitGTK 6.0 桌面程序，TAR.GZ。
- macOS ARM64/Intel：最低 macOS 15.0，DMG；ad-hoc 签名，未做 Apple Developer ID 签名或公证。
- Windows x64/ARM64：便携 ZIP 和 Inno Setup 安装器；未做 Authenticode 签名。
- Android：仅 ARM64，固定发布证书签名 APK/AAB，递增内部构建号。
- iOS：真实 iPhoneOS SDK 链接的 ARM64 无签名 IPA；不能直接安装，侧载需由安装者签名；未验证真机运行。

正式身份的桌面程序沿用旧数据目录与单实例锁；开发构建仍使用独立目录。首次试用建议先备份配置、vault/key 和未上传编辑副本；不可与旧版同时打开同一数据目录。

自动验证覆盖构建、领域回归、发布身份、包格式和摘要，不等同实体键盘/输入法、系统认证、真实设备、正式自动升级、长连接和性能验收。iOS 文件导入/导出仍未提供。

每个平台的 `verification-*.json` 记录本轮验证范围；`update-manifest.json` 与 `SHA256SUMS` 绑定实际上传资产。
