# 开发验证

根目录为统一的原生应用工作区；内嵌管理库位于 `crates/management`，Cargo 包名仍为 `xscc`。两者共用根目录 Cargo.lock，Rust 工具链为 1.99。

管理库检查从仓库根目录运行：

```sh
cargo fmt -p xscc -- --check
cargo clippy --locked -p xscc --all-targets -- -D warnings
cargo test --locked -p xscc
```

协议来自清单中固定的 xscs Git revision。临时联调可使用 Cargo 命令行 patch；不要提交针对外部工作目录的 path 依赖或临时覆盖后的锁文件。

管理库测试覆盖宿主回调、持久结果重放、摘要确认、绑定隔离、授权观察失效、地址上报和严格 TLS 入口。完整原生生命周期与会话鉴权仍等待稳定版本验收。

原生构建在本仓库根目录执行，沿用 `.cargo/config.toml`、`docs/BUILD-TOOLCHAIN.md` 和导入的应用构建工作流；遵守 `AGENTS.md`，不在本地运行 Flutter Rust Bridge 代码生成器。目录与来源说明见 [源码整合](rustdesk-source.md)。
