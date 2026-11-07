XSCC IddCx driver — source-only unsigned prototype

This independent driver is being maintained to replace the unsafe adapter-wide
reconfiguration used by the current virtual-display integration. It has its own
Root\\XsccIdd identity, control interface, UMDF service and isolated host process.
It does not take over Root\\MttVDD or devices owned by another remote application.

Current product binaries, menus and their existing driver payloads are unchanged.
This project is not loaded, installed, or automatically selected by XSCC.
No functionality has been rolled back. The abandoned recovery worktree is separate.

Control contract
- Start with zero monitors; Add is the only automatic arrival path.
- Query/List/Add/Remove are versioned, size-checked buffered IOCTLs.
- Only System and elevated administrators may open the control device.
- Each Add belongs to the control file handle that created it.
- Closing/crashing that owner removes its monitors. A racing Add notices the
  closing lease and undoes its own arrival; failed cleanup terminates this UMDF
  process rather than reporting successful removal.
- Hosts must retain a separate long-lived handle for persistent displays, and
  use session handles for temporary displays. There is no XML auto-provisioning.
- Normal removal stops that screen's worker, then calls IddCxMonitorDeparture.
  It does not disable/restart the adapter or disturb another owner's monitors.
- Add with kNoConnector allocates an available connector atomically; Remove is
  explicit per connector and cannot remove another handle's screen.

Host transport — not product backend wiring
- control/ is an independent, dependency-free Windows x86_64 Rust library.
- Owner::open enumerates only the fixed control GUID and requires the interface's
  actual devnode to have both a ROOT\XSCCIDD instance ID and Root\XsccIdd hardware ID.
  It rejects ambiguity; it never discovers or controls MTT through guessed IOCTLs.
- Owner keeps the same control file open for Query/List/Add/Remove. Separate opens
  use read/write sharing but create independent file leases, not shared ownership.
- Query/List cover completed arrivals across all owners. List is not a list of
  this handle's screens; Remove relies on the driver's same-file lease check.
- Add accepts the selected mode table and fractional refresh without a 60 Hz cap.
  The driver remains authoritative for timing arithmetic and actual acceptance.
- No Drop/CloseHandle or synchronous Windows/driver call has a promised wall-clock
  completion bound. Normal shutdown should explicitly Remove and verify inventory;
  a failed mutating call must be reconciled, never automatically retried as a new Add.
- CPU response-boundary tests and Windows MSVC compilation do not load a device.
  This library is not called by the current product and does not install a driver,
  map connector IDs to CCD paths, restore physical layouts, or switch the backend.

Mode and lifecycle contract
- Mode tables are per monitor, validated before arrival, immutable while connected.
  A new table uses an explicit remove/add of that connector.
- Fractional refresh is represented in 1/10000 Hz; monitor/target dividers differ
  as required by IddCx. Pixel clocks use checked 64-bit arithmetic.
- The prototype declares 32 connector slots and 4096 modes per screen. These are
  implementation/message capacities, not measured GPU or Windows limits.
- Inventory is driver arrival/departure state. It is not CCD active-mode proof.
- D3D creation/processing belongs to a dedicated worker, not Assign.
- Cancellation is checked even while frames continuously arrive; stop has priority.
- Exactly one worker owns and deletes its accepted swapchain.
- An explicit stop wait is limited to 1500 ms. Timeout is an unrecoverable error,
  not successful removal. IddCx critical-error reporting/fail-fast may terminate
  the UMDF process and disconnect its virtual screens.
- The wait budget does not prove a wall-clock bound on Windows/GPU calls.
- A new adapter starts empty after PnP stop/start. The host must reapply its
  persistent policy; the driver does not silently recreate screens.

Validation and delivery limits
The pure protocol/timing witnesses run without WDF, D3D or display devices.
CI builds with SignMode=Off and checks API use, INF and generated catalog integrity.
Unsigned artifacts are not usable production installers. Do not reuse the original
upstream catalog: any changed INF/DLL requires a new valid driver signature.

Real IddCx reentry, high-rate content, repeated hotplug, owner-process crash,
PnP restart, suspend/resume, and AMD/Intel/NVIDIA driver coexistence still require
a signed package and dedicated hardware acceptance. Compilation is not evidence
that the user's system hang, audible beeping, or visible 60fps limit is solved.
No local virtual-screen tests are authorized by this source-only project.

Upstream reference and license hashes are in UPSTREAM.json; LICENSE preserves
the upstream MIT notice. The build output includes both files and this document.
