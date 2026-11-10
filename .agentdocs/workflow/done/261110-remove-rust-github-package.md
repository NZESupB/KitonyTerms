# 项目 Rust 退出与 GitHub Wails 打包

## 目标与授权

2026-11-10 用户明确要求继续到移除 Rust 环境并上传 GitHub 打包。本任务范围是项目内 Rust/Dioxus 源码、Cargo 工具链与旧发布链退出，正式入口改为 app/ 的 Go/Wails/React；允许本次提交、推送 dev 和创建独立 Wails 预发布构建。保留现有 v0.1.17 正式 Release、main、旧标签、用户未提交的发布文档及未上传编辑副本。系统级工具链卸载不属于应用源码迁移。

平台策略：Android 仅 ARM64；macOS 最低15.0，停止12–14；iOS仅无签名设备IPA，不要求App Store或开发者账号，设备安装另需侧载签名。前阶段本机结果与限制见原全平台迁移任务。

## 现状与决策

- 基线dev为1f2072e，迁移提交d97ef4e已推送NZESupB/KitonyTerms；正式v0.1.17及滚动alpha仍为旧Rust产物且保持。
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
- [x] 仅提交本任务变更并推送dev，修复两轮原生CI失败后第三轮全平台打包成功。
- [x] 下载核对实际Release资产、稳定版/旧引用与用户材料；记录验收边界并归档本任务。

## 验收边界

Rust退出表示新源码/构建/发布不需要Rust，不等同所有实体输入、系统认证、真实升级、长期连接和性能测试均通过。未执行项目必须明确列出。旧Rust数据和更新记录的格式兼容必须继续保留。当前本机缺完整iOS/Linux环境，使用GitHub原生runner补构建证据；iOS实际可用IPA须以真实SDK链接成功为准。

## 首轮GitHub与修正

流水线42800902806/attempt1绑定d97ef4e。统一Go/React验证、移动号、Linux两架构、macOS ARM64、正式证书Android ARM64 APK/AAB、真实iPhoneOS SDK链接的无签名IPA均已成功；Intel macOS仍在执行。Windows两架构领域测试发现夹具本机盘符被误作SFTP相对路径，以及Inno嵌入模板CRLF导致契约字符串失败；原始job日志分别保存，不声称Windows打包通过。

修正使用统一`remoteTestPath`与实际初始权限比较，保留原测试覆盖；`.iss`固定LF，Intel无效目标测试使用当前目标。`ci-fixture-fix-local`格式/vet与session、两打包工具race退出0（50.150/4.965/4.025秒）；后续以新提交重新执行整套GitHub发布。

第二轮42804437969/attempt1绑定28d6945：Windows两架构全部领域测试和Inno契约已通过，真实PE打包前的工程识别因go.mod CRLF被LF字节前缀误拒绝。统一验证、Linux两架构、macOS ARM64、Android与iOS继续成功；Intel尚在运行时保存状态。已保留两份第二轮Windows日志。

工具改为按首行模块声明校验，现有失败材料测试使用CRLF并实际进入编译；`crlf-module-fix-local`格式/vet/两工具race退出0（4.168/5.142秒），27项发布契约通过。CI的Python固定UTF-8，后续以新提交重新执行，未创建或覆盖任何Release。

## 最终 GitHub 发布与实际下载核验

第三轮工作流 `42807234257` / attempt 1 绑定提交 `d3e8f686814c55cb97ff95e005f2c6dacdebe31f`，11项任务全部成功。GitHub Release `v0.1.18`（Release ID `440301750`）已公开为 prerelease，非 Latest；实际下载27项、共117,274,203字节，与发布资产大小、GitHub摘要、`SHA256SUMS`及更新清单逐项一致。六个原生桌面目标均有对应成功CI任务和构建身份记录。

下载复核另验证Linux两架构ELF与归档布局、Windows ARM64/amd64 PE与便携包、两份macOS DMG的镜像完整性/只读挂载/strict签名/包内身份/15.0最低系统和SDK/资源摘要，并确认挂载已卸载。macOS ARM64下载包隔离`--update-info`身份回读通过且未创建数据目录；Intel目标由原生CI验证。Android构建号 `1794310861`；APK v2与AAB签名均经独立检查且证书摘要一致（`363f85d3ad32603e75e7c61b9aaabf0d542de476254f36211be13d1c203c541a`），仅ARM64并满足16 KiB ZIP/ELF对齐。无签名iOS设备IPA经真实SDK链接及下载架构/版本/平台检查。

发布前后比对确认稳定 `v0.1.17`（Release ID `423169368`）的12项资产ID、大小、GitHub摘要和发布元数据未变，Latest仍为`v0.1.17`；`main`、`v0.1.17`、`alpha`远端引用与基线一致。项目已跟踪源码无Rust文件/Cargo工程清单；仅保留防止Rust构建链回归的负向测试和历史说明。本任务没有卸载系统Rust。

自动化原生打包不等同于产品验收关闭：正式旧包升级/恢复和生产数据接管、系统认证/Hello/UAC、实体输入、移动真机安装与行为、最低macOS 15系统运行、性能及长稳仍需对应环境单独验证。`v0.1.17`稳定发布和上述用户未提交材料保持不变。
