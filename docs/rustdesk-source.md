# 应用源码与管理库整合

自定义 RustDesk 的 `streaming-foundation` 分支代码快照已直接复制为 xscc 应用主体。历史来源提交为 `8f7f6e6a2708d96a9d19ef7af56c7c4dee3c9d0a`，详见根目录 `rustdesk-source.json`；后续只维护 xscc，不依赖或同步原 RustDesk 应用仓库，也不保留其提交历史、子模块或子树关系。

```text
xscc/
  Cargo.toml / Cargo.lock     统一的原生应用工作区
  src/                       原生应用，包括 management 注册/存储基础
  libs/                      共享协议、直接传输、捕获等
  flutter/                   原生应用 UI
  crates/management/
    Cargo.toml               包名 xscc，Apache-2.0
    src/ / tests/             已有内嵌管理库与测试
  .github/workflows/         应用构建和管理库验证
  LICENCE                    来源应用许可证
```

应用的 Cargo 包名仍为 `rustdesk`，避免改变原生桥接、打包和平台工程现有约定。其 xscc 依赖指向 `crates/management`，不再从 GitHub 获取另一份管理库。根目录工作区和 Cargo.lock 同时包含应用与管理库；公共管理协议继续固定 xscs 的 Git revision。

本轮只做代码归并、路径和构建清单适配，不更改原生运行行为。来源快照已有的直接加密传输、主控串流偏好、注册和受保护绑定存储基础保留；完整管理任务生命周期、授权列表接入和原生会话执行仍按验收计划推进。硬件、编码、画质等设置由主控端选择。

## 验证与构建

在 xscc 根目录运行管理库检查，使用包选择避免不必要地构建全部原生依赖：

```sh
cargo fmt -p xscc -- --check
cargo clippy --locked -p xscc --all-targets -- -D warnings
cargo test --locked -p xscc
```

原生源码、平台资源、脚本和 `.github/workflows` 都位于根目录，继续使用来源快照中的相对路径。原生工作区检查例如：

```sh
cargo metadata --locked --format-version 1 > /dev/null
cargo test --locked -p base --lib
```

完整平台依赖与工具链见 `docs/BUILD-TOOLCHAIN.md`；桥接输入和生成约束见 `AGENTS.md`。源码归并不代表已完成 Windows、Wayland、macOS、iOS 和 Android 原生构建与运行验收。

应用原有 README 保存在 `docs/upstream-rustdesk-README.md`，许可证和第三方声明保留原样。`rustdesk-source.json` 是一次性来源归属记录，不包含后续同步或构建依赖。

## 本轮变更范围

- `Cargo.toml`：将应用的 xscc Git 依赖改为 `crates/management` 路径，并把管理库加入工作区，保证同仓库版本一起构建。
- `Cargo.lock`：统一应用和管理库锁定依赖，去除外部 xscc 源记录，纳入管理库测试依赖；async-trait 对齐管理库原先的 0.1.92，以通过 Rust 1.99 严格 Clippy。
- `.gitignore`：合并管理库和原生应用的构建产物规则；来源快照中的图片等已有资源均明确纳入跟踪。
- `.github/workflows/management.yml`：保留原管理库跨平台检查，改用 `-p xscc`，增加工作区和本地依赖清单核对；应用构建工作流按原相对路径复制到根目录。
- 原管理库 `src/`、`tests/`、`Cargo.toml`、`LICENSE-APACHE` 移到 `crates/management/`，运行逻辑和测试内容不变。
- README、来源记录和管理库开发/验收文档随目录变更更新，来源应用 README 原样保存。

未改动原生应用运行代码、管理库运行代码或原生运行路径的行为；本次不新增会话鉴权、串流选项或硬件控制功能。
