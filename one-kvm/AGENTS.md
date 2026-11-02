# Managed XSCC KVM host

One-KVM is the independently built bare-metal Web KVM module inside XSCC.
Each configured instance owns its identity, authorization, devices and data. XSCS is the authority
for instance enrollment, controller relationships, credential rotation and leases.
The browser UI uses the same repository-owned xcss-web Rust/WASM fork as XSCS;
Axum owns HTTP, WebSocket and media authorization.

* Deploy only native binaries on physical Linux or Windows hosts. Runtime
  container images and container publishing are not supported. Cross-compilation
  containers under `build/cross/` are build tools only.
* Preserve capture, codecs, HID, audio, ATX and virtual-media implementations.
* Scope browser routes, cookies, WebSockets and hardware configuration to the
  selected instance; never reuse another instance's authorization or devices.
* Require current instance authorization before accepting control or media,
  and close existing channels when authorization expires or is revoked.
* Bind native XSCC session proofs to the actual encrypted transport host key.
  Do not accept plaintext fallback or reuse a local password as xscs identity.
* Keep manager credentials and pair keys in protected server storage; the
  browser receives only an opaque HttpOnly cookie and a CSRF token.
* Device enrollment is an explicit local CLI operation. An unauthenticated
  network request must never replace the device's manager or binding.
* Parse external data at its boundary and retain validated types. Propagate
  database and I/O errors; copy only data required by the current operation.
* Retain original licenses, notices and source records for imported code.
* Product builds use Cargo, wasm-bindgen and `tools/web-build`; keep retired
  Vue/Vite source outside the product entry and avoid production fallbacks.

Run affected tests, formatting and Clippy. Verify real WASM resources and native
protocol compatibility separately from physical capture/streaming acceptance.
