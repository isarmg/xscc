XSCC GPUI macOS native candidate

Target: macOS 26 on Apple Silicon. Controller and Host are separate apps.
This candidate is not signed or notarized for distribution. Packaging does not
install the app, register a service, or grant macOS privacy permissions.

Host is a foreground, logged-in desktop application. Keep it running during
remote sessions. Unattended login-window hosting and launchd installation are
not included. Controller does not need screen-recording/accessibility grants
merely to view and control a remote computer.

Before hosting, review the Host app in System Settings > Privacy & Security:
- Screen & System Audio Recording allows screen/system-audio capture.
- Accessibility allows authorized remote keyboard/mouse control.
Input Monitoring may also be needed for keyboard-layout handling. Microphone
permission is needed only when selecting a microphone as the audio source.
Only grant permissions you intend to use. Restart the Host after changing
permissions if macOS requests it. No permission is granted automatically.

This bundle uses the native GPUI Metal renderer and Rust session core, without
Flutter. Video presentation currently uploads CPU-decoded frames. Native
VideoToolbox-to-Metal zero-copy presentation is not implemented by this package.
See docs/gpui-macos.md in the matching source checkout for build and acceptance.
