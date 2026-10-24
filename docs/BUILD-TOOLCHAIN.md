# Build toolchain

## Versions

- Rust and Cargo: **1.99.0**. `rust-toolchain.toml` selects Rust, rustfmt and Clippy. The reusable root core uses edition 2021; the GPUI application workspaces use edition 2024.
- CMake: **4.4.4** where the build pins CMake explicitly.
- Windows: LLVM/Clang **23.1.3**, installed from the official architecture-specific MSI.
- Linux build containers: Ubuntu **26.04**, Clang/libclang/LLVM **21.1.6** from its stable package repositories.
- Apple platforms: Xcode **26** or newer, with deployment target **26.0**.
- Android: JDK **27**, Gradle **9.8.1**, AGP **9.4.0**, NDK **r30 / 30.0.16248370**, cargo-ndk **4.1.2**. The GPUI package uses GameActivity and the native Rust library.

Android GPUI builds target Java bytecode 17 and minimum API 29. The dedicated native workflow validates the actual APK, JNI interfaces, input privacy checks and 16 KB alignment.

Keep the Rust version in the toolchain file, root manifest and GPUI workflows synchronized. GPUI applications call the Rust core directly; Flutter/Dart SDKs and generated Flutter bridges are outside the product build path.

Official version sources:

- [Rust stable manifest](https://static.rust-lang.org/dist/channel-rust-stable.toml)
- [LLVM 23.1.3](https://github.com/llvm/llvm-project/releases/tag/llvmorg-23.1.3)
- [CMake releases](https://cmake.org/download/)
- [Ubuntu 26.04](https://releases.ubuntu.com/26.04/)
- [Android NDK](https://developer.android.com/ndk/downloads)
- [Gradle Java compatibility](https://docs.gradle.org/current/userguide/compatibility.html)

## Build targets

Applications are deployed natively on physical devices. Docker, Compose and Kubernetes deployment are not supported. Containers in the CI workflows and the AppImage helper are build/packaging environments only; they do not run the deployed application.

- Windows 11, native x86_64. Startup and installation require workstation build 22000 or newer; Windows 11 uses the Windows 10/11 compatibility GUID.
- Linux x86_64, Wayland. Standard Portal/PipeWire and the optional unattended DRM variant share the same architecture.
- macOS 26, Apple Silicon.
- iOS 26 and iPadOS 26, ARM64 devices, client only. Debug simulator builds use the separate `aarch64-apple-ios-sim` library.
- Android ARM64, API 29 or newer, client only.

Browser clients and other architectures are not build targets. CPU/GPU suitability is determined by the capture, codec and rendering path, not hardware release dates.

Linux Deb packaging measures GLIBC requirements from all staged ELF files and records the highest requirement in the libc6 dependency. RPM uses its automatic ELF dependencies. AppImage recipes use Ubuntu 26.04 libraries.

## Repository-owned support

The controller, host and mobile management applications build without sibling
`xcss` or `xcsc` checkouts, Git dependencies or package downloads. The contour
implementation and palettes used by both GPUI applications live in
`experiments/gpui-validation/src/theme.rs`; its source URL records attribution.
The `xscs-protocol` dependency remains pinned to the server protocol crate and
does not import the server support library. Native management CI checks this
dependency boundary alongside the workspace layout.

For local/private-network testing, enter `http://127.0.0.1:18104`,
`http://localhost:18104`, or an explicit private IP such as
`http://192.168.31.245:18104` in the instance binding form. HTTP accepts only
loopback, RFC1918 IPv4 literals and IPv6 ULA literals; public IPs and domain
names require HTTPS. The same origin is used for enrollment, protected-storage
namespace and subsequent WS (or HTTPS/WSS) connections. User information,
non-root paths, queries and fragments remain invalid. These tests use the same
instance credentials and authorization as other managed connections;
administrator passwords are not instance binding passwords.

## Checks

Prepare native dependencies using `.github/workflows/gpui-windows-beta.yml`, `gpui-unix-beta.yml`, `gpui-android-smoke.yml` or `gpui-ios-management.yml` for the target platform. Windows binary codec caches are scoped to the hosted toolchain image to avoid reusing incompatible MSVC/CRT artifacts.

Desktop applications use `experiments/gpui-validation/Cargo.toml`: `live-session` for Controller and `native-host` for Host. Mobile applications use `experiments/gpui-mobile-management/Cargo.toml` with `mobile-management,mobile-session`. A management-only or rendering probe build is a diagnostic variant, not the complete controller.

From the repository root:

```sh
rustc --version
cargo --version
cargo metadata --locked --format-version 1 > /dev/null
cargo test --locked --workspace
```

The media library also has an independent codec-only build:

```sh
cargo check --locked -p scrap --lib --no-default-features
```

Capture hosts enable `scrap/wayland` on Linux; this also enables `scrap/capture`. A codec-only consumer does not enable capture. Run the codec-only check separately from a workspace-wide build so Cargo does not merge capture features from the host.
