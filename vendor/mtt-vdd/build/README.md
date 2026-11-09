# Build-only reproduction

Run `python3 vendor/mtt-vdd/verify-source.py` before compiling. It validates every
original Git blob/hash, the exact maintained diff, production stop-check placement,
terminate-first pending-event ordering, and the unchanged worker destructor.

The deterministic mock uses the production stop-decision header. A producer that
always returns successful frames requests stop after 128 iterations; the next
iteration must stop without needing an E_PENDING branch. The other groups cover
simultaneously ready events and invalid wait results. These tests create no
Windows/D3D/WDF objects, devices or monitor paths.

Linux CPU reproduction:

```sh
c++ -std=c++17 -Wall -Wextra -Werror -UNDEBUG vendor/mtt-vdd/tests/SwapChainStopTests.cpp -o /tmp/mtt-stop-tests
/tmp/mtt-stop-tests
```

The same source has also passed native MSVC 14.42 with /W4 /WX /UNDEBUG locally.
No complete driver build is claimed by that CPU result. The manual-only
`.github/workflows/mtt-vdd-maintenance.yml` prepares an unsigned x64 UMDF package
using fixed NuGet/SDK/WDK inputs, runs the CPU tests, API validation, INF verification
and catalog generation, and records source/toolchain hashes. It contains no
installation, runtime display, certificate-import, BCD or Secure Boot step.

A changed DLL cannot reuse the upstream catalog: catalog membership hashes change.
The artifact remains UNSIGNED and is not a replacement product installer. A valid
new signing path and the target machine's actual installation/load policy must be
verified separately; this project does not assume that UMDF requires either the
same policy as a kernel driver or no signing checks at all. The independent PR7
Root\XsccIdd disposable-VM runtime harness is not an installer for this Root\MttVDD
candidate and must not be reused against a real machine without an explicit review.

Primary policy references:
- https://learn.microsoft.com/en-us/windows-hardware/drivers/install/test-signing-a-driver-package-s-catalog-file
- https://learn.microsoft.com/en-us/windows-hardware/drivers/install/verifying-the-signature-of-a-test-signed-catalog-file
- https://learn.microsoft.com/en-us/windows-hardware/drivers/dashboard/code-signing-reqs
- https://learn.microsoft.com/en-us/windows-hardware/drivers/dashboard/code-signing-attestation

No source fix bounds a Windows/GPU/DDI call that itself stops returning. The
cancellation change removes frame/event starvation while retaining actual worker
ownership until join; it is not proof of physical-GPU safety or high-refresh FPS.
