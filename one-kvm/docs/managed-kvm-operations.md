# 构建、裸机安装、实例绑定与维护

## 构建边界

本模块位于 XSCC 仓库的 `one-kvm/`。以下源码命令均从该目录执行，使用自身 `Cargo.lock`、`rust-toolchain.toml` 和 `[patch.crates-io]`，不加入 GPUI 工作区。只部署原生二进制；`build/cross/` 中的容器仅用于编译 Linux 裸机产物。

采用固定 Rust 1.99.0，先生成真实 Rust/WASM 前端，再编译宿主；前端资源会嵌入可执行文件：

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129 --locked
cargo run --manifest-path tools/web-build/Cargo.toml --locked --release -- build --product-root .
cargo build --locked --release
```

Linux 仍需真实 FFmpeg、libyuv、ALSA、udev、D-Bus、OpenSSL 等开发依赖，以及 CMake、汇编器和 C/C++ 工具链。固定 libyuv 修订为 `957f295ea946cbbd13fcfc46e7066f2efa801233`，必须提供实际 `RGB24ToNV12` 等符号；系统旧库不能替代。依赖闭包见 `build/cross/Dockerfile.*`。使用 `cross` 编译 Linux 产物时运行 `bash build/build-images.sh`，随后 `bash build/package-deb.sh`；后者也接受 `amd64`、`arm64`、`armhf` 单架构。

Windows 在 MSVC 原生环境准备本目录 `vcpkg.json`、`vcpkg-configuration.json` 固定的 `x64-windows-static` 依赖，设置 `VCPKG_ROOT` 与 `VCPKG_INSTALLED_DIR`，再执行：

```powershell
.\build\windows\build.ps1 -Configuration release -Package
```

产物为 `target/native-windows/one-kvm_<版本>_windows_amd64.zip`，内含 Web 宿主、启动脚本、来源和许可记录；包外提供 SHA-256。顶层 `.github/workflows/one-kvm-native.yml` 构建 Linux/Windows 裸机候选，不发布容器、不自动部署。

## Linux 安装

安装本模块构建的 DEB：

```bash
sudo dpkg -i one-kvm_<版本>_amd64.deb
```

安装程序注册 `one-kvm.service`，不自动启动未配置的宿主。服务沿用历史 `/etc/one-kvm` 数据根，保留旧安装的身份与配置，使用 `UMask=0077`；硬件路径保留所需的 root 运行方式。先完成下面的本机实例创建和绑定，再 `sudo systemctl start one-kvm`。需要再次修改绑定时先 `sudo systemctl stop one-kvm`。升级或卸载保留整个数据根，不能通过删除身份来解决启动问题。

## Windows 安装与启动

解压完整 ZIP 到固定目录，用当前登录用户运行：

```powershell
.\Start-OneKvm.ps1
```

新安装默认数据根为 `%LOCALAPPDATA%\XSCC\One-KVM`；若安装目录旁已有旧版 `one-kvm/one-kvm.db`，启动脚本继续使用该目录，默认监听 `127.0.0.1:8080`；在本机浏览器打开该地址。需要局域网监听时可显式传 `-Address 0.0.0.0`，再按下文配置 HTTPS/网络可达性。

可选 `Register-OneKvmTask.ps1` 注册仅当前用户交互登录时运行的计划任务。它不自动启动、不安装 SCM 系统服务、不扩大权限或修改防火墙；完成绑定后显式 `Start-ScheduledTask -TaskName 'XSCC One-KVM Web Host'`。`Unregister-OneKvmTask.ps1` 只移除指向当前安装目录的任务，保留数据。

桌面显示器采集和 Windows 系统输入要求用户已登录并保持交互桌面。Session 0、登录屏幕、UAC 安全桌面和多个独立用户座席不在该运行方式的支持范围。系统键鼠作用于同一个桌面；真实外接 KVM 的 CH9329 等输入后端仍按可用设备配置。

## XSCS 与多实例准备

XSCS 需要支持 `backend=one_kvm` 的配套版本。每组设备分别创建一个 `controlled / one_kvm` 实例并保存各自授权码。浏览器控制端创建 `controller / one_kvm`，原生 XSCC 控制端继续使用 `controller / rustdesk`，分别授予它们到目标实例的访问关系。

旧版 XSCS v3 数据需停服后使用 XSCS 仓库的精确离线迁移工具；本模块不自动改写中央数据库。完整迁移与备份步骤见 XSCS `docs/one-kvm.md`。

一个服务最多承载 64 个本地实例（含默认实例），共用 HTTP/HTTPS 入口，各实例独占数据目录、配置、被控身份及凭据、采集/HID/音频状态及原生监听端口。实例名仅使用 CLI 允许的安全标识；不把路径或授权码当实例名。

## 本机创建与绑定

以下示例为 Linux；Windows 将 `one-kvm` 替换为 `.\one-kvm.exe`，`ROOT` 替换为所选用户数据根，使用同样参数。所有本机创建/绑定命令都须在整个宿主服务停止后运行。Linux 使用与服务相同的 root 身份执行这些命令；Windows 使用与宿主相同的交互用户，避免另建一套身份或改变私有目录 ACL。

```bash
# 默认实例直接使用旧数据根，保留原绑定与硬件配置。
one-kvm --data-dir /etc/one-kvm management status

# 为第二组设备创建实例；省略 native-port 时选择未使用的端口。
one-kvm --data-dir /etc/one-kvm --instance room-b management create --native-port 21119
chmod 600 /run/kvm-room-b-code
one-kvm --data-dir /etc/one-kvm --instance room-b management bind \
  --server https://xscs.example.com \
  --code-file /run/kvm-room-b-code \
  --address kvm.example.com:21119
one-kvm --data-dir /etc/one-kvm management list
one-kvm --data-dir /etc/one-kvm --instance room-b management status
```

调整已有实例端口可执行 `one-kvm --data-dir ROOT --instance room-b management set-port --native-port 21120`，再同步修改向 XSCS 报告的可达地址。

默认实例名为 `default`，继续读取原数据根中的数据库与 `xscs-managed`，不复制或轮换旧身份。新增实例位于 `ROOT/instances/<实例名>/`。每个实例向 XSCS 报告最多四个控制端实际可达的原生地址，NAT 外部端口可与本机监听端口不同。原生 TCP 入口与浏览器 HTTP/HTTPS 入口不同。

Windows 授权码文件也须仅当前用户可读；不能直接把授权码写进命令行。绑定完成后删除临时授权码文件。`management unbind` 仅解除选中实例的绑定，不是停止服务命令。创建或绑定本机实例后重启宿主，使 Web 路由与运行实例同步。

## Web 分配设备

启动宿主并打开 `/`，选择实例进入 `/instances/<实例名>/`，用已获授权的浏览器控制端码登录。在该实例页面选择显示器/视频源、键盘与鼠标后端、音频输入输出和平台提供的 ATX/USB/虚拟媒体等资源。配置保存在该实例的数据目录；切换实例后独立登录和操作，不沿用另一实例的 cookie、媒体或硬件配置。

宿主在 `ROOT/xscs-managed/controllers` 统一保护和复用浏览器控制端身份，沿用旧控制端资料。同一控制端授权码可以分别登录多个获授权的被控实例；实例会话、Cookie 与受控设备授权仍分别检查。退出一个实例不会影响同一控制端在另一个实例的有效会话，最后一个有效浏览器会话退出时才注销对应中央控制端凭据。

网页不能替未绑定设备完成本机 XSCS 绑定。授权只允许当前实例的配置与控制；设备能力必须以页面枚举和实际运行结果为准。共享同一操作系统桌面的输入、独占硬件和未接入设备不能因为创建了多个实例就获得物理隔离能力。

## HTTPS 与反向代理

可直接提供 TLS，也可在受控反向代理后运行。显式启用 HTTPS 时成对提供证书与私钥：

```bash
one-kvm --data-dir /etc/one-kvm \
  --address 0.0.0.0 --enable-https --https-port 8443 \
  --ssl-cert /etc/one-kvm/server.crt --ssl-key /etc/one-kvm/server.key
```

省略证书参数时在数据根 `certs/` 加载或创建自签名证书。代理终结 HTTPS 时设置浏览器外部来源，并把完整 `/instances/<实例名>/` 路径保留转发：

```bash
ONE_KVM_PUBLIC_ORIGIN=https://kvm.example.com \
  one-kvm --data-dir /etc/one-kvm --address 127.0.0.1 --port 8080
```

来源用于同源检查与 Secure cookie，不从任意转发请求头推断可信来源。代理须转发 Cookie/Origin、WebSocket 升级、长连接媒体和上传，并避免缓存认证响应；不要把所有实例路径重写到同一根目录。HTTP 代理模式要求主宿主持久配置 `web.https_enabled=false`。

## 升级与验收

用本模块配套构建离线替换二进制，保留完整数据根、各实例目录、密钥和数据库。先停宿主，备份，再更新；不得用上游在线升级包覆盖。撤权、注销或轮换某一实例凭据后，该实例已建立的控制和媒体会话应退出，其余实例继续按自己的授权运行。

分别验证 Linux/Windows 构建与包内容、真实 WASM 路径、多实例授权隔离、不同原生端口、重启后的身份持久性和设备配置；随后在对应物理机器验证显示器/采集、键鼠、音频、ATX/USB 等功能。`managed-kvm-validation.md` 中 2026-10-26 的记录是导入前历史证据，不能视为本次多实例与 Windows 改造已经通过。
