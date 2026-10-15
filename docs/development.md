# 开发验证

当前使用 Rust 1.99。运行 `cargo fmt --check`、`cargo clippy --locked --all-targets -- -D warnings`、`cargo test --locked`。

协议来自 Cargo.toml 固定的 xscs Git revision。尚未推送的本地配套分支可临时使用 Cargo 命令行 patch 指向该仓库的 `crates/protocol` 进行联调；不要提交 path 依赖或把临时 patch 的锁文件作为发行输入。

测试覆盖宿主回调、持久结果重放、摘要确认、绑定隔离、过期和严格 TLS 入口。真实 RustDesk 的存储与生命周期仍等待稳定版本验收。
