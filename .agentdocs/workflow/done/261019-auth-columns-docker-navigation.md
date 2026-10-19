# Windows 认证、文件列宽、Docker 监控与运维导航

## 目标与范围

依据用户批准的 `.pi/plan/windows-认证-文件列宽-docker-监控与侧栏返回导航的端到端实施方案-20261019-1521.md` 完成四项实施与当前环境适用的自动验证。不执行 Git 暂存、提交、推送或发布。原生验收保留在本记录，不以自动检查替代。

## 实施阶段

- [x] 运维请求按 OperationKey 分槽；页面与请求结果解耦，会话内父列表保留筛选、排序、滚动和选中身份；所有详情的加载/错误/空结果可返回。
- [x] Docker CPU/内存类型化，按完整 ID 合并；存储和镜像大小限定字段投影，批次最多 64，慢查询串行，前台优先，退避与过期快照；补正资源/网络按钮。
- [x] 文件六列布局，类型识别，Pointer/键盘调节、取消、恢复自适应；应用级原子保存失败不污染内存，最新配置只更新列宽。
- [x] Windows Hello 桌面 HWND 桥、显式账户密码入口、当前 token 账户预填、空密码真实 LogonUserW 验证、SID 匹配及错误引导；保留单次 Verified、Zeroizing、目标代次隔离和 30 秒清除。
- [x] 整体自查、Rust 门禁、SSH 回环、SSR 浏览器交互与 Windows API 类型检查；架构说明已同步。

## 实现决策与边界

- 页面只由导航改变；请求通过 pending 身份索引写入独立槽。动作在列表显示，成功后标记关联列表待刷新，旧列表仍在途时也保留该标记。每领域最多保留当前一个详情。
- Docker 继续经独立 SSH exec 使用校验后的 CLI 参数；runtime 约 2 秒、列表约 10 秒，storage TTL 60～300 秒；普通请求上限 12 秒，storage/image 上限 45 秒。输出仍受 8 MiB/64 KiB 的 stdout/stderr 上限控制。CPU 不截断至 100%，大小不可用与零分开，删除清单裁剪缓存并拒绝迟到数据恢复。
- 镜像逻辑大小直接按 image ID 查询，与容器可写层、rootfs 分列；不推断独占空间，卷、bind mount、日志不混入这些字段。
- 列宽变更只更新最新 Config 的该字段；窗口缩放不写偏好，None 为自适应。JS 按实例注册卸载函数，DOM 已移除时仍释放监听/观察器。滚动桥在清理阶段只取消帧，不读取已改变页面的 scrollTop；Rust 仅接收列表实例的滚动。
- Hello 取消/认证失败不自动进入密码窗口；仅明确不支持的 HRESULT 或未配置等可回退。空密码输入传 NUL 缓冲，经系统验证成功且 SID 匹配才允许查看；未能实际验证的账户策略不能推断为成功。

## 已执行验证（2026-10-19）

最终门禁全部退出 0：

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --all-features -- -D warnings
node --check crates/kt-ui/src/assets/sftp_columns.js
node --check crates/kt-ui/src/assets/operations_viewport.js
git diff --check
git diff --cached --check
```

- 工作区测试：569 通过、0 失败、1 项既有忽略；包含 31 项配置测试、285 项 UI 测试、12 项 SSH roundtrip。新增覆盖配置兼容/往返/失败、请求结果身份、返回后迟到事件、生产调度器退避/取消/慢查询串行、动作与列表竞态、Docker 多核 CPU/单位/不可用大小/非法输入/8 MiB 超限。
- 定向执行过 `cargo test -p kt-config`、`cargo test -p kt-ui` 及 `cargo test -p kt-core --test roundtrip`；此前实施阶段的 remote_ops 与列宽定向测试亦已执行。初次新增测试的编译、断言与测试位置问题均在最终门禁前修正。
- scratch 真实 Dioxus SSR 组件夹具 28 页；实际组件包括 SftpColumns/SftpColumnHeader/SidebarSftpEntry/OperationsDetail/DockerUsage，使用产品 CSS 与 JS。Headless Chrome 289 项命名断言全部通过：176/220/480/720px 文件区域、中英文、浅深主题、0.85/1/1.5 界面缩放、表头/行误差 ≤1 CSS px、六列拖动、键盘、Esc/blur/pointercancel/lostpointercapture、卸载清理、加载/失败/空详情可返回、200 行身份和滚动/动作焦点恢复。
- PostCSS 与 css-tree 均成功解析当前 CSS；浏览器无 pageerror。内置预览因宿主会话面板未显示而未打开，不记录为视觉截图人工核验通过。
- Windows 认证实际源码在隔离 scratch crate 下 `cargo clippy --all-targets --target x86_64-pc-windows-msvc -- -D warnings` 通过，检查 Hello/CredUI/token API 与 Windows 专用测试的类型；没有执行 Windows 测试二进制。
- 真实项目执行 `cargo check -p kt-ui --all-targets --target x86_64-pc-windows-msvc` 退出 101，停于 ring C 编译找不到 Windows SDK 的 `assert.h`；本机不能完成真实 Windows kt-ui 构建。

检查日志/夹具/结果均位于本会话 `$PI_SCRATCH_DIR`：`final-gates.json`、`final-*.log`、`browser-results.json`、`ui-preview/`、`ui-fixtures/`、`windows-auth-check/`、`windows-workspace-check.log`，不进入产品源文件。

## 未验收的原生场景

- [ ] Windows 11/较旧 Windows 的真实 Hello（PIN/指纹/人脸）、未配置、设备忙、取消、账户密码、空密码 Enter/确定、账户限制/锁定、其他账户 SID 拒绝、域/UPN/Microsoft 账户。
- [ ] Windows 实际 kt-ui 全目标构建与原生测试；需 Windows SDK/MSVC 与交互会话。
- [ ] 系统认证期间切换目标/关闭弹窗、成功后实际 30 秒清除的原生端到端。
- [ ] 原生 WebView 的拖动与文件双击/右键/上传拖放共存、重启和多会话偏好恢复；SSR/JS 和 Store 测试不能替代。
- [ ] 真实 SSH Docker 的 CPU/内存负载、写入可写层、共享镜像、停止/删除容器、权限不足，与 docker stats/inspect 对照。
- [ ] 真实 100～200 容器刷新耗时、并发与 UI 长任务/终端输入性能，不能由静态 200 行夹具断言达标。

本次授权范围的实现和适用自动验证已完成；以上条件保留为实机验收，不归入已通过项。
