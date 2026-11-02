# 多实例与裸机改造验证

日期：2026-11-02。基于 `isarmg/One-KVM` 的 `67216384cd90d954c202827b76c8020779691279` 导入 XSCC 后的本地修改。历史分支验收另见 [原记录](managed-kvm-validation.md)，本文件仅记录本次实际运行的检查。

## 软件检查

| 检查 | 结果 |
| --- | --- |
| 完整 Linux 宿主 `cargo test --locked --all-targets` | 585 passed / 0 failed / 0 ignored；包含实例根页、重定向、API、静态资源、资源冲突、回滚、清源及恢复回归 |
| 完整宿主 `cargo build --locked --bin one-kvm` | 通过；嵌入真实 Web/WASM，不是关闭默认特性的空宿主 |
| 真实 CLI/HTTP 宿主验收 | 30 项通过：三个实例创建/启动、独立数据库和端口、真实嵌入 HTML/WASM、未授权拒绝、错误 Cookie、整机维护边界、在线目录锁及正常停止 |
| 完整宿主普通 Clippy | `cargo clippy --locked --all-targets` 通过，保留既有风格/复杂度警告；不宣称全宿主 `-D warnings` 通过 |
| 管理 workspace 测试与严格 Clippy | 32 项通过；`--workspace --all-targets -- -D warnings` 通过；同一浏览器控制端跨两个目标的身份复用、Cookie/授权隔离和单目标退出已覆盖 |
| Rust Web 前端 | 6 项通过；WASM all-targets Clippy `-D warnings` 与格式检查通过 |
| Web 构建器 | 33 项通过；真实 release WASM 和 35 个发布文件生成成功 |
| 浏览器路径辅助函数 | 3 项通过，拒绝跨实例路径和编码逃逸 |
| 裸机包与已有 GPUI 工作流 | 实际构建/解包 DEB、保留数据目录、显式启动、shell/PowerShell 语法和现有工作流边界检查通过 |
| Windows 平台源码 | 实际显示器/GDI/HID/WASAPI 模块的 GNU 目标检查通过；输入映射、BGR 行填充、HID 重新配置及监听事务检查通过；没有执行完整 MSVC 产品链接 |

Linux 验证使用 Rust 1.99.0、真实 FFmpeg、固定修订 `957f295ea946cbbd13fcfc46e7066f2efa801233` 的 libyuv（含 JPEG），以及实际 ALSA/udev/D-Bus/Opus 等库。依赖在隔离目录提取/编译，OpenSSL 使用系统库（`OPENSSL_NO_VENDOR=1`），测试关闭调试信息以控制构建体积。没有使用占位媒体符号或替换硬件算法。普通宿主 Clippy 与管理/WASM 的严格检查分别记录；原宿主仍保留既有风格/复杂度警告。

## 浏览器检查与边界

真实 Chromium 154 加载实例前缀内的当前 WASM，验证 Windows 显示器及键鼠配置、视频/音频保存、原生端口和 NAT 地址、上传、MJPEG、WebSocket HID、本地原始 Opus 解码、401/退出与迟到麦克风取消，以及实例 Cookie 边界。API/媒体使用明确的同源夹具，这不是物理 KVM 验收。

Firefox 157 通过相同的页面、配置、上传、MJPEG/HID、撤权清理和 Cookie 边界检查。独立探针表明本环境 Firefox WebAudio 在用户点击后仍不能恢复运行，因此音频项目显式跳过。两浏览器均无法产生本地 ICE 候选，WebRTC 环回显式跳过，不能报告 RTC 端到端通过。

本次没有采集卡、HID/ATX/OTG 等实际硬件，也没有 Windows 交互桌面。完整 Windows MSVC 构建、计划任务运行、显示器/DPI/输入/音频/热插拔和实际流媒体性能仍须对应平台验收。Windows 系统键鼠与全局 OTG gadget 各自独占给一个实例；多个实例不产生独立 Windows 用户桌面。

## 回归面

XSCC 根目录只调整独立模块的 workspace 排除、资源忽略规则、维护说明、产品入口文档和新增裸机 CI。One-KVM 保留自己的锁文件与依赖补丁。

现有启动路径将单实例运行时改为宿主聚合；现有设备配置入口在硬件应用和持久化间加入资源预留/失败回滚；现有媒体恢复固定到当前分配的来源并拒绝迟到恢复；Windows 平台入口加入真实桌面显示器、系统输入与音频端点；原有 Web 调用统一使用实例前缀。上述路径必须改变，才能使多个独立身份实际控制各自设备。浏览器控制端身份池只复用受保护的中央身份，目标授权和会话继续按实例检查。

从导入版本修改的现有宿主源码文件如下；新增的 `instances`、资源注册表、Windows 显示器/输入模块及 Web 路径模块分别承载新增功能，不改写 XSCC 原生 GPUI 的运行路径：

- `src/audio/capture_windows.rs`
- `src/audio/device_windows.rs`
- `src/audio/recovery.rs`
- `src/config/mod.rs`
- `src/config/schema/hid.rs`
- `src/config/store.rs`
- `src/hid/backend.rs`
- `src/hid/factory.rs`
- `src/hid/mod.rs`
- `src/lib.rs`
- `src/main.rs`
- `src/otg/service.rs`
- `src/platform/defaults.rs`
- `src/platform/mod.rs`
- `src/platform/windows.rs`
- `src/runtime/builder.rs`
- `src/runtime/supervisor.rs`
- `src/runtime/usb.rs`
- `src/stream/mjpeg.rs`
- `src/video/capture/windows.rs`
- `src/video/device/windows.rs`
- `src/video/stream_manager.rs`
- `src/video/streamer.rs`
- `src/web/handlers/audio_api.rs`
- `src/web/handlers/config/apply.rs`
- `src/web/handlers/config/atx.rs`
- `src/web/handlers/config/audio.rs`
- `src/web/handlers/config/mod.rs`
- `src/web/handlers/config/rustdesk.rs`
- `src/web/handlers/config/types.rs`
- `src/web/handlers/config/usb_update.rs`
- `src/web/handlers/config/video.rs`
- `src/web/handlers/hid_api.rs`
- `src/web/handlers/setup.rs`
- `src/webrtc/webrtc_streamer.rs`

浏览器路径和设备表单的改动限于 `web/src/media.js`、`web/xcss-web/src/` 的 API、导航、设置与初始化适配；`tools/web-build/src/assets.rs` 只调整相对资源 URL，其来源清单已更新。`crates/management/src/lib.rs` 将浏览器身份所有者共享给同一宿主的实例，并在授权、退出与清理时保留目标设备边界。

移除的部署路径为运行镜像 Dockerfile、容器入口、镜像打包及发布工作流；`build/cross/` 仅保留编译裸机产物。Linux 服务沿用旧数据根，Windows 启动脚本保留旧安装目录，避免升级生成另一套身份。

当前真实 WASM：`web/dist/assets/one_kvm_web_bg-18a1ab19bd9cbd19.wasm`，SHA-256 `18a1ab19bd9cbd19a53dce664f61cdb0ee608c0849d048caea32092ab3f8c37d`。

真实宿主启动验收使用上述完整二进制和独立临时目录，未连接用户 XSCS，也没有绑定或操作用户硬件。源码二进制 SHA-256：`969b09e370f4585523a86f4dd6c2ca58093b5fb6099f5a6787a3842261131d12`。本次执行证据为 `/tmp/xscc-real-host-acceptance.json`；浏览器夹具证据为 `/tmp/one-kvm-instance-browser.json` 与 `/tmp/one-kvm-instance-firefox-browser.json`。
