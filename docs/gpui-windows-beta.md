# GPUI Windows beta packaging / Windows beta 打包

## 界面更新（2026-10-24）

控制端和被控端采用统一的设备工作台：管理连接、授权设备和在线状态分别展示，未绑定时直接提供实例绑定表单；访问地址、解绑及会话控制仍使用原有管理操作。界面跟随系统深浅主题，并优先使用系统中文无衬线字体。

桌面和移动界面已移除临时 IP 直连入口及其界面会话路径。Windows、Linux、macOS 候选包固定为 `managed`，iOS/Android 候选工作流不再发布 IP 测试变体。底层测试模块仅保留用于已有内部回归，不属于默认应用入口。此前下载或安装的二进制不会随源码编辑自动更新，需重新构建并安装后验证界面。

## 会话与连接更新（0.1.0-beta.5）

当前候选的会话工具栏独立于画面，设备侧栏可折叠；后台状态刷新不再让操作按钮短暂失效。地址候选在完整认证后竞速选定，远程光标使用独立的状态与图像缓存。详见 [发布说明](releases/gpui-0.1.0-beta.5.md)。

## 管理握手热修复（0.1.0-beta.4）

本轮桌面候选包修复受管连接在直接密钥协商后发送 Hello 的加密边界，避免对端因明文消息解密失败而断开。Windows、Linux 与 macOS 两种角色统一使用新包版本 `0.1.0-beta.4`，不复用旧 `beta.3` 标签。详见 [发布说明](releases/gpui-0.1.0-beta.4.md)。

已安装的 Windows Host 先关闭界面，再通过原有卸载程序移除旧服务，最后正常安装新 setup；这两个安装操作可由一次管理员批准的升级助手顺序执行。卸载只移除已知安装文件与专用服务，保留受管身份和配置。升级不得执行解绑、删除管理目录或修改其 ACL；本机验收应比较升级前后的身份与绑定文件摘要。

## Scope / 范围

The independent `GPUI Windows beta candidates` workflow builds two variants of
`experiments/gpui-validation` for `x86_64-pc-windows-msvc`:

- Controller: `--features live-session`, packaged as `xscc-controller.exe`.
- Host: `--features native-host`, packaged as `xscc-host.exe`.

Each variant uses an isolated Cargo target directory. Both use the original
`xscc-gpui-validation` binary target before packaging. The workflow builds the GPUI application and native Rust core directly. Driver
installers, signing services and update-feed publishers are outside this candidate
build. Native libraries use the root vcpkg manifest and pinned revision, matching the
existing Windows native dependency set. Actual codec capability depends on the
machine and selected build features; no zero-copy claim is made and no driver is
installed.

The presentation path transfers ownership of decoded packed BGRA8 pixels to
GPUI, compacts padded rows only when needed, and reuses same-size DirectX atlas
allocations through ordered uploads. The frame owner returns to a bounded pool
after the final image reference is released. It
retains only the latest pending frame. Coalesced frame/session notifications wake
the UI; the two-second management refresh is independent of video presentation.
Hardware codec negotiation does not imply direct GPU-texture presentation.

## Native UI architecture / 原生界面架构

- `client.rs` owns the window layout and management panel. The controller places
  its scrollable management panel beside the video viewport; the host uses the
  full window for management. GPUI caches the panel independently of video frames.
- `remote_video.rs` owns the active session, connection generation, input state,
  focus handling and frame notifications. It publishes management-facing state
  only for session changes, commands and errors. Closing the view releases input
  and disconnects its session.
- `presentation.rs` owns UI-thread image resources. The session mailbox retains
  the latest pending frame; no history of decoded frames is accumulated.
- `gpui_management.rs` exposes typed status and binding commands. It decodes the
  existing owner's responses, so a successful command supplies the updated status
  without an immediate second RPC. Authorization remains in the management owner
  and transport; the interface does not accept a caller-supplied transport address or permit.
- `gpui_session.rs` adapts native media and input; `gpui_host.rs` owns process and
  service startup. Both applications continue using the existing native runtime.

Video notifications invalidate the video view and thin window layout. The cached
management panel is rebuilt for its own state, input or viewport changes, rather
than for each decoded frame. This boundary reduces UI work without changing the
decoder, transport or CPU-to-GPU upload path.

## Controls / 操作

在绑定表单中使用 Tab / Shift+Tab 切换输入框，Enter 提交。绑定后从服务端
授权列表选择主机；连接失败时查看连接提示，管理服务离线时可重新连接服务端。

点击远程画面后发送键鼠，Ctrl+Alt+Enter 切换窗口全屏。切换窗口或断开连接
会释放已按下的键鼠。此版本仍使用文本/快捷键输入，不等同于游戏所需的原始扫描码捕获。

多屏主机可在“屏幕”栏切换。切换期间暂停输入，收到主机确认后显示新画面；
若主机未响应，可断开后重新连接。窗口缩放保持画面比例；会话中的分辨率设置则修改当前远端显示器的模式，仅提供主机声明支持的分辨率。

状态栏区分目标 FPS 和解码输出 FPS。接收吞吐包括协议流量，网络往返来自
现有探测；这些数值不代表实际显示帧率或端到端操作延迟。音频沿用原生播放
路径，仍需在 Windows 控制端和被控端上验证实际播放。

控制端和被控端分别打包；候选构建成功不代表真实串流、Windows 11 图形或服务验收通过。
禁止把测试图案、仅能打开的窗口或通过 --help 的程序宣传为完整远程桌面。

## Produce candidates / 构建候选包

1. Commit the intended source and confirm its SHA on GitHub. A push on
   `main` or `feat/management-prep` touching the workflow's bounded paths builds
   `0.1.0-beta.5` candidates; build-info identifies the exact revision.
2. Alternatively run `workflow_dispatch` on the intended ref with the explicit
   label `0.1.0-beta.5`. Labels name packages only; they do not change crate
   versions, create tags or publish. GitHub may require a manual workflow to be
   present on the default branch; the bounded push trigger covers initial setup.
3. Require both matrix jobs to pass: formatting, Clippy, unit tests, release
   build, executable `--help`, AMD64 PE/signature checks, NSIS build and upload.
   The controller also gets silent install/uninstall smoke checks on the CI
   server, including uninstall registration and unknown-file preservation.
4. Each artifact contains its unsigned installer and a ZIP: controller uses
   `*-portable.zip`, host uses `*-files.zip` (file archive, installation required).
   External metadata, README, dependency inventory and checksum filenames all
   have the same role/version/platform prefix as the installer, so both artifacts
   can be attached to one Release without filename collisions. Inside each package,
   `build-info.json`, `README.txt` and `dependencies.txt` retain their short names.
   JSON records role, source SHA, lockfile hash and
   exact build run. Verify hashes before installation; hashes are not signatures.

The workflow has `contents: read`, needs no signing credentials, and cannot
publish a Release. NSIS 3.11 comes from the Chocolatey registry. Rust 1.99.0 and
existing action pins follow the repository. Windows Server 2025 is a build and
installer-smoke runner, **not Windows 11 runtime acceptance**. The separately authorized host smoke test installs and starts only its dedicated
LocalSystem service on the disposable runner, keeps it unbound, verifies its
SCM path/account, then uninstalls it and checks identity/unknown-file retention.
It supplies no server address or credentials and makes no connection to a user
device. It does not change firewall or security settings. Identity read failures
are reported without changing ACLs; TCP observations are not a packet-level
proof of zero external traffic. Unavailable pinned dependencies must fail the build, not cause
silent version switching or removal of `--locked`.

Both installers and archives include the application's unchanged `LICENCE`,
`THIRD_PARTY_NOTICES.txt` and the full upstream `GPUI-LICENSE-APACHE.txt` from
the vendored GPUI source originating at Zed commit
`28d0a3547f16b9609b301b48ab363bfca649c864`. XSCC maintains it independently in
`vendor/gpui`; application dependencies resolve to local paths. The experimental
Windows surface implementation does not change the production CPU-backed decoder
frame path. The notice identifies
the adapted text-input example and XSCC's changes. These notices do not assert
that all dependencies use one license; broader dependency review remains a gate.
The host matrix explicitly tests `management::host_tests` in addition to native
GPUI and IPC authorization tests. With hardware codec support enabled, CI also
runs `client::hwcodec::tests` and checks the release executable through redirected
stdout/stderr: `--help`, rejection of the probe without `--stdout`, and one valid
`RUSTDESK_HWCODEC_CONFIG=` JSON record. The codec probe has a 45-second external
timeout; an empty capability list is valid and is not evidence of real GPU support.

## Windows manifest ownership

The pinned GPUI platform embeds the single application manifest, including DPI,
heap, compatibility and common-controls settings. The application build script
must not embed a second MANIFEST#1 resource. After linking, the Windows release
workflow adds the required top-level `XSCC.Application.controller` /
`XSCC.Application.host` identity (`win32`, `amd64`), using the beta version's four
numeric components (each 0–65535). GPUI omits this application identity. The SDK
manifest tool also changes the host's execution level to `requireAdministrator`;
controller remains `asInvoker`. It re-extracts and checks
all manifest content before CLI smoke, hashing or packaging. A validation failure
blocks the candidate. An unprepared development host executable must be launched
from an already elevated session; the actual host installer ships the verified
administrator manifest.

## Runtime dependency evidence / 运行库依赖

The Windows target explicitly sets `-C target-feature=+crt-static` for both
variants, in addition to static vcpkg libraries. Packaging records MSVC
`dumpbin /DEPENDENTS` output in `dependencies.txt` and fails if the executable
imports a VC++ redistributable runtime such as VCRUNTIME or MSVCP. No redistributable,
driver or arbitrary DLL is downloaded on the user's device. The optional virtual
display driver is bundled with the host and registered on demand for an authorized virtual-display request or headless session.
This direct-import inspection is not proof that every dynamically loaded graphics
library is available; actual clean Windows 11 startup remains a release gate.

## Installer behavior / 安装行为

Both packages check x86_64 architecture and OS build >= 22000. Supported runtime
is Windows 11; the installer smoke runner is not a supported end-user target.
Both application installers are unsigned and preserve identity/configuration on removal.
They make no firewall, security-policy or update-feed modifications. The host bundles the
signed virtual display driver files described below; driver registration is deferred until
an authorized session explicitly requests a virtual display or needs headless provisioning.

Controller:
- Normal-user installation under `%LOCALAPPDATA%\Programs\XSCC-Controller`.
- Current-user Start menu shortcut and HKCU Apps uninstall registration.
- No service registration or administrator request.

Host:
- Administrator installer under `%ProgramFiles%\XSCC-Host`.
- Dedicated `XSCC Host` LocalSystem service via `--install-service-only`; its
  service process uses `--service`/`--server`. Host UI requires administrator
  approval via its embedded executable manifest.
- All-users Start menu shortcut and HKLM Apps uninstall registration.
- Uninstall calls only `--uninstall-service-only` before removing known files.
  On failure files remain for diagnosis. Service-owned identity is preserved.
- This first beta requires uninstall before reinstall/upgrade to avoid overwriting
  a running service binary. A pre-existing host executable blocks installation.
- The host `*-files.zip` is only a file archive. Use its setup installer; the
  executable requires the protected installation path and dedicated SCM service.

Do not disable security controls to bypass unsigned-publisher warnings. No
publisher identity or signed-build claim is made.

## Promotion gate / 发布门槛

Before publishing a remote-streaming beta, retain evidence for the exact SHA and
artifacts covering:

- Clean Windows 11 x86_64 controller install/uninstall as a standard user.
- Host administrator install, dedicated service identity/path, correct service
  start/stop/removal and retained identity after reinstall.
- GUI rendering and clean exit on actual Windows graphics hardware; default
  startup must not silently present a diagnostic pattern as remote content.
- Genuine authenticated peer connection, decoded remote frames and actual
  keyboard/pointer delivery between controller and host.
- Rejection, timeout and disconnect states shown honestly, with recovery tested.
- No unrequested listener, privilege grant, driver or firewall/security changes.
- Clean-machine runtime DLL inventory and third-party license/source review.
- No credentials/private state in artifacts, diagnostics or release notes.

CI build success is not a substitute for these runtime checks. List unvalidated
features explicitly, including audio, clipboard, file transfer, elevated desktop,
hardware codecs and unattended access, rather than implying they work.

After the release owner approves the verified scope, create a new immutable tag
at the tested SHA (suggested `gpui-v0.1.0-beta.1`) and a GitHub **prerelease**, not
latest/stable. Attach both role artifacts from the same successful candidate run,
including hashes/build metadata and source link. State `Windows 11 x86_64 /
unsigned / beta` and actual validation limits. Never overwrite tags or mix source
revisions. This workflow does not automate publication or create placeholders.

## Existing paths / 现有路径

The GPUI Cargo package remains `xscc-gpui-validation` at `0.1.0`, while the
management crates use `2.0.0-dev`. The candidate `PACKAGE_VERSION` and actual
source SHA are recorded separately by packaging. A Cargo package version does
not identify a published release or establish a signed-build claim. The desktop
product entries are the separately built Controller and Host described above.

NSIS references: [normal-user execution level](https://nsis.sourceforge.io/Reference/RequestExecutionLevel)
and [scripting reference](https://nsis.sourceforge.io/Docs/Chapter4.html).


## Virtual display driver / 虚拟显示器驱动

Native Windows Host defaults to the configurable Virtual Display Driver 25.7.23 backend.
The original AMD64 driver-only archive is pinned in
`packaging/gpui-windows/virtual-display-vdd-driver.json`. Package preparation checks its
size and SHA256, every extracted file hash, the AMD64 PE architecture, valid signatures
of both the DLL and catalog, and the pinned SignPath Foundation publisher. The original
MIT license is included as `mtt_vdd/VDD-LICENSE-MIT.txt`, with source provenance in
`mtt_vdd/driver-source.json`. Existing Amyuni files and their original license/instructions
remain in `usbmmidd_v2` for compatibility; that license includes advertisement-page consent.
Controller packages contain neither driver. No driver or certificate is installed on CI.

Installing Host copies the files into Program Files. A permitted explicit add request
with keyboard control, or the existing authorized headless-login path, uses normal
Windows SetupAPI to install the driver on demand. A background display poll does not
install it. Headless provisioning opens an already permitted remote desktop and does
not require keyboard control. Windows driver-signature enforcement remains enabled;
no test certificate or test-signing policy is introduced.

The owned configuration merges all raw adapter modes and existing custom resolutions
and refresh rates. It has no application-defined four-screen, ten-mode or 60 Hz ceiling.
The driver exposes no reliable maximum monitor count; `null` represents an unknown
maximum, with each requested count confirmed against actual Windows enumeration. Mode
changes are tested, applied and read back, including changes to Hz without resizing.
Actual capture, encoder and receiver rendering capabilities still apply. The newest
virtual monitor can be removed, retaining the rule that protects the only display.

Configuration is written atomically to protected `mtt_vdd/config`; a failed change
restores the prior configuration and device state and reports failures. Existing VDD
devices/configuration owned outside XSCC are not taken over. Normal device restart
is used rather than the upstream pipe reload operation. Uninstall removes only the
known bundled files and preserves owned configuration and the registered shared driver.
Build, signature and configuration tests do not establish physical GPU compatibility,
maximum display count, streaming throughput, or end-to-end latency.
