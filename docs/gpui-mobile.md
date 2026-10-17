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


## First-frame follow-on (explicit `mobile-session` feature)

The iOS preview has an opt-in software-decoded session path with direct touch and an explicit view-only mode. It links the existing
Rust client with `gpui-mobile-session` and no default features, attaches the already
opened manager, and reuses authorization, TCP login, software decode and the BGRA
presentation helper. Desktop and Flutter cfg behavior is retained when this feature
is off. Clipboard remains disabled in protocol options and locally. Foreground
output-only audio is opt-in per session, using the existing Opus decoder and CPAL
output; it never requests microphone access.
The direct-touch mode enables pointer input only through an explicit session
capability, with current server permission and authorization checks; view-only
switching releases input and reconnects with input disabled. The root `connect`
entry remains view-only. Software-keyboard commits and a control-key bar use a separate explicit keyboard
capability. Preedit stays local; macOS/Windows allow native Unicode while Linux
allows only printable ASCII plus separate control keys. No clipboard fallback,
complete hardware-keyboard integration or inertial scrolling is added.
Right-click is an explicit one-shot next-tap action, cancelled by dragging, black
bars, multiple fingers or lifecycle/input-permission changes; device validation
remains open.
Unmodified non-repeating Enter/Tab events also support Android IME actions; the
identical hardware control events are accepted without claiming full hardware input. Native build success and real-device first-frame/revocation tests
remain separate acceptance gates. No physical-device streaming is claimed.

### Foreground remote audio

The sound button starts disabled for each connection and uses system volume.
It reuses Opus decoding, streaming resampling and the CPAL output backend;
iOS owns a Playback AVAudioSession and Android requests foreground media audio
focus using the existing GameActivity context. No input stream, recording
permission, background service or background-audio entitlement is added.

Mute, permission withdrawal, inactivity and disconnect close the output gate and
discard pending audio. Platform interruption or output failure stops playback and
shows a retry message; the user explicitly enables sound again. The mobile packet
queue is capped at 64 frames / 256 KiB, rejects frames above 64 KiB, and discards
packets resident locally for over 150 ms. A discard resets decoder/resampler
continuity and clears PCM; mobile PCM capacity is also 150 ms. These are local
resource/queue budgets, not measured end-to-end latency. Desktop/Flutter playback
keeps its prior path and buffer policy.

Physical-device acceptance must cover audible remote mono/stereo playback, system
volume, mute/disconnect without old audio replay, headset/Bluetooth route changes,
focus/phone interruption, background/foreground and reconnect. Compilation and
packaging do not establish those results.
