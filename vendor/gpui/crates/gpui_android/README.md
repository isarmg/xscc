# Android GPUI rendering and IME slice

Experimental single-Activity ARM64 backend, minimum Android API 29. This is a
native GPUI/WGPU rendering test, not a usable remote-control or login client.

Implemented: native window and surface lifetime, touch/tap/drag/scroll through
GPUI's portable recognizers, physical navigation keys, foreground task wakeup,
background executor, activity lifecycle, Back/keyboard dismissal, content viewport and
system Latin/Chinese/emoji font loading through API 29's font matcher.

Experimental text input: GameTextInput snapshots, UTF-16 selections/composition,
commit/delete, editor actions, protected password EditorInfo, and keyboard insets.
The offline smoke app includes ordinary and password fields; it never connects
to a management server. Real-device keyboard behavior remains unverified.

Not implemented: clipboard, credential storage, mouse input,
accessibility bridge, system dialogs, multiwindow, remote video/audio/control.
Unsupported fallible operations return errors. Do not use this backend for
real password entry until the protected keyboard and final-DEX no-text-logging
gates are verified and device acceptance passes.

Only draw a surface between InitWindow and TerminateWindow. The latter releases
the WGPU surface before returning to Android. Device loss uses WGPU recovery.
Frame requests are coalesced and bounded at a 60 Hz fallback cadence; background,
idle, and detached windows block waiting for events. Display-native Choreographer
pacing is a later improvement, not claimed by this slice.

The smoke app lives in `experiments/gpui-android-smoke`; Android packaging and the
pinned build instructions live in `packaging/gpui-android`. The Android facade is
`gpui_platform::android(AndroidApp)`, explicitly passed the current Activity's
context on its `android_main` thread. Context-free desktop constructors are not
available on Android. No process-global Activity or unsafe Send/Sync GUI object
is used.

## Source and license provenance

- GPUI and gpui_wgpu: existing vendored sources, Apache-2.0; existing provenance
  and notices remain in the parent vendored workspace.
- android-activity 0.6.1: https://github.com/rust-mobile/android-activity,
  MIT OR Apache-2.0. Rust glue maintained by rust-mobile, not a Google Rust SDK.
  `game-activity` only. Its crate includes its matching native GameActivity
  sources; do not add a second native copy via Gradle Prefab.
- ndk 0.9.0 / ndk-sys 0.6.0: https://github.com/rust-mobile/ndk,
  MIT OR Apache-2.0. NativeWindow owns reference-counted ANativeWindow handles.
- jni 0.22.4: https://github.com/jni-rs/jni-rs, MIT OR Apache-2.0.
- androidx.games:games-activity:4.4.0: Google AndroidX / Android Games SDK,
  Apache-2.0. The Java package version follows android-activity 0.6.1's README.
- WGPU stays at the existing pinned gfx-rs revision in the parent workspace.

Dependency archive checksums are committed in Cargo.lock. No third-party source
was copied into this crate. Keep dependency licenses/notices when distributing.

## Verification

Cross-build the real ARM64 library and inspect the APK/ELF using the dedicated
CI workflow. A successful Cargo check does not establish APK link success or
on-device operation. Device QA must cover API29 and modern target36 devices,
first frame, English/Chinese text, tap counter, list scrolling, delayed async
wake, Back, Home/resume, lock/unlock, rotation and surface replacement. Confirm
no blank/stale atlas rendering or GPU work while backgrounded.

Input conversion tests are beside input.rs and require an Android test harness;
they are not represented as host-tested. The dependency-free production density
helper can be exercised on the host with:

    rustc --edition 2024 --test src/geometry.rs -o /tmp/gpui-android-geometry-tests
    /tmp/gpui-android-geometry-tests

## IME contract and acceptance

GameTextInput's Java `InputConnection` uses `Editable` and `Selection` offsets.
Its Java State integers pass unchanged through the bundled C++ JNI State adapter
and android-activity 0.6.1's TextInputState; those offsets are UTF-16, not UTF-8.
Only slicing Rust text converts to byte boundaries. Invalid surrogate-interior
caret offsets round down; composition boundaries expand to whole scalar values.
Native snapshots are compared with the focused GPUI handler, and rejected or
truncated edits are read back before composing ranges are applied. A generation-bound Java InputConnection guard rejects old-session reads and
writes; tagged JNI events are checked again on the GPUI thread. Untagged
android-activity TextEvent/TextAction snapshots are deliberately ignored. Handler calls occur outside paint/update.

Secure fields use TYPE_TEXT_VARIATION_PASSWORD, NO_SUGGESTIONS and
IME_FLAG_NO_PERSONALIZED_LEARNING. The latter is an IME request, not a guarantee
about third-party keyboards. EditorInfo changes restart the keyboard connection;
backgrounding and window destruction invalidate the session and clear Java
editable text. Embedding Activities must extend the supplied AndroidImeBridge. No Rust text/password logs exist.
Upstream Java GameTextInput contains text logging: packaging must remove these
calls and verify the final DEX before any password-enabled APK is distributed.

Source: android-activity 0.6.1's bundled GameActivity/GameTextInput JNI, and
https://github.com/rust-mobile/android-games-sdk/tree/android-activity-4.0.0/game-text-input/src/main/java/com/google/androidgamesdk/gametextinput
(Android SDK sources, Apache-2.0). Official API guidance:
https://developer.android.com/games/agdk/add-support-for-text-input
https://developer.android.com/reference/android/text/InputType
https://developer.android.com/reference/android/view/inputmethod/EditorInfo

Host-test the production Unicode helper:

    rustc --edition 2024 --test src/text_state.rs -o /tmp/gpui-android-text-tests
    /tmp/gpui-android-text-tests

Device acceptance must exercise Latin/Chinese composition, commit/cancel, emoji
and surrogate-boundary caret movement, selected replacement, delete, field
switches during composition, full-field rejection, secure/nonsecure switches,
editor actions, keyboard open/close/Back, Home/resume and rotation while editing.
Confirm keyboard insets move visible content without changing full layout size;
inspect logcat for absence of test secrets. Floating keyboards and hardware
keyboard text/selection behavior also require device verification.

The Activity retains GameTextInput’s edge-to-edge configuration and GameActivity’s
soft-input window policy. Insets are converted from physical to logical pixels
once. Surface resizing and keyboard occlusion on API29 and current Android
versions remain explicit device acceptance checks.
