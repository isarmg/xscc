# 源码来源

管理库原先从 xscs 客户端独立提取，原始历史保存在 https://github.com/isarmg/xscs 。`feat/management-prep` 将其收敛为 RustDesk 进程内嵌入的管理库；旧独立 agent、系统服务、CLI 和回环 HTTPS 适配不属于当前交付。

本轮将 https://github.com/isarmg/rustdesk 的 `streaming-foundation` 分支代码快照直接复制为 xscc 的应用主体，来源提交 `8f7f6e6a2708d96a9d19ef7af56c7c4dee3c9d0a`。来源仅用于历史归属，不是后续开发或构建入口，不建立子模块、子树或 RustDesk 远端同步关系。

原管理库移到 `crates/management`，包名仍为 xscc；应用使用本地路径依赖，同一工作区和锁文件配套维护。原生运行代码和协议保持来源快照行为，原来的应用 README 保存在 `docs/upstream-rustdesk-README.md`，许可证、作者和第三方来源声明均保留。
