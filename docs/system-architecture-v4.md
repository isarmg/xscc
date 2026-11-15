# XSCC / XSCS v4 system architecture

This is a coordinated protocol upgrade. Management v4 and native session
revision 4 reject older or future peers. There is one implementation per
boundary; no QUIC product path and no TCP-only media fallback.

## Ownership

| Boundary | Source owner | Runtime responsibility |
| --- | --- | --- |
| Enrollment and management contract | XSCS `crates/protocol` | Typed requests, immutable role/backend, policy and task messages |
| Authority and persistent state | XSCS server domain and storage | Accounts, bilateral ACLs, credentials, leases, durable receipts |
| Device HTTP / WebSocket boundary | XSCS `crates/server/src/http/clients.rs` | Bounded input parsing and management connection lifetime |
| Device executor | XSCC `crates/management` | Management stream, task execution and acknowledged journal |
| Native identity | XSCC `crates/native-management` | Protected binding files, enrollment, live permits and channel-bound proofs |
| Reliable control and native media | XSCC `crates/session-transport` | Encrypted TCP control and authenticated bounded UDP video/audio |
| Host capture and codecs | XSCC core / `libs/scrap` | Hardware selection, encoder state and GPU retirement |
| Local platform operations | XSCC `src/management/owner.rs` | Typed host display requests outside identity management |
| Frame lifetime contract | XSCC `crates/session-model` | Borrowed pixels, extent/stride and submission receipt |
| Shared presentation and input fields | XSCC `crates/native-ui` | GPUI resources, Maple fonts and controls; no capture/codec dependency |
| Desktop / mobile applications | XSCC `apps/desktop`, `apps/mobile` | Native application lifecycle and platform interaction |
| Bare-metal KVM | XSCC `one-kvm` | Existing capture/HID/WebRTC/ATX/virtual media with the same native identity and session transport |

The root core package keeps its existing library and service output names.
Applications use a single root Cargo lock. One-KVM remains separately built but
points to the same management, transport and base source directories.

## Native data flow

1. A locally declared application role/backend is checked against the central
   enrollment response before credentials are written.
2. The host listener owns its UDP receiver and issues weak registration leases
   to accepted TCP connections. Closing one listener cannot affect another or
   allow an old connection to register on a replacement listener.
3. Authenticated TCP establishes keys and binds bilateral instance permission
   proofs to the actual host identity. Current authorization is rechecked while
   the session lives; an earlier policy copy never grants continued access.
4. The connection owns the transport. Capture producers share its bounded media
   sender and epoch/frame sequence ledger. TCP control never shares a video
   socket-write queue. Low and high refresh rates use this same path.
5. Local epoch ownership is installed before its message can be observed by the
   peer. Queue drops signal a reference gap; only a valid keyframe resumes a
   dependent video sequence. Expired audio is discarded on intake.
6. Decoder buffers cross into presentation through a small frame interface;
   extracting the UI does not add a pixel copy or change the meaning of receive,
   decode or UI submission counters.
7. Connection closure revokes all producer leases and unregisters UDP before
   releasing the transport owner. Display removal still waits for existing GPU
   retirement/quarantine checks. Adding a virtual screen never promotes it to
   the primary screen automatically.

## Upgrade and retained data

Prepare all v4 applications and the v4 server as one upgrade cohort. Do not
replace only the public server while installed applications still use v2/v3.
Stop XSCS, make an independent backup, then run the explicit offline
`scripts/migrate-management-v4.py` tool in the XSCS repository following its
usage and release documentation. It checks the schema and exclusive state lock,
creates a private backup and clears only obsolete live observations.

Account IDs, device IDs, installation bindings, credential material, ACLs and
durable task payloads/fingerprints/receipts are retained. The task record format
`xscs-task/2` and encrypted handshake format `KX1` remain single canonical
formats, separate from the management and application session revisions.
Historical source archives and licenses do not participate in builds.

## Validation and changed runtime paths

The architecture check in `.github/scripts/check-system-architecture.py` guards
dependency direction and source ownership. Existing native product workflows
compile desktop host/controller, Android, iOS, Linux, macOS and bare-metal KVM.
The extracted transport tests use real loopback TCP/UDP sockets, including
fragmented video, audio scheduling, control isolation, epoch cancellation,
listener replacement and revocation. Management tests exercise protected
storage, enrollment, policy expiry, execution receipts and KVM recovery.

The required runtime changes are enrollment and capability validation, the
native session revision check, explicit listener ownership, unified media
producer queues, typed local display IPC and shared UI resource paths.
XSCS device HTTP/WebSocket routing and the explicit offline observation upgrade
change with the shared protocol. Administrative authentication, CSRF/origin
checks, durable task semantics, GPU retirement guards and hardware fallbacks
remain owned by their existing components.

Build and unit-test results do not establish physical-device frame rates or
virtual-display stability. This refactor task does not install applications,
run a production database migration, deploy XSCS or create a virtual screen.
