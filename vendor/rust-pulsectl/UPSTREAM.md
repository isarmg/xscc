# Vendored rust-pulsectl

- Repository: https://github.com/rustdesk-org/pulsectl
- Exact source revision: `aa34dde499aa912a3abc5289cc0b547bd07dd6e2`
- Upstream package: `rust-pulsectl` 0.2.12; Rust library name `pulsectl`
- Original author: Kristopher Ruzic
- License: GPL-3.0-or-later, as declared by upstream `GPL-3.0+`; upstream notice is preserved in `LICENSE.md`.

Copied without modification: `Cargo.toml`, `README.md`, `LICENSE.md`, `examples/`,
and the remaining source files. Upstream Git metadata and Cargo.lock are omitted;
the consuming application lockfiles select dependency versions.

The only production changes are modern libpulse-binding API names:

1. `pulse::context::flags::NOFLAGS` → `pulse::context::FlagSet::NOFLAGS`.
2. `pulse::volume::VOLUME_NORM` → `pulse::volume::Volume::NORMAL`.

One unit test pins the existing nominal-volume conversion to PulseAudio's normal
level (65536). No runtime behavior, connection flags, volume scaling, dependency
version requirement, or error handling is otherwise changed. This allows the
application's current libpulse-binding 2.30.1 to compile without downgrading it.

The rustdesk-org repository still pointed to this exact revision when checked on
2026-10-17. The original krruzic/pulsectl repository's HEAD
`b521d5c554688f86fe44c0dc15cf2487498186ce` was version 0.2.6 and still used both old
names; it was not substituted for the application's pinned source.
