# Hardware video codecs

This library was imported from `rustdesk-org/hwcodec` revision
`778df1f99597722473b29443bac22ae6c23946fe` and `rustdesk-org/externals`
revision `8903740a1f47884906a6e347ad3d8d56304d9771`. The import has no
standalone license declaration. The source files and SDK headers carry
their original notices. The repository maintains the codec implementation,
including owned FFmpeg inputs and output planes, GPU retirement, and
Direct-MFX bitrate control.

The library is organized by media buffer ownership, container muxing and
GPU driver. The Windows-only SDK adapters share the product's C++/Rust
bindings. FFmpeg and the Intel dispatch library are built by the native
package recipes in `res/vcpkg`.
