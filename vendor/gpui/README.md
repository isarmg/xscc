# XSCC GPUI

GPUI is maintained here as part of XSCC. See [UPSTREAM.md](UPSTREAM.md) for source provenance, licenses, and the initial local changes.

The workspace contains 20 crates. Applications use `gpui_platform` as the single
public facade; XSCC imports it under the dependency name `gpui`. Linux defaults
to Wayland.

## Toolchain and validation

The GPUI crate requires Rust 1.99 or newer.

Run Windows GNU cross-checks from this directory with a configured cross compiler:

```sh
cargo +1.99.0 clippy --locked --target x86_64-pc-windows-gnu -p gpui --example windows_shared_surface --no-deps
cargo +1.99.0 check --locked --target x86_64-pc-windows-gnu -p gpui_windows --features test-support --tests
cargo +1.99.0 clippy --locked --target x86_64-pc-windows-gnu -p gpui_windows --features test-support --tests --no-deps
```

For Windows WARP execution and graphics-device testing, see
[the surface validation instructions](../../experiments/windows-texture-bridge/gpui-surface/README.md).
