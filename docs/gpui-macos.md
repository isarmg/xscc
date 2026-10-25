# GPUI macOS native adaptation

Target: macOS 26, Apple Silicon (`aarch64-apple-darwin`). Controller and Host are
separate foreground applications. This path uses the vendored GPUI macOS/Metal
backend and native Rust session core. This is the macOS product UI entry.

## Build on an Apple Silicon Mac

Use the repository-pinned Rust toolchain, full Xcode with the Metal compiler,
and the repository-pinned vcpkg manifest with `arm64-osx` dependencies. Set
`VCPKG_ROOT` to that prepared checkout. The native dependencies include libvpx,
libyuv, Opus, AOM and FFmpeg. Do not reuse Windows/Linux build artifacts.

```sh
export MACOSX_DEPLOYMENT_TARGET=26.0
export VCPKG_DEFAULT_TRIPLET=arm64-osx
export CARGO_TARGET_DIR="$PWD/target/gpui-macos-controller"
cargo build --locked --release \
  --manifest-path experiments/gpui-validation/Cargo.toml \
  --target aarch64-apple-darwin --features live-session
python3 .github/scripts/package-gpui-macos.py \
  --binary "$CARGO_TARGET_DIR/aarch64-apple-darwin/release/xscc-gpui-validation" \
  --role controller --version 0.1.0-beta.4 \
  --output "$PWD/dist/XSCC Controller.app"
```

Build Host separately with `--features native-host`, a different
`CARGO_TARGET_DIR`, `--role host`, and `--output "$PWD/dist/XSCC Host.app"`.
The macOS native core dependency enables `screencapturekit` for system audio.
Hardware codecs remain capability-probed; Apple Silicon
is not itself proof that a specific codec/profile works in a live session.

The packager checks the actual Mach-O executable, arm64-only architecture,
macOS deployment target 26.0 and linked native library paths before creating
an app. Non-system dylibs fail packaging rather than producing a bundle that
only runs on the build machine. Build static codec dependencies. The two roles
receive distinct bundle identifiers and executable names. The beta revision
plus one is the bundle build counter, while the three-component release version
is the short version. The output must not already exist.

Packaging does not execute the binary, sign/notarize the app, install it, alter
Gatekeeper, grant permissions, or register launch services. An unsigned candidate
is not a distribution-ready release. The caller must supply the binary built
for the selected role; role selection does not convert a controller into a host.

## Permissions and lifecycle

Host currently runs only while its foreground application remains open in the
logged-in desktop session. There is no GPUI macOS launchd installer or
login-window host support. Do not reuse the legacy RustDesk service/update
installer: its identities and lifecycle are not the separate GPUI apps.

The Host requires Screen & System Audio Recording for capture and Accessibility
for injected input. Existing native permission checks are
`platform::macos::is_can_screen_recording(false)` and
`platform::macos::is_process_trusted(false)`; `false` does not prompt or grant.
Input Monitoring can affect keyboard-layout handling, and microphone access is
needed only for an explicitly selected microphone source. The GPUI settings UI
does not yet provide a complete macOS permission onboarding flow. Review grants
in System Settings and restart when macOS requires it. Controller should not be
blocked on Host capture/input privileges merely to control another computer.

Do not infer permission from a successful build or bundle creation. macOS privacy
identity depends on the final application/signature; grants for an older
application do not establish permission for these separate bundles.

## Rendering, input and audio boundaries

- GPUI uses its native Metal window renderer on macOS. Remote video still passes
  through the CPU-backed frame mailbox and GPUI image upload; this is not a
  VideoToolbox-to-Metal zero-copy decoder surface implementation.
- Host-side `server::connection::Retina::on_mouse_event` already converts frame
  coordinates to macOS logical coordinates around the display origin. GPUI must
  not divide coordinates by the display scale again. The existing native
  `retina` test covers a scale-two display with a nonzero origin.
- Modifier/key normalization, held-key release, Retina scaling, mixed-DPI monitor
  switches and high-resolution trackpad scrolling need native acceptance. Existing
  pure Rust input tests cannot establish AppKit event behavior or system shortcuts.
- ScreenCaptureKit supplies the existing native system-audio capture backend;
  playback uses the existing client audio path. Capture permission, restart and
  audio-device changes must be tested on macOS, not inferred from linkage.

## Validation and release gate

Portable packaging checks:

```sh
python3 .github/scripts/test-gpui-macos-package.py
```

On the native runner, build and test both feature sets with the exact lockfile,
then run the packager against each release binary. Exercise `--help` and the
headless codec probe (`--check-hwcodec-config --stdout`) before interactive tests.
Do not run a host/service or change privacy grants as a build smoke test.

Before calling the macOS candidate usable, record physical-Mac evidence for:

1. Both apps launch from their bundles on macOS 26 without missing dylibs.
2. Controller connects/authenticates, receives video/audio, reconnects, and closes
   without leaving remote keys/buttons held.
3. Host capture and input work after user-granted permissions; denial is visible
   and does not masquerade as a working remote desktop.
4. Command/Control/Option, shifted release ordering, Retina/mixed-DPI displays,
   monitor changes, resize and trackpad scrolling behave correctly.
5. System audio, mute, device changes and permission revocation/recovery work.
6. Sustained frame pacing, CPU/GPU usage and memory are measured on actual hardware.

Linux-hosted metadata/plist tests are not a macOS compile, native runner result,
physical-device acceptance, signing or distribution approval.
