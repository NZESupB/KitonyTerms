# 设置、主题与字体

- [settings.rs](../../../crates/kt-ui/src/components/settings.rs)：设置布局、字体选择与预览。
- [theme.rs](../../../crates/kt-ui/src/components/theme.rs)、[app.css](../../../crates/kt-ui/src/assets/app.css)：系统深浅主题和菜单变量。
- [配置测试](../../../crates/kt-config/src/lib.rs)：theme/font 往返和兼容行为。

验证入口：`cargo test -p kt-config`。移动端显示效果仍需目标平台交互验收；不保留无法对应日志的旧编译失败或成功记录。
