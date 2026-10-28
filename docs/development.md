# 开发验证

根工作区包含 Rust 核心、`crates/management`、桌面与移动 GPUI 产品；内嵌管理库的 Cargo 包名仍为 `xscc`。这些产品和核心测试共用根目录 `Cargo.lock`，Rust 工具链为 1.99。 产品 CI 用 `cargo clippy --no-deps` 保持原产品包的严格 lint 范围，管理库由独立 CI 严格检查；工作区共享依赖解析，输出目录仍由各打包流程显式指定。仓库内维护的 GPUI 框架仍保留自己的开发工作区；不能将该框架工作区的独立测试等同于产品依赖验收。

输入边界、类型保证、错误传播与热路径简化遵循[避免过度防御性编程](defensive-programming.md)。

管理库检查从仓库根目录运行：

```sh
cargo fmt -p xscc -p xscc-native-management -- --check
cargo clippy --locked -p xscc -p xscc-native-management --all-targets -- -D warnings
cargo test --locked -p xscc -p xscc-native-management
```

协议来自清单中固定的 xscs Git revision。临时联调可使用 Cargo 命令行 patch；不要提交针对外部工作目录的 path 依赖或临时覆盖后的锁文件。

管理库测试覆盖宿主回调、持久结果重放、摘要确认、绑定隔离、授权观察失效、地址上报和严格 TLS 入口。原生管理库还覆盖平台存储、身份恢复及双向会话证明。Windows、Linux、macOS 26 CI 均明确运行两个管理包的测试；移动 CI 使用实际 aarch64 iOS/Android target 检查管理库，Android 使用 API 29 的 NDK 编译器，iOS 最低版本 26。桌面 GPUI 应用 CI 在 `main`、`feat/management-prep` 与 PR 触发对应原生检查；Linux 被控端另编译 DRM 与显示唤醒可选路径。完整构建与实际运行验收应分别记录。

产品只从 GPUI 应用 manifest 构建，复用根工作区的 Rust 核心及 `.cargo/config.toml`。桌面控制端 / 被控端分别启用 `live-session` / `native-host`；移动控制端启用 `mobile-management,mobile-session`。工具链见 [原生构建](BUILD-TOOLCHAIN.md)，入口见 [项目首页](../README.md)，来源见 [源码整合](rustdesk-source.md)。旧前端构建和桥接生成不属于 GPUI 产品流程。

管理层的历史联调验证了 HTTPS 注册、WSS 列表、地址上报、双向配对证明、状态脱敏、撤权与解绑。当前 GPUI UI 直接调用同一原生管理所有者；这些管理测试仍不替代捕获、媒体、输入和物理设备串流验收。

macOS 离线 DMG 更新必须同时提供签名元数据：`--update update.dmg --metadata rustdesk-update.json --signature rustdesk-update.json.sig`。裸 DMG 参数会被拒绝；当前在线更新入口继续使用自己的发行校验流程。

Linux DRM 光标 IPC 的每条 `DrmCursor` 消息必须明确携带布尔字段 `hot_measured`；缺失字段会被拒绝。生产者测量与接收端推断的热点继续分别按显式 `true` / `false` 处理。

## GPUI 产品构建

```sh
cargo run --locked --manifest-path experiments/gpui-validation/Cargo.toml --features live-session
cargo run --locked --manifest-path experiments/gpui-validation/Cargo.toml --features native-host
```

移动应用在 `experiments/gpui-mobile-management` 构建，安装包必须经过对应 Android / iOS 原生打包与验证；见 [移动维护说明](gpui-mobile.md)。这些路径沿用 `experiments/` 目录名，产品界面均为 GPUI。

## GPUI 桌面应用

控制端使用 `live-session`，被控端使用 `native-host`，两者共用仓库内维护的 GPUI 和原生会话核心。

- [Windows 11 构建与打包](gpui-windows-beta.md)
- [Linux Wayland 构建与运行](gpui-linux.md)
- [macOS 26 Apple Silicon 构建与运行](gpui-macos.md)

Linux/macOS 被控端在已登录的桌面会话中运行。原生 CI 检查编译、单元测试和打包后的命令行入口；图形会话、捕获授权及真实串流另做实机验收。
