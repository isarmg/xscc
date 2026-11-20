# Audio device I/O

Low-level library for audio input and output in pure Rust.

This library currently supports the following:

- Enumerate supported audio hosts.
- Enumerate all available audio devices.
- Get the current default input and output devices.
- Enumerate known supported input and output stream formats for a device.
- Get the current default input and output stream formats for a device.
- Build and run input and output PCM streams on a chosen device with a given stream format.

Currently, supported hosts include:

- Linux (via ALSA or JACK)
- Windows (via WASAPI)
- macOS (via CoreAudio)
- iOS (via CoreAudio)
- Android (via Oboe)
- Emscripten

Note that on Linux, the ALSA development files are required. These are provided
as part of the `libasound2-dev` package on Debian and Ubuntu distributions and
`alsa-lib-devel` on Fedora.

## Compiling for Web Assembly

If you are interested in using CPAL with WASM, please see [this guide](https://github.com/RustAudio/cpal/wiki/Setting-up-a-new-CPAL-WASM-project) in our Wiki which walks through setting up a new project from scratch.

## Feature flags for audio backends

Some audio backends are optional and will only be compiled with a [feature flag](https://doc.rust-lang.org/cargo/reference/features.html).

- JACK (on Linux): `jack`
- ScreenCaptureKit (on macOS): `screencapturekit`

Oboe can either use a shared or static runtime. The static runtime is used by default, but activating the
`oboe-shared-stdcxx` feature makes it use the shared runtime, which requires `libc++_shared.so` from the Android NDK to
be present during execution.
