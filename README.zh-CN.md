# KitonyTerms

[English](README.md) | **中文**

KitonyTerms 是一个用 **Rust** 与 [Dioxus](https://dioxuslabs.com/)
构建的跨平台 SSH 客户端。SSH、终端模拟、SFTP、监控、配置和机密存储
都保留在 Rust crate 中；桌面与移动界面通过系统原生 WebView 栈渲染。

## 当前形态

- **主应用：** GUI-only 应用 `kitonyterms`，由 `kt-app` 提供。
- **支持平台：** macOS / Windows / Linux 的 `x64` 与 `aarch64` 桌面产物，
  以及 Android / iOS 的 `aarch64` 移动产物。不构建 32 位产物。
- **移动端界面：** Android 与 iOS 的 WebView 内容采用沉浸式布局，页面通过安全区适配
  避免被状态栏和导航栏遮挡。
- **核心引擎：** `kt-core` 中实现纯 Rust SSH 客户端、终端网格、SFTP 任务、远端监控和
  只读运维查询，不依赖 UI。
- **界面：** 当前稳定版 Dioxus desktop/mobile，桌面系统窗口或移动 WebView、响应式连接/SFTP
  区域、终端工作区、监控横条、状态栏、弹窗与设置面板。
- **验证：** workspace 每个 crate 都有单元测试或集成测试覆盖；clippy 以
  `-D warnings` 执行。

## 已实现能力

- SSH 终端会话：支持密码、公钥、keyboard-interactive、ssh-agent/Pageant 认证。
- 侧栏保存会话与分组：支持重连、编辑、复制、删除，并可合并 `~/.ssh/config`。
- 主机密钥信任流程：使用 `known_hosts.toml` 记录指纹；未知或变化的主机密钥需要确认，
  支持“仅允许一次”和“信任此主机”。
- 本机加密机密保险库：密码与私钥口令加密存储，UI 启动时自动打开。
- 单跳 `ProxyJump`、TCP 级代理（`Direct`、`System`、`SOCKS5`、`HTTP CONNECT`）
  和可选 agent forwarding。
- 终端渲染：RGB 颜色、常见文本属性、光标样式、scrollback、分屏、触发器高亮、
  可选行号和可选时间戳。
- SFTP 文件浏览：列表、上传、下载、创建目录、删除、重命名、远端路径导航、跟随终端目录，
  以及通过本地编辑器编辑远端文件后回传。
- 编辑器设置：默认编辑器选择和右键“打开方式”条目。
- 远端 CPU、内存、磁盘、网络、负载、运行时长和延迟监控。
- 只读远端运维中心（Linux/WSL Linux）：系统服务、进程、网络连接、Docker 容器和指标详情。
  每次查询使用独立 SSH exec 通道，仅使用当前 SSH 用户权限。Docker 列表中的容器可打开
  独立 SSH PTY，在容器内运行 `/bin/sh`，输入、尺寸、滚动和关闭状态与宿主终端隔离。
- 浅色/深色主题与中文/英文界面语言设置。
- 通过 WebDAV 或一次性局域网分享同步非机密配置；密码保险库、保险库密钥和
  `known_hosts.toml` 永不进入同步载荷。

## 当前边界

- `kt-app` 主二进制只提供 GUI 入口：无参数、`--gui`、`--help`。
  历史上的 `--safe`、`--system-ssh`、`--show-log`、`--list` 会明确报错。
- 尚未实现多跳 `ProxyJump` 链。
- 触发器规则暂不可在 UI 中编辑，完整语法高亮尚未实现。
- macOS 与 Windows 桌面发布包目前未正式代码签名 / 未 Apple 公证，因此 Gatekeeper
  与 SmartScreen 可能需要用户手动确认。
- Android APK 使用固定 keystore 与递增 `versionCode`，可覆盖安装较早版本。iOS IPA
  刻意保持未签名状态，不能直接安装，必须由用户自行重签。
- 当前不上传 TestFlight 或 App Store；iOS 后续覆盖更新取决于用户自己的重签配置，
  项目 CI 不保证更新连续性。
- headless 客户端仅作为 `kt-core` 示例用于调试核心管线，不是主要产品入口。
- 运维中心的只读查询目前仅支持 Linux/WSL Linux，不执行 `sudo`，也不执行远端写操作。
  Docker 容器终端是运维中心唯一的交互式远端写入例外，仍使用当前 SSH 用户权限，并且只允许
  固定的 `docker exec -it <id> /bin/sh` 命令。当前 SSH 用户无权访问、命令缺失或 systemd
  不可用时，会明确显示查询失败；只读查询暂未提供用户主动取消按钮。

## 快速开始

需要当前 Rust stable 通道；仓库的 `rust-toolchain.toml` 跟随 `stable`，不固定数字版本。

### Linux 依赖

Ubuntu/Debian：

```bash
sudo apt install libwebkit2gtk-4.1-dev \
  libgtk-3-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  libxdo-dev \
  libssl-dev \
  pkg-config
```

macOS 和 Windows 本地开发不需要额外系统依赖。

### 移动端打包

移动构建使用当前稳定版 Dioxus CLI：

```bash
rustup component add llvm-tools-preview
cargo install dioxus-cli --locked
dx bundle --release --platform android --target aarch64-linux-android --package-types apk --package kt-app
dx build --release --platform ios --target aarch64-apple-ios --package kt-app
```

Android 还需要 Android SDK 35、Build Tools 35.0.0 与 NDK 27.2；iOS 编译只需要 Xcode。
release strip 会调用宿主工具链的 `rust-objcopy`；仓库打包脚本会在构建前预检该工具，并为
Linux/macOS 配置 LLVM 动态库搜索路径。CI 应为 Android job 创建受保护的
`mobile-signing` Environment，并配置以下 Secrets：

- Android（PKCS#12 keystore）：`ANDROID_KEYSTORE_BASE64`、`ANDROID_KEYSTORE_PASSWORD`、
  `ANDROID_KEY_ALIAS`、`ANDROID_CERT_SHA256`；密钥密码不同时再配置
  `ANDROID_KEY_PASSWORD`。

Android 与 iOS 都固定使用 `com.kitonyterms.app`。后续更新不得更换 Android keystore。
iOS 产物不包含签名身份或 provisioning profile，安装前必须自行重签；若要覆盖已有安装，
每次重签需保持同一 Apple Team/application identifier、Bundle ID 与兼容 Entitlements，
且应用版本和 build number 不得低于已安装版本。部分免费账号或侧载工具会改写 Bundle ID
或产生短期签名，因此无法保证无缝更新。

`mobile-signing` 的 deployment branch/tag policy 应只允许 `main` 与正式 `v*` tag；同时应
保护 `main` 与 `v*` 标签创建权限，避免可修改 workflow 的未审查提交读取生产签名材料。
移动内部构建号由 Alpha/Release 共用的 concurrency 锁按 UTC 秒分配，并以旧计数器值
`1,787,238,032` 作为切换下限。分配器不再写 GitHub API，值不超过 Android
`versionCode` 上限 `2,100,000,000`。正常 runner 时钟下锁可避免同秒重复；若要在时钟回拨或
workflow 并发语义变化时仍提供绝对保证，必须引入外部持久化原子计数器。

### 启动应用

```bash
cargo run -p kt-app
```

入口检查：

```bash
cargo run -p kt-app -- --gui
cargo run -p kt-app -- --help
```

在桌面开发机上预览手机 Shell，可启用仅用于开发的特性，并把窗口短边缩到 600 CSS 像素以下：

```bash
cargo run -p kt-app --features phone-preview
```

在 UI 中，从侧栏创建连接，选择认证方式后连接；如需复用连接，可保存会话。
保存的密码和私钥口令会进入加密保险库，不会写入 `config.toml`。

在“设置”中，WebDAV 使用用户填写的完整 HTTP(S) 资源地址，并通过 ETag 条件写避免
静默覆盖并发修改。局域网分享使用临时随机端口 HTTP 服务、一次性配对码和十分钟
有效期。局域网协议 v2 使用 26 位 Crockford Base32 高熵配对秘密（去掉了 I/L/O/U），
同时以二维码展示；移动端可以直接用摄像头扫码，也可以手动输入，输入时大小写、空格、
连字符及 O/I/L 易混字符都会被规范化。旧版 v1 和短配对码不再接受。发现本机地址时会把 VPN 与
隧道网卡排到最后，避免开着 VPN 时分享出对端无法访问的地址。同步内容只包含等价于
`config.toml` 的非机密设置与已保存会话。

## 开发者地图

```text
kt-app
  Dioxus desktop/mobile 入口、桌面窗口/图标/菜单设置、最小 CLI 参数处理

kt-ui
  Dioxus 组件、AppState/Store 桥接、终端工作区、SFTP 侧栏、
  监控 UI、Operations 工具轨/抽屉、容器终端面板、弹窗、设置、
  主机密钥/认证提示

kt-core
  SessionManager、russh 连接/认证、PTY shell、终端引擎、
  SFTP worker、远端监控、allowlist 运维查询、独立容器 PTY runner、
  UI <-> core 消息协议

kt-config
  配置路径、TOML 模型、会话、应用设置、known_hosts、ssh_config 合并

kt-secrets
  Argon2id + XChaCha20-Poly1305 本机机密保险库

kt-sync
  非机密配置快照的 WebDAV 与一次性局域网传输层
```

关键边界是 `kt-core`：它负责协议和终端行为，并且不依赖 UI。
`kt-ui` 只通过 `ToCore` / `FromCore` 消息与它通信，并渲染 selector 风格的轻量视图模型。

## 存储模型

- `config.toml`：非机密的会话配置与应用设置。
- `known_hosts.toml`：受信任主机密钥指纹与最近访问元数据。
- `secrets.vault`：加密存储密码与私钥口令。
- `secrets.vault.key`：当前安装独立生成的本机密码库密钥；应与 vault 一样保持私有。
- Android 上述文件位于应用私有 `files/config` 与 `files/data` 目录。
- 旧固定密钥密码库会在启动时迁移到当前安装独立密钥。
- 无法自动打开的旧主密码保险库会备份为 `secrets.vault.legacy*`；
  新机密会继续写入新建的加密保险库。

机密值不应写入配置文件或日志。

## 验证

维护者门禁：

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
bash -n .github/scripts/allocate-mobile-build-number.sh
bash -n .github/scripts/prepare-rust-objcopy.sh
bash -n .github/scripts/package-android-apk.sh
bash -n .github/scripts/package-ios-ipa.sh
bash -n .github/scripts/publish-alpha.sh
```

workspace 测试覆盖应用入口、配置与机密存储、SSH/终端/SFTP 核心行为、
UI 状态流转及纯 UI 逻辑。测试数会随覆盖持续增长，因此 README 不再维护
容易过期的固定统计。

核心集成测试
[`crates/kt-core/tests/roundtrip.rs`](crates/kt-core/tests/roundtrip.rs)
会在回环地址启动真实的进程内 `russh` 服务端，验证连接、密码认证、宿主 PTY、shell 数据、
`TermEngine` 和 `GridSnapshot`，以及独立 allowlist exec 查询、stderr 隔离、容器 PTY 输入/尺寸/
滚动/关闭和恶意容器 ID 拒绝。协议 Debug 测试确保远端 payload 不会写入日志。当前测试尚未覆盖
用户主动取消控件或真机 GBoard/IME。

## 发布自动化

GitHub Actions 有两条打包流程：

- `.github/workflows/release.yml`：`v*` 标签只有在阻断式 RustSec 依赖扫描通过后，
  才会创建包含桌面端、签名 Android APK 与未签名 iOS IPA 的正式 GitHub Release。
- `.github/workflows/alpha.yml`：仅 `main` push 更新滚动 `alpha` 预发布。RustSec 扫描只告警；
  新产物会先完整上传到不可见草稿，再切换固定 `alpha` tag 与公开 Release，失败时恢复旧
  tag/Release，避免公开半套资产。`alpha` 因此始终指向最新成功发布的 Alpha 源码。

两条 workflow 共用同一套产物命名：Linux/macOS/Windows x `x64`/`aarch64`，以及
Android/iOS `aarch64`。Android 打包会检查包名、版本、ABI、图标与签名证书；iOS 打包会
检查唯一 `Payload/*.app`、Info.plist 版本信息、arm64 架构，并确认不存在 provisioning
profile 或代码签名残留。Android 签名 Secrets 缺失或不匹配时直接失败。GitHub 不允许
prerelease 标记为 Latest，因此 Alpha 使用滚动 tag 与最新发布的预发布语义，不承诺在
后续正式 Release 发布后仍永久位于 Release 列表第一项。

## 路线图快照

- [x] 核心 SSH + 终端引擎
- [x] Dioxus desktop GUI
- [x] 会话持久化、加密保险库、SFTP、监控
- [x] 主机密钥信任流程、分屏、ssh-agent、ProxyJump、触发器高亮
- [x] UI 模块化与 selector 驱动的主界面面板
- [x] Release/Alpha 打包与维护治理
- [x] Android APK 稳定签名与可自行重签的未签名 iOS IPA 打包
- [ ] 多跳 ProxyJump、可编辑触发器规则、更完整的终端高亮

## 许可证

Apache-2.0
