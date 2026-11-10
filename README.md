# KitonyTerms

基于 **Go / Wails v3.0.0 / React** 的跨平台 SSH 与服务器管理工作台。生产工程位于 [`app/`](app/README.md)，项目构建不需要 Rust、Cargo 或 Dioxus。

支持 SSH、单跳 ProxyJump、代理、agent 认证与转发、终端、SFTP、内外部编辑器、命令历史、监控、服务与 Docker 管理、WebDAV/LAN 配置同步及桌面更新。

- [运行、数据与发布说明](README.zh-CN.md)
- [工程命令与平台验证范围](app/README.md)
- [迁移实现和验收矩阵](docs/dioxus-to-wails-v3.md)

```bash
cd app
npm --prefix frontend ci
go mod download
go install github.com/wailsapp/wails/v3/cmd/wails3@v3.0.0
wails3 dev
```

开发使用 Go 1.27.3、Node.js 24 和平台原生 SDK。开发构建的数据目录独立；具有发布身份的桌面程序沿用原版本的配置/vault/信任与单实例锁。详细路径见中文说明。

发布覆盖 Linux x64/ARM64、macOS ARM64/Intel（最低15.0）、Windows x64/ARM64、Android ARM64 APK/AAB 和 iOS ARM64 无签名 IPA。GitHub Actions 完成构建及包校验后创建独立预发布，不覆盖已有稳定版。

iOS 产物需安装者重签才能侧载；构建成功不表示真机、系统认证、实体输入、正式升级和长期稳定性已验收。各包验证边界见附带的 `verification-*.json`。
