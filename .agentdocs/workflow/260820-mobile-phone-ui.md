# 手机 UI：待交互验收

## 实现出处

- [device.rs](../../crates/kt-ui/src/device.rs)：设备阈值、phone-preview、键盘遮挡量。
- [phone_shell](../../crates/kt-ui/src/components/phone_shell/)：四标签、触屏动作与键盘桥。
- [inline_editor.rs](../../crates/kt-ui/src/components/inline_editor.rs)：内置编辑、大小限制和回传失败保留。

实现代码及单测可查；当前文档不保留无日志或附件对应的历史 smoke 成功声明。

## 待验收

- [ ] Android/iOS 实际 SSH 会话中测试中文 IME、组合输入、退格及粘滞 Ctrl/Alt，确认无丢字或重复输入。
- [ ] 软键盘弹出、收起和旋转时确认终端及编辑器按钮不被遮挡，PTY 尺寸同步。
- [ ] 原生桌面 phone-preview 窗口核对触屏布局与会话切换。

执行时记录平台、提交、步骤和结果；源码单测不能替代这些交互验收。
