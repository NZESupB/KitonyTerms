# 维护与验证

生产工程为app/的Go/Wails/React，不需要Rust/Cargo/Dioxus。固定Wails CLI/模块/runtime为3.0.0，GitHub使用Go1.27.3/Node24。

## 完整门禁

```bash
cd app
bash scripts/check.sh
```

包括Go格式/vet/internal+tools race、Wails绑定无漂移、前端format/typecheck/lint/测试/生产build。CI各桌面另执行原生Go领域测试和真实打包；Windows默认非race以避免额外CGO工具链。测试只使用临时目录/公开夹具/私有生成密钥，不写用户正式数据。

仓库根发布契约：

```bash
python3 -B .github/scripts/test-update-manifest.py
bash -n .github/scripts/package-android-apk.sh
bash -n .github/scripts/package-ios-ipa.sh
```

Python脚本用`python3 -B`，不留下pycache。发布通过[workflow](../.github/workflows/release.yml)，清单版本来自frontend/package.json，dev手动构建在所有job通过后才创建预发布tag；现有Release不覆盖。

## 定向回归

在app/执行：

| 领域 | 命令 |
|---|---|
| TOML/vault/信任/落盘 | `go test -race ./internal/storage` |
| SSH/PTY/agent/ProxyJump/SFTP/运维取消 | `go test -race -timeout=240s ./internal/session` |
| 系统认证/秘密读取 | `go test -race ./internal/osauth ./internal/secretview` |
| 目录快照/权限 | `go test -race ./internal/datasnapshot ./tools/data-snapshot` |
| WebDAV/LAN/旧同步夹具 | `go test -race ./internal/syncconfig` |
| 更新归档/安装/helper/恢复/跨进程锁 | `go test -race -timeout=240s ./internal/update` |
| Windows资源/打包边界 | `go test ./tools/windows-package` |
| macOS加载命令/签名包/DMG | `go test ./tools/macos-package`；真实包专项需设置原有显式测试开关和对应SDK |
| 平台旧目录与隔离 | `go test ./internal/platform` |

正式回归测试与冻结公开夹具保留。旧Rust生成器/子进程测试已退役；不能把删除的跨语言工具描述为当前检查仍执行。`testdata/legacy-*`的来源说明与摘要见测试夹具文档；旧源码可从历史tag恢复。

## 原生与发布验收

- Windows：实际update-info、PE架构/图标/版本/manifest、ZIP/Inno；UAC取消/跨用户/占用/正式安装重启需另验。
- macOS：实际minos15.0与SDK、ad-hoc签名、只读update-info零业务写、DMG实际只读挂载/回读/卸载；签名公证/最旧系统/GUI升级另验。
- Linux：GTK4/WebKitGTK6原生CGO，ELF/身份和执行权限；无GUIserver不可作为桌面验收。
- Android：固定证书v2签名、ARM64唯一ABI、APK/AAB库一致、16KiB ZIP/ELF对齐与版本；真机/系统provider/OEM后台另验。
- iOS：完整设备SDK真实链接、arm64/platform2、无签名Payload及资源；无签名IPA不直接安装，真机另验。
- Release：固定commit/tag/run/attempt与完整job集合，实际下载27资产回读大小/SHA、更新manifest和平台验证记录；不能仅看发布页。

文档链接和`git diff --check`/`git diff --cached --check`须通过。检查未执行、失败或跳过都分别记录；不由构建推导实体输入、认证、正式升级或性能通过。内部文档仅记录已确认架构与长期约束，不改变无关历史任务。
