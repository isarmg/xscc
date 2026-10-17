# GPUI Windows beta packaging / Windows beta 打包

## Scope / 范围

The independent `GPUI Windows beta candidates` workflow builds two variants of
`experiments/gpui-validation` for `x86_64-pc-windows-msvc`:

- Controller: `--features live-session`, packaged as `xscc-controller.exe`.
- Host: `--features native-host`, packaged as `xscc-host.exe`.

Each variant uses an isolated Cargo target directory. Both use the original
`xscc-gpui-validation` binary target before packaging. The workflow never calls
Flutter, Dart, the old RustDesk installer, driver installers, signing services,
or update-feed publishers. Existing workflows are unchanged. Native libraries use the root vcpkg manifest and pinned revision, matching the
existing Windows native dependency set. Actual codec capability depends on the
machine and selected build features; no zero-copy claim is made and no driver is
installed.

The current presentation path copies decoded pixels into an owned, packed BGRA8
frame (explicit row stride), then uploads it through GPUI's image renderer. It
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
若主机未响应，可断开后重新连接。画面按窗口比例缩放，不会修改主机分辨率。

状态栏区分目标 FPS 和解码输出 FPS。接收吞吐包括协议流量，网络往返来自
现有探测；这些数值不代表实际显示帧率或端到端操作延迟。音频沿用原生播放
路径，仍需在 Windows 控制端和被控端上验证实际播放。

控制端和被控端分别打包；候选构建成功不代表真实串流、Windows 11 图形或服务验收通过。
禁止把测试图案、仅能打开的窗口或通过 --help 的程序宣传为完整远程桌面。

## Produce candidates / 构建候选包

1. Commit the intended source and confirm its SHA on GitHub. A push on
   `feat/management-prep` touching the workflow's bounded paths builds candidates
   with label `0.1.0-beta.1`; build-info identifies the exact revision.
2. Alternatively run `workflow_dispatch` on the intended ref with an explicit
   label such as `0.1.0-beta.2`. Labels name packages only; they do not change crate
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
driver or arbitrary DLL is silently downloaded or installed on the user's device.
This direct-import inspection is not proof that every dynamically loaded graphics
library is available; actual clean Windows 11 startup remains a release gate.

## Installer behavior / 安装行为

Both packages check x86_64 architecture and OS build >= 22000. Supported runtime
is Windows 11; the installer smoke runner is not a supported end-user target.
Both are unsigned, preserve identity/configuration on removal, and have no driver,
firewall, security-policy or update-feed modifications.

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

At checkout `6227e14efbae64de9cfbef32d5cd6e046a6258b4`, the GPUI crate is named
`xscc-gpui-validation` at `0.1.0`; the fetched repository tag is `v1.0.0`.
These are distinct from the main management crates' `2.0.0-dev` version.
The legacy RustDesk Windows pipeline uses Flutter, per-machine MSI custom
actions and optional `SIGN_BASE_URL`/`SIGN_SECRET_KEY` signing. Their presence
in YAML does not establish that valid signing credentials are configured.
This preview does not reuse that pipeline or make a signed-build claim.

NSIS references: [normal-user execution level](https://nsis.sourceforge.io/Reference/RequestExecutionLevel)
and [scripting reference](https://nsis.sourceforge.io/Docs/Chapter4.html).
