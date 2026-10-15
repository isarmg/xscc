# 开发验证

根目录为统一的原生应用工作区；内嵌管理库位于 `crates/management`，Cargo 包名仍为 `xscc`。两者共用根目录 Cargo.lock，Rust 工具链为 1.99。

管理库检查从仓库根目录运行：

```sh
cargo fmt -p xscc -p xscc-native-management -- --check
cargo clippy --locked -p xscc -p xscc-native-management --all-targets -- -D warnings
cargo test --locked -p xscc -p xscc-native-management
```

协议来自清单中固定的 xscs Git revision。临时联调可使用 Cargo 命令行 patch；不要提交针对外部工作目录的 path 依赖或临时覆盖后的锁文件。

管理库测试覆盖宿主回调、持久结果重放、摘要确认、绑定隔离、授权观察失效、地址上报和严格 TLS 入口。原生管理库还覆盖平台存储、身份恢复及双向会话证明。Windows、Linux、macOS 26 CI 均明确运行两个管理包的测试；移动 CI 使用实际 aarch64 iOS/Android target 检查管理库，Android 使用 API 29 的 NDK 编译器，iOS 最低版本 26。应用 CI 在 `feat/management-prep` 也会触发原生检查。完整构建与实际运行验收应分别记录。

原生构建在本仓库根目录执行，沿用 `.cargo/config.toml`、`docs/BUILD-TOOLCHAIN.md` 和导入的应用构建工作流；遵守 `AGENTS.md`，不在本地运行 Flutter Rust Bridge 代码生成器。目录与来源说明见 [源码整合](rustdesk-source.md)。

桥接 CI 生成接口后单独分析新增的管理界面，检查 Dart 与原生接口一致。2026-10-15 的本机临时联调使用测试 CA、TLS 代理、当前 xscs 数据库和两个 `NativeManager`，验证 HTTPS 注册、WSS 列表、地址上报、双向配对证明、状态脱敏、两端撤权与解绑。该联调没有运行捕获、媒体或输入，不作为物理设备串流验收。
