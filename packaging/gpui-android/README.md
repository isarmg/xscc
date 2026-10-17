# GPUI Android smoke candidate

This separate ARM64/API 29+ APK exercises the experimental GPUI Android window,
touch input and activity/surface lifecycle. It is not a remote-control client:
no management login, remote stream, host service or network permission is added.
It does not replace the Flutter beta (package: `org.xscc.gpui.smoke`).

CI builds the independent `experiments/gpui-android-smoke` Rust workspace and
packages its actual `libxscc_gpui_android_smoke.so` with GameActivity 4.4.0.
Rust android-activity 0.6.1 supplies the native GameActivity glue; Gradle supplies
the matching Java side, without a second Prefab/CMake native implementation.

The release-optimized APK uses the runner's disposable Android debug key.
This is a test signature, not a production identity; later runs may require
uninstalling the prior smoke package. No signing secret is required or retained.

CI checks the signature, package/min/target SDK/ABI, required library, ELF
AArch64 LOAD segment alignment and uncompressed native ZIP 16 KB alignment.
Passing packaging checks does not prove rendering or touch/lifecycle behavior.

## Physical-device acceptance (still required)

Install on an ARM64 Android device (API 29+), then:
1. Launch and confirm the GPUI smoke view is visibly rendered.
2. Tap the interactive control and confirm its visible state changes.
3. Rotate; background and resume; lock/unlock. Confirm rendering and input recover.
4. Repeat surface recreation and check logcat for native crashes or stale surfaces.
5. Repeat on a 16 KB-page device; record `adb shell getconf PAGE_SIZE`.

Record device/OS, commit, observed results and logcat. Do not label a build as
runtime-tested until these checks have actually been performed.

References: [GameActivity integration](https://developer.android.com/games/agdk/game-activity/get-started),
[16 KB page sizes](https://developer.android.com/guide/practices/page-sizes).


## IME logging privacy gate

GameActivity 4.4.0 ships Java debug logs that include input text. Only the
R8-optimized release variant is enabled here; debug APK generation is disabled.
The local rules remove debug/verbose log calls while retaining info/warn/error
diagnostics. The AndroidX annotation keep rules are restated unchanged for other
classes; only GameSDK JNI classes permit body optimization, retaining all names
and members.

Before upload, CI inspects every final DEX, rejects debug/verbose Log references
and surviving sensitive log argument formats, and compares GameSDK class/member
definitions with the unmodified Maven AAR. A missing JNI declaration or residual
text-log format fails packaging. This gate must pass on the actual APK; rule text
alone is not evidence. Physical-device IME behavior and logcat still need testing.
