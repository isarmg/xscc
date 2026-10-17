# XSCC Flutter mobile test candidates

These candidates reuse the existing Flutter mobile client and native Rust session engine. They are not GPUI mobile applications or production releases. Mobile devices are controllers only. Bind a controller instance with the management server's authorization code; allowed hosts and revocation remain server-managed. Desktop-only test-LAN mode is not included.

## Android

Install the `.apk` on an ARM64 Android device running Android 10 / API 29 or newer. The application ID is `org.sarmg.xscc.beta`, with display name `XSCC测试版`, so it does not replace RustDesk.

The APK is release-optimized but signed with an ephemeral CI debug key. It is an installable test package, not an official release-signing identity. A later build may have a different key; uninstalling the old test build may be necessary and removes its local settings and enrollment. Enroll again with the server if required. Do not use it for a production deployment.

`SHA256SUMS` checks artifact integrity; `BUILD-INFO.txt` identifies the exact source commit. CI checks the APK signature, ARM64 native library, application ID and minimum SDK. No physical-device login, streaming, input or revocation test is implied by a successful build.

## iOS / iPadOS

The iOS candidate requires iOS/iPadOS 26 or newer on ARM64. It is unsigned and must be signed by you with a matching Apple identity and provisioning profile before device installation. It is not immediately installable, an App Store release or a TestFlight upload. The IPA contains `Payload/Runner.app` from an ARM64 device archive. Re-sign the app and embedded frameworks using your own team and a provisioning profile matching `org.sarmg.xscc.beta`, or use your signing tool to change the bundle ID and apply a matching profile. No certificate, provisioning profile or Apple login is included or requested by this build. The workflow does not upload to Apple services.

## Device acceptance

After installation/signing, verify enrollment, authorized host listing, video and audio, touch/keyboard input, reconnect, and server-side revocation. No unauthorized host should become available. These checks remain pending until performed on physical devices.
