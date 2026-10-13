# 发布产物范围与 Node.js 24

## 目标与范围

按用户要求，macOS 仅发布 ARM64 DMG；Windows 仅发布 AMD64 便携 ZIP 和 Inno Setup 安装器。Linux x64/ARM64 与 Android/iOS ARM64 沿用现有产物。所有 JavaScript Actions 使用 Node.js 24，保留 composite Action。

## 实施约束

- 发布与 Alpha 使用同一四目标桌面矩阵；更新清单以 macOS DMG、Windows ZIP/安装器和 Linux TAR.GZ 为准。
- 为保留 macOS 应用内更新，新增 DMG 包类型与原生只读挂载解包，不生成额外 macOS ZIP。
- 镜像内容只允许复制 KitonyTerms.app；路径、条目数、体积和内部链接沿用归档安全约束。复制后卸载镜像，再执行既有二进制架构、bundle ID 和 codesign 校验。
- 后续按用户要求将版本升级至 0.1.3，七个内部 crate 继承 workspace.package；渠道、发布触发条件、Ubuntu runner 与移动签名流程沿用现有配置。
- 用户已授权提交本次改动并推送 dev 分支。

## 阶段

- [x] 收敛两条 workflow 与清单生成器
- [x] DMG 解包、选择契约及原生安装回归
- [x] 升级 Node.js 24 Actions 与发布契约回归
- [x] Rust、Python、workflow 和文档验证

## 验证入口

完整 Rust 门禁、Python 清单测试、移动打包契约、两条 workflow YAML/矩阵/Actions 声明检查、diff 与文档链接检查。本机为 macOS，可用临时签名应用包和 DMG 校验实际挂载/卸载及安装事务。远端新 workflow 未执行时明确说明。

## 验证状态与边界

- fmt、workspace/all-targets check、workspace test、两组 Clippy 通过；测试为 500 通过、1 项既有忽略。
- Python 清单/发布契约 8 项通过，普通与 UTF-8 模式均通过。两条 workflow 的 YAML、依赖图、四目标矩阵、Action 输入和 Shell 语法通过；上游 action.yml 确认 7 个 node24 与 1 个 composite，Alpha 所用发布输出 id 存在。
- 真实 DMG 的签名包替换、回滚、启动确认、损坏镜像/越界链接清理及旧挂载回收通过。原生测试改为镜像生命周期互斥，创建镜像复用有界 hdiutil；本机并发创建曾出现等待，已回收旧测试进程并完整重跑通过。
- 使用会话 scratch 下此前安装的 cargo-audit。刷新数据库后，quinn-proto 0.11.17 命中 RUSTSEC-2026-0338/0340/0343，已定向升级至 0.11.18；除此依赖补丁与七个内部包版本外，锁文件未改变。补丁后编译、两组 Clippy 与审计通过，审计退出码 0，仍有 11 项既有上游告警；未新增审计豁免。
- 文档本地链接、Python 语法与 diff 检查通过。新 workflow 尚未在 GitHub 执行；真实 GUI 重启及 Windows/Linux 原生安装仍按既有验收入口验证。
