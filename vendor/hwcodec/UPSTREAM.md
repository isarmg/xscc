# hwcodec provenance

- Upstream repository: https://github.com/rustdesk-org/hwcodec
- Imported revision: `778df1f99597722473b29443bac22ae6c23946fe`.
- Included `externals` submodule: https://github.com/rustdesk-org/externals,
  revision `8903740a1f47884906a6e347ad3d8d56304d9771`.
- Imported from the exact source selected by the root and desktop application
  lockfiles. The mobile software-only workspace does not depend on hwcodec.

The snapshot has no standalone top-level license file or Cargo license declaration.
All upstream source, copyright notices and SDK notices are retained; this record
does not introduce or change their licenses. The complete source snapshot,
including dev/example sources and SDK headers, is retained. Git administration,
Cargo checkout markers and build targets are omitted.

Local changes add an explicit bitrate-control setter to the VRAM wrapper and
Direct-MFX backend. It represents high kbps targets with BRCParamMultiplier,
rescales the associated buffer/delay units, queries native acceptance, and keeps
the prior BRC fields for restoration when leaving the explicit-limit mode.
The existing automatic setter and codec profile/level selection are unchanged.

BRCParamMultiplier semantics:
https://cdrdv2-public.intel.com/709602/mediasdk-man.pdf

SOURCE_FILES.sha256 lists every imported upstream file before local patches.
The local .gitattributes keeps these source/SDK bytes unchanged across checkouts.
Original SDK/sample redistributables, including SDL2 import/runtime binaries,
remain part of this complete source snapshot; application packaging does not
include these unused example binaries.

Upstream whitespace is retained without formatting the SDK snapshot. Local
patches are checked separately against the exact imported source revision.

The FFmpeg RAM decoder reuses its previous packet's owned pixel-plane buffers.
The synchronous callback still copies FFmpeg-owned planes before returning; no
GPU resources or borrowed codec frame pointers escape to the application. Each
packet clears the visible output before decode, including no-output/error paths.

The common, FFmpeg RAM and mux binding generators allowlist their actual C
interface types, functions and constants. Recursive dependency types remain
enabled, as do bindgen layout tests. This avoids importing unrelated Darwin
libc records whose old bindgen 0.59 field-offset tests dereference null pointers;
the native decoded-plane regression tests remain enabled. No C ABI or codec
link dependencies change.
