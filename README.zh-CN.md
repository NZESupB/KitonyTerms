# KitonyTerms

KitonyTerms 是基于 Go / Wails v3.0.0 / React 的跨平台 SSH 与服务器管理工作台。唯一生产工程位于 `app/`；Rust、Cargo、Dioxus 及旧 crate 工作区已从当前构建链退出，历史版本可从旧 Git 标签获取。

## 开发与检查

需要 Go 1.27.3、Node.js 24 和 Wails CLI v3.0.0；Go 模块、前端 runtime 与 CLI 保持同一 Wails 版本。依赖由 `app/go.mod`、`app/go.sum` 和 `app/frontend/package-lock.json` 固定。

```bash
cd app
go mod download
npm --prefix frontend ci
go install github.com/wailsapp/wails/v3/cmd/wails3@v3.0.0
wails3 dev
```

macOS 桌面需 Xcode Command Line Tools；最低系统15.0。Linux 桌面使用 GTK4、WebKitGTK 6.0、pkg-config、libxkbcommon 与 fontconfig，Ubuntu 24.04 安装对应 `libgtk-4-dev libwebkitgtk-6.0-dev libxkbcommon-dev fontconfig pkg-config`。Windows 使用 WebView2。

```bash
cd app
bash scripts/check.sh
bash scripts/compile.sh
# 显式无GUI服务，不代表Linux桌面验收：
bash scripts/compile.sh linux amd64 server
```

完整命令、移动构建与测试边界见 [工程说明](app/README.md)。发布脚本契约检查从仓库根运行：

```bash
python3 -B .github/scripts/test-update-manifest.py
```

## 功能

SSH 密码/公钥/键盘交互/agent、单跳 ProxyJump、代理与 agent 转发；xterm.js 终端、命令历史与目录同步；SFTP 单文件/目录批次、内置 UTF-8 编辑和外部编辑回传；进程、服务、端口、网络和 Docker 管理、独立容器终端、监控；WebDAV/LAN 配置同步和二维码；原子配置保存、KTVT vault、known_hosts、退出保护及桌面后台更新。

同步不包含 vault/key、known_hosts、更新偏好或未上传编辑材料。查看已保存密码须通过平台系统认证；Linux与移动端不提供密码查看。iOS文件导入/导出尚未提供。

## 数据目录与兼容

无发布身份的本地开发构建使用 `os.UserConfigDir()/KitonyTerms-Wails-Development/{config,data}`，防止开发运行影响正式数据。`KITONYTERMS_DATA_DIR=/绝对路径` 可显式指定隔离根，配置和数据分别位于其 `config/`、`data/`。

具有校验通过的发布身份时，桌面沿用旧 `directories::ProjectDirs` 目录：

| 平台 | 配置 | 数据 |
|---|---|---|
| macOS | `~/Library/Application Support/com.kitony.KitonyTerms` | 与配置相同 |
| Windows | `%APPDATA%/kitony/KitonyTerms/config` | `%APPDATA%/kitony/KitonyTerms/data` |
| Linux | `$XDG_CONFIG_HOME/kitonyterms`，默认 `~/.config/kitonyterms` | `$XDG_DATA_HOME/kitonyterms`，默认 `~/.local/share/kitonyterms` |
| Android/iOS | 应用私有根的 `config/` | 应用私有根的 `data/` |

配置为 `config.toml`，信任为 `known_hosts.toml`，vault 与密钥为 `secrets.vault`/`secrets.vault.key`。桌面继续使用 `kitonyterms.lock` 阻止同目录并发实例。旧 TOML/vault/同步 envelope/更新记录由固定公开夹具回归，不再需要 Rust 生成器。

备份必须同时保存配置、vault/key、信任、更新事务及未上传编辑副本；离线工具 `go run ./tools/data-snapshot --help` 提供显式目录快照/校验/恢复，恢复仅允许新目录，不覆盖合并。验证和故障注入仅在临时数据上进行。

## 构建与发布

[GitHub workflow](.github/workflows/release.yml) 使用 Go/Node/Wails，六个桌面目标在对应架构原生 runner 构建。`dev` 的手动 workflow_dispatch 或匹配项目版本的 `v*` tag 触发；全部门禁和移动/桌面包检查成功后创建 prerelease，不设为 Latest，不覆盖已有 Release。

| 平台 | 产物 | 约束 |
|---|---|---|
| Linux x64/ARM64 | TAR.GZ | GTK4/WebKitGTK6 原生桌面依赖 |
| macOS ARM64/Intel | DMG | 最低15.0，ad-hoc签名，未公证 |
| Windows x64/ARM64 | ZIP、Inno Setup EXE | 未做代码签名 |
| Android ARM64 | 正式证书签名APK/AAB | SDK36、NDK28.2.13676358、JDK21；不支持x86 |
| iOS ARM64 | 无签名设备IPA | 完整Xcode/iPhoneOS SDK链接；侧载需重签，无App Store/开发者账号构建前提 |

Android GitHub环境 `mobile-signing` 提供 `ANDROID_KEYSTORE_BASE64`、`ANDROID_KEYSTORE_PASSWORD`、`ANDROID_KEY_ALIAS`、`ANDROID_KEY_PASSWORD` 和 `ANDROID_CERT_SHA256`。脚本仅在runner临时目录解码证书，校验APK v2签名证书、版本、ABI与16KiB对齐；不提交密钥。

发布包含六份构建身份、八个桌面包、三个移动包、八份平台验证记录、`update-manifest.json` 与 `SHA256SUMS`，共27项。清单绑定真实提交/GitHub run/attempt、大小和SHA-256；同名旧Release拒绝覆盖。macOS另执行签名、最低系统、只读身份探测和DMG实际挂载回读/卸载。

构建、结构校验和领域回归不代替真实GUI/设备/系统认证、成功正式升级、最旧支持系统、长连接及性能验收。各平台的已执行与开放项见[迁移矩阵](docs/dioxus-to-wails-v3.md)和发布附件。
