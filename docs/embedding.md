# RustDesk 宿主接入合同

xscc 是 RustDesk 的库依赖，不是独立 agent。`xscs_protocol` 通过 `xscc::protocol` 重导出，宿主不应消费另一份协议版本。

## 宿主负责

1. 使用 RustDesk 已有 Tokio runtime；在控制端和被控端分别启用管理任务，各自绑定服务端创建的同角色实例；不能将普通观看端自动注册为被控主机。
2. 显式取得用户/管理员同意后，通过 `/xscc/v2/pairing`、`/xscc/v2/enroll` 完成绑定。设备授权码、管理通道随机凭据和 RustDesk 的对等连接密码是不同秘密。
3. 在请求 enrollment 前持久保存安装身份和随机 256-bit 管理凭据；响应丢失后保留原身份，先查询 `/xscc/v2/identity`，避免另建身份。
4. 将凭据保存在正确用户/服务上下文的受保护存储中，不放入 URL、日志、命令参数或执行事实。
5. 实现 `RustDeskHost::diagnostics`，直接读取进程内真实状态。未实际接入时报告 `DiagnosticSnapshot::awaiting_integration()`，不能用测试夹具宣称 ready。
6. 实现异步 `Journal`。create 必须原子排他，replace 必须持久提交；保留 operation ID 与不可变指纹、绑定和已确认摘要。已确认记录可以压缩为去重事实，不能直接删除。存储需要独占宿主安装命名空间，并限制未收存结果数量（最多 4096）。
7. 持有 `ManagerConnection::run` 的任务，传入 watch shutdown；退出或撤销时先停止投递，再等待返回。watch sender 被释放也必须视为关闭。

## 组装接口

通过 `Capabilities` 声明 `backend = rustdesk`、`execution_mode = embedded` 和实际平台。构建 `Executor::new_with_capabilities(binding, capabilities, native_host, native_journal)` 并持有 `Arc`。

`ManagerConnection::new` 只接受受保护绑定配置中的 WSS 地址和独立 credential。调用其异步 `run(binding, capabilities, executor, health_receiver, shutdown_receiver)`；宿主用 HealthObservation 上报带采集时间的诊断观察与 addresses（IP/域名及端口）。只给被控端上报真实可连接地址，最多四个；控制端始终发送空地址数组。观察超过 30 秒后撤回地址，正常心跳每 15 秒上报一次，服务端在 45 秒或离线后停止向控制端提供旧地址。无须新的 runtime、独立服务、IPC 或 Flutter bridge。

当前合同只允许 `ReadDiagnostics`；结果在宿主 journal 中先持久化，Server 提交后返回指纹与报告摘要，校验后才能压缩。WSS 中断不会取消已开始的执行，重连补交同一结果。RustDesk 的配置、对等连接认证和媒体链路继续由 RustDesk 自己持有。

本分支不提供 enrollment UI、平台存储实现或 RustDesk 启动 hook；这些必须在稳定版本集成时实现并通过原生验收。

## 控制权限观察

在调用 run() 前订阅 `let policy = connection.access_policy();`。服务端在 Hello 后及每 5 秒更新自己的角色与直接授权对端；控制端看到获准控制的被控端及最新地址，被控端看到获准接入的控制端。HTTP `/xscc/v2/access` 也可使用已绑定的 Bearer 凭据获取同样的快照；不要使用管理员会话。

宿主通过 `policy.borrow().as_ref().and_then(|observation| observation.policy())` 获取当前仍在租约内的列表，或用 `AccessObservation::permits(&peer_binding)` 检查已认证的完整安装绑定。每次使用都必须检查；不能缓存一次 true 作为永久许可。30 秒租约使用单调时钟，断线、撤销和退出清空观察，过期副本也返回无效。列表不含凭据，默认无授权，未绑定和已撤销实例不下发。授权在实例两侧是同一份关系，实例更换凭据后安装绑定更新。

管理授权不是原生会话身份证明。稳定后的 RustDesk 接入必须把加密验证的对端身份绑定到 manager_id/device_id/installation_id，不能信任来源 IP 或自报 UUID；拒绝无有效授权的新会话，并关闭被撤销的既有会话。本库不实现 RustDesk 的密钥证明、连接 hook 或会话强制关闭。

硬件设备、编码器、编码格式、画质、帧率由 RustDesk 主控端选择并与被控端协商，xscc 不接受服务端远程修改这些设置的任务。
