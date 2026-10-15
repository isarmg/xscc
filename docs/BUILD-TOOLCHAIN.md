# Build toolchain

## Versions

- Rust and Cargo: **1.99.0**. `rust-toolchain.toml` selects Rust, rustfmt and Clippy; the crate edition remains 2021.
- Flutter: **3.47.7**, with Dart **3.13.5**. Application builds and bridge generation use the same SDK.
- CMake: **4.4.4** where the build pins CMake explicitly.
- Windows: LLVM/Clang **23.1.3**, installed from the official architecture-specific MSI.
- Linux build containers: Ubuntu **26.04**, Clang/libclang/LLVM **21.1.6** from its stable package repositories.
- Apple platforms: Xcode **26** or newer, with deployment target **26.0**.
- Android: JDK **27**, Gradle **9.8.1**, AGP **9.4.0**, Kotlin **2.4.21**, NDK **r30 / 30.0.16248370**, cargo-ndk **4.1.2**.

Android builds target Java/Kotlin bytecode 17 and Android API 29. The AGP 9 legacy DSL/Kotlin switches remain enabled for the current Flutter plugins. The latest Gradle/AGP combination is newer than Kotlin's fully tested Gradle 9.7 / AGP 9.3.1 range; the Android CI build checks their integration.

Keep the Rust version in the toolchain file, root manifest and workflows synchronized. Keep Flutter bridge generation and application builds on the same Flutter version.

Official version sources:

- [Rust stable manifest](https://static.rust-lang.org/dist/channel-rust-stable.toml)
- [Flutter stable manifest](https://storage.googleapis.com/flutter_infra_release/releases/releases_linux.json)
- [LLVM 23.1.3](https://github.com/llvm/llvm-project/releases/tag/llvmorg-23.1.3)
- [CMake releases](https://cmake.org/download/)
- [Ubuntu 26.04](https://releases.ubuntu.com/26.04/)
- [Android NDK](https://developer.android.com/ndk/downloads)
- [Kotlin compatibility](https://kotlinlang.org/docs/gradle-configure-project.html)
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

## Checks

Prepare the platform's native dependencies as shown in `.github/workflows/flutter-build.yml`. CI generates the native Flutter bridge using its pinned code generator.

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
