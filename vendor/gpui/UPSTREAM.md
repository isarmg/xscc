# GPUI source provenance

This directory contains the XSCC-maintained GPUI implementation. It is ordinary
repository source, not a submodule, generated download, or build-time patch.

Initial source: [zed-industries/zed](https://github.com/zed-industries/zed), commit
[`28d0a3547f16b9609b301b48ab363bfca649c864`](https://github.com/zed-industries/zed/tree/28d0a3547f16b9609b301b48ab363bfca649c864).
The machine-readable inventory is [upstream.json](upstream.json).

## Scope and licensing

The 28 workspace members are the recursive Cargo path-dependency closure of
`gpui` and `gpui_platform`, including development/build dependencies and
platform-specific dependencies. The iOS and web backends remain because the
platform facade references them during Cargo resolution; their inclusion does
not enable a web application or expand XSCC's supported-platform policy.

Crate sources, examples, benchmarks, tests, shaders, Windows resources, and the
fonts referenced by tests/examples are retained. Font licenses accompany the
font files. All 28 workspace crates declare Apache-2.0; original crate license
files and root Apache/GPL license texts are preserved. Existing attribution and
copyright notices remain intact. License symlinks were materialized as ordinary
files. No symlink points outside this repository.

The Zed editor application, unrelated crates, Git metadata, upstream workflows,
and agent instructions are not included. The root Cargo manifest contains only
the required dependency declarations, inherited package fields and lints.
Dependency declarations retain their original versions, Git revisions and
features. The upstream workspace's patches and build profiles are not imported:
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
