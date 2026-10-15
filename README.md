# xscc

xscc 直接维护自定义 RustDesk 原生应用和进程内管理库。应用代码来自 RustDesk 的 `streaming-foundation` 分支快照，后续只在本仓库开发；不使用 RustDesk 子模块、子树同步或另一个应用仓库。

- 根目录 `src/`、`libs/`、`flutter/`：原生应用、直接加密传输、屏幕捕获和 UI。
- `crates/management/`：包名为 `xscc` 的内嵌管理库，原生应用通过仓库内路径依赖它。
- `crates/native-management/`：实例绑定、平台私有存储、后台生命周期、持久回执和原生会话证明。
- 根目录 `Cargo.toml`、`Cargo.lock`：统一 Rust 工作区和锁文件。

桌面应用已接入实例绑定、WSS 管理通道、授权主机列表、被控地址上报、只读诊断与持久回执。首页可绑定服务端创建的控制端或被控端实例，并从授权列表发起原生连接。新会话验证双方实例与当前授权，授权撤销、失效或解绑后关闭连接。硬件、编码、画质等设置由 RustDesk 主控端选择，服务端不保存或下发这些设置。

- [源码与目录整合](docs/rustdesk-source.md)
- [管理库接入合同](docs/embedding.md)
- [原生管理接入与使用](docs/native-management.md)
- [稳定版本验收条件](docs/readiness.md)
- [开发验证](docs/development.md)
- [原生构建工具链](docs/BUILD-TOOLCHAIN.md)
- [来源应用说明](docs/upstream-rustdesk-README.md)

Rust 工具链为 1.99。公共管理协议使用 xscs 固定 Git revision；原生应用通过本地路径使用管理库。默认启用 `managed` 功能，未绑定时保留原有直连流程。此轮管理接入面向 Windows、Linux 和 macOS 桌面端；移动端仍需单独接入和验收。协议与存储测试不代替各平台的实际串流验收。

RustDesk 原始许可证保留在 [LICENCE](LICENCE)，管理库使用 Apache-2.0，见 [crates/management/LICENSE-APACHE](crates/management/LICENSE-APACHE)。作者和第三方声明保留原样，来源快照记录在 [rustdesk-source.json](rustdesk-source.json)。
