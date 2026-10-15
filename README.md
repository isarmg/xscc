# xscc

`feat/management-prep` 将 xscc 改为 **RustDesk 进程内嵌入的 Rust 管理库**，只支持 RustDesk。它没有独立可执行程序、CLI、系统服务或安装包。

本分支是接入准备，不是已经完成的 RustDesk 集成。当前只定义设备绑定、严格 WSS 通道、诊断任务、宿主持久执行事实和收存确认；配置写入、会话授权与媒体控制暂不提供。

RustDesk 提供已有 Tokio runtime、主机状态实现、受保护凭据和持久存储，显式启动、停止管理任务。xscc 不创建 runtime、不启动其他进程、不访问回环 HTTP 或扫描 RustDesk 配置文件。

- [宿主接入合同](docs/embedding.md)
- [等待 RustDesk 稳定的验收条件](docs/readiness.md)
- [开发验证](docs/development.md)

协议唯一源码仍由 xscs 的 `crates/protocol` 维护，依赖必须固定完整 Git revision。实际嵌入需要先统一本库与稳定 RustDesk 的 Rust 工具链、依赖和原生权限模型；当前库使用 Rust 1.99，尚未完成 RustDesk 原生构建验收。
