# XSCC GPUI mobile management preview

This is an **iOS 26 / iPadOS 26 / Android API29+ ARM64 preview**. The default build remains
management-only. The explicit `mobile-session` build adds the software-decoded screen and direct-touch
session slice; device connection and first-frame acceptance are still required. It is a separate application with bundle identifier
`org.sarmg.xscc.gpui.preview`; the delivered Flutter applications remain unchanged.
The unsigned IPA must be signed by the user with their own valid Apple identity
and provisioning profile before device installation. CI does not create signing
credentials or claim physical-device acceptance.

Implemented paths use the real `xscc-native-management::NativeManager`:

- Persistent installation UUID, private binding store, server settings and single-owner lock.
- HTTPS enrollment using the server-issued **controller** instance password,
  restoring a saved binding, and explicit two-step local unbind.
- Authorized device names, identities and online state from the current management
  policy; no fixtures or fake device rows. Revoked/expired authorization is handled
  by the existing manager and its policy, without a second authorization cache.
- Serialized async operations outside the UI thread. Repeated action clicks are
  ignored until completion. Backgrounding shuts down the owner; returning to the
  foreground resumes it and refreshes the policy. Shutdown before iOS suspension
  is best-effort; durable settings are written as each operation completes.
- The existing GPUI desktop palette, Mac-mini contour and native text field,
  with system safe-area/keyboard bounds and a scrollable mobile layout.

On iOS the app uses Foundation's user-domain Application Support directory; on
Android it uses AndroidApp.internal_data_path. Both use the manager's protected
`xscc-managed` child directory in the application sandbox. It does not
reuse a desktop home directory, Flutter's app sandbox, or a second management
identity for each window/session. The standalone preview has its own installation
identity because it is a separate app; future media integration must use this same
manager, its grants and session checks.

## Optional screen and touch session slice

The `mobile-session` feature adds a real “view screen” action to online authorized
controlled devices. It attaches the existing `Arc<NativeManager>` to the native
session adapter, reuses its target/proof/periodic authorization checks and existing
TCP application login/decoder, and uploads owned BGRA frames through GPUI. There
is one manager and one application-owned Tokio runtime; no Flutter bridge, desktop
IPC, host listener, capture, additional binding store or QUIC feature is enabled.

The UI defaults to direct touch: tap to left-click, move one finger beyond the
threshold to drag, and move two fingers to scroll. A second finger releases an
active drag; cancellation or a third finger cancels the gesture. There is no
right-click, hardware-keyboard integration, inertial scrolling or clipboard integration.
The visible view-only switch releases input and reconnects with input disabled on
the wire. The root adapter's existing `connect` API remains view-only; only the
explicit `connect_touch` entry enables pointer input. The app uses the explicit
`connect_interactive` entry for touch plus the software keyboard, subject to current
server permission and authorization. Clipboard remains protocol-disabled. Remote
audio is output-only and initially off for each connection; the sound button
enables it using system volume. Mute, inactivity, interruption and disconnect stop
playback and discard old audio. See the [audio boundary and device acceptance](../../docs/gpui-mobile.md#foreground-remote-audio).

The remote software keyboard submits final IME commits naturally; preedit remains
local. A short local suffix supports composition and grapheme deletion, with a
position-isolated synthetic anchor for empty-document Backspace. This is not a
copy of the remote document. Pointer/control-key navigation, focus loss, background,
mode changes and revocation cancel that context. Edits outside the known suffix
are rejected rather than guessing remote selection. Failed sends clear context.
There is no text logging or clipboard fallback. macOS and Windows targets use native
Unicode text injection; Linux targets accept printable ASCII only. Return, Tab and
the control-key bar use separate single-press protocol commands. Hardware keyboard
support and native Wayland Unicode remain separate unfinished work. Unmodified,
non-repeating Enter/Tab key-down events are accepted for Android IME actions;
the identical hardware control events also work, but other hardware keys are not
integrated. IME actions finalize marked text and emit the control in one batch.

Touches beginning in contain black bars produce no remote input. Dragging outside
the image clamps coordinates; move packets are frame-coalesced, and button edges
are preserved. Focus loss releases held input; background, revocation and disconnect
also clear queued movement and the image. If a release cannot be sent, the session
is closed and no further input is sent. Delivery during network loss or iOS
suspension is best-effort, not a guaranteed remote acknowledgement; host-side
held-button cleanup must also be validated.

Disconnect, background/disconnected lifecycle, and management mutation clear the
session and invalidate its image. Generation checks reject late connection results;
frame consumption checks current authorization. Server revocation also closes the
native transport through its existing periodic permit check. Returning to the
foreground does not automatically reconnect the screen session.

Android uses the same Panel and NativeManager through the Android-only cdylib
entry; the existing iOS binary entry is unchanged. Both platforms enable screen
and pointer input only with `mobile-session`. The separate
[Android backend smoke slice](../../packaging/gpui-android/README.md) remains an
offline regression baseline. Software decode is the initial mobile media path; no hardware
decoding, zero-copy performance or successful device first frame is claimed here.

## Build and verification

The independent Cargo workspace and committed `Cargo.lock` live in this directory.
Use the repository's pinned Rust toolchain on macOS 26 with Xcode 26+:

```sh
cargo fmt --manifest-path experiments/gpui-mobile-management/Cargo.toml -- --check
cargo test --locked --manifest-path experiments/gpui-mobile-management/Cargo.toml
IPHONEOS_DEPLOYMENT_TARGET=26.0 cargo clippy --locked \
  --manifest-path experiments/gpui-mobile-management/Cargo.toml \
  --target aarch64-apple-ios --features mobile-session --bin xscc-gpui-mobile-management -- -D warnings
IPHONEOS_DEPLOYMENT_TARGET=26.0 cargo build --locked --release \
  --manifest-path experiments/gpui-mobile-management/Cargo.toml \
  --target aarch64-apple-ios --features mobile-session --bin xscc-gpui-mobile-management
```

The `gpui-ios-management.yml` workflow builds and packages an unsigned screen/touch
preview IPA with the repository’s iOS codec dependencies. Omit `mobile-session`
to retain the management-only source path. A successful compile/package is not evidence of device execution.
The non-iOS executable exists for host compilation/tests only; it reports the
unsupported app platform rather than opening a production binding store.

## Device acceptance still required

1. Self-sign/install on an iPhone and iPad running OS 26; verify launch and rotation.
2. Enter a valid HTTPS server and controller instance password. Confirm real rows
   match the server policy; reject a controlled/host instance password.
3. Close/relaunch: the same persisted UUID/binding resumes without re-enrollment.
4. Revoke or expire the binding on the server: stale peers disappear; restore valid
   authorization and confirm recovery. Check TLS/network failure and wrong codes.
5. Background during enrollment/reconnect/unbind, return repeatedly, and verify
   one owner, no UI blocking, and correct durable results after forced termination.
6. In the session build, select a real authorized device, verify the first frame,
   disconnect, revoke during playback, background during connection, and reselect
   rapidly. Old images/connections must never revive. Verify taps, drag release outside
   the image, two-finger scroll, system cancellation and background during a drag.
   Switch to view-only and verify the host refuses input; revoke server input
   permission while dragging. Exercise software-keyboard composition/commit/cancel,
   emoji deletion, empty Backspace, navigation, and view-only rejection. Verify
   Linux non-ASCII is explicitly rejected without altering the clipboard.
   Full hardware-keyboard integration and clipboard remain disabled.
7. Enable remote sound, test mute and system volume, then interrupt audio focus,
   unplug headphones, background, disconnect and reconnect. Old audio must not
   replay; interruptions require explicitly enabling sound again.
8. Exercise repeated taps, Cancel/Confirm unbind, soft keyboard/IME, keyboard dismissal,
   safe areas, landscape, Dynamic Type/accessibility and long lists on both devices.

No real server enrollment or physical-device test has been performed by the local
source checks. Compilation, packaging and physical execution must be reported
separately.

## Android management entry

Build the `xscc_gpui_android_management` cdylib from this workspace with
`--lib --no-default-features --features mobile-management,mobile-session --locked` through
`cargo ndk -t arm64-v8a -p 29`. Packaging reuses the existing Android Activity,
protected IME bridge and final-DEX privacy gates. The management flavor has
INTERNET permission; the smoke flavor retains its no-network contract. Omit
`mobile-session` for management only. The session build uses the existing pinned
Android codec dependencies (VPX/AOM software decode), without `mediacodec`,
`hwcodec`, `flutter` or capture features. GameActivity owns the Android context;
this entry never invokes Flutter JNI initialization or replaces ndk_context.
The root session configuration home is the same private directory, without
reading another application's credentials. The existing NativeManager TLS and
authorization policy remain in force.

The entry obtains its private directory only from AndroidApp.internal_data_path;
it never uses HOME, a desktop configuration path, external storage or Flutter
credentials. NativeManager creates the existing xscc-managed directory and owns
the installation UUID, binding and runtime task. The appId is
org.sarmg.xscc.gpui.preview, separate from both smoke and Flutter; each app has its
own installation identity and requires its own server-authorized enrollment.
Initial management activity follows the actual Android window focus/visibility;
the shared lifecycle observer handles subsequent foreground/background changes.
The Android session adapter holds only a weak manager reference, so Activity
destruction does not permanently retain the installation lock. If old work still
holds the previous owner during recreation, initialization fails explicitly rather
than replacing it or opening a second owner. Runtime shutdown remains best-effort.

Android acceptance must verify protected text editing, first enrollment, policy
list updates, restart preserving identity/binding, two-step unbind, and
Home/resume/rotation. The session build also requires the first-frame, touch,
view-only, revocation and background-during-drag checks above on Android.
Successful cross-compilation and APK/privacy checks do not
establish physical-device operation or a working remote-control session.
