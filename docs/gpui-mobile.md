# GPUI mobile native application

Android ARM64 (API 29+) and iOS / iPadOS 26 ARM64 use the same
[`gpui-mobile-management`](../experiments/gpui-mobile-management/README.md)
product UI workspace. The `mobile-management,mobile-session` build includes
authorized screen sessions, touch, software-keyboard commits and opt-in remote
audio. The management-only default is a diagnostic variant. The existing
`experiments/` path and preview package identifiers are retained; they do not
select another frontend.

## Platform backends and packages

`gpui_ios` provides UIKit lifecycle, Metal presentation, native text input,
touch, safe areas and keyboard occlusion. `gpui_android` provides the ARM64
GameActivity window/rendering and lifecycle path, with the protected Java IME
integration used by the management APK. Both are selected by the vendored
`gpui_platform` facade.

Android native build and final APK gates are described in
[`packaging/gpui-android`](../packaging/gpui-android/README.md). Its `smoke`
flavor is a separate no-network platform test, not the controller product.
iOS native device packaging produces an unsigned preview IPA; installation
requires the user's valid Apple signing identity and provisioning profile.
Dedicated native CI checks compilation, linkage and packages. Physical-device
streaming, touch, IME, audio and lifecycle acceptance remain separate evidence.

## File and runtime boundaries

- `experiments/gpui-mobile-management`: application-owned Tokio runtime,
  in-process manager adapter, GPUI management/session UI and native sandbox path.
- Shared desktop `theme.rs`, `text_field.rs` and `presentation.rs`: reuse the
  GPUI appearance, text editing and owned-frame image path.
- `crates/native-management`: protected storage, enrollment, policy/revocation,
  controller-role restrictions and unbind. All sessions use the same owner.
- `src/gpui_session.rs` and the native client: authorization, session proofs,
  existing transport/login and software decode under `gpui-mobile-session`.
- `packaging/gpui-ios` / `packaging/gpui-android` and their native workflows:
  application identities, SDK/ABI restrictions and package validation.

Mobile products are foreground controllers. They do not enable desktop host,
capture or desktop IPC. GPUI calls the Rust core directly; there is no Dart UI
or Flutter bridge in this product path. Retained legacy frontend sources are
not another supported mobile entry.

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
presentation helper. The management-only variant omits media when this feature is off. Clipboard remains disabled in protocol options and locally. Foreground
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
resource/queue budgets, not measured end-to-end latency. Desktop playback keeps its own native path and buffer policy.

Physical-device acceptance must cover audible remote mono/stereo playback, system
volume, mute/disconnect without old audio replay, headset/Bluetooth route changes,
focus/phone interruption, background/foreground and reconnect. Compilation and
packaging do not establish those results.

### Internal direct-connection compatibility

Product mobile workflows publish the managed GPUI application and the separate
no-network Android smoke test. They do not publish temporary IP-direct packages,
and the normal UI has no temporary IP entry. Retained `test-lan-direct` module
and packaging checks serve internal regression work only. An internally built
test artifact is not a managed product or a release candidate; its authentication
and host setup limits are documented in [the internal test notes](test-lan-direct.md).
