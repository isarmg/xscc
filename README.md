# XSCC

XSCC 使用 **GPUI 作为唯一产品界面**，复用本仓库的 Rust 原生管理、加密传输、编解码、屏幕捕获与输入核心。桌面控制端和被控端是分别构建、分别绑定身份的应用；Android、iOS / iPadOS 只提供控制端。

## 产品入口

| 产品 | 源码入口 | 构建特性 |
| --- | --- | --- |
| Windows / Linux / macOS 控制端 | `experiments/gpui-validation` | `live-session` |
| Windows / Linux / macOS 被控端 | `experiments/gpui-validation` | `native-host` |
| Android / iOS / iPadOS 控制端 | `experiments/gpui-mobile-management` | `mobile-management,mobile-session` |

`experiments/` 是沿用的目录名称，上述路径是当前 GPUI 产品源码入口。桌面不带 `live-session` 的构建是本地渲染诊断；移动不带 `mobile-session` 的构建只验证实例管理。`gpui-android-smoke` 是独立的平台测试应用。

准备各平台原生依赖后，从仓库根目录运行桌面应用：

```sh
# 控制端
cargo run --locked --manifest-path experiments/gpui-validation/Cargo.toml --features live-session
# 被控端：独立角色构建
cargo run --locked --manifest-path experiments/gpui-validation/Cargo.toml --features native-host
```

两种角色构建的原始二进制同名，打包时分别生成控制端和被控端。平台依赖、独立输出目录及安装方式见 [Windows](docs/gpui-windows-beta.md)、[Linux Wayland](docs/gpui-linux.md)、[macOS](docs/gpui-macos.md)；移动构建与包验证见 [移动端](docs/gpui-mobile.md)。根 Cargo 包提供复用的原生核心和后台命令，图形应用从上表的 manifest 启动。

## 绑定与连接

在 XSCS 创建控制端、被控端实例及允许控制关系，然后在各自 GPUI 首页绑定对应实例。从授权设备列表发起会话，连接过程验证实例身份、当前授权和原生登录；撤权、解绑或管理连接失效后关闭会话。正式部署使用 HTTPS / WSS；本机或私网测试可填写 `http://127.0.0.1:18104`、`http://localhost:18104` 或显式私网 IP（例如 `http://192.168.31.245:18104`）。HTTP 仅接受本机、RFC1918 IPv4 私网字面地址和 IPv6 ULA；公网 IP 或域名使用 HTTPS。

GPUI 产品入口使用实例绑定和授权列表，不显示临时 IP 直连入口。画质、编码和目标帧率由控制端会话选择，服务端不下发这些设置。候选构建、包校验与物理设备上的串流质量 / 延迟验收分别记录。

## 仓库与维护

- `src/`、`libs/`：共享 Rust 原生会话、传输、捕获、编解码与输入核心。
- `crates/management/`、`crates/native-management/`：内嵌管理库、私有存储、后台生命周期和会话证明。
- `vendor/gpui/`：本仓库维护的 GPUI 平台后端与渲染实现。
- 根 `Cargo.lock` 和各 GPUI 应用锁文件固定相应依赖；Rust 工具链为 1.99。
- `flutter/` 和旧 Rust 前端适配仍有历史源码与回归夹具，保留源码不代表产品入口继续支持它们。GPUI 应用直接使用 Rust 核心，不需要 Flutter SDK、Dart 或 Flutter bridge 生成。旧 `build.py --flutter` 不是 GPUI 产品构建命令。

[开发验证](docs/development.md) · [原生工具链](docs/BUILD-TOOLCHAIN.md) · [管理接入](docs/native-management.md) · [验收条件](docs/readiness.md) · [性能优化记录](docs/performance-2026-10-24.txt) · [源码来源](docs/rustdesk-source.md)

支持 Windows 11 x86_64、Linux x86_64 Wayland、macOS 26 Apple Silicon、iOS / iPadOS 26 ARM64，以及 Android ARM64 API 29 起。硬件能力按实际捕获、codec 和渲染后端探测。

RustDesk 来源许可证见 [LICENCE](LICENCE)，管理库许可证见 [LICENSE-APACHE](crates/management/LICENSE-APACHE)。来源快照记录在 [rustdesk-source.json](rustdesk-source.json)，旧版说明作为 [历史资料](docs/upstream-rustdesk-README.md) 保存。
