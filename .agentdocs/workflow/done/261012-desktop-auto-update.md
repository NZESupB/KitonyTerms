# 桌面自动更新

## 目标与依据

执行已批准计划 `.pi/plan/桌面三端应用内自动更新方案-20261012-1435.md`。桌面六种 target 自动检查和下载，确认重启后安装；不涉及移动更新、Git 写操作或远端发布。

## 阶段

- [x] 更新协议、下载校验、缓存与安装事务
- [x] 桌面辅助进程、单实例互斥及重启
- [x] 设置界面、自动调度及重启保护
- [x] 发布元数据与更新清单
- [x] 回归测试、本机项目门禁与文档

## 约束

更新配置仅本机保存，不参与同步；正式/Alpha 隔离；开发构建禁用自动更新。下载按资产 ID 绑定，校验大小和 SHA-256。替换先备份，失败恢复。编辑、传输和同步未结束时阻止更新重启。

## 验证状态

2026-10-12 本机实现验证完成：`cargo fmt --all -- --check`、`cargo check --workspace --all-targets`、`cargo test --workspace`（495 通过，1 忽略）、两种 workspace Clippy `-D warnings` 均通过。`cargo audit` 退出 0，保留 11 项允许的上游告警，未新增豁免。

四项 Python 清单测试、两条 workflow 的 YAML/六目标/清单依赖校验、Shell 语法及 diff 空白检查通过。macOS aarch64 本机编译/测试通过，签名临时 `.app` 使用原生 codesign/ditto 验证整包交换、回滚和启动确认。macOS x64 的 kt-update 交叉检查通过；从实际源码抽取 Windows 安装、恢复、进程和注册表 API 函数，在 x64/aarch64 MSVC target 的隔离项目类型检查通过。

Windows 完整 updater 交叉编译在 ring 的 C 编译阶段因宿主缺少 Windows SDK `assert.h` 失败；不视为通过。Linux 与 Windows 完整原生构建和安装、macOS 实际 GUI 更新重启、六种 target 的真实发布产物仍待对应系统/运行器验收；具体场景见维护文档。

## 可复用边界

安装与 UI 职责见 architecture.md 的桌面更新章节；验证入口与待验收场景见 maintenance.md。macOS ZIP 中 AppleDouble 元数据不得作为普通资源写入应用包，否则会破坏 codesign 的资源清单。事务只在新版本 UI 确认启动后清理备份。

本次未执行 Git 提交/推送、Release 创建或远端发布。
