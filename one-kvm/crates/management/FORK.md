# Management source ownership

This directory contains the One-KVM management application and snapshots of the
source-owned XSCC/XSCS management components. Original package names and licenses
are retained. `SOURCE.json` records each exact original file hash and source Git
revision, including whether that source directory had uncommitted changes when
imported. `sources/*.original.tar.gz` preserves the complete original component
inputs, including those uncommitted changes. Archive hashes and every product
addition/modification/removal are recorded alongside the original inventory.

- `vendor/client`: XSCC embedded executor, bounded WSS transport, durable receipt
  interfaces and Apache-2.0 license. Its protocol dependency points to the local
  snapshot; diagnostic test literals accept the backend-specific optional field.
- `vendor/native`: XSCC enrollment, private platform storage, journal and native
  session proofs under AGPL-3.0-only (original `LICENCE` copied alongside it).
  The mobile GPUI-only integration fixture is omitted because it references an
  external UI source file outside this product. Product changes add an expected-role constructor, One-KVM backend capabilities,
  Linux ARM64 and ARMv7, a role/backend pairing check before credential creation, a new-profile
  preflight before allocating persistent browser identities, a cheap
  connection-state projection and an address-independent browser permission
  projection. Existing native proof domains, canonical transcript serialization,
  role directions, epoch derivation and key handling stay byte-compatible.
- `vendor/protocol`: XSCS v2 types with `one_kvm`, Linux ARM64 / ARMv7 and optional
  `one_kvm_version`; the standalone manifest expands workspace dependency values.

The top-level application stores browser controller credentials only in private
server-side directories. An opaque HttpOnly cookie identifies an in-memory,
limited-duration session. CSRF and Origin checks belong to the HTTP boundary;
`AccessGrant` supplies constant-time CSRF comparison and current bilateral ACL
checks. The host must close streams when its permit is cancelled.
Observed expiry or loss of either side's permit permanently invalidates that
browser session. Background cleanup also observes idle sessions, and restoring
an ACL requires explicit login to obtain a new cookie.
Browser login reports malformed input as invalid and central code, role or backend
rejections as denied. Service, protocol and protected-state failures retain their
source errors; remote unbind failures remain retryable failures.

Controller identity recovery reuses the SHA-256 namespace of the canonical
XSCS origin plus authorization code. A restart invalidates browser cookies but
preserves the controller installation and saved credential for explicit re-login.
Logout invalidates the old cookie, CSRF and permit immediately, durably marks
pending unbinding, and stops the controller connection. Failed unbinding is
retried, including after restart. Re-entering a still-valid code explicitly first
finishes pending unbinding, then obtains new credentials and a fresh cookie while
retaining the installation UUID. The central pairing response decides whether a
code remains usable; this product adds no permanent logout tombstone.

Expiry of only the browser cookie closes its controller connection while retaining
the protected identity for explicit re-login. Foreground re-enrollment and each
background unbind share the existing browser gate so a stale retry cannot revoke
fresh credentials; cleanup releases the gate between owners and cancels requests
during shutdown. At most eight live browser sessions and 64 retained identity
owners/directories are allowed. Reusing an owner also prevents an old invalid
permit's reference from blocking immediate re-login on its private-directory lock.


Validated on Linux x86_64 with Rust 1.99.0: the complete independent workspace
passed 31 tests, including two loopback HTTP/WSS authorization/recovery scenarios,
transport replacement and lease invalidation, private-store failure cases, and
unchanged native transcript proofs. Workspace/all-target Clippy with `-D warnings`
and Rustfmt checks passed. ARM variants are source/type checked here; no ARM
hardware or cross-target runtime acceptance is implied.
