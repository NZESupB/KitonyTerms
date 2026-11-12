# KitonyTerms Go / Wails / React 工程

当前生产入口位于本目录。CLI、Go模块与前端runtime固定Wails v3.1.0；Go1.27.3、Node24，依赖使用go.sum和npm lockfile。项目源码、构建、检查和发布不再需要Rust/Cargo/Dioxus。旧跨语言工具已退役，固定公开TOML/vault/同步/更新记录夹具继续由Go回归。

## 开发与编译

```bash
go mod download
npm --prefix frontend ci
go install github.com/wailsapp/wails/v3/cmd/wails3@v3.1.0
wails3 dev -config ./build/config.yml
bash scripts/compile.sh
```

编译入口支持darwin/windows/linux的amd64与arm64；macOS需要SDK，Linux桌面需要对应架构CGO和GTK4/WebKitGTK6。macOS最低15.0。无发布身份的程序使用系统配置目录下`KitonyTerms-Wails-Development/{config,data}`。

`KITONYTERMS_DATA_DIR=/absolute/isolated-root`显式指定独立数据根。正式身份的桌面构建沿用旧数据目录和`kitonyterms.lock`，具体映射、兼容限制与旧版升级步骤见[项目说明](../README.md)。回归与故障注入必须使用临时副本，不能指向当前正式数据。

无GUI服务须显式选择：

```bash
bash scripts/compile.sh linux amd64 server
KITONYTERMS_DATA_DIR=/absolute/isolated-root WAILS_SERVER_HOST=127.0.0.1 WAILS_SERVER_PORT=19246 ./bin/compile/linux-amd64-server/kitonyterms
```

server回环检查不代表原生GUI、文件选择器、认证或剪贴板验收。macOS字体枚举需cgo/CoreText；Windows使用GDI，Linux使用有界`fc-list`；移动端为预设，失败来源明确返回。

## 自动检查

```bash
bash scripts/check.sh
# 从仓库根执行发布契约：
python3 -B .github/scripts/test-update-manifest.py
```

`check.sh`检查固定Wails版本、Go格式/vet/internal+tools race、生成绑定无漂移、前端格式/类型/lint/测试/生产构建。`frontend/bindings/`仅由`wails3 generate bindings -ts -i`生成，不手改。

领域回归覆盖SSH/agent/ProxyJump、PTY、SFTP批次/取消、操作与容器通道、vault/信任/原子保存、同步、快照、更新归档/事务/helper/锁与打包工具。旧实现固定夹具读取不需要Rust；不把历史跨语言验证数量当作当前执行结果。Windows权限缺失仅按明确错误码跳过，必须报告跳过分支。

## 离线数据快照

[data-snapshot](tools/data-snapshot/main.go)只接受显式绝对源和新目标，不自动发现用户目录。正常退出源应用与helper后运行：

```bash
go run ./tools/data-snapshot snapshot --config /absolute/source/config --data /absolute/source/data --output /absolute/private/snapshot-new
go run ./tools/data-snapshot verify --snapshot /absolute/private/snapshot-new
go run ./tools/data-snapshot restore --snapshot /absolute/private/snapshot-new --output /absolute/private/restored-new
```

源需已有`kitonyterms.lock`，以及可取得的已有helper/installation锁；工具不补锁。配置和数据可同目录或互不嵌套，源/目标不重叠；恢复只到全新根，不覆盖合并。普通文件、空目录、vault/key、信任、未知文件、未上传编辑副本和更新材料按字节保留，运行锁不复制。

上限8GiB/50000项/64层/16MiB清单；拒绝链接、特殊文件、不安全名称、大小写/NFC冲突，前后摘要/源身份和目标回读后刷盘发布。Unix0700/0600；Windows写入前设置并回读当前用户受保护DACL。不复制ACL/xattr/所有者，清单不认证来源；备份含解密密钥，应按敏感材料保管。失败保留.partial，需verify后才能恢复。

`pendingUpdate`仅提示已有事务；journal中的原绝对路径不改写、不执行，不能把恢复到新目录等同原位置正式升级。

## 平台打包

- Android仅ARM64；SDK36/NDK28.2.13676358/JDK21。`wails3 task android:package ARCH=arm64`和`android:bundle`要求正式签名四变量及`MOBILE_BUILD_NUMBER`。x86请求明确拒绝，旧fat/all-archs名称仅为ARM64别名。
- iOS无签名设备IPA：`wails3 task ios:package:ipa MOBILE_BUILD_NUMBER=1 OUTPUT=/absolute/new-output`，需要完整Xcode/iPhoneOS SDK；无开发者账号/App Store要求，侧载另需重签。生成器验证设备arm64/最低15.0、图标/启动屏幕、无签名/描述文件、Payload字节和执行权限。iOS文件交换仍未实现。
- 桌面发布通过[GitHub脚本](../.github/scripts/build-desktop.py)，六目标分别在原生runner构建并执行只读`--update-info`。Windows复用[资源与Inno工具](tools/windows-package/main.go)，macOS复用[签名与DMG回读工具](tools/macos-package/main.go)，Linux保留桌面入口和产品图标。

发布公开身份来自构建时注入；运行环境不能开启更新或改变来源。`app/frontend/package.json`为发布版本来源；schema1记录绑定版本/完整commit/run/attempt/target，更新清单绑定包大小/SHA和资产身份。GitHub打包成功后创建正式稳定Release并设为Latest，不覆盖已有Release或旧tag；旧预发布说明仅描述其历史通道。稳定客户端读取`/releases/latest`并拒绝预发布，安装或升级须按[安装与升级指南](../README.md)完整备份，不能以包校验代替正式升级验收。

## 验证边界

2026-11-10之前的本机平台证据与开放项见[迁移矩阵](../docs/dioxus-to-wails-v3.md)：macOS ARM64当前WKWebView基本IPC/设置/字体/连接表单/退出通过；Intel本机因无Rosetta只完成编译；Android ARM64 API35基本检查13项通过；Windows ARM64窗口/WebView2和领域测试通过，页面调试端点超时，GUI未验收。本机没有完整iOS SDK或Linux桌面环境。

GitHub原生构建与IPA真实SDK链接另以本轮job和实际下载资产为准。实体键鼠/IME、系统认证/UAC/OpenSSH服务、真机/OEM后台与文件provider、正式成功升级/生产数据接管、最低系统、长连接/性能/八小时稳定性仍需独立验收；不得由编译或模拟器结果推导通过。
