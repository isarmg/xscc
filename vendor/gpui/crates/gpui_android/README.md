# Android GPUI rendering slice

Experimental single-Activity ARM64 backend, minimum Android API 29. This is a
native GPUI/WGPU rendering test, not a usable remote-control or login client.

Implemented: native window and surface lifetime, touch/tap/drag/scroll through
GPUI's portable recognizers, physical navigation keys, foreground task wakeup,
background executor, activity lifecycle, back-to-finish, content viewport and
system Latin/Chinese/emoji font loading through API 29's font matcher.

Not implemented: IME/text editing, clipboard, credential storage, mouse input,
accessibility bridge, system dialogs, multiwindow, remote video/audio/control.
Unsupported fallible operations return errors. Do not use this backend for
password entry until secure IME support is implemented and tested.

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
