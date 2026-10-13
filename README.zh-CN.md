# KitonyTerms

[English](README.md) | 中文

Rust + Dioxus 跨平台 SSH 客户端。提供会话与分组、密码/公钥/keyboard-interactive/agent 认证、单跳 ProxyJump、TCP 代理、终端、SFTP、监控和配置同步。

## 功能与边界

- SFTP 支持批量上传、下载、目录操作、内置文本编辑器和桌面外部编辑器。
- Linux/WSL 运维中心提供服务、进程、网络、Docker/Compose 查询与管理，支持能力探测、取消和独立容器终端。管理动作使用类型化命令，部分动作需要 sudo 认证。
- 桌面布局固定紧凑；手机使用独立触屏界面。支持系统/浅色/深色主题、中英文和终端字体设置。
- WebDAV 与局域网仅同步非机密配置，不包含 vault、vault key 或 known_hosts。WebDAV 填写完整资源 URL；LAN v2 使用 26 位配对秘密或二维码。
- Windows AMD64、macOS ARM64 和 Linux x64/aarch64 发布构建支持后台检查和下载更新，用户确认重启后安装。
- 主程序为 GUI；支持无参数、`--gui`、`--help`。多跳 ProxyJump、触发器规则 UI 编辑和完整语法高亮尚未提供。

实现入口：[应用](crates/kt-app/src/main.rs)、[界面](crates/kt-ui/src/components/app.rs)、[运维协议](crates/kt-core/src/remote_ops.rs)、[配置同步](crates/kt-sync/src/lib.rs)。

## 本地运行

使用 [Rust stable 工具链](rust-toolchain.toml) 和 [Cargo.lock](Cargo.lock) 锁定的依赖。

Ubuntu/Debian 系统依赖与 [CI](.github/workflows/alpha.yml) 一致：

```bash
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev pkg-config
cargo run -p kt-app
```

macOS/Windows 安装 Rust 和目标平台构建工具后运行 `cargo run -p kt-app`。桌面预览手机界面：

```bash
cargo run -p kt-app --features phone-preview
```

把窗口短边缩至 600 CSS px 以下。从侧栏创建并保存连接；密码与私钥口令写入加密 vault。

## 存储

[配置路径](crates/kt-config/src/lib.rs)和 [Store](crates/kt-ui/src/store.rs) 管理：

- `config.toml`：会话与非机密设置；`known_hosts.toml`：受信任主机密钥。
- `secrets.vault`：加密密码与私钥口令；`secrets.vault.key`：当前安装的本机密钥，两者均需保持私有。
- Android 使用应用私有 `files/config`、`files/data`。
- 旧固定密钥 vault 自动迁移；不能自动打开的旧主密码 vault 备份为 `secrets.vault.legacy*` 后重建。
- 桌面数据目录下的 `updates/` 保存更新开关、缓存和安装事务，不参与 WebDAV/LAN 同步。

## 桌面应用内更新

打开「设置 → 软件更新」查看当前版本、渠道、目标版本、最近检查时间和下载进度。发布构建默认开启自动检查和自动下载；启动后约 5 秒检查，运行期间每 6 小时检查，失败后退避重试。可分别关闭两个开关，也可点击「立即检查」或手动下载。

下载完成后点击「重启并更新」，确认连接将断开后才开始安装，也可选择「稍后」。请先保存并关闭内置编辑器、结束外部编辑、等待 SFTP 任务与配置同步完成；准备辅助程序失败时应用继续运行。检查、下载或校验失败可在此页面重试。

Windows 便携版替换当前 EXE，安装版使用系统安装器并可能显示 UAC；macOS 只读挂载 DMG、验证签名应用包并卸载镜像后替换整个 `.app`，Linux 替换实际启动的可执行文件。安装目录不可写、从 DMG 或 App Translocation 运行时，请使用页面中的发布下载入口，先安装到可写目录。更新保留原有配置、vault 和 known_hosts；替换失败或事务中断时尝试恢复旧版本，备份在新版本 UI 启动确认后清理。

正式版仅更新至更高正式版本，Alpha 按工作流构建号和重试序号更新，不自动切换渠道。开发构建禁用更新，Android/iOS 不提供此入口。现有旧版本需先手动安装一次包含更新功能的版本；之后发布的版本须附带有效更新清单，才能通过应用内更新。

## 构建与发布

[Alpha](.github/workflows/alpha.yml) 在 main push 时更新滚动预发布；[Release](.github/workflows/release.yml) 在 v* tag 时发布正式版本，tag 版本须匹配 [Cargo.toml](Cargo.toml)。正式发布阻断 RustSec 问题，Alpha 仅告警。

产物：macOS ARM64 DMG、Windows AMD64 便携 ZIP 与 Inno Setup 安装包、Linux x64/aarch64 TAR.GZ，以及 Android/iOS aarch64 包。Android APK 使用固定证书签名；iOS IPA 未签名，安装前必须自行重签。JavaScript Actions 统一使用 Node.js 24。

移动构建环境和命令以两条 workflow 为准：Dioxus CLI、`llvm-tools-preview`、Android SDK/NDK 或 Xcode。打包入口：

- [Android](.github/scripts/package-android-apk.sh)：`mobile-signing` Environment 提供 `ANDROID_KEYSTORE_BASE64`、`ANDROID_KEYSTORE_PASSWORD`、`ANDROID_KEY_ALIAS`、`ANDROID_CERT_SHA256`；不同密钥密码另设 `ANDROID_KEY_PASSWORD`。缺失或不匹配即失败。
- [iOS](.github/scripts/package-ios-ipa.sh)：生成未签名 IPA，不使用 Android 签名秘密。
- [构建号](.github/scripts/allocate-mobile-build-number.sh)：共享 concurrency 锁内按 UTC 秒分配；时钟回拨时不保证绝对唯一递增。
- [objcopy 预检](.github/scripts/prepare-rust-objcopy.sh)：在 dx 前准备 LLVM 动态库路径。

两平台标识为 [com.kitonyterms.app](Dioxus.toml)。Android 更新保持同一签名证书；iOS 覆盖更新取决于重签身份和配置。

桌面构建注入渠道、commit、构建号、重试序号和 target；[清单生成器](.github/scripts/generate-update-manifest.py) 汇总四个桌面目标，计算包大小与 SHA-256，macOS 使用 DMG、Windows 使用 ZIP/安装器。缺少产物或身份不一致会阻断清单发布；Alpha 在草稿中校验完整清单后切换公开发布，手动构建只生成产物。

## 验证与开发

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo audit
python3 -B .github/scripts/test-update-manifest.py
bash -n .github/scripts/publish-alpha.sh
```

[SSH 回环测试](crates/kt-core/tests/roundtrip.rs)覆盖 exec、PTY、取消和代次隔离；[移动打包契约](crates/kt-app/tests/mobile_packaging_contract.rs)覆盖脚本与产物规则。测试代码存在不代表当前环境已通过验证。

[更新测试](crates/kt-update/src/tests.rs)覆盖本地 HTTP、取消、限流与下载校验；安装事务和归档安全测试位于同 crate，macOS 使用真实 DMG 和临时签名应用包验证挂载清理、整包交换与回滚。Windows 安装版/便携版、macOS GUI 和 Linux 的真实安装重启仍须在对应系统验收。

模块边界见[架构](.agentdocs/architecture.md)，定向验证见[维护规程](.agentdocs/maintenance.md)，真机输入验收见[手机任务](.agentdocs/workflow/260820-mobile-phone-ui.md)。

许可证：Apache-2.0（[workspace 声明](Cargo.toml)）。
