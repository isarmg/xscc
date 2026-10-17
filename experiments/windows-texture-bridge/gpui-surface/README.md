# GPUI Windows shared-surface integration proof

Status: an **experimental surface implementation maintained in `vendor/gpui`**.
XSCC builds that local source directly; the production decoder/frame path remains
CPU-backed and does not publish these experimental surfaces. The isolated CI
builds the example and runs one private WARP pixel test. No successful runtime
execution of that test has yet been established. The source originated at Zed
commit `28d0a3547f16b9609b301b48ab363bfca649c864`; provenance and licenses are
retained in `vendor/gpui`. Previous Windows GNU cross-checks passed before
vendoring. No native Windows/MSVC link, display, or hardware validation is
claimed. Do not advertise this implementation as a performance improvement.

## Exact upstream gap and route

At the original upstream revision:

- `crates/gpui/src/elements/surface.rs`: `SurfaceSource` only carries Apple's
  `CVPixelBuffer`; `surface()` is Apple-only.
- `crates/gpui/src/window.rs`: `paint_surface()` is Apple-only.
- `crates/gpui/src/scene.rs`: `PaintSurface` lacks a Windows texture payload.
- `crates/gpui_windows/src/directx_renderer.rs`: `draw_surfaces()` returns success
  without drawing anything.

The existing scene already batches surfaces in paint order with content masks.
The maintained implementation extends those points with an owned immutable BGRA texture, imports it
onto the renderer's own device, and draws it through a separate instance of the
existing polychrome sprite shader pipeline. The separate instance avoids
clobbering ordinary atlas-sprite buffers. Only opaque video pixels are supported;
premultiplied transparency would require a different blending/coverage path. A clamp sampler avoids wrap sampling at
video edges. No child HWND, separate swapchain, or out-of-band composition layer
is added; GPUI remains responsible for presentation, DPI, clipping, and z-order.

The example is the actual integration entry point: synthetic CPU pattern →
producer DEFAULT texture → **one GPU isolation copy** into an owned shared
texture → GPUI `surface()` → scene → DirectX draw → GPUI swapchain. CPU generation
is instrumentation. There is no consumer CPU readback/upload or atlas insertion
in the added surface path. This is not a decoder or zero-copy demonstration.

## Ownership and synchronization

`WindowsSurface::from_published_texture` retains a typed `ID3D11Texture2D` COM
reference. Its unsafe contract requires a uniquely owned, immutable publication:
producer obtains key 0, submits the copy, flushes, and releases key 1 before
publication. Nobody may mutate the resource or acquire its mutex while a surface
clone remains. The constructor validates a legacy keyed-mutex shared, single
mip/slice/sample BGRA8 DEFAULT shader-resource descriptor. It cannot dynamically
verify external aliasing, submitted GPU writes, or opaque (alpha-255) SDR pixel semantics;
that is why construction is unsafe. Never pass a decoder's recyclable output
slot directly to this API.

Renderer and source adapter LUIDs must match. Every draw imports the resource,
acquires key 1, submits sampling, unbinds the texture, flushes, and releases key 1.
Acquisition uses the raw HRESULT and requires exactly S_OK. WAIT_TIMEOUT and
WAIT_ABANDONED are not treated as success. Acquisition has a zero timeout: an
unavailable key fails the draw without sampling or presenting the incomplete
frame. There is no automatic retry; another application draw is required.
Release is attempted even if instance
buffer setup fails after acquisition. Resource-owned legacy handles are never
closed with CloseHandle.

Releasing back to the same key permits retained-scene replay and multiple surface
instances to sample the same immutable image. The example draws the same source
twice and alternates between two independent sources on foreground-square clicks.
There is no decoder-return callback, so dropping a scene cannot call back into
and block the decoder. COM references keep source textures alive through drawing.

**Deliberate proof limitations:** zero-timeout mutex acquisition does not bound
other driver calls or GPU work. Imports/SRVs are created each draw, and each
published frame needs an independent texture. No mutable pool,
concurrent producer, caching, frame dropping, retry scheduling, throughput,
latency target, or device-loss recovery is proven. Ancestor opacity is captured in the scene and applied by the sprite shader.
The surface element’s own style opacity and rounded corners remain unsupported
(upstream surface style limitation). Errors propagate through GPUI's
existing renderer error path; app initialization success is not a render PASS.

GPUI uses `windows` 0.62 while the standalone bridge probe uses 0.61.3. Their Rust
COM wrapper types are different. The example uses GPUI's matching dependency;
production integration must deliberately align versions or define a reviewed COM
boundary, not transmute wrappers or rely on matching interface names.

## Reproduce on Windows 11 x64

Rust 1.99.0 and MSVC Windows build tools must already be available.
Run directly from XSCC's repository root; no separate checkout or patch is needed:

```powershell
# Type-check the maintained example.
./experiments/windows-texture-bridge/gpui-surface/check.ps1

# Type-check and build the actual GPUI example.
./experiments/windows-texture-bridge/gpui-surface/check.ps1 -Mode Build

# Type-check, build, and launch the example.
./experiments/windows-texture-bridge/gpui-surface/check.ps1 -Mode Run
```

The commands use `vendor/gpui/Cargo.lock`. Cargo may fetch locked build
dependencies. They do not install drivers or alter system settings.

The presentation example intentionally requires physical adapter 0 and never
falls back to WARP. GPUI independently selects its first usable adapter; mismatch
is an explicit renderer error. A future adapter-selection API is needed before
claiming multi-GPU support. The existing standalone `--warp` test remains separate
and cannot validate this draw path.

Save the commands, stdout/stderr, exit codes, Windows build, GPU/driver version,
and screenshots. Inspect the existing debugger's D3D messages where available.
Check these observable cases; do not record PASS merely because a window opened:

1. The blue/orange pattern is visible in both large and small surfaces; its edges
   are clipped to the parent. The small copy has 50% ancestor opacity and blends
   with the image behind it. The foreground magenta/yellow square stays above it.
2. Click the foreground square at least 30 times. Both patterns and the square
   change together, and the UI still responds. Repeated clicks exercise same-key
   reacquisition and duplicated-source draw; record errors or stalls.
3. Resize, cover/uncover, minimize/restore, and move across different-DPI monitors.
   Texture proportions, clipping and foreground ordering remain correct. Closing
   the window must not hang. Repeat launch/close to observe resource cleanup.

This is a manual display acceptance check, not a screenshot assertion, frame-rate
benchmark, leak certification, or device-loss test. The isolated `.github/workflows/gpui-windows-surface.yml` runs **Build**, builds
the private test binary, then runs exactly one WARP draw test on Windows 2025
with Rust 1.99.0 against the maintained `vendor/gpui` workspace. It
is restricted to pushes on `feat/management-prep` touching this proof, the vendored source, or that
workflow, uses read-only repository permissions, and never launches the GUI.
Its existence is not evidence of a successful CI run. Real presentation
acceptance needs a Windows physical-GPU session. The beta workflow also builds the vendored source, but its production decoder
remains CPU-backed. A native Windows build remains a validation gate.

## Private WARP shader-pixel test

The maintained implementation includes exactly one surface test,
`directx_renderer::windows_surface_tests::windows_surface_warp_draw_pixels`,
inside the private Windows renderer. It explicitly creates three distinct WARP
software devices on the same adapter. It does not create an HWND, swapchain,
DirectComposition target, or visible window. Missing WARP or any GPU/API failure
fails the test; it never silently skips.

The fixture calls the actual maintained `draw_surfaces()` and existing `draw_quads()`
with real shader pipelines and a standalone BGRA render target. An odd-width
9×5 source is copied once into an immutable shared publication. Its two surface
clones remain after the original publication/producer handles are dropped. The
test first holds key 1 on a competing consumer, checks that drawing fails in
under one second without changing target pixels, and releases the key before
asserting the result. It then clears and draws three times, sampling exact interior pixels (±1 rounding)
for content-mask clipping, red/green source colors, 50% opacity over blue,
full-opacity duplicate sampling, and a foreground green quad. Row-pitch-aware
staging readback is test instrumentation. Each nonblocking Map retry is bounded
by two seconds; the CI execution step has a two-minute outer timeout for driver
calls that may block independently of that retry limit.

The workflow first lists the exact test and requires exactly one matching entry;
then it executes that exact name, so a misspelled filter cannot produce a green
zero-test result. `--features test-support` is needed for upstream test-only
`PlatformWindow::render_to_image` trait declarations to agree across crates.
To inspect and run it manually from `vendor/gpui`:

```powershell
$TestName = 'directx_renderer::windows_surface_tests::windows_surface_warp_draw_pixels'
cargo +1.99.0 test --locked -p gpui_windows --features test-support --lib $TestName -- --exact --list
# Verify that exactly this named test is listed before running:
cargo +1.99.0 test --locked -p gpui_windows --features test-support --lib $TestName -- --exact --nocapture --test-threads=1
```

A future pass establishes **WARP software-rendered GPUI surface importer/shader
pixel verification**. Manual ordering of draw calls does not prove Scene batching,
element-tree ordering, DPI, presentation, real decoding, physical GPU behavior,
latency, device recovery, or production readiness. The physical-adapter example
remains unchanged and separate. This test has been cross-type-checked and linted,
not executed on Windows yet.

## Vendored-source integration checks

- The source import was verified against the applied original patch before the
  duplicate patch file was removed. The example and private WARP test now live
  directly in the maintained source tree.
- The application lockfile changed only by removing the 20 former Zed Git source
  entries. Package versions, checksums, and dependency edges are unchanged.
- Full all-platform application metadata resolves with `--offline --locked` once
  the locked dependency cache is populated; all 20 GPUI path packages resolve
  inside `vendor/gpui`.
- On Linux, the application passed `cargo check --offline --locked --all-targets`,
  `cargo test --offline --locked` (13 tests), and `cargo clippy --offline --locked
  --all-targets`, using its manifest and Rust 1.99.0. The existing seven
  non-Windows dead-code warnings remain. Application formatting and workflow
  YAML parsing also passed. These do not establish Windows execution.

## Historical pre-vendoring authoring checks

- `git apply --check` against a full clean checkout at the exact pinned commit:
  passed.
- Rust 1.99.0 `rustfmt --check` for every changed Rust file (with `skip_children`
  for the partial source inspection tree): passed.
- Workflow YAML parsing and patch whitespace checks: passed. PowerShell itself
  was not executed on Linux.
- `cargo +1.99.0 check --locked --target x86_64-pc-windows-gnu -p gpui
  --example windows_shared_surface` in a disposable patched checkout: **passed**
  with Rust 1.99.0 and the real Windows GNU target. This checks the modified
  `gpui`, `gpui_windows`, and integration example, without linking or executing it.
- Initial cross-checks exposed absent cross-C and resource compilers. Scratch-only
  Zig 0.15.2 from PyPI supplied real C/C++/archive compilation, with the Rust target
  spelling normalized for Zig. A windres-argument adapter invoked real Zig RC
  COFF generation from GPUI's actual manifest; its output was verified as x86-64
  COFF with resource sections. No manifest feature, dependency, or source check
  was disabled, and no fake build artifact was substituted. Scratch CMake 4.4.4
  and Ninja 1.13.2 were also installed from PyPI. System settings were unchanged.
- Renderer-specific Windows-target Clippy passed without warnings using
  `cargo +1.99.0 clippy --no-deps --locked --target x86_64-pc-windows-gnu
  -p gpui_windows --lib` in the same scratch compiler environment.
- The new private WARP test target passed both Windows GNU
  `cargo +1.99.0 check --locked --target x86_64-pc-windows-gnu -p gpui_windows
  --features test-support --tests` and the corresponding
  `cargo +1.99.0 clippy --no-deps ... --features test-support --tests`, without
  warnings. These commands do not execute the Windows test binary.
- Full-example Clippy with Rust 1.99.0 is blocked by an unchanged upstream
  `geometry.rs:3723` literal, `relative(1.618_034)`, now rejected by the
  deny-by-default `clippy::approx_constant` GOLDEN_RATIO lint. No lint suppression
  or unrelated geometry edit was used. This is not a full-example lint pass.
- Native Windows/MSVC build, interactive presentation acceptance, physical GPU,
  and decoder tests remain unverified. A successful cross-check does not establish
  any of those results.

## Next production milestone

After compile/display results, replace immutable per-frame allocation/import
with a bounded bridge-slot pool and explicit ownership states.
Keep the decoder frame alive until decode-to-copy completion; allow decoder reuse
only after the isolation copy has completed reading it. Keep a published bridge
slot immutable until render-to-reuse completion, including scene replay and
retained previous frames. A COM reference alone does not reserve a decoder slot.
Use nonblocking acquisition with scheduled redraw or retained-last-frame fallback,
same-adapter import caching invalidated with renderer device generation, bounded
latest-frame policy, and a tested CPU fallback. Cross-adapter fallback must be
explicit and measured. Test renderer/device loss, resolution changes, slow GPU,
window closure, and real concurrent decoding before touching production runtime.
Then use GPU tracing to verify the actual decode/copy/render timeline and absence
of consumer CPU video readback/upload. GPU copies must remain labelled GPU copies.
