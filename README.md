# xscc

xscc 直接维护自定义 RustDesk 原生应用和进程内管理库。应用代码来自 RustDesk 的 `streaming-foundation` 分支快照，后续只在本仓库开发；不使用 RustDesk 子模块、子树同步或另一个应用仓库。

- 根目录 `src/`、`libs/`、`flutter/`：原生应用、直接加密传输、屏幕捕获和 UI。
- `crates/management/`：包名为 `xscc` 的内嵌管理库，原生应用通过仓库内路径依赖它。
- 根目录 `Cargo.toml`、`Cargo.lock`：统一 Rust 工作区和锁文件。

管理库提供控制端/被控端实例绑定、WSS 管理通道、授权列表租约、被控地址上报、只读诊断与结果持久化。应用已有注册和受保护绑定存储基础；完整管理任务生命周期、授权列表接入和原生会话鉴权仍按稳定版本接入计划完成。硬件、编码、画质等设置由 RustDesk 主控端选择，服务端不保存或下发这些设置。

- [源码与目录整合](docs/rustdesk-source.md)
- [管理库接入合同](docs/embedding.md)
- [稳定版本验收条件](docs/readiness.md)
- [开发验证](docs/development.md)
- [原生构建工具链](docs/BUILD-TOOLCHAIN.md)
- [来源应用说明](docs/upstream-rustdesk-README.md)

Rust 工具链为 1.99。公共管理协议仍使用 xscs 固定 Git revision；原生应用只通过本地路径使用管理库，不从 GitHub 下载另一份 xscc。各平台原生构建继续使用导入的构建流程，本轮源码整合不等于完成原生运行验收。

RustDesk 原始许可证保留在 [LICENCE](LICENCE)，管理库使用 Apache-2.0，见 [crates/management/LICENSE-APACHE](crates/management/LICENSE-APACHE)。作者和第三方声明保留原样，来源快照记录在 [rustdesk-source.json](rustdesk-source.json)。
