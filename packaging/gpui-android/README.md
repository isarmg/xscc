# GPUI Android candidates

The ARM64/API 29+ smoke flavor exercises the experimental GPUI Android window,
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

GameActivity 4.4.0 ships Java debug logs that include input text. The fixed
upstream InputConnection source under `vendor/game-text-input` removes only its
30 debug-log statements and their argument expressions; its provenance records
the upstream revision and SHA256. A SHA-pinned AAR filter removes exactly the
three replaced class files and preserves all other AAR entries. Only the
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


## Management preview flavor

The management flavor uses `org.sarmg.xscc.gpui.preview` and displays
“XSCC GPUI 管理预览”. It builds the Android library from
`experiments/gpui-mobile-management` with `mobile-management,mobile-session`, reusing the
native management UI. This preview includes the existing software-video receive/decode path and
pointer/touch controls, natural software-keyboard commits and a control-key bar,
but no desktop hosting. Unmodified, non-repeating Enter/Tab events also support
IME actions and identical hardware control events; full hardware-keyboard support
is unfinished. macOS/Windows targets accept native Unicode; Linux targets accept
printable ASCII and separate control keys without clipboard fallback. Foreground
remote audio playback is initially off and explicitly enabled per session with
the sound button; microphone and clipboard remain disabled. Audio playback,
interruptions, route changes and system-volume behavior need physical-device testing. Server authorization, first decoded
frame, display, pointer and IME behavior still require physical-device acceptance
against the intended server; successful packaging alone does not prove a session.

Management and TEST IP direct build the pinned NDK ARM64 codec dependencies (the same native
dependency script/cache used by the Flutter candidate, without Flutter codegen).
The dependency gate rejects Flutter, host/capture and hardware-codec features.
Smoke continues to build without these codec dependencies or Internet access.

All flavors share the IME bridge, source privacy patch and all final DEX,
JNI, signature and 16 KB gates. Smoke has no Internet permission; management and TEST IP direct
add only Internet. Each may use its exact application-ID-scoped AndroidX
signature permission for non-exported receivers. Backup remains disabled.
There are no recording, storage or accessibility permissions, and no Flutter
credential migration. Each package uses its own Android private application
storage. These APKs remain disposable-debug-key previews.

## TEST-only LAN direct APK

The separate `landirect` flavor is named **XSCC TEST IP** and uses application
ID `org.sarmg.xscc.gpui.testdirect`. CI enables `test-lan-direct` only for this
flavor and publishes a `TEST-IP-direct` APK with `TEST-ONLY.txt`. The ordinary
`management` flavor still excludes this feature. Dependency and binary-marker
gates reject mismatched package modes before publishing an APK.

This test mode has **no identity authentication or per-session host confirmation**.
There is no private-address or subnet restriction: any reachable IPv4/IPv6 address
is allowed, and anyone reaching the test listener can control the desktop. Use a
matching desktop TEST host from the same source revision, explicitly
started with `--test-lan-host` and `XSCC_TEST_LAN_BIND` (normally port 21119).
See [test host instructions](../../docs/test-lan-direct.md). It neither enables
normal host access nor changes the normal managed connection. INTERNET remains
the only Android network permission; no microphone or clipboard capability is
added. Actual-device connections and input still require device testing.

Remove this flavor, feature and test entry points before production release;
renaming a TEST artifact does not make it a production package.
