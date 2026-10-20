# 文件夹下载文案与整体进度

## 目标与约束

- 文案为“正在下载 {文件名} {文件大小}”，复用现有 B/KB/MB/GB 格式化。
- 底部原生进度条按整个下载批次字节统计，累计在 core 中维护，按请求 ID 隔离。
- 清单扫描及总量未知使用未确定进度；成功事件后才显示 100%，错误不显示完成。
- 保留上传、编辑器的单文件进度语义和现有下载安全校验。

## 阶段

- [x] core 扫描统计总量，传递结构化批次进度。
- [x] UI 文案、整体进度、主题及可访问语义。
- [x] 单元、OpenSSH 集成、渲染布局验证。
- [x] 项目门禁、自查、架构说明与归档。

## 验证入口

按 `.agentdocs/maintenance.md` 执行 Rust 门禁，并显式运行 `cargo test -p kt-core recursive_download_against_openssh -- --ignored`。渲染夹具及自动化脚本保存在 session scratch，不把渲染检查视为原生 WebView 端到端验收。

## 验证结果与边界

- `cargo fmt --all -- --check`、`cargo check --workspace --all-targets`、两组 Clippy（默认／全特性）及差异检查通过。
- `cargo test --workspace -- --test-threads=1`：589 通过，1 默认忽略；该 OpenSSH 专项已通过 `cargo test -p kt-core recursive_download_against_openssh -- --ignored` 单独运行。
- 递归下载测试覆盖不同大小文件的单调累计、嵌套中文文件、零字节、空目录、覆盖及链接拒绝。单元测试覆盖实际传输大小变化、跨文件累计、成功／失败后迟到事件及单位边界。
- 实际 `DownloadNoticeView`、大小格式和中英文资源生成 20 份 SSR 夹具，在明暗主题、中英文、360/640/1280 宽度和 100%/140% 缩放的 240 种组合下通过 2329 项浏览器断言；CSS 经 PostCSS 与 css-tree 结构校验通过。
- 临时夹具首次在 runtime 外创建 EventHandler 失败；改用不带回调的 Fixture 根组件，通过 `VirtualDom::new_with_props`、`rebuild_in_place` 后渲染成功，实际组件无需修改。
- 浏览器 SSR 夹具的事件适配仅用于 DOM、布局和焦点核验，不代表生产 Dioxus 事件链路端到端验证。内置预览面板因调用会话不可见未打开；未人工查看截图，也未操作原生 WebView、系统选择器或真实 SSH 下载交互。
