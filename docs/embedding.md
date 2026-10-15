# RustDesk 宿主接入合同

xscc 是 RustDesk 的库依赖，不是独立 agent。`xscs_protocol` 通过 `xscc::protocol` 重导出，宿主不应消费另一份协议版本。

## 宿主负责

1. 使用 RustDesk 已有 Tokio runtime；在受管主机角色启用管理任务，普通观看端不自动注册为主机。
2. 显式取得用户/管理员同意后，通过 `/xscc/v2/pairing`、`/xscc/v2/enroll` 完成绑定。设备授权码、管理通道随机凭据和 RustDesk 的对等连接密码是不同秘密。
3. 在请求 enrollment 前持久保存安装身份和随机 256-bit 管理凭据；响应丢失后保留原身份，先查询 `/xscc/v2/identity`，避免另建身份。
4. 将凭据保存在正确用户/服务上下文的受保护存储中，不放入 URL、日志、命令参数或执行事实。
5. 实现 `RustDeskHost::diagnostics`，直接读取进程内真实状态。未实际接入时报告 `DiagnosticSnapshot::awaiting_integration()`，不能用测试夹具宣称 ready。
6. 实现异步 `Journal`。create 必须原子排他，replace 必须持久提交；保留 operation ID 与不可变指纹、绑定和已确认摘要。已确认记录可以压缩为去重事实，不能直接删除。存储需要独占宿主安装命名空间，并限制未收存结果数量（最多 4096）。
7. 持有 `ManagerConnection::run` 的任务，传入 watch shutdown；退出或撤销时先停止投递，再等待返回。watch sender 被释放也必须视为关闭。

## 组装接口

通过 `Capabilities` 声明 `backend = rustdesk`、`execution_mode = embedded` 和实际平台。构建 `Executor::new_with_capabilities(binding, capabilities, native_host, native_journal)` 并持有 `Arc`。

`ManagerConnection::new` 只接受受保护绑定配置中的 WSS 地址和独立 credential。调用其异步 `run(binding, capabilities, executor, health_receiver, shutdown_receiver)`；宿主用 HealthObservation 上报带采集时间的诊断观察。无须新的 runtime、独立服务、IPC 或 Flutter bridge。

当前合同只允许 `ReadDiagnostics`；结果在宿主 journal 中先持久化，Server 提交后返回指纹与报告摘要，校验后才能压缩。WSS 中断不会取消已开始的执行，重连补交同一结果。RustDesk 的配置、对等连接认证和媒体链路继续由 RustDesk 自己持有。

本分支不提供 enrollment UI、平台存储实现或 RustDesk 启动 hook；这些必须在稳定版本集成时实现并通过原生验收。
