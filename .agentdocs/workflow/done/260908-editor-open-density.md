# 编辑器与固定紧凑布局

- [inline_editor.rs](../../../crates/kt-ui/src/components/inline_editor.rs)：阻止右键冒泡，保留文本控件原生编辑操作。
- [external_edit.rs](../../../crates/kt-ui/src/components/external_edit.rs)：editor_menu_entries 合并探测结果与已有配置。
- [app.css](../../../crates/kt-ui/src/assets/app.css)与[配置测试](../../../crates/kt-config/src/lib.rs)：固定紧凑布局，旧 density 字段兼容读取并在保存时移除。

本会话在代码提交 `642ff4c` 上执行 `cargo test -p kt-config` 和 `cargo test -p kt-ui editor_menu`，均返回成功；其他历史门禁未在此记录为已复验。完整检查入口见[维护规程](../../maintenance.md)。
