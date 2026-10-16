# Slint 与 GPUI 验证

本轮并列验证两个框架，最终选型由用户决定。现有 Flutter 应用继续作为功能基线；本轮不自动替换。

## 范围与版本

- Slint 固定 `1.18.1`，使用 Winit/Android 原生后端，分别验证软件绘制与 FemtoVG/WGPU 纹理路径。
- GPUI 固定 Zed 上游提交 `28d0a3547f16b9609b301b48ab363bfca649c864`。不使用落后于上游源码的 crates.io `0.2.2` 包评估当前能力。
- 两套原型均固定 `libc 0.2.189`：首次 iOS 检查发现 `0.2.190` 将 dyld 声明限定为 macOS，而共享 `backtrace` 仍在 iOS 调用，导致编译失败。此约束仅在实验 workspace 中，不改变正式应用的依赖。
- 两套程序分别位于 `experiments/slint-validation`、`experiments/gpui-validation`，各自独立 Cargo workspace/lockfile，不接入正式应用构建。
- 两者复用 `experiments/shared/video.rs` 的测试图案、单帧邮箱与可选 VP9 回放，以及相同的 `NativeManager` 实现；键鼠事件编码为现有 `base::message_proto::Message` 后再解码检查。
- `--state-dir` 必须显式指定私有的测试目录。默认不读取正式应用的绑定与凭据。原型不启动被控端监听，不向远端注入输入。

## 目前可以验证什么

1. 原生窗口创建、连续帧显示、尺寸缩放与等比例显示。
2. Slint 外部 RGBA 纹理导入；GPUI `RenderImage` 的 BGRA 上传及旧图像释放。
3. 实际界面事件到 xscc 键鼠协议消息的转换；只统计事件，不记录输入内容。
4. 使用既有管理库读取实例状态和允许列表；Slint 另提供测试实例绑定入口。
5. Linux 可选调用现有 `scrap::codec::Decoder` 回放 VP9 IVF，区别于仅显示测试图案。

Slint 提供第二窗口和输入法观察框。GPUI 当前原型的输入法、触摸与第二窗口尚未接入，不能由普通键盘回调推断这些功能已通过。

## 关键技术边界

- Slint 1.18.1 的 WGPU `Image::try_from(texture)` 接受 `Rgba8Unorm`/`Rgba8UnormSrgb`。原型另试导入 `R8Unorm`，记录是否拒绝；这个结果不等同于验证所有 YUV 格式。NV12/P010 等硬解输出需要另做纹理共享、GPU 颜色转换与同步适配。
- GPUI 上游已包含 `gpui_ios`；应以所固定提交的实际构建结果判断 iOS 能力。此提交 `gpui_platform::current_platform` 未提供 Android 分支，第三方移动端实现需要单独评估，不能当成官方后端已通过。
- GPUI 的 `Window::paint_surface` 在 macOS/iOS 可接收 `CVPixelBuffer`；同一接口没有提供 Windows/Linux 实现。这是源码能力记录，本轮 BGRA 回放原型没有验证该原生接口。
- CPU RGBA 上传会复制帧数据。两者的测试图案/VP9 软件解码结果均不能作为硬解零拷贝证明。
- `source_ready_to_submit_ms` 从帧生成/软件解码完成开始，到 UI 设置图像或 WGPU 上传调用结束；不包含网络、解码、GPU 执行、合成器与屏幕扫描时间。
- `submitted_fps` 是提交率，不是屏幕实际显示帧率；`render_callbacks` 也不是显示扫描完成次数。
- `SLINT_WGPU_CPU=1` 配合 Mesa Lavapipe 只能验证软件 Vulkan 接口路径。JSON 记录实际 Slint adapter 和是否允许 CPU adapter。
- 本机为 Debian/WSL，目前未发现 `/dev/dri` 或 `/dev/dxg`。即使图形测试通过，也不能据此宣告物理 GPU/硬解可靠。

## 构建与运行

从 xscc 根目录运行。Linux 需准备编译器、OpenSSL、Fontconfig、FreeType、XKB/Wayland/X11 的开发依赖。GPUI 还需要其上游平台后端要求的系统依赖。

```sh
cargo build --locked --manifest-path experiments/slint-validation/Cargo.toml
cargo build --locked --manifest-path experiments/gpui-validation/Cargo.toml

python3 experiments/compare-ui.py \
  --slint experiments/slint-validation/target/debug/xscc-slint-validation \
  --gpui experiments/gpui-validation/target/debug/xscc-gpui-validation \
  --width 1920 --height 1080 --fps 60 --seconds 30 \
  --output /tmp/xscc-ui-comparison-new
```

对比程序按顺序运行两者，使用同样尺寸、FPS、时长与可选 `--ivf` 文件，保留退出码、标准输出、错误、JSON；Linux 同时保存 `/usr/bin/time` 的 CPU 时间和峰值 RSS。输出目录必须尚不存在。不要并行跑两者的性能测试。

可选 VP9 路径需 Linux 下的 `--features xscc-codec`，以及 libvpx/libaom/libyuv 开发依赖；如发行版不提供 `libyuv.pc`，需为该已安装库提供正确的 pkg-config 元数据。两者必须用相同文件和构建配置运行。

Slint 单独运行支持 `--renderer software`、`--snapshot new.png`；这两个选项属于诊断手段，不用于宣称其 GPU 性能优于 GPUI。

## 正式选型前的验收

要求保持：控制端支持 Linux amd64、macOS 26 Apple Silicon、Windows 11 amd64、iOS 26 ARM64、Android ARM64/API 29+；被控端仅三个桌面平台。

每个控制端都需记录以下证据，缺项明确记为未验证或不支持：

- 完整应用构建、安装、启动及退出；交叉 `cargo check` 只算编译检查。
- 实际受管实例绑定、允许列表、加密连接、原生认证、ACL 撤销与断线清理。
- 真实硬解到显示路径、颜色正确性、尺寸切换、多屏和 DPI、前后台恢复。
- 中文输入法、组合键、修饰键释放、滚轮、相对鼠标；移动端触摸和外接键鼠。
- 同主机、同编码、同分辨率/FPS/码率，与 Flutter 基线对比提交率、丢帧、P95/P99 端到端延迟、CPU/GPU/RSS，并连续运行至少 30 分钟。

1080p60 作为基础负载；90/120 FPS 和 4K 依据实际硬件及当前串流目标另测。只有测量条件相同，才比较两个框架的数值。没有完成全部平台与完整会话验收前，不把原型判定为可直接替换。

## 回归范围

正式应用、Flutter 界面、管理库、共享协议和 xscs 服务端均未改动。新增内容仅为隔离原型、验证入口、CI 和这份记录。关闭/不构建原型时，现有应用沿用原路径。

## 官方依据

- [Slint WGPU 集成](https://docs.slint.dev/latest/docs/rust/slint/wgpu_30/)
- [Slint iOS 接入](https://docs.slint.dev/latest/docs/slint/guide/platforms/mobile/ios/)
- [GPUI 平台构造器，固定提交](https://github.com/zed-industries/zed/blob/28d0a3547f16b9609b301b48ab363bfca649c864/crates/gpui_platform/src/gpui_platform.rs)
- [GPUI iOS 跟踪说明](https://github.com/zed-industries/zed/issues/43206)
