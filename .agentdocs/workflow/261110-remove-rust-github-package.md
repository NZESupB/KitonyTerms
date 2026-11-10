# 项目 Rust 退出与 GitHub Wails 打包

## 目标与授权

2026-11-10 用户明确要求继续到移除 Rust 环境并上传 GitHub 打包。本任务范围是项目内 Rust/Dioxus 源码、Cargo 工具链与旧发布链退出，正式入口改为 app/ 的 Go/Wails/React；允许本次提交、推送 dev 和创建独立 Wails 预发布构建。保留现有 v0.1.17 正式 Release、main、旧标签、用户未提交的发布文档及未上传编辑副本。系统级工具链卸载不属于应用源码迁移。

平台策略：Android 仅 ARM64；macOS 最低15.0，停止12–14；iOS仅无签名设备IPA，不要求App Store或开发者账号，设备安装另需侧载签名。前阶段本机结果与限制见原全平台迁移任务。

## 现状与决策

- 当前dev提交1f2072e，远端NZESupB/KitonyTerms，具有push权限；正式v0.1.17及滚动alpha仍为Rust产物。
- app/约410个未跟踪源文件，运行层没有Rust/Dioxus调用。Rust依赖仅旧workflow、打包脚本和验证用app/tools/interop；保留旧TOML/vault二进制夹具，不再要求Rust编译器生成。
- 正式桌面数据目录需沿用旧directories::ProjectDirs映射；开发构建继续隔离。实际验证均指定临时目录，禁止在当前用户正式数据上试写。
- 新GitHub打包使用固定Wails3.0.0、Go1.27.3、Node24，Linux使用原生GTK4/WebKitGTK6；Android使用SDK36/NDK28.2/JDK21，iOS用完整Xcode runner。
- 使用独立v0.1.18预发布（latest=false），不覆盖v0.1.17正式Release或滚动alpha。产物逐项核对提交/run/attempt、大小和SHA-256；macOS另查包签名/身份并卸载镜像。首次CI失败须保留日志并修正重跑。
- 独立前端只读检查因耗时停止，不能称独立review通过；主代理自审和实际测试继续。

## 分阶段实施

- [x] 核对旧ProjectDirs、构建身份、GitHub发布契约并保存基线；Windows两目录均Roaming。
- [x] 实现Go生产/开发路径与冻结兼容夹具，六桌面目标及27项发布资产契约。
- [x] 项目Cargo/Dioxus/crates/target/旧interop已移至私有scratch；README/当前代理文档改为Go生产入口，历史证据保留。
- [x] 当前完整check-current-first退出0：Go格式/vet/internal+tools race、绑定无漂移、前端格式/类型/lint/47测试/build。当前文档100链接有效；3份用户保留材料摘要保持。
- [ ] 仅提交本任务变更并推送dev，跟踪GitHub全平台打包；修复确定的CI问题。
- [ ] 下载核对实际Release资产、记录未验证平台行为、回收本轮资源并归档本任务。

## 验收边界

Rust退出表示新源码/构建/发布不需要Rust，不等同所有实体输入、系统认证、真实升级、长期连接和性能测试均通过。未执行项目必须明确列出。旧Rust数据和更新记录的格式兼容必须继续保留。当前本机缺完整iOS/Linux环境，使用GitHub原生runner补构建证据；iOS实际可用IPA须以真实SDK链接成功为准。
