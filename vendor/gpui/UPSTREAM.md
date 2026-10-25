# GPUI source provenance

This directory contains the XSCC-maintained GPUI implementation. It is ordinary
repository source, not a submodule, generated download, or build-time patch.

Initial source: [zed-industries/zed](https://github.com/zed-industries/zed), commit
[`28d0a3547f16b9609b301b48ab363bfca649c864`](https://github.com/zed-industries/zed/tree/28d0a3547f16b9609b301b48ab363bfca649c864).
The machine-readable inventory is [upstream.json](upstream.json).

## Scope and licensing

The initial import contained 28 workspace members. The maintained workspace now
contains 20. It keeps native platform backends, required build dependencies,
core tests, benchmarks, shaders, resources, and their font licenses. iOS remains
available for future client work; the Web backend has been removed.

Original Apache-2.0 attribution and the root Apache/GPL license texts are
preserved. Font licenses remain beside their fonts. License symlinks were
materialized as ordinary files.

The Zed editor application, unrelated crates, Git metadata, upstream workflows,
and agent instructions are not included. The root Cargo manifest contains only
the required dependency declarations, inherited package fields and lints.
Retained external dependencies keep their original versions and Git revisions. The upstream workspace's patches and build profiles are not imported:
Cargo already ignored those root patches/profiles when XSCC consumed GPUI as a
Git dependency. XSCC's consuming workspace lockfile governs application builds.
The standalone lockfile is seeded from the pinned upstream lock, pruned to this
closure, with the five formerly patched dependencies resolved normally through
their unchanged manifest constraints.

## Initial XSCC changes

The existing reviewed Windows immutable shared-surface experiment is incorporated
into the source (the former `0001-windows-immutable-surface.patch`), including its
example and WARP regression tests. The initial import matched that patch exactly.
The changed paths, including subsequent maintenance, are listed in `upstream.json`.

`WindowsSurface` remains experimental: it requires a separately owned immutable
opaque SDR BGRA D3D11 texture with keyed-mutex synchronization. The renderer
imports it on each draw; it is not a production decoder-slot lease mechanism.
The application must explicitly construct this surface to use the new path.
Vendoring does not switch XSCC's existing CPU presentation path or enable
shared-texture presentation for production sessions.

Further fixes belong directly in this source tree, with ordinary XSCC review and
testing. Any future upstream import must be explicit and update this provenance;
builds must not silently advance the upstream revision.

## Rust 1.99 maintenance

The GPUI crate now declares Rust 1.99 as its minimum toolchain. Its `phi()` helper
uses `std::f32::consts::GOLDEN_RATIO` instead of an approximate literal, satisfying
Rust 1.99 Clippy without a lint suppression. Both values have identical `f32`
bits (`0x3fcf1bbd`), so the returned layout fraction is unchanged. Dependency
versions and the pinned upstream revision are unchanged.

## Windows surface acquisition

Shared-surface drawing now uses zero-timeout mutex acquisition. A busy surface
fails the draw without presenting an incomplete frame; another application draw
is required to retry. The WARP fixture includes consumer contention coverage.
This changes only the experimental surface path, not application video sessions.

## Native workspace simplification

The application uses `gpui_platform` as its single GPUI facade, importing it as
`gpui`. Linux defaults and facade features enable Wayland without X11.
The direct Linux backend still contains its optional X11 implementation.

`SharedString` is an internal `gpui` module, retaining the `gpui::SharedString`
public API and string serialization. The former standalone crate is removed.

The Web backend, two network-image demos, unused optional GitHub downloader,
and six associated tooling packages are removed. The three style tests formerly
run through the performance harness now use ordinary `#[test]` attributes with
the same assertions. Core tests, benchmark support, and Windows WARP tests remain.

## Video image ownership and ordered uploads

`RenderImage::from_bgra` retains an owned, packed pixel buffer until the final
image reference is released. `Window::replace_image` explicitly invalidates the
previous image, like `drop_image`, and reuses a same-sized atlas tile only when
the backend provides an ordered upload. DirectX uses its existing
`UpdateSubresource` upload; WGPU snapshots upload bytes and submits them on its
existing render queue. Metal retains the fresh-image path. Size changes,
device loss, unsupported backends and failed uploads retain the fresh-image
fallback. No decoder texture is imported or reused through this API.

Core image/atlas tests cover retained ownership, replacement failure, changed
size and device reset. WGPU tests cover upload storage reuse and pixel swizzling.
Physical GPU throughput and actual screen presentation require device testing.

## Application-painted pointers

`CursorStyle::Hidden` clears the native pointer on Windows, registers a transparent
NSCursor on macOS, and uses `wl_pointer.set_cursor(None)` on Wayland. Wayland skips
cursor-shape-v1 for this style, including pointer entry and restore-after-typing.
Regular controls restore their existing cursor styles. The optional, non-product
X11 backend explicitly uses its arrow fallback. Mobile backends retain their
existing system pointer behavior.
