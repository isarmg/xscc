# 导入前软件改造验收记录

此文档保留 2026-10-26 上游分支证据；不表示本次并仓、多实例或 Windows 裸机 Web 改造已通过。当前验证应另行记录。

日期：2026-10-26。结果来自本地修改后的源码与独立临时数据目录；没有提交、推送、发布或迁移线上状态。

## 自动化检查

| 检查 | 结果与边界 |
| --- | --- |
| One-KVM 默认完整宿主 | `cargo check --all-targets`、源码二进制构建通过；最终 `cargo test --all-targets` **564 passed / 0 failed / 0 ignored** |
| 自包含管理组件 | 独立 workspace **31 passed**；加强撤权断言后两项集成用例再次通过；Clippy `-D warnings` 与 fmt 通过 |
| Rust WASM 前端 | 纯 Rust **2 passed**；WASM all-targets Clippy `-D warnings`、fmt 与离线 Rust builder 通过；34 文件 / 12,436,491 字节 |
| XSCS | workspace **314 passed / 0 failed / 3 helper ignored**，Web **14 passed**；Clippy `-D warnings`、格式、迁移与包边界检查通过 |
| 格式与差异 | One-KVM Cargo fmt；三个仓库 `git diff --check` 通过 |

宿主普通 Clippy 完成，**仍有 84 条既有模块的风格/复杂度警告**，所以全仓 `-D warnings` 未通过；没有通过降低 crate lint 级别掩盖。新增管理授权、原生 proof、签名身份与 UAC PATCH 模块没有 Clippy 警告。管理组件、WASM 前端与 XSCS 的严格检查分别通过，不能据此把整个宿主称为严格 lint 清零。

完整 Linux 宿主使用真实 FFmpeg、固定源码修订的 libyuv、bundled turbojpeg/Opus 和实际系统库。缺失开发依赖在工作区隔离提取/编译，没有安装系统包、替换算法或使用空符号。不是 `--no-default-features` 空宿主验证。

## 真实 XSCS 与原生 XSCC

隔离 XSCS 使用本次 v4 源码构建；One-KVM 使用源码构建产物和本地 CLI 实际绑定的 `controlled / one_kvm` 实例。原生测试客户端直接使用当前 XSCC 的 `base::direct_transport::client::connect_handshake`、NativeManager 和原 protobuf 实现。

以下均通过：真实 KX1 加密握手、实际宿主签名公钥绑定的双向 HMAC 证明、Accepted、LoginResponse PeerInfo、加密 TestDelay 往返、中央撤权后既有连接关闭、撤权旧 permit 重连拒绝。此次夹具先等待双侧策略收敛，登录耗时约 2.303 秒；撤权后连接关闭约 4.987 秒。这些是本次观察值，不是服务时延承诺。

真实原生报告：`/tmp/one-kvm-fullstack-ozu9emqz/native-interop.json`。

## 真实浏览器与真实后端

源码 One-KVM + 隔离 XSCS 的浏览器验收通过 **36 项 HTTP/流程断言与 20 项 Chromium 断言**：匿名硬件 API 拒绝、Origin、HttpOnly/SameSite Cookie、CSRF、合法/非法地址更新、受保护初始化读取、退出与同码重登、两个独立控制端隔离。

两控制端各自建立真实事件/HID WebSocket 并收到消息。撤销第一个控制端的 ACL 后，它的 WebSocket 关闭、HTTP 与 setup 返回 401，另一控制端的 HTTP 和 WebSocket 保持可用。恢复 ACL 后旧 cookie 仍返回 401，显式重新登录才产生可用新会话。当前真实 WASM 返回 `200 application/wasm`。

真实全栈报告：`/tmp/one-kvm-fullstack-ozu9emqz/acceptance-evidence.json`。验收后已正常停止本次临时 KVM 与 XSCS 进程；18140、21118、18226 不再监听，私有数据与证据保留。

## 浏览器媒体适配

Chromium 153.0.8010.12 与 Firefox 155.0 都加载当前 Rust WASM。采用同源 API/媒体测试服务，另外验证了真实浏览器之间的 WebRTC 视频解码（320px）、RTCDataChannel HID 编码、本地原始 ESM Opus 解码并启动 AudioBufferSource、multipart 上传、终端 iframe、键鼠与设置交互。

Fetch/XHR 401、退出、迟到的麦克风权限返回后，所有 WebSocket/MJPEG 与麦克风 tracks 均停止。这些用例发现并修复了退出后已卸载 img 仍收流和迟到 getUserMedia tracks 未停止的问题。该组媒体测试使用 API/媒体替身，和上一节真实后端证据分别记录。

报告：`/tmp/one-kvm-web-browser.json`。

## 来源与未覆盖边界

XSCS Web 三组件的 128 个来源文件与原始 Git archive 一致；本地 Opus 四份完整 npm 原包与 SHA-512、一致性文件清单已核对。原始 Vue Web 的 **272/272** 个已跟踪文件在 `web/legacy-vue` 中与原始 HEAD 字节一致。生产不回退到 Vue。

本机没有采集卡或实际 HID/ATX/USB 硬件。真实后端 snapshot 返回 503；MJPEG 路由授权得到验证，但没有收到并验收实际视频帧。物理视频、键鼠、麦克风/UAC、ATX 和虚拟媒体仍需要对应设备做现场验收。此记录不声明硬件端到端通过、交叉架构运行通过或已经部署。

## 本次产物校验

- `/mnt/sarmg.org/kongzhi/.build-target/debug/one-kvm`：SHA-256 `954d886f3d94085082c617232530978f9a00327348feb2838c506ea3555c077f`
- `web/dist/assets/one_kvm_web_bg-b1d4ba069eb74918.wasm`：SHA-256 `b1d4ba069eb7491869f52c6d7a265a95b0622afe484872740e81f86714e9801d`
