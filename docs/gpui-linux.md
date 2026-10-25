# GPUI Linux native preview

## Scope

Native Linux x86_64, Wayland desktop sessions only. Controller and foreground host
use the existing managed session transport, codec and portal backends. This is not
an unattended host installer; no system service, root access, device permissions
or compositor policy is changed. Keep the host application open.

The live application is selected with `live-session`; `native-host` additionally
selects the separate host identity and listener. The default diagnostic build is
not a remote desktop client. The application uses CPU-backed BGRA presentation;
GPU zero-copy and physical hardware acceleration are not acceptance claims.

## Build and package

Use Rust from `rust-toolchain.toml`, a native x86_64 Linux build environment,
Clang/libclang, pkg-config, the repository-pinned vcpkg codecs (FFmpeg, Opus, VPX,
AOM and libyuv), and the native development libraries used by GTK3, GLib,
GStreamer, PulseAudio, D-Bus, Wayland, Fontconfig, OpenSSL and xkbcommon.
Set `VCPKG_ROOT` to the pinned installation with the `x64-linux` triplet. The live
build follows the existing vcpkg codec path; `linux-pkg-config` is a separate core
build option and is not implicitly enabled by the live application.

From the repository root:

```sh
.github/scripts/check-gpui-linux.sh preflight
.github/scripts/check-gpui-linux.sh check all
.github/scripts/check-gpui-linux.sh test all
cargo build --locked --release --manifest-path experiments/gpui-validation/Cargo.toml --features live-session
.github/scripts/package-gpui-linux.sh controller 0.1.0-beta.4 /path/to/release/xscc-gpui-validation /path/to/artifacts
cargo build --locked --release --manifest-path experiments/gpui-validation/Cargo.toml --features native-host
.github/scripts/package-gpui-linux.sh host 0.1.0-beta.4 /path/to/release/xscc-gpui-validation /path/to/artifacts
```

Package each executable immediately after its role-specific build: both builds
have the same Cargo binary name. Archives include a user-session launcher,
README, license and SHA-256 checksum sidecar. They are dynamically linked native
archives, not self-contained AppImages. Packaging verifies ELF x86_64 format but
does not prove the supplied binary was built with the requested role; CI owns the
role-to-build mapping. Build checks can run in CI containers, while deployment
and functional acceptance require native desktops.

## Runtime requirements and remaining acceptance

- Start `launch.sh` as the logged-in desktop user. Do not use sudo. The launcher
  rejects root and a missing Wayland session indicator; it does not prove the
  compositor or portal is reachable.
- Host capture requires PipeWire, its GStreamer integration, session D-Bus and
  a matching xdg-desktop-portal backend supporting ScreenCast/RemoteDesktop.
  Portal consent remains mandatory. Cancellation must not grant capture/input.
- Foreground input uses the portal RemoteDesktop session. The legacy `--server`
  path selects uinput helpers and is outside this package's supported scope.
- Linux host audio starts the existing `_pa` listener in a dedicated thread at
  GPUI host initialization, so native capture can feed the existing audio service.
  It serves the audio service using a PulseAudio monitor source. Playback also
  uses PulseAudio; PipeWire desktops need PulseAudio compatibility (`pipewire-pulse`).
  Missing devices and listener errors use the existing backend logging. Thread
  creation errors are returned to initialization. No audio device is opened until
  an audio client connects. Existing live listeners are not replaced.
- Native controller/host startup retains separate identities. No system-level
  lifecycle or automatic login-screen startup is provided for this package.
- Hardware codec availability is runtime-probed, not inferred from model names.
  A successful build does not validate a driver, actual encoder/decoder or display.
- Existing GPUI limitations include no remote cursor callbacks in its session
  handler and clipboard integration remaining disabled; do not promise these.

On physical Linux desktops, accept both controller and foreground host: connect
and reconnect; approve and cancel portal permission; switch displays; verify
scaled pointer coordinates; test non-US keyboard layout, held modifiers and focus
loss; play/capture audio; close the host and verify disconnect. Test both supported
GNOME/KDE portal implementations available to the deployment rather than treating
one compositor as proof for another. Verify the package's `ldd` output has no
missing libraries before launch. Do not add blanket permissions to make a test pass.

## Validation boundary

Native CI compiles and tests the actual controller/host feature sets and checks
their packages. Local library tests and archive checks can run without a compositor;
they do not establish a working Wayland capture or input session. The Linux audio
listener hook is limited to `gpui-host`; other GPUI platforms retain their own
native startup and permission paths.
