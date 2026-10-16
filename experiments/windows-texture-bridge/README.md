# Windows D3D11 texture bridge probe

Standalone, opt-in driver interoperability experiment. It does not depend on or
modify GPUI, the production session runtime, Flutter, or beta packaging CI. No
window is created.

## What this tests

Two distinct D3D11 devices on the same adapter transfer synthetic BGRA pixels:

1. CPU-generated frame → producer-only DEFAULT texture (`UpdateSubresource`).
2. Producer texture → bridge-owned shared texture (`CopyResource`).
3. Keyed-mutex handoff → consumer device imports the texture.
4. Consumer texture → CPU-readable staging texture → exact BGRA comparison.

Step 2 is the proposed **one GPU-to-GPU isolation copy**. Steps 1 and 4 are
intentional test instrumentation, not a proposed production rendering path.
This is **not zero-copy**, hardware decoding, GPUI rendering, presentation, or a
latency benchmark. WARP executes the graphics work in software.

All operations are deliberately serialized on one thread; each device has its
own immediate context. This tests device/resource interoperability and key
handoff, **not concurrent producer/consumer correctness** or a frame-slot pool.
Each producer handoff is delayed by 2 ms before consumption.

The probe checks:

- Two distinct devices and equal adapter LUIDs; hardware mode never falls back
  to WARP. It selects DXGI adapter 0 and fails if that adapter is software.
- Shared BGRA8, one mip, one array slice, non-MSAA, DEFAULT resource descriptors.
- An unavailable mutex key returns exactly `WAIT_TIMEOUT`, not success.
- Every acquisition must return exactly `S_OK`. Positive timeout/abandonment
  statuses are rejected through the raw COM HRESULT, rather than windows-rs's
  success/failure conversion.
- Thirty changing frames at each of 64×48, 257×129, then 64×48. Each size phase
  recreates all textures; odd width exercises row-pitch handling.
- Exact per-row pixels, including alpha, and device-removal status.
- Mutex acquisitions and nonblocking staging-map retries have a 2-second limit.
  These limits do not guarantee recovery from a hung driver/API call.

COM objects own their resources for the complete phase. The legacy shared handle
returned by `IDXGIResource::GetSharedHandle` is resource-owned and must **not** be
passed to `CloseHandle`. The creating resource stays alive through consumer use.
There is no exported raw pointer, resource cache, or production lifecycle change.

## Run on Windows 11 x64

Prerequisites: an existing Rust toolchain and normal Windows build tools. This
probe does not install a driver, debug layer, or change system settings.
Run from the repository root in PowerShell:

```powershell
# Basic API and pixel verification; does NOT validate a physical GPU.
cargo run --release --locked --manifest-path experiments/windows-texture-bridge/Cargo.toml -- --warp

# Physical GPU/driver verification on adapter 0; no software fallback.
cargo run --release --locked --manifest-path experiments/windows-texture-bridge/Cargo.toml -- --hardware

# Optional: only if the D3D debug layer is already installed.
cargo run --release --locked --manifest-path experiments/windows-texture-bridge/Cargo.toml -- --hardware --debug
```

Exactly one of `--warp` or `--hardware` is required. Unsupported sharing, adapter
mismatch, API failures, timeout, or pixel mismatch exits nonzero. The probe stops
instead of recreating devices or silently using a different adapter. `--debug`
fails if its prerequisites are absent; it does not install them. Debug messages
can be inspected using an existing debugger; stdout alone does not certify an
empty debug-layer log.

A successful run prints the mode, adapter and LUID, then three `PASS size=...`
lines and a final diagnostic-only `PASS`. Save the command, complete stdout/stderr,
exit code, Windows build, and GPU/driver version with any test report. A WARP pass
and a hardware pass must be reported separately. No Windows runtime pass has yet
been established by this experiment's authoring checks.

## Isolated Windows CI

`.github/workflows/windows-texture-bridge.yml` runs only for pushes to
`feat/management-prep` changing this experiment or that workflow. It uses the
existing pinned Rust 1.99.0 toolchain on `windows-2025`, with read-only repository
permissions. It formats, lints, tests, builds, then runs only `--warp`; the run step
has a two-minute outer timeout. The GPUI beta workflow is unchanged. No hardware
mode, debug layer, codec dependency, or system-setting installation is involved.

A green run provides actual Windows WARP API evidence, not physical GPU or GPUI
display evidence. Authoring/cross-check success does not imply that this CI run
has occurred or passed; inspect its final run log separately.

## Authoring checks (Linux)

```sh
cargo test --locked --manifest-path experiments/windows-texture-bridge/Cargo.toml
cargo check --locked --target x86_64-pc-windows-gnu --manifest-path experiments/windows-texture-bridge/Cargo.toml
```

The cross-check requires that Windows Rust target to be installed. It type-checks
the Windows implementation but does not link or execute D3D11. The two portable
unit tests cover BGRA patterns, padded rows, and invalid buffers only. Running
the binary on Linux intentionally fails with “no GPU test was performed”.

## Gate before the next experiment

1. Obtain WARP and physical-hardware results above. A failure blocks claiming
   working interop for that device; inspect the actual HRESULT/debug output.
2. Separately test a GPUI Windows surface patch with a synthetic shared texture,
   including clipping, z-order, DPI and window/device lifecycle. This crate does
   not establish any of those properties.
3. Only then connect H.264 hardware output using one GPU isolation copy. A raw
   decoder texture reference must not outlive exclusive access to its mutable
   output slot. Same-LUID matching alone does not solve ownership/synchronization.
4. Test actual cross-thread use, bounded slots/drop policy, device loss, resize,
   and CPU fallback before touching the production frame path. Use GPU tracing
   to verify absence of video CPU readback/upload in that different experiment.

## API references

- [Resource flags and Windows 8+ WARP shared-resource support](https://learn.microsoft.com/en-us/windows/win32/api/d3d11/ne-d3d11-d3d11_resource_misc_flag)
- [AcquireSync exact return statuses](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/nf-dxgi-idxgikeyedmutex-acquiresync)
- [OpenSharedResource and producer Flush](https://learn.microsoft.com/en-us/windows/win32/api/d3d11/nf-d3d11-id3d11device-opensharedresource)
- [Legacy shared-handle lifetime](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/nf-dxgi-idxgiresource-getsharedhandle)

The legacy handle API keeps this small probe close to the existing decoder's
sharing route. A future production bridge can evaluate NT handles independently;
Microsoft recommends `SHARED_NTHANDLE | SHARED_KEYEDMUTEX` for modern sharing.
