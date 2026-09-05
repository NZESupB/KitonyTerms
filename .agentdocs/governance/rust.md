# Rust 开发基线

适用依据：本会话项目指令；工具链与依赖分别见 [rust-toolchain.toml](../../rust-toolchain.toml)、[Cargo.toml](../../Cargo.toml) 和 [Cargo.lock](../../Cargo.lock)。

- 修改限于任务范围，复用现有模块；功能变化保留对应回归测试。
- 按[维护规程](../maintenance.md)执行适用检查；失败或未执行的检查不得记为通过。
- 密码、私钥、验证码与令牌不得进入明文配置或日志。
- 文件、外部命令、认证和并发操作需保留输入校验、失败反馈与资源清理。
