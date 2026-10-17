# XSCC GPUI mobile management preview

This is an **iOS 26 / iPadOS 26 ARM64 management preview**, not a working remote
control client. It is a separate application with bundle identifier
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

There is deliberately no remote-connect button: video, audio, remote keyboard and
pointer input, codec integration, session handoff, and real-device lifecycle/IME
acceptance are not implemented or verified here. This management application is iOS-only. The separate
[Android backend smoke slice](../../packaging/gpui-android/README.md) adds an
experimental GameActivity rendering/touch/lifecycle implementation, pending CI and
physical-device verification; it has no IME, management UI or streaming. No host mode,
listener, desktop IPC, Flutter bridge, QUIC experiment or session code is compiled
into this preview.

## Build and verification

The independent Cargo workspace and committed `Cargo.lock` live in this directory.
Use the repository's pinned Rust toolchain on macOS 26 with Xcode 26+:

```sh
cargo fmt --manifest-path experiments/gpui-mobile-management/Cargo.toml -- --check
cargo test --locked --manifest-path experiments/gpui-mobile-management/Cargo.toml
IPHONEOS_DEPLOYMENT_TARGET=26.0 cargo clippy --locked \
  --manifest-path experiments/gpui-mobile-management/Cargo.toml \
  --target aarch64-apple-ios --bin xscc-gpui-mobile-management -- -D warnings
IPHONEOS_DEPLOYMENT_TARGET=26.0 cargo build --locked --release \
  --manifest-path experiments/gpui-mobile-management/Cargo.toml \
  --target aarch64-apple-ios --bin xscc-gpui-mobile-management
```

The `gpui-ios-management.yml` workflow builds and packages an unsigned management
preview IPA. A successful compile/package is not evidence of device execution.
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
6. Exercise repeated taps, Cancel/Confirm unbind, soft keyboard/IME, keyboard dismissal,
   safe areas, landscape, Dynamic Type/accessibility and long lists on both devices.

No real server enrollment or physical-device test has been performed by the local
source checks. Compilation, packaging and physical execution must be reported
separately.
