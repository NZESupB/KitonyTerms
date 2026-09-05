# 运维能力探测与取消

- [remote_ops.rs](../../../crates/kt-core/src/remote_ops.rs)：ProbeCapabilities、Cancelled 和固定命令。
- [session.rs](../../../crates/kt-core/src/session.rs)：取消路由、pending 和连接代次。
- [state.rs](../../../crates/kt-ui/src/state.rs)：懒探测、取消重试及快照保留。
- [roundtrip.rs](../../../crates/kt-core/tests/roundtrip.rs)：开通道、读取、sudo 等待取消，以及断线/重连隔离测试。

验证入口：`cargo test -p kt-core --test roundtrip`。这些测试覆盖协议状态机，不替代真实 Linux 主机或 GUI 人工验收。当前请求模型未实现 Prepare/Commit 两阶段协议或 PID 启动身份校验。
