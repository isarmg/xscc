# One-KVM Rust Web

产品界面使用与 XSCS 同源的定制 xcss-web / Leptos CSR。Axum 提供本机 API、媒体与输入，XSCS 管理设备及控制端的实例身份和授权关系。浏览器只持有同源会话 Cookie 与内存 CSRF，设备凭据和配对密钥保留在服务端。

```sh
cargo +1.99.0 run --locked --manifest-path tools/web-build/Cargo.toml -- build --product-root . --offline
```

命令从 XSCC 的 `one-kvm/` 目录运行，需要 wasm32-unknown-unknown 与 wasm-bindgen 0.2.129。`WASM_BINDGEN` 可指定本机已验证的工具。生产构建不运行 npm、Vite、Vue 或 Pinia。

`xcss-web/src` 提供应用路由、表单、状态和交互，`src/media.js` 仅封装浏览器 Fetch、WebRTC、WebSocket、WebCodecs 及文件上传。构建器和完整框架源码/许可证保存在仓库内，导入记录见 `STACK-SOURCE.json`。`source-archives/` 保留该记录中三个完整原始组件目录的 Git 快照，并保存导入时的文件清单。`tools/web-build/SOURCE.json` 记录构建器当前文件摘要与多实例相对资源路径修改，原始归档和来源修订保持不变。历史 Vue 源码放在 `legacy-vue/`，不参与产品构建或失败回退。

当前界面提供：

- XSCS 控制端授权码登录、服务端会话恢复、授权后的硬件初始化。
- WebRTC / MJPEG 画面、浏览器及服务端共同支持的编码选择、码率预设、截图、全屏。
- 绝对 / 相对鼠标、物理及虚拟键盘、组合键、ASCII 文本粘贴、音频播放及 UAC 麦克风。
- ATX、网络唤醒、镜像上传 / 下载 / 挂载、虚拟 U 盘及文件操作。
- 每实例的显示器/视频源、键鼠后端、音频设备、ATX 和虚拟媒体设置，以及原生 XSCC 编码、监听端口和可达地址上报。
- Windows 显示器采集与桌面键鼠，键盘和鼠标分别启停，WASAPI 输入和系统播放音频回环选择。
- 按平台能力显示蓝牙配对、OTG 自检和 Computer Use 会话。整机终端、USB 复位、服务重启、Web 监听和看门狗配置由本机管理员维护。

所有实例共用同一份 Web/WASM 资源，入口为 `/instances/<id>/`。HTTP API、WebSocket、WebRTC 信令、MJPEG、上传、下载和截图始终留在当前实例前缀下，浏览器 Cookie 按实例路径分隔。页头显示当前实例，切换实例入口返回本机实例列表。各实例独立验证其 XSCS 控制授权，设备配置不跨实例复制。

Windows 桌面输入使用登录用户的共享桌面。显示器分配映射鼠标坐标，不创建独立 Windows 会话，也不隔离实体键盘或鼠标；通过多台 CH9329 或其他硬件输出后端连接不同被控设备时应分别选择实际连接的设备。

原生端口与上报地址分别编辑，适用于 NAT 外部端口不同的部署。绑定 XSCS 设备只由本机命令行执行。浏览器不提供本机账户、旧 RustDesk 密码、VNC / RTSP / Redfish 独立入口或上游二进制更新开关。旧版的可拖动控制台布局、接线示意图和任意扩展 JSON 编辑器保存在历史源码中，未迁入生产界面；现有设置使用明确字段表单。输入法文本仍受 USB HID 键盘布局限制，当前粘贴支持 ASCII。

Opus 使用本地固定版本的原始 ESM 源码，运行时不访问 CDN。`vendor/opus-decoder/` 保存完整 npm 发行包及依赖、注册表元数据、校验和、原始许可证及单独标记的补充许可文本；说明见该目录的 `NOTICE.md` 和 `SOURCE.json`。框架与资源还保留原来的来源及许可记录。

可从仓库根运行源码检查：

```sh
node --test web/tests/instance-paths.mjs
cargo +1.99.0 fmt --manifest-path web/xcss-web/Cargo.toml -- --check
cargo +1.99.0 test --locked --manifest-path web/xcss-web/Cargo.toml --lib
cargo +1.99.0 clippy --locked --manifest-path web/xcss-web/Cargo.toml --target wasm32-unknown-unknown --all-targets -- -D warnings
```

`tests/managed-browser.mjs` 以当前 `dist` 启动隔离的同源 API / 媒体替身并运行 Chromium 与 Firefox，覆盖真实 WASM 挂载、实例前缀与 Cookie 边界、会话与 CSRF、Windows 显示器及键鼠初始化、每实例视频/音频设备选择、原生设置及 NAT 地址、上传、WebRTC 环回视频解码及 RTCDataChannel 输入、MJPEG、本地 Opus 解码，以及 Fetch / 上传 401、退出及迟到麦克风权限结果的媒体清理。Playwright 只用于验收，通过 `PLAYWRIGHT_MODULE=/absolute/path/to/playwright/index.mjs node web/tests/managed-browser.mjs` 指定隔离安装；结果默认写入 `/tmp/one-kvm-web-browser.json`。该测试不证明采集硬件、真实 XSCS 服务或原生 XSCC 全链路已通过验收。
