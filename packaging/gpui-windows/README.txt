XSCC Controller Windows beta candidate (unsigned)

Target: Windows 11 x86_64. This is the separate GPUI controlling-device package,
built with the native live-session integration. Use XSCC Controller from the
Start menu to open the app. Diagnostic test patterns are not remote video.
See the release's exact runtime validation and limitations before use; a CI
build or --help test alone does not establish successful remote streaming.

Installation is per-user in %LOCALAPPDATA%\Programs\XSCC-Controller and adds
a Start menu entry. It does not register a Windows service, install drivers,
or change firewall/security settings. Close the app before upgrading or
uninstalling. Application identity/configuration are preserved on uninstall.
The separate XSCC Host package runs on the controlled device and has different
administrator/service requirements. Do not install the host merely to use
the controller.

The executable and installer are unsigned. Windows may warn about an unknown
publisher. Do not disable antivirus, SmartScreen or other security controls.
SHA256SUMS.txt verifies download integrity only; it is not a code signature.

See build-info.json for the exact source revision, dependency lock hash and
build run. Corresponding source is available at the recorded source URL;
LICENCE contains the AGPL-3.0 license. Do not use production credentials or
sensitive sessions to evaluate an unvalidated beta candidate. Audio, clipboard,
file transfer, elevated-desktop interaction and hardware codec support must
not be assumed unless the release explicitly reports their validation.

The build requests static MSVC CRT linkage. dependencies.txt records direct DLL
imports; it does not replace clean-machine graphics/runtime acceptance. No
Visual Studio installation or unverified DLL downloads should be assumed.
