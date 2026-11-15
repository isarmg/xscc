# Management source ownership

One-KVM uses the repository's canonical `crates/management` and
`crates/native-management` packages. Both native applications and this host
consume the exact same XSCS v4 protocol revision through those packages.
There are no compiled copies of these libraries under this directory.

`SOURCE.json`, `sources/*.original.tar.gz` and `licenses/` preserve the original
snapshot revisions, input hashes and licenses. They describe historical inputs,
not dependencies or a backward-compatible implementation.

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


The v4 switch was checked with the shared native management unit tests and all
three One-KVM integration tests (authorization, isolated identity, logout and
restart recovery). Full product builds are recorded by the native One-KVM CI.
Hardware capture and input acceptance require physical-device testing.
