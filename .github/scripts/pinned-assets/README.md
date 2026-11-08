# Pinned build assets

The `sha512-5d2ffc4e...` file is the exact missing-`cstdint` patch already
pinned by `res/vcpkg/mfx-dispatch/portfile.cmake`. Its complete SHA512 is its
filename suffix; the downloader verifies that same mandatory caller pin.

Upstream source: https://github.com/lu-zero/mfx_dispatch/commit/d6241243f85a0d947bdfe813006686a930edef24.patch?full_index=1

The original endpoint repeatedly returned HTTP 504. The official GitHub commit
patch API returned the same patch with abbreviated blob IDs. Expanding its one
`index` line using the official commit and parent blob IDs reproduced the exact
2816 bytes matching the existing SHA512. No hunk, version or checksum changed.

Upstream license: BSD-3-Clause. See `LICENSE.mfx-dispatch` for the original notice
from the pinned mfx-dispatch 1.35.1 source.

These files are immutable build inputs, not a writable download cache.

License source: https://raw.githubusercontent.com/lu-zero/mfx_dispatch/1.35.1/LICENSE

License SHA256: `f03bff2bd2f916104d12d50caeaab41875086943ea8b88983eafa5d84890593c`.
