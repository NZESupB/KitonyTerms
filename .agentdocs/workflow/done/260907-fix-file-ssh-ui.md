# 文件上传与 SSH 界面

- [SFTP](../../../crates/kt-core/src/sftp.rs)：UploadBatch、目录创建及覆盖提交。
- [sidebar.rs](../../../crates/kt-ui/src/components/sidebar.rs)：原生拖放批次、覆盖确认与上传入口。
- [dialog.rs](../../../crates/kt-ui/src/components/dialog.rs)：输入控件右键事件边界。
- [工具轨](../../../crates/kt-ui/src/components/main_shell/)与[样式](../../../crates/kt-ui/src/assets/app.css)：可见状态和主题呈现。

回归测试随对应源码保存；统一验证命令见[维护规程](../../maintenance.md)。不将归档状态作为当前平台 GUI 验收凭据。
