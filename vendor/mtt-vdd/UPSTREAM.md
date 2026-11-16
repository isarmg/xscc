# MTT VDD maintenance candidate

The `upstream` snapshot is imported byte-for-byte from the official
VirtualDrivers/Virtual-Display-Driver tag `25.7.23`, commit
`d437ebc9b44a14ce6e5cc9c8b7f6beb08d6faf77`. `upstream.json` records every Git blob,
SHA256, size and exact import scope. `SOURCE_FILES.sha256` covers the original
snapshot; all original source, MIT license and Microsoft sample notices remain.

`source` is independently maintained. Stop is checked before every acquisition
and preferred over simultaneously ready frame events. The original worker join
remains: a timeout never frees a live worker or permits a use-after-free. This
addresses event starvation; it does not bound a stuck Windows/GPU/DDI call.

Monitor timing now uses horizontal and vertical totals for the pixel clock,
with exact declared refresh rationals. Target pixel-clock multiplication also
uses 64 bits. Both monitor-description callbacks generate their output from the
current configured mode list instead of appending to a process-global cache
whose first entries could outlive the configuration. The existing driver source
patch records these timing changes together with cancellation priority; no
upstream file is rewritten. This is a candidate correction, not evidence that
secondary-output application presentation has exceeded 60 FPS on real hardware.

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

The maintained INF additionally follows the official Windows 11 reflector
Include/Needs pattern from the pinned IddSample INF recorded in upstream.json.
The UMDF DLL is run from DIRID 13 as required by current Driver Package Isolation
guidance, rather than copying to drivers/UMDF. The explicit old WUDFRd.sys service
declaration is replaced by the system WUDFRD.inf sections. The hardware identities,
UMDF service name, device group, IndirectKmd filter and IddCx extension remain.
The INF now explicitly targets Windows 11 build 22000 and later, matching the
product support boundary. Original upstream INF bytes remain unchanged; patch
0002 records the complete packaging-only maintenance change. No driver code,
display modes, monitor count or signing-validation requirement is changed.

### 2026-11-16 runtime trace corrections

A bounded local IddCx trace (24,241 events, zero lost or undecoded) reported a rejected render preference for an indirect adapter with the same name and memory size as the AMD render GPU, invalid cursor dimensions (-1,-1), and missing frame statistics beyond 100 frames. Adapter enumeration now queries the actual WDDM adapter type and excludes display-only indirect and software adapters before matching names. Creating a D3D11 device alone is insufficient on this host: both the render and display-only alias return success. The query uses the existing OneCoreUAP import library without a new dependency. Missing/out-of-range hardware cursor dimensions preserve the existing Windows composited cursor path; they no longer issue invalid hardware-cursor requests or claim unsupported hardware-cursor functionality. Each successfully consumed surface reports its actual metadata frame number and acquisition QPC to IddCx; this virtual sink invents no encoder or transmission timings. Stop priority, sole worker ownership, and the join sequence are preserved. These observed API violations are independently actionable; fixing them is not yet evidence that the secondary virtual output's Present(1) 60Hz limit is solved. No primary-display promotion is introduced.
