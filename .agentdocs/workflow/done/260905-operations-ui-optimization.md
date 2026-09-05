# 运维与界面优化

- [运维请求](../../../crates/kt-core/src/remote_ops.rs)与[运维 UI](../../../crates/kt-ui/src/components/operations.rs)：Docker/Compose、服务和进程的查询、管理与 sudo 挑战。
- [SFTP UI](../../../crates/kt-ui/src/components/sftp.rs)与[设置](../../../crates/kt-ui/src/components/settings.rs)：文件打开入口及终端设置。
- 后续状态以[能力探测与取消](260907-operations-capabilities-cancel.md)、[固定紧凑布局](260908-editor-open-density.md)为准。

验证入口：`cargo test -p kt-core remote_ops`、`cargo test -p kt-ui components::operations`。这里记录实现与测试出处，不保留无运行产物对应的历史通过数量。
