# XSCC GPUI mobile management preview

This is an **iOS 26 / iPadOS 26 ARM64 preview**. The default build remains
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

The app uses Foundation's user-domain Application Support directory in its own
sandbox, then the manager's protected `xscc-managed` child directory. It does not
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
right-click, keyboard, inertial scrolling, audio playback or clipboard integration.
The visible view-only switch releases input and reconnects with input disabled on
the wire. The root adapter's existing `connect` API remains view-only; only the
explicit `connect_touch` entry enables pointer input, subject to current server
permission and authorization. Audio and clipboard remain protocol-disabled.

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

This management application is iOS-only. The separate
[Android backend smoke slice](../../packaging/gpui-android/README.md) is an
experimental GameActivity rendering/touch/lifecycle implementation. Its separate
acceptance status is documented there; it does not share this iOS session UI. Software decode is the initial media path; no hardware
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
   permission while dragging. Audio, keyboard and clipboard remain disabled.
7. Exercise repeated taps, Cancel/Confirm unbind, soft keyboard/IME, keyboard dismissal,
   safe areas, landscape, Dynamic Type/accessibility and long lists on both devices.

No real server enrollment or physical-device test has been performed by the local
source checks. Compilation, packaging and physical execution must be reported
separately.
