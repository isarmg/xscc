# Windows display control

This library sends monitor control requests to existing Windows indirect display
packages. It uses the Windows Software Device API for the indexed driver with
hardware ID `RustDeskIddDriver` and Configuration Manager to open each driver's
device interface. `protocol.rs` preserves the indexed driver's on-wire layout.

The indexed driver install command looks for
`RustDeskIddDriver/RustDeskIddDriver.inf` next to the application executable.
The application can link this library without distributing a separate controller
DLL. The Windows driver itself continues to use its own installation package.
