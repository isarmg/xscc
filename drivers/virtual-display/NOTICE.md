# Virtual display driver

This repository maintains the Windows indirect display driver source. Rust
owns the XML profile, mode timing, monitor state and swapchain worker. The WDK
bridge provides Windows driver callbacks and calls the inline UMDF, IddCx and
Direct3D interfaces. The virtual driver consumes surfaces and reports dropped
frames to IddCx. The host captures and sends video separately.

The bundled EDID and INF originate from VirtualDrivers/Virtual-Display-Driver
25.7.23, commit `d437ebc9b44a14ce6e5cc9c8b7f6beb08d6faf77`. The WDK bridge
uses patterns from Microsoft's indirect display sample. Both components are
covered by the accompanying MIT license and notices.

The driver reads the host's `vdd_settings.xml` from the existing `VDDPATH`
registry value. A legacy `option.txt` is accepted when no XML file is present.
The driver uses the packaged profile when neither file is present. A malformed
profile fails device initialization with a diagnostic in Windows debug output.
Custom EDID is supported with checksum validation. Legacy driver logging, named
pipe controls, EDID spoofing and cursor configuration are omitted. Windows
continues to composite the cursor.

The manual Rust virtual display driver workflow produces an unsigned Windows
x64 package and runs Windows API and INF validation using the pinned SDK and
WDK. It preserves the existing INF hardware identity and intended filenames.
The release driver manifest must correspond to a newly signed package before
this implementation can be distributed through the installer.

Run the profile and timing tests with
`cargo test --locked --offline --manifest-path drivers/virtual-display/Cargo.toml`.
