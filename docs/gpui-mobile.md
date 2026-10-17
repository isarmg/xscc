# GPUI mobile: management preview boundary

The first deliverable is the independent
[`gpui-mobile-management`](../experiments/gpui-mobile-management/README.md) iOS 26 /
iPadOS 26 ARM64 management preview. Its unsigned IPA is a self-signing input, not a
complete remote-control release. Existing Flutter mobile packages remain the
working baseline, subject to their own device acceptance.

## What the vendored backend actually provides

`vendor/gpui/crates/gpui_ios` implements UIKit application/scene lifecycle,
Metal-backed window presentation, native text input, touch delivery, safe areas
and keyboard occlusion. `gpui_platform::current_platform` selects it on iOS.
Existing framework-validation builds only proved compilation/linking of the
framework probe; they did not exercise a production controller session or install
an application on an iPhone/iPad.

The starting facade had no Android branch or `gpui_android` crate. A separate
[Android backend smoke slice](../packaging/gpui-android/README.md) now introduces a
GameActivity-based rendering, touch, font and lifecycle implementation for ARM64 /
API 29+. Its CI and physical-device verification are pending; it does not provide
IME, management binding or remote streaming. This iOS management app is not an
Android deliverable. Android cannot be enabled by adding it to the desktop cfg.

## File and runtime boundaries

- `experiments/gpui-mobile-management`: separate workspace/lock, executable-owned
  Tokio runtime, in-process manager adapter, management UI and native sandbox path.
- Shared desktop `theme.rs` and `text_field.rs`: imported by path. The existing
  theme only gains an opt-in `mobile-management` feature gate. Desktop defaults
  and production UI paths are unchanged.
- `crates/native-management`: reused as-is for protected storage, UUID, enrollment,
  policy/revocation, management connection, controller-role restrictions and unbind.
- `packaging/gpui-ios` and the dedicated workflow: minimum OS/device family,
  unsigned bundle construction and archive validation.

No root desktop cfg is loosened. `librustdesk`, desktop host/listener/IPC, Flutter
and codecs are excluded from this slice. The forthcoming session adapter must
reuse the existing server authorization, session proofs and decoder business
logic; it must not invent another protocol, bypass grants, or own a second identity.
Android may reuse the management adapter and appearance after its backend is
validated and IME is implemented; storage and lifecycle entry remain platform-specific.

## Verification limits

Local checks intentionally avoid native builds in the space-constrained Linux
executor. Dedicated macOS/iOS CI must establish compile, test and link results for
the exact revision; the preview README records the physical-device acceptance
matrix. Simulator, packaging, host tests and device tests are different evidence.
Neither a green build nor an unsigned IPA is a claim that remote control works.
