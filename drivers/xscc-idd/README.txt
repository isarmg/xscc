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
- Query/List/OwnedList/Add/Remove are versioned, size-checked buffered IOCTLs.
- Only System and elevated administrators may open the control device.
- Each Add belongs to the control file handle that created it.
- Closing/crashing that owner removes its monitors. A racing Add notices the
  closing lease and undoes its own arrival; failed cleanup terminates this UMDF
  process rather than reporting successful removal.
- This prototype implements temporary control-file leases only. Device-level
  persistent policy across host exit or driver restart is not implemented, so
  the product persistent=true contract is not supported or wired to this driver.
  There is no XML auto-provisioning.
- Normal removal stops that screen's worker, then calls IddCxMonitorDeparture.
  It does not disable/restart the adapter or disturb another owner's monitors.
- Add with kNoConnector allocates an available connector atomically; Remove is
  explicit per connector and cannot remove another handle's screen.

Host transport — not product backend wiring
- control/ is an independent, dependency-free Windows x86_64 Rust library.
- Owner::open enumerates only the fixed control GUID and requires the interface's
  actual devnode to have both a ROOT\XSCCIDD instance ID and Root\XsccIdd hardware ID.
  It rejects ambiguity; it never discovers or controls MTT through guessed IOCTLs.
- Owner keeps the same control file open for Query/List/OwnedList/Add/Remove. Separate opens
  use read/write sharing but create independent file leases, not shared ownership.
- Query/List cover completed arrivals across all owners. OwnedList/Owner::owned_list()
  returns only this handle's screens; Remove relies on the driver's same-file lease check.
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
- The prototype exposes 16 connector slots per adapter, matching the IddCx
  MaxMonitorsSupported limit observed in disposable Windows CI. This is not a
  measured GPU monitor limit. Each screen accepts up to 4096 modes.
- Inventory is driver arrival/departure state. It is not CCD active-mode proof.
- D3D creation/processing belongs to a dedicated worker, not Assign.
- Cancellation is checked even while frames continuously arrive; stop has priority.
- One owner deletes each accepted swapchain. Explicit Remove and worker errors
  retain the worker-owned path. Assign/Unassign may claim callback ownership before
  requesting stop: after the last frame DDI/surface access, that callback deletes
  the chain without waiting for the worker's D3D/DXGI release tail.
- Frame quiescence, DeleteReturned and native thread exit are separate conditions.
  DeleteReturned means the sole WdfObjectDelete call returned; WDF object destruction
  and native GPU cleanup may still be pending. The registry retains the session
  until DeleteReturned and the actual native thread handle are both signaled.
- A callback that loses ownership to a worker retains the bounded wait/fail-stop
  path. A deletion claim or a retained WDF reference does not permit early return
  or a second public Delete. Device/adapter cleanup still waits for real workers
  outside registry locks before permitting driver code to retire.
- The local consume/discard sink reports actual presentation numbers and acquisition
  QPC as DROPPED and checks IddCx frame-statistics results. It does not claim remote
  encoding/transmission or invent those timing fields.
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

The opt-in disposable Windows VM runtime job can use GitHub's pre-existing test
signing boot configuration. It records enabled options without changing boot or
code-integrity policy. In addition to lease/PnP checks, active phases verify real
CCD 1280x720@120/1 and continuous successful frame/statistics DDIs before explicit
Remove or OS deactivation. Exact-root UMDF PID/current-chain WPP evidence and saved
layout restoration are required. Guest-only signatures, CCD mode and software-VM
frame calls do not establish production-policy loading, visible 120 FPS or
dedicated-GPU safety. See the run matching the exact source commit for results.

Real IddCx reentry, high-rate content, repeated hotplug, owner-process crash,
PnP restart, suspend/resume, and AMD/Intel/NVIDIA driver coexistence still require
a signed package and dedicated hardware acceptance. Compilation is not evidence
that the user's system hang, audible beeping, or visible 60fps limit is solved.
CI-only runtime experiments do not authorize or run virtual-screen tests on the
user's computer.

Upstream reference and license hashes are in UPSTREAM.json; LICENSE preserves
the upstream MIT notice. The build output includes both files and this document.
