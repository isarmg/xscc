# Native library sources

The root vcpkg manifest resolves libraries and build recipes from this directory.
Each source archive belongs to its port, so changing the source invalidates that
port's build cache. Local patches apply at extraction, and each port installs the
library's license. Port names follow the native package and library identifiers
used by vcpkg and the platform linkers. The desktop hardware video profile is
a default manifest feature. Software-only builds can omit it with
`vcpkg install --x-no-default-features`.

| Responsibility | Source |
| --- | --- |
| AV1 | AOM 3.14.1, revision `03087864cf4bea6abb0d28f95cf7843511413d8f`, with the existing AOM 3.9.1 compatibility option at revision `8ad484f8a18ed1853c094e7d3a4e023b2a92df28` |
| VP8 and VP9 | libvpx 1.15.2 |
| Voice codec | Opus 1.5.2 |
| Pixel conversion | libyuv revision `0faf8dd0e004520a61a603a4d2996d5ecc80dc3f` |
| JPEG decoding for pixel conversion | libjpeg-turbo 3.2.0 |
| Hardware video, H.264/HEVC decoding, and MP4 muxing | FFmpeg 7.1.1 and the local codec patches |
| Graphics driver interfaces | AMD AMF 1.4.35, NVIDIA codec headers 12.1.14.0, and Intel MFX dispatch 1.35.1 |
| CPU feature detection for Android | cpu_features 0.11.0 |
| Cryptography for Windows ARM64 | libsodium 1.0.22 |
| Native package metadata | pkgconf 3.0.3 |

The archived sources are maintained in the repository. The native build still
uses host tools and the target platform SDK. Tool provisioning is handled by the
CI runner and vcpkg's host-tool discovery. The local VCPKG_ROOT build scripts are
under `res/native-package-build`.
