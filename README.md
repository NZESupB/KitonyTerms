# KitonyTerms

KitonyTerms 是基于 **Go / Wails v3.1.0 / React** 的跨平台 SSH 与服务器管理工作台。React 负责界面与 xterm.js 终端，Go 负责连接、文件、配置、凭据和更新事务，Wails 提供桌面与移动端宿主。

当前生产工程位于 [`app/`](app/README.md)。从发行包安装不需要 Go、Node.js、Rust 或 Cargo；从源码开发也不再需要 Rust/Dioxus。

## 版本与下载

当前正式稳定版为 **v0.2.1**，由 GitHub Release 的 `/releases/latest` 提供；历史 `v0.2.0`、`v0.1.19`、`v0.1.18` 等发布页和资产保留，可按版本选择下载。

- [v0.2.1 正式版](https://github.com/NZESupB/KitonyTerms/releases/tag/v0.2.1)提供多平台安装包；本版使用 Go / Wails v3.1.0 / React。
- [v0.1.18 Wails 预发布版](https://github.com/NZESupB/KitonyTerms/releases/tag/v0.1.18)与 [v0.1.17 稳定版](https://github.com/NZESupB/KitonyTerms/releases/tag/v0.1.17)保留为历史版本，旧版说明用于解释原升级路径，不代表当前 Latest。

稳定客户端读取 `/releases/latest`，可发现本正式版。Wails 版已有原生 CI 打包、领域回归和实际下载包核验，但尚不能承诺旧正式版在全部平台无缝升级；首次迁移请先阅读“从 v0.1.17 升级”。

## 功能

- SSH 密码、公钥、键盘交互、agent 认证与转发；单跳 ProxyJump、HTTP/SOCKS5/系统代理及 SSH config。
- 多会话终端、目录同步、内存命令历史和监控。
- SFTP 文件及目录批次上传/下载、取消、内置 UTF-8 编辑器和外部编辑器回传。
- 进程、服务、端口、网络与 Docker 管理，以及独立容器终端。
- WebDAV/LAN 配置同步、二维码配对、主机指纹确认、加密密码库、退出保护和桌面更新。

平台能力有差异：已保存密码查看需要 macOS/Windows 系统认证；Linux 和移动端不提供该功能。iOS 文件导入/导出尚未提供。同步只传非机密配置，不能替代完整备份。

## 安装

从对应 Release 的 **Assets** 下载与设备架构匹配的文件。文件名中的 `x64` 表示 Intel/AMD 64 位，`aarch64` 表示 ARM64。

| 平台 | v0.2.1 产物 | 前提与安装方式 |
|---|---|---|
| Windows x64 / ARM64 | `windows-*-setup.exe`、`windows-*.zip` | 需要 WebView2；安装版运行 Setup，便携版解压后运行 `kitonyterms.exe`。未做 Authenticode 代码签名 |
| macOS Apple Silicon / Intel | `macos-aarch64.dmg`、`macos-x64.dmg` | **macOS 15.0 或以上**；打开 DMG，将 `KitonyTerms.app` 拖到固定应用目录后运行。当前为 ad-hoc 签名，未公证 |
| Linux x64 / ARM64 | `linux-*.tar.gz` | 需要 GTK4、WebKitGTK 6.0、libxkbcommon、fontconfig 等运行库；解压后运行顶层 `kitonyterms`，保留整个解压目录。包在 Ubuntu 24.04 runner 构建，更旧发行版兼容性未保证 |
| Android ARM64 | `android-aarch64.apk`、`android-aarch64.aab` | APK 用于安装；最低声明 API 24，不支持 x86/x86_64。AAB 是分发输入，不能直接当 APK 安装 |
| iOS ARM64 | `ios-aarch64-unsigned.ipa` | iOS 15.0 或以上的设备包；**无签名 IPA 不能直接安装**，需要自行选择侧载工具并完成签名，不是 App Store 包 |

完整文件名以 `kitonyterms-v0.2.1-` 开头。下载页同时提供 `SHA256SUMS`、`update-manifest.json`、`build-*.json` 和 `verification-*.json`。安装前可用 macOS 的 `shasum -a 256`、Linux 的 `sha256sum` 或 Windows 的 `Get-FileHash -Algorithm SHA256` 计算安装包摘要，与 `SHA256SUMS` 对应行比较。

macOS 遇到系统安全提示时，按系统提示核实来源并在“隐私与安全性”中处理。不要通过关闭系统安全保护来安装。需要应用内更新时，应先将应用移出只读 DMG；不要直接从镜像或临时转移目录长期运行。

## 基本使用

1. 新建连接，填写主机、端口、用户名与认证方式。首次连接核对主机指纹后再信任；指纹变化时应先向服务器管理员确认。
2. 在终端中操作服务器；左侧 SFTP 面板用于文件与目录传输，运维入口用于查看服务、进程和 Docker 等资源。
3. 内置编辑器适合不超过 1 MiB 的 UTF-8 文本。使用外部编辑器时，确认修改已上传，再结束编辑任务或退出应用；失败保留的本地副本需要自行保管。
4. 在设置中调整语言、界面字体/缩放、终端字体/字号、外部编辑器与更新代理。SSH 代理与应用更新代理分别配置。
5. WebDAV/LAN 同步只同步连接和设置；目标设备仍需自行配置密码、私钥和主机信任。活动 SSH 会话、终端内容、命令历史和未提交草稿不会随升级或同步恢复。

## 数据与旧版兼容

### 文件和目录

应用主要使用文件持久化，当前没有需要执行 SQL 升级的内置 SQLite 等应用数据库：

| 文件或目录 | 内容 | Wails 版处理 |
|---|---|---|
| `config.toml` | 连接、分组、认证方式、代理和设置 | 沿用旧字段；合法旧配置由兼容夹具覆盖。保存可能改变 TOML 排版、注释和字段顺序，不保证字节不变 |
| `known_hosts.toml` | 已信任主机指纹 | 沿用旧结构与原目录 |
| `secrets.vault` + `secrets.vault.key` | KTVT v1 加密密码库和安装密钥 | 沿用旧加密格式与凭据 ID；必须成套保留 |
| `updates/preferences.json` | 自动检查/下载和更新代理 | 沿用旧偏好结构；无效代理会回退到系统代理并报告状态 |
| `updates/` 内 task/journal/备份 | 正在进行的更新事务 | 兼容旧字段及缺省布局；不代表已验证完整正式包跨架构升级 |
| 编辑暂存文件 | 尚未上传的本地修改 | 文件与任务状态分开；旧副本不会自动恢复成 Wails 编辑任务 |

**发布版桌面程序**沿用旧版 `ProjectDirs` 对应目录，同一用户正常安装无需搬运普通配置文件：

| 平台 | 配置目录 | 数据目录 |
|---|---|---|
| macOS | `~/Library/Application Support/com.kitony.KitonyTerms` | 与配置目录相同 |
| Windows | `%APPDATA%\kitony\KitonyTerms\config` | `%APPDATA%\kitony\KitonyTerms\data` |
| Linux | `$XDG_CONFIG_HOME/kitonyterms`，默认 `~/.config/kitonyterms` | `$XDG_DATA_HOME/kitonyterms`，默认 `~/.local/share/kitonyterms` |
| Android | 应用私有 `files/config` | 应用私有 `files/data` |
| iOS Wails 版 | 应用私有 Application Support 下的 `config/` | 同一根下的 `data/`；旧版沙盒接管未完成验证 |

开发构建使用 `os.UserConfigDir()/KitonyTerms-Wails-Development/{config,data}`，与正式目录隔离。因此自行编译后看不到旧连接，不一定是数据丢失。桌面环境变量 `KITONYTERMS_DATA_DIR=/绝对路径` 显式选择另一数据根，其下分别使用 `config/`、`data/`；这不会把开发构建变成正式发行版或启用自动更新。

### 不能视为完全兼容的情况

- **手改或扩展配置**：旧 TOML 解析器允许忽略未知字段，Wails 会拒绝未知字段，并对名称、端口、主题、字号等作校验，原文件不被覆盖。例如旧主题别名 `auto`/`system-default` 应在备份后改为 `system`；界面缩放必须在 `0.85–1.40`、终端字号在 `8–48`。未知自定义字段应先另存，再在迁移副本中处理，不能直接删除唯一原件。
- **历史密码库**：正常 `secrets.vault`/`secrets.vault.key` 可直接读取；更早固定密钥库会先保留 `secrets.vault.legacy-v1.backup` 再换成安装独立密钥。若使用自定义主密码、密钥缺失/错误或库损坏，Wails 会报错并保留原件，当前没有自定义主密码导入入口。应先找回匹配密钥，或在能打开旧库的环境中恢复凭据并重新保存，不能靠重命名文件解决。
- **未上传编辑副本**：v0.1.17 外部编辑副本通常位于系统临时目录 `kitonyterms/sftp-edit/`；Wails 新副本位于数据目录 `sftp-edit/`。旧任务、文件与远端的关联不会自动接续。先上传或另存，再升级；仅备份配置/数据目录不一定包含旧临时副本。
- **设备相关内容**：私钥文件、`~/.ssh/config`、外部编辑器和它们的绝对路径不包含在安装包或配置同步中，换机器后需另行准备和校验。
- **移动端容器**：Android 覆盖安装须保留相同包名和签名；卸载后重装会丢失私有数据。iOS 重签可能改变应用身份/容器，当前没有已验证的自动沙盒搬迁或完整凭据迁移向导。
- **读取上限**：配置和主机信任文件各不超过 4 MiB，密码库不超过 16 MiB；超限会明确拒绝。需在保留原件的副本中整理数据并核验，当前没有通用的超限文件迁移器，不能截断配置或密文文件。

因此，当前结论是：**已知旧格式的主要持久化数据兼容，存在明确限制，不能承诺所有历史数据和运行状态完全兼容。**

## 从 v0.1.17 升级

### 当前建议：备份后手动安装 Wails 预发布版

1. **确认系统与架构**：macOS 12–14 应继续使用旧版或先升级系统；Android x86/x86_64 不支持 Wails 包。Linux 先确认新桌面运行库可用。
2. **完成工作**：保存并上传编辑内容，另存仍未上传的本地副本，断开连接并正常退出旧应用。若旧版正在安装/恢复更新，先让原事务完成或按原版恢复流程处理，不删除 `updates/` 来绕过它。
3. **做完整离线备份**：保存上表配置和数据目录的全部内容，尤其 `secrets.vault`/`secrets.vault.key`、信任、更新材料和未知文件；另存旧临时编辑副本、外部私钥及旧安装包。备份含解密密钥，应按敏感文件保管。**WebDAV/LAN 同步不是完整备份。**
4. **替换应用，保留数据**：Windows 安装版关闭旧应用后运行新 Setup，使用原安装目录，按需确认 UAC；便携版在固定可写位置解压替换。macOS 将新 `.app` 复制到原应用位置；Linux 替换解压应用目录。Android 安装相同包名/证书的新 APK，不先卸载。iOS 需要手动签名、安装，并单独安排数据迁移。
5. **验证后再继续使用**：确认版本、连接列表、设置、密码读取和主机信任正常，再进行少量连接/传输测试。正式桌面包应自动读取旧目录，不应为了显示连接而随意重建或移动密码库。
6. **异常时保留材料**：停止进一步写入，保留错误、旧包及完整备份。不要删除 vault/key、journal 或编辑副本来“重置”；恢复新目录的离线快照不等于已完成原位置更新恢复。

Android `v0.1.17` 与本次 `v0.1.18` 已通过实际 APK 包名、签名证书一致性及版本号递增检查，具备覆盖安装的必要条件，但尚未完成正式旧包到新包的真机覆盖升级验收。桌面更新引擎也已实现兼容任务记录、helper、回滚与启动确认，但**完整的 v0.1.17 → Wails 正式包升级、系统授权和生产数据接管仍未全平台验收**，暂不称为已支持无缝升级。

需要检查备份或在隔离目录试迁移的开发者，可使用[离线快照工具](app/README.md#离线数据快照)。它只恢复到全新目录，不覆盖/合并现有生产目录；普通安装用户不需要为安装应用而配置 Go。

### 自动检查更新

稳定版更新器请求 GitHub `/releases/latest`，并要求 Release 为非 draft、非 prerelease；随后核对版本、通道、目标包、大小、摘要与构建身份。当前正式版为 `v0.2.1`，`v0.1.18` 等历史预发布版不会被稳定更新器选用。

- 安装或试用当前 Wails 正式版：从上方 `v0.2.1` 页面手动下载，按备份流程安装；已安装的稳定客户端也可能通过应用内更新发现本版。
- 历史 `v0.1.17` 客户端的检测规则由已安装的旧程序决定，不能通过修改新版本源码或客户端设置来改写。
- 发布稳定版并保持兼容的 schema 1 更新清单与有效目标包，是稳定更新器发现新版的前提；仅把预发布标为 Latest 不能满足客户端的全部校验。
- 即使收到稳定更新，保存未上传内容、确认退出/重启、Windows UAC 和必要的安装目录授权仍可能需要用户操作。

## 源码开发与验证

固定 Go **1.27.3**、Node.js **24.22.1**，Wails CLI、Go 模块与前端 runtime 同为 **v3.1.0**。依赖由 `app/go.mod`、`app/go.sum` 和 `app/frontend/package-lock.json` 固定。

```bash
cd app
go mod download
npm --prefix frontend ci
go install github.com/wailsapp/wails/v3/cmd/wails3@v3.1.0
wails3 dev -config ./build/config.yml
```

macOS 开发需要 Xcode Command Line Tools；iOS 构建需要完整 Xcode/iPhoneOS SDK。Linux 开发需要对应架构 CGO、GTK4/WebKitGTK 6.0、libxkbcommon、fontconfig 和 pkg-config；Windows 桌面使用 WebView2。开发环境要求与“下载应用即可运行”分别处理。

```bash
# 在 app/ 下执行完整门禁和宿主桌面编译
bash scripts/check.sh
bash scripts/compile.sh

# 在仓库根检查发布资产契约
cd ..
python3 -B .github/scripts/test-update-manifest.py
```

完整门禁检查 Go 格式/vet/race、Wails 绑定漂移、前端格式/类型/lint/测试/构建。兼容回归读取真实旧实现生成的公开 TOML/vault/同步/更新记录夹具，不依赖 Rust。移动构建、显式 server 模式、快照约束和打包入口见[工程说明](app/README.md)。

GitHub 发布链使用六个原生桌面 runner、Android ARM64 签名构建和真实 SDK 链接的无签名 iOS IPA。历史 `v0.1.18` 的 11 个任务及实际下载的 27 项资产核验已通过；本版实际发布结果以对应 Release 的 `verification-*.json`、`SHA256SUMS` 与 `update-manifest.json` 为准。包内身份、签名检查与哈希不等于真机、系统认证、最低系统实机、正式旧包升级或长期性能验收。已执行项和开放项见[迁移矩阵](docs/dioxus-to-wails-v3.md)。
