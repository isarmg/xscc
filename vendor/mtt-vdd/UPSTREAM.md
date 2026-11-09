# MTT VDD maintenance candidate

The `upstream` snapshot is imported byte-for-byte from the official
VirtualDrivers/Virtual-Display-Driver tag `25.7.23`, commit
`d437ebc9b44a14ce6e5cc9c8b7f6beb08d6faf77`. `upstream.json` records every Git blob,
SHA256, size and exact import scope. `SOURCE_FILES.sha256` covers the original
snapshot; all original source, MIT license and Microsoft sample notices remain.

`source` is an independently maintained candidate. Its only driver behavior
change is cancellation priority: test stop before every acquisition and prefer
stop over simultaneously ready frame events. No format, refresh, monitor count,
HDR, EDID, rendering or control behavior is intentionally changed. The original
worker join remains: a timeout never frees a live worker or permits a use-after-free.
This addresses event-starvation; it does not bound a stuck Windows/GPU/DDI call.

The patch is recorded under `patches`; the small stop-decision header is local
MIT code used by both the driver and deterministic device-free CPU tests.
Builds must verify this complete inventory and the declared patch before compiling.

No signed DLL or catalog is imported or reused. The product's current signed
driver and packaging manifest remain pinned to their original payload. Any new
DLL/INF requires independently valid signing and native safety acceptance before
installation; this source candidate provides neither. No installation, test
signing, Secure Boot change or virtual monitor is part of this source import.

The manual-only `mtt-vdd-maintenance.yml` workflow and build script reuse the
independent IDD prototype's fixed NuGet 6.11.1 / SDK+WDK 10.0.26100.6584 basis.
The separate project compiles the maintained source, with SignMode=Off and API/INF
validation required. Upstream's original project remains preserved as evidence;
its hard-coded Downloads header path, signing settings and post-build commands
are never executed. Legacy source warnings are retained as warnings; local CPU
regressions remain /W4 /WX. No runtime or installation job exists. Merely preparing
this workflow is not evidence that the driver has compiled or passed validation.
