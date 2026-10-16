XSCC Host Windows beta candidate (unsigned)

Target: Windows 11 x86_64. This is the separate controlled-device host package.
The installer requires administrator approval and registers the XSCC Host
Windows service running as LocalSystem. Host UI also requires administrator
approval. The service owns host identity/configuration; controller installs
use a separate per-user application and do not register this service.

Install location: %ProgramFiles%\XSCC-Host. Start XSCC Host from the Start menu.
No drivers, firewall rules or security-policy changes are installed. Do not
expose an unauthenticated host or disable security controls to test the beta.
Close the host UI before upgrading or uninstalling. Uninstall stops/removes
only the dedicated service and installed package files, preserving identity
and configuration. Do not manually delete service state during an upgrade.

This is an unsigned beta candidate. Windows may warn about an unknown
publisher. SHA256SUMS.txt verifies integrity, not publisher identity. See
build-info.json for the exact source and CI run. A successful CI package build
is not evidence of Windows 11 graphics, real peer streaming or service runtime
acceptance; review the release's explicit validation and limitations first.

The package includes AGPL-3.0 LICENCE; corresponding source is linked in
build-info.json. Do not evaluate unvalidated builds with sensitive sessions.

The build requests static MSVC CRT linkage. dependencies.txt records direct DLL
imports; it does not replace clean-machine graphics/runtime acceptance. No
Visual Studio installation or unverified DLL downloads should be assumed.
